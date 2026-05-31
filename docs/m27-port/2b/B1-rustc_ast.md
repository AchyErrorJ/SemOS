# B1 — rustc_ast

**Date:** 2026-05-30
**Phase:** 2b
**Assigned crates / files:** `compiler/rustc_ast/` (the whole crate)
**Status:** COMPLETE
**Token cost (self-report):** ~30k tokens / ~28 tool uses / ~12 min wall
**Source LOC patched:** 11,553 LOC across 14 modified files

Read RECIPE + HANDOFF_TEMPLATE + A2 + A2-followup + plan §1 +
experiment-log tail in ~10 min. Then a single grep showed only 30
`std::` occurrences across 14 files — overwhelmingly trivial
`use std::{fmt,cmp,iter,…}` imports. The crate is a pure data-model
crate (AST node definitions + a visitor + macro-expansion glue); it
has no FS, no IO, no process, no thread, no env beyond a single
`std::env::var` site in `attr/version.rs`. This made rustc_ast an
unusually clean recipe-following port for a foundation-tier crate.

## 1. Per-file diff summary

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 22 | `[workspace] members = []` header | none |
| `src/lib.rs` | 45 → 57 | `#![no_std]` + `#[macro_use] extern crate alloc;` (recipe §1.2). `#![no_std]` is the FIRST inner attribute (precedes `#![feature(...)]` block per A2-followup's lesson). | none |
| `src/ast.rs` | 4,224 | `std::borrow::{Borrow, Cow}` → `alloc::borrow::*`; `std::{cmp, fmt}` → `core::*`. One mid-file `impl std::fmt::Debug for InlineAsmOptions` (lines 2748-2752) → `core::fmt::*`. | none |
| `src/ast_traits.rs` | 356 | `std::fmt` → `core::fmt`; `std::marker::PhantomData` → `core::marker::PhantomData`. | none |
| `src/node_id.rs` | 42 | `std::fmt` → `core::fmt`. | none |
| `src/token.rs` | 1,208 | `std::borrow::Cow` → `alloc::borrow::Cow`; `std::fmt` → `core::fmt`. | none |
| `src/tokenstream.rs` | 1,018 | `std::borrow::Cow` → `alloc::`; `std::sync::Arc` → `alloc::sync::Arc`; `std::hash::Hash`, `std::ops::Range`, `std::{cmp,fmt,iter,mem}` → `core::*`. | none |
| `src/mut_visit.rs` | 384 | `std::ops::DerefMut` → `core::ops::DerefMut`. **Removed** unused `use std::panic;` (verified by grep — no `panic::`, `catch_unwind`, `panic_any`, `resume_unwind`, `AssertUnwindSafe`, or `set_hook` anywhere in file). | none |
| `src/visit.rs` | 1,192 | Sole change: inline macro arg `std::borrow::Cow<'_, str>` → `alloc::borrow::Cow<'_, str>` at line 376 (inside `walk_attr_args!` arg list expansion). No top-of-file imports needed editing. | none |
| `src/attr/data_structures.rs` | 101 | `std::fmt` → `core::fmt`. | none |
| `src/attr/mod.rs` | 1,005 | `std::fmt::Debug` → `core::fmt::Debug`; `std::sync::atomic::{AtomicU32, Ordering}` → `core::sync::atomic::*` (atomics live in `core` since forever — direct substitution). | none |
| `src/attr/version.rs` | 43 | `std::fmt` → `core::fmt`. `std::sync::OnceLock` → **`semos_std::sync::OnceLock`** (futex-backed shim landed by A2 in commit `18d80dd`). `std::env::var("RUSTC_OVERRIDE_VERSION_STRING")` → **`semos_std::env::var(...)`** (`env::var` returns `Result<String, VarError>` same as std). | none |
| `src/expand/autodiff_attrs.rs` | 299 | `std::fmt::{self, Display, Formatter}` → `core::*`; `std::str::FromStr` → `core::str::FromStr`. One mid-file return type `-> std::fmt::Result` (line 195) → `-> fmt::Result` (rebound to the `core::fmt::Result` alias imported at the top). | none |
| `src/expand/typetree.rs` | 91 | `std::fmt` → `core::fmt`. | none |
| `src/util/literal.rs` | 333 | `std::{ascii, fmt, str}` → `core::{ascii, fmt, str}`. | none |

Files **not modified** (verified via grep for `\bstd::`):
- `src/entry.rs` (49 LOC) — `EntryPointType` enum, no std refs.
- `src/format.rs` (283 LOC) — uses `rustc_data_structures::fx::FxHashMap`, no direct std.
- `src/expand/mod.rs` (7 LOC) — re-export module.
- `src/expand/allocator.rs` (92 LOC) — `AllocatorKind` enum + `format!`-based name builder; `format!` resolves via the crate-root alloc prelude.
- `src/util/case.rs` (6 LOC) — empty enum.
- `src/util/classify.rs` (323 LOC) — pure logic. The `::std::ops::FnOnce(...)` reference at line 310 is inside a `///` doc comment (an illustrative example of a Rust path); harmless.
- `src/util/comments.rs` (133 LOC) — pure logic.
- `src/util/comments/tests.rs` (64 LOC) — `#[cfg(test)]` only.
- `src/util/parser.rs` (220 LOC) — pure logic.
- `src/util/unicode.rs` (35 LOC) — `const` table.

## 2. Decisions made (architectural)

**None — pure recipe application.**

The crate is the AST definition crate; it pre-dates the rest of the
compile pipeline and has no host-vs-target body split (RECIPE §1.5
not needed), no R4 B1 FatalError sites (RECIPE §1.6 — `mut_visit.rs`'s
`std::panic` import was unused; no `panic::catch_unwind` anywhere in
the crate), no R4 B2 scoped_tls sites, no R4 B5 PathBuf sites, no R3
hash-consolidation sites, no `// M27 §1.x` incremental-compilation
sites, and no macro-emitted `::std::*` tokens (RECIPE §1.4 — checked
the 5 macro-bearing files).

The closest thing to a non-mechanical choice was the
`semos_std::sync::OnceLock` substitution in `attr/version.rs`, which
the RECIPE table covers: `std::sync::OnceLock` is exactly what
semos_std now exposes (per RECIPE §2's surface table). Used directly
without a marker.

Same for `semos_std::env::var`. The shape matches std's: returns
`Result<String, VarError>` with `Err` on missing/invalid.

## 3. Deferred work, line-precise (the load-bearing section)

**Nothing deferred.** All 11,553 LOC across all 18 source files are
either patched or verified clean. Future Phase 5 integration may need
to add an explicit `semos_std = { path = "..." }` dep to
`Cargo.toml` so `attr/version.rs`'s `semos_std::sync::OnceLock` /
`semos_std::env::var` imports resolve — A2's rustc_span pattern was
the same (uses `semos_std::*` without a `[dependencies]` entry; the
parent's workspace-level dep injection wires it up at integration
time). Mention only.

## 4. New API gaps discovered

**None — semos_std surface was sufficient.**

The only two semos_std touches rustc_ast needs (`sync::OnceLock` and
`env::var`) are both in the R2 top-6 that the parent already landed
in commits `18d80dd` / `7ebc0f7`. The recipe's surface table
(RECIPE §2) is complete for rustc_ast.

Concretely: rustc_ast does NOT need `BorrowedBuf`, does NOT need any
PathBuf API surface, does NOT need `scoped_thread_local!`, does NOT
need `thread_local!`, does NOT need `Cow<Path>`, does NOT need
`io::Error::other(msg)` — the per-file scan confirmed every site.

## 5. Phase-routing summary

**No markers added.** This is the cleanest agent run since A6
(proc-macros, 0 source edits). The grep for `// M27` returns only
the explanatory comments at `lib.rs`, `mut_visit.rs`,
`attr/version.rs` — no `// M27 R3`, no `// M27 R4 Bx`, no
`// M27 §1.x`, no `// M27 TODO(Phase <n>):`. Integration is purely
the (cosmetic) `semos_std` dep wiring.

## 6. Surprises worth flagging upward

1. **rustc_ast is dramatically less std-coupled than rustc_span.**
   30 raw `std::` mentions across 11,553 LOC (~0.26 per 100 LOC) vs
   rustc_span's hundreds across 12,327 LOC. Almost all of rustc_ast's
   std references are `std::{fmt, cmp, iter, mem, ops, str, hash,
   borrow, marker}` — all trivially in `core::*` or `alloc::*`. A2
   warned that rustc_ast was "the heaviest of the 2b cycle-breakers";
   in mechanical terms it turned out to be the lightest of the
   foundation-tier crates. Worth retuning the Phase 2b token forecast.

2. **`std::panic` import in `mut_visit.rs:11` was dead code.** No
   `panic::*` references anywhere in the file (the `panic!` macro at
   line 357 is from the prelude). Removed cleanly. If a future
   upstream-rebase reintroduces real `panic::catch_unwind` use here,
   that'll surface as a compile error against `core::panic` and
   become a real R4 B1 site at that time.

3. **One inline `std::*` in a macro arg position** (`visit.rs:376`,
   inside a `walk_*` macro arg list giving `Cow<'_, str>` as a typetree
   path). RECIPE §1.4 calls these out for macro bodies that *emit*
   `::std::*`; this is the inverse — the arg list happens to *contain*
   a `std::` path. Same treatment (substitute to `alloc::borrow`).

4. **`std::sync::atomic` → `core::sync::atomic` is identical.** All
   atomics have been in `core` since 1.0. The `attr/mod.rs` site is
   a one-line rename. No SeqCst-vs-Relaxed subtleties to worry about
   here.

5. **`semos_std` is used without a Cargo.toml dep entry.** A2 set the
   precedent in rustc_span; rustc_ast follows. The parent's workspace
   integration (Phase 5) will wire `semos_std` in via a
   `[patch.crates-io]` or workspace-level path-dep injection.

6. **No host-vs-target split needed.** Unlike rustc_fs_util,
   rustc_log, etc., rustc_ast has no FS / IO / process surface, so
   the `cfg(target_os = "none")` pattern (RECIPE §1.5) is not
   needed.

## 7. Recipe additions

None — the RECIPE as written carried this port end-to-end.

One small confirmation worth folding into RECIPE §1.2 if the recipe
gets a revision pass: **`#![no_std]` must precede `#![feature(...)]`**
in crates that have a feature block, not just precede `extern crate`.
A2-followup §lib.rs already documented this for rustc_span; rustc_ast
confirms the pattern generalizes. RECIPE §1.2's note about "doc
comments → `#![…]` attributes → `extern crate` items" is correct as
written but doesn't pin `#![no_std]` as needing to be FIRST among the
inner attributes; in practice it does.
