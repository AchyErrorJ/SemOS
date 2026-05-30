//! This library contains code that is common to both the `cranelift-codegen` and
//! `cranelift-codegen-meta` libraries.

// D.2 port: add no_std so this builds for x86_64-unknown-none.
#![no_std]
#![deny(missing_docs)]

pub mod constant_hash;
pub mod constants;

/// Version number of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
