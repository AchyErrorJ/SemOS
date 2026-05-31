# F1 — rustc_codegen_ssa

**Date:** 2026-05-31
**Phase:** 4-codegen
**Assigned crates / files:** `compiler/rustc_codegen_ssa/` (54 src files, ~26.7k LOC) — entirely untouched at start
**Status:** IN PROGRESS

## 1. Per-file diff summary (running)

Tracking patches as I land them. Files marked DONE are committed to source tree.

| File | Status | Changes | Markers |
|------|--------|---------|---------|
| Cargo.toml | DONE | `[workspace] members = []` header. | — |
| src/lib.rs | DONE | D1 pattern + `#[macro_use] extern crate alloc;` + host-only `extern crate std;`. `BTreeSet` → `alloc::collections::BTreeSet`; `Arc` → `alloc::sync::Arc`; `io` + `Path/PathBuf` host/target cfg-split (semos_std::io / semos_std::path on target). No mod-line changes here — back submodules gated in back/mod.rs. | — |
| src/back/mod.rs | DONE | `use std::borrow::Cow` → `alloc::borrow::Cow`. **Whole-module cfg-gates at mod line:** `apple`, `command`, `link`, `linker` all `#[cfg(not(target_os = "none"))]`. `versioned_llvm_target` split: host body unchanged; SemOS body returns `Cow::Borrowed(&sess.target.llvm_target)` without apple. | // M27 §1.7 |
| src/base.rs | IN PROGRESS | Done: `std::cmp` → `core::cmp`, `BTreeSet` → `alloc::collections::BTreeSet`, `Arc` → `alloc::sync::Arc`, `Duration/Instant` cfg-split (semos_std::time on target). `use crate::back::link::are_upstream_rust_objects_already_included` cfg-split — SemOS-target stub returns `false`. `crate::back::linker::{exported_symbols,linked_symbols}` calls cfg-split — SemOS produces empty `Vec`. | // M27 §1.7 |

## 2. Decisions made (architectural)

### D1 lib.rs pattern (RECIPE §1.2)
Per task brief, used D1 (`cfg_attr(target_os = "none", no_std)` + `extern crate alloc;` + host-only `extern crate std;`). The crate has heavy host-only surface (back/link, back/linker, back/command, back/apple, back/apple/tests, back/link/raw_dylib) so D1 over legacy `#![no_std]` is mandatory per E2's added rule.

### M27 §1.7 — drop external-linker subsystem (back/{link,linker,command,apple})
Per `M27_RUSTC_PORT_PLAN.md` §1.7 + `R2_std_surface.md` §2.2 + task brief: cg_clif emits ET_EXEC bytes directly (proven in semos-cc D.2), so semos-rustc bypasses the SSA link step. Four whole modules gated at `mod` declaration line in `back/mod.rs`:
- `back/link.rs` (1500 LOC) — the big linker driver (`Command::new(linker).spawn`).
- `back/linker.rs` — gcc/msvc/wasm linker-flag generation + `exported_symbols`/`linked_symbols` builders.
- `back/command.rs` — `Command` wrapper.
- `back/apple.rs` — Apple-specific linker bits (xcrun, codesign).

Submodules (`back/link/raw_dylib.rs`, `back/linker/tests.rs`, `back/apple/tests.rs`) ride along since their parents are gated; nothing else changed for them.

**Cross-references requiring fixup in non-gated code:**
- `src/base.rs:42` — `use crate::back::link::are_upstream_rust_objects_already_included;` → cfg-gated import with a SemOS-target local stub returning `false`.
- `src/base.rs:892,895` — `crate::back::linker::{exported_symbols,linked_symbols}(tcx, c)` call sites → cfg-gated; SemOS arm uses `Vec::new()` (no external linker = no symbol table needed at link time).
- `src/traits/backend.rs:19` — `use crate::back::link::link_binary;` + default `fn link` impl that calls `link_binary(...)`. Will cfg-gate the import and the default impl body (TBD).
- `src/errors.rs:19,355-360,557-583` — references to `crate::back::command::Command` and `std::process::ExitStatus` in `LinkingFailed`/`ProcessingDymutilFailed`/`UnableToRunDsymutil`/`StrippingDebugInfoFailed`/`UnableToRun` Diagnostic structs. These structs are referenced only from `back/link.rs` (verified). Will cfg-gate the whole structs.
- `src/back/metadata.rs:26,222,429,434` — `use super::apple;` and uses in `macho_object_build_version_for_target`. Apple-only code path; cfg-gate.

## 3. Deferred work, line-precise

(populated at end / incrementally)

## 4. New API gaps discovered

(populated incrementally)

## 5. Phase-routing summary

(populated at end)

## 6. Surprises worth flagging upward

(populated at end)

## 7. Recipe additions

(populated at end)
