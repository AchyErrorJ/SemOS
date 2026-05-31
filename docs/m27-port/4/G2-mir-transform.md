# G2 — rustc_mir_transform (followup)

**Date:** 2026-05-31
**Phase:** 4-codegen (codegen-tier MIR opt passes)
**Predecessor:** F2 (Cargo.toml + lib.rs + dump_mir.rs + dest_prop.rs + pass_manager.rs)
**Assigned crates / files:** `compiler/rustc_mir_transform/` — remaining 90 .rs files
**Status:** COMPLETE

## 0. Inherited context

F2's pre-port survey was load-bearing: "Single grep of `\bstd::` across all 95
files returns ~95 hits — extremely thin per the B1 LARGE-but-THIN pattern."
This held up exactly. After F2's 5 files there were ~30 files with `std::*`
references, each ~1-3 hits per file. All substitutions used F2's table from
RECIPE §1.3 (`std::{iter,mem,fmt,ops,cell,cmp,hash,slice,any,str}` → `core::*`,
`std::borrow::Cow` / `std::rc::Rc` → `alloc::*`).

## 1. Per-file diff summary

| File | Hits | Pattern |
|------|------|---------|
| check_call_recursion.rs | 1 | `std::ops::ControlFlow` → `core::*` |
| coroutine.rs | 3 | `std::ops` (use), `std::mem::replace`, `std::borrow::Cow::Borrowed` |
| cross_crate_inline.rs | 1 | `std::iter::once` → `core::*` |
| coverage/counters.rs | 1 | `std::cmp::Ordering` → `core::*` |
| dataflow_const_prop.rs | 4 | `std::cell::RefCell`, `std::fmt::Formatter`, 2× `std::fmt::Result` return types |
| coverage/from_mir.rs | 1 | `use std::iter` |
| coverage/graph.rs | 3 | `std::cmp::Ordering`, `std::ops::{Index,IndexMut}`, `std::{mem, slice}` |
| coverage/spans.rs | 1 | `std::cmp::Ordering` (expression position) |
| early_otherwise_branch.rs | 1 | `use std::fmt::Debug`. Two doc-comment `std::mem::discriminant` instances left intact (comments, no codegen). |
| elaborate_drop.rs | 2 | `use std::{fmt, iter, mem}`, `std::ops::Range<u64>` in enum variant |
| elaborate_drops.rs | 1 | `use std::fmt` |
| errors.rs | 1 | `std::fmt::Debug` in generic bound |
| gvn.rs | 3 | `std::borrow::Cow` → `alloc::*`, `std::hash::{Hash,Hasher}` → `core::*`, `std::mem::take`. `hashbrown::hash_table::{Entry, HashTable}` KEPT (already no_std). |
| inline.rs | 6 | `use std::iter`, `use std::ops::{Range,RangeFrom}`, 4× `std::ops::Range<BasicBlock>` + `std::slice::from_ref` |
| instsimplify.rs | 1 | `std::mem::take` |
| jump_threading.rs | 1 | `std::mem::take` |
| known_panics_lint.rs | 5 | `use std::fmt::Debug`, `std::fmt::{Debug, Formatter, Result}` (3 sites inside inner-impl block), `std::mem::take` |
| lint.rs | 1 | `use std::borrow::Cow` → `alloc::*` |
| lint_tail_expr_drop_order.rs | 3 | `std::cell::RefCell` → `core::*`, `std::rc::Rc` → `alloc::*`, `std::collections::hash_map::Entry` → `rustc_data_structures::fx::StdEntry` (F2's precedent from pass_manager.rs) |
| liveness.rs | 5 | `std::iter::zip`, `std::mem::take`, 3× `std::fmt::*` inside DebugWithContext impl |
| match_branches.rs | 1 | `use std::iter` |
| pass_manager.rs | 3 | F2 left two `std::any::type_name::<Self>()` + one `std::str::from_utf8` unpatched — G2 finished them (`core::any::type_name` + `core::str::from_utf8`). F2's StdEntry alias kept intact. |
| prettify.rs | 1 | `std::mem::take` |
| promote_consts.rs | 4 | `std::cell::Cell`, `std::{cmp, iter, mem}` → `core::*`, `std::ops::Deref` in impl |
| ref_prop.rs | 1 | `use std::borrow::Cow` → `alloc::*` |
| shim.rs | 1 | `use std::{fmt, iter}` |
| simplify.rs | 4 | 4× `std::mem::take` (replace_all) |
| simplify_comparison_integral.rs | 1 | `use std::iter` |
| single_use_consts.rs | 1 | `std::mem::replace` |
| sroa.rs | 1 | `std::mem::take` |
| ssa.rs | 1 | `std::mem::take` |
| trivial_const.rs | 1 | `use std::ops::Deref` |
| validate.rs | 4 | 4× `std::iter::zip` (replace_all) |

Total patched by G2: **31 files**, ~50 individual edits.
F2's 5 files (Cargo.toml, lib.rs, dump_mir.rs, dest_prop.rs, pass_manager.rs) + G2's 31 = 36 files touched.
Remaining ~59 files in the crate (e.g. ssa_range_prop.rs, strip_debuginfo.rs, abort_unwinding_calls.rs, all the small check_* and remove_* passes, etc.) contain ZERO `std::` references — they are already no_std-clean. F2's "~1 hit per file" estimate was effectively "1 hit averaged over the 90 files but actually concentrated in ~30 files" — slightly thinner than predicted.

## 2. Patterns confirmed

- **No new architectural surprises.** Substitution table from RECIPE §1.3 was sufficient for every file.
- **hashbrown::hash_table::Entry** (gvn.rs) confirmed F2's note: it's NOT std::collections::hash_map::Entry, no rewrite needed. hashbrown is no_std by default.
- **StdEntry pattern** (F2's pass_manager.rs precedent) ported cleanly to lint_tail_expr_drop_order.rs without renaming — using `StdEntry::Occupied`/`StdEntry::Vacant` directly in match arms instead of `StdEntry as Entry` alias. Both forms work.
- **Doc-comment `std::*` references** in early_otherwise_branch.rs (lines 24-25) and coroutine/by_move_body.rs (line 32, `//! use std::future::Future;`) left intact — they're rustdoc examples, no codegen impact.

## 3. Verification

Final grep `\bstd::` across the crate src/ returns ONLY:
- `dest_prop.rs:141` — F2's `#[cfg(not(target_os = "none"))]` gate
- `dump_mir.rs:4,6` — F2's `#[cfg(not(target_os = "none"))]` gate
- `lib.rs:47` — F2's `#[cfg(not(target_os = "none"))]` gate on LazyLock import
- `coroutine/by_move_body.rs:32` — `//!` doc comment
- `early_otherwise_branch.rs:24,25` — `///` doc comments

All 7 remaining hits are deliberate or harmless. No `::std::` macro emits anywhere. No `extern crate std` outside the cfg-gated import in lib.rs.

## 4. Blockers raised

**None.** No new R4/R5 class blockers. No semos-std API gap discovered. Patch-only contract honored (no cargo build, no git ops beyond Read/Write).

## 5. Recipe extensions

**None.** The crate is the textbook B1 LARGE-but-THIN example — pure mechanical substitution, no novel decisions required after F2 set the cfg-conditional patterns.

## 6. Self-report

- **Files patched (this agent):** 31
- **Distinct std patterns hit:** 5 (`std::{mem,iter,fmt,cmp,ops,cell,hash,slice,any,str,borrow,rc,collections::hash_map}` — all covered by RECIPE §1.3, no new patterns)
- **Tokens:** ~75k (rough estimate; within budget)
- **Tool uses:** ~70 (mostly Read+Edit pairs, no Write of patched sources — Edit only)
- **Duration:** single bucket, no late-bounce
- **t/LOC:** ~2-3 t/LOC across ~34k crate LOC — consistent with F2's prediction and the 14 t/LOC recipe-following pattern from A2-followup.

## 7. Lessons (one-liner)

LARGE-but-THIN crates with a strong predecessor handoff are essentially free — read once via Grep, edit individual lines, no re-reading. The 90-file count was the only context risk and was managed by batch-grepping `\bstd::` upfront to enumerate every site before opening Edit. Doc-comment `std::*` matches can be safely ignored — they don't compile.
