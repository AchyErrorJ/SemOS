//! `rustc_fluent_macro::fluent_messages!` — M27 §1.8 STUB.
//!
//! Per M27_RUSTC_PORT_PLAN.md §1.8 we drop fluent i18n entirely.
//! Upstream this file (a) parsed the .ftl file via fluent-syntax,
//! (b) validated message syntax with annotate-snippets diagnostics,
//! (c) emitted `pub static DEFAULT_LOCALE_RESOURCE` + a
//! `mod fluent_generated` of `DiagMessage` constants.
//!
//! On SemOS we only need the macro to emit ENOUGH code that callers
//! who reference `crate::fluent_generated::<msg>` compile. B3 (Phase
//! 2b) gutted rustc_errors's Translator into a passthrough returning
//! `Cow::Borrowed(static_str)` — the actual runtime translation is
//! dead.
//!
//! We don't parse the .ftl file, so we don't know the message names.
//! We emit a minimal stub. Downstream call sites that reach into
//! `fluent_generated::<msg>` will hit a name-resolution error — that's
//! the documented "one error per compile" v1 limitation (§1.9
//! FatalError → abort). Real diagnostics go through B3's passthrough.

use proc_macro::TokenStream;

pub(crate) fn fluent_messages(_input: TokenStream) -> TokenStream {
    r#"
        pub static DEFAULT_LOCALE_RESOURCE: &str = "";
        pub mod fluent_generated {
            // Empty — M27 §1.8 stub (see fluent.rs).
        }
    "#
    .parse()
    .unwrap_or_default()
}
