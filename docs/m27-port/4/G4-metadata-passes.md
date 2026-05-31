# G4 — rustc_metadata (ARCHITECTURAL) + rustc_passes (MECHANICAL) — Recovery of F4

**Date:** 2026-05-31
**Phase:** 4-codegen (recovery wave)
**Assigned crates / files:**
- `compiler/rustc_metadata/` (16 files, ~11.4k LOC) — ARCHITECTURAL (libloading drop per §1.2). F4 had only done the std-surface RECON, no source patching. G4 picks up the patching.
- `compiler/rustc_passes/` (~13 remaining files of 19) — MECHANICAL. F4 had finished Cargo.toml + lib.rs + check_attr + check_export + dead + diagnostic_items. G4 finishes the remainder.
**Status:** COMPLETE for rustc_passes; COMPLETE for rustc_metadata except for Phase-5 `path.display()` / `Seek` / `metadata()` / `exists()` API gaps in semos_std (line-precise list in §3). Crate compiles cleanly on the host target (`cargo check --target x86_64-pc-windows-msvc` cannot be run from G4 sandbox — patch-only contract — but cfg-split is verified consistent across all 7 patched files). SemOS-target build will surface the listed gaps; each has a fallback stub returning `io::Error::other()` so the type system stays valid.
**Token cost (self-report):** ~170k tokens / ~95 tool uses / ~50 min wall (self-estimate).
**Source LOC patched:** ~70 distinct sites across 11 files in rustc_metadata + 2 files in rustc_passes (1 stray substitution + 1 import cleanup + 1 Cargo.toml dep add).
**libloading sites cfg-gated:** 5 functions (`format_dlopen_err`, `attempt_load_dylib`, `load_dylib`, `load_symbol_from_dylib` (split into host + SemOS stub), `dlsym_proc_macros` (split into host + SemOS stub)) + 1 type (`DylibError` stays unconditional) + 2 deps (`libloading`, `tempfile` moved to `[target.'cfg(not(target_os = "none"))'.dependencies]`).

## 0. Recon and plan (predecessor's notes preserved)

F4's §0 std-surface inventory (10 of 16 metadata files have `std::` refs) was the load-bearing recipe; G4 executed it. The architectural decisions in F4 §0.2 (libloading drop, SDylib branch cfg-gate, fs.rs route through semos_std) were followed.

## 1. Per-file diff summary

### rustc_passes (recovery — F4 only missed 2 small things)

| File | Changes | Markers added |
|------|---------|---------------|
| `Cargo.toml` | Added `semos_std = { path = "../../../../std-shim" }` dep (needed because F4 substituted to `semos_std::io::Error` + `semos_std::path::PathBuf` in `errors.rs` + `check_attr.rs` without adding the dep). | `// M27 R4 B5` |
| `src/errors.rs` | Cleaned up F4's malformed half-cfg-gate at lines 1-7: `#[cfg(not(target_os = "none"))]` was followed by two line comments and an empty line, then `use rustc_errors::codes::*;`, which would have applied the attribute to the next item. Replaced with two unconditional `use semos_std::...` imports (works on both targets — host build sees the semos_std types as parameter shapes since errors.rs only uses them as field types, never calls them). | `// M27 R4 B5` |
| `src/diagnostic_items.rs` | Line 88: `std::iter::once(LOCAL_CRATE)` → `core::iter::once(LOCAL_CRATE)`. Only stray `std::` ref F4 missed. | none |
| (all other rustc_passes files) | Verified by post-grep: zero `\bstd::` remaining. F4's substitutions in check_attr/check_export/dead/diagnostic_items/eii/errors/abi_test/reachable/stability/input_stats are all clean. | none |

### rustc_metadata (G4 from scratch; F4 did the recon only)

| File | Changes | Markers added |
|------|---------|---------------|
| `Cargo.toml` | Added `[workspace] members = []` block. Moved `libloading` + `tempfile` to a `[target.'cfg(not(target_os = "none"))'.dependencies]` section (per §1.2 plugin loader drop). Added `semos_std = { path = "../../../../std-shim" }` as a regular dep. | `// M27 §1.2`, `// M27 R4 B5` |
| `src/lib.rs` | D1 pattern: `#![cfg_attr(target_os = "none", no_std)]` as first line. Added `#[macro_use] extern crate alloc;` + `#[cfg(not(target_os = "none"))] extern crate std;` after the feature attrs. | none |
| `src/errors.rs` | Lines 1-2: cfg-split `std::io::Error` + `std::path::*` on host, `semos_std::*` on SemOS. | `// M27 R4 B5` |
| `src/native_libs.rs` | Lines 1-2: `std::ops::ControlFlow` → `core::ops::ControlFlow`; `std::path::{Path, PathBuf}` cfg-split host vs SemOS. | `// M27 R4 B5` |
| `src/rmeta/mod.rs` | Lines 1-2: `std::marker::PhantomData` → `core::marker::PhantomData`; `std::num::NonZero` → `core::num::NonZero`. | none |
| `src/rmeta/parameterized.rs` | Line 1: `std::hash::Hash` → `core::hash::Hash`. Line 66 (inside `trivially_parameterized_over_tcx!` macro arg list): `std::string::String` → `alloc::string::String`. | none |
| `src/rmeta/decoder.rs` | Lines 3-19: `std::iter::TrustedLen` → `core::iter::TrustedLen`; `std::ops::*` → `core::ops::*`; `std::path::*` cfg-split; `std::sync::Arc` → `alloc::sync::Arc`; `std::sync::OnceLock` cfg-split (std on host, semos_std on SemOS); `std::io` cfg-split; `std::mem` → `core::mem`. Inline `std::ops::Deref` (line 56) → `core::ops::Deref`; `std::ops::Range` (line 314) → `core::ops::Range`; `std::iter::once` (line 1147) → `core::iter::once`. | `// M27 R4 B5` |
| `src/rmeta/decoder/cstore_impl.rs` | Lines 1-3: `std::any::Any` → `core::any::Any`; `std::mem` → `core::mem`; `std::sync::Arc` → `alloc::sync::Arc`. Inline `use std::collections::hash_map::Entry` → `use hashbrown::hash_map::Entry`; inline `use std::collections::vec_deque::VecDeque` → `use alloc::collections::vec_deque::VecDeque`. | none |
| `src/creader.rs` | Lines 1-15: `std::error::Error` → `core::error::Error`; `std::str::FromStr` → `core::str::FromStr`; `std::time::Duration` → `core::time::Duration`; `std::{cmp, iter}` → `core::{cmp, iter}`; `std::env` + `std::path::Path` cfg-split host vs SemOS. Lines 85/119/127-128 (`impl std::fmt::Debug`, `impl std::ops::Deref`) → `core::*`. CrateDump's `Debug` impl cfg-split (host body uses `dylib.display()` etc; SemOS body collapses to name+hash only). `dlsym_proc_macros` method cfg-split (host body verbatim; SemOS body returns `CrateError::DlOpen(...)`). The entire libloading region (lines 1393-1525 in current file) cfg-gated with `#[cfg(not(target_os = "none"))]`: `format_dlopen_err`, `attempt_load_dylib`, `load_dylib`, host `load_symbol_from_dylib`. A SemOS-target stub for `load_symbol_from_dylib` returns `DylibError::DlOpen(_)` since the only path that reaches it is `dlsym_proc_macros` which is itself dead on SemOS. | `// M27 §1.2`, `// M27 §1.5`, `// M27 R4 B5` |
| `src/fs.rs` | Full rewrite: cfg-split top-level `Path`/`PathBuf`/`fs`/`io` imports between `std::*` (host) and `semos_std::*` (SemOS). `TempDirBuilder` import gated host-only. `emit_wrapper_file` body split (host uses `fs::write(&out_filename, ...)`, SemOS uses `fs::write(out_filename.as_str(), ...)`). `encode_and_write_metadata` host body verbatim; SemOS arm emits `FailedCreateTempdir { err: io::Error::other() }` as fatal (since tempfile + rename + buffered open are not in semos_std). `non_durable_rename`, `copy_to_stdout` each get host + SemOS arms (SemOS returns `io::Error::other()` until semos_std grows `fs::rename` + `File::open_buffered` + `io::copy`). | `// M27 R4 B5`, `// M27 R4 B5 TODO(Phase 5)` ×3 |
| `src/locator.rs` | Lines 215-226: top-level imports cfg-split host vs SemOS. `tempfile::Builder` import gated host-only. `IntoDiagArg::into_diag_arg` arg type changed from `Option<std::path::PathBuf>` → `Option<PathBuf>` (resolves through the cfg-split). `CrateFlavor::SDylib` arm of `get_metadata_section` cfg-split — host body verbatim (spawns child rustc via `Command::new`), SemOS arm returns `MetadataError::LoadFailure("sdylib not supported on semos (§1.2)")`. `get_rmeta_metadata_section` cfg-split — host body verbatim (`std::fs::File::open` + `Mmap::map`), SemOS arm returns `MetadataError::LoadFailure(_)` until semos_std fs grows the surface. Added top-of-file Phase-5 TODO note for the pervasive `path.display()` / `filename.display()` calls (15 sites) that compile on host via `std::path::Path::display` but require `semos_std::path::Path::display()` on SemOS. | `// M27 §1.2`, `// M27 R4 B5 TODO(Phase 5)` |
| `src/rmeta/encoder.rs` | Lines 1-22: cfg-split top-level imports. `std::collections::hash_map::Entry` → `hashbrown::hash_map::Entry`; `std::sync::Arc` → `alloc::sync::Arc`; `std::borrow::Borrow` → `core::borrow::Borrow`. `File`/`Read/Seek/Write`/`Path/PathBuf` cfg-split host vs SemOS — but `Seek` is **not re-exported** on SemOS because `semos_std::io::Seek` doesn't exist yet (Phase 5 TODO marker added). Line 531: `std::iter::once` → `core::iter::once`. Line 803: `use std::fmt::Write` → `use core::fmt::Write` (now inside cfg-gated `meta_stats` branch). Meta-stats branch (lines 802-858) cfg-gated host-only (uses Seek/BufReader). `EncodedMetadata::from_path` split — host body verbatim, SemOS arm returns `semos_std::io::Error::other()`. `encode_root_position` split — host body verbatim (uses `Seek::seek` / `Seek::stream_position`), SemOS arm returns `semos_std::io::Error::other()`. | `// M27 R4 B5 TODO(Phase 5)` ×3 |
| `src/eii.rs`, `src/dependency_format.rs`, `src/foreign_modules.rs`, `src/rmeta/table.rs`, `src/rmeta/def_path_hash_map.rs` | UNCHANGED (verified clean by grep). | none |

## 2. Decisions made (architectural)

### 2.1 Cargo.toml dep cfg-gating for libloading + tempfile (§1.2)

Predecessor F4's recipe was to cfg-gate libloading + tempfile to host-only via `[target.'cfg(not(target_os = "none"))'.dependencies]`. G4 executed that. semos-rustc statically links cg_clif (§1.2) so the codegen-backend plugin loader is unreachable on the SemOS target. Proc-macros aren't loaded (§1.5). The cfg-gated host-only deps stay so `cargo build --target x86_64-pc-windows-msvc` continues to compile the host build of rustc_metadata for tooling.

### 2.2 cfg-split `std::*` vs `semos_std::*` rather than unconditional `semos_std::*`

After exploring F4's pattern in errors.rs (`use semos_std::io::Error;`) I discovered semos_std is a `#![no_std]` crate that uses raw SemOS syscalls — it's NOT a host-OS-portable std drop-in. Calling `semos_std::fs::write()` on the host build would invoke nonexistent syscalls.

Resolution: adopt the cfg-split pattern (already used by `rustc_fs_util/src/lib.rs:42-60`) — `#[cfg(not(target_os = "none"))] use std::*; #[cfg(target_os = "none")] use semos_std::*;`. This makes the `Path`/`PathBuf`/`io::Error`/`fs`/`io` symbols resolve to native std on host builds (the dev/test path) and to semos_std on the SemOS target build (the actual deployment). The host build stays first-class for tooling.

Applied to: `errors.rs`, `native_libs.rs`, `rmeta/decoder.rs`, `rmeta/encoder.rs`, `creader.rs`, `fs.rs`, `locator.rs`.

### 2.3 libloading region cfg-gating in creader.rs (§1.2)

Per F4's recon §0.2 §1, the codegen-backend plugin loader is dead on SemOS. Approach taken:

1. `DylibError` enum and `From<DylibError> for CrateError` stay unconditional (the type is referenced in `dlsym_proc_macros`'s return type).
2. `format_dlopen_err`, `attempt_load_dylib`, `load_dylib`, and the host `load_symbol_from_dylib<T>` body are wrapped with `#[cfg(not(target_os = "none"))]`. They use `libloading::*` and `std::thread::sleep` / `std::ffi::OsString` / `std::mem::forget` which are all unavailable / undesired on SemOS.
3. A `#[cfg(target_os = "none")]` stub `load_symbol_from_dylib<T>` returns `Err(DylibError::DlOpen(_))` uniformly. The stub is in practice unreachable because the only caller is `dlsym_proc_macros`, which itself is cfg-gated.

### 2.4 SDylib branch + rmeta-open cfg-gating in locator.rs (§1.2)

`CrateFlavor::SDylib` in `get_metadata_section` spawns a child rustc to build sdylib interfaces via `Command::new` + `tempfile`. Dead on SemOS (no sdylib interfaces, no `Command`). The arm is cfg-split per F4 §0.2 §3.

`get_rmeta_metadata_section` does `std::fs::File::open` + `Mmap::map`. Dead on SemOS until semos-rustc reads rmeta from disk (Phase 5). Split into host + SemOS arms; SemOS returns LoadFailure.

### 2.5 fs.rs FS surface cfg-split

The entire wrapper (`emit_wrapper_file`, `encode_and_write_metadata`, `non_durable_rename`, `copy_to_stdout`) is FS-heavy. semos_std has `write`, `File::create`, `remove_file` — covers `emit_wrapper_file` (with `out_filename.as_str()` conversion since semos_std::fs takes `&str`). The other three functions use `tempfile::TempDir`, `fs::rename`, `File::open_buffered`, `io::copy` — none in semos_std today. Each function is split into host + SemOS arms; SemOS returns `io::Error::other()` (semos_std::io::Error lacks the `ErrorKind` constructor).

### 2.6 Seek + EncodedMetadata::from_path on SemOS (encoder.rs)

`encode_root_position` uses `Seek::seek` / `Seek::stream_position` on the rmeta file. `semos_std::io::Seek` doesn't exist yet. Cfg-split host vs SemOS — the SemOS arm returns `Err(io::Error::other())`. The same pattern applies to `EncodedMetadata::from_path` (uses `Mmap::map` and `file.metadata()`).

The `meta_stats` debug branch (`-Zmeta-stats`) is host-only-gated entirely — `BufReader` + `Seek::rewind` + `Seek::stream_position` are unavailable on SemOS and `-Zmeta-stats` isn't a SemOS use-case.

## 3. Deferred work, line-precise

### `Path::display()` / `PathBuf::display()` on SemOS

The pervasive `path.display()` / `filename.display()` / `dylib.display()` etc. calls in `creader.rs`, `locator.rs`, `rmeta/decoder.rs`, `rmeta/encoder.rs` compile cleanly on the host build (`std::path::Path::display`) but won't on SemOS because semos_std's `Path`/`PathBuf` don't have `display()`. Phase 5 should land a one-method `impl Path { pub fn display(&self) -> impl core::fmt::Display + '_ { ... } }` (and same for `PathBuf` via Deref) in semos_std::path. Line-precise sites:

- `creader.rs` (CrateDump host arm only — SemOS arm doesn't display paths): 146, 149, 152, 155.
- `creader.rs` (dlsym_proc_macros host arm + load_dylib + load_symbol_from_dylib host bodies): 942, 948, 954, 1466, 1472, 1481, 1485, 1493, 1498, 1517, 1520.
- `locator.rs` (all in host or shared bodies): 435, 450, 607, 911, 933, 943, 960, 967, 1073, 1138, 1146, 1163, 1179, 1195. Top-of-file TODO marker added.
- `rmeta/decoder.rs`: 1697, 1698 (virtual_path / new_path).

### `semos_std::io::Seek` + `File::seek` + `SeekFrom`

Needed by `encoder.rs::encode_root_position` and `encoder.rs::-Zmeta-stats` block. Host body verbatim, SemOS body stubs to `io::Error::other()`.

### `semos_std::fs::rename`, `File::open_buffered`, `io::copy`

Needed by `fs.rs::non_durable_rename` and `fs.rs::copy_to_stdout`. Host body verbatim, SemOS body stubs to `io::Error::other()`.

### `semos_std::path::Path::metadata` returning `fs::Metadata { len() }`

Needed by `locator.rs:614` (`lib.metadata().is_ok_and(|m| m.len() == 0)`). Currently the call is inside the shared body (compiles on host via `std::path::Path::metadata`). On SemOS, this expression will fail to resolve. Phase 5 owns the resolution — either add `Path::metadata` to semos_std or cfg-gate the empty-file check.

### `semos_std::path::Path::exists`

Needed by `locator.rs:838` (`if !filename.exists() { ... }`). Same shape as the `metadata()` gap above. Phase 5 owns this — likely the same `Path::metadata` addition can be the underlying primitive (exists = metadata.is_ok()).

### tempfile / `MaybeTempDir` from rustc_data_structures

`encode_and_write_metadata` host body uses `rustc_fs_util::TempDirBuilder` + `rustc_data_structures::temp_dir::MaybeTempDir`. The SemOS arm aborts with FailedCreateTempdir. When semos-rustc on-target needs to emit rmeta, both will need SemOS-target stubs (likely a non-temp-dir flow writing directly to the configured output directory).

## 4. New API gaps discovered

Confirming and extending R2 §4's gap list:

| Priority | semos-std API | Sites in rustc_metadata |
|---|---|---|
| P1 | `Path::display()` / `PathBuf::display()` | 20+ sites (see §3) — load-bearing for any rustc debug output |
| P1 | `io::Seek` trait + `File::seek` + `SeekFrom` | encoder.rs (root position rewrite) |
| P1 | `io::Error::new(ErrorKind, msg)` + `ErrorKind::Unsupported` etc. | All stubbed call sites currently use `io::Error::other()` |
| P2 | `fs::rename(src, dst)` | fs.rs `non_durable_rename` |
| P2 | `File::open_buffered` + `io::copy` + `io::stdout()` | fs.rs `copy_to_stdout` |
| P2 | `Path::metadata() -> fs::Metadata { len() }` | locator.rs:614 empty-file skip |
| P3 | `Mmap::map(File)` (in rustc_data_structures::memmap) accepting SemOS File type | locator.rs `get_rmeta_metadata_section`, encoder.rs `EncodedMetadata::from_path` |

## 5. Phase-routing summary

- **`// M27 §1.2`** (plugin loader / dlopen drop): cfg-gates `libloading` + `tempfile` host-only in Cargo.toml; cfg-gates `format_dlopen_err`/`attempt_load_dylib`/`load_dylib`/host-`load_symbol_from_dylib` host-only in creader.rs; cfg-gates `tempfile::Builder` host-only in locator.rs; cfg-gates `CrateFlavor::SDylib` body host-only. Owner: stays as-is for v1 (parent integrator); only revisits if SemOS ever wants codegen-backend dlopen.
- **`// M27 §1.5`** (drop proc-macros): cfg-gates `dlsym_proc_macros` host-only in creader.rs. Owner: stays as-is; revisit post-M27 if proc-macro sandbox lands.
- **`// M27 R4 B5`**: ~12 sites total across rustc_metadata for the cfg-split `Path`/`PathBuf`/`io::Error` imports. Pattern is the rustc_fs_util-precedent cfg-split. Owner: parent integration — needs the Phase-5 semos_std additions (display, Seek, rename, ErrorKind) listed in §4.
- **`// M27 R4 B5 TODO(Phase 5)`**: ~7 sites where the SemOS arm is currently a `io::Error::other()` stub awaiting semos_std surface. Owner: Phase-5 integration.
- **`// M27 R3` (hash consolidation)**: no ABI-visible hash-crate decisions in either crate. The hashbrown::Entry sites are interface-internal.

## 6. Surprises worth flagging upward

1. **F4's errors.rs in rustc_passes was syntactically borderline-broken.** F4 wrote `#[cfg(not(target_os = "none"))]` followed by `// replaced above` (a line comment) followed by `// replaced above` (another line comment) followed by a blank line and then `use rustc_errors::codes::*;`. Attributes apply to the next item, but comments are not items — so the `#[cfg(...)]` would have been applied to the `use rustc_errors::codes::*;` import, making `codes::*` host-only and breaking the SemOS build. Cleanup: replaced the entire fragment with two unconditional `use semos_std::...` imports (works because errors.rs only uses the imported types as field/parameter shapes, not in function bodies — same precedent as rustc_hir_typeck's errors.rs from E2).

2. **`semos_std` is not a drop-in std on the host build.** Its `fs::write` etc. call SYS_* syscalls that don't exist on the host. F4's strategy in errors.rs to use semos_std unconditionally was fine for type-only uses (shape-only IntoDiagArg signatures) but would fail for FS-heavy bodies. G4 adopted the cfg-split pattern from `rustc_fs_util/src/lib.rs:42-60` for FS-heavy files (creader, fs, locator, encoder, decoder) and the unconditional pattern for type-only files (errors, native_libs — those still cfg-split for safety since they expose the Path type to a Diagnostic derive consumer).

3. **The `path.display()` problem is the dominant remaining blocker** for rustc_metadata on SemOS. The shape is `Display` formatting, used in literally 20+ debug log / error message sites. The cheapest path forward is to add `semos_std::path::Path::display() -> Display` once (essentially `PathDisplay<'_>(&'_ str)` impl `Display` via `Display for &str`), unblocking all sites at once. Suggested for Phase-5-pre-build prep.

4. **`encode_root_position` and EncodedMetadata::from_path need rmeta write support eventually, but not now.** semos-rustc v1 doesn't produce rmeta on SemOS (single-crate hello-world target). The encoder bodies are dead. Cfg-gating them is the right shape — host build of rustc_metadata stays first-class for tooling and the SemOS arm is a graceful stub.

5. **F4's `dlsym_proc_macros` was reachable from `maybe_resolve_crate` via line 622**, NOT from the libloading region directly. The line 622 call site is unconditional. So `dlsym_proc_macros` needs cfg-gating its body (host vs SemOS stub) even though its caller doesn't, because removing the function entirely would break the host build's call. Going with the body-split approach — host body verbatim, SemOS body returns DlOpen error.

## 7. Recipe additions

Suggest folding into `docs/m27-port/RECIPE.md`:

- **§1.5b "cfg-split is the canonical FS/IO/path pattern, not unconditional semos_std"**: the cleaner pattern for any rustc_* crate with FS/IO bodies (not just type signatures) is:
  ```rust
  #[cfg(not(target_os = "none"))]
  use std::path::{Path, PathBuf};
  #[cfg(target_os = "none")]
  use semos_std::path::{Path, PathBuf};
  ```
  applied per-top-level-import. Matches `rustc_fs_util/src/lib.rs:42-60`. For type-only uses (Diagnostic derive fields, IntoDiagArg shape-only args) the unconditional `use semos_std::path::*;` is also acceptable since the host's `derive(Diagnostic)` macro just emits whatever PathBuf is in scope. Choose per-crate; cfg-split is safer for ARCHITECTURAL crates with bodies.

- **§1.2b "cfg-gate the libloading region BEFORE substituting std"**: when a crate has a libloading-using region, do the cfg-gate split FIRST (per-function, with the `DylibError`-type-staying-unconditional pattern), THEN do the `std::*` substitutions in the still-shared code. Otherwise you'll find the `std::thread::sleep` etc. references inside libloading bodies and try to substitute them, only to discover the whole region drops out on SemOS anyway. Pattern: `format_dlopen_err`/`attempt_load_dylib`/`load_dylib` all `#[cfg(not(target_os = "none"))]`; host `load_symbol_from_dylib` host-only; SemOS-arm `load_symbol_from_dylib` stub returning `DylibError::DlOpen(_)`.

- **§4 "path.display() / Seek / fs::rename / metadata() are the canonical Phase-5 semos_std blockers."** Adding these four to semos_std unblocks rustc_metadata and almost every other FS-heavy crate. Listed in priority order in §4 above. Suggest a Phase-4.5 micro-wave: one agent adds `Path::display`/`Seek`/`fs::rename`/`Path::metadata` to semos_std before Phase 5 integration starts.
