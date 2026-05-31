// M27 §1.8: i18n / fluent dropped.
//
// The original Translator drove a fluent-bundle (rustc_error_messages::
// FluentBundle / LazyFallbackBundle) and resolved DiagMessage::Fluent-
// Identifier(id, attr) by looking up the identifier in the loaded .ftl
// resources for the active locale, falling back to the compile-time
// English bundle on miss.
//
// Per plan §1.8, SemOS hard-codes English. The new Translator keeps the
// exact same public shape (so callers in rustc_errors / downstream crates
// compile unchanged) but does NOT touch fluent_bundle at all: every
// DiagMessage::Str is returned as-is, and every DiagMessage::Fluent-
// Identifier(id, _) is returned as its identifier string (which the
// host-side rustc_fluent_macro will resolve to the English template
// content during its build-time codegen — those constants come out of
// `crate::fluent_generated::*` as `Cow::Borrowed(static_str)`).
//
// The Arc<FluentBundle> + LazyFallbackBundle fields remain in the struct
// to preserve the field names that some downstream code (e.g.
// rustc_interface::interface::Compiler / rustc_session) may construct.
// They are NEVER read on the SemOS target. We keep them as Option/the
// rustc_error_messages-provided types so the layout is identical.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub use rustc_error_messages::{FluentArgs, LazyFallbackBundle};

use crate::error::TranslateError;
use crate::{DiagArg, DiagMessage, FluentBundle};

/// Convert diagnostic arguments (a rustc internal type that exists to implement
/// `Encodable`/`Decodable`) into `FluentArgs` which is necessary to perform translation.
///
/// Typically performed once for each diagnostic at the start of `emit_diagnostic` and then
/// passed around as a reference thereafter.
pub fn to_fluent_args<'iter>(iter: impl Iterator<Item = DiagArg<'iter>>) -> FluentArgs<'static> {
    let mut args = if let Some(size) = iter.size_hint().1 {
        FluentArgs::with_capacity(size)
    } else {
        FluentArgs::new()
    };

    for (k, v) in iter {
        args.set(k.clone(), v.clone());
    }

    args
}

#[derive(Clone)]
pub struct Translator {
    /// Kept for API compat; ignored on SemOS (i18n dropped).
    pub fluent_bundle: Option<Arc<FluentBundle>>,
    /// Kept for API compat; ignored on SemOS.
    pub fallback_fluent_bundle: LazyFallbackBundle,
}

impl Translator {
    pub fn with_fallback_bundle(
        resources: Vec<&'static str>,
        with_directionality_markers: bool,
    ) -> Translator {
        Translator {
            fluent_bundle: None,
            fallback_fluent_bundle: crate::fallback_fluent_bundle(
                resources,
                with_directionality_markers,
            ),
        }
    }

    /// Convert `DiagMessage`s to a string, performing translation if necessary.
    pub fn translate_messages(
        &self,
        messages: &[(DiagMessage, crate::Style)],
        args: &FluentArgs<'_>,
    ) -> Cow<'_, str> {
        Cow::Owned(
            messages
                .iter()
                .map(|(m, _)| self.translate_message(m, args).unwrap_or(Cow::Borrowed("")))
                .collect::<String>(),
        )
    }

    /// Convert a `DiagMessage` to a string. On SemOS this is a no-op pass-through:
    /// `Str` is returned as-is; `FluentIdentifier` is returned as the identifier text
    /// itself. The hardcoded-English fluent_generated constants downstream are already
    /// resolved at build-time by rustc_fluent_macro, so most calls won't even reach
    /// the `FluentIdentifier` arm with a non-pre-resolved identifier.
    pub fn translate_message<'a>(
        &'a self,
        message: &'a DiagMessage,
        _args: &'a FluentArgs<'_>,
    ) -> Result<Cow<'a, str>, TranslateError<'a>> {
        match message {
            DiagMessage::Str(msg) => Ok(Cow::Borrowed(msg)),
            DiagMessage::FluentIdentifier(id, _attr) => {
                // M27 §1.8: with i18n dropped we surface the identifier string itself.
                // This is acceptable degraded output (e.g. "errors_delayed_at_with_newline")
                // and never errs. Argument-substitution placeholders ("{$var}") inside the
                // English fluent templates are NOT expanded — that's the documented price
                // of dropping fluent. Callers that need real text construct DiagMessage::Str
                // explicitly (rustc emits both forms).
                Ok(Cow::Borrowed(id.as_ref()))
            }
        }
    }
}
