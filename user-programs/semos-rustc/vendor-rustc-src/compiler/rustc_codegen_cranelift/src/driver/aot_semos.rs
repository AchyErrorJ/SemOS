//! SemOS-native AOT driver — Phase 5b iter 4 real codegen.
//!
//! Replaces upstream `driver::aot` (host-only: heavy `std::fs` /
//! `std::process::Command` / `std::thread::Builder` / jobserver use
//! plus the dropped `back::link` subsystem) with a single-task,
//! single-CGU driver that:
//!
//! 1. Builds one `ObjectModule` via the existing `make_module`-style
//!    plumbing (cranelift-object + cg_clif's `build_isa`).
//! 2. Walks `tcx.collect_and_partition_mono_items().codegen_units`
//!    and lowers every `MonoItem::Fn` via cg_clif's
//!    `base::codegen_fn` + `base::compile_fn`.
//! 3. Lowers every `MonoItem::Static` via `constant::codegen_static`.
//! 4. Synthesises the `main` entry wrapper via
//!    `main_shim::maybe_create_entry_wrapper`.
//! 5. Calls `module.finish().object.write()` for the in-memory ELF
//!    bytes, then writes them to the executable output path via
//!    `semos_std::fs::write`.
//!
//! Skipped on purpose (iter 4 scope): allocator-shim module, debug
//! info (DWARF), global asm (no external assembler), incremental
//! cache work products, multi-CGU parallelism. The single-CGU
//! single-module path is enough for DEMO 80 (`fn main() { println!
//! ("hi"); }`); richer scenarios stay TODO.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use cranelift_codegen::Context;
use cranelift_codegen::ir::Function;
use cranelift_module::{Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use rustc_codegen_ssa::{CodegenResults, CrateInfo};
use rustc_data_structures::fx::FxIndexMap;
use rustc_middle::dep_graph::{WorkProduct, WorkProductId};
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::ty::TyCtxt;
use rustc_session::Session;
use rustc_session::config::{OutputFilenames, OutputType};

use crate::debuginfo::TypeDebugContext;
use crate::unwind_module::UnwindModule;

/// Opaque handle returned by `codegen_crate` and consumed by
/// `join_codegen`. Real codegen happens entirely inside `run_aot`;
/// `join` just unpacks the result.
pub(crate) struct OngoingCodegen {
    crate_info: CrateInfo,
}

impl OngoingCodegen {
    pub(crate) fn join(
        self,
        _sess: &Session,
        _outputs: &OutputFilenames,
    ) -> (CodegenResults, FxIndexMap<WorkProductId, WorkProduct>) {
        // The real machine code went to the SemOS VFS inside run_aot.
        // CodegenResults.modules is the rustc-driver-tracked list of
        // produced object files; we don't track them through the rustc
        // pipeline because semos-rustc emits the final ELF directly
        // (no separate object → link step).
        let results = CodegenResults {
            modules: Vec::new(),
            allocator_module: None,
            crate_info: self.crate_info,
        };
        let work_products = FxIndexMap::default();
        (results, work_products)
    }
}

/// SemOS-target entry that replaces `cg_clif::driver::aot::run_aot`.
pub(crate) fn run_aot(tcx: TyCtxt<'_>) -> Box<OngoingCodegen> {
    semos_std::println!("[aot_semos] run_aot reached — codegen starting");
    // 1. Build one shared ObjectModule for the entire crate.
    //    Single-module / single-task is fine on SemOS — no jobserver,
    //    no parallel CGUs, no incremental cache.
    let isa = crate::build_isa(tcx.sess, false);
    let builder = match ObjectBuilder::new(
        isa,
        String::from("semos_rustc_output"),
        default_libcall_names(),
    ) {
        Ok(b) => b,
        Err(_) => tcx.dcx().fatal("aot_semos: ObjectBuilder::new failed"),
    };
    let mut module: UnwindModule<ObjectModule> = UnwindModule::new(ObjectModule::new(builder), true);

    // 2. Walk every codegen unit and lower its mono items.
    let cgus = tcx.collect_and_partition_mono_items(()).codegen_units;
    for cgu in cgus.iter() {
        let mono_items = cgu.items_in_deterministic_order(tcx);
        super::predefine_mono_items(tcx, &mut module, &mono_items);

        let mut type_dbg = TypeDebugContext::default();
        let mut ctx = Context::new();
        let mut global_asm = String::new();
        let mut codegened: Vec<crate::base::CodegenedFunction> = Vec::new();

        // First pass: lower each Fn from MIR to Cranelift IR.
        for (mono_item, _data) in &mono_items {
            match mono_item {
                MonoItem::Fn(instance) => {
                    let f = crate::base::codegen_fn(
                        tcx,
                        cgu.name(),
                        None, // no debug context — iter 5+ if we want DWARF
                        &mut type_dbg,
                        Function::new(),
                        &mut module,
                        *instance,
                    );
                    codegened.push(f);
                }
                MonoItem::Static(def_id) => {
                    let _ = crate::constant::codegen_static(tcx, &mut module, *def_id);
                }
                MonoItem::GlobalAsm(_item_id) => {
                    // Iter 4 skips global asm — needs an external
                    // assembler on host (gas), which we don't have.
                }
            }
        }

        // Second pass: compile each function's Cranelift IR to machine code.
        for f in codegened {
            crate::base::compile_fn(
                &tcx.prof,
                tcx.output_filenames(()),
                false, // should_write_ir
                &mut ctx,
                &mut module,
                None,
                &mut global_asm,
                f,
            );
        }
    }

    // 3. Synthesise the `main` entry wrapper (calls user main + exits).
    crate::main_shim::maybe_create_entry_wrapper(tcx, &mut module, false, true);

    // 4. Finalize the module → in-memory ELF bytes. cranelift-object emits a
    //    RELOCATABLE object (ET_REL): SHF_ALLOC sections + a symbol table +
    //    relocations, but no PT_LOAD program headers and no fixed addresses.
    //    The SemOS kernel loader only accepts ET_EXEC/ET_DYN with PT_LOAD, so
    //    we run a minimal in-process link to lay the alloc sections at fixed
    //    vaddrs, resolve relocations, and emit a single-PT_LOAD executable.
    let product = module.finish();
    let rel_bytes = match product.object.write() {
        Ok(b) => b,
        Err(e) => tcx
            .dcx()
            .fatal(alloc::format!("aot_semos: ObjectProduct::write failed: {:?}", e)),
    };
    let elf_bytes = match link_object_to_executable(&rel_bytes) {
        Ok(b) => b,
        Err(e) => tcx.dcx().fatal(alloc::format!("aot_semos: link failed: {}", e)),
    };

    // 5. Write the ELF to the executable output path. OutFileName::path()
    //    returns OutFileName which is Real(PathBuf) | Stdout; we only
    //    write to a real path.
    let out_file_name = tcx.output_filenames(()).path(OutputType::Exe);
    use rustc_session::config::OutFileName;
    match out_file_name {
        OutFileName::Real(ref path_buf) => {
            semos_std::println!(
                "[aot_semos] writing {} bytes of ELF to {}",
                elf_bytes.len(), path_buf.as_str());
            if let Err(e) = semos_std::fs::write(path_buf.as_str(), &elf_bytes) {
                tcx.dcx().fatal(alloc::format!(
                    "aot_semos: fs::write({}) failed: {:?}",
                    path_buf.as_str(),
                    e
                ));
            }
            semos_std::println!("[aot_semos] ELF write OK");
        }
        OutFileName::Stdout => {
            tcx.dcx().fatal("aot_semos: -o - (stdout) not supported on SemOS target");
        }
    }

    let target_cpu = tcx
        .sess
        .opts
        .cg
        .target_cpu
        .clone()
        .unwrap_or_else(|| tcx.sess.target.cpu.to_string());
    let crate_info = CrateInfo::new(tcx, target_cpu);
    Box::new(OngoingCodegen { crate_info })
}

// ===========================================================================
// Minimal ELF64 relocatable (ET_REL) -> executable (ET_EXEC) linker
// ===========================================================================
//
// cranelift-object emits an ET_REL object. The SemOS kernel ELF loader
// (kernel-core/src/process/elf.rs) requires ET_EXEC/ET_DYN with PT_LOAD
// program headers. This stand-in for the dropped `back::link` lays every
// SHF_ALLOC section contiguously from a fixed base, resolves the common
// x86-64 relocation kinds against defined symbols, and emits ONE R+W+X
// PT_LOAD segment (file offset 0 == vaddr base, so the ELF headers map too).
//
// Scope (DEMO 80): single static executable, no dynamic linking, no external
// (undefined) symbols, no TLS, no COMDAT. Sufficient for a `#![no_std]
// #![no_main]` program whose `_start` is self-contained; richer programs will
// surface an explicit error here naming the unsupported case.

const LINK_BASE: u64 = 0x40_0000; // user_layout::USER_CODE_BASE (4 MiB)

#[inline]
fn rd_u16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
#[inline]
fn rd_u32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
#[inline]
fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

struct AllocSec {
    file_off: usize, // offset within the source object
    size: usize,
    is_nobits: bool, // .bss — reserve memsz, no file bytes
    out_off: usize,  // offset within the output image (== vaddr - LINK_BASE)
    vaddr: u64,
}

fn link_object_to_executable(obj: &[u8]) -> Result<Vec<u8>, String> {
    use alloc::collections::BTreeMap;

    // ---- ELF header validation ----
    if obj.len() < 64 || &obj[0..4] != b"\x7fELF" {
        return Err(String::from("not an ELF file"));
    }
    if obj[4] != 2 || obj[5] != 1 {
        return Err(String::from("expected 64-bit little-endian ELF"));
    }
    let e_type = rd_u16(obj, 16);
    if e_type != 1 {
        return Err(alloc::format!("expected ET_REL (1), got e_type={}", e_type));
    }
    if rd_u16(obj, 18) != 0x3E {
        return Err(String::from("expected e_machine=EM_X86_64"));
    }
    let e_shoff = rd_u64(obj, 40) as usize;
    let e_shentsize = rd_u16(obj, 58) as usize;
    let e_shnum = rd_u16(obj, 60) as usize;
    if e_shoff == 0 || e_shnum == 0 {
        return Err(String::from("no section headers"));
    }

    let sh = |i: usize| -> usize { e_shoff + i * e_shentsize };
    let sh_type = |i: usize| rd_u32(obj, sh(i) + 4);
    let sh_flags = |i: usize| rd_u64(obj, sh(i) + 8);
    let sh_offset = |i: usize| rd_u64(obj, sh(i) + 24) as usize;
    let sh_size = |i: usize| rd_u64(obj, sh(i) + 32) as usize;
    let sh_link = |i: usize| rd_u32(obj, sh(i) + 40) as usize;
    let sh_addralign = |i: usize| rd_u64(obj, sh(i) + 48) as usize;
    let sh_entsize = |i: usize| rd_u64(obj, sh(i) + 56) as usize;

    const SHF_ALLOC: u64 = 0x2;
    const SHT_NOBITS: u32 = 8;
    const SHT_SYMTAB: u32 = 2;
    const SHT_RELA: u32 = 4;

    // ---- Lay out SHF_ALLOC sections contiguously from LINK_BASE ----
    // File layout: [64 ehdr][56 phdr] then section data. vaddr = LINK_BASE +
    // file_off, so the single PT_LOAD with p_offset=0 maps headers+data.
    let headers_end = 64 + 56;
    let mut cursor = (headers_end + 15) & !15;
    let mut alloc_secs: Vec<AllocSec> = Vec::new();
    let mut shndx_to_alloc: BTreeMap<usize, usize> = BTreeMap::new();
    // PROGBITS-type alloc sections first (file-backed), NOBITS last.
    for pass_nobits in [false, true] {
        for i in 0..e_shnum {
            if sh_flags(i) & SHF_ALLOC == 0 {
                continue;
            }
            let is_nobits = sh_type(i) == SHT_NOBITS;
            if is_nobits != pass_nobits {
                continue;
            }
            let align = sh_addralign(i).max(1);
            cursor = (cursor + align - 1) & !(align - 1);
            let vaddr = LINK_BASE + cursor as u64;
            shndx_to_alloc.insert(i, alloc_secs.len());
            alloc_secs.push(AllocSec {
                file_off: sh_offset(i),
                size: sh_size(i),
                is_nobits,
                out_off: cursor,
                vaddr,
            });
            cursor += sh_size(i);
        }
    }
    if alloc_secs.is_empty() {
        return Err(String::from("no SHF_ALLOC sections to load"));
    }
    // filesz stops after the last file-backed (non-NOBITS) section.
    let mut filesz = headers_end;
    for s in &alloc_secs {
        if !s.is_nobits {
            filesz = filesz.max(s.out_off + s.size);
        }
    }
    let memsz = cursor;

    // ---- Build the output image and copy section bytes ----
    let mut out = alloc::vec![0u8; filesz];
    for s in &alloc_secs {
        if !s.is_nobits && s.size > 0 {
            out[s.out_off..s.out_off + s.size]
                .copy_from_slice(&obj[s.file_off..s.file_off + s.size]);
        }
    }

    // ---- Resolve symbols: name -> absolute vaddr (defined symbols only) ----
    // Find the symbol table and its string table.
    let mut symtab = None;
    for i in 0..e_shnum {
        if sh_type(i) == SHT_SYMTAB {
            symtab = Some(i);
            break;
        }
    }
    let symtab = symtab.ok_or_else(|| String::from("no .symtab"))?;
    let strtab = sh_link(symtab);
    let strtab_off = sh_offset(strtab);
    let str_at = |off: usize| -> String {
        let start = strtab_off + off;
        let mut end = start;
        while end < obj.len() && obj[end] != 0 {
            end += 1;
        }
        String::from_utf8_lossy(&obj[start..end]).into_owned()
    };
    let sym_off = sh_offset(symtab);
    let sym_ent = sh_entsize(symtab).max(24);
    let sym_count = sh_size(symtab) / sym_ent;
    // sym index -> resolved vaddr (None if undefined/unsupported)
    let sym_addr = |idx: usize| -> Option<u64> {
        let so = sym_off + idx * sym_ent;
        let st_shndx = rd_u16(obj, so + 6) as usize;
        let st_value = rd_u64(obj, so + 8);
        shndx_to_alloc
            .get(&st_shndx)
            .map(|&ai| alloc_secs[ai].vaddr + st_value)
    };

    // Entry point: the `_start` symbol.
    let mut entry: Option<u64> = None;
    for idx in 0..sym_count {
        let so = sym_off + idx * sym_ent;
        let name = str_at(rd_u32(obj, so) as usize);
        if name == "_start" {
            entry = sym_addr(idx);
            break;
        }
    }
    let entry = entry.ok_or_else(|| String::from("no defined `_start` symbol"))?;

    // ---- Apply relocations (RELA sections targeting alloc sections) ----
    const R_X86_64_64: u32 = 1;
    const R_X86_64_PC32: u32 = 2;
    const R_X86_64_PLT32: u32 = 4;
    const R_X86_64_32: u32 = 10;
    const R_X86_64_32S: u32 = 11;
    let mut reloc_count = 0usize;
    for i in 0..e_shnum {
        if sh_type(i) != SHT_RELA {
            continue;
        }
        // sh_info of a RELA section names the section it patches.
        let target = rd_u32(obj, sh(i) + 44) as usize;
        let &tgt_ai = match shndx_to_alloc.get(&target) {
            Some(a) => a,
            None => continue, // relocations for a non-loaded section (e.g. debug)
        };
        let tgt = &alloc_secs[tgt_ai];
        if tgt.is_nobits {
            continue;
        }
        let r_off = sh_offset(i);
        let r_ent = sh_entsize(i).max(24);
        let r_num = sh_size(i) / r_ent;
        for r in 0..r_num {
            let ro = r_off + r * r_ent;
            let r_offset = rd_u64(obj, ro) as usize;
            let r_info = rd_u64(obj, ro + 8);
            let r_addend = rd_u64(obj, ro + 16) as i64;
            let r_sym = (r_info >> 32) as usize;
            let r_type = (r_info & 0xffff_ffff) as u32;
            let s = sym_addr(r_sym).ok_or_else(|| {
                let so = sym_off + r_sym * sym_ent;
                alloc::format!(
                    "unresolved/undefined symbol `{}` in relocation",
                    str_at(rd_u32(obj, so) as usize)
                )
            })?;
            let patch = tgt.out_off + r_offset; // offset in `out`
            let p_vaddr = tgt.vaddr + r_offset as u64; // address of the patch site
            match r_type {
                R_X86_64_64 => {
                    let val = (s as i64 + r_addend) as u64;
                    out[patch..patch + 8].copy_from_slice(&val.to_le_bytes());
                }
                R_X86_64_PC32 | R_X86_64_PLT32 => {
                    let val = (s as i64 + r_addend - p_vaddr as i64) as i32;
                    out[patch..patch + 4].copy_from_slice(&val.to_le_bytes());
                }
                R_X86_64_32 | R_X86_64_32S => {
                    let val = (s as i64 + r_addend) as u32;
                    out[patch..patch + 4].copy_from_slice(&val.to_le_bytes());
                }
                other => {
                    return Err(alloc::format!(
                        "unsupported relocation type {} (extend link_object_to_executable)",
                        other
                    ));
                }
            }
            reloc_count += 1;
        }
    }

    // ---- Write the ELF + program header ----
    const PT_LOAD: u32 = 1;
    const PF_X: u32 = 1;
    const PF_W: u32 = 2;
    const PF_R: u32 = 4;
    // ELF header
    out[0..4].copy_from_slice(b"\x7fELF");
    out[4] = 2; // ELFCLASS64
    out[5] = 1; // ELFDATA2LSB
    out[6] = 1; // EV_CURRENT
    out[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    out[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
    out[20..24].copy_from_slice(&1u32.to_le_bytes());
    out[24..32].copy_from_slice(&entry.to_le_bytes()); // e_entry
    out[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    out[40..48].copy_from_slice(&0u64.to_le_bytes()); // e_shoff (none)
    out[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    out[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    out[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    // Program header (single R+W+X PT_LOAD covering the whole image)
    let ph = 64;
    out[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
    out[ph + 4..ph + 8].copy_from_slice(&(PF_R | PF_W | PF_X).to_le_bytes());
    out[ph + 8..ph + 16].copy_from_slice(&0u64.to_le_bytes()); // p_offset
    out[ph + 16..ph + 24].copy_from_slice(&LINK_BASE.to_le_bytes()); // p_vaddr
    out[ph + 24..ph + 32].copy_from_slice(&LINK_BASE.to_le_bytes()); // p_paddr
    out[ph + 32..ph + 40].copy_from_slice(&(filesz as u64).to_le_bytes()); // p_filesz
    out[ph + 40..ph + 48].copy_from_slice(&(memsz as u64).to_le_bytes()); // p_memsz
    out[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align

    semos_std::println!(
        "[aot_semos] linked {} alloc section(s), {} reloc(s); entry=0x{:x} filesz={} memsz={}",
        alloc_secs.len(), reloc_count, entry, filesz, memsz
    );
    Ok(out)
}
