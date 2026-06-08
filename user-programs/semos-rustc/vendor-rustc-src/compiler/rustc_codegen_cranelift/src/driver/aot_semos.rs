//! SemOS-native AOT driver — Phase 5b iter 3 stub.
//!
//! Upstream cg_clif's `driver::aot` is host-only (heavy `std::fs` /
//! `std::process::Command` / `std::thread::Builder` / jobserver use,
//! plus the `rustc_codegen_ssa::back::link` subsystem which we dropped
//! per §1.7). On SemOS target we need a different driver: emit object
//! bytes via cranelift-object (which is already no_std-clean from
//! Stage G iter 6), then write them to the SemOS VFS via
//! `semos_std::fs::write`. No external linker, no thread pool, no
//! jobserver — single-task in-process codegen.
//!
//! **iter 3 scope:** minimal stub so `CraneliftCodegenBackend::
//! codegen_crate` returns a valid `OngoingCodegen` and `join_codegen`
//! produces a valid `CodegenResults`. This satisfies `run_compiler`'s
//! type contracts without actually emitting code yet. Real codegen
//! wiring (mono-item collection + cg_clif's `base::codegen_fn` per
//! function + finalize → ELF bytes → fs::write) lands in iter 4.
//!
//! The stub returns an empty `CodegenResults` (zero modules) which
//! tells downstream code "this crate has nothing to link" — fine
//! for the `rustc --version` path; a real `.rs` source compile will
//! produce no output but at least won't fatal-panic in the driver
//! contract.

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

use rustc_codegen_ssa::{CodegenResults, CrateInfo};
use rustc_data_structures::fx::FxIndexMap;
use rustc_middle::dep_graph::{WorkProduct, WorkProductId};
use rustc_middle::ty::TyCtxt;
use rustc_session::Session;
use rustc_session::config::OutputFilenames;

/// Opaque handle returned by `codegen_crate` and consumed by
/// `join_codegen`. Holds enough state for join to build a CodegenResults.
pub(crate) struct OngoingCodegen {
    crate_info: CrateInfo,
}

impl OngoingCodegen {
    pub(crate) fn join(
        self,
        _sess: &Session,
        _outputs: &OutputFilenames,
    ) -> (CodegenResults, FxIndexMap<WorkProductId, WorkProduct>) {
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
///
/// iter 3 stub: collect just enough metadata for CrateInfo and return.
/// No actual mono-item codegen yet — iter 4 wires that.
pub(crate) fn run_aot(tcx: TyCtxt<'_>) -> Box<OngoingCodegen> {
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
