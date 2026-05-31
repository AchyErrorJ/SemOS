# H1 — §1.8 i18n drop (rustc_error_messages stubs)

**Date:** 2026-05-31
**Phase:** 5b — Stage E iteration 2 (transitive-dep wall closer)
**Assigned crates / files:** `compiler/rustc_error_messages/` (Cargo.toml +
2 source files; total ~640 LOC). Verification sweep across rustc_errors,
rustc_session, rustc_interface, rustc_hir, rustc_target, rustc_middle,
rustc_lint_defs, rustc_type_ir for re-exports/imports.
**Status:** IN PROGRESS
**Token cost (self-report):** filled at end.
**Source LOC patched:** filled at end.

## 0. Context recap (why this exists)

Stage D's first `cargo check` fell over because rustc_error_messages
still listed `fluent-bundle`, `fluent-syntax`, `intl-memoizer`,
`icu_list`, `icu_locale`, `unic-langid`, `rustc_baked_icu_data` and
`tracing` as direct deps. Those crates transitively drag in `once_cell`
(245 errors), `log` (100), `stable_deref_trait` (17),
`rustc-stable-hash` (31), `regex-syntax` (1) — none of which are
no_std-clean. The B3 + B3-followup work on rustc_errors already gutted
the LOGIC that read fluent bundles, but the ABI types (FluentArgs,
FluentBundle, FluentValue, FluentError, LanguageIdentifier, langid!,
FluentResource, FluentType) were still re-exported FROM
rustc_error_messages by `pub use fluent_bundle::*;` /
`pub use unic_langid::*;` lines. As a result the deps stayed on the
Cargo.toml.

This handoff completes §1.8 at the Cargo.toml + source level:

1. Drop the seven external deps + `tracing` from
   `rustc_error_messages/Cargo.toml`.
2. Apply D1 cfg_attr pattern to `lib.rs`; gate the host-only fluent
   fallback path with `cfg(not(target_os = "none"))`.
3. Provide LOCAL stub types (same names) for every fluent_bundle /
   unic_langid type the wider workspace consumes via
   `rustc_error_messages::*`. No external crate is touched —
   downstream crates keep their existing import lines.

The B3 design intent (KEEP the Translator surface, gut the bodies) is
preserved exactly. This patch is purely the Cargo-level + stub-types
finish that B3 deferred.

## 1. Per-file diff summary (planned; updated as edits land)

| File | LOC | Changes | Markers |
|------|----:|---------|---------|
| `Cargo.toml` | 22 | Drop fluent-bundle, fluent-syntax, icu_list, icu_locale, intl-memoizer, rustc_baked_icu_data, tracing, unic-langid. Keep rustc_ast / rustc_ast_pretty / rustc_data_structures / rustc_macros / rustc_serialize / rustc_span. Add `[workspace] members = []` header per RECIPE §1.1. | M27 §1.8 |
| `src/lib.rs` | ~628 | D1 cfg_attr pattern; replace fluent imports with local stub types (FluentArgs, FluentValue, FluentError, FluentBundle, FluentResource, FluentType, LanguageIdentifier, langid!). Gut `fluent_bundle()` (always returns `Ok(None)`). Gut `fallback_fluent_bundle()` (returns a unit LazyFallbackBundle). Drop `fluent_value_from_str_list_sep_by_and` complex body (becomes simple FluentValue::Custom with String join). Drop `icu_locale_from_unic_langid` helper. Drop tracing::{instrument,trace}. std::* → core/alloc per RECIPE §1.3. | M27 §1.8 |
| `src/diagnostic_impls.rs` | ~206 | D1 cfg_attr (extern crate alloc). std::* → core/alloc. `std::path::PathBuf` cfg-split per RECIPE §1.3 (host: std::path, target: semos_std::path). `std::backtrace::Backtrace` cfg-split (target uses a local empty-shim type — already pattern from rustc_errors lib.rs). `std::process::ExitStatus` cfg-split. `std::error::Error` → `core::error::Error`. `std::ffi::CString` cfg-split (alloc::ffi::CString exists since 1.64 stable). `std::num::ParseIntError` → `core::num::ParseIntError`. | M27 R4 B5, M27 §1.8 |

## 2. Stub types catalog (the §1.8 ABI promise)

The local stubs replace these external crate exports. Each stub has
the same public name + same constructor signatures used at the call
sites we audited.

| Stub | Replaces | Why callers need it |
|------|----------|---------------------|
| `FluentValue<'a>` | `fluent_bundle::FluentValue` | `From<DiagArgValue>` impl, `From<&str>`, `From<String>`, `From<i32>` |
| `FluentArgs<'a>` | `fluent_bundle::FluentArgs` | `.new()`, `.with_capacity(n)`, `.set(k, v)`, `IntoIterator` |
| `FluentError` | `fluent_bundle::FluentError` | Stored in `error.rs::TranslateErrorKind::Fluent { errs }` |
| `FluentBundle` (type alias) | `IntoDynSyncSend<fluent_bundle::FluentBundle<FluentResource, IntlLangMemoizer>>` | `Option<Arc<FluentBundle>>` field in Translator |
| `FluentResource` | `fluent_bundle::FluentResource` | Used in type alias only; no constructor in our scope |
| `FluentType` | `fluent_bundle::types::FluentType` | Trait re-exported but never impl'd outside this crate |
| `LanguageIdentifier` | `unic_langid::LanguageIdentifier` | `FromStr`, `Hash`, `PartialEq`, `Eq`, `Clone`, `Debug` — used in rustc_session options |
| `langid!` | `unic_langid::langid!` | One call site (`langid!("en-US")`) inside this crate |

All stubs are `pub` from `rustc_error_messages::*`. Downstream:

- `rustc_errors::lib.rs:112-116` keeps `pub use rustc_error_messages::{… FluentBundle, LanguageIdentifier, LazyFallbackBundle, …};`.
- `rustc_errors::translation.rs:29` keeps `pub use rustc_error_messages::{FluentArgs, LazyFallbackBundle};`.
- `rustc_errors::error.rs:13` keeps `use rustc_error_messages::{FluentArgs, FluentError};`.
- `rustc_session::options.rs:1005` keeps `rustc_errors::LanguageIdentifier::from_str(s).ok();`.
- `rustc_session::session.rs:965` keeps `Option<Arc<rustc_errors::FluentBundle>>` as a parameter type.

All those compile against the stubs.

## 3. Decisions made (architectural)

(updated as work lands)

- **STUB shape is opaque-as-possible.** FluentResource is a unit
  struct, FluentBundle is a generic single-tuple wrapper that takes
  one type parameter (matching the upstream
  `FluentBundle<FluentResource, IntlLangMemoizer>` shape via type
  alias collapse). Callers never construct FluentBundle directly
  (B3 + Translator-builder both go through the rustc_error_messages
  `new_bundle`/`fallback_fluent_bundle` helpers, which we keep but
  reimplement as no-op constructors).

(more decisions logged as I edit each file)

## 4. Deferred work, line-precise

(filled at end if anything ships unfinished)

---

## Progress checkpoint 1 — Cargo.toml + lib.rs + diagnostic_impls.rs landed

### `compiler/rustc_error_messages/Cargo.toml`
- Stripped 8 deps: `fluent-bundle`, `fluent-syntax`, `icu_list`, `icu_locale`, `intl-memoizer`, `rustc_baked_icu_data`, `tracing`, `unic-langid`.
- Kept the 6 internal deps (rustc_ast, rustc_ast_pretty, rustc_data_structures, rustc_macros, rustc_serialize, rustc_span).
- Added `[workspace] members = []` header per RECIPE §1.1.
- Total drop: **8 external/internal deps removed**.

### `compiler/rustc_error_messages/src/lib.rs`
- Applied D1 `#![cfg_attr(target_os = "none", no_std)]` pattern (RECIPE §1.2).
- `extern crate alloc;` (macro_use), `extern crate std;` (host only).
- Replaced top-of-file fluent imports with `pub use self::fluent_stubs::{...}`.
- Cfg-split `std::path::Path` / `std::io` → `semos_std::path::Path` / `semos_std::io` per RECIPE §1.3.
- `fluent_bundle()` loader body gutted: always returns `Ok(None)`.
- `fallback_fluent_bundle()` returns `Arc::new(IntoDynSyncSend(RawFluentBundle::new()))`.
- `LazyFallbackBundle` simplified to `Arc<FluentBundle>` (was `Arc<LazyLock<FluentBundle, Box<dyn FnOnce()->FluentBundle+DynSend>>>`).
- `fluent_value_from_str_list_sep_by_and` reimplemented as a pure string-join (English "a, b, and c") — was an icu_list + intl_memoizer Custom-FluentValue.
- Removed `register_functions` (was a fluent-bundle function registrar).
- Removed `icu_locale_from_unic_langid`.
- Removed `From<(FluentResource, Vec<ParserError>)>` and `From<Vec<FluentError>>` impls on `TranslationBundleError` — both relied on real fluent error types.
- `TranslationBundleError::ParseFtl(ParserError)` → `ParseFtl(String)` (the parser error type is gone; the variant is unreachable on SemOS anyway).
- `IntoDiagArg::into_diag_arg`'s `path: &mut Option<std::path::PathBuf>` → `path: &mut Option<__IntoDiagArgPathBuf>` where the alias is cfg-split (host: `std::path::PathBuf`, target: `semos_std::path::PathBuf`). This unblocks the inconsistency between trait def and impl sites — all 30+ impl sites in rustc_* already use `semos_std::path::PathBuf` on the SemOS target.
- Added inner `mod fluent_stubs { ... }` providing local replacements:
  - `FluentValue<'a>` enum (Str + Number variants, Display, From for &str/String/i32/f64/Cow<str>, `into_owned` helper)
  - `FluentArgs<'a>` struct (Vec-backed; new/with_capacity/set/get/iter/IntoIterator for both by-ref and by-value)
  - `FluentError` (single `Unreachable` variant; Display + Error)
  - `FluentResource` (unit struct)
  - `FluentBundle` (unit struct with `new()`; outer `type FluentBundle = IntoDynSyncSend<RawFluentBundle>` preserves wrapper shape)
  - `FluentType` (empty trait)
  - `LanguageIdentifier` (`tag: String` field; FromStr / Display / Hash / Encodable / Decodable / PartialEq / Eq / Clone / Debug)
  - `LanguageIdentifierError` (Display + Error)
- Added `#[macro_export] macro_rules! langid!` — runtime-validates via `LanguageIdentifier::from_str` (was a unic_langid compile-time macro). The only target-side users would be host-only test modules already gated by `cfg(not(target_os = "none"))`.

### `compiler/rustc_error_messages/src/diagnostic_impls.rs`
- D1 pattern via parent crate's `#![cfg_attr]`. Removed the top-of-file `use std::{backtrace::Backtrace, path::Path, path::PathBuf, process::ExitStatus, error::Error, num::ParseIntError, io};` block; cfg-split each (RECIPE §1.3).
- `Backtrace` becomes a local stub on the SemOS target (mirrors rustc_errors's approach).
- `ExitStatus` becomes a local unit type on the SemOS target.
- `CString` from `alloc::ffi::CString` (stable since 1.64).
- `Box<dyn std::error::Error>` → `Box<dyn core::error::Error>`.
- `std::num::NonZero<u32>` → `core::num::NonZero<u32>`.
- Trait impl signatures rewritten from `&mut Option<std::path::PathBuf>` → `&mut Option<__IntoDiagArgPathBuf>` everywhere.
- Macros `into_diag_arg_using_display!` and `into_diag_arg_for_number!` updated to emit `$crate::__IntoDiagArgPathBuf` in the path parameter.

### Stub catalog summary

**8 stub types/aliases added:** `FluentValue`, `FluentArgs`, `FluentError`, `FluentResource`, `FluentBundle` (inner unit + outer type alias), `FluentType`, `LanguageIdentifier`, `LanguageIdentifierError`. Plus the `langid!` macro and the `__IntoDiagArgPathBuf` cfg-alias.

Next checkpoint: verification sweep of downstream consumers.
