#![no_std]
// tidy-alphabetical-start
#![allow(internal_features)]
#![cfg_attr(bootstrap, feature(array_windows))]
#![feature(associated_type_defaults)]
#![feature(if_let_guard)]
#![feature(macro_metavar_expr)]
#![feature(proc_macro_diagnostic)]
#![feature(proc_macro_internals)]
#![feature(try_blocks)]
#![feature(yeet_expr)]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;

mod build;
mod errors;
mod mbe;
mod placeholders;
// M27 §1.5: proc-macro server (load+drive client-side dylib via mpsc).
// Production body is host-only on SemOS v1 (no dlopen). Module is
// retained on the SemOS target as a thin shape-preserving stub via
// the in-file cfg gates inside proc_macro_server.rs.
mod proc_macro_server;
mod stats;

pub use mbe::macro_rules::{MacroRulesMacroExpander, compile_declarative_macro};
pub mod base;
pub mod config;
pub mod expand;
pub mod module;
pub mod proc_macro;

pub fn provide(providers: &mut rustc_middle::query::Providers) {
    providers.derive_macro_expansion = proc_macro::provide_derive_macro_expansion;
}

rustc_fluent_macro::fluent_messages! { "../messages.ftl" }
