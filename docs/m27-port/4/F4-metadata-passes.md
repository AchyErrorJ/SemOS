# F4 — rustc_metadata (ARCHITECTURAL) + rustc_passes (MECHANICAL)

**Date:** 2026-05-31
**Phase:** 4-codegen
**Assigned crates / files:**
- `compiler/rustc_metadata/` (16 files, ~11.4k LOC) — ARCHITECTURAL (libloading drop per §1.2)
- `compiler/rustc_passes/` (19 files, ~9.4k LOC) — MECHANICAL
**Status:** IN PROGRESS (notes written incrementally as work proceeds)
**Token cost (self-report):** TBD on completion
**Source LOC patched:** TBD on completion

## 0. Recon and plan (before patching)

### rustc_metadata std-surface inventory

Files with `std::` references (10 of 16):
- `lib.rs` — needs D1 pattern (host-only modules likely needed for libloading)
- `creader.rs` — libloading plugin loader; PRIMARY architectural surgery
- `fs.rs` — 5× `std::fs` ops + `tempfile` indirection (rmeta wrapper file)
- `locator.rs` — crate search; `std::env::current_exe`, `std::process::Command`,
  `std::fs::File::open`, `tempfile`, dylib metadata loader bridge
- `errors.rs` — `std::io::Error` + `std::path::{Path, PathBuf}` (diag types)
- `native_libs.rs` — `std::ops::ControlFlow` + `std::path::{Path, PathBuf}`
- `rmeta/decoder.rs` — `std::sync::{Arc, OnceLock}`, `std::path::*`,
  `std::ops::Deref`, `std::iter::TrustedLen`, misc
- `rmeta/encoder.rs` — `std::{fs, io, path, sync}`, `BufReader`, `Seek` (rmeta file output)
- `rmeta/decoder/cstore_impl.rs` — `std::any::Any`, `std::mem`, `std::sync::Arc`,
  inline `std::collections::{hash_map::Entry, vec_deque::VecDeque}`
- `rmeta/mod.rs` — `std::marker::PhantomData`, `std::num::NonZero`
- `rmeta/parameterized.rs` — `std::hash::Hash` + macro-emitted `std::string::String`

Files NO std::: `dependency_format.rs`, `eii.rs`, `foreign_modules.rs`,
`rmeta/table.rs`, `rmeta/def_path_hash_map.rs`.

### Architectural decisions for rustc_metadata

Per task brief §1.2 (statically link cg_clif → no dlopen of codegen backends):

1. **`creader.rs` libloading sites** (lines 1367–1474): the `attempt_load_dylib`,
   `load_dylib`, `load_symbol_from_dylib`, `DylibError` machinery — cfg-gate the
   whole libloading-using region with `#[cfg(not(target_os = "none"))]`. On
   SemOS, `load_symbol_from_dylib` is unreachable because:
   - cg_clif is statically linked (no codegen-backend dlopen).
   - Proc-macro crates aren't loaded on SemOS (§1.5 drop).
   The SemOS arm provides a stub that returns `DylibError::DlOpen(...)` (it's
   called only from `dlsym_proc_macros` which itself is only reached for
   proc-macro crates which we never load).
2. **`fs.rs` rmeta wrapper file writes**: route through `semos_std::fs` +
   `semos_std::path`. `tempfile` indirection via `rustc_fs_util::TempDirBuilder`
   stays — it's just a type, the actual semantics inside `rustc_fs_util`
   are the previous agent's concern. Substitute imports only.
3. **`locator.rs` SDylib branch**: spawning `rustc -Zbuild-sdylib-interface` via
   `Command::new` is dead on SemOS (only used for sdylib interfaces, which don't
   exist when single codegen + no proc-macros). The entire `CrateFlavor::SDylib`
   arm in `get_metadata_section` gets cfg-gated; SemOS variant returns
   `MetadataError::LoadFailure("sdylib not supported on semos")`.
4. **`Mmap::map(file)`** in `get_rmeta_metadata_section`: `Mmap` comes from
   `rustc_data_structures::memmap` — that crate's own porting decision. F4
   substitutes the file-open import to `semos_std::fs::File`. (If
   `rustc_data_structures::memmap::Mmap::map` rejects the semos_std File type,
   that's a Phase 5 integration concern.)
5. **Cargo.toml**: cfg-gate the `libloading` dep behind
   `[target.'cfg(not(target_os = "none"))'.dependencies]`. The `tempfile` dep
   stays for host build; replaced with no-op on SemOS via cfg.

### rustc_passes std-surface inventory

Files with `std::` references (8 of 19, all mechanical):
- `check_attr.rs` — `std::cell::Cell`, `std::collections::hash_map::Entry`,
  `std::slice`, inline `std::path::PathBuf` (IntoDiagArg), `std::iter::repeat_n`
- `check_export.rs` — `std::iter`, `std::ops::ControlFlow`, inline `std::mem::replace`
- `dead.rs` — `std::mem`, `std::ops::ControlFlow`
- `diagnostic_items.rs` — inline `std::iter::once` (×2)
- `eii.rs` — `std::iter`
- `errors.rs` — `std::io::Error`, `std::path::{Path, PathBuf}`
- `input_stats.rs` — inline `std::fmt::Write`
- `reachable.rs` — inline `&dyn std::fmt::Display`
- `stability.rs` — `std::num::NonZero`

Files NO std::: `abi_test.rs`, `debugger_visualizer.rs`, `entry.rs`,
`hir_id_validator.rs`, `lang_items.rs`, `layout_test.rs`, `lib_features.rs`,
`upvars.rs`, `weak_lang_items.rs`, plus `lib.rs` (mod decls only).

All rustc_passes patches are pure RECIPE §1.3 substitutions.

## 1. Per-file diff summary

(Incremental — will be filled in as files land.)

### rustc_metadata

| File | Changes | Markers added |
|------|---------|---------------|
| `Cargo.toml` | TBD | TBD |
| `src/lib.rs` | TBD | TBD |
| `src/creader.rs` | TBD | TBD |
| `src/fs.rs` | TBD | TBD |
| `src/locator.rs` | TBD | TBD |
| `src/errors.rs` | TBD | TBD |
| `src/native_libs.rs` | TBD | TBD |
| `src/rmeta/decoder.rs` | TBD | TBD |
| `src/rmeta/encoder.rs` | TBD | TBD |
| `src/rmeta/decoder/cstore_impl.rs` | TBD | TBD |
| `src/rmeta/mod.rs` | TBD | TBD |
| `src/rmeta/parameterized.rs` | TBD | TBD |

### rustc_passes

| File | Changes | Markers added |
|------|---------|---------------|
| `Cargo.toml` | TBD | TBD |
| `src/lib.rs` | TBD | TBD |
| `src/check_attr.rs` | TBD | TBD |
| `src/check_export.rs` | TBD | TBD |
| `src/dead.rs` | TBD | TBD |
| `src/diagnostic_items.rs` | TBD | TBD |
| `src/eii.rs` | TBD | TBD |
| `src/errors.rs` | TBD | TBD |
| `src/input_stats.rs` | TBD | TBD |
| `src/reachable.rs` | TBD | TBD |
| `src/stability.rs` | TBD | TBD |

## 2. Decisions made (architectural)

(Filled in incrementally.)

## 3. Deferred work, line-precise

TBD on completion.

## 4. New API gaps discovered

TBD on completion.

## 5. Phase-routing summary

TBD on completion.

## 6. Surprises worth flagging upward

TBD on completion.

## 7. Recipe additions

TBD on completion.
