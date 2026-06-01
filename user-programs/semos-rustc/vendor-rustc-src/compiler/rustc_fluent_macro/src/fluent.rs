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
use std::fs;
use std::path::PathBuf;

/// Stage F8: extended stub that actually parses the .ftl file to
/// extract message names so downstream `fluent_generated::<name>`
/// references resolve. Each name becomes a `pub const NAME:
/// DiagMessage = DiagMessage::FluentIdentifier(Cow::Borrowed("name"))`.
///
/// We don't translate or validate — the §1.8 i18n drop means runtime
/// translation is a passthrough — but we DO need every reachable name
/// to exist as a static binding so type-checking succeeds.
pub(crate) fn fluent_messages(input: TokenStream) -> TokenStream {
    // Parse the input: a single string literal naming the .ftl file
    // (relative to the calling crate's src dir).
    let input_str = input.to_string();
    let trimmed = input_str.trim().trim_matches('"');
    // The .ftl path is relative to CARGO_MANIFEST_DIR/src/.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let mut path = PathBuf::from(&manifest_dir);
    path.push("src");
    path.push(trimmed);

    // Read the .ftl file; on error emit only the empty stub.
    let contents = fs::read_to_string(&path).unwrap_or_default();

    // Extract names. Fluent message names are at the start of a line,
    // followed by ` = ` (with optional `.attribute = `). We only care
    // about top-level names (no leading whitespace, no `.attribute`).
    let mut names: Vec<String> = Vec::new();
    for line in contents.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let head = &line[..eq_pos];
            let name = head.trim();
            if name.is_empty() || name.starts_with('#') || name.contains('.') {
                continue;
            }
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();

    // Emit `pub const NAME: DiagMessage = DiagMessage::FluentIdentifier(...)`
    // for each name. `DiagMessage` and `Cow` come from rustc_error_messages,
    // which downstream callers re-export under `crate::fluent_generated`.
    let mut consts = String::new();
    for name in &names {
        consts.push_str(&format!(
            "    pub const {name}: rustc_error_messages::DiagMessage = \
             rustc_error_messages::DiagMessage::FluentIdentifier(\
             ::alloc::borrow::Cow::Borrowed(\"{name}\"), None);\n"
        ));
    }

    let out = format!(
        r#"
        pub static DEFAULT_LOCALE_RESOURCE: &str = "";
        pub mod fluent_generated {{
            // M27 §1.8 — name-only stubs parsed from messages.ftl.
{consts}
        }}
        "#
    );

    out.parse().unwrap_or_default()
}
