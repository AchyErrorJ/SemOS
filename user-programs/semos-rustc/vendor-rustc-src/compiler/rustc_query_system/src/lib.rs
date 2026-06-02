// tidy-alphabetical-start
#![allow(internal_features)]
#![feature(assert_matches)]
#![feature(core_intrinsics)]
#![feature(min_specialization)]
// tidy-alphabetical-end

// M27 Phase 5b Stage F10: no_std on SemOS target. Host build still uses
// std for parking_lot etc.; the SemOS target is fully no_std + alloc.
#![cfg_attr(target_os = "none", no_std)]

#[macro_use]
extern crate alloc;

#[cfg(not(target_os = "none"))]
extern crate std;

pub mod cache;
pub mod dep_graph;
mod error;
pub mod ich;
pub mod query;
mod values;

pub use error::{HandleCycleError, QueryOverflow, QueryOverflowNote};
pub use values::Value;

rustc_fluent_macro::fluent_messages! { "../messages.ftl" }
