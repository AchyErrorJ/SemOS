//! semos-rustc — M27 Phase 5b iter 1: first rustc_driver_impl link.
//!
//! Stage H iters 1–4 brought every previously-broken `rustc_*` crate
//! green target-side, so this binary can now `cargo build` against the
//! full compiler infrastructure. This iter wires the smallest possible
//! call into `rustc_driver_impl::Callbacks` to prove the linkage works
//! end-to-end on x86_64-unknown-none.
//!
//! Expected behaviour: the binary links (huge — 200+ rustc_* crates plus
//! the Cranelift codegen stack), boots Ring-3, prints a banner, then
//! attempts `rustc_driver_impl::run_compiler(["rustc", "--version"], &mut
//! cb)`. The driver's host-only paths are cfg-stubbed (panic-hook,
//! signal handler, dylib loader, pager) so this call will reach the
//! point where `get_codegen_backend` panics with
//! "requires statically-linked backend" — the next Phase 5b iter wires
//! cg_clif in via a custom `Callbacks::config` hook so the driver doesn't
//! need the dlopen-based loader.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use semos_std::println;

/// Smallest `rustc_driver::Callbacks` impl. The trait has all-default
/// methods; we just need a concrete type to pass as `&mut dyn Callbacks`.
struct SemosCallbacks;
impl rustc_driver_impl::Callbacks for SemosCallbacks {}

semos_std::main!(fn main() {
    println!("semos-rustc Phase 5b iter 1 — rustc_driver_impl link smoke");
    println!(
        "Cranelift stack: codegen+frontend+module+object + cg_clif = OK target-side"
    );
    println!(
        "rustc_* tier: passes/mir_build/mir_transform/hir_analysis/hir_typeck/interface/driver_impl/driver = OK"
    );

    // Build a minimal argv. `rustc --version` exercises the early option
    // parser without needing a real codegen backend.
    let args: Vec<String> = vec![String::from("rustc"), String::from("--version")];

    println!("invoking rustc_driver_impl::run_compiler({:?})", args);
    let mut cb = SemosCallbacks;
    // run_compiler diverges on fatal errors but returns normally on success.
    // For --version it should print to stdout then return.
    rustc_driver_impl::run_compiler(&args, &mut cb);
    println!("run_compiler returned cleanly");
});
