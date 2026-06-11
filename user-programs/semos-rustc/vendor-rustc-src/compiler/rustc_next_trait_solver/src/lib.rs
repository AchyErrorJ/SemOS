//! Crate containing the implementation of the next-generation trait solver.
//!
//! This crate may also contain things that are used by the old trait solver,
//! but were uplifted in the process of making the new trait solver generic.
//! So if you got to this crate from the old solver, it's totally normal.

// M27 Stage F5: no_std + alloc — `x86_64-unknown-none` has no `std`.
// The crate body uses `Vec`/`Box`/`Option`/`Result` etc.; the alloc
// prelude reaches them. `#[macro_use]` brings `vec!`/`format!` in.
#![cfg_attr(target_os = "none", no_std)]

// tidy-alphabetical-start
#![allow(rustc::direct_use_of_rustc_type_ir)]
#![allow(rustc::usage_of_type_ir_inherent)]
#![allow(rustc::usage_of_type_ir_traits)]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;

pub mod canonical;
pub mod coherence;
pub mod delegate;
pub mod placeholder;
pub mod resolve;
pub mod solve;
