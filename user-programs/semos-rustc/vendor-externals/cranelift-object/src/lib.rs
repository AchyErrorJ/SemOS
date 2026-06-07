//! Top-level lib.rs for `cranelift_object`.
//!
//! This re-exports `object` so you don't have to explicitly keep the versions in sync.

#![deny(missing_docs)]
// M27 Phase 5c Stage G iter 6: port cranelift-object to no_std.
// rustc_codegen_cranelift builds against x86_64-unknown-none so this
// transitive must compile without std. extern crate alloc gives us
// Vec/Box/String/format! via macro_use, plus prelude types.
#![no_std]

#[macro_use]
extern crate alloc;

mod backend;

pub use crate::backend::{ObjectBuilder, ObjectModule, ObjectProduct};

/// Version number of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use object;
