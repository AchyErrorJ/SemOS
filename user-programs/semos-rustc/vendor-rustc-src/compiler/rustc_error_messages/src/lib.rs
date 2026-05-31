// tidy-alphabetical-start
#![allow(internal_features)]
#![cfg_attr(target_os = "none", no_std)]
#![feature(rustc_attrs)]
// tidy-alphabetical-end

// M27 §1.8: i18n / fluent dropped. The crate retains the public type
// names that the wider workspace consumes from `rustc_error_messages::*`
// (FluentArgs, FluentValue, FluentError, FluentBundle, FluentResource,
// FluentType, LanguageIdentifier, langid!) — see the local stub module
// at the bottom of the file — but no longer pulls in fluent-bundle /
// fluent-syntax / icu_list / icu_locale / intl-memoizer / unic-langid /
// rustc_baked_icu_data / tracing. Diagnostic messages are returned as
// English literal `Cow::Borrowed(static_str)` by rustc_errors's
// Translator (translation.rs); the loader functions
// `fluent_bundle()` and `fallback_fluent_bundle()` are kept here as
// no-ops so callers in rustc_interface / rustc_session compile
// unchanged.

#[macro_use]
extern crate alloc;

#[cfg(not(target_os = "none"))]
extern crate std;

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

use rustc_data_structures::sync::IntoDynSyncSend;
use rustc_macros::{Decodable, Encodable};
use rustc_span::Span;

// M27 §1.8: local stub types (see bottom of file) replacing the
// `fluent_bundle::*` / `unic_langid::*` public surface.
pub use self::fluent_stubs::{
    FluentArgs, FluentBundle as RawFluentBundle, FluentError, FluentResource, FluentType,
    FluentValue, LanguageIdentifier,
};

mod diagnostic_impls;
pub use diagnostic_impls::DiagArgFromDisplay;

// `Path` is host-only here — the `fluent_bundle()` loader's signature
// took `&[&Path]`, which we keep for ABI compat. On the SemOS target we
// route through `semos_std::path::Path`. The loader body is gutted (it
// always returns `Ok(None)`), but the parameter type still needs to
// resolve.
#[cfg(not(target_os = "none"))]
use std::path::Path;
#[cfg(target_os = "none")]
use semos_std::path::Path;

#[cfg(not(target_os = "none"))]
use std::io;
#[cfg(target_os = "none")]
use semos_std::io;

// `FluentBundle` is the rustc-side alias for the (formerly) `IntoDynSyncSend`-
// wrapped fluent_bundle::FluentBundle. We preserve the wrapper shape so the
// type alias is bit-identical to the upstream definition — any downstream
// `Option<Arc<FluentBundle>>` field types are unchanged.
pub type FluentBundle = IntoDynSyncSend<RawFluentBundle>;

#[derive(Debug)]
pub enum TranslationBundleError {
    /// Failed to read from `.ftl` file.
    ReadFtl(io::Error),
    /// Failed to parse contents of `.ftl` file.
    ParseFtl(/* M27 §1.8: was fluent_syntax::parser::ParserError */ String),
    /// Failed to add `FluentResource` to `FluentBundle`.
    AddResource(FluentError),
    /// `$sysroot/share/locale/$locale` does not exist.
    MissingLocale,
    /// Cannot read directory entries of `$sysroot/share/locale/$locale`.
    ReadLocalesDir(io::Error),
    /// Cannot read directory entry of `$sysroot/share/locale/$locale`.
    ReadLocalesDirEntry(io::Error),
    /// `$sysroot/share/locale/$locale` is not a directory.
    LocaleIsNotDir,
}

impl fmt::Display for TranslationBundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranslationBundleError::ReadFtl(e) => write!(f, "could not read ftl file: {e}"),
            TranslationBundleError::ParseFtl(e) => {
                write!(f, "could not parse ftl file: {e}")
            }
            TranslationBundleError::AddResource(e) => write!(f, "failed to add resource: {e}"),
            TranslationBundleError::MissingLocale => write!(f, "missing locale directory"),
            TranslationBundleError::ReadLocalesDir(e) => {
                write!(f, "could not read locales dir: {e}")
            }
            TranslationBundleError::ReadLocalesDirEntry(e) => {
                write!(f, "could not read locales dir entry: {e}")
            }
            TranslationBundleError::LocaleIsNotDir => {
                write!(f, "`$sysroot/share/locales/$locale` is not a directory")
            }
        }
    }
}

impl core::error::Error for TranslationBundleError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            TranslationBundleError::ReadFtl(e) => Some(e),
            TranslationBundleError::ParseFtl(_) => None,
            TranslationBundleError::AddResource(e) => Some(e),
            TranslationBundleError::MissingLocale => None,
            TranslationBundleError::ReadLocalesDir(e) => Some(e),
            TranslationBundleError::ReadLocalesDirEntry(e) => Some(e),
            TranslationBundleError::LocaleIsNotDir => None,
        }
    }
}

// M27 §1.8: removed `From<(FluentResource, Vec<ParserError>)>` and
// `From<Vec<FluentError>>` impls — both relied on real fluent error
// types that no longer exist. The loader fn below never produces these
// variants on SemOS (it short-circuits to Ok(None)), so the conversions
// are unreachable.

/// Returns Fluent bundle with the user's locale resources from
/// `$sysroot/share/locale/$requested_locale/*.ftl`.
///
/// On SemOS (§1.8 i18n drop) this always returns `Ok(None)`; English
/// diagnostics are baked into the generated `fluent_generated::*`
/// constants by `rustc_fluent_macro` (host-side codegen).
pub fn fluent_bundle(
    _sysroot_candidates: &[&Path],
    _requested_locale: Option<LanguageIdentifier>,
    _additional_ftl_path: Option<&Path>,
    _with_directionality_markers: bool,
) -> Result<Option<Arc<FluentBundle>>, TranslationBundleError> {
    Ok(None)
}

/// Type alias for the result of `fallback_fluent_bundle` - a reference-counted pointer to the
/// fallback fluent bundle. On SemOS (§1.8 i18n drop) we collapse the historical
/// `Arc<LazyLock<FluentBundle, Box<dyn FnOnce() -> FluentBundle + DynSend>>>` shape down to a
/// plain `Arc<FluentBundle>` — nothing reads from this on the target build.
pub type LazyFallbackBundle = Arc<FluentBundle>;

/// Return the default `FluentBundle` with standard "en-US" diagnostic messages.
/// On SemOS this returns a no-op bundle; the bytes are never read.
pub fn fallback_fluent_bundle(
    _resources: Vec<&'static str>,
    _with_directionality_markers: bool,
) -> LazyFallbackBundle {
    Arc::new(IntoDynSyncSend(RawFluentBundle::new()))
}

/// Identifier for the Fluent message/attribute corresponding to a diagnostic message.
type FluentId = Cow<'static, str>;

/// Abstraction over a message in a subdiagnostic (i.e. label, note, help, etc) to support both
/// translatable and non-translatable diagnostic messages.
///
/// Translatable messages for subdiagnostics are typically attributes attached to a larger Fluent
/// message so messages of this type must be combined with a `DiagMessage` (using
/// `DiagMessage::with_subdiagnostic_message`) before rendering. However, subdiagnostics from
/// the `Subdiagnostic` derive refer to Fluent identifiers directly.
#[rustc_diagnostic_item = "SubdiagMessage"]
pub enum SubdiagMessage {
    /// Non-translatable diagnostic message.
    Str(Cow<'static, str>),
    /// Identifier of a Fluent message. Instances of this variant are generated by the
    /// `Subdiagnostic` derive.
    FluentIdentifier(FluentId),
    /// Attribute of a Fluent message. Needs to be combined with a Fluent identifier to produce an
    /// actual translated message. Instances of this variant are generated by the `fluent_messages`
    /// macro.
    ///
    /// <https://projectfluent.org/fluent/guide/attributes.html>
    FluentAttr(FluentId),
}

impl From<String> for SubdiagMessage {
    fn from(s: String) -> Self {
        SubdiagMessage::Str(Cow::Owned(s))
    }
}
impl From<&'static str> for SubdiagMessage {
    fn from(s: &'static str) -> Self {
        SubdiagMessage::Str(Cow::Borrowed(s))
    }
}
impl From<Cow<'static, str>> for SubdiagMessage {
    fn from(s: Cow<'static, str>) -> Self {
        SubdiagMessage::Str(s)
    }
}

/// Abstraction over a message in a diagnostic to support both translatable and non-translatable
/// diagnostic messages.
///
/// Intended to be removed once diagnostics are entirely translatable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Encodable, Decodable)]
#[rustc_diagnostic_item = "DiagMessage"]
pub enum DiagMessage {
    /// Non-translatable diagnostic message or a message that has been translated eagerly.
    ///
    /// Some diagnostics have repeated subdiagnostics where the same interpolated variables would
    /// be instantiated multiple times with different values. These subdiagnostics' messages
    /// are translated when they are added to the parent diagnostic. This is one of the ways
    /// this variant of `DiagMessage` is produced.
    Str(Cow<'static, str>),
    /// Identifier for a Fluent message (with optional attribute) corresponding to the diagnostic
    /// message. Yet to be translated.
    ///
    /// <https://projectfluent.org/fluent/guide/hello.html>
    /// <https://projectfluent.org/fluent/guide/attributes.html>
    FluentIdentifier(FluentId, Option<FluentId>),
}

impl DiagMessage {
    /// Given a `SubdiagMessage` which may contain a Fluent attribute, create a new
    /// `DiagMessage` that combines that attribute with the Fluent identifier of `self`.
    ///
    /// - If the `SubdiagMessage` is non-translatable then return the message as a `DiagMessage`.
    /// - If `self` is non-translatable then return `self`'s message.
    pub fn with_subdiagnostic_message(&self, sub: SubdiagMessage) -> Self {
        let attr = match sub {
            SubdiagMessage::Str(s) => return DiagMessage::Str(s),
            SubdiagMessage::FluentIdentifier(id) => {
                return DiagMessage::FluentIdentifier(id, None);
            }
            SubdiagMessage::FluentAttr(attr) => attr,
        };

        match self {
            DiagMessage::Str(s) => DiagMessage::Str(s.clone()),
            DiagMessage::FluentIdentifier(id, _) => {
                DiagMessage::FluentIdentifier(id.clone(), Some(attr))
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            DiagMessage::Str(s) => Some(s),
            DiagMessage::FluentIdentifier(_, _) => None,
        }
    }
}

impl From<String> for DiagMessage {
    fn from(s: String) -> Self {
        DiagMessage::Str(Cow::Owned(s))
    }
}
impl From<&'static str> for DiagMessage {
    fn from(s: &'static str) -> Self {
        DiagMessage::Str(Cow::Borrowed(s))
    }
}
impl From<Cow<'static, str>> for DiagMessage {
    fn from(s: Cow<'static, str>) -> Self {
        DiagMessage::Str(s)
    }
}

/// Translating *into* a subdiagnostic message from a diagnostic message is a little strange - but
/// the subdiagnostic functions (e.g. `span_label`) take a `SubdiagMessage` and the
/// subdiagnostic derive refers to typed identifiers that are `DiagMessage`s, so need to be
/// able to convert between these, as much as they'll be converted back into `DiagMessage`
/// using `with_subdiagnostic_message` eventually. Don't use this other than for the derive.
impl From<DiagMessage> for SubdiagMessage {
    fn from(val: DiagMessage) -> Self {
        match val {
            DiagMessage::Str(s) => SubdiagMessage::Str(s),
            DiagMessage::FluentIdentifier(id, None) => SubdiagMessage::FluentIdentifier(id),
            // There isn't really a sensible behaviour for this because it loses information but
            // this is the most sensible of the behaviours.
            DiagMessage::FluentIdentifier(_, Some(attr)) => SubdiagMessage::FluentAttr(attr),
        }
    }
}

/// A span together with some additional data.
#[derive(Clone, Debug)]
pub struct SpanLabel {
    /// The span we are going to include in the final snippet.
    pub span: Span,

    /// Is this a primary span? This is the "locus" of the message,
    /// and is indicated with a `^^^^` underline, versus `----`.
    pub is_primary: bool,

    /// What label should we attach to this span (if any)?
    pub label: Option<DiagMessage>,
}

/// A collection of `Span`s.
///
/// Spans have two orthogonal attributes:
///
/// - They can be *primary spans*. In this case they are the locus of
///   the error, and would be rendered with `^^^`.
/// - They can have a *label*. In this case, the label is written next
///   to the mark in the snippet when we render.
#[derive(Clone, Debug, Hash, PartialEq, Eq, Encodable, Decodable)]
pub struct MultiSpan {
    primary_spans: Vec<Span>,
    span_labels: Vec<(Span, DiagMessage)>,
}

impl MultiSpan {
    #[inline]
    pub fn new() -> MultiSpan {
        MultiSpan { primary_spans: vec![], span_labels: vec![] }
    }

    pub fn from_span(primary_span: Span) -> MultiSpan {
        MultiSpan { primary_spans: vec![primary_span], span_labels: vec![] }
    }

    pub fn from_spans(mut vec: Vec<Span>) -> MultiSpan {
        vec.sort();
        MultiSpan { primary_spans: vec, span_labels: vec![] }
    }

    pub fn push_span_label(&mut self, span: Span, label: impl Into<DiagMessage>) {
        self.span_labels.push((span, label.into()));
    }

    /// Selects the first primary span (if any).
    pub fn primary_span(&self) -> Option<Span> {
        self.primary_spans.first().cloned()
    }

    /// Returns all primary spans.
    pub fn primary_spans(&self) -> &[Span] {
        &self.primary_spans
    }

    /// Returns `true` if any of the primary spans are displayable.
    pub fn has_primary_spans(&self) -> bool {
        !self.is_dummy()
    }

    /// Returns `true` if this contains only a dummy primary span with any hygienic context.
    pub fn is_dummy(&self) -> bool {
        self.primary_spans.iter().all(|sp| sp.is_dummy())
    }

    /// Replaces all occurrences of one Span with another. Used to move `Span`s in areas that don't
    /// display well (like std macros). Returns whether replacements occurred.
    pub fn replace(&mut self, before: Span, after: Span) -> bool {
        let mut replacements_occurred = false;
        for primary_span in &mut self.primary_spans {
            if *primary_span == before {
                *primary_span = after;
                replacements_occurred = true;
            }
        }
        for span_label in &mut self.span_labels {
            if span_label.0 == before {
                span_label.0 = after;
                replacements_occurred = true;
            }
        }
        replacements_occurred
    }

    /// Returns the strings to highlight. We always ensure that there
    /// is an entry for each of the primary spans -- for each primary
    /// span `P`, if there is at least one label with span `P`, we return
    /// those labels (marked as primary). But otherwise we return
    /// `SpanLabel` instances with empty labels.
    pub fn span_labels(&self) -> Vec<SpanLabel> {
        let is_primary = |span| self.primary_spans.contains(&span);

        let mut span_labels = self
            .span_labels
            .iter()
            .map(|&(span, ref label)| SpanLabel {
                span,
                is_primary: is_primary(span),
                label: Some(label.clone()),
            })
            .collect::<Vec<_>>();

        for &span in &self.primary_spans {
            if !span_labels.iter().any(|sl| sl.span == span) {
                span_labels.push(SpanLabel { span, is_primary: true, label: None });
            }
        }

        span_labels
    }

    /// Returns `true` if any of the span labels is displayable.
    pub fn has_span_labels(&self) -> bool {
        self.span_labels.iter().any(|(sp, _)| !sp.is_dummy())
    }

    /// Clone this `MultiSpan` without keeping any of the span labels - sometimes a `MultiSpan` is
    /// to be re-used in another diagnostic, but includes `span_labels` which have translated
    /// messages. These translated messages would fail to translate without their diagnostic
    /// arguments which are unlikely to be cloned alongside the `Span`.
    pub fn clone_ignoring_labels(&self) -> Self {
        Self { primary_spans: self.primary_spans.clone(), ..MultiSpan::new() }
    }
}

impl From<Span> for MultiSpan {
    fn from(span: Span) -> MultiSpan {
        MultiSpan::from_span(span)
    }
}

impl From<Vec<Span>> for MultiSpan {
    fn from(spans: Vec<Span>) -> MultiSpan {
        MultiSpan::from_spans(spans)
    }
}

// M27 §1.8: simplified — was a fluent-aware list formatter that used
// icu_list + intl_memoizer to localize the "a, b, and c" join. Now
// just concatenates the strings with "and"-separation in English.
pub fn fluent_value_from_str_list_sep_by_and(l: Vec<Cow<'_, str>>) -> FluentValue<'_> {
    let mut buf = String::new();
    let n = l.len();
    for (i, s) in l.iter().enumerate() {
        if i > 0 {
            if i == n - 1 {
                if n == 2 {
                    buf.push_str(" and ");
                } else {
                    buf.push_str(", and ");
                }
            } else {
                buf.push_str(", ");
            }
        }
        buf.push_str(s);
    }
    FluentValue::Str(Cow::Owned(buf))
}

/// Simplified version of `FluentArg` that can implement `Encodable` and `Decodable`. Collection of
/// `DiagArg` are converted to `FluentArgs` (consuming the collection) at the start of diagnostic
/// emission.
pub type DiagArg<'iter> = (&'iter DiagArgName, &'iter DiagArgValue);

/// Name of a diagnostic argument.
pub type DiagArgName = Cow<'static, str>;

/// Simplified version of `FluentValue` that can implement `Encodable` and `Decodable`. Converted
/// to a `FluentValue` by the emitter to be used in diagnostic translation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Encodable, Decodable)]
pub enum DiagArgValue {
    Str(Cow<'static, str>),
    // This gets converted to a `FluentNumber`, which is an `f64`. An `i32`
    // safely fits in an `f64`. Any integers bigger than that will be converted
    // to strings in `into_diag_arg` and stored using the `Str` variant.
    Number(i32),
    StrListSepByAnd(Vec<Cow<'static, str>>),
}

/// Converts a value of a type into a `DiagArg` (typically a field of an `Diag` struct).
/// Implemented as a custom trait rather than `From` so that it is implemented on the type being
/// converted rather than on `DiagArgValue`, which enables types from other `rustc_*` crates to
/// implement this.
// M27 R4 B5 TODO(Phase 4/5): IntoDiagArg's `path: &mut Option<std::path::PathBuf>`
// parameter is inconsistent with all 4+ impl sites (rustc_hir, rustc_errors, rustc_middle,
// rustc_borrowck, rustc_const_eval, rustc_trait_selection, rustc_hir_typeck) which use
// `semos_std::path::PathBuf`. The trait param stays on `std::path::PathBuf` on the host build
// and is cfg-split to `semos_std::path::PathBuf` on the SemOS target via the
// `__rustc_error_messages_PathBuf` alias defined below. All downstream impl sites use that
// alias so they don't need their own cfg gates.
#[cfg(not(target_os = "none"))]
pub use std::path::PathBuf as __IntoDiagArgPathBuf;
#[cfg(target_os = "none")]
pub use semos_std::path::PathBuf as __IntoDiagArgPathBuf;

pub trait IntoDiagArg {
    /// Convert `Self` into a `DiagArgValue` suitable for rendering in a diagnostic.
    ///
    /// It takes a `path` where "long values" could be written to, if the `DiagArgValue` is too big
    /// for displaying on the terminal. This path comes from the `Diag` itself. When rendering
    /// values that come from `TyCtxt`, like `Ty<'_>`, they can use `TyCtxt::short_string`. If a
    /// value has no shortening logic that could be used, the argument can be safely ignored.
    fn into_diag_arg(self, path: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue;
}

impl IntoDiagArg for DiagArgValue {
    fn into_diag_arg(self, _: &mut Option<__IntoDiagArgPathBuf>) -> DiagArgValue {
        self
    }
}

impl From<DiagArgValue> for FluentValue<'static> {
    fn from(val: DiagArgValue) -> Self {
        match val {
            DiagArgValue::Str(s) => FluentValue::Str(s),
            DiagArgValue::Number(n) => FluentValue::Number(n as f64),
            DiagArgValue::StrListSepByAnd(l) => {
                // l: Vec<Cow<'static, str>> — the helper takes Vec<Cow<'a, str>>
                // and returns FluentValue<'a>. 'static fits 'a here.
                fluent_value_from_str_list_sep_by_and(l)
            }
        }
    }
}

// =====================================================================
// M27 §1.8 — local stub types
// =====================================================================
//
// The `fluent_stubs` module supplies replacement types for the
// previously-re-exported fluent-bundle / unic-langid surface. The shapes
// are intentionally minimal: just enough to satisfy the callers we
// audited (rustc_errors, rustc_session, rustc_interface, rustc_middle,
// rustc_hir, rustc_target, rustc_lint_defs, rustc_type_ir).
//
// Callers never construct these directly except via the public
// constructors below (FluentArgs::new, FluentArgs::with_capacity,
// FluentValue::from(&str|String|i32), LanguageIdentifier::from_str).
//
// On the SemOS target the loader bodies short-circuit (see
// `fluent_bundle()` above), so the dummy bodies never get exercised in
// the diagnostic-emission hot path.
mod fluent_stubs {
    use alloc::borrow::Cow;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use core::fmt;
    use core::str::FromStr;

    use rustc_macros::{Decodable, Encodable};

    /// Stub replacing `fluent_bundle::FluentValue`. Only the variants that
    /// the surviving rustc-side call sites need; `Custom`/`None`/`Error`
    /// from upstream are dropped along with the fluent backend.
    #[derive(Clone, Debug, PartialEq)]
    pub enum FluentValue<'a> {
        Str(Cow<'a, str>),
        Number(f64),
    }

    impl<'a> FluentValue<'a> {
        pub fn into_owned(self) -> FluentValue<'static> {
            match self {
                FluentValue::Str(Cow::Borrowed(s)) => FluentValue::Str(Cow::Owned(s.to_string())),
                FluentValue::Str(Cow::Owned(s)) => FluentValue::Str(Cow::Owned(s)),
                FluentValue::Number(n) => FluentValue::Number(n),
            }
        }
    }

    impl<'a> fmt::Display for FluentValue<'a> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                FluentValue::Str(s) => f.write_str(s),
                FluentValue::Number(n) => write!(f, "{n}"),
            }
        }
    }

    impl<'a> From<&'a str> for FluentValue<'a> {
        fn from(s: &'a str) -> Self {
            FluentValue::Str(Cow::Borrowed(s))
        }
    }
    impl<'a> From<Cow<'a, str>> for FluentValue<'a> {
        fn from(s: Cow<'a, str>) -> Self {
            FluentValue::Str(s)
        }
    }
    impl From<String> for FluentValue<'static> {
        fn from(s: String) -> Self {
            FluentValue::Str(Cow::Owned(s))
        }
    }
    impl From<i32> for FluentValue<'static> {
        fn from(n: i32) -> Self {
            FluentValue::Number(n as f64)
        }
    }
    impl From<f64> for FluentValue<'static> {
        fn from(n: f64) -> Self {
            FluentValue::Number(n)
        }
    }

    /// Stub replacing `fluent_bundle::FluentArgs`. Backed by a `Vec` of
    /// (name, value) pairs. The only consumer (rustc_errors's
    /// to_fluent_args + Translator's no-op translate_message) needs
    /// `new`, `with_capacity`, `set`, and `IntoIterator`-by-reference.
    #[derive(Clone, Debug, Default)]
    pub struct FluentArgs<'a> {
        entries: Vec<(Cow<'a, str>, FluentValue<'a>)>,
    }

    impl<'a> FluentArgs<'a> {
        pub fn new() -> Self {
            Self { entries: Vec::new() }
        }

        pub fn with_capacity(cap: usize) -> Self {
            Self { entries: Vec::with_capacity(cap) }
        }

        pub fn set<K, V>(&mut self, key: K, value: V)
        where
            K: Into<Cow<'a, str>>,
            V: Into<FluentValue<'a>>,
        {
            self.entries.push((key.into(), value.into()));
        }

        pub fn get(&self, key: &str) -> Option<&FluentValue<'a>> {
            self.entries.iter().find_map(|(k, v)| if k == key { Some(v) } else { None })
        }

        pub fn iter(&self) -> core::slice::Iter<'_, (Cow<'a, str>, FluentValue<'a>)> {
            self.entries.iter()
        }
    }

    impl<'a> IntoIterator for FluentArgs<'a> {
        type Item = (Cow<'a, str>, FluentValue<'a>);
        type IntoIter = alloc::vec::IntoIter<Self::Item>;
        fn into_iter(self) -> Self::IntoIter {
            self.entries.into_iter()
        }
    }

    impl<'a, 'b> IntoIterator for &'b FluentArgs<'a> {
        type Item = &'b (Cow<'a, str>, FluentValue<'a>);
        type IntoIter = core::slice::Iter<'b, (Cow<'a, str>, FluentValue<'a>)>;
        fn into_iter(self) -> Self::IntoIter {
            self.entries.iter()
        }
    }

    /// Stub replacing `fluent_bundle::FluentError`. Never constructed on
    /// the SemOS target; kept for API compat (rustc_errors::error.rs
    /// stores `Vec<FluentError>` in a variant that is unreachable post-§1.8).
    #[derive(Clone, Debug)]
    pub enum FluentError {
        // Empty: variants are unreachable on the SemOS target.
        Unreachable,
    }

    impl fmt::Display for FluentError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("fluent error (unreachable on SemOS)")
        }
    }

    impl core::error::Error for FluentError {}

    /// Stub replacing `fluent_bundle::FluentResource`. Unit struct — the
    /// loader never constructs one on SemOS.
    #[derive(Debug)]
    pub struct FluentResource;

    /// Stub replacing the `IntoDynSyncSend<fluent_bundle::FluentBundle<...>>`
    /// inner type. The outer type alias `pub type FluentBundle =
    /// IntoDynSyncSend<RawFluentBundle>` at the crate root preserves the
    /// historical wrapper shape so `Option<Arc<FluentBundle>>` in
    /// rustc_session is bit-identical.
    #[derive(Debug)]
    pub struct FluentBundle;

    impl FluentBundle {
        pub fn new() -> Self {
            FluentBundle
        }
    }

    /// Stub replacing `fluent_bundle::types::FluentType`. No impls exist
    /// outside this crate today; kept for `pub use` ABI compat.
    pub trait FluentType {}

    /// Stub replacing `unic_langid::LanguageIdentifier`. Stores the raw
    /// string form (e.g. "en-US"). Used in rustc_session as the type of
    /// the `translate_lang` option; `FromStr` is the only constructor
    /// touched by call sites.
    #[derive(Clone, Debug, PartialEq, Eq, Hash, Encodable, Decodable)]
    pub struct LanguageIdentifier {
        tag: String,
    }

    impl LanguageIdentifier {
        pub fn new(tag: impl Into<String>) -> Self {
            Self { tag: tag.into() }
        }
    }

    impl fmt::Display for LanguageIdentifier {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.tag)
        }
    }

    impl FromStr for LanguageIdentifier {
        type Err = LanguageIdentifierError;
        fn from_str(s: &str) -> Result<Self, Self::Err> {
            // Minimal validation: a tag must be ASCII and non-empty. The
            // upstream parser was much more elaborate (RFC 4647) but the
            // only call site (rustc_session) just round-trips an opaque
            // string into the diagnostic emitter, which then does nothing
            // with it on SemOS.
            if s.is_empty() || !s.is_ascii() {
                return Err(LanguageIdentifierError);
            }
            Ok(Self { tag: s.to_string() })
        }
    }

    #[derive(Debug)]
    pub struct LanguageIdentifierError;

    impl fmt::Display for LanguageIdentifierError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("invalid language identifier")
        }
    }

    impl core::error::Error for LanguageIdentifierError {}

}

/// `langid!` macro stub. The upstream `unic_langid::langid!` did a
/// compile-time parse of a literal tag into a `LanguageIdentifier`;
/// the only call site that survives is `langid!("en-US")` in
/// `rustc_fluent_macro` (which is a HOST proc-macro, unaffected) and
/// the gated `rustc_errors/src/tests.rs` (also host-only). We provide
/// a runtime variant that returns `LanguageIdentifier::from_str(...)
/// .unwrap()` so any stray on-target use compiles, even if it would
/// be a panic at runtime.
#[macro_export]
macro_rules! langid {
    ($tag:literal) => {{
        // Build via FromStr to avoid taking a dep on a const parser.
        <$crate::LanguageIdentifier as ::core::str::FromStr>::from_str($tag)
            .expect("invalid langid literal")
    }};
}
