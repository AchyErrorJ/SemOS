# C2 — rustc_ast_pretty + rustc_ast_lowering + rustc_ast_passes

**Date:** 2026-05-31
**Phase:** 3-frontend (Wave 1, "AST-tail" cluster — three downstream crates of B1's rustc_ast)
**Assigned crates / files:**
- `compiler/rustc_ast_pretty/` (12 files, ~4,901 LOC)
- `compiler/rustc_ast_lowering/` (15 files inc. messages.ftl, ~11,063 LOC)
- `compiler/rustc_ast_passes/` (6 files inc. messages.ftl, ~3,662 LOC)
**Status:** COMPLETE
**Token cost (self-report):** ~70k tokens / ~75 tool uses / ~25 min wall
**Source LOC patched:** ~19,626 LOC across 33 files; only ~21 lines actually modified.

The B1 LARGE-but-THIN insight held end-to-end: across 19.6k LOC, raw
`std::` mentions in real (non-doc, non-comment) code totaled **24
substitution sites** spread over 14 of the 33 files. All three crates
are downstream of the already-ported rustc_ast (B1) and inherit its
no_std hygiene; their own std touch is dominated by `core`-shape
items (`core::mem`, `core::ops`, `core::fmt`, `core::iter`) plus a
handful of `alloc::sync::Arc` and `alloc::borrow::Cow` imports.
Zero markers added. Zero architectural decisions. Zero new semos-std
API gaps. One Cargo.toml integration flag (hashbrown dep, §4).

## 1. Per-file diff summary

### rustc_ast_pretty (no markers)

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 17 → 20 | `[workspace] members = []` header. | none |
| `src/lib.rs` | 8 → 18 | `#![no_std]` first (before the `#![feature]` block per A2-followup lesson) + `#[macro_use] extern crate alloc;`. | none |
| `src/helpers.rs` | 40 | `std::borrow::Cow` → `alloc::borrow::Cow`. | none |
| `src/pp.rs` | 444 | Line 138-140: `std::borrow::Cow` → `alloc::`, `std::collections::VecDeque` → `alloc::collections::VecDeque`, `std::{cmp, iter}` → `core::{cmp, iter}`. Line 321: inline `std::mem::forget` → `core::mem::forget`. | none |
| `src/pp/convenience.rs` | 79 | `std::borrow::Cow` → `alloc::borrow::Cow`. | none |
| `src/pp/ring.rs` | 64 | `std::collections::VecDeque` → `alloc::`, `std::ops::{Index, IndexMut}` → `core::ops::*`. | none |
| `src/pprust/mod.rs` | 80 | `std::borrow::Cow` → `alloc::borrow::Cow`. | none |
| `src/pprust/state.rs` | 2,048 | `std::borrow::Cow` → `alloc::`, `std::sync::Arc` → `alloc::sync::Arc`. Three mid-file `impl std::ops::Deref/DerefMut` and one `trait PrintState<'a>: std::ops::Deref + std::ops::DerefMut` bound (lines 432, 439, 446) → `core::ops::*`. | none |
| `src/pprust/state/expr.rs` | 969 | `std::fmt::Write` → `core::fmt::Write`. | none |
| `src/pprust/state/fixup.rs` | 248 | None (no std refs). | none |
| `src/pprust/state/item.rs` | 842 | None (no std refs). | none |
| `src/pprust/tests.rs` | 54 | None (`#[cfg(test)]`-gated; skip per A4 precedent). | none |

### rustc_ast_lowering (no markers)

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 29 → 32 | `[workspace] members = []` header. | none |
| `src/lib.rs` | 2,687 | `#![no_std]` first + `#[macro_use] extern crate alloc;`. `use std::mem` → `use core::mem`; `use std::sync::Arc` → `use alloc::sync::Arc`. `impl std::fmt::Display for ImplTraitPosition` (line 384) + its `fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result` body → `core::fmt::*`. 13× `std::mem::take(...)` and 2× `std::mem::replace(...)` call sites (lines 645–659, 700–704) → `core::mem::*`. Line 1560 left alone (doc-comment example). | none |
| `src/asm.rs` | 498 | `std::collections::hash_map::Entry` → **`hashbrown::hash_map::Entry`** (recipe §1.3; see §4 for the Cargo.toml integration flag). `std::fmt::Write` → `core::fmt::Write`. | none |
| `src/block.rs` | 118 | None (no std refs). | none |
| `src/contract.rs` | 321 | `std::sync::Arc` → `alloc::sync::Arc`. | none |
| `src/delegation.rs` | 655 | `std::iter` → `core::iter`. | none |
| `src/errors.rs` | 443 | None (no std refs; all derive-macro-generated). | none |
| `src/expr.rs` | 2,213 | `std::mem` / `std::ops::ControlFlow` / `std::sync::Arc` → `core::mem` / `core::ops::ControlFlow` / `alloc::sync::Arc`. Three mid-file call sites: line 639 `std::slice::from_ref` → `core::slice::from_ref`; line 1276 `std::iter::once` → `core::iter::once`; line 1757 `std::slice::from_ref` → `core::slice::from_ref`. The 14 remaining `::std::*` mentions are all `///`/`//` desugaring documentation (describes the HIR each lang-item emits) — left alone per B1 precedent. | none |
| `src/format.rs` | 505 | `std::borrow::Cow` → `alloc::borrow::Cow`. Line 259 `std::slice::from_ref` → `core::slice::from_ref`. | none |
| `src/index.rs` | 365 | None (no std refs). | none |
| `src/item.rs` | 1,860 | Two inline `std::mem::take(...)` (lines 1799, 1802) → `core::mem::take(...)`. | none |
| `src/pat.rs` | 529 | `std::sync::Arc` → `alloc::sync::Arc`. | none |
| `src/path.rs` | 571 | `std::sync::Arc` → `alloc::sync::Arc`. Lines 159/179-186 are doc-comment narrative (showing example path strings) — left alone. | none |
| `src/stability.rs` | 135 | `std::fmt` → `core::fmt`. | none |
| `messages.ftl` | 134 | Not Rust (fluent i18n table). No changes. | none |

### rustc_ast_passes (no markers)

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 23 → 26 | `[workspace] members = []` header. | none |
| `src/lib.rs` | 21 → 31 | `#![no_std]` first + `#[macro_use] extern crate alloc;`. | none |
| `src/ast_validation.rs` | 1,843 | `std::mem` → `core::mem`, `std::ops::{Deref, DerefMut}` → `core::ops::*`, `std::str::FromStr` → `core::str::FromStr`. | none |
| `src/errors.rs` | 901 | None (no std refs; all derive-macro-generated). | none |
| `src/feature_gate.rs` | 616 | None (no std refs). | none |
| `messages.ftl` | 258 | Not Rust. No changes. | none |

## 2. Decisions made (architectural)

**None — pure recipe application.**

All three crates fit the B1 "LARGE-but-THIN" profile: bulk of the LOC
is AST/HIR enum match-arms, visitor walkers, and derive-macro error
structs. The std touch is the same handful of primitives
(`core::mem`, `core::fmt`, `core::iter`, `core::ops`, `alloc::sync::Arc`,
`alloc::borrow::Cow`) repeated across files. RECIPE §1.3's substitution
table covered every site.

No R4 B1 (FatalError) sites — these are pre-error-emission crates
(rustc_ast_lowering uses `DiagCtxtHandle` references and reports
errors via the standard rustc_errors handle, not catch_fatal_errors).
No R4 B2 (scoped_tls) sites. No R4 B5 (PathBuf) sites — the entire
cluster is pure AST/HIR data shuffling. No `// M27 §1.x`
incremental-comp gates. No R3 hash-consolidation sites. No
`cfg(target_os = "none")` host-vs-target split (no FS / IO / process
surface anywhere).

## 3. Deferred work, line-precise (the load-bearing section)

**Nothing deferred.** All 33 source files were either patched or
verified clean (12 of 33 had no `std::` refs at all in real code
positions: `block.rs`, `errors.rs` ×2, `feature_gate.rs`, `index.rs`,
`fixup.rs`, `item.rs` in `ast_pretty`, `tests.rs`, and the two
`messages.ftl`s).

A final triple-check grep across all three crates:

```
\bstd::    in non-comment, non-doc code positions  → 0 matches
::std::    in any position                          → only in /// and // comments (desugaring narrative)
\buse std  → 0 matches anywhere
```

## 4. New API gaps discovered

**None — semos_std surface was sufficient. One Cargo.toml integration flag for the parent.**

`rustc_ast_lowering/src/asm.rs` line 1 needs **`hashbrown` as a direct
dep** in `Cargo.toml`. Per RECIPE §1.3 the canonical substitution is
`std::collections::hash_map::Entry → hashbrown::hash_map::Entry`. The
asm.rs site uses this as `Entry::Occupied(o)` / `Entry::Vacant(v)`
match arms (lines 383, 428) against `FxHashMap`'s entry API. Per
`rustc_data_structures/src/fx.rs`'s `StdEntry` re-export, the on-target
hashbrown::hash_map::Entry resolves correctly because `FxHashMap` is
`HashMap<K, V, BuildHasherDefault<FxHasher>>` from hashbrown when
`target_os = "none"`. **Action at integration:** add
`hashbrown = { version = "*", default-features = false, features = ["inline-more"] }`
to `rustc_ast_lowering/Cargo.toml`, OR rewrite the import to
`use rustc_data_structures::fx::StdEntry as Entry;` (eliminates the
direct hashbrown dep — recommended if parent prefers consolidation).
B4's rustc_data_structures-sync handoff already wired the
target-conditional alias, so the consumer choice is cosmetic.

## 5. Phase-routing summary

**No markers added.** This is the cleanest non-A6 run since B1. The
grep for `// M27` across all three crates returns only the four
explanatory `// M27 Phase 3 C2:` comments inside the three lib.rs
files (the `#![no_std]` + `extern crate alloc;` rationale per RECIPE
§1.2 / B1 precedent). Integration is purely (a) the workspace dep
wiring and (b) the hashbrown Cargo flag in §4 above.

## 6. Surprises worth flagging upward

1. **B1 LARGE-but-THIN insight generalized cleanly to a *downstream*
   cluster.** B1 was the cycle-foundation (rustc_ast itself); we
   inherited its hygiene and expected to find more std touches further
   from the data model. We did not. Across 19.6k LOC of three
   downstream crates, only 24 raw `std::` substitution sites in real
   code — *lower* density than rustc_ast's 30 / 11.5k LOC (0.12 vs
   0.26 per 100 LOC). The "AST-tail" cluster is even thinner than
   the AST-root.

2. **`std::mem::take` is the single most common pattern in these
   crates.** rustc_ast_lowering/src/lib.rs alone has 13 sites; item.rs
   adds 2 more. All resolve to `core::mem::take` 1-to-1 (mem has been
   in core since 1.0). Mention only — no action needed.

3. **`rustc_ast_lowering/src/expr.rs` has 14 `::std::*` mentions in
   doc/line comments**, all describing the desugaring that
   async/await/range/try produce in HIR. None compile. The code that
   *implements* those desugarings uses `LangItem::*` enum variants
   (lang-item indirection), so the `::std::*` paths in the comments
   are pure narrative. Same situation as rustc_ast B1's `classify.rs`
   doc-string. Confirmed: B1's call to leave doc-string std references
   alone generalizes.

4. **`messages.ftl` is not Rust** — fluent i18n message tables. Per
   plan §1 decision 8 (drop i18n), these files are dead weight in the
   SemOS target build but harmless. The `fluent_messages!` macro
   invocation in each crate's lib.rs (`rustc_ast_lowering` and
   `rustc_ast_passes` have one each, missing in `rustc_ast_pretty`)
   becomes a no-op once `rustc_fluent_macro` is the no-op shim. No
   action here.

5. **Three files needed `Cargo.toml` workspace header but no source
   touches at all:** errors.rs in ast_lowering, errors.rs and
   feature_gate.rs in ast_passes. These are derive-macro-heavy
   diagnostic structs; the macro expansion is std-clean (no
   `::std::*` emissions detected in expanded shape based on the
   `Diagnostic` derive's known output). Same observation A6 made for
   proc-macros: macro-heavy code is std-free at source.

6. **No `tracing::*` substitution needed in these three crates' bodies.**
   `tracing::debug` / `tracing::instrument` / `tracing::trace` are
   used as attribute macros and function calls in `rustc_ast_lowering/
   src/lib.rs` and elsewhere. RECIPE §2 lists `tracing` as a stub-when-
   needed surface. Per B3's no-op `tracing` shim landed during Phase
   2b, these resolve unchanged at integration time.

## 7. Recipe additions

None — RECIPE carried the port end-to-end. One observation worth
folding into a future RECIPE revision (cosmetic):

- **B1's call to leave `///` and `//` `::std::*` mentions alone**
  (doc/narrative content) is now confirmed across four crates
  (rustc_ast, rustc_ast_pretty, rustc_ast_lowering, rustc_ast_passes).
  RECIPE §1.4 talks about macro *bodies* that emit `::std::*` tokens
  (which DO need rewriting); the inverse — `::std::*` in doc/narrative
  text — is implicitly handled by the recipe but worth one explicit
  line. Suggested addition to RECIPE §1.4: *"Doc comments (`///`) and
  line comments (`//`) that mention `::std::*` are pure narrative;
  leave them alone. Only rewrite paths that compile."*
