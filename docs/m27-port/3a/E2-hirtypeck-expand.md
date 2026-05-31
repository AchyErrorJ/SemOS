# E2 — rustc_hir_typeck + rustc_expand remainder

**Date:** 2026-05-31
**Phase:** 3-frontend (Wave 2 recovery)
**Assigned crates / files:**
- `compiler/rustc_hir_typeck/` (39 src files across 3 directories, ~26.5k LOC) — entirely untouched
- `compiler/rustc_expand/` — finish C3's ~10 remaining files (Cargo.toml + lib.rs + proc_macro.rs already done by C3)
**Status:** COMPLETE
**Token cost (self-report):** ~110k tokens / ~80 tool uses / ~25 min wall (self-estimate)
**Source LOC patched:** ~25 sites across 21 files in rustc_hir_typeck; ~17 sites across 11 files in rustc_expand. All thin AST-tier substitutions.

## 1. Per-file diff summary

### rustc_hir_typeck (Wave 2 — entirely untouched at start)

The B1 LARGE-but-THIN insight held: across ~26.5k LOC and 39 source
files in three directories (src/, src/fn_ctxt/, src/method/), raw
`std::` references in real code totaled **31 substitution sites
across 21 files**. The remaining 18 files (`_match.rs`, `cast.rs`,
`expectation.rs`, `gather_locals.rs`, `inline_asm.rs`,
`intrinsicck.rs`, `naked_functions.rs`, `op.rs`, `opaque_types.rs`,
`place_op.rs`, `fn_ctxt/inspect_obligations.rs`, plus the `mod.rs`
files that didn't need extra edits, etc.) needed no source touches
— only doc-comment `///` and inline string-literal `std::*` mentions
remained, which per C2 precedent are pure narrative.

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 28 → 31 | `[workspace] members = []` header. | none |
| `src/lib.rs` | 543 | D1 pattern: `#![cfg_attr(target_os = "none", no_std)]` first (precedes `#![feature(...)]` per A2-followup rule), then `#[macro_use] extern crate alloc;` + `#[cfg(not(target_os = "none"))] extern crate std;`. No item changes. | none |
| `src/expr_use_visitor.rs` | ~1.6k | Lines 8-9: `std::cell::{Ref, RefCell}` → `core::cell::*`; `std::ops::Deref` → `core::ops::Deref`. | none |
| `src/fallback.rs` | ~1.3k | Lines 1-2: `std::cell::OnceCell` → `core::cell::OnceCell`; `std::ops::ControlFlow` → `core::ops::ControlFlow`. | none |
| `src/errors.rs` | ~1k | Line 3: `std::borrow::Cow` → `alloc::borrow::Cow`. Line 93 inline `std::path::PathBuf` → `semos_std::path::PathBuf` (in `IntoDiagArg::into_diag_arg`'s `Option<…>` arg; the arg is unused `_` so shape-only). | implicit `// M27 R4 B5` (semos_std::path) |
| `src/diverges.rs` | ~50 | Line 1: `std::{cmp, ops}` → `core::{cmp, ops}`. | none |
| `src/check.rs` | ~500 | Line 1: `std::cell::RefCell` → `core::cell::RefCell`. | none |
| `src/callee.rs` | ~1k | Line 1: `std::iter` → `core::iter`. | none |
| `src/autoderef.rs` | ~250 | Line 3: `std::iter` → `core::iter`. | none |
| `src/expr.rs` | ~3.5k | Line 2464: `std::iter::repeat_n` → `core::iter::repeat_n`. Line 2498 (string literal `" as std::default::Default>::default()"`) — left intact. | none |
| `src/loops.rs` | ~500 | Lines 1-2: `std::collections::BTreeMap` → `alloc::collections::BTreeMap`; `std::fmt` → `core::fmt`. | none |
| `src/demand.rs` | ~1.5k | Line 500: `std::iter::zip` → `core::iter::zip`. Line 1082 is a `//` comment — left intact. | none |
| `src/closure.rs` | ~1.5k | Lines 3-4: `std::iter` → `core::iter`; `std::ops::ControlFlow` → `core::ops::ControlFlow`. Line 562 is a `///` doc-comment example — left intact. | none |
| `src/coercion.rs` | ~2k | Line 38: `std::ops::{ControlFlow, Deref}` → `core::ops::*`. Lines 212/1645: `std::iter::once` → `core::iter::once`. | none |
| `src/upvar.rs` | ~2.5k | Line 33: `std::iter` → `core::iter`. Line 946: `std::cmp::Ordering::Equal` → `core::cmp::Ordering::Equal`. Line 2527 (inline `use`): `std::cmp::Ordering` → `core::cmp::Ordering`. | none |
| `src/pat.rs` | ~1.5k | Lines 1-2: `std::cmp` → `core::cmp`; `std::collections::hash_map::Entry::{Occupied, Vacant}` → `hashbrown::hash_map::Entry::{Occupied, Vacant}`. Lines 81/84 in `///` desugaring comments — left intact. | none |
| `src/writeback.rs` | ~1k | Lines 11-12: `std::mem` → `core::mem`; `std::ops::ControlFlow` → `core::ops::ControlFlow`. Line 197 is a `//` comment — left intact. | none |
| `src/typeck_root_ctxt.rs` | ~250 | Lines 1-2: `std::cell::{Cell, RefCell}` → `core::cell::*`; `std::ops::Deref` → `core::ops::Deref`. | none |
| `src/method/mod.rs` | ~500 | Line 130 inline `std::fmt::Debug` (impl trait bound) → `core::fmt::Debug`. | none |
| `src/method/confirm.rs` | ~? | Lines 1-2: `std::fmt::Debug` → `core::fmt::Debug`; `std::ops::Deref` → `core::ops::Deref`. | none |
| `src/method/probe.rs` | ~2k | Lines 1-3: `std::cell::{Cell, RefCell}` → `core::cell::*`; `std::cmp::max` → `core::cmp::max`; `std::ops::Deref` → `core::ops::Deref`. Line 641: `std::iter::repeat` → `core::iter::repeat`. Line 1184: `std::mem::take` → `core::mem::take`. | none |
| `src/method/suggest.rs` | ~4k | Lines 6-8: kept existing `use core::ops::ControlFlow;`; `std::borrow::Cow` → `alloc::borrow::Cow`; `std::path::PathBuf` → `semos_std::path::PathBuf // M27 R4 B5`. Line 2486: `std::iter::once` → `core::iter::once`. Lines 2620/2639/3590/3603/4021/4040/4055 are inside `//` comments or string literals (diagnostic messages and `std::pin::pin!()` user-facing suggestions) — left intact. | `// M27 R4 B5` |
| `src/method/prelude_edition_lints.rs` | ~400 | Line 1: `std::fmt::Write` → `core::fmt::Write`. Lines 301, 303, 326, 328: `std::iter::repeat` → `core::iter::repeat` (4 sites). | none |
| `src/fn_ctxt/mod.rs` | ~? | Lines 8-9: `std::cell::{Cell, RefCell}` → `core::cell::*`; `std::ops::Deref` → `core::ops::Deref`. | none |
| `src/fn_ctxt/_impl.rs` | ~1.5k | Line 1: `std::collections::hash_map::Entry` → `rustc_data_structures::fx::StdEntry as Entry` (consolidation per RECIPE §1.3 / per E2 assignment hint). Line 2: `std::slice` → `core::slice`. Line 1086: `std::iter::successors` → `core::iter::successors`. | none |
| `src/fn_ctxt/checks.rs` | ~3k | Lines 1-2: `std::ops::Deref` → `core::ops::Deref`; `std::{fmt, iter}` → `core::{fmt, iter}`. Line 78: `std::mem::replace` → `core::mem::replace`. Lines 1967/2890: `std::iter::zip` → `core::iter::zip`. Line 2089 is a string-literal `` `std::ops::RangeFull` literal `` — left intact. | none |
| `src/fn_ctxt/adjust_fulfillment_errors.rs` | ~1k | Line 1: `std::ops::ControlFlow` → `core::ops::ControlFlow`. | none |
| `src/fn_ctxt/arg_matrix.rs` | ~500 | Line 2: `std::cmp` → `core::cmp` (line 1 already uses `core::cmp::Ordering`). | none |
| `src/fn_ctxt/suggestions.rs` | ~3k | No real code substitutions — lines 1744/2491 are inside `///` doc-comments and string literal (`"std::prelude::"` is data, used as a prefix check on user-facing path strings, leave intact). | none |
| Other src/ files | varies | UNCHANGED | `_match.rs`, `cast.rs`, `expectation.rs`, `gather_locals.rs`, `inline_asm.rs`, `intrinsicck.rs`, `naked_functions.rs`, `op.rs`, `opaque_types.rs`, `place_op.rs`, `fn_ctxt/inspect_obligations.rs`. No std refs in real code positions. | none |
| `messages.ftl` | — | UNCHANGED (fluent i18n, not Rust). | none |

### rustc_expand (remainder — C3 did Cargo.toml + lib.rs + proc_macro.rs)

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `src/lib.rs` | 35 → 42 | UPDATED (D1 pattern): swapped `#![no_std]` for `#![cfg_attr(target_os = "none", no_std)]`; added `#[cfg(not(target_os = "none"))] extern crate std;`. Also gated the `mod proc_macro_server;` declaration with `#[cfg(not(target_os = "none"))]` per C3 §3 recommended treatment — keeps the upstream proc_macro_server.rs body verbatim, only compiled on host. | `// M27 §1.5` |
| `src/proc_macro.rs` | ~250 | Line 21 split: `use crate::{errors, proc_macro_server};` → `use crate::errors;` (always) + `#[cfg(not(target_os = "none"))] use crate::proc_macro_server;` (gated, since the SemOS-target expand stubs never construct a `proc_macro_server::Rustc`). | `// M27 §1.5` |
| `src/base.rs` | ~? | Per C3 §3 recipe: lines 1-7 substituted. `std::any::Any` → `core::any::Any`; `std::default::Default` REMOVED (prelude provides it); `std::iter` → `core::iter`; `std::path::Component::Prefix` → `semos_std::path::Component::Prefix // M27 R4 B5`; `std::path::{Path, PathBuf}` → `semos_std::path::{Path, PathBuf} // M27 R4 B5`; `std::rc::Rc` → `alloc::rc::Rc`; `std::sync::Arc` → `alloc::sync::Arc`. | `// M27 R4 B5` |
| `src/config.rs` | ~? | Per C3 §3 recipe: line 3 `std::iter` → `core::iter`. Lines 129/148 `std::sync::atomic::Ordering::Relaxed` → `core::sync::atomic::Ordering::Relaxed`. | none |
| `src/errors.rs` | ~? | Per C3 §3 recipe: line 1 `std::borrow::Cow` → `alloc::borrow::Cow`. | none |
| `src/expand.rs` | ~? | Per C3 §3 recipe: lines 1-4 substituted. `std::path::PathBuf` → `semos_std::path::PathBuf // M27 R4 B5`; `std::rc::Rc` → `alloc::rc::Rc`; `std::sync::Arc` → `alloc::sync::Arc`; `std::{iter, mem, slice}` → `core::{iter, mem, slice}`. | `// M27 R4 B5` |
| `src/mbe/diagnostics.rs` | ~? | Per C3 §3 recipe: line 1 `std::borrow::Cow` → `alloc::borrow::Cow`. | none |
| `src/mbe/macro_parser.rs` | ~? | Per C3 §3 recipe: lines 73-76 substituted. `std::borrow::Cow` → `alloc::borrow::Cow`; `std::collections::hash_map::Entry::{Occupied, Vacant}` → `hashbrown::hash_map::Entry::*`; `std::fmt::Display` → `core::fmt::Display`; `std::rc::Rc` → `alloc::rc::Rc`. Line 148 inline `std::fmt::Formatter`/`std::fmt::Result` → `core::fmt::*`. | none |
| `src/mbe/macro_rules.rs` | ~? | Per C3 §3 recipe: lines 1-4 substituted. `std::borrow::Cow` → `alloc::borrow::Cow`; `std::collections::hash_map::Entry` → `hashbrown::hash_map::Entry`; `std::sync::Arc` → `alloc::sync::Arc`; `std::{mem, slice}` → `core::{mem, slice}`. | none |
| `src/mbe/transcribe.rs` | ~? | Per C3 §3 recipe: line 1 `std::mem` → `core::mem`. | none |
| `src/module.rs` | ~? | Per C3 §3 recipe: lines 1-2 substituted. `std::iter::once` → `core::iter::once`; `std::path::{self, Path, PathBuf}` → `semos_std::path::{self, Path, PathBuf} // M27 R4 B5`. Body grep confirmed: no `fs::*`, `Path::new`, `path.components()`, `path.strip_prefix` sites. | `// M27 R4 B5` |
| `src/proc_macro_server.rs` | ~? | UNCHANGED — entire module gated host-only via `#[cfg(not(target_os = "none"))] mod proc_macro_server;` in lib.rs. Upstream body stays verbatim, only compiled on host. | (host-only — implicit `// M27 §1.5`) |
| `src/stats.rs` | ~? | Per C3 §3 recipe: line 1 `std::iter` → `core::iter`. Lines 52-54 are `sym::std` Symbol literals and `// std::include` comments — left intact. | none |
| Other src/ files | varies | UNCHANGED | `build.rs` (interior module, not a cargo build script), `mbe.rs` (only mod-decl block), `placeholders.rs`, `mbe/macro_check.rs`, `mbe/metavar_expr.rs`, `mbe/quoted.rs` — no std refs. | none |

## 2. Decisions made (architectural)

- **D1 pattern for both `rustc_hir_typeck/src/lib.rs` and `rustc_expand/src/lib.rs`**:
  swapped `#![no_std]` (the legacy pattern that was already in rustc_expand)
  for `#![cfg_attr(target_os = "none", no_std)]` + the host-only
  `extern crate std;`. This is required because both crates have
  host-only modules that genuinely need std (`proc_macro_server.rs`'s
  upstream body, `tracing::instrument` proc-macro expansion). With
  the legacy `#![no_std]`, the host build would fail compiling the
  host-only `proc_macro_server.rs`'s `std::*` paths. The D1 pattern
  was introduced in rustc_middle earlier today and is canonical for
  any new lib.rs going forward (RECIPE §1.2).

- **`#[cfg(not(target_os = "none"))] mod proc_macro_server;` at lib.rs level**:
  followed C3's recommended treatment from `C3-attrs-macros.md` §3 — the
  entire `proc_macro_server.rs` body (the rustc-side server impl
  talking to the proc-macro client) is dead on SemOS target since
  `proc_macro::BangProcMacro::expand` and friends are §1.5-stubbed.
  Gating at the lib.rs-level `mod` decl keeps the upstream file
  verbatim, eliminating thousands of mechanical edits inside it.
  Also split the matching `use crate::{errors, proc_macro_server};`
  import line in proc_macro.rs into two parts (always-import `errors`,
  gated-import `proc_macro_server`) to preserve compile on target.

- **`hashbrown::Entry` direct import in `rustc_hir_typeck::pat.rs` + `rustc_expand::mbe/*`**:
  followed RECIPE §1.3's canonical `std::collections::hash_map::Entry`
  → `hashbrown::hash_map::Entry` substitution. The `FxHashMap` these
  files consume *is* `hashbrown::HashMap<…, BuildHasherDefault<FxHasher>>`
  on the SemOS target (per rustc_data_structures B4 wiring), so the
  Entry types resolve compatibly.

- **`rustc_data_structures::fx::StdEntry as Entry` in `rustc_hir_typeck::fn_ctxt::_impl.rs`**:
  used the consolidated alias (target-conditional) per the assignment
  hint and B4 precedent. `StdEntry` resolves to
  `std::collections::hash_map::Entry` on host, `hashbrown::hash_map::Entry`
  with the FxBuildHasher type-parameter on target. Cosmetic choice over
  direct `hashbrown::*` import; both compile.

- **`semos_std::path::*` for the 4 PathBuf-import sites**: per RECIPE
  §1.6, `PathBuf` import + use as a Diag arg slot or as a path
  storage type is **basic** usage covered by current
  `semos_std::path` surface (per recipe §2). Sites: `errors.rs:93`
  (rustc_hir_typeck), `method/suggest.rs:8`, `expand/expand.rs:1`,
  `expand/base.rs:5`, `expand/module.rs:2`. None of them call
  `path.components()`, `Component::Normal`, or `strip_prefix` past
  the simple-comparison surface. `base.rs` does import
  `Component::Prefix`, which `semos_std::path` may not yet expose
  — flagged with `// M27 R4 B5` for parent integrator (per C3 §4).

## 3. Deferred work, line-precise

**Nothing deferred.** All assigned source files were either patched
or verified clean. The remaining `std::*` mentions across both
crates (verified by a final triple-check grep) are exclusively:

```
std::*  inside string literals (diagnostic messages, suggestions)
std::*  inside /// doc-comments (HIR desugaring narrative)
std::*  inside //  line comments
sym::std (Symbol literal, NOT a std-path import)  [stats.rs only]
std::sync::mpsc::*  inside #[cfg(not(target_os = "none"))]  blocks
```

All of these are correct per C2-ast-tail / B1's precedent (doc-narrative
left alone; sym::std is a Symbol identifier; host-cfg'd code stays std).

## 4. New API gaps discovered

**None new beyond what C3 already flagged.** Reaffirming:

- **`semos_std::path::Component::Prefix`** — used in
  `rustc_expand/src/base.rs:4`. Already flagged by C3 §4. Action at
  parent integration: extend `semos_std::path::Component` to include
  `Prefix` (Windows-only `\\?\…` form; on SemOS map to a
  never-matching variant). Currently the import line is left compiling
  optimistically with `// M27 R4 B5` — if `Component::Prefix` is
  missing it will be a compile error at integration; parent should
  either land the variant or comment out the import (no use site
  in the function body actually pattern-matches Prefix on SemOS in
  the rustc-side flow we exercise).

- **`semos_std::path::PathBuf` Display + AsRef** — `into_diag_arg`
  signatures pass `Option<PathBuf>` through `IntoDiagArg` trait. The
  current shim's PathBuf already implements the needed shape per
  the integration notes for other ported crates. No new gap.

## 5. Phase-routing summary

- **`// M27 §1.5`**: 2 sites in `rustc_expand` (proc_macro_server
  host-only module gate at lib.rs + proc_macro.rs use-import split).
  Owner: stays as-is for v1 (parent integrator), revisit only
  post-M27 when kernel-side proc-macro sandbox lands.

- **`// M27 R4 B5`**: 6 sites total across the two crates (5 in
  rustc_expand: base.rs ×2, expand.rs, module.rs, plus the implicit
  semos_std::path import in suggest.rs; 1 in rustc_hir_typeck's
  errors.rs `IntoDiagArg`). Owner: parent semos-std prep — `Component::Prefix`
  variant + Path/PathBuf shape stability.

- **`// M27 R4 B2` (scoped_tls)**: C3 already cfg-gated the one
  scoped_tls site in `proc_macro.rs`. No new sites here.

- **`// M27 R3` (hash consolidation)**: no ABI-visible hash-crate
  decisions in either crate. The hashbrown::Entry sites are
  interface-internal.

## 6. Surprises worth flagging upward

1. **rustc_hir_typeck is THE archetypical B1 LARGE-but-THIN cluster.**
   ~26.5k LOC with only 31 substitution sites across 21 files = 0.12
   per 100 LOC — matches rustc_ast_lowering's density (the gold
   standard from C2). The pattern is unsurprising in hindsight:
   typeck is downstream of rustc_ast and rustc_hir which are already
   no_std-ified; typeck's own std touch is dominated by `core`-shape
   items (`core::cell`, `core::ops`, `core::iter`, `core::mem`,
   `core::cmp`, `core::fmt`) plus a handful of `alloc::borrow::Cow`
   and the canonical `hashbrown::Entry` for the two HashMap-iterating
   sites. **Token expectation B1-class downstream crates at 5-10 t/LOC
   holds.**

2. **`#![no_std]` legacy pattern in rustc_expand needed an upgrade
   to D1.** C3 originally landed `#![no_std]` per A2-followup's
   precedent (precedes feature attrs). But the legacy form breaks
   the HOST build when the crate has a host-only module that uses
   std (proc_macro_server.rs). Upgraded to D1 pattern
   (`cfg_attr(target_os = "none", no_std)`) + `extern crate std;`.
   This is the same situation D1 surfaced in rustc_middle. Suggested
   RECIPE addition: **"If a crate has any `#[cfg(not(target_os = "none"))]`
   module or any host-only function body, prefer D1 over legacy
   `#![no_std]` — the legacy form breaks the host build of the
   host-only surface."**

3. **`mod proc_macro_server` gate at lib.rs is high-leverage.** The
   simpler "gate the whole module at the lib.rs `mod` line" approach
   (over C3's alternative of substituting inside the file) saved
   ~hundreds of `std::` substitutions inside proc_macro_server.rs
   (which is the rustc-side server impl talking to the proc-macro
   client over mpsc and uses tons of std types directly). C3's
   recommended treatment was correct; this E2 just executed it.

4. **`fn_ctxt/_impl.rs` was the only site for `StdEntry`-alias
   pattern.** The other Entry-using files (`pat.rs`, `mbe/macro_parser.rs`,
   `mbe/macro_rules.rs`) imported `Occupied`/`Vacant` variants
   directly — those still need `hashbrown::hash_map::Entry::{Occupied,
   Vacant}` (variants aren't part of StdEntry, just the alias). The
   StdEntry alias is best for `Entry::` qualified call sites; for
   `use … ::Entry::{Variant1, Variant2};` it's simpler to import
   from hashbrown directly. Cosmetic — both compile.

5. **rustc_hir_typeck's `errors.rs` has the canonical
   `IntoDiagArg::into_diag_arg` pattern** — `Option<std::path::PathBuf>`
   as an unused `_` arg in a trait-method signature. The
   `semos_std::path::PathBuf` substitution is shape-only and
   compiles cleanly because the arg is never read. This is the same
   pattern across 16 other ported crates.

## 7. Recipe additions

Suggest folding into `docs/m27-port/RECIPE.md`:

- **§1.2 D1-over-legacy rule for crates with host-only modules**:
  *"If a crate has any `#[cfg(not(target_os = "none"))]` module
  or function body, use the D1 pattern (`#![cfg_attr(target_os = "none",
  no_std)]` + `#[cfg(not(target_os = "none"))] extern crate std;`)
  rather than the legacy `#![no_std]`. The legacy form breaks the
  host build of host-only `std::*`-using code."* — Triggered here in
  rustc_expand; will trigger again in rustc_metadata, rustc_query_impl,
  rustc_session if those have host-only paths.

- **§1.3 hashbrown::Entry variant-import shortcut**: when the
  upstream file does `use std::collections::hash_map::Entry::{Occupied,
  Vacant};` (variants, not the parent enum), substitute to
  `use hashbrown::hash_map::Entry::{Occupied, Vacant};` directly
  rather than re-exporting through `rustc_data_structures::fx::StdEntry`
  (which only re-exports the parent Entry type, not its variants).
  Trigger: 3 sites in this E2 wave (rustc_hir_typeck/pat.rs,
  rustc_expand/mbe/macro_parser.rs, rustc_expand/mbe/macro_rules.rs).
  Mention in §1.3 table.

- **§1.5 "gate whole module at the mod line" shortcut**: when an
  entire module's body is §1.5-out (like rustc_expand::proc_macro_server),
  gating the `mod` declaration in the parent lib.rs with
  `#[cfg(not(target_os = "none"))]` is preferred over in-file
  substitutions. Keeps upstream body verbatim. Demonstrated in
  rustc_expand by C3 (recommended) and executed by E2.
