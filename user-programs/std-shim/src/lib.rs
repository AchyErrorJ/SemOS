//! `semos-std` — minimal std shim over Semantic OS syscalls.
//!
//! Phase 14 M25 Tier 1 foundation. The end-state goal is a near-complete
//! drop-in for upstream `std` so cargo can rebuild rustc against it. This
//! crate is far less than that today — it's the scaffolding plus the few
//! surfaces a "hello world" Rust program actually exercises: print,
//! panic, exit, args, env, basic file I/O.
//!
//! Programs use it like upstream std:
//! ```ignore
//! #![no_std]            // we don't ship the full std
//! #![no_main]           // we provide _start via the macro below
//! use semos_std::{println, fs};
//! semos_std::main!(fn main() { println!("hi"); });
//! ```
//!
//! The dependency is on `core` only — semos-std re-exports core's
//! `fmt`, `mem`, `ptr`, `slice`, etc. unchanged, so user code can pretend
//! it's using std.
//!
//! ## What's wired today
//!
//! - [`io::write_str`] / `print!` / `println!` — SYS_WRITE on stdout (fd=1)
//! - [`process::exit`] — SYS_EXIT
//! - [`process::abort`] — SYS_EXIT(0xABEEFu64 ^ ...) wrapper for panics
//! - [`env::args`] — placeholder; argv parsing tracked in M25 Tier 2
//! - [`fs::read`] / [`fs::write`] — SYS_OPEN + SYS_FREAD/FWRITE + SYS_CLOSE
//! - [`thread::sleep_ticks`] — SYS_SLEEP for raw tick counts (Duration
//!   wrapper waits on a real Duration type; not yet bridged)
//!
//! Everything else returns an unimplemented stub. The full surface lives
//! in `docs/STD_SHIM_SURFACE.md`.

#![no_std]
#![allow(unsafe_op_in_unsafe_fn)]

// The `alloc` crate provides Vec/String/Box/Arc — wired up via our
// own GlobalAlloc impl below (M25 Tier 2 #50). Pulled in via build-std
// in .cargo/config.toml.
extern crate alloc as core_alloc;

// Re-export the `alloc` crate's surface so downstream programs can do
// `use semos_std::vec::Vec` exactly as they'd `use std::vec::Vec`.
// `alloc` reuses the same module names std does (vec, string, boxed,
// collections, sync), so this is enough for a transparent rename.
pub use core_alloc::{boxed, collections, format, rc, string, sync, vec};

// Re-export core surface so downstream programs can do `use semos_std::fmt`
// the way they'd `use std::fmt`. Keeps the migration cost from raw-syscall
// code low.
pub use core::{
    cmp, fmt, hint, marker, mem, ops, option, ptr, result, slice, str,
};

mod alloc_impl;
pub mod arch;
pub mod env;
pub mod fs;
pub mod io;
pub mod process;
pub mod thread;

#[doc(hidden)]
pub use core as __core;

// Install our SYS_HEAP_ALLOC-backed allocator as the global. Downstream
// programs that depend on semos-std inherit this automatically — no
// further #[global_allocator] needed.
#[global_allocator]
static SEMOS_ALLOCATOR: alloc_impl::SemosAllocator = alloc_impl::SemosAllocator::new();

/// Define the program's `_start` entry. Wraps the user's `main` function
/// so it gets called with no arguments and its return value (anything
/// the `Termination` trait would accept upstream) is sent to
/// `process::exit`.
///
/// Use like:
/// ```ignore
/// semos_std::main!(fn main() {
///     semos_std::println!("hello");
/// });
/// ```
#[macro_export]
macro_rules! main {
    (fn main() $body:block) => {
        #[no_mangle]
        #[link_section = ".text._start"]
        pub extern "C" fn _start() -> ! {
            // Run the user's main, then exit cleanly. main() can call
            // process::exit() itself for non-zero codes.
            fn __semos_user_main() $body
            __semos_user_main();
            $crate::process::exit(0);
        }

        #[panic_handler]
        fn __semos_panic(_info: &$crate::__core::panic::PanicInfo) -> ! {
            // TODO: write panic_info to stderr once that's wired.
            $crate::process::exit(1);
        }
    };
}
