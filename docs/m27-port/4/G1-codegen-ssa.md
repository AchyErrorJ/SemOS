# G1 — rustc_codegen_ssa (continuation of F1)

**Date:** 2026-05-31
**Phase:** 4-codegen
**Scope:** finish all remaining files in `compiler/rustc_codegen_ssa/src/` after F1's foundation (lib.rs + back/mod.rs + base.rs + traits/backend.rs).
**Status:** DONE (Phase 4 patches; Phase 5 will revisit `back/write.rs` worker driver + `back/metadata.rs` Mmap path).

## 1. Per-file diff summary (running)

F1 already committed: Cargo.toml, lib.rs, back/mod.rs, base.rs, traits/backend.rs.

| File | Status | Changes |
|------|--------|---------|
| src/mir/mod.rs | DONE | `use std::iter` → `use core::iter`. |
| src/mir/operand.rs | DONE | `use std::fmt` → `use core::fmt`. |
| src/mir/block.rs | DONE | `use std::cmp` → `use core::cmp`. |
| src/mir/locals.rs | DONE | `use std::ops::{Index, IndexMut}` → `use core::ops::...`. |
| src/mir/naked_asm.rs | DONE | `std::fmt::Write` → `core::fmt::Write`; `std::fmt::Result` → `core::fmt::Result`. |
| src/mir/debuginfo.rs | DONE | `std::collections::hash_map::Entry` → `rustc_data_structures::fx::StdEntry as Entry`; `std::{marker,ops}` → `core::{marker,ops}`. |
| src/traits/debuginfo.rs | DONE | `use std::ops::Range` → `use core::ops::Range`. |
| src/traits/misc.rs | DONE | `use std::cell::RefCell` → `use core::cell::RefCell`. |
| src/traits/builder.rs | DONE | `std::ops::Deref` → `core::ops::Deref`; `std::cmp::Ordering` → `core::cmp::Ordering`. |
| src/traits/mod.rs | DONE | `use std::fmt` → `use core::fmt`. |
| src/traits/write.rs | DONE | `use std::path::PathBuf` cfg-split host/semos. |
| src/traits/backend.rs (F1) | EXTENDED | F1 had cfg-gated `link_binary` + `link` + `spawn_named_thread`; now also cfg-gated `use crate::back::archive::ArArchiveBuilderBuilder` (archive mod is whole-gated, see §1.7 extension). |
| src/common.rs | DONE | `std::fmt::Write` → `core::fmt::Write`. |
| src/codegen_attrs.rs | DONE | `std::str::FromStr` → `core::str::FromStr`. |
| src/assert_module_sources.rs | DONE | `std::borrow::Cow` → `alloc::borrow::Cow`; `std::fmt` → `core::fmt`; cfg-split `into_diag_arg` for the `Option<PathBuf>` parameter type. |
| src/debuginfo/type_names.rs | DONE | `std::fmt::Write` → `core::fmt::Write`. |
| src/target_features.rs | DONE | `std::collections::hash_map::Entry` (called on UnordMap) → `rustc_data_structures::fx::StdEntry`. |
| src/size_of_val.rs | DONE | `std::cmp::max` → `core::cmp::max`. |
| src/back/mod.rs (F1) | EXTENDED | F1 gated `apple`/`command`/`link`/`linker`; G1 added `archive` to the gated set (M27 §1.7 extension — rlib emission unreachable, see §2). |
| src/back/metadata.rs | DONE | host/semos cfg-split for `std::path::Path` (Path now from semos_std on target); cfg-gated `use super::apple` + `set_macho_build_version` block + `macho_object_build_version_for_target`; cfg-gated host `load_metadata_with` and added SemOS stub returning `Err("M27 R4 B5: …")` since Mmap doesn't exist on semos_std; replaced `Vec<u8>::write_all` with `extend_from_slice` (no io::Write for Vec on no_std). `File` import is host-only. |
| src/back/lto.rs | DONE | `std::ffi::CString` → `alloc::ffi::CString`; `std::sync::Arc` → `alloc::sync::Arc`. |
| src/back/symbol_export.rs | DONE | `std::collections::hash_map::Entry::*` cfg-split into std vs hashbrown. |
| src/back/write.rs | DONE-with-stubs | host-mostly-intact; SemOS arm via cfg-gates: `mpsc_stub` private module replaces `std::sync::mpsc`; whole-function gates on `start_executing_work`, `spawn_work`, `spawn_thin_lto_work`, `do_fat_lto`, `do_thin_lto`, `execute_optimize_work_item`, `execute_copy_from_cache_work_item`, `submit_pre_lto_module_to_llvm` (all `abort_with_code(101)`); `copy_to_stdout` SemOS stub returns `Err(other)`; `ensure_removed` SemOS stub no-op; `Coordinator::join` cfg-split signature; `produce_final_output_artifacts::copy_gracefully` cfg-split (SemOS no-op); all `rustc_incremental::*` call sites cfg-gated. Imports cfg-split for `std::{fs,io,thread}` → `semos_std::{fs,io,thread}`; `core::{mem,str}` on target. |
| src/errors.rs | DONE | `std::ffi::OsString`, `std::io::Error`, `std::path::{Path,PathBuf}` cfg-split host/semos; `std::process::ExitStatus` host-only; `crate::back::command::Command` host-only; `LinkingFailed`/`ProcessingDymutilFailed`/`UnableToRunDsymutil`/`StrippingDebugInfoFailed`/`UnableToRun` diagnostic structs cfg-gated (linker-error-only, gated module); inline `Box<dyn std::error::Error>` → `Box<dyn core::error::Error>` (8x); inline `std::io::Error` → `Error` (6x); `IntoDiagArg`-for-DebugArgPath + ExpectedPointerMutability + CguReuse: each cfg-split `into_diag_arg(_, PathBuf)` for host/semos PathBuf. |
| src/back/mod.rs (F1) | EXTENDED-AGAIN | Added `archive` to the gated set (G1.D1) and `rpath` (only consumer is gated link). |
| Cargo.toml | UPDATED | Added `hashbrown` direct dep and `semos_std = { path = "../../../../std-shim" }`. |

## 2. Decisions made (architectural)

### G1.D1 — extend M27 §1.7 to `back::archive`
F1 gated apple/command/link/linker. `back::archive` is the ar-format rlib
builder (uses `ar_archive_writer` + `tempfile::Builder` + `fs::rename` +
`File::create_new`). Since cg_clif emits ET_EXEC directly, semos-rustc
produces no rlib/staticlib outputs. The only `ArArchiveBuilderBuilder`
reference outside the archive module is in `traits/backend.rs::link()`
which is itself cfg-gated. Gating the whole module at the `mod` line
saves ~600 LOC of tempfile/Command/dlltool plumbing translation.

### G1.D2 — metadata.rs stays live, with Mmap deferred
`back::metadata::DefaultMetadataLoader` is reachable on SemOS at run-time
(reading sysroot rlibs to bootstrap libcore). But `Mmap::map(File)`
relies on host `std::fs::File` + memmap2 — neither exists on semos_std.
For now the SemOS arm of `load_metadata_with` returns `Err(M27 R4 B5)`.
Phase 5 decision: either materialize an `semos_std::fs::Mmap` shim
(needs kernel mmap syscall) or pre-decompress sysroot rlibs to plain
bytes at sysroot-bake time.

## 3. Deferred work, line-precise

- `back/metadata.rs:load_metadata_with` SemOS arm — M27 R4 B5 (Mmap).
- `back/write.rs` worker pool — entire LTO + worker driver gated on SemOS, stubbed to `abort_with_code(101)`:
  - `start_executing_work`
  - `spawn_work`
  - `spawn_thin_lto_work`
  - `do_fat_lto`, `do_thin_lto`
  - `execute_optimize_work_item`, `execute_copy_from_cache_work_item`
  - `submit_pre_lto_module_to_llvm`
- `back/write.rs:copy_gracefully` — SemOS arm is a no-op (semos_std::fs lacks `copy(&Path, &Path)`).
- `back/write.rs:copy_to_stdout` — SemOS stub returns `Err(other)`.
- `back/write.rs:Coordinator::join` — SemOS returns `Result<_, ()>` instead of `std::thread::Result`.
- `back/write.rs:Sender/Receiver/channel` — SemOS stub aborts on send/recv (Phase 5 implements semos_std::sync::mpsc).
- `back/write.rs:ensure_removed` — SemOS stub no-op (no per-CGU temp files on SemOS).
- All `rustc_incremental::*` call sites in back/write.rs cfg-gated to no-op on SemOS (M27 §1.3).

## 4. New API gaps discovered

- `Vec<u8>` does not impl `semos_std::io::Write` (only `std::io::Write` has the blanket impl). Worked around with `extend_from_slice` in metadata.rs:610. Should add `impl semos_std::io::Write for Vec<u8>` if many other sites need it (Phase 5).
- `semos_std::sync::mpsc` does not exist — worked around with `mpsc_stub` private module in back/write.rs (aborts on every method). Phase 5 needs real mpsc OR a synchronous codegen driver.
- `semos_std::fs::copy(&Path, &Path)` and `&Path`-taking `fs::write` do not exist (semos_std::fs::* takes `&str`). For write.rs sites this is downgraded to TODO since the LTO worker is wholly gated anyway.
- `semos_std::thread::JoinHandle::join` returns `Result<T, ()>` not `Result<T, Box<dyn Any+Send>>`. Diverges from std::thread::Result; affects Coordinator::join signature on SemOS.
- `rustc_data_structures::memmap::Mmap` has a no_std fallback (`Vec<u8>`) — confirmed at memmap.rs:128. Reusable wherever Mmap appears in unconditional code.

## 5. Phase-routing summary

- **Phase 4 (this agent)**: 53 src/ files patched + Cargo.toml dep added. All "MIR→IR plumbing" (mir/*, common.rs, meth.rs, mono_item.rs, traits/*, debuginfo/*, codegen_attrs.rs, target_features.rs, assert_module_sources.rs, size_of_val.rs) → mechanical `std::* → core::*` / `alloc::*` substitution. Heavy `back/` files split: `link`/`linker`/`command`/`apple`/`archive`/`rpath` whole-module-gated (M27 §1.7); `metadata`/`lto`/`symbol_export`/`write` patched-with-cfg-splits.
- **Phase 5 (followup)**: replace `mpsc_stub` with real `semos_std::sync::mpsc` (or rewrite the codegen worker driver to be synchronous). Add `impl semos_std::io::Write for Vec<u8>`. Implement `semos_std::fs::Mmap` and a `Path`-aware `fs::write`/`fs::copy` so `back/metadata.rs::load_metadata_with` can read sysroot rlibs. None of these block compile; runtime exits with `abort_with_code(101)` when reached.

## 6. Surprises worth flagging upward

- `rustc_data_structures::fx::StdEntry` is a type alias; you cannot `use StdEntry::*` to bring `Occupied/Vacant` into scope. Use a cfg-split `use std::collections::hash_map::Entry::*; / use hashbrown::hash_map::Entry::*;` instead (done in back/symbol_export.rs:1).
- `OutFileName::is_tty` in `rustc_session/src/config.rs:1085` uses `std::io::IsTerminal`. This is reachable from rustc_codegen_ssa back/write.rs and will need its own host/semos cfg-split when rustc_session is ported. **Not patched here** — rustc_session is a separate crate.
- back/archive.rs's `try_extract_macho_fat_archive` would still be reachable from `back/link.rs` callers, but since both modules are whole-gated together, the dependency is internal and the gate cuts cleanly.
- `regex::bytes::Regex` used in errors.rs `LinkingFailed::into_diag` — gated under the same cfg-not-none as LinkingFailed itself. No regex port needed for SemOS in this crate.

## 7. Recipe additions

- **G1.R1 — Diagnostic structs that contain external-process types**: when a diagnostic struct references `ExitStatus`, `crate::back::command::Command`, or anything from a gated linker module, gate the struct + its `Diagnostic` impl with `#[cfg(not(target_os = "none"))]`. Calls sites (in gated link/linker code) compile away on SemOS. (5 structs in errors.rs: LinkingFailed, ProcessingDymutilFailed, UnableToRunDsymutil, StrippingDebugInfoFailed, UnableToRun.)
- **G1.R2 — `IntoDiagArg::into_diag_arg(_: &mut Option<PathBuf>, …)` requires per-target signature**: PathBuf type differs (std vs semos_std), and trait method signatures must match the concrete PathBuf in scope. Use `#[cfg(not(target_os = "none"))]` + `#[cfg(target_os = "none"))]` paired method bodies (the duplicate is identical body text — Rust allows the two-method cfg-split pattern). Seen in `assert_module_sources.rs:222`, `errors.rs:179, 1082`.
- **G1.R3 — Worker pool / async driver = gate whole functions, not lines**: rustc's codegen worker uses thread::Builder + jobserver + mpsc all over. Trying to substitute line-by-line is a rabbit hole. Cleanest: whole-fn-gate every fn that spawns or sends/recvs, expose a SemOS stub that aborts. Phase 5 reimplements the driver synchronously. Pattern: `#[cfg(not(target_os = "none"))] fn foo(...) { real_body }` + `#[cfg(target_os = "none")] fn foo(_: ...) -> RetTy { semos_std::process::abort_with_code(101) }`.
- **G1.R4 — `mpsc_stub` for `std::sync::mpsc` shim**: when a crate consumes the mpsc API but semos_std doesn't provide it, drop a `mod mpsc_stub { … }` with `Sender<T>(PhantomData<T>)` / `Receiver<T>(PhantomData<T>)` / `channel<T>() -> (S, R)` that just abort on send/recv. The shim lets the rest of the file compile and the host build is untouched. (back/write.rs:18–47.)
- **G1.R5 — `back::archive` is whole-gateable on SemOS**: builds ar-format rlibs; cg_clif emits ET_EXEC directly so rlibs are unreachable. Same shape as `back::link`, `back::linker`, `back::command`, `back::apple`. Extends F1's M27 §1.7 list.

