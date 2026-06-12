#![cfg_attr(target_os = "none", no_std)]
// tidy-alphabetical-start
#![allow(internal_features)]
#![cfg_attr(bootstrap, feature(array_windows))]
#![feature(array_windows)]
#![feature(associated_type_defaults)]
#![feature(if_let_guard)]
#![feature(macro_metavar_expr)]
// option B: proc_macro_diagnostic/internals don't exist on nightly-2026-02-01 and
// proc-macros aren't needed to compile core, so gate them (and all proc-macro code)
// behind the procmacro_stub cfg set for the host build.
#![cfg_attr(all(not(target_os = "none"), not(procmacro_stub)), feature(proc_macro_diagnostic))]
#![cfg_attr(all(not(target_os = "none"), not(procmacro_stub)), feature(proc_macro_internals))]
#![feature(try_blocks)]
#![feature(yeet_expr)]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;

#[cfg(not(target_os = "none"))]
extern crate std as semos_std;

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
#[cfg(all(not(target_os = "none"), not(procmacro_stub)))]
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
