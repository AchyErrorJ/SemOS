// M27 Stage H iter 1: no_std hygiene per RECIPE §1.2.
#![cfg_attr(target_os = "none", no_std)]
// tidy-alphabetical-start
#![feature(decl_macro)]
#![cfg_attr(not(target_os = "none"), feature(file_buffered))]
#![feature(iter_intersperse)]
#![feature(try_blocks)]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;
#[cfg(not(target_os = "none"))]
extern crate std;

#[cfg(target_os = "none")] use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};
mod callbacks;
pub mod errors;
pub mod interface;
mod limits;
pub mod passes;
mod proc_macro_decls;
mod queries;
pub mod util;

pub use callbacks::setup_callbacks;
pub use interface::{Config, run_compiler};
pub use passes::{DEFAULT_QUERY_PROVIDERS, create_and_enter_global_ctxt, parse};
pub use queries::Linker;

#[cfg(test)]
mod tests;

rustc_fluent_macro::fluent_messages! { "../messages.ftl" }
