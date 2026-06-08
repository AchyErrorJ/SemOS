// Stage H iter 4: no_std hygiene so the empty re-export crate works on
// x86_64-unknown-none. (Without this, rustc tries to link std as the
// crate's implicit prelude.)
#![cfg_attr(target_os = "none", no_std)]

// This crate is intentionally empty and a re-export of `rustc_driver_impl` to allow the code in
// `rustc_driver_impl` to be compiled in parallel with other crates.

pub use rustc_driver_impl::*;
