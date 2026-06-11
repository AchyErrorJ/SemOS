// M27 Stage F9: no_std + alloc.
#![cfg_attr(target_os = "none", no_std)]

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

// Stage F9: getopts is host-only (CLI option parsing). Original stub
// (F9 / H iter 4) was a no-op — parse() threw argv away, opt_present()
// always returned false. Phase 5b iter 6 makes the stub actually parse:
// it registers (short, long, has_arg) tuples through the builder methods
// rustc uses (optopt/optmulti/optflag/optflagmulti), then walks argv at
// parse() time matching `--long`, `--long=val`, `-short`, and `-short val`.
// opt_present/opt_str/opt_strs return live results. Just enough surface
// to let `rustc --version` and `rustc /file.rs -o /out` work end-to-end.
#[cfg(target_os = "none")]
pub mod getopts {
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    #[derive(Clone)]
    struct OptDef {
        short: String,
        long: String,
        has_arg: bool,
    }

    pub struct Matches {
        pub free: Vec<String>,
        // (canonical_long, optional_value, position_in_argv)
        // canonical_long is OptDef.long if non-empty, else short. Used so
        // queries by EITHER short or long match the same entry.
        present: Vec<(String, Option<String>, usize)>,
        defs: Vec<OptDef>,
    }

    impl Matches {
        fn matches_name(&self, def_long: &str, query: &str) -> bool {
            if def_long == query { return true; }
            // query might be short form — check OptDef table.
            self.defs.iter().any(|d| d.long == def_long && d.short == query)
        }
        pub fn opt_present(&self, name: &str) -> bool {
            self.present.iter().any(|(n, _, _)| self.matches_name(n, name))
        }
        pub fn opts_present(&self, names: &[String]) -> bool {
            names.iter().any(|n| self.opt_present(n.as_str()))
        }
        pub fn opt_str(&self, name: &str) -> Option<String> {
            self.present.iter()
                .find(|(n, _, _)| self.matches_name(n, name))
                .and_then(|(_, v, _)| v.clone())
        }
        pub fn opt_strs(&self, name: &str) -> Vec<String> {
            self.present.iter()
                .filter(|(n, _, _)| self.matches_name(n, name))
                .filter_map(|(_, v, _)| v.clone())
                .collect()
        }
        pub fn opt_strs_pos(&self, name: &str) -> Vec<(usize, String)> {
            self.present.iter()
                .filter(|(n, _, _)| self.matches_name(n, name))
                .filter_map(|(_, v, pos)| v.clone().map(|s| (*pos, s)))
                .collect()
        }
        pub fn opt_count(&self, name: &str) -> usize {
            self.present.iter().filter(|(n, _, _)| self.matches_name(n, name)).count()
        }
        pub fn opt_positions(&self, name: &str) -> Vec<usize> {
            self.present.iter()
                .filter(|(n, _, _)| self.matches_name(n, name))
                .map(|(_, _, pos)| *pos)
                .collect()
        }
        pub fn opt_get<T: core::str::FromStr>(&self, name: &str) -> core::result::Result<Option<T>, T::Err> {
            match self.opt_str(name) {
                Some(s) => s.parse::<T>().map(Some),
                None => Ok(None),
            }
        }
        pub fn opt_default(&self, name: &str, def: &str) -> Option<String> {
            Some(self.opt_str(name).unwrap_or_else(|| String::from(def)))
        }
    }

    pub struct Options {
        opts: Vec<OptDef>,
    }
    impl Options {
        pub fn new() -> Self { Self { opts: Vec::new() } }
        pub fn optopt(&mut self, short: &str, long: &str, _: &str, _: &str) -> &mut Self {
            self.opts.push(OptDef { short: short.to_string(), long: long.to_string(), has_arg: true });
            self
        }
        pub fn optmulti(&mut self, short: &str, long: &str, _: &str, _: &str) -> &mut Self {
            self.opts.push(OptDef { short: short.to_string(), long: long.to_string(), has_arg: true });
            self
        }
        pub fn optflag(&mut self, short: &str, long: &str, _: &str) -> &mut Self {
            self.opts.push(OptDef { short: short.to_string(), long: long.to_string(), has_arg: false });
            self
        }
        pub fn optflagmulti(&mut self, short: &str, long: &str, _: &str) -> &mut Self {
            self.opts.push(OptDef { short: short.to_string(), long: long.to_string(), has_arg: false });
            self
        }
        fn find_long(&self, name: &str) -> Option<&OptDef> {
            self.opts.iter().find(|d| d.long == name)
        }
        fn find_short(&self, name: &str) -> Option<&OptDef> {
            self.opts.iter().find(|d| d.short == name)
        }
        fn canonical(d: &OptDef) -> String {
            if !d.long.is_empty() { d.long.clone() } else { d.short.clone() }
        }
        pub fn parse(&self, args: &[String]) -> Result<Matches, Fail> {
            let mut free = Vec::new();
            let mut present = Vec::new();
            let mut i = 0;
            while i < args.len() {
                let arg = args[i].clone();
                if arg == "--" {
                    // remainder is positional
                    for rest in &args[i+1..] { free.push(rest.clone()); }
                    break;
                }
                if let Some(rest) = arg.strip_prefix("--") {
                    // Long form: --name or --name=value
                    let (name, inline_val) = if let Some(eq) = rest.find('=') {
                        (&rest[..eq], Some(rest[eq+1..].to_string()))
                    } else {
                        (rest, None)
                    };
                    if let Some(def) = self.find_long(name) {
                        let canon = Self::canonical(def);
                        if def.has_arg {
                            let val = match inline_val {
                                Some(v) => Some(v),
                                None => {
                                    i += 1;
                                    args.get(i).cloned()
                                }
                            };
                            present.push((canon, val, i));
                        } else {
                            present.push((canon, None, i));
                        }
                    }
                    // unrecognized long flag: silently ignore (lenient stub)
                } else if arg.len() > 1 && arg.starts_with('-') {
                    let rest = &arg[1..];
                    if let Some(def) = self.find_short(rest) {
                        let canon = Self::canonical(def);
                        if def.has_arg {
                            i += 1;
                            let val = args.get(i).cloned();
                            present.push((canon, val, i));
                        } else {
                            present.push((canon, None, i));
                        }
                    } else if let Some(def) = self.find_short(&rest[..1]) {
                        // -Xvalue form (short flag with attached value)
                        let canon = Self::canonical(def);
                        if def.has_arg {
                            present.push((canon, Some(rest[1..].to_string()), i));
                        } else {
                            present.push((canon, None, i));
                        }
                    }
                    // unrecognized short flag: silently ignore
                } else {
                    free.push(arg);
                }
                i += 1;
            }
            Ok(Matches { free, present, defs: self.opts.clone() })
        }
        pub fn usage(&self, brief: &str) -> String { String::from(brief) }
        pub fn usage_with_format<F: FnOnce(&mut dyn Iterator<Item = String>) -> String>(
            &self, format: F,
        ) -> String {
            let mut empty = core::iter::empty();
            format(&mut empty)
        }
    }
    /// Stage H iter 4: parse-error variant used by rustc_driver_impl.
    /// The lenient stub `parse` always succeeds so these variants are
    /// constructed only on the host code path.
    #[derive(Debug)]
    pub enum Fail {
        ArgumentMissing(String),
        UnrecognizedOption(String),
        OptionMissing(String),
        OptionDuplicated(String),
        UnexpectedArgument(String),
    }
    impl core::fmt::Display for Fail {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "{:?}", self)
        }
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
