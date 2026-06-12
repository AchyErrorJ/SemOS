// M27 DEMO 80 option B — host rustc driver.
// Drives the vendored rustc front+back end on the host so we can compile
// core/compiler_builtins for x86_64-unknown-none with a metadata symbol table
// matching semos-rustc. Backend is cg_clif (same make_codegen_backend trick as
// the SemOS binary), avoiding rustc_codegen_llvm in the vendored tree.
// Link rustc_driver first so all transitive rustc_private crates are pulled
// into the host binary in rlib format (rustc bootstrap requirement).
extern crate rustc_driver;
// rustc_interface uses these via query providers / dynamic dispatch; force
// them to be built as rlibs and linked into the final binary.
extern crate rustc_hir_typeck;
extern crate rustc_mir_build;

use rustc_codegen_ssa::traits::CodegenBackend;
use rustc_driver_impl::{Callbacks, run_compiler};
use rustc_interface::interface::Config;
use rustc_session::config::Options;
use rustc_target::spec::Target;

struct HostCallbacks;

impl Callbacks for HostCallbacks {
    fn config(&mut self, config: &mut Config) {
        config.make_codegen_backend = Some(Box::new(
            |_opts: &Options, _target: &Target| -> Box<dyn CodegenBackend> {
                rustc_codegen_cranelift::__rustc_codegen_backend()
            },
        ));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut cb = HostCallbacks;
    run_compiler(&args, &mut cb);
}
