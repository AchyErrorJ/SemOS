//! Various checks
//!
//! # Note
//!
//! This API is completely unstable and subject to change.

// tidy-alphabetical-start
#![cfg_attr(target_os = "none", no_std)]
#![feature(assert_matches)]
#![feature(associated_type_defaults)]
#![feature(box_patterns)]
#![feature(if_let_guard)]
#![feature(iterator_try_collect)]
#![feature(never_type)]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;
#[cfg(not(target_os = "none"))]
extern crate std;

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rustc_middle::query::Providers;

mod abi;
mod assoc;
mod common_traits;
mod consts;
mod errors;
mod implied_bounds;
mod instance;
mod layout;
mod needs_drop;
mod nested_bodies;
mod opaque_types;
mod representability;
pub mod sig_types;
mod structural_match;
mod ty;

rustc_fluent_macro::fluent_messages! { "../messages.ftl" }

pub fn provide(providers: &mut Providers) {
    abi::provide(providers);
    assoc::provide(providers);
    common_traits::provide(providers);
    consts::provide(providers);
    implied_bounds::provide(providers);
    layout::provide(providers);
    needs_drop::provide(providers);
    opaque_types::provide(providers);
    representability::provide(providers);
    ty::provide(providers);
    instance::provide(providers);
    structural_match::provide(providers);
    nested_bodies::provide(providers);
}
