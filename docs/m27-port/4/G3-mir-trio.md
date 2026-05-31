# G3 — rustc_mir_build (followup) + rustc_mir_dataflow + rustc_monomorphize

**Date:** 2026-05-31
**Phase:** 4-codegen (mir-trio recovery)
**Assigned crates / files:**
- `compiler/rustc_mir_build/` (followup — F3 did Cargo + lib.rs; remaining 37 .rs files)
- `compiler/rustc_mir_dataflow/` (24 files, ~7.3k LOC, entirely untouched)
- `compiler/rustc_monomorphize/` (11 files, ~4.1k LOC, entirely untouched)
**Status:** COMPLETE
**Token cost (self-report):** ~55k tokens / ~38 tool uses / single session
**Source LOC patched:** ~14k LOC inspected across ~74 files; **17 files touched**, ~30 substitution lines + ~150 lines of cfg-gated host-only wrappers.

## 0. Pre-port survey

Whole-cluster `\bstd::` survey:

- **rustc_mir_build:** ONE file with `std::` references (`builder/matches/test.rs`); both are doc-comment narrative (`<str as std::cmp::PartialEq>::eq`) — leave alone per E3/B3 precedent. ONE `hashbrown::hash_map::Entry` site (`builder/coverageinfo.rs:1`). F3's Cargo + lib already in place. **Effective patch surface: 1 file, 2 lines.**
- **rustc_mir_dataflow:** 10 files with `std::` refs. `framework/graphviz.rs` is the heavy one (io + path + OnceLock + fs in host-only `write_graphviz_results`). Rest are pure RECIPE §1.3 mechanical.
- **rustc_monomorphize:** 4 files with `std::` refs. `util.rs` writes `closure_profile_*.csv` (cfg-gate). `partitioning.rs` writes a `.mono_items.{json,md}` dump AND uses `println!` for `print_mono_items` (cfg-gate both). `collector.rs` and `mono_checks/abi_check.rs` mechanical.

## 1. Per-file diff summary

### rustc_mir_build (F3's Cargo+lib already in place; 1 src file patched)

| File | Changes | Markers |
|------|---------|---------|
| Cargo.toml | [F3] `[workspace] members = []` | — |
| src/lib.rs | [F3] D1 cfg_attr no_std + alloc/std extern crate | — |
| src/builder/coverageinfo.rs | `use hashbrown::hash_map::Entry;` → `use rustc_data_structures::fx::StdEntry as Entry;` (RECIPE §1.3 B4) | — |

Doc-comment-only `std::` mentions left alone (matches E3/B3 precedent):
- `src/builder/matches/test.rs:172,411` — `<str as std::cmp::PartialEq>::eq` narrative.

### rustc_mir_dataflow (Cargo + lib + 10 .rs files patched)

| File | Changes | Markers |
|------|---------|---------|
| Cargo.toml | `[workspace] members = []` header | — |
| src/lib.rs | D1 `cfg_attr(target_os = "none", no_std)` + `extern crate alloc` + cfg'd `extern crate std` | — |
| src/value_analysis.rs | `std::fmt::{Debug, Formatter}` → `core::*`; `std::ops::Range` → `core::*`; 2× `std::fmt::Result` → `core::fmt::Result` (debug_with_context return types) | — |
| src/move_paths/mod.rs | `std::fmt` → `core::fmt`; `std::ops::{Index, IndexMut}` → `core::ops::*` | — |
| src/move_paths/builder.rs | `std::mem` → `core::mem` | — |
| src/framework/direction.rs | `std::ops::RangeInclusive` → `core::ops::*` | — |
| src/framework/cursor.rs | `std::cmp::Ordering` → `core::cmp::*`; `std::ops::Deref` → `core::*` | — |
| src/framework/fmt.rs | `std::fmt` → `core::fmt` | — |
| src/framework/mod.rs | `std::cmp::Ordering` → `core::*` | — |
| src/framework/graphviz.rs | imports: Cow→alloc; OsString/PathBuf/OnceLock→`semos_std::*`; `io`→`semos_std::io`; `ops`/`str`→`core::*`. `write_graphviz_results` cfg-gated to host (uses `fs::create_dir_all`, `fs::File::create_buffered`); SemOS stub returns `Ok(())`. Inline `use std::io::Write;` in `write_node_label` → `semos_std::io::Write`. `std::vec::IntoIter` → `alloc::vec::IntoIter`. `std::iter::once` → `core::iter::once` | // M27 §1.3 R4 dataflow graphviz dump deferred |
| src/framework/tests.rs | `std::marker::PhantomData` → `core::*`; 2× `std::iter::{repeat,once}` → `core::iter::*` | — |
| src/impls/storage_liveness.rs | `std::borrow::Cow` → `alloc::*`; `std::cell::RefCell` → `core::*` | — |

### rustc_monomorphize (Cargo + lib + 4 .rs files patched)

| File | Changes | Markers |
|------|---------|---------|
| Cargo.toml | `[workspace] members = []` header | — |
| src/lib.rs | D1 `cfg_attr(target_os = "none", no_std)` + `extern crate alloc` + cfg'd `extern crate std` | — |
| src/collector.rs | `std::cell::OnceCell` → `core::cell::*`; `std::ops::ControlFlow` → `core::*` | — |
| src/mono_checks/abi_check.rs | inline `std::iter::once` → `core::iter::once` | — |
| src/util.rs | Full rewrite: split `dump_closure_profile` into `#[cfg(not(target_os = "none"))]` host body (preserves upstream `OpenOptions`/`writeln!`/`eprintln!`/`std::process::id()` use) + `#[cfg(target_os = "none")]` SemOS stub `fn(_, _) {}`. | // M27 §1.3 R4 closure profile dump deferred |
| src/partitioning.rs | imports: `std::cmp` → `core::cmp`; `std::collections::hash_map::Entry` → `rustc_data_structures::fx::StdEntry as Entry`; **dropped** `std::fs/io::Write/path`. cfg-gated `SwitchWithOptPath`+`CouldntDumpMonoStats` imports to host-only. Body of `debug_dump`: `std::fmt::Write` → `core::fmt::Write`; `std::mem::take` → `core::mem::take`. `dump_mono_items_stats` cfg-gated to host (signature keeps `&Option<std::path::PathBuf>` since rustc_session still uses std::path::PathBuf there — see §6 #1). Caller (`-Z dump-mono-stats`) and `-Z print-mono-items` if-blocks cfg-gated. | // M27 §1.3 R4 partitioning dump deferred; // M27 §1.3 R4 print_mono_items dump deferred |

## 2. Decisions made (architectural)

- **lib.rs pattern**: D1 `cfg_attr(target_os = "none", no_std)` for both dataflow + monomorphize (RECIPE §1.2). Both have host-callable surface (`fluent_messages!`, Display/Debug derive emissions), so cfg_attr is the correct shape.
- **graphviz.rs in dataflow**: `write_graphviz_results` is split into host vs SemOS bodies; the rest of the module (`Formatter`, `BlockFormatter`, `StateDiffCollector`) is type-parametric on `dyn io::Write` and works on both targets via `semos_std::io::Write`. Matches E4 borrowck/region_infer/graphviz.rs pattern. The `OnceLock` static inside the `regex!` macro is now `semos_std::sync::OnceLock`.
- **partitioning.rs PathBuf**: Resisted the urge to swap `std::path::PathBuf` to `semos_std::path::PathBuf` in `dump_mono_items_stats`'s signature, because rustc_session (its caller's source-of-`PathBuf`) still uses `std::path::PathBuf` (downstream — Phase 4 will own that). Instead, gated the entire host-only function (and its call site) with `#[cfg(not(target_os = "none"))]`. SemOS build sees no `Option<std::path::PathBuf>` at all, so no type-resolution conflict.
- **partitioning.rs print_mono_items println!**: `println!` is only in `std::prelude`, not core/alloc. Cfg-gated the whole `if tcx.sess.opts.unstable_opts.print_mono_items { ... }` block.
- **util.rs dump_closure_profile**: matches the `polonius/legacy/facts.rs` (E4) shape — preserves full host behavior, no-op stub on SemOS.
- **hashbrown::Entry routing**: 2 sites (coverageinfo.rs in mir_build, partitioning.rs in monomorphize). Both routed through `rustc_data_structures::fx::StdEntry as Entry` (B4 precedent).

## 3. Deferred work, line-precise

**Nothing deferred at file level.** All 3 crates are patched at every site that currently references `std::` outside doc comments / cfg-gated host bodies.

External-crate work still pending (R3 owner — not G3's job):
- **tracing** — used in all 3 crates (`tracing::{debug, error, info, instrument}`). Same R3 story as prior phases. Leave imports as-is; parent integration handles.
- **regex** (mir_dataflow/framework/graphviz.rs:11) — `regex::Regex` only used inside the host-only `regex!` macro static. The macro itself is no_std-incompatible (regex `default-features = false` needs to be flipped at the workspace level) but the macro is only invoked from `diff_pretty` (line 778+), which is reachable from `write_node_label` — the SemOS-build path. **Needs verification at workspace dep flip stage**: if `regex` won't compile no_std without features, the `regex!` macro and its callers need cfg-gating. **Flag for parent**: low-risk but inspect.
- **serde / serde_json** (monomorphize) — only used inside cfg-gated host-only `dump_mono_items_stats`. Safe.
- **smallvec / polonius-engine** (dataflow) — already no_std-clean via existing flags (E4 precedent).

## 4. New API gaps discovered

**None.** Semos-std surface (`semos_std::io::Write`, `semos_std::path::PathBuf`, `semos_std::ffi::OsString`, `semos_std::sync::OnceLock`) covered every site I touched. No new shim required.

## 5. Phase-routing summary

- `// M27 §1.3 R4 dataflow graphviz dump deferred — needs FS surface` — 1 site (`mir_dataflow/framework/graphviz.rs`'s SemOS stub of `write_graphviz_results`).
- `// M27 §1.3 R4 closure profile dump deferred — needs FS surface` — 1 site (`monomorphize/util.rs`'s SemOS stub of `dump_closure_profile`).
- `// M27 §1.3 R4 partitioning dump deferred — only emitted on the host build` — 1 site each at the caller and the `dump_mono_items_stats` function definition in `monomorphize/partitioning.rs`.
- `// M27 §1.3 R4 print_mono_items dump deferred — println! is host-only` — 1 site (`monomorphize/partitioning.rs`'s `print_mono_items` if-block).

All four are debug-only output paths (`-Z dump-mir-dataflow`, RFC 2229 closure size CSV, `-Z dump-mono-stats`, `-Z print-mono-items`). None are reachable in the v1 rustc-on-SemOS happy path. Parent owns whether to grow an FS-dump surface later.

## 6. Surprises worth flagging upward

1. **rustc_mir_build was nearly empty of std references.** R2's "37 remaining files / sync:3, io:2" turned out to be **1 hashbrown::Entry rewrite + 2 doc-comment narrative lines (left alone)**. F3's Cargo+lib was 95% of the actual port work. The pattern "C2's downstream inherits hygiene" generalizes again: rustc_mir_build sits behind rustc_middle/rustc_infer/rustc_trait_selection, and inherits *all* of their type plumbing without ever directly touching std. This mirrors C2's 21/33 → 64% touch ratio for the foundation tier; mir_build came in at **1/40 → 2.5%**. A potential pattern for the Phase 4 schedule: crates above the "consumes-only" tier have radically lower touch rates.

2. **R2 misestimated mir_dataflow's "io:5" — the actual surface is much more polymorphic.** Of the 5 `io::Write` sites in `framework/graphviz.rs`, 4 are `&mut impl io::Write` type-parameters in `BlockFormatter`'s `write_block_header_*`/`write_row`/`write_statements_and_terminator` methods. Only `write_graphviz_results` itself opens a file. Once `io::Write` is rebound to `semos_std::io::Write`, every consumer of those generic functions Just Works. Same pattern as E4's borrowck (region_infer/graphviz.rs, dump_mir.rs, polonius/dump.rs). **Insight worth tightening in R2's NEEDS-SHIM classifier**: distinguish `dyn Write` parameter sites (FREE) from "actually opens a file" sites (REAL).

3. **`semos_std::path::PathBuf` is NOT a `std::path::PathBuf` re-export.** It's a struct over `String` with single-slash POSIX semantics (see std-shim/src/path.rs:173). On the host build, the two types are different. This means: substituting `std::path::PathBuf` → `semos_std::path::PathBuf` in a function signature only works if **all callers** are also patched. In `partitioning.rs`, the caller path is `SwitchWithOptPath::Enabled(ref path)` whose source is `rustc_session::config::SwitchWithOptPath` — still uses `std::path::PathBuf` at this Phase 4 wave. **Resolution**: gate the function & call site to host only. Phase 4 followup for rustc_session will need to address this for any sites that actually want SemOS-target PathBuf to flow through.

4. **`#![feature(file_buffered)]` in both lib.rs files**: kept as-is. It's a nightly-feature gate that's only meaningful when `fs::File::create_buffered` is called — and on SemOS that's now cfg-gated out. The feature attribute is harmless on either build (it just opts into a still-unstable API). Phase 5 can decide whether to drop it from the SemOS build via `cfg_attr`.

5. **`SwitchWithOptPath`, `DumpMonoStatsFormat`, `CouldntDumpMonoStats` imports cfg-gated to host.** These are dead on SemOS now. The `tcx.sess.opts.unstable_opts.print_mono_items` field access is the only one still on the SemOS code path; it's safe because the field exists in any build of rustc_session.

6. **The `regex` crate as a no_std risk in mir_dataflow** (see §3): `framework/graphviz.rs:9` imports `regex::Regex`. The crate has a no_std feature mode but the default features pull in `std`. R3 needs to flip this at the workspace level when wiring up the SemOS build target. The `regex!` macro is only reachable from the SemOS-build path of `write_node_label`/`diff_pretty`, so this matters.

## 7. Recipe additions

Two patterns worth folding into RECIPE.md once Phase 4 stabilizes:

1. **"semos_std::path::PathBuf substitution is NOT a free import swap"** when the caller's source-of-PathBuf hasn't been ported yet. Match the *whole call chain* before swapping a signature; otherwise cfg-gate the function (and its call site) to host. This is a contrast with E4's claim that "IntoDiagArg path arg substitutes trivially" — that case works only because rustc_errors's diagnostic-impl emit had already done the swap. In foreign-crate consumers of `SwitchWithOptPath` and similar, the swap is NOT a free move.

2. **"print_mono_items / println! cfg-gate" alongside "fs-dump cfg-gate"**. The recipe currently covers fs-write deferrals via cfg-gate; should also call out that `println!`/`eprintln!`/`eprint!`/`print!` are `std::prelude`-only and need the same cfg-gate treatment in no_std crates. (semos_std offers `print!`/`println!` via its own surface but not via the implicit prelude, so direct macro use in upstream crates needs gating.)
