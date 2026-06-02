// M27 Stage F9: no_std + alloc.
#![no_std]

// tidy-alphabetical-start
#![allow(internal_features)]
#![feature(default_field_values)]
#![feature(iter_intersperse)]
#![feature(macro_derive)]
#![feature(rustc_attrs)]
// To generate CodegenOptionsTargetModifiers and UnstableOptionsTargetModifiers enums
// with macro_rules, it is necessary to use recursive mechanic ("Incremental TT Munchers").
#![recursion_limit = "256"]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;

pub mod errors;

pub mod utils;
pub use lint::{declare_lint, declare_lint_pass, declare_tool_lint, impl_lint_pass};
pub use rustc_lint_defs as lint;
pub mod parse;

pub mod code_stats;
#[macro_use]
pub mod config;
pub mod cstore;
pub mod filesearch;
mod macros;
mod options;

// Stage F9: println!/print! are std-prelude macros, unavailable on
// no_std. SemOS stubs are no-ops since CLI help / `--print` output
// doesn't run on the target.
#[cfg(target_os = "none")]
#[macro_export]
macro_rules! __semos_stub_println { ($($arg:tt)*) => { () }; }
#[cfg(target_os = "none")]
#[macro_export]
macro_rules! __semos_stub_print { ($($arg:tt)*) => { () }; }
#[cfg(target_os = "none")]
pub(crate) use __semos_stub_println as println;
#[cfg(target_os = "none")]
pub(crate) use __semos_stub_print as print;

// Stage F9: getopts is host-only (CLI option parsing). SemOS-target
// rustc doesn't read argv via getopts, so we stub the surface used.
#[cfg(target_os = "none")]
mod getopts {
    use alloc::string::String;
    use alloc::vec::Vec;
    pub struct Matches {
        pub free: Vec<String>,
    }
    impl Matches {
        pub fn opt_present(&self, _name: &str) -> bool { false }
        pub fn opts_present(&self, _names: &[String]) -> bool { false }
        pub fn opt_str(&self, _name: &str) -> Option<String> { None }
        pub fn opt_strs(&self, _name: &str) -> Vec<String> { Vec::new() }
        pub fn opt_strs_pos(&self, _name: &str) -> Vec<(usize, String)> { Vec::new() }
        pub fn opt_count(&self, _name: &str) -> usize { 0 }
        pub fn opt_positions(&self, _name: &str) -> Vec<usize> { Vec::new() }
        pub fn opt_get<T: core::str::FromStr>(&self, _name: &str) -> core::result::Result<Option<T>, T::Err> { Ok(None) }
        pub fn opt_default(&self, _name: &str, def: &str) -> Option<String> {
            Some(String::from(def))
        }
    }
    pub struct Options;
    impl Options {
        pub fn new() -> Self { Self }
        pub fn optopt(&mut self, _: &str, _: &str, _: &str, _: &str) -> &mut Self { self }
        pub fn optmulti(&mut self, _: &str, _: &str, _: &str, _: &str) -> &mut Self { self }
        pub fn optflag(&mut self, _: &str, _: &str, _: &str) -> &mut Self { self }
        pub fn optflagmulti(&mut self, _: &str, _: &str, _: &str) -> &mut Self { self }
    }
}

pub mod search_paths;

mod session;
pub use session::*;

pub mod output;

// Stage F9: on host getopts is an extern crate; on SemOS it's our
// local stub module. Re-export only on host since the local stub
// is private to this crate.
#[cfg(not(target_os = "none"))]
pub use ::getopts;

rustc_fluent_macro::fluent_messages! { "../messages.ftl" }

/// Requirements for a `StableHashingContext` to be used in this crate.
/// This is a hack to allow using the `HashStable_Generic` derive macro
/// instead of implementing everything in `rustc_middle`.
pub trait HashStableContext: rustc_ast::HashStableContext + rustc_hir::HashStableContext {}
