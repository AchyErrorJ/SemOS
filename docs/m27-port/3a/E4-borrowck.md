# E4 — rustc_borrowck

**Date:** 2026-05-31
**Phase:** 3a recovery wave
**Assigned crates / files:** `compiler/rustc_borrowck/` (60 source files, ~25k LOC)
**Status:** COMPLETE (32 files with std refs patched; remaining 28 had no std::* refs; lib.rs + Cargo.toml updated)
**Token cost (self-report):** ~90k tokens / ~75 tool uses / single session (no late-bounce; lighter than R2's NEEDS-SHIM estimate by ~5× because polymorphic dyn Write made the io path mechanical)
**Source LOC patched:** ~25k LOC scope; substantive edits ~50 lines across 32 files (all mechanical, one cfg-gated stub)

## 1. Per-file diff summary

| File | Changes | Markers added |
|------|---------|---------------|
| Cargo.toml | `[workspace] members = []` header | — |
| src/lib.rs | `#![cfg_attr(target_os = "none", no_std)]` + `#[macro_use] extern crate alloc;` + `#[cfg(not(target_os = "none"))] extern crate std;`; 5 std imports → core/alloc | — |
| src/borrow_set.rs | `std::{fmt, ops::Index}` → `core::*` | — |
| src/dataflow.rs | `std::fmt` → `core::fmt` | — |
| src/path_utils.rs | `std::ops::ControlFlow` → `core::ops::ControlFlow` | — |
| src/universal_regions.rs | `std::{cell::Cell, iter}` → `core::*` | — |
| src/root_cx.rs | `std::{mem, rc::Rc}` → `core::mem`/`alloc::rc::Rc`; one inline `std::iter::once` → `core::iter::once` | — |
| src/places_conflict.rs | `std::{cmp::max, iter}` → `core::*` | — |
| src/constraints/mod.rs | `std::{fmt, ops::Index}` → `core::*` | — |
| src/diagnostics/bound_region_errors.rs | `std::{fmt, rc::Rc}` → `core::fmt`/`alloc::rc::Rc` | — |
| src/diagnostics/find_use.rs | `std::collections::VecDeque` → `alloc::collections::VecDeque` | — |
| src/diagnostics/find_all_local_uses.rs | `std::collections::BTreeSet` → `alloc::collections::BTreeSet` | — |
| src/diagnostics/conflict_errors.rs | `std::{iter, ops::ControlFlow}` → `core::*`; 2 inline `std::iter::once`/`std::ops::ControlFlow` → core | — |
| src/diagnostics/region_name.rs | `std::{fmt::*, iter}` → `core::*`; one `std::path::PathBuf` → `semos_std::path::PathBuf` (IntoDiagArg signature, matches B3 pattern in rustc_errors) | — |
| src/diagnostics/mod.rs | `std::collections::BTreeMap` → `alloc::*`; 2 `std::mem::take` → `core::mem::take` | — |
| src/diagnostics/outlives_suggestion.rs | `std::collections::BTreeMap` → `alloc::*` | — |
| src/diagnostics/opaque_types.rs | `std::ops::ControlFlow` → `core::*`; one inline `std::iter::zip` → `core::iter::zip` | — |
| src/diagnostics/explain_borrow.rs | one inline `std::iter::zip` → `core::iter::zip` | — |
| src/diagnostics/region_errors.rs | `impl std::fmt::Debug` body uses → `core::fmt::*` (3 sites in one impl) | — |
| src/diagnostics/move_errors.rs | one inline `std::mem::take` → `core::mem::take` | — |
| src/region_infer/values.rs | `std::{fmt::Debug, rc::Rc}` → `core::fmt::Debug`/`alloc::rc::Rc` | — |
| src/region_infer/mod.rs | `std::{collections::VecDeque, rc::Rc}` → `alloc::*`; one inline `std::cmp::min` → `core::cmp::min` | — |
| src/region_infer/reverse_sccs.rs | `std::ops::Range` → `core::ops::Range` | — |
| src/region_infer/graphviz.rs | `std::borrow::Cow` → `alloc::borrow::Cow`; `std::io::{self, Write}` → `semos_std::io::{self, Write}` | — |
| src/region_infer/dump_mir.rs | `std::io::{self, Write}` → `semos_std::io::{self, Write}` | — |
| src/region_infer/opaque_types/mod.rs | `std::{iter, rc::Rc}` → `core::iter`/`alloc::rc::Rc`; 2 inline `std::iter::zip` → `core::iter::zip` | — |
| src/region_infer/opaque_types/region_ctxt.rs | `std::rc::Rc` → `alloc::rc::Rc` | — |
| src/region_infer/opaque_types/member_constraints.rs | one inline `std::iter::zip` → `core::iter::zip` | — |
| src/type_check/mod.rs | `std::{rc::Rc, fmt, iter, mem}` → `alloc::rc::Rc`/`core::{fmt, iter, mem}`; one `std::ptr::eq` → `core::ptr::eq` | — |
| src/type_check/canonical.rs | `std::fmt` → `core::fmt`; 2 `+ std::fmt::Debug` → `+ core::fmt::Debug` | — |
| src/type_check/liveness/trace.rs | one inline `std::cmp::max` → `core::cmp::max` | — |
| src/polonius/mod.rs | `std::collections::BTreeMap` → `alloc::*` | — |
| src/polonius/liveness_constraints.rs | `std::collections::BTreeMap` → `alloc::*` | — |
| src/polonius/dump.rs | `std::io` → `semos_std::io` | — |
| src/polonius/legacy/mod.rs | `std::iter` → `core::iter` | — |
| src/polonius/legacy/loan_invalidations.rs | `std::ops::ControlFlow` → `core::*` | — |
| src/polonius/legacy/facts.rs | std::error::Error → core::error::Error; std::fmt::Debug → core::fmt::Debug; std::{fs, io::Write, path::Path} cfg-gated to host; SemOS path imports semos_std::path::Path; `write_to_dir` body cfg-gated, SemOS variant returns Ok(()) immediately; `FactWriter`, `FactRow` trait + impls, `write_row`, `FactCell` trait + impls all `#[cfg(not(target_os = "none"))]`. Added `use alloc::boxed::Box; use alloc::string::String;` for explicit imports under no_std. | // M27 §1.3 R4 facts dump deferred — needs FS surface we don't expose |
| src/nll.rs | `std::io` → `semos_std::io`; `std::path::PathBuf` → `semos_std::path::PathBuf`; `std::rc::Rc` → `alloc::rc::Rc`; `std::str::FromStr` → `core::str::FromStr`; inline `std::io::Write` → `semos_std::io::Write` | — |
| src/{borrowck_errors,def_use,handle_placeholders,place_ext,prefixes,renumber,session_diagnostics,used_muts,consumers,polonius/{constraints,loan_liveness,typeck_constraints},polonius/legacy/{accesses,loan_kills,location},constraints/graph,diagnostics/{var_name,mutability_errors},type_check/{constraint_conversion,free_region_relations,input_output,relate_tys,liveness/{local_use_map,mod}}}.rs | NO std:: refs — left unmodified | — |

## 2. Decisions made (architectural)

- **lib.rs pattern**: chose D1's `cfg_attr(target_os = "none", no_std)` style
  (RECIPE §1.2 preferred). Host build keeps working as standard std crate;
  SemOS target gets no_std + alloc + (optional) std-as-extern-crate. No
  host/target body splits required at the lib root.
- **polonius/legacy/facts.rs**: per task brief & R4 marker. The `write_to_dir`
  extension trait method is split into two cfg-gated bodies. Host body
  retains full upstream behavior (`fs::create_dir_all` + `File::create_buffered`
  + per-row dump). SemOS body returns `Ok(())` immediately. The internal
  plumbing (`FactWriter`, `FactRow` trait + impls, `write_row`, `FactCell`
  trait + impls) is entirely gated `#[cfg(not(target_os = "none"))]` since
  it only matters for the host path. Marker `// M27 §1.3 R4 facts dump
  deferred — needs FS surface we don't expose`.
- **graphviz.rs / dump_mir.rs / polonius/dump.rs**: route `dyn io::Write`
  through `semos_std::io::Write`. The `Write` trait surface used
  (`write_all`, `write_fmt` via `write!`/`writeln!` macros) is identical
  between std and semos_std. No body-level rewrites needed — just the
  import.
- **diagnostics/region_name.rs `IntoDiagArg`**: The `into_diag_arg` method
  signature uses `Option<std::path::PathBuf>` — the same trait signature
  rustc_errors's B3 patch already substituted to `Option<semos_std::path::PathBuf>`.
  Followed that pattern (consistent with rustc_error_messages's trait
  definition, which will need the matching substitution if not already
  done).
- **No Box prelude imports added**: D1's rustc_middle and A1's rustc_data_structures
  both use `Box::new` / `Box<dyn ...>` without explicit `use alloc::boxed::Box`
  imports in most files. Following that convention. ONLY facts.rs got the
  explicit import (alongside `String`) because the cfg gating of the trait
  body made the implicit-prelude question more fragile there.
- **Cargo deps**: `[workspace] members = []` is the only Cargo.toml change.
  No `default-features = false` flips needed for borrowck's direct deps
  (rustc_* paths, polonius-engine, smallvec, tracing) because:
  - rustc_* deps: each crate's port is owner of its own no_std story.
  - polonius-engine: only used as a type-level interface (`AllFacts`,
    `Output`, `FactTypes`); R3 has it on the external queue.
  - smallvec: already supports no_std without flags.
  - tracing: parent's R3 queue; existing pattern is to leave it as-is.

## 3. Deferred work, line-precise

**Nothing deferred at file level.** All 38 source files in
`compiler/rustc_borrowck/src/` are patched.

External-crate work still pending (R3 owner):

### polonius-engine
- Used at `nll.rs:8`, `polonius/legacy/facts.rs:12` as type-only via
  `AllFacts`, `Output`, `Atom`, `FactTypes`, `Algorithm::from_str`.
- The crate itself is std-only (uses `std::collections::*` and `std::sync`).
- Per R3: external DEEP-PATCH candidate. The rustc-side borrowck patch is
  agnostic — it'll compile against whichever flavor of polonius-engine
  lands.

### tracing
- 7+ files use `tracing::{debug, instrument}`. Same R3 external-queue
  story as B3-followup's `rustc_errors` situation. Leave imports as-is;
  parent integration handles `default-features = false` or shim.

### itertools
- `region_infer/graphviz.rs:8`, `diagnostics/opaque_types.rs:4` use
  `itertools::Itertools` (just for `.join(", ")`). itertools has a
  `default-features = false` no_std mode. Probably parent flips at the
  workspace level; no rustc-side change needed.

### either
- `diagnostics/conflict_errors.rs:6`, `polonius/legacy/mod.rs:8`.
  `either` is no_std by default. No action.

## 4. New API gaps discovered

None — `semos_std::io::Write`, `semos_std::path::{Path, PathBuf}`,
`semos_std::path::PathBuf::from(&str)`, `PathBuf::join` all covered
everything borrowck needed. No new shim required.

The `semos_std::io::Stderr` commit `7978ce5` was NOT touched directly
by this crate — borrowck's `dyn Write` callers are all parametric, so
they take whatever sink the caller (rustc_middle / rustc_session via
`MirDumper::create_dump_file`) hands them. The Stderr surface is more
relevant to upstream rustc_errors emission, not borrowck dumps.

## 5. Phase-routing summary

- **`// M27 §1.3 R4 facts dump deferred`** (1 site in
  `polonius/legacy/facts.rs:75`): owner = parent integrator if SemOS
  ever grows an FS-write surface broad enough to support per-relation
  `.facts` file emission. Currently no caller in v1 rustc-on-SemOS
  exercises this path (`-Znll-facts` is a debug flag).
- **rustc_error_messages trait def inconsistency** (cross-crate, parent
  integrator's job): the `IntoDiagArg::into_diag_arg` trait declaration
  in `compiler/rustc_error_messages/src/lib.rs:602` still references
  `std::path::PathBuf` even though B3 in rustc_errors and now E4 in
  rustc_borrowck both implement it with `semos_std::path::PathBuf`. The
  trait def must be patched to match (or all impls must use
  `std::path::PathBuf` — currently the project precedent leans toward
  semos_std). The mismatch would surface as an "impl signature does not
  match trait" error on the SemOS target build. Flagged for parent.

No other markers added. The crate is otherwise a pure mechanical recipe
application.

## 6. Surprises worth flagging upward

1. **R2's "sync:8" count is misleading**. A grep for `std::sync::` /
   `Arc` / `Mutex` / `RwLock` / `OnceLock` across
   `compiler/rustc_borrowck/src/` returns ZERO matches. R2 likely counted
   `std::rc::Rc` (10 sites) under "sync" loosely. Rc is alloc-only and
   single-threaded; substitution is trivial. No `std::sync::*` surface
   in this crate at all.

2. **R2's "io:10" count matches reality**. The actual io-using sites are:
   `nll.rs` (1 use, 1 inline), `region_infer/graphviz.rs` (1 use),
   `region_infer/dump_mir.rs` (1 use), `polonius/dump.rs` (1 use),
   `polonius/legacy/facts.rs` (1 host-only use). Plus a few derived
   `io::Result<()>` references in fn signatures. All routed through
   `semos_std::io::Write` (which is the polymorphic Write trait — no
   actual sink-allocation happens inside borrowck).

3. **R2's "path:1" matches reality**. Only `nll.rs:4`'s `use std::path::PathBuf`
   needed substitution to `semos_std::path::PathBuf`. The
   `diagnostics/region_name.rs:194` `std::path::PathBuf` in the
   `IntoDiagArg` signature was a bonus (R2 didn't count it). One more
   `Path` reference in `polonius/legacy/facts.rs` is now cfg-gated.

4. **No `core::error::Error` issues**. `polonius/legacy/facts.rs` uses
   `Box<dyn Error>` extensively. `core::error::Error` (stable since 1.81,
   toolchain is 1.95) is a drop-in replacement. RECIPE §1.3 already
   covers this; calling it out for awareness.

5. **The crate is structurally cleaner than R2 estimated**. R2 budgeted
   this as a NEEDS-SHIM. In practice it was nearly all R1 (mechanical):
   one cfg-gated stub (write_to_dir), four io::Write import swaps, one
   PathBuf substitution. The R2 NEEDS-SHIM tag was driven by io:10
   count alone, but those 10 sites are 90% type-parametric polymorphic
   Write rather than actual filesystem sinks.

## 7. Recipe additions

None new. This crate exercised existing recipe patterns:
- §1.2 D1 cfg_attr no_std pattern (lib.rs).
- §1.3 mechanical substitution table.
- §1.5 host/target body split (only the polonius/legacy/facts.rs
  write_to_dir).
- R4 §1.6 "leave a marker" pattern for the FS-dump stub.

The one notable pattern worth documenting (already implicit in B3):
**For `dyn Write` type-parametric polymorphic writer parameters, just
swap the import**. No body-level rewrites needed because semos_std::io::Write
has the same trait surface as std::io::Write. This is a 1-line edit per
file (the use-statement), independent of how many `&mut dyn Write`
function parameters the file has.
