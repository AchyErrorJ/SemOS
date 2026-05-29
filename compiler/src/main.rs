//! semos-compiler — Phase 14 M27 / Session D.1.
//!
//! Host-side ELF emitter. Builds a Cranelift IR function
//! `i64 add(i64 a, i64 b) { return a + b; }`, lowers it to x86_64 machine
//! code, hand-emits an x86_64 `_start` shim that calls `add(1, 2)` through
//! the System V ABI and prints a marker via SYS_WRITE before
//! SYS_EXIT(rax), then wraps the whole thing in an ET_EXEC ELF at
//! `USER_CODE_BASE = 0x400000` with one R+X PT_LOAD segment. Writes the
//! result to `compiler/out/semos_cc_hello.elf` for the kernel to
//! `include_bytes!` and `SYS_SPAWN`.
//!
//! Why no `cranelift-object` + linker: doing the ELF wrap by hand keeps
//! the host-side toolchain trivial (no host `ld` dependency, no rel-obj
//! → executable link step) and matches the byte-for-byte layout in
//! `kernel-core/src/process/elf.rs::create_hello_elf`. It also sets up
//! D.2 where the compiler will run *on* SemOS and won't have a linker.

use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName};
use cranelift_codegen::isa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::{verify_function, Context};
use cranelift_codegen::ir::types::I64;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use std::fs;
use std::path::PathBuf;
use target_lexicon::Triple;

// ---- SemOS user layout (must match kernel-x86_64::paging::user_layout) ----
const USER_CODE_BASE: u64 = 0x400000;
const ELF_HEADER_SIZE: u64 = 64;
const PHDR_SIZE: u64 = 56;
/// File offset (and segment offset from base) of `_start`.
const ENTRY_FILE_OFFSET: u64 = ELF_HEADER_SIZE + PHDR_SIZE; // 0x78
const ENTRY_VADDR: u64 = USER_CODE_BASE + ENTRY_FILE_OFFSET; // 0x400078

// ELF constants we need (the loader's strict set; see kernel-core/src/process/elf.rs).
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_R: u32 = 4;

// SemOS syscall numbers (must match kernel-core::syscall::numbers).
const SYS_WRITE: u32 = 0;
const SYS_EXIT: u32 = 2;

/// Marker the kernel-side DEMO greps for in captured stdout. Distinct
/// from DEMO 71's "cg_clif" so the two demos can't accidentally share a
/// PASS signal.
const MARKER: &[u8] = b"semos-cc D1\n";

/// Exact length of the hand-emitted `_start` shim. Constant — we compute
/// the relative offsets below assuming this.
const SHIM_LEN: usize = 47;

fn build_add_function() -> Function {
    let mut sig = Signature::new(isa::CallConv::SystemV);
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));
    sig.returns.push(AbiParam::new(I64));

    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut fbc = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut func, &mut fbc);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);

        let a = b.block_params(entry)[0];
        let bb = b.block_params(entry)[1];
        let sum = b.ins().iadd(a, bb);
        b.ins().return_(&[sum]);
        b.finalize();
    }
    func
}

/// Emit the `_start` shim. `rel32_to_add` is the displacement from the
/// byte *after* the `call` instruction (offset 37 inside the shim) to
/// the first byte of the Cranelift-emitted `add` function.
fn emit_start_shim(rel32_to_add: i32) -> [u8; SHIM_LEN] {
    // The marker string sits immediately after the shim, so its vaddr is
    // a fixed compile-time constant regardless of how big `add` ends up.
    let marker_vaddr: u64 = ENTRY_VADDR + SHIM_LEN as u64;
    let marker_len: u32 = MARKER.len() as u32;

    let mut s = [0u8; SHIM_LEN];
    let mut p = 0usize;

    // mov rdi, marker_vaddr            ; arg0 = buf
    //   48 BF <imm64>                  ; 10 B
    s[p] = 0x48; s[p + 1] = 0xBF; p += 2;
    s[p..p + 8].copy_from_slice(&marker_vaddr.to_le_bytes()); p += 8;

    // mov esi, marker_len              ; arg1 = len (zero-extends to rsi)
    //   BE <imm32>                     ; 5 B
    s[p] = 0xBE; p += 1;
    s[p..p + 4].copy_from_slice(&marker_len.to_le_bytes()); p += 4;

    // mov eax, SYS_WRITE (0)           ; syscall #
    //   B8 <imm32>                     ; 5 B
    s[p] = 0xB8; p += 1;
    s[p..p + 4].copy_from_slice(&SYS_WRITE.to_le_bytes()); p += 4;

    // syscall                          ; 2 B
    s[p] = 0x0F; s[p + 1] = 0x05; p += 2;

    // mov edi, 1                       ; SystemV arg0 to add()
    //   BF <imm32>                     ; 5 B
    s[p] = 0xBF; p += 1;
    s[p..p + 4].copy_from_slice(&1u32.to_le_bytes()); p += 4;

    // mov esi, 2                       ; SystemV arg1 to add()
    //   BE <imm32>                     ; 5 B
    s[p] = 0xBE; p += 1;
    s[p..p + 4].copy_from_slice(&2u32.to_le_bytes()); p += 4;

    // call rel32                       ; jumps to Cranelift add()
    //   E8 <imm32>                     ; 5 B
    s[p] = 0xE8; p += 1;
    s[p..p + 4].copy_from_slice(&rel32_to_add.to_le_bytes()); p += 4;

    // mov rdi, rax                     ; exit code = add(1, 2) = 3
    //   48 89 C7                       ; 3 B
    s[p] = 0x48; s[p + 1] = 0x89; s[p + 2] = 0xC7; p += 3;

    // mov eax, SYS_EXIT (2)
    //   B8 <imm32>                     ; 5 B
    s[p] = 0xB8; p += 1;
    s[p..p + 4].copy_from_slice(&SYS_EXIT.to_le_bytes()); p += 4;

    // syscall                          ; 2 B
    s[p] = 0x0F; s[p + 1] = 0x05; p += 2;

    assert_eq!(p, SHIM_LEN, "_start shim length drifted");
    s
}

/// Wrap [shim][marker][add_bytes] into a SemOS-shape ET_EXEC ELF.
fn emit_elf(shim: &[u8], marker: &[u8], add_bytes: &[u8]) -> Vec<u8> {
    let body_len = shim.len() + marker.len() + add_bytes.len();
    let total = ENTRY_FILE_OFFSET as usize + body_len;
    let mut buf = vec![0u8; total];

    // ---- ELF64 header ----
    buf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    buf[4] = 2; // ELFCLASS64
    buf[5] = 1; // ELFDATA2LSB (little endian)
    buf[6] = 1; // EV_CURRENT
    // bytes 7..16 stay zero (OS/ABI, ABI version, padding)
    buf[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    buf[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
    buf[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    buf[24..32].copy_from_slice(&ENTRY_VADDR.to_le_bytes()); // e_entry
    buf[32..40].copy_from_slice(&ELF_HEADER_SIZE.to_le_bytes()); // e_phoff = 64
    // e_shoff (40..48), e_flags (48..52) stay zero
    buf[52..54].copy_from_slice(&(ELF_HEADER_SIZE as u16).to_le_bytes()); // e_ehsize
    buf[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());       // e_phentsize
    buf[56..58].copy_from_slice(&1u16.to_le_bytes());                     // e_phnum
    // e_shentsize/shnum/shstrndx stay zero

    // ---- Program header (one PT_LOAD covering everything) ----
    let ph = ELF_HEADER_SIZE as usize;
    let total_u64 = total as u64;
    buf[ph     ..ph +  4].copy_from_slice(&PT_LOAD.to_le_bytes());
    buf[ph +  4..ph +  8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
    buf[ph +  8..ph + 16].copy_from_slice(&0u64.to_le_bytes());           // p_offset = 0
    buf[ph + 16..ph + 24].copy_from_slice(&USER_CODE_BASE.to_le_bytes()); // p_vaddr
    buf[ph + 24..ph + 32].copy_from_slice(&USER_CODE_BASE.to_le_bytes()); // p_paddr
    buf[ph + 32..ph + 40].copy_from_slice(&total_u64.to_le_bytes());      // p_filesz
    buf[ph + 40..ph + 48].copy_from_slice(&total_u64.to_le_bytes());      // p_memsz
    buf[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());      // p_align

    // ---- Body: shim | marker | Cranelift add ----
    let mut off = ENTRY_FILE_OFFSET as usize;
    buf[off..off + shim.len()].copy_from_slice(shim);
    off += shim.len();
    buf[off..off + marker.len()].copy_from_slice(marker);
    off += marker.len();
    buf[off..off + add_bytes.len()].copy_from_slice(add_bytes);

    buf
}

fn main() -> Result<(), String> {
    println!("[semos-compiler] M27 Session D.1 — emit a SemOS ET_EXEC via Cranelift");

    // 1. Target: x86_64-unknown-none, the triple our user programs build against.
    let triple = "x86_64-unknown-none"
        .parse::<Triple>()
        .map_err(|e| format!("parse triple: {}", e))?;
    println!("[semos-compiler] target triple: {}", triple);

    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "speed")
        .map_err(|e| format!("set opt_level: {}", e))?;
    flag_builder
        .set("is_pic", "false") // ET_EXEC, matches our linker.ld
        .map_err(|e| format!("set is_pic: {}", e))?;
    let flags = settings::Flags::new(flag_builder);

    let isa = isa::lookup(triple)
        .map_err(|e| format!("isa lookup: {}", e))?
        .finish(flags)
        .map_err(|e| format!("isa finish: {}", e))?;
    println!("[semos-compiler] ISA: {}", isa.name());

    // 2. Build `i64 add(i64, i64)` IR and verify.
    let func = build_add_function();
    verify_function(&func, isa.as_ref())
        .map_err(|e| format!("verify: {}", e))?;
    println!("[semos-compiler] IR verified:\n{}", func.display());

    // 3. Lower add() to machine code.
    let mut ctx = Context::for_function(func);
    let compiled = ctx
        .compile(isa.as_ref(), &mut Default::default())
        .map_err(|e| format!("compile: {:?}", e))?;
    let add_bytes: Vec<u8> = compiled.code_buffer().to_vec();
    println!(
        "[semos-compiler] add() compiled to {} bytes",
        add_bytes.len()
    );

    // 4. Build the `_start` shim. The `call rel32` target lives at:
    //     end-of-shim (= SHIM_LEN) + marker.len()
    // and the rel32 is measured from the byte *after* the call instruction
    // (offset 37 inside the shim).
    //
    //   call_end_offset = 37
    //   add_start_offset = SHIM_LEN + MARKER.len() = 47 + MARKER.len()
    //   rel32 = add_start_offset - call_end_offset = 10 + MARKER.len()
    let rel32: i32 = (SHIM_LEN as i32 - 37) + MARKER.len() as i32;
    let shim = emit_start_shim(rel32);

    // 5. Wrap into ET_EXEC.
    let elf = emit_elf(&shim, MARKER, &add_bytes);

    // 6. Write to compiler/out/semos_cc_hello.elf for the kernel to
    //    include_bytes! at build time.
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out");
    fs::create_dir_all(&out_dir).map_err(|e| format!("mkdir out: {}", e))?;
    let elf_path = out_dir.join("semos_cc_hello.elf");
    fs::write(&elf_path, &elf).map_err(|e| format!("write elf: {}", e))?;

    println!(
        "[semos-compiler] wrote {} byte ELF → {}",
        elf.len(),
        elf_path.display()
    );

    // 7. Sanity dump (so a serial log proves what we built).
    print!("[semos-compiler] header bytes:");
    for (i, b) in elf.iter().take(64).enumerate() {
        if i % 16 == 0 { print!("\n    {:04X}:", i); }
        print!(" {:02X}", b);
    }
    println!();
    print!("[semos-compiler] _start  bytes:");
    let shim_off = ENTRY_FILE_OFFSET as usize;
    for (i, b) in elf[shim_off..shim_off + SHIM_LEN].iter().enumerate() {
        if i % 16 == 0 { print!("\n    {:04X}:", i); }
        print!(" {:02X}", b);
    }
    println!();
    print!("[semos-compiler] add()   bytes:");
    let add_off = shim_off + SHIM_LEN + MARKER.len();
    for (i, b) in elf[add_off..add_off + add_bytes.len()].iter().enumerate() {
        if i % 16 == 0 { print!("\n    {:04X}:", i); }
        print!(" {:02X}", b);
    }
    println!();

    println!("[semos-compiler] OK — ELF ready, expected: exit 3 + marker on stdout");
    Ok(())
}
