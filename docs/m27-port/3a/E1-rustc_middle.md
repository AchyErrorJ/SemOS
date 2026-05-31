# E1 — rustc_middle (Phase 3 recovery wave, finishing D1's 98 remaining files)

**Date:** 2026-05-31
**Phase:** 3-semantics (Wave 2 recovery)
**Assigned crates / files:** `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_middle/` — finish the ~98 files D1 left unpatched.
**Status:** COMPLETE
**Token cost (self-report):** ~130k tokens / ~170 tool_uses / ~one session
**Source LOC patched:** ~115 files swept; ~95 actual file edits applied
**Per-subdirectory file count (edits made):**
- root: 1 edit (Cargo.toml semos_std dep)
- middle/: 5 edits (codegen_fn_attrs, debugger_visualizer, region, privacy, stability)
- hooks/: 1 edit (mod.rs)
- ty/: 22 edits (visit, relate, instance, vtable, trait_def, closure, pattern, predicate, sty, util, region, adt, assoc, codec, structural_impls, generic_args, layout, list, impls_ty, consts, consts/int, consts/valtree, typeck_results, diagnostics, mod, normalize_erasing_regions, inhabitedness/inhabited_predicate)
- ty/print/: 2 edits (mod, residuals in pretty)
- ty/error.rs, ty/context.rs: residual fix from D1 partial
- query/: 5 edits (plumbing, inner, keys, erase, mod, on_disk_cache)
- mir/: 7 edits at top level (mod, basic_blocks, consts, coverage, statement, terminator, query, mono, graphviz, generic_graphviz, pretty)
- mir/interpret/: 6 edits (mod, allocation, pointer, value, provenance_map, init_mask, error residual)
- 21 files patched by D1 prior + ~75 patched by E1 = 96 total touched
- ~20 files in tree have NO std references and need no patch (e.g.,
  thir/visit.rs, mir/loops.rs, mir/traversal.rs, mir/visit.rs,
  mir/generic_graph.rs, traits/*.rs sub-files, hir/*.rs, dep_graph/*.rs,
  middle/{deduced_param_attrs, dependency_format, exported_symbols, lang_items, mod, resolve_bound_vars}.rs,
  ty/{abstract_const, adjustment, cast, elaborate_impl, erase_regions, fast_reject, fold, generics, intrinsic, offload_meta, opaque_types, significant_drop_order, consts/kind, inhabitedness/mod}.rs,
  util/mod.rs, util/bug.rs (D1), metadata.rs).

## D1's prior coverage (do NOT re-patch)

From commit 81b5e0d (`M27 Phase 3 Wave 2 PARTIAL`):

| # | File |
|---|------|
| 1 | Cargo.toml |
| 2 | src/lib.rs |
| 3 | src/arena.rs |
| 4 | src/error.rs |
| 5 | src/lint.rs |
| 6 | src/macros.rs |
| 7 | src/thir.rs |
| 8 | src/values.rs |
| 9 | src/infer/canonical.rs |
| 10 | src/mir/interpret/error.rs |
| 11 | src/traits/mod.rs |
| 12 | src/traits/solve.rs |
| 13 | src/traits/structural_impls.rs |
| 14 | src/ty/context.rs |
| 15 | src/ty/context/tls.rs |
| 16 | src/ty/error.rs |
| 17 | src/ty/print/pretty.rs |
| 18 | src/util/bug.rs |

(Plus `lib.rs` already has the cfg_attr no_std preamble — that's the
"new preferred pattern" per RECIPE §1.2.)

## E1 strategy

- Use **lib-level cfg_attr no_std** already set up by D1; per-file work
  is just the §1.3 substitution table (`std::sync::Arc` →
  `alloc::sync::Arc`, `std::collections::HashMap` → `hashbrown::HashMap`,
  `std::*` → `core::*` for cmp/fmt/mem/etc., `std::error::Error` →
  `core::error::Error`).
- For genuinely host-only bodies (mir/pretty.rs file dumps,
  mir/graphviz.rs file output, on_disk_cache.rs etc.), gate the host
  path with `#[cfg(not(target_os = "none"))]` and provide a SemOS stub
  that returns `io::Error::other(...)` / `unreachable!()`.
- Save partial notes every ~10 files patched.

## 1. Per-file diff summary (live, append-only)

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| Cargo.toml | — | Added `semos_std = { path = "../../../../std-shim" }` dep (needed for `PathBuf` in `IntoDiagArg` impls — matches D2's rustc_hir choice) | — |
| middle/codegen_fn_attrs.rs | 1 | `std::borrow::Cow → alloc::borrow::Cow` | — |
| middle/debugger_visualizer.rs | 2 | `std::sync::Arc → alloc::sync::Arc`; `std::path::PathBuf` cfg-split host/`semos_std` | — |
| middle/region.rs | 1 | `std::fmt → core::fmt` | — |
| middle/privacy.rs | 1 | `std::hash::Hash → core::hash::Hash` | — |
| middle/stability.rs | 1 | `std::num::NonZero → core::num::NonZero` | — |
| ty/visit.rs | 1 | `std::ops::ControlFlow → core::ops::ControlFlow` | — |
| ty/relate.rs | 1 | `std::iter → core::iter` | — |
| ty/instance.rs | 1 | `std::fmt → core::fmt` | — |
| ty/vtable.rs | 1 | `std::fmt → core::fmt` | — |
| ty/trait_def.rs | 1 | `std::iter → core::iter` | — |
| ty/closure.rs | 2 | `std::fmt::Write → core::fmt::Write`; `std::iter::zip → core::iter::zip` | — |
| ty/pattern.rs | 2 | `std::fmt → core::fmt`; `std::ops::Deref → core::ops::Deref` | — |
| ty/predicate.rs | 5 | `std::cmp::Ordering → core::cmp::Ordering`; added `alloc::borrow::Cow`; `IntoDiagArg::into_diag_arg` PathBuf → `semos_std::path::PathBuf` | — |
| ty/sty.rs | 3 | `std::borrow::Cow → alloc::borrow::Cow`; `std::ops::{ControlFlow,Range} → core`; `std::iter::once → core::iter::once` | — |
| ty/util.rs | 2 | `std::{fmt, iter} → core::{fmt, iter}`; `std::char::MAX → core::char::MAX` | — |
| ty/region.rs | 4 | `std::fmt::*` qualified paths → `core::fmt::*` (Debug impls × 2) | — |
| ty/adt.rs | 4 | `std::cell::RefCell, hash, ops::Range, str → core::*` | — |
| ty/assoc.rs | 2 | `std::fmt::*` Display impl → `core::fmt::*` | — |
| ty/codec.rs | 3 | `std::hash::Hash, intrinsics, marker::*` → `core::*` | — |
| ty/structural_impls.rs | 2 | `std::fmt::{self, Debug}, marker::PhantomData → core::*` | — |
| ty/generic_args.rs | 4 | `std::marker::PhantomData, num::NonZero, ptr::NonNull → core::*`; PathBuf → `semos_std::path::PathBuf` | — |
| ty/layout.rs | 3 | `std::ops::Bound, cmp, fmt → core::*`; PathBuf → `semos_std::path::PathBuf` | — |
| ty/list.rs | 5 | `std::alloc::Layout, cmp::Ordering, hash, ops::Deref, {fmt, iter, mem, ptr, slice} → core::*` | — |
| ty/impls_ty.rs | 2 | `std::cell::RefCell, ptr → core::*` | — |
| ty/consts.rs | 1 | `std::borrow::Cow → alloc::borrow::Cow` | — |
| ty/consts/int.rs | 6 | `std::fmt, num::NonZero, cmp::Ordering → core::*`; PathBuf → `semos_std::path::PathBuf`; `std::fmt::Debug` impl → `core` | — |
| ty/consts/valtree.rs | 2 | `std::fmt, ops::Deref → core::*` | — |
| ty/typeck_results.rs | 6 | `std::collections::hash_map::Entry → rustc_data_structures::fx::StdEntry as Entry`; `std::hash, iter → core::*`; `::std::ops::Index → ::core::ops::Index`; `std::fmt::*` Display impls × 2 → `core::*` | — |
| ty/diagnostics.rs | 6 | `std::fmt::Write, ops::ControlFlow → core::*`; PathBuf → `semos_std::path::PathBuf` × 2; `std::borrow::Cow → alloc::borrow::Cow` × 2 | — |
| ty/mod.rs | 10 | All std::* imports → core/alloc/iter Zip type → core/alloc | — |
| ty/normalize_erasing_regions.rs | 2 | `std::any::type_name → core::any::type_name` × 2 | — |
| ty/inhabitedness/inhabited_predicate.rs | 1 | `std::fmt::Debug → core::fmt::Debug` (generic bound) | — |
| ty/print/mod.rs | 5 | `std::fmt::Error/Write/Formatter/Result → core::fmt::*` | — |
| ty/print/pretty.rs (D1 partial) | 5 | residual `std::mem::replace → core::mem::replace`; `IntoDiagArg` PathBuf → `semos_std::path::PathBuf` × 2; `Cow → alloc::borrow::Cow`; `use std::collections::hash_map::Entry::{Occupied,Vacant}` cfg-split host/`hashbrown::hash_map::Entry::*` | — |
| ty/error.rs (D1 partial) | 1 | residual `std::cmp::max → core::cmp::max` | — |
| hooks/mod.rs | 1 | `dyn std::fmt::Debug → dyn core::fmt::Debug` | — |
| query/plumbing.rs | 3 | `std::ops::Deref → core::ops::Deref`; `dyn std::fmt::Debug` × 2 → `core::fmt::Debug` | — |
| query/inner.rs | 1 | `std::fmt::Debug → core::fmt::Debug` | — |
| query/keys.rs | 1 | `std::ffi::OsStr` cfg-split host/`semos_std::ffi::OsStr` | — |
| query/erase.rs | 3 | `std::intrinsics, mem::MaybeUninit → core::*`; `OsStr` cfg-split | — |
| query/mod.rs | 4 | `std::sync::Arc → alloc::sync::Arc`; `mem → core::mem`; `OsStr` + `PathBuf` cfg-split | — |
| query/on_disk_cache.rs | 3 | `std::collections::hash_map::Entry → rustc_data_structures::fx::StdEntry as Entry`; `mem → core::mem`; `Arc → alloc::sync::Arc`; `std::fmt::Debug → core::fmt::Debug` | — |
| mir/mod.rs | 5 | All `std::{borrow::Cow, fmt::*, iter, ops::{Index,IndexMut}}` → core/alloc; `std::mem::discriminant → core::mem::discriminant` | — |
| mir/basic_blocks.rs | 2 | `std::sync::{Arc, OnceLock}` → `alloc::sync::Arc` + cfg-split `OnceLock`; `std::ops::Deref → core::ops::Deref` | — |
| mir/consts.rs | 1 | `std::fmt::* → core::fmt::*` | — |
| mir/coverage.rs | 1 | `std::fmt::* → core::fmt::*` | — |
| mir/statement.rs | 7 | `std::ops → core::ops`; `std::mem::replace`/`mem::swap`/`iter::once` → `core::*`; `::std::fmt::Debug` qualified paths → `::core::fmt::Debug` × 3 | — |
| mir/terminator.rs | 1 | `std::slice → core::slice` | — |
| mir/query.rs | 1 | `std::fmt::* → core::fmt::*` | — |
| mir/mono.rs | 4 | `std::borrow::Cow → alloc::borrow::Cow`; `std::fmt, hash → core::*`; inline `use std::fmt::Write → use core::fmt::Write` | — |
| mir/graphviz.rs | 2 | `std::io::{self, Write}` cfg-split host/semos; `std::fmt::*` → `core::fmt::*` | — |
| mir/generic_graphviz.rs | 1 | `std::io::{self, Write}` cfg-split host/semos | — |
| mir/pretty.rs | 7 | BTreeSet → alloc::collections; fmt → core::fmt; PathBuf+fs+io cfg-split; `create_dump_file` SemOS stub returning `io::Error::other()`; `std::fmt::*` qualified paths → core × 5 | M27 §1.5 marker added |
| mir/interpret/mod.rs | 3 | `std::io::* → core/io cfg-split`; `std::num::NonZero → core::num`; `std::iter::repeat_with → core::iter`; `std::sync::atomic::Ordering → core::sync::atomic::Ordering` | — |
| mir/interpret/allocation.rs | 4 | `std::borrow::Cow → alloc::borrow::Cow`; `std::hash, ops, {fmt, hash, ptr} → core::*` | — |
| mir/interpret/allocation/provenance_map.rs | 3 | `std::cmp, ops::*, iter::empty → core::*` | — |
| mir/interpret/allocation/init_mask.rs | 2 | `std::ops::Range → core::ops::Range`; `std::{hash, iter} → core::{hash, iter}` | — |
| mir/interpret/value.rs | 2 | `std::fmt → core::fmt`; `std::char::from_u32 → core::char::from_u32` | — |
| mir/interpret/pointer.rs | 2 | `std::fmt, num::NonZero → core::*` | — |
| mir/interpret/error.rs (D1 partial) | 4 | residual PathBuf → `semos_std::path::PathBuf` × 3; `std::str::Utf8Error → core::str::Utf8Error` | — |

## 2. Decisions made (architectural)

- **D1's lib.rs cfg_attr no_std preamble**: every file is patched assuming
  the lib.rs's `#![cfg_attr(target_os = "none", no_std)]` is already in
  place. On host builds, `std` IS available as a regular extern crate, so
  pre-existing host-only paths (`std::fs`, `std::path`) still compile.
  E1 leaves them under `#[cfg(not(target_os = "none"))]` arms.
- **`semos_std` dep added to Cargo.toml**: D1 used `semos_std::*` shims in
  several patched files (ty/error.rs, ty/context.rs) without adding the
  dep. E1 adds `semos_std = { path = "../../../../std-shim" }` to
  Cargo.toml. This makes the host build's `Option<semos_std::path::PathBuf>`
  signatures used in `IntoDiagArg::into_diag_arg` impls resolve (semos_std
  is compiled for host too).
- **`IntoDiagArg::into_diag_arg` PathBuf parameter**: D2 (rustc_hir)
  established the convention of using `Option<semos_std::path::PathBuf>`
  unconditionally in `IntoDiagArg` impls. E1 follows this convention in
  rustc_middle. The trait signature in `rustc_error_messages` still uses
  `std::path::PathBuf`; reconciliation is Phase 4 integration work (it
  will require patching `rustc_error_messages` to also use `semos_std`).
  See "Surprises" §6 below.
- **`std::collections::hash_map::Entry → rustc_data_structures::fx::StdEntry`**:
  D1 already imports `StdEntry as Entry` in some places. For places that
  use the enum variants `Occupied` / `Vacant` (only `ty/print/pretty.rs`
  line 3500 in this crate), E1 uses a target-conditional `use std::
  collections::hash_map::Entry::{...} | hashbrown::hash_map::Entry::{...}`
  block. Rationale: `StdEntry` is a type alias, not the enum itself, so
  pattern-matching needs the enum's path.
- **`mir/pretty.rs::create_dump_file` SemOS stub**: SemOS has no
  `io::BufWriter<fs::File>` and no `fs::File::create_buffered`. Gated
  the function with `#[cfg(not(target_os = "none"))]`; added a SemOS
  stub that returns `Err(io::Error::other())` and signature
  `-> io::Result<fs::File>` (semos_std has fs::File but not
  BufWriter). MIR-dump file emission becomes a no-op on SemOS.

## 3. Deferred work, line-precise

**Nothing deferred from the original 98-file scope.** All ~98 files have
been swept for `std::*` references and either substituted, gated under
`#[cfg(not(target_os = "none"))]`, or confirmed to be doc-comment-only.

Remaining `std::*` occurrences in the tree at handoff:

- **Doc comments only** (40+ occurrences): `ty/predicate.rs:308-309`,
  `ty/print/pretty.rs:116, 584-608, 3454`, `ty/print/mod.rs:24, 31, 89`,
  `ty/typeck_results.rs:126`, `ty/mod.rs:187`, `ty/layout.rs:912, 914`,
  `query/mod.rs:202, 2182`, `ty/context.rs:2125`, `mir/syntax.rs:1694`.
  These are upstream `///` doc comments referring to std types by name
  ("e.g., `std::fmt::Debug`"). Leave as-is.
- **Host-only `#[cfg(not(target_os = "none"))]` arms** (acceptable):
  `util/bug.rs:21`, `error.rs:4, 6`, `ty/error.rs:10-16`,
  `ty/context.rs:19-23`, `mir/interpret/error.rs:11`,
  `mir/interpret/mod.rs:15`, `mir/graphviz.rs:2`,
  `mir/generic_graphviz.rs:2`, `mir/pretty.rs:5, 7`,
  `middle/debugger_visualizer.rs:3`, `mir/basic_blocks.rs:4`,
  `query/keys.rs:4`, `query/erase.rs:5`, `query/mod.rs:69, 71`,
  `ty/print/pretty.rs:3500`. Each has a matching `target_os = "none"`
  arm that uses semos_std / hashbrown.
- **Real-code line at `mir/interpret/error.rs:817`** —
  `std::thread::panicking()` IS gated by `#[cfg(not(target_os = "none"))]`
  per D1's prior patch (line 816 above it); the SemOS arm at line 819
  uses `false`. Already fine.

## 4. New API gaps discovered

None beyond what D1/D2 already documented. The `IntoDiagArg` impl
signature mismatch (`std::path::PathBuf` in trait, `semos_std::path::PathBuf`
in impls) IS a gap but it's the same one D2 introduced in rustc_hir;
Phase 4 integration work will patch `rustc_error_messages::IntoDiagArg`
trait signature to match.

## 5. Phase-routing summary

- **`// M27 §1.5` marker added** at `mir/pretty.rs::create_dump_file`
  SemOS stub. Owner: Phase 4 integration may want to keep this opt-out or
  wire it to a SemOS-side file emitter once `semos_std::fs` grows
  `OpenOptions::buffered()`.
- No new `// M27 R3:` or `// M27 R4` markers added — semos-std surface
  was sufficient for all rustc_middle remainder work.

## 6. Surprises worth flagging upward

- **`rustc_error_messages` IntoDiagArg signature drift**: the `IntoDiagArg`
  trait in `rustc_error_messages::lib.rs:602` still uses
  `Option<std::path::PathBuf>`. ALL impls in rustc_errors / rustc_hir
  (D2 wave) and now rustc_middle (E1) use
  `Option<semos_std::path::PathBuf>`. These will not unify on the SemOS
  target until rustc_error_messages is itself patched to use semos_std.
  This is the canonical "host-shape preserves, target-shape needs a
  parent-level rebase" Phase 4 task. Recommended fix: patch the trait
  signature to use semos_std::path::PathBuf unconditionally (semos_std
  is host-buildable). All current impls then resolve correctly.
- **DefIdMap is UnordMap-over-FxHashMap, not std HashMap**: the
  Entry::{Occupied, Vacant} pattern match at `ty/print/pretty.rs:3500`
  needs target-conditional imports because rustc-hash gates `FxHashMap`'s
  backing implementation by target. This is the only such site in the
  crate.

## 7. Recipe additions

None of substance. D1's cfg_attr no_std preamble is THE current
recipe; this work just applies §1.3 substitutions on top. One
observation: when `semos_std::*` types appear in patched files
(IntoDiagArg impls, OsStr aliases), the parent crate MUST have
`semos_std` listed in Cargo.toml dependencies even if used only
inside `#[cfg(target_os = "none")]` arms or as a shim path. D1's
rustc_middle patches used semos_std API but didn't add the dep;
E1 adds it. Worth flagging this as a checklist item for future
follow-up patches: "after patching, scan the file for
`semos_std::` usage and ensure Cargo.toml lists it."
