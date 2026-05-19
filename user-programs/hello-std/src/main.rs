//! hello-std.elf — Phase 14 M25 acceptance test for the std shim.
//!
//! Same observable behaviour as `hello.elf` ("Hello from real Rust
//! ELF!" then exit 0), but produced through the `semos_std` shim
//! instead of raw inline-asm syscalls. Proves the shim's `println!`
//! macro + `process::exit` lowering work end-to-end.
//!
//! Built with: cargo build --release  (from this directory)
//! Output: target/x86_64-unknown-none/release/hello-std

#![no_std]
#![no_main]

use semos_std::{main, println};

main!(fn main() {
    println!("Hello from semos-std!");
});
