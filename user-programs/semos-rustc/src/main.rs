//! semos-rustc — M27 Phase 5b iter 2: static cg_clif backend injection.
//!
//! iter 1 (commit `f9ae4fb`) proved the binary links and reaches
//! `rustc_driver_impl::run_compiler`. The driver then needs a codegen
//! backend; upstream rustc dlopens one from a shared object, but on
//! SemOS that path is cfg-stubbed to panic ("requires statically-linked
//! backend"). iter 2 plugs the gap by overriding
//! `Callbacks::config(&mut Config)` to set
//! `config.make_codegen_backend = Some(<cg_clif factory>)`. The driver
//! takes that precedence path (interface.rs:454) and never touches the
//! dlopen loader.
//!
//! After iter 2, `run_compiler(["rustc", "--version"], &mut cb)` should
//! parse args, print version, and exit cleanly. A real `.rs` source
//! invocation reaches the AOT driver (`cg_clif::driver::aot`) which is
//! currently host-gated — iter 3 wires a SemOS-native AOT path.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use rustc_codegen_ssa::traits::CodegenBackend;
use rustc_driver_impl::Callbacks;
use rustc_interface::interface::Config;
use rustc_session::config::Options;
use rustc_target::spec::Target;
use semos_std::println;

struct SemosCallbacks;

impl Callbacks for SemosCallbacks {
    fn config(&mut self, config: &mut Config) {
        // Inject cg_clif as the static codegen backend. `make_codegen_backend`
        // takes precedence over the dylib loader (rustc_interface/src/
        // interface.rs:454) so the dlopen path that's cfg-stubbed to panic
        // on SemOS never fires.
        config.make_codegen_backend = Some(Box::new(
            |_opts: &Options, _target: &Target| -> Box<dyn CodegenBackend> {
                rustc_codegen_cranelift::__rustc_codegen_backend()
            },
        ));
    }
}

semos_std::main!(fn main() {
    println!("semos-rustc Phase 5b iter 2 — cg_clif statically wired as backend");

    let args: Vec<String> = vec![String::from("rustc"), String::from("--version")];

    println!("invoking rustc_driver_impl::run_compiler({:?})", args);
    let mut cb = SemosCallbacks;
    rustc_driver_impl::run_compiler(&args, &mut cb);
    println!("run_compiler returned cleanly");
});
