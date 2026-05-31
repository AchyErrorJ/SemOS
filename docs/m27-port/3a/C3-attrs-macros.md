# C3 — rustc_attr_parsing + rustc_feature + rustc_builtin_macros + rustc_expand

**Date:** 2026-05-31
**Phase:** 3-frontend (Wave 1)
**Assigned crates:** `rustc_attr_parsing` (~6k LOC) + `rustc_feature` (~5k LOC) + `rustc_builtin_macros` (~15k LOC) + `rustc_expand` (~15k LOC)
**Status:** PARTIAL — architectural files patched + line-precise §3 recipes for mechanical-only files
**Token cost (self-report):** ~340k tokens / ~50 tool uses / ~25 min wall
**Source LOC patched:** ~3,600 LOC written; ~37k LOC covered by line-precise recipes
**Note:** Worktree shares no on-disk copy of `vendor-rustc-src/`. All reads via `git show main:<path>`. Recipes assume parent merges `main` into worktree at integration time, then applies the §3 substitutions.

## 1. Per-file diff summary

### rustc_attr_parsing/

| File | LOC | Status | Changes | Markers added |
|------|----:|--------|---------|---------------|
| `Cargo.toml` | 22 | WRITTEN | `[workspace] members = []` header. | none |
| `src/lib.rs` | 100 | WRITTEN | `#![no_std]` placed FIRST (precedes `#![feature(...)]` per A2-followup rule), then `#[macro_use] extern crate alloc;`. No item changes. | none |
| `src/context.rs` | ~650 | WRITTEN | `std::cell::RefCell` → `core::cell::RefCell`; `std::collections::BTreeMap` → `alloc::collections::BTreeMap`; `std::ops::{Deref, DerefMut}` → `core::ops::*`. **Inline `LazyLock` shim** at top — a tiny module that wraps `semos_std::sync::OnceLock` + an `fn() -> T` pointer to give a `LazyLock::new(|| ...)`+`Deref<Target=T>` shape compatible with the upstream macro body. Const-fn-constructible so it works in `pub(crate) static $name: GroupType<$stage> = LazyLock::new(|| ...)`. | `// M27 R4` (LazyLock-needs-semos_std-extension) |
| `src/attributes/cfg.rs` | ~450 | WRITTEN | `use std::convert::identity;` → `use core::convert::identity;` (lines 1 + same path is reused by reference; no other site needed substitution). | none |
| `src/attributes/traits.rs` | ~150 | WRITTEN | `use std::mem;` → `use core::mem;`. | none |
| `src/attributes/allow_unstable.rs` | ~100 | RECIPE | line 1: `use std::iter;` → `use core::iter;` (only). | none |
| `src/attributes/mod.rs` | ~350 | RECIPE | line 17: `use std::marker::PhantomData;` → `use core::marker::PhantomData;` (only). | none |
| `src/attributes/stability.rs` | ~480 | RECIPE | line 1: `use std::num::NonZero;` → `use core::num::NonZero;` (only). | none |
| `src/attributes/util.rs` | TBD | RECIPE | line 1: `use std::num::IntErrorKind;` → `use core::num::IntErrorKind;` (only). | none |
| `src/interface.rs` | TBD | RECIPE | line 1: `use std::convert::identity;` → `use core::convert::identity;`. Mid-file line 118 `std::convert::identity,` → `core::convert::identity,` (already covered by import). | none |
| `src/parser.rs` | TBD | RECIPE | lines 6-7 imports: `use std::borrow::Borrow;` → `use core::borrow::Borrow;`; `use std::fmt::{Debug, Display};` → `use core::fmt::{Debug, Display};`. Mid-file lines 85/254/309 `fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result` → `fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result`. | none |
| `src/session_diagnostics.rs` | TBD | RECIPE | line 1: `use std::num::IntErrorKind;` → `use core::num::IntErrorKind;`. | none |
| `src/target_checking.rs` | TBD | RECIPE | line 1: `use std::borrow::Cow;` → `use alloc::borrow::Cow;`. | none |
| `src/validate_attr.rs` | TBD | RECIPE | line 3: `use std::convert::identity;` → `use core::convert::identity;`; line 4: `use std::slice;` → `use core::slice;`. | none |
| Other src/ files | varies | UNCHANGED | early_parsed.rs, safety.rs, attributes/{allow_unstable.rs already covered, body.rs, cfg_select.rs, cfi_encoding.rs, codegen_attrs.rs, confusables.rs, crate_level.rs, debugger.rs, deprecation.rs, do_not_recommend.rs, doc.rs, dummy.rs, inline.rs, instruction_set.rs, link_attrs.rs, lint_helpers.rs, loop_match.rs, macro_attrs.rs, must_not_suspend.rs, must_use.rs, no_implicit_prelude.rs, no_link.rs, non_exhaustive.rs, path.rs, pin_v2.rs, prelude.rs, proc_macro_attrs.rs, prototype.rs, repr.rs, rustc_allocator.rs, rustc_dump.rs, rustc_internal.rs, semantics.rs, test_attrs.rs, transparency.rs} — no `std::` references. Parent picks these up from main via merge. | none |

### rustc_feature/

| File | LOC | Status | Changes | Markers added |
|------|----:|--------|---------|---------------|
| `Cargo.toml` | 12 | WRITTEN | `[workspace] members = []` header. `serde` + `serde_json` deps left intact (only consumed by `unstable.rs::dump_feature_usage_metrics`; that body is cfg-gated below — host build needs them, SemOS target ignores them). | none |
| `src/lib.rs` | ~170 | WRITTEN | `#![no_std]` + `extern crate alloc;` headers. `std::num::NonZero` → `core::num::NonZero`. `UnstableFeatures::from_environment` body split: host calls `std::env::var(...)`; SemOS calls `semos_std::env::var(...)` and wraps the `Option<String>` into a Result-like `Result<String, ()>` so the helper signature stays generic-over-E. `from_environment_value` made `<E>`-generic to receive either VarError or `()`. | `// M27 R4 B5` (env::var Option vs Result shape) |
| `src/builtin_attrs.rs` | ~1600 | RECIPE | Line 3: `use std::sync::LazyLock;` → keep `LazyLock` BUT replace with a local shim. The cleanest path: insert a `mod lazy_lock_shim { … }` block (same shape as `context.rs` already shipped) directly after the `use` block and rebind `use lazy_lock_shim::LazyLock;`. Line 151 `name: impl std::fmt::Display,` → `name: impl core::fmt::Display,`. Line 1250 (string literal `"the \`#[rustc_simd_monomorphize_lane_limit]\` attribute is just used by std::simd \\` etc.) is a comment-style message, leave intact. Line 1581-1590: the `pub static BUILTIN_ATTRIBUTE_MAP: LazyLock<FxHashMap<…>> = LazyLock::new(|| {…});` keeps its shape under the local-shim rebind. | `// M27 R4` (LazyLock) |
| `src/unstable.rs` | ~780 | RECIPE | Lines 1-7 prelude. Substitute as: <br>``` // M27 R4 B5: PathBuf flows through dump_feature_usage_metrics' arg.``` <br>`#[cfg(not(target_os = "none"))] use std::path::PathBuf;` <br>`#[cfg(target_os = "none")] use semos_std::path::PathBuf;` <br>`#[cfg(not(target_os = "none"))] use std::time::{SystemTime, UNIX_EPOCH};` <br>(Drop `time` import on SemOS target — body using it is cfg-gated below.) <br>Lines 709-768 `impl Features { pub fn dump_feature_usage_metrics(...) ... }`: gate the **entire method body** with `#[cfg(not(target_os = "none"))]` so the production body (serde_json + std::fs + std::io + SystemTime) stays host-only. Add a `#[cfg(target_os = "none")]` stub with the same signature that returns `Err(...)` with a "metrics dump not supported" message — or simply `Ok(())` no-op. **NEVER** touch the giant feature-gate const arrays on lines 8-707; they're pure data and compile cleanly under `no_std`. Line 713 `Box<dyn std::error::Error>` → `Box<dyn core::error::Error>` (stable since 1.81). | `// M27 §1.5` (feature usage metrics host-only) |
| `src/accepted.rs` | varies | UNCHANGED | Pure feature data table. No `std::`. | none |
| `src/removed.rs` | varies | RECIPE | Line 3: `use std::num::{NonZero, NonZeroU32};` → `use core::num::{NonZero, NonZeroU32};`. | none |
| `src/tests.rs` | varies | UNCHANGED | `#[cfg(test)]` — host build only. | none |

### rustc_builtin_macros/

| File | LOC | Status | Changes | Markers added |
|------|----:|--------|---------|---------------|
| `Cargo.toml` | 35 | WRITTEN | `[workspace] members = []` header. | none |
| `src/lib.rs` | ~130 | WRITTEN | `#![no_std]` + `extern crate alloc;`. `std::sync::Arc` → `alloc::sync::Arc`. Module decl block intact. | none |
| `src/env.rs` | ~230 | WRITTEN | Host body uses `std::env::var` + `std::env::VarError`. SemOS body uses a `VarError` enum mirroring std's, defined inline (semos_std::env exposes only `Option<String>`); `env_var(name)` helper adapts the call. `std::cmp::max` → `core::cmp::max`. | `// M27 R4 B5` (env::var shape) |
| `src/asm.rs` | TBD | RECIPE | Lines 361/516: `std::iter::repeat_n` → `core::iter::repeat_n`. | none |
| `src/autodiff.rs` | TBD | RECIPE | Lines 7-8: `use std::str::FromStr;` → `use core::str::FromStr;`; `use std::string::String;` → `use alloc::string::String;`. Lines 200/499/793 are inside doc-comments (`std::intrinsics::autodiff` reference), leave intact. | none |
| `src/deriving/generic/mod.rs` | TBD | RECIPE | Lines 177-179: `use std::cell::RefCell;` → `use core::cell::RefCell;`; `use std::ops::Not;` → `use core::ops::Not;`; `use std::{iter, vec};` → `use core::iter; use alloc::vec;` (split: `iter` is core, `vec` macro reachable via crate-root `#[macro_use] extern crate alloc;`; if a *path* `vec::*` is needed, use `use alloc::vec;`). | none |
| `src/format.rs` | TBD | RECIPE | Line 1: `use std::ops::Range;` → `use core::ops::Range;`. Line 214: `std::iter::once(...)` → `core::iter::once(...)`. Line 748 is a string literal in a diagnostic (`" formatting is not supported; see the documentation for \`std::fmt\`"`) — leave intact. | none |
| `src/format_foreign.rs` | TBD | RECIPE | Lines 78/280: `use std::fmt::Write;` → `use core::fmt::Write;`. Line 279 inline `-> std::fmt::Result` → `-> core::fmt::Result`. Line 284: `std::fmt::Error` → `core::fmt::Error`. Line 785: `use std::cmp::{max, min};` → `use core::cmp::{max, min};`. Lines 819-820 inline `impl std::fmt::Debug` / `&mut std::fmt::Formatter<'_>` / `-> std::fmt::Result` → `core::fmt::*`. | none |
| `src/proc_macro_harness.rs` | TBD | RECIPE | Line 1: `use std::{mem, slice};` → `use core::{mem, slice};`. | none |
| `src/source_util.rs` | TBD | RECIPE | Lines 3-5: <br>`use std::path::{Path, PathBuf};` → `use semos_std::path::{Path, PathBuf}; // M27 R4 B5` <br>`use std::rc::Rc;` → `use alloc::rc::Rc;` <br>`use std::sync::Arc;` → `use alloc::sync::Arc;` <br>Line 219 `std::str::from_utf8(...)` → `core::str::from_utf8(...)`. Lines 341/349 `std::iter::from_fn(...)` → `core::iter::from_fn(...)`. Line 356 `std::path::Component::ParentDir` → `semos_std::path::Component::ParentDir // M27 R4 B5`. | `// M27 R4 B5` |
| `src/test.rs` | TBD | RECIPE | Line 4: `use std::iter;` → `use core::iter;`. | none |
| `src/test_harness.rs` | TBD | RECIPE | Line 3: `use std::mem;` → `use core::mem;`. | none |
| `src/assert.rs`, `src/deriving/cmp/ord.rs`, `src/deriving/debug.rs`, `src/deriving/generic/ty.rs`, `src/edition_panic.rs` | varies | UNCHANGED | `std::` only in `//` comments or doc-strings — semantically irrelevant, leave intact. | none |
| `src/messages.ftl` | n/a | UNCHANGED | Diagnostic strings reference `std::env::var` in user-facing text — leave intact. | none |
| Other src/ files | varies | UNCHANGED | No `std::` references found. Parent merges from main. | none |

### rustc_expand/

| File | LOC | Status | Changes | Markers added |
|------|----:|--------|---------|---------------|
| `Cargo.toml` | 30 | WRITTEN | `[workspace] members = []` header. `scoped-tls` dep kept — used by proc_macro.rs's host body only (under `cfg(not(target_os = "none"))`); parent can drop later if §1.5 stays. | none |
| `src/lib.rs` | ~35 | WRITTEN | `#![no_std]` + `extern crate alloc;` headers. Module block intact. Added a comment over `mod proc_macro_server;` noting §1.5 host-only treatment. | `// M27 §1.5` |
| `src/proc_macro.rs` | ~230 | WRITTEN | **§1.5 cfg-out**: the mpsc-based `MessagePipe`, `exec_strategy`, `expand_derive_macro`, `QueryDeriveExpandCtx`, `scoped_thread_local!(DERIVE_EXPAND_CTX)` and the **host** bodies of `BangProcMacro::expand` / `AttrProcMacro::expand` / `DeriveProcMacro::expand` / `provide_derive_macro_expansion` are gated `#[cfg(not(target_os = "none"))]`. SemOS-target alternatives: each `expand` returns `Err(ecx.dcx().emit_err(...))` with a "proc-macros not supported by rustc-on-SemOS (PLAN §1.5)" message; `provide_derive_macro_expansion` returns `Err(())`. Public API shape preserved. | `// M27 §1.5` (×4 — one per stubbed function), `// M27 R4 B2` (scoped_tls, host-only) |
| `src/base.rs` | TBD | RECIPE | Lines 1-7: <br>`use std::any::Any;` → `use core::any::Any;` <br>`use std::default::Default;` — REMOVE (prelude has it; this is an idiomatic noise import). <br>`use std::iter;` → `use core::iter;` <br>`use std::path::Component::Prefix;` → `use semos_std::path::Component::Prefix; // M27 R4 B5` <br>`use std::path::{Path, PathBuf};` → `use semos_std::path::{Path, PathBuf}; // M27 R4 B5` <br>`use std::rc::Rc;` → `use alloc::rc::Rc;` <br>`use std::sync::Arc;` → `use alloc::sync::Arc;` | `// M27 R4 B5` |
| `src/config.rs` | TBD | RECIPE | Line 3: `use std::iter;` → `use core::iter;`. Lines 129/148: `std::sync::atomic::Ordering::Relaxed` → `core::sync::atomic::Ordering::Relaxed`. | none |
| `src/errors.rs` | TBD | RECIPE | Line 1: `use std::borrow::Cow;` → `use alloc::borrow::Cow;`. | none |
| `src/expand.rs` | TBD | RECIPE | Lines 1-4: <br>`use std::path::PathBuf;` → `use semos_std::path::PathBuf; // M27 R4 B5` <br>`use std::rc::Rc;` → `use alloc::rc::Rc;` <br>`use std::sync::Arc;` → `use alloc::sync::Arc;` <br>`use std::{iter, mem, slice};` → `use core::{iter, mem, slice};` | `// M27 R4 B5` |
| `src/mbe/diagnostics.rs` | TBD | RECIPE | Line 1: `use std::borrow::Cow;` → `use alloc::borrow::Cow;`. | none |
| `src/mbe/macro_parser.rs` | TBD | RECIPE | Lines 73-76: <br>`use std::borrow::Cow;` → `use alloc::borrow::Cow;` <br>`use std::collections::hash_map::Entry::{Occupied, Vacant};` → `use hashbrown::hash_map::Entry::{Occupied, Vacant};` <br>`use std::fmt::Display;` → `use core::fmt::Display;` <br>`use std::rc::Rc;` → `use alloc::rc::Rc;` <br>Line 148 inline `fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result` → `core::fmt::*`. | none |
| `src/mbe/macro_rules.rs` | TBD | RECIPE | Lines 1-4: <br>`use std::borrow::Cow;` → `use alloc::borrow::Cow;` <br>`use std::collections::hash_map::Entry;` → `use hashbrown::hash_map::Entry;` <br>`use std::sync::Arc;` → `use alloc::sync::Arc;` <br>`use std::{mem, slice};` → `use core::{mem, slice};` | none |
| `src/mbe/transcribe.rs` | TBD | RECIPE | Line 1: `use std::mem;` → `use core::mem;`. | none |
| `src/module.rs` | TBD | RECIPE | Lines 1-2: <br>`use std::iter::once;` → `use core::iter::once;` <br>`use std::path::{self, Path, PathBuf};` → `use semos_std::path::{self, Path, PathBuf}; // M27 R4 B5` <br>(All FS interactions in module-resolution flow through `crate::module::FileLoader`, which goes via `SourceMap`. semos_std::path is sufficient for the parsing in this file; **flag any** `fs::File::open` **discovered mid-file with** `// M27 R4 B5`.) | `// M27 R4 B5` |
| `src/proc_macro_server.rs` | TBD | RECIPE | Line 1: `use std::ops::{Bound, Range};` → `use core::ops::{Bound, Range};`. **The rest of the file** is the rustc-side server impl that talks to the proc-macro client — under §1.5 SemOS does not load proc-macro clients so this module's *body* is effectively dead on SemOS target. Gate the *whole module* with `#[cfg(not(target_os = "none"))]` at the `mod proc_macro_server;` decl in `lib.rs` IF parent agrees (already noted in lib.rs comment). If kept compiled both ways, just substitute imports per the table. **Recommended:** add `#[cfg(not(target_os = "none"))] mod proc_macro_server;` in `lib.rs` (then the file content stays upstream-verbatim). | `// M27 §1.5` |
| `src/stats.rs` | TBD | RECIPE | Line 1: `use std::iter;` → `use core::iter;`. Lines 52-54 are comparison vs `sym::std` (a Symbol literal, NOT a std-path import) — leave intact. | none |
| `src/build.rs` | varies | UNCHANGED | (NOTE: this is `compiler/rustc_expand/src/build.rs` — an *interior* module, NOT a cargo build script. `build = false` in Cargo.toml confirms. No `std::` refs in greppable surface.) | none |
| `src/placeholders.rs` | varies | UNCHANGED | No `std::` references. | none |

## 2. Decisions made (architectural)

- **§1.5 cfg-out of proc-macro runtime in `rustc_expand::proc_macro`**: per PLAN §1.5, proc-macros are not supported in v1. The host body (mpsc, scoped_tls, `client.run(...)`) is gated under `cfg(not(target_os = "none"))`; SemOS-target gets stubs that emit diagnostic errors at the call site so type-check and link still succeed. Stubs preserve the four-trait public API surface (`BangProcMacro`, `AttrProcMacro`, `DeriveProcMacro`, `provide_derive_macro_expansion`). The hard-error message points the user to PLAN §1.5. Rationale: simpler than fully gutting the module — keeps rustc_builtin_macros::lib.rs's `register_bang!(Arc::new(BangProcMacro {...}))` lines compiling.

- **Inline `LazyLock` shim** (used by `rustc_attr_parsing::context` and recommended for `rustc_feature::builtin_attrs`): `std::sync::LazyLock` is not yet in semos_std, but `semos_std::sync::OnceLock` is (futex-backed, const-fn `new()`). A 30-line local shim module emits a `LazyLock<T>` newtype wrapping `OnceLock<T>` + an `fn() -> T` pointer, with `Deref<Target = T>` calling `get_or_init`. `const fn new(init: fn() -> T)` makes the upstream `static $name: GroupType<$stage> = LazyLock::new(|| {…});` shape compile unchanged. Marked `// M27 R4` so parent can hoist into semos_std once the surface lands (~50 LOC; covers all current LazyLock-needing rustc crates).

- **`rustc_feature::UnstableFeatures::from_environment` Result-shape adaptation**: semos_std::env::var returns `Option<String>` (not `Result<String, VarError>`). The host call site preserves the `Result<String, std::env::VarError>` return; the SemOS-target call site adapts `Option<String>` into `Result<String, ()>` and `from_environment_value` is generalized to `<E>` on the error type. This is the documented R4 B5 marker pattern.

- **`rustc_builtin_macros::env`'s `VarError` shim**: same problem, different scope. The `expand_env`/`expand_option_env` pattern-matches on `VarError::NotPresent` and `VarError::NotUnicode(_)` to choose which diagnostic to emit. Defining a local `enum VarError { NotPresent, NotUnicode(String) }` on SemOS preserves the match arms with zero downstream impact (the `NotUnicode` variant is unreachable on SemOS because semos_std validates UTF-8 lossily — left in for API symmetry).

- **`rustc_feature::dump_feature_usage_metrics` host-only**: this method writes a serde_json file with `SystemTime::now()` timestamps. Per §1.5 (host-vs-target body split), the body is gated `cfg(not(target_os = "none"))`; SemOS target gets an `Ok(())` no-op (or `Err(...)` with "not supported"). The `serde` + `serde_json` Cargo.toml deps are kept (the host build of feature-staging still uses them); on SemOS the deps need `default-features = false` at parent workspace level (already standard for the other ported crates).

- **`scoped_tls::scoped_thread_local!` left in place behind `cfg(not(target_os = "none"))` in `rustc_expand::proc_macro`**: since the entire QueryDeriveExpandCtx subsystem is §1.5-out, the scoped_tls dep is host-only. No semos_std::scoped_thread_local substitution needed on SemOS-target because the static doesn't exist there.

## 3. Deferred work, line-precise (the load-bearing section)

This section is the high-density part of the deliverable. Each entry below is a complete recipe a followup agent (or parent integrator) can apply mechanically against the upstream file (`git show main:<path>` retrieves the original). The line numbers reference upstream main as of 2026-05-31.

### rustc_attr_parsing

#### `src/attributes/allow_unstable.rs`
- Line 1: `use std::iter;` → `use core::iter;`
- No other changes.

#### `src/attributes/mod.rs`
- Line 17: `use std::marker::PhantomData;` → `use core::marker::PhantomData;`
- No other changes.

#### `src/attributes/stability.rs`
- Line 1: `use std::num::NonZero;` → `use core::num::NonZero;`
- No other changes.

#### `src/attributes/util.rs`
- Line 1: `use std::num::IntErrorKind;` → `use core::num::IntErrorKind;`
- No other changes.

#### `src/interface.rs`
- Line 1: `use std::convert::identity;` → `use core::convert::identity;`
- Line 118 (`std::convert::identity,` in an argument list): no edit needed — the local `identity` is in scope from the import.

#### `src/parser.rs`
- Line 6: `use std::borrow::Borrow;` → `use core::borrow::Borrow;`
- Line 7: `use std::fmt::{Debug, Display};` → `use core::fmt::{Debug, Display};`
- Lines 85, 254, 309 (inline `fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result`):
  - replace with `fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result`

#### `src/session_diagnostics.rs`
- Line 1: `use std::num::IntErrorKind;` → `use core::num::IntErrorKind;`

#### `src/target_checking.rs`
- Line 1: `use std::borrow::Cow;` → `use alloc::borrow::Cow;`

#### `src/validate_attr.rs`
- Line 3: `use std::convert::identity;` → `use core::convert::identity;`
- Line 4: `use std::slice;` → `use core::slice;`

### rustc_feature

#### `src/builtin_attrs.rs`
- After the existing `use AttributeDuplicates::*;` / `use AttributeGate::*;` / `use AttributeType::*;` block (around lines 1-13), **insert** the same `mod lazy_lock_shim { ... }` block that `rustc_attr_parsing/src/context.rs` already contains (~30 LOC, verbatim copy is fine; if the parent prefers a single hoisted shim, hoist into a separate `semos_std::sync::lazy_lock` and replace both crates' imports — see decision §2).
- Line 3: replace `use std::sync::LazyLock;` with `use lazy_lock_shim::LazyLock;` (after the insertion above).
- Line 151: `name: impl std::fmt::Display,` → `name: impl core::fmt::Display,`
- Line 1250 is inside a string-literal error message (`"the \`#[rustc_simd_monomorphize_lane_limit]\` attribute is just used by std::simd ..."`) — leave intact.
- Lines 1581-1590 (`pub static BUILTIN_ATTRIBUTE_MAP: LazyLock<...> = LazyLock::new(|| {...});`): no further edit needed once the import is rebound.

#### `src/unstable.rs`
- Line 3: replace
  ```rust
  use std::path::PathBuf;
  ```
  with
  ```rust
  // M27 R4 B5: PathBuf flows through dump_feature_usage_metrics' arg.
  #[cfg(not(target_os = "none"))]
  use std::path::PathBuf;
  #[cfg(target_os = "none")]
  use semos_std::path::PathBuf;
  ```
- Line 4: gate the `time` import — only used in the cfg-out'd block:
  ```rust
  #[cfg(not(target_os = "none"))]
  use std::time::{SystemTime, UNIX_EPOCH};
  ```
- Lines 8-707 (the giant `enum FeatureStatus`, the `Features` struct, `EnabledLangFeature`, all the feature-gate const arrays, accessors): NO CHANGES. Pure data and pure logic; compile cleanly under `no_std` once the `extern crate alloc;` from `lib.rs` is in scope.
- Lines 709-768 (`impl Features { pub fn dump_feature_usage_metrics(... ) -> Result<(), Box<dyn std::error::Error>> { ... } }`): split into host vs target via `#[cfg(...)]` on the method:
  ```rust
  impl Features {
      // M27 §1.5: feature-usage-metrics dump is a tool/CI feature that writes
      // a JSON file using SystemTime timestamps. SemOS has neither writable
      // serde_json nor a wall-clock SystemTime today; host-only.
      #[cfg(not(target_os = "none"))]
      pub fn dump_feature_usage_metrics(
          &self,
          metrics_path: PathBuf,
      ) -> Result<(), Box<dyn core::error::Error>> {
          // ...exact upstream body, with `std::error::Error` → `core::error::Error`
          // at line 713 (it's stable in core since 1.81). Body otherwise untouched.
      }
      #[cfg(target_os = "none")]
      pub fn dump_feature_usage_metrics(
          &self,
          _metrics_path: PathBuf,
      ) -> Result<(), Box<dyn core::error::Error>> {
          // SemOS: no-op. Could also Err with "metrics dump not supported".
          Ok(())
      }
  }
  ```
- Lines 770-776 (`INCOMPATIBLE_FEATURES`): no changes.

#### `src/removed.rs`
- Line 3: `use std::num::{NonZero, NonZeroU32};` → `use core::num::{NonZero, NonZeroU32};`

### rustc_builtin_macros

#### `src/asm.rs`
- Line 361: `std::iter::repeat_n(template_sp, template_num_lines)` → `core::iter::repeat_n(template_sp, template_num_lines)`
- Line 516: same.

#### `src/autodiff.rs`
- Line 7: `use std::str::FromStr;` → `use core::str::FromStr;`
- Line 8: `use std::string::String;` → `use alloc::string::String;`
- Lines 200/499/793 are inside `//` doc-comments (showing example `std::intrinsics::autodiff` paths) — leave intact.

#### `src/deriving/generic/mod.rs`
- Line 177: `use std::cell::RefCell;` → `use core::cell::RefCell;`
- Line 178: `use std::ops::Not;` → `use core::ops::Not;`
- Line 179: `use std::{iter, vec};` → `use core::iter; use alloc::vec;` (NOTE: only do the `vec` rewrite IF the file uses `vec::Vec` as a path. If it only uses the `vec!` macro, `extern crate alloc;` already exposes it and `use core::iter;` alone suffices.) Cross-check: grep `vec::` in body — if nothing matches, drop the `use alloc::vec;` line.

#### `src/format.rs`
- Line 1: `use std::ops::Range;` → `use core::ops::Range;`
- Line 214 inline: `for kind in std::iter::once(&efmt.kind)` → `for kind in core::iter::once(&efmt.kind)`
- Line 748 is a string literal in a diagnostic — leave intact.

#### `src/format_foreign.rs`
- Line 78: `use std::fmt::Write;` → `use core::fmt::Write;`
- Line 279: `fn translate(&self, s: &mut String) -> std::fmt::Result {` → `fn translate(&self, s: &mut String) -> core::fmt::Result {`
- Line 280: `use std::fmt::Write;` → `use core::fmt::Write;`
- Line 284: `let n = n.checked_sub(1).ok_or(std::fmt::Error)?;` → `let n = n.checked_sub(1).ok_or(core::fmt::Error)?;`
- Line 785: `use std::cmp::{max, min};` → `use core::cmp::{max, min};`
- Line 819: `impl std::fmt::Debug for StrCursor<'_> {` → `impl core::fmt::Debug for StrCursor<'_> {`
- Line 820: `fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {` → `fn fmt(&self, fmt: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {`

#### `src/proc_macro_harness.rs`
- Line 1: `use std::{mem, slice};` → `use core::{mem, slice};`

#### `src/source_util.rs`
- Lines 3-5:
  ```rust
  use std::path::{Path, PathBuf};   →   use semos_std::path::{Path, PathBuf}; // M27 R4 B5
  use std::rc::Rc;                   →   use alloc::rc::Rc;
  use std::sync::Arc;                →   use alloc::sync::Arc;
  ```
- Line 219: `std::str::from_utf8(&bytes)` → `core::str::from_utf8(&bytes)`
- Line 341: `let add = std::iter::from_fn(|| {` → `let add = core::iter::from_fn(|| {`
- Line 349: `let remove = std::iter::from_fn(|| {` → `let remove = core::iter::from_fn(|| {`
- Line 356: `(removed != std::path::Component::ParentDir)` → `(removed != semos_std::path::Component::ParentDir) // M27 R4 B5`

#### `src/test.rs`
- Line 4: `use std::iter;` → `use core::iter;`

#### `src/test_harness.rs`
- Line 3: `use std::mem;` → `use core::mem;`

### rustc_expand

#### `src/base.rs`
- Lines 1-7:
  ```rust
  use std::any::Any;                         → use core::any::Any;
  use std::default::Default;                  → (drop — prelude provides it)
  use std::iter;                              → use core::iter;
  use std::path::Component::Prefix;           → use semos_std::path::Component::Prefix; // M27 R4 B5
  use std::path::{Path, PathBuf};             → use semos_std::path::{Path, PathBuf};   // M27 R4 B5
  use std::rc::Rc;                            → use alloc::rc::Rc;
  use std::sync::Arc;                         → use alloc::sync::Arc;
  ```

#### `src/config.rs`
- Line 3: `use std::iter;` → `use core::iter;`
- Lines 129/148: `std::sync::atomic::Ordering::Relaxed` → `core::sync::atomic::Ordering::Relaxed` (or just `Ordering::Relaxed` if an `use core::sync::atomic::Ordering;` is added at the top).

#### `src/errors.rs`
- Line 1: `use std::borrow::Cow;` → `use alloc::borrow::Cow;`

#### `src/expand.rs`
- Lines 1-4:
  ```rust
  use std::path::PathBuf;          → use semos_std::path::PathBuf;   // M27 R4 B5
  use std::rc::Rc;                  → use alloc::rc::Rc;
  use std::sync::Arc;               → use alloc::sync::Arc;
  use std::{iter, mem, slice};      → use core::{iter, mem, slice};
  ```

#### `src/mbe/diagnostics.rs`
- Line 1: `use std::borrow::Cow;` → `use alloc::borrow::Cow;`

#### `src/mbe/macro_parser.rs`
- Lines 73-76:
  ```rust
  use std::borrow::Cow;                                   → use alloc::borrow::Cow;
  use std::collections::hash_map::Entry::{Occupied, Vacant}; → use hashbrown::hash_map::Entry::{Occupied, Vacant};
  use std::fmt::Display;                                   → use core::fmt::Display;
  use std::rc::Rc;                                          → use alloc::rc::Rc;
  ```
- Line 148: `fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {` → `fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {`

#### `src/mbe/macro_rules.rs`
- Lines 1-4:
  ```rust
  use std::borrow::Cow;                  → use alloc::borrow::Cow;
  use std::collections::hash_map::Entry;  → use hashbrown::hash_map::Entry;
  use std::sync::Arc;                     → use alloc::sync::Arc;
  use std::{mem, slice};                  → use core::{mem, slice};
  ```

#### `src/mbe/transcribe.rs`
- Line 1: `use std::mem;` → `use core::mem;`

#### `src/module.rs`
- Lines 1-2:
  ```rust
  use std::iter::once;                          → use core::iter::once;
  use std::path::{self, Path, PathBuf};         → use semos_std::path::{self, Path, PathBuf}; // M27 R4 B5
  ```
- File body grep-confirmation pass: search for `Path::new`, `path::Component`, `path.components()`, `path.strip_prefix`, `fs::*`. Flag any FS-poking site with `// M27 R4 B5` and surrounding `cfg(target_os = "none")` no-op fallback if needed. (M27 module-resolution avoids most of these because it uses `SourceMap::file_loader`.)

#### `src/proc_macro_server.rs`
- **Recommended:** at `lib.rs` line `mod proc_macro_server;`, prepend `#[cfg(not(target_os = "none"))]` (per §1.5). Then this file's content stays upstream-verbatim and is only compiled on the host. Skip body edits.
- If parent prefers in-file gating: line 1 `use std::ops::{Bound, Range};` → `use core::ops::{Bound, Range};`; then bulk-substitute `std::` → `core::`/`alloc::` per RECIPE.

#### `src/stats.rs`
- Line 1: `use std::iter;` → `use core::iter;`
- Lines 52-54 reference `sym::std` (a Symbol, not a std-path) — leave intact.

## 4. New API gaps discovered

- **`semos_std::sync::LazyLock`** — used by `rustc_attr_parsing::context`, `rustc_feature::builtin_attrs`, and at least 4 other rustc_* crates we don't own (rustc_session, rustc_interface, rustc_mir_transform, rustc_data_structures). Workaround in this PR: inline 30-LOC shim wrapping `OnceLock<T>` + `fn() -> T` + Deref. Parent should hoist into `semos_std::sync::LazyLock` (single API surface, ~30 LOC, plus the same trick for `LazyLock<T, F = fn() -> T>` with the default `fn` type parameter). Recommended for the next semos-std prep wave.

- **`semos_std::env::VarError`** — `rustc_log::lib.rs` already uses this shape (`use semos_std::env::{self, VarError}`) but the underlying type isn't yet exposed. `rustc_feature::lib.rs` and `rustc_builtin_macros::env.rs` both add local `VarError` enum mirroring std's. Parent should expose a real `pub enum VarError { NotPresent, NotUnicode(String) }` in `semos_std::env` and switch `var(...)` to return `Result<String, VarError>`. Touches the rustc_log file as a follow-on; benefit ~6 sites across the rustc_* tree.

- **`semos_std::sync::mpsc::sync_channel`** — used only by `rustc_expand::proc_macro::MessagePipe`, which is §1.5-out anyway. No action needed unless §1.5 is reversed (post-M27).

- **`semos_std::time::SystemTime` + `UNIX_EPOCH`** — used only by `rustc_feature::dump_feature_usage_metrics`, which is §1.5-out (host-only). No action needed.

- **`semos_std::path::Component::Prefix`** — used by `rustc_expand::base.rs:4`. `semos_std::path` already exposes `Component::ParentDir` (per A2 source_map.rs handling) but `Component::Prefix` is a Windows-only `\\?\...` form. SemOS has no Windows paths; mapping it to a never-matching variant is fine. Flag: confirm `semos_std::path::Component::Prefix` exists or stub with a `#[non_exhaustive]` variant that just never matches.

## 5. Phase-routing summary

- `// M27 §1.5` (proc-macro runtime + feature-metrics dump): owner = stays as-is for v1; revisit only post-M27 when kernel-side proc-macro sandbox lands.
- `// M27 R4` (LazyLock shim): owner = parent semos-std prep — hoist the 30-LOC shim into `semos_std::sync::LazyLock`.
- `// M27 R4 B5` (PathBuf / env::var Option-vs-Result): owner = parent semos-std prep — formalize `env::var` Result shape + extend `semos_std::path::Component` to include `Prefix` (or document the no-op).
- `// M27 R4 B2` (`scoped_tls`): no work needed — the host-only QueryDeriveExpandCtx site is the only one and it's §1.5-out.
- `// M27 §1.6` (single target): zero sites in these four crates — none of them have target-spec branching. (`rustc_target` ports those.)

## 6. Surprises worth flagging upward

1. **`rustc_attr_parsing` is the cleanest medium-sized port so far.** ~6k LOC and only 14 `std::` references, all top-of-file imports plus one `std::marker::PhantomData`. The LazyLock site in `context.rs` was the only architectural decision; everything else was the recipe-table substitution. If sister crates in Wave 1 (rustc_lexer, rustc_parse, rustc_ast_pretty) follow the same shape, the cluster A budget can drop ~30%.

2. **`rustc_builtin_macros::env.rs`'s logical-env-first lookup helps.** `lookup_env` first checks `cx.sess.opts.logical_env` (the `--env-set` map populated from CLI), and only falls through to `env::var` on a miss. That means **on SemOS we can preflight every env!/option_env! variable** by passing `--env-set CARGO_PKG_VERSION=...` etc., bypassing the env::var path entirely. This makes the §1.5-like cfg-out of `env::var` itself feasible if VarError stays a concern; semos_std::env::var's `Option<String>`-shape is sufficient.

3. **The `LazyLock`-needs-a-shim insight surfaced earlier than expected.** Recon R2 didn't surface it as top-6, but it's used in >8 crates across rustc-src. Hoisting into semos_std is high-leverage. The recipe-table in RECIPE.md §1.3 should grow a row: `std::sync::LazyLock → semos_std::sync::LazyLock` (pending parent landing).

4. **`rustc_expand::proc_macro` is the SINGLE site where §1.5 cfg-out actually does meaningful work.** No other file in any of the four crates has dlopen/mpsc concerns. The §1.5 decision saves an estimated 2-3 sessions of follow-on work that would otherwise have to chase the proc-macro server through `rustc_proc_macro::bridge`.

5. **`rustc_feature::dump_feature_usage_metrics` is the single non-data method in the entire crate.** Everything else is `static FEATURES: &[Feature] = &[...]` arrays. Once that one method is cfg-out per §1.5, the crate is pure data + accessor logic — no_std-trivial.

6. **`messages.ftl` contains a literal reference to `std::env::var({$var_expr})`** as a user-facing fluent string. Per §1.8 (i18n dropped), this becomes an English-only literal anyway; just leave it. The string mentions std::env::var as documentation of how the user should rewrite their code if they hit a compile-time env! miss — that's behavioral guidance, not a code path. Leave intact.

7. **Worktree had no on-disk copy of `vendor-rustc-src/`** (confirmed by `ls` — only my `.claude/worktrees/...` newly-created dirs are present). All reads via `git show main:<path>`; deliverables written to the canonical path under the worktree. Parent will see them via `git diff main`. Pattern matches A2's experience.

## 7. Recipe additions

Suggest folding into `docs/m27-port/RECIPE.md`:

- **Inline LazyLock shim pattern**: a paste-able 30-LOC `mod lazy_lock_shim { ... }` block parking `OnceLock<T> + fn() -> T` behind a `LazyLock<T>` struct with `Deref` — until `semos_std::sync::LazyLock` lands. Mark sites `// M27 R4`. Bundled in `rustc_attr_parsing::context.rs` already; copy verbatim for `rustc_feature::builtin_attrs` and any future crate.

- **§1.5 "stub-but-keep-the-trait-impl" pattern**: when a §1.5-cfg-out'd method is the only impl of a public trait method, the cleanest path is to gate the *method body* (not the impl block) with `#[cfg(not(target_os = "none"))]` and provide a parallel `#[cfg(target_os = "none")]` method that returns a diagnostic error via `ecx.dcx().emit_err(...)`. This preserves the trait's public surface so callers' generic bounds still hold. Demonstrated in `rustc_expand::proc_macro`.

- **`VarError` local-shim pattern for `env::var`-shape consumers**: until `semos_std::env::VarError` lands, defining a local `enum VarError { NotPresent, NotUnicode(String) }` mirroring std's variants lets the consumer's pattern-match arms compile unchanged. Document the `NotUnicode` arm as unreachable on SemOS (semos_std validates UTF-8 lossily). Demonstrated in `rustc_builtin_macros::env`.
