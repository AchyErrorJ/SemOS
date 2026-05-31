//! The Rust Abstract Syntax Tree (AST).
//!
//! # Note
//!
//! This API is completely unstable and subject to change.

// M27 Phase 2b B1: rustc_ast runs against semos_std (no full std) on
// the SemOS-host build. `#![no_std]` MUST be the first inner attribute,
// before the `#![feature(...)]` block (Rust attribute-ordering rules
// learned in A2-followup §lib.rs).
#![no_std]
// tidy-alphabetical-start
#![cfg_attr(bootstrap, feature(array_windows))]
#![doc(test(attr(deny(warnings), allow(internal_features))))]
#![feature(associated_type_defaults)]
#![feature(box_patterns)]
#![feature(if_let_guard)]
#![feature(iter_order_by)]
#![feature(macro_metavar_expr)]
#![recursion_limit = "256"]
// tidy-alphabetical-end

// M27 Phase 2b B1: alloc prelude — provides Vec/String/Box/format!/vec!
// crate-wide. The `#[macro_use]` is what reaches vec!/format! into
// submodules without per-file `use alloc::vec;`.
#[macro_use]
extern crate alloc;

pub mod util {
    pub mod case;
    pub mod classify;
    pub mod comments;
    pub mod literal;
    pub mod parser;
    pub mod unicode;
}

pub mod ast;
pub mod ast_traits;
pub mod attr;
pub mod entry;
pub mod expand;
pub mod format;
pub mod mut_visit;
pub mod node_id;
pub mod token;
pub mod tokenstream;
pub mod visit;

pub use self::ast::*;
pub use self::ast_traits::{AstNodeWrapper, HasAttrs, HasNodeId, HasTokens};

/// Requirements for a `StableHashingContext` to be used in this crate.
/// This is a hack to allow using the `HashStable_Generic` derive macro
/// instead of implementing everything in `rustc_middle`.
pub trait HashStableContext: rustc_span::HashStableContext {}
