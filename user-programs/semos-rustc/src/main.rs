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
use rustc_driver_impl::{Callbacks, Compilation};
use rustc_interface::interface::{Config, Compiler};
use rustc_middle::ty::TyCtxt;
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

    // iter 8 diagnostics: print pipeline checkpoints so we can see how far
    // a real compile gets when diagnostics are otherwise swallowed.
    fn after_crate_root_parsing(
        &mut self,
        _c: &Compiler,
        _k: &mut rustc_ast::ast::Crate,
    ) -> Compilation {
        println!("[semos-rustc] checkpoint: crate root parsed");
        Compilation::Continue
    }

    fn after_expansion<'tcx>(&mut self, _c: &Compiler, _tcx: TyCtxt<'tcx>) -> Compilation {
        println!("[semos-rustc] checkpoint: macro expansion done");
        Compilation::Continue
    }

    fn after_analysis<'tcx>(&mut self, _c: &Compiler, _tcx: TyCtxt<'tcx>) -> Compilation {
        println!("[semos-rustc] checkpoint: analysis (typeck+borrowck) done — entering codegen");
        Compilation::Continue
    }
}

/// Phase 5b iter 6: shell argv came from semos_std::env::args(). We
/// intercept `--version`/`-V` and `--help`/`-h` BEFORE calling into
/// rustc_driver_impl because the upstream diagnostic init
/// (rustc_session::session::mk_emitter → AnnotateSnippetEmitter +
/// Translator + stderr_destination) still pulls cfg-gated host
/// surface that silently aborts before any rustc-side println fires.
/// Intercepting here keeps `semos-rustc --version` working end-to-end
/// (proves binary load + semos_std::io::stdout) while we grind through
/// the rest of the rustc diag stack in later iters.
fn intercept_short_circuit(args: &[String]) -> bool {
    for a in args {
        match a.as_str() {
            "--version" | "-V" => {
                println!("rustc 1.86.0-semos (M27 Phase 5b iter 6)");
                println!("backend: cranelift (statically linked)");
                return true;
            }
            "--help" | "-h" => {
                println!("semos-rustc — SemOS-native Rust compiler (M27 port)");
                println!("");
                println!("USAGE: semos-rustc [OPTIONS] <INPUT>");
                println!("");
                println!("OPTIONS:");
                println!("  -V, --version    Print version info and exit");
                println!("  -h, --help       Print this help");
                println!("  -o <PATH>        Write output executable to <PATH>");
                println!("");
                println!("EXAMPLE:");
                println!("  semos-rustc /hello.rs -o /tmp/hello.elf");
                return true;
            }
            _ => {}
        }
    }
    false
}

semos_std::main!(fn main() {
    // Skip argv[0] like rustc_driver_impl::run_compiler does internally.
    let argv: Vec<String> = semos_std::env::args().into_iter().skip(1).collect();

    if intercept_short_circuit(&argv) {
        return;
    }

    // DEMO 80 step C3: decode-compatibility self-test. Embeds a host-built
    // compiler_builtins.rmeta and asks rustc_metadata to decode it — proving
    // (or disproving) that host-built crate metadata is schema-compatible with
    // this semos-rustc before we build any disk-sysroot plumbing.
    if argv.iter().any(|a| a == "--c3-selftest") {
        static CB_RMETA: &[u8] = include_bytes!("../test-sources/compiler_builtins.rmeta");
        println!("[c3] embedded compiler_builtins.rmeta: {} bytes", CB_RMETA.len());
        rustc_metadata::locator::semos_c3_probe(
            CB_RMETA.to_vec(),
            option_env!("CFG_VERSION").unwrap_or("1.84.0-semos-m27"),
        );
        println!("[c3] selftest returned");
        return;
    }

    println!("semos-rustc Phase 5b iter 8 [BUILD-TAG abc123] — invoking rustc_driver_impl::run_compiler");
    println!("argv: {:?}", argv);
    println!("[main] about to call rustc_driver_impl::run_compiler");

    // Prepend the binary name back since run_compiler strips argv[0].
    let mut args: Vec<String> = vec![String::from("semos-rustc")];
    args.extend(argv);

    let mut cb = SemosCallbacks;
    rustc_driver_impl::run_compiler(&args, &mut cb);
    println!("run_compiler returned cleanly");
});
