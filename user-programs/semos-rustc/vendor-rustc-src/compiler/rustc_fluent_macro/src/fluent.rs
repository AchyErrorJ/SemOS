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

    // Extract names. Fluent format:
    //   parent_name = main message
    //       .attribute = subdiag message
    //
    // For each parent, emit:
    //   pub const PARENT_NAME: DiagMessage = ...
    //   pub mod PARENT_NAME_subdiag { pub const ATTRIBUTE: SubdiagMessage = ...; }
    //
    // The Diagnostic derive macro references both forms.
    let mut parents: Vec<String> = Vec::new();
    let mut subdiags: Vec<(String, Vec<String>)> = Vec::new(); // (parent, [attrs])
    let mut current_parent: Option<String> = None;
    for line in contents.lines() {
        // Indented `.attr = ...` lines.
        let trimmed = line.trim_start();
        if (line.starts_with(' ') || line.starts_with('\t')) && trimmed.starts_with('.') {
            if let Some(eq_pos) = trimmed.find('=') {
                let attr = trimmed[1..eq_pos].trim();
                if !attr.is_empty()
                    && attr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && let Some(parent) = &current_parent
                {
                    // Add to last subdiag entry if it matches; else create.
                    if let Some(last) = subdiags.last_mut() {
                        if last.0 == *parent {
                            last.1.push(attr.to_string());
                            continue;
                        }
                    }
                    subdiags.push((parent.clone(), vec![attr.to_string()]));
                }
            }
            continue;
        }
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
                parents.push(name.to_string());
                current_parent = Some(name.to_string());
            }
        }
    }
    // Stage F9 ext: rustc Diagnostic derive lets authors reference a
    // sibling sub-message via `#[note(<crate_prefix>_<attr>)]` even
    // when the attr is defined nested under another message. We
    // flatten by emitting top-level consts `<crate_prefix>_<attr>`
    // for each attr seen, using the FIRST underscore-separated token
    // of any parent as the crate prefix.
    let crate_prefix: Option<String> = parents
        .first()
        .and_then(|p| p.split('_').next().map(|s| s.to_string()));
    if let Some(prefix) = &crate_prefix {
        let mut flat: Vec<String> = Vec::new();
        for (_, attrs) in &subdiags {
            for a in attrs {
                let combined = format!("{prefix}_{a}");
                if !parents.contains(&combined) && !flat.contains(&combined) {
                    flat.push(combined);
                }
            }
        }
        for f in flat {
            parents.push(f);
        }
    }
    parents.sort();
    parents.dedup();

    let mut consts = String::new();
    for name in &parents {
        consts.push_str(&format!(
            "    pub const {name}: rustc_error_messages::DiagMessage = \
             rustc_error_messages::DiagMessage::FluentIdentifier(\
             ::alloc::borrow::Cow::Borrowed(\"{name}\"), None);\n"
        ));
    }
    // Emit subdiag modules for EVERY parent. The Diagnostic derive
    // references `fluent_generated::<parent>_subdiag::<attr>` for any
    // `#[label]`/`#[help]`/`#[note]`/etc. field attribute regardless of
    // whether the .ftl defines that attr — we have to provide the
    // module shape unconditionally with a comprehensive attr set.
    let common_attrs: &[&str] = &[
        "label", "help", "note", "suggestion", "warn", "note_1", "note_2",
        "see_issue", "first_note", "second_note", "suggestion_short",
        "suggestion_verbose", "suggestion_remove", "suggestion_add",
        "remove_note", "add_note", "verbose_help", "long_help",
    ];
    let mut subdiag_mods = String::new();
    for parent in &parents {
        let mut all_attrs: Vec<String> = common_attrs.iter().map(|s| s.to_string()).collect();
        for (p, attrs) in &subdiags {
            if p == parent {
                for a in attrs {
                    if !all_attrs.contains(a) {
                        all_attrs.push(a.clone());
                    }
                }
            }
        }
        all_attrs.sort();
        all_attrs.dedup();
        let mut attr_lines = String::new();
        for attr in &all_attrs {
            attr_lines.push_str(&format!(
                "        pub const {attr}: rustc_error_messages::SubdiagMessage = \
                 rustc_error_messages::SubdiagMessage::FluentAttr(\
                 ::alloc::borrow::Cow::Borrowed(\"{attr}\"));\n"
            ));
        }
        subdiag_mods.push_str(&format!(
            "    #[allow(non_camel_case_types, non_snake_case, dead_code)]\n    pub mod {parent}_subdiag {{\n{attr_lines}    }}\n"
        ));
    }

    // Also emit a top-level `_subdiag` module. The Diagnostic derive
    // emits literal paths like `crate::fluent_generated::_subdiag::label`
    // — the `_subdiag` segment isn't substituted with the parent slug,
    // it's a literal placeholder module name in the generated code.
    let mut top_subdiag = String::new();
    let mut top_attrs: Vec<String> = common_attrs.iter().map(|s| s.to_string()).collect();
    for (_, attrs) in &subdiags {
        for a in attrs {
            if !top_attrs.contains(a) {
                top_attrs.push(a.clone());
            }
        }
    }
    for extra in &["note_once", "help_once"] {
        if !top_attrs.contains(&(*extra).to_string()) {
            top_attrs.push((*extra).to_string());
        }
    }
    top_attrs.sort();
    top_attrs.dedup();
    for attr in &top_attrs {
        top_subdiag.push_str(&format!(
            "            pub const {attr}: rustc_error_messages::SubdiagMessage = \
             rustc_error_messages::SubdiagMessage::FluentAttr(\
             ::alloc::borrow::Cow::Borrowed(\"{attr}\"));\n"
        ));
    }

    let out = format!(
        r#"
        pub static DEFAULT_LOCALE_RESOURCE: &str = "";
        pub mod fluent_generated {{
            // M27 §1.8 — name-only stubs parsed from messages.ftl.
{consts}{subdiag_mods}
            #[allow(non_camel_case_types, non_snake_case, dead_code)]
            pub mod _subdiag {{
{top_subdiag}            }}
        }}
        "#
    );

    out.parse().unwrap_or_default()
}
