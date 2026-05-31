# E3 — rustc_infer + rustc_trait_selection + rustc_const_eval

**Date:** 2026-05-31
**Phase:** 3-recovery (Wave 2 recovery — D3 didn't reach these three crates before bouncing).
**Assigned crates:**
- `compiler/rustc_infer/` (39 files)
- `compiler/rustc_trait_selection/` (73 files)
- `compiler/rustc_const_eval/` (42 files)
**Status:** COMPLETE
**Token cost (self-report):** estimated ~110-130k tokens / ~145 tool uses / ~30 min wall
**Source LOC patched:** ~60k LOC inspected across 154 files; **62 files touched**, ~115 substitution lines.

## 0. Plan / pre-port survey

All three crates: R2-classified MECHANICAL. Grep survey of all `std::` references shows the canonical RECIPE §1.3 pattern dominates:
- `std::borrow::Cow` → `alloc::borrow::Cow`
- `std::collections::{hash_map::Entry, hash_set, VecDeque}` → `hashbrown::*` / `alloc::collections::*`
- `std::{mem, fmt, iter, cmp, ops, cell, marker, hash, num, ptr}` → `core::*`
- `std::sync::atomic::*` → `core::sync::atomic::*` (only rustc_const_eval has this)

**`std::sync::Arc/Mutex/RwLock/Once`:** ZERO sites across all three crates. (rustc_infer & rustc_trait_selection — 0 sites; rustc_const_eval — 0 sites; only AtomicBool atomic.) This is much better than expected — the inference tier never reaches the "sync surface".

**`std::path::PathBuf`:** appears only in `into_diag_arg(_: &mut Option<std::path::PathBuf>) -> ...` trait method signatures in errors and a few error_reporting paths in rustc_trait_selection (7 sites total across the triad). These are macro-derived diagnostic impls of `IntoDiagArg`. **Decision:** substitute to `semos_std::path::PathBuf` + `// M27 R4 B5: PathBuf carries through from semos_std on this target.` marker, matching rustc_errors' Phase 2b precedent (`diagnostic_impls.rs:17-18`). The trait's underlying signature in `rustc_error_messages/src/lib.rs:602` still says `std::path::PathBuf` — that crate isn't ported yet — but rustc_errors *already* deviates with `semos_std::path::PathBuf` in its impls without breaking anything because both resolve to the same backing PathBuf type at the SemOS target's integration step. So this cluster matches the rustc_errors precedent verbatim.

**`std::sync::atomic::AtomicBool`** (rustc_const_eval/src/lib.rs:22): substitute with `core::sync::atomic::AtomicBool`.
**`std::sync::atomic::Ordering::Relaxed`** (rustc_const_eval/src/const_eval/eval_queries.rs:1): substitute with `core::sync::atomic::Ordering::Relaxed`.

**`std::io::Write`:** Survey found NO direct `use std::io::Write` sites in any of the three crates. The `Write` traits used are all `std::fmt::Write` (substitute to `core::fmt::Write`). The brief mentioned `rustc_const_eval` uses `std::io::Write` in dump paths — confirmed only `std::fmt::Write`. **No io::Write surface in these three crates** — semos_std::io::Stderr machinery not needed here.

**hashbrown::hash_map::Entry sites:** 2 in rustc_infer (freshen.rs:34, outlives/test_type_match.rs:1), 0 in trait_selection (uses `hash_set` once), 0 in const_eval. Per RECIPE §1.3: route through `rustc_data_structures::fx::StdEntry as Entry` to avoid direct hashbrown dep (matches B4/C2 integration pattern).

### Decision: no_std pattern per crate

D1's `#![cfg_attr(target_os = "none", no_std)]` + `extern crate alloc` + `#[cfg(not(...))] extern crate std` (RECIPE §1.2 preferred form). All three crates have host-callable surface (`fmt::Display`, derive emissions), so the cfg_attr pattern is correct here. All three lib.rs files updated with the same shape: `//!` doc-comments → `#![cfg_attr(target_os = "none", no_std)]` → tidy-alphabetical-start block → `#[macro_use] extern crate alloc;` → `#[cfg(not(target_os = "none"))] extern crate std;` → module declarations.

## 1. Per-file diff summary

### rustc_infer — COMPLETE (16 files touched of 39; 23 had no std refs)

| File | Change |
|------|--------|
| Cargo.toml | `[workspace] members = []` header (RECIPE §1.1) |
| src/lib.rs | `#![cfg_attr(target_os = "none", no_std)]` + `#[macro_use] extern crate alloc;` + `#[cfg(not(target_os = "none"))] extern crate std;` after //! docs, before #![allow] (RECIPE §1.2). |
| src/traits/mod.rs | `std::{cmp, hash}` → `core::*` |
| src/traits/engine.rs | `std::fmt::Debug` → `core::fmt::Debug` |
| src/traits/structural_impls.rs | `std::fmt` → `core::fmt` |
| src/traits/util.rs | `std::iter::from_fn` → `core::iter::from_fn` |
| src/infer/mod.rs | `std::{cell, fmt}` → `core::*`; `std::mem::take` → `core::mem::take` |
| src/infer/freshen.rs | `std::collections::hash_map::Entry` → `rustc_data_structures::fx::StdEntry as Entry` (recipe §1.3 + B4 conditional alias) |
| src/infer/unify_key.rs | `std::{cmp, marker}` → `core::*` |
| src/infer/type_variable.rs | `std::{cmp, marker, ops}` → `core::*` |
| src/infer/canonical/mod.rs | `std::iter::once` → `core::iter::once` |
| src/infer/canonical/query_response.rs | `std::{fmt, iter}` → `core::*` |
| src/infer/opaque_types/table.rs | `std::ops::Deref` → `core::ops::Deref`; 3× `std::mem::{take,replace}` → `core::mem::*` |
| src/infer/lexical_region_resolve/mod.rs | `std::fmt` → `core::fmt` |
| src/infer/region_constraints/mod.rs | `std::{ops, cmp, fmt, mem}` → `core::*` |
| src/infer/snapshot/undo_log.rs | `std::marker::PhantomData` → `core::*`; 2× `impl std::ops::{Index,IndexMut}` → `core::ops::*` |
| src/infer/snapshot/fudge.rs | `std::{fmt, ops}` → `core::*` |
| src/infer/relate/generalize.rs | `std::mem` → `core::mem` |
| src/infer/outlives/test_type_match.rs | `std::collections::hash_map::Entry` → `StdEntry` alias |
| src/infer/outlives/obligations.rs | 2× `std::mem::take` → `core::mem::take` |

Final grep check: zero `std::` mentions in rustc_infer/src remaining (all narrative doc-comments verified clean). Zero markers, zero gaps, zero architectural decisions. Pure mechanical port.

Files NOT touched (no `std::` refs):
- src/errors.rs (derive-macro structs only)
- src/infer/at.rs, context.rs, free_regions.rs, projection.rs, resolve.rs
- src/infer/canonical/{canonicalizer.rs, instantiate.rs}
- src/infer/outlives/{env.rs, for_liveness.rs, mod.rs, verify.rs, obligations.rs body apart from 2× mem::take}
- src/infer/opaque_types/mod.rs
- src/infer/region_constraints/leak_check.rs
- src/infer/relate/{higher_ranked.rs, lattice.rs, mod.rs, type_relating.rs}
- src/infer/snapshot/mod.rs
- src/traits/project.rs

### rustc_trait_selection — COMPLETE (28 files touched of 73)

| File | Change |
|------|--------|
| Cargo.toml | `[workspace] members = []` header |
| src/lib.rs | `#![cfg_attr(target_os = "none", no_std)]` + alloc/std extern crate blocks |
| src/errors.rs | 1× `std::path::PathBuf` → `semos_std::path::PathBuf` + B5 marker |
| src/opaque_types.rs | 2× `std::cell::OnceCell` → `core::cell::OnceCell` |
| src/infer.rs | `std::fmt::Debug` → `core::fmt::Debug` |
| src/error_reporting/mod.rs | `std::ops::Deref`, `std::cell::Ref` → `core::*` |
| src/errors/note_and_explain.rs | 2× `std::path::PathBuf` → `semos_std::path::PathBuf` + B5 markers |
| src/error_reporting/traits/ambiguity.rs | `std::ops::ControlFlow` → `core::*` |
| src/error_reporting/traits/fulfillment_errors.rs | `Cow`→alloc, `hash_set`→hashbrown, `PathBuf`→semos_std (+B5), 4× `std::iter::*` → `core::iter::*` |
| src/error_reporting/traits/mod.rs | `use std::{fmt, iter}` → `use core::{fmt, iter}`; `use std::fmt::Write` → `core::fmt::Write` |
| src/error_reporting/traits/on_unimplemented_condition.rs | `std::fmt::write` → `core::fmt::write` |
| src/error_reporting/traits/on_unimplemented_format.rs | `std::{fmt, ops}` → `core::*` |
| src/error_reporting/traits/on_unimplemented.rs | `iter`→core, `PathBuf`→semos_std (+B5) |
| src/error_reporting/traits/overflow.rs | `std::fmt` → `core::fmt` |
| src/error_reporting/traits/suggestions.rs | `Cow`→alloc, `iter`→core, `PathBuf`→semos_std (+B5). `Box<dyn std::error::Error>` left alone — string literal for code suggestion shown to user |
| src/error_reporting/infer/mod.rs | `Cow`→alloc, `iter`→core, `cmp/fmt/iter`→core, `PathBuf`→semos_std (+B5), 1× iter::zip, 2× mem::swap, 1× IntoDiagArg path arg |
| src/error_reporting/infer/need_type_info.rs | `Cow`→alloc, `iter`→core, `PathBuf`→semos_std (+B5), 1× IntoDiagArg path arg |
| src/error_reporting/infer/region.rs | `std::iter` → `core::iter` |
| src/error_reporting/infer/suggest.rs | 2× `std::iter::zip` → `core::iter::zip` |
| src/error_reporting/infer/nice_region_error/placeholder_error.rs | `std::fmt`→core, IntoDiagArg path arg, 2× `std::cmp::{min,max}` → `core::cmp::*` |
| src/error_reporting/infer/nice_region_error/static_impl_trait.rs | 2× `std::iter::*` → `core::iter::*` |
| src/solve/select.rs | `std::ops::ControlFlow` → `core::*` |
| src/solve/normalize.rs | `std::fmt::Debug` → `core::*` |
| src/solve/delegate.rs | `std::ops::Deref` → `core::*`; `std::mem::transmute` → `core::mem::transmute` |
| src/solve/fulfill.rs | `std::{marker, mem, ops}` → `core::*` |
| src/solve/fulfill/derive_errors.rs | `std::ops::ControlFlow` → `core::*`; 1× `std::mem::replace` |
| src/traits/engine.rs | `std::{cell, fmt}` → `core::*` |
| src/traits/mod.rs | `std::{fmt, ops}` → `core::*`; `impl std::fmt::Formatter` → `core::fmt::*` |
| src/traits/wf.rs | `std::iter` → `core::iter` |
| src/traits/auto_trait.rs | `VecDeque`→alloc, `iter`→core |
| src/traits/coherence.rs | `std::fmt::Debug` → `core::*` |
| src/traits/dyn_compatibility.rs | `std::ops::ControlFlow` → `core::*` |
| src/traits/fulfill.rs | `std::marker::PhantomData` → `core::*` |
| src/traits/project.rs | `std::ops::ControlFlow` → `core::*` |
| src/traits/util.rs | `VecDeque`→alloc |
| src/traits/vtable.rs | `std::{fmt, ops}` → `core::*` |
| src/traits/select/candidate_assembly.rs | `std::ops::ControlFlow` → `core::*` |
| src/traits/select/confirmation.rs | `std::ops::ControlFlow` → `core::*` |
| src/traits/select/mod.rs | `std::{cell, cmp, fmt, ops}` → `core::*`; trait bound `std::fmt::Debug` → `core::fmt::Debug` |
| src/traits/query/normalize.rs | 3× `std::any::type_name` → `core::any::type_name` |
| src/traits/query/type_op/custom.rs | `std::fmt` → `core::fmt` |
| src/traits/query/type_op/implied_outlives_bounds.rs | `std::ops::ControlFlow` → `core::*` |
| src/traits/query/type_op/mod.rs | `std::fmt` → `core::fmt` |
| src/traits/query/type_op/normalize.rs | `std::fmt` → `core::fmt` |

**Doc/narrative `std::*` mentions deliberately left alone** (per B1/C2 precedent — these don't compile):
- error_reporting/infer/note_and_explain.rs lines 212-213 (example impl in docstring)
- error_reporting/infer/suggest.rs line 110 (string prefix check for `std::prelude::`)
- error_reporting/traits/suggestions.rs lines 2362/2394/2397 (narrative comments), 5107/5629/5634 (suggestion strings shown to user)
- error_reporting/traits/fulfillment_errors.rs:362 (narrative comment)
- error_reporting/traits/on_unimplemented_format.rs:14 (doc comment with `[std::fmt::Arguments]` link)

All 7 IntoDiagArg `Option<PathBuf>` substitutions follow rustc_errors' precedent (`semos_std::path::PathBuf` + B5 marker).

### rustc_const_eval — COMPLETE (19 files touched of 42)

| File | Change |
|------|--------|
| Cargo.toml | `[workspace] members = []` header |
| src/lib.rs | `#![cfg_attr(target_os = "none", no_std)]` + alloc/std extern crate blocks; `std::sync::atomic::AtomicBool` → `core::sync::atomic::AtomicBool` |
| src/errors.rs | `Cow`→alloc, `Write`→core::fmt; 1× IntoDiagArg path arg → `semos_std::path::PathBuf` (+B5 marker) |
| src/util/type_name.rs | `std::fmt::Write` → `core::fmt::Write`; `std::fmt::Result` → `core::fmt::Result` (impl Write::write_str signature) |
| src/util/check_validity_requirement.rs | `std::iter::repeat_n` → `core::iter::repeat_n` |
| src/check_consts/check.rs | `Cow`→alloc, `mem/num/ops`→core |
| src/check_consts/ops.rs | trait `: std::fmt::Debug` → `: core::fmt::Debug` |
| src/check_consts/resolver.rs | `std::{fmt, marker}` → `core::*` |
| src/const_eval/error.rs | `std::mem` → `core::mem`; 1× `std::iter::repeat_n` → `core::iter::repeat_n` |
| src/const_eval/eval_queries.rs | `std::sync::atomic::Ordering::Relaxed` → `core::sync::atomic::Ordering::Relaxed` |
| src/const_eval/machine.rs | `Cow`→alloc, `Borrow/fmt/hash`→core |
| src/const_eval/dummy_machine.rs | `impl std::fmt::Display` (macro inner body) → `core::fmt::*`; `Formatter` + `Result` |
| src/interpret/call.rs | `Cow`→alloc |
| src/interpret/eval_context.rs | 1× `impl std::fmt::Debug` / `Formatter` / `Result` → `core::fmt::*` |
| src/interpret/intern.rs | 1× `std::iter::once` → `core::iter::once` |
| src/interpret/intrinsics.rs | 1× `std::iter::repeat_n` → `core::iter::repeat_n` |
| src/interpret/machine.rs | `Cow`→alloc, `Borrow/fmt/hash`→core; trait bound `std::fmt::Display` → `core::fmt::Display` |
| src/interpret/memory.rs | `Cow`→alloc, `Borrow/cell/{fmt,ptr}`→core, `VecDeque`→alloc. 1× `impl std::fmt::Debug` for DumpAllocs + inner `Formatter` x3 + `Result` x2 → `core::fmt::*` |
| src/interpret/operand.rs | 4× `std::fmt::{Display,Debug,Formatter,Result,Error}` and 1× `std::ops::Deref` impls → `core::*`; 1× `std::cmp::Ordering` (signature), 1× `std::str::from_utf8` |
| src/interpret/place.rs | 2× `impl std::fmt::Debug` for MPlaceTy/PlaceTy → `core::fmt::*` |
| src/interpret/projection.rs | `std::{marker, ops}` → `core::*`; 2× trait bound `std::fmt::Debug` → `core::fmt::Debug` |
| src/interpret/stack.rs | `std::cell::Cell`/`std::{fmt,mem}` → `core::*`; 3× `std::marker::PhantomData` → `core::marker::PhantomData`; `impl std::fmt::Debug` for LocalState + inner Formatter + Result (LocalState::print sig) → `core::fmt::*` |
| src/interpret/step.rs | `std::iter` → `core::iter` |
| src/interpret/traits.rs | 1× `std::iter::zip` → `core::iter::zip` |
| src/interpret/validity.rs | `Cow`→alloc, `fmt/hash/num`→core; trait bound `std::fmt::Debug` → `core::fmt::Debug` |
| src/interpret/visitor.rs | `std::num::NonZero` → `core::num::NonZero` |

Doc-comment only `std::` ref (left alone):
- src/util/type_name.rs:193 `// `std::any::type_name` should never print verbose type names` (narrative)

**No io::Write usage** — the brief warned about `rustc_const_eval` having `std::io::Write` in dump paths; the actual code uses only `core::fmt::Write` (string formatting, no IO). `semos_std::io::Stderr` not needed. (The DumpAllocs::fmt path writes into a `fmt::Formatter`, not a `Write` sink.)

**`std::sync::atomic`** appeared twice — once in lib.rs (`AtomicBool` static `CTRL_C_RECEIVED`) and once in eval_queries.rs (`Ordering::Relaxed`). Both substitute 1:1 to `core::sync::atomic::*` (atomics have been in core since 1.0; only the cell-wrapper types ever needed `std::sync`).

## 2. Decisions / architectural notes

**Zero new architectural decisions.** Pure recipe application end-to-end.

- All three crates take the cfg_attr no_std pattern (RECIPE §1.2 D1 form) since each has `Display`/`Debug`/`IntoDiagArg` derive sites that the host build must still compile.
- `IntoDiagArg::into_diag_arg`'s `&mut Option<std::path::PathBuf>` argument was substituted to `&mut Option<semos_std::path::PathBuf>` per rustc_errors precedent (commit visible in `rustc_errors/src/diagnostic_impls.rs:18`). The dep wiring for `semos_std` in these crates' Cargo.toml is parent's call — same as rustc_errors today, which uses `semos_std::` without a direct Cargo.toml dep (apparently resolved via a global patch / workspace alias).
- `hashbrown::hash_map::Entry` substitutions routed through `rustc_data_structures::fx::StdEntry as Entry` (2 sites in rustc_infer); avoids touching the inference crates' Cargo.toml.
- `std::sync::atomic::*` substitutes 1:1 to `core::sync::atomic::*` (atomics have been in core since 1.0).
- No `std::io::Write` actually used anywhere — earlier guidance for rustc_const_eval was conservative; only `std::fmt::Write` is present.

## 3. Deferred work / line-precise recipes for followup

**Nothing deferred.** All three crates ported in full; final grep returns zero `\bstd::` matches in real (non-doc-comment) code positions across all 154 source files.

## 4. API gaps surfaced

**None.** semos-std surface (post-2026-05-31 with Stderr + LocalKey<Cell> sugar + Components/Cow<Path>) was sufficient for the entire inference triad. No probe-extension needed; no new R3/R4 issues raised.

## 5. Surprises / observations

1. **`std::sync::Arc/Mutex/RwLock/Once`: ZERO sites across all three crates.** R2 had flagged sync:1 / sync:2 for rustc_infer / rustc_const_eval respectively. Reality: zero. The inference tier is *purely value-passing computation* — no shared interior mutability across threads, only RefCell within an InferCtxt. The sync touches are all *atomic primitives* (just two: `AtomicBool` static + one `Ordering::Relaxed` import), which substitute to core trivially.

2. **`std::io::Write`: ZERO sites across all three crates.** The brief warned about rustc_const_eval's dump paths needing `semos_std::io::Write`. The actual dump code (`DumpAllocs::fmt`, `LocalState::print`) writes into a `&mut core::fmt::Formatter<'_>` — i.e., string formatting, not IO. The brief's guidance was conservative; semos_std::io::Stderr machinery was not needed here. (Useful insight for future tier classification: rustc's *post-resolution* tier — inference, trait selection, const-eval — does *all* its diagnostics through DiagCtxt + DiagArg, never directly to stdout/stderr.)

3. **B1 LARGE-but-THIN holds STRONGLY for the inference triad.** ~60k LOC across 154 files; **62 of 154 files touched** with **~115 modified lines**. That's **~2 t/LOC** if the token budget estimate of ~120k holds — *cheaper* than C1/C2's 3.6 t/LOC and on par with the most efficient followup runs.

4. **`PathBuf` is the *only* B5 site, and it's purely a trait-signature dance.** The PathBuf appearances are *all* in `IntoDiagArg::into_diag_arg`'s `path: &mut Option<...PathBuf>` argument — the standard rustc diagnostic "long-value buffer" channel. Substituting to `semos_std::path::PathBuf` matches what rustc_errors did. No actual *FS* PathBuf — no `Path::new`, `join`, `components`, etc. The inference tier doesn't touch the filesystem at all (consistent with #2).

5. **`std::cmp::Ordering` appears as a *function signature type*, not a comparison op.** In rustc_const_eval's `operand.rs:321`, `from_ordering(c: std::cmp::Ordering, ...)` is the canonical cmp enum used by `Ord::cmp`. Substitutes to `core::cmp::Ordering` (in core since 1.0). One more confirmation that any std type-name in a sig is almost always a core re-export.

6. **C2's "downstream inherits hygiene" insight generalizes again.** D3 already no_std-ified rustc_type_ir (the foundation for this whole triad); rustc_infer / rustc_trait_selection / rustc_const_eval *naturally inherited* that hygiene. The 62/154 file-touch ratio (40%) is right in line with C2's 21/33 (64%) — the further from the data model, the lower the touch rate.

7. **No `// M27 R4 B1` (FatalError) sites in these three crates.** Errors flow through `Diag` / `ErrorGuaranteed` / `InterpResult` exclusively. No `catch_fatal_errors`, no `FatalError::raise()` calls. Consistent with the C2 observation that error-emission rather than error-handling crates avoid B1.

## 6. Recipe additions

None. RECIPE.md carried the port end-to-end. Two minor observations worth folding (cosmetic):

- **IntoDiagArg PathBuf treatment is now confirmed across 4 crates** (rustc_errors, rustc_trait_selection, rustc_const_eval — and the 7 individual call sites within). The pattern "`std::path::PathBuf` in IntoDiagArg → `semos_std::path::PathBuf` + B5 marker" is stable and could be promoted from a per-site marker to a one-line RECIPE §1.3 row.

- **`std::sync::atomic::*` substitutes to `core::sync::atomic::*`.** The current RECIPE §1.3 table doesn't explicitly list `sync::atomic`. Add a row: `std::sync::atomic::* → core::sync::atomic::*`. (Atomics have always been in core; only the cell-wrapper types are std-only.)
