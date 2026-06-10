#![cfg_attr(target_os = "none", no_std)]
// tidy-alphabetical-start
#![allow(internal_features)]
#![cfg_attr(bootstrap, feature(array_windows))]
#![feature(array_windows)]
#![feature(associated_type_defaults)]
#![feature(if_let_guard)]
#![feature(macro_metavar_expr)]
#![cfg_attr(not(target_os = "none"), feature(proc_macro_diagnostic))]
#![cfg_attr(not(target_os = "none"), feature(proc_macro_internals))]
#![feature(try_blocks)]
#![feature(yeet_expr)]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;

#[cfg(not(target_os = "none"))]
extern crate std;

mod build;
mod errors;
mod mbe;
mod placeholders;
// M27 §1.5: proc-macro server (load+drive client-side dylib via mpsc).
// Production body is host-only on SemOS v1 (no dlopen / mpsc / dlopen).
// Per C3 §3 recommendation, the entire module is host-only — the
// SemOS-target `proc_macro` module's expand() stubs do not reach
// any proc_macro_server::Rustc constructor, so the module never compiles
// on target. Upstream file body is preserved verbatim under this gate.
#[cfg(not(target_os = "none"))]
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
