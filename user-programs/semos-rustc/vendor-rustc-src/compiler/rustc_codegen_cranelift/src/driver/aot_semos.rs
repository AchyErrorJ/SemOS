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

    // 4. Finalize the module → in-memory ELF bytes.
    let product = module.finish();
    let elf_bytes = match product.object.write() {
        Ok(b) => b,
        Err(e) => tcx
            .dcx()
            .fatal(alloc::format!("aot_semos: ObjectProduct::write failed: {:?}", e)),
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
