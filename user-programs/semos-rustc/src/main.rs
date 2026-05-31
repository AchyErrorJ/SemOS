//! semos-rustc — M27 Phase 5b: rustc on SemOS Ring 3.
//!
//! Eventual goal (DEMO 80): drive `rustc_driver_impl` to compile a Rust
//! source file into a SemOS ELF, written back to the path namespace.
//!
//! Stage 1 (this file, scaffold-only): a stub `_start` that proves the
//! Cargo.toml + build.rs + link.ld template works end-to-end on the
//! SemOS Ring-3 layout (non-PIE ET_EXEC at USER_CODE_BASE 0x400000),
//! mirroring the semos-cc D.2 shape. Prints a marker, exits cleanly.
//!
//! Stage 2 (after Phase 5b integration wave): replace the stub with
//! `rustc_driver::run_compiler` invocations + cg_clif statically linked
//! as the codegen backend. The 48 ported rustc_* crates in
//! `vendor-rustc-src/` are the dep graph rustc_driver_impl pulls in.

#![no_std]
#![no_main]

use semos_std::println;

semos_std::main!(fn main() {
    println!("semos-rustc Phase 5b scaffold — stage 1");
    println!("rustc_driver_impl integration TBD (next-session work)");
});
