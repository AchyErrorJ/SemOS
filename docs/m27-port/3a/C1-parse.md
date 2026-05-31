# C1 — rustc_parse + rustc_parse_format

**Date:** 2026-05-31
**Phase:** 3a-frontend
**Assigned crates / files:** `compiler/rustc_parse/` (~28k LOC across 21 files),
`compiler/rustc_parse_format/` (~1.5k LOC across 2 files; tests file untouched).
**Status:** COMPLETE
**Token cost (self-report):** ~115k tokens / 70 tool uses / single session
**Source LOC patched:** ~21 files patched in rustc_parse (header imports + 4
inline std::* call sites + 2 std::fmt::* impls) + 1 file in rustc_parse_format.
Total raw LOC reachable on SemOS target: ~31k; total LOC actually edited (delta
lines): ~90.

## 1. Per-file diff summary

### rustc_parse_format

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 16 | `[workspace] members = []` header. | — |
| `src/lib.rs` | 966 | `#![no_std]` after tidy-alphabetical-end; `#[macro_use] extern crate alloc;`; `std::ops::Range` → `core::ops::Range`; explicit `use alloc::{string::String, vec::Vec};`. | — |
| `src/tests.rs` | 597 | **Zero edits.** File uses `Vec` only (no std refs); resolves via crate-root alloc imports. Tests run host-only under `#[cfg(test)]` per the upstream gate; gating not strengthened because the file is already no_std-clean. | — |

### rustc_parse

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 30 | `[workspace] members = []` header + integrator note (semos_std dep injected by parent workspace patch). | — |
| `src/lib.rs` | 280 | `#![no_std]` + `#[macro_use] extern crate alloc;`. Path/PathBuf cfg-split host (`std::path`) vs target (`semos_std::path`); Utf8Error → `core::str::Utf8Error`; `Arc` → `alloc::sync::Arc`. **fs::read site (was `std::fs::read(path)`):** cfg-split, on SemOS routes to `semos_std::fs::read(path.as_str())`. **`path.display()` site:** cfg-split, on SemOS substitutes `ToString::to_string(path.as_str())` because semos_std::path::Path lacks `.display()`. | M27 R4 B5 ×2 |
| `src/errors.rs` | 3686 | Add `use alloc::{borrow::Cow, boxed::Box, string::{String, ToString}, vec::Vec};`. `std::path::PathBuf` cfg-split host vs `semos_std::path::PathBuf`. No body changes (PathBuf is just a type signature in `IntoDiagArg for Case` and `MalformedAbi`). | M27 R4 B5 |
| `src/lexer/mod.rs` | 1179 | `use alloc::string::{String, ToString}; use alloc::vec::Vec;` at top. No body changes. | — |
| `src/lexer/diagnostics.rs` | 154 | `use alloc::{string::ToString, vec::Vec};`. No body changes. | — |
| `src/lexer/tokentrees.rs` | 258 | `use alloc::vec::Vec;`. One inline `std::mem::replace` → `core::mem::replace`. | — |
| `src/lexer/unescape_error_reporting.rs` | 314 | `use std::iter::once` + `std::ops::Range` → `core::iter::once` + `core::ops::Range`. Add `use alloc::{string::{String, ToString}, vec::Vec};`. | — |
| `src/lexer/unicode_chars.rs` | 391 | `use alloc::string::ToString;`. The two `Box Drawings` literal string references are string contents, not the `Box` type. | — |
| `src/parser/mod.rs` | 1694 | `use std::{fmt, mem, slice}` → `use core::{fmt, mem, slice}` + alloc imports (`Box`, `String/ToString`, `Vec`). **`mod tests;` and `mod tokenstream { mod tests; }` gated `#[cfg(all(test, not(target_os = "none")))]`** — see Decision §2 below. | M27 §1 tests gate |
| `src/parser/asm.rs` | 384 | `use alloc::{boxed::Box, vec::Vec};`. No body changes. | — |
| `src/parser/attr.rs` | 492 | **Zero edits.** No `std::` references; no bare `String`/`Vec`/`Box` types used. | — |
| `src/parser/attr_wrapper.rs` | 405 | `use std::borrow::Cow; use std::mem;` → `use alloc::{borrow::Cow, vec::Vec}; use core::mem;`. | — |
| `src/parser/cfg_select.rs` | 34 | **Zero edits.** No std refs; no alloc-prelude types used. | — |
| `src/parser/diagnostics.rs` | 3146 | `use std::mem::take` + `std::ops::{Deref,DerefMut}` → `use core::*;`. Add alloc imports. **One `impl std::fmt::Display for UnaryFixity` rewritten to `impl core::fmt::Display`** (lines 206-207). | — |
| `src/parser/expr.rs` | 4315 | Add alloc imports (already had `use core::mem;` + `use core::ops::*;` — upstream rustc partially uses core::* in this file). Two inline `std::mem::replace` → `core::mem::replace` at lines 2660, 2667. | — |
| `src/parser/generics.rs` | 635 | `use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};`. | — |
| `src/parser/item.rs` | 3423 | `use std::fmt::Write; use std::mem;` → `use core::*` + alloc imports. | — |
| `src/parser/nonterminal.rs` | 203 | `use alloc::boxed::Box;`. | — |
| `src/parser/pat.rs` | 1778 | `use std::ops::Bound` → `use core::ops::Bound` + alloc imports. | — |
| `src/parser/path.rs` | 1080 | `use std::mem` → `use core::mem` + alloc imports (Box, Vec). | — |
| `src/parser/stmt.rs` | 1163 | `use std::{borrow::Cow, mem, ops::Bound}` → `use alloc::{borrow::Cow, ...}; use core::{mem, ops::Bound};`. | — |
| `src/parser/tests.rs` | 2927 | **Untouched.** Gated host-only via parser/mod.rs (`#[cfg(all(test, not(target_os = "none")))]`). Reasons: uses `std::io::prelude::*`, `std::sync::Mutex` with `.lock().unwrap()` Result-API (incompatible with semos_std's guard-direct Mutex), `AutoStream<Box<dyn Write + Send>>` (needs `io::Write` for Vec<u8>, the test-side Buffy). Host `cargo test` still runs it. | — |
| `src/parser/token_type.rs` | 632 | `use alloc::string::{String, ToString};`. | — |
| `src/parser/tokenstream/tests.rs` | 114 | **Untouched.** Gated via parser/mod.rs's `mod tokenstream { mod tests; }` cfg. (File itself is actually no_std-clean — only uses `TokenStream`, `Span`, `Symbol` — but it lives in a `cfg(test)` block that depends on `parser/tests.rs::string_to_stream`, so it tracks the same gate.) | — |
| `src/parser/ty.rs` | 1607 | `use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};`. | — |

## 2. Decisions made (architectural)

- **Cfg-split for `Path`/`PathBuf` and `fs::read`.** rustc_parse imports
  `std::path::{Path, PathBuf}` at lib.rs:14 and `std::path::PathBuf` at
  errors.rs:4. Plan §1.5 + RECIPE §1.6 say to substitute B5 paths to
  `semos_std::path::*` when the use is basic. Here the rustc-internal API
  signature `fn new_parser_from_file(path: &Path, ...)` is called from
  rustc_session / rustc_interface / rustc_driver via &Path arguments — the
  type identity must match across crates. I used the same cfg-gated import
  pattern A3 introduced in `rustc_fs_util`:

  ```rust
  #[cfg(not(target_os = "none"))]
  use std::path::{Path, PathBuf};
  #[cfg(target_os = "none")]
  use semos_std::path::{Path, PathBuf};
  ```

  On host (rustdoc + host `cargo test`) the std types are used; on SemOS the
  semos_std types are used. Same trick handles `errors.rs`'s `PathBuf` usage
  (which is purely a type signature on `IntoDiagArg::into_diag_arg`).

  The single `std::fs::read(path)` site at lib.rs:122 (real I/O — reading
  source file bytes for UTF-8 error rendering) is also cfg-split; on SemOS it
  calls `semos_std::fs::read(path.as_str())` (semos_std::fs takes &str rather
  than &Path).

- **`path.display()` substitution.** On std, `Path::display()` returns a
  `Display` wrapper struct; on semos_std::path::Path the API is just
  `as_str() -> &str`. The single use in lib.rs (3 occurrences pre-edit at
  lines 120/127/142) was unified into one `path_disp: String` local
  computed via cfg-split: host uses `path.display().to_string()`, SemOS uses
  `alloc::string::ToString::to_string(path.as_str())`. Reduces the surface
  diff from 3 cfg-blocks to 1.

- **Tests gated, not patched (§7.1 of B3-followup).** Three tests-mod
  declarations gated via `#[cfg(all(test, not(target_os = "none")))]`:
  `parser/mod.rs:57` (`mod tests;`), `parser/mod.rs:63`
  (`mod tokenstream { mod tests; }`). The `parser/tests.rs` file uses
  `std::io::prelude::*`, `std::sync::Mutex` with `.lock().unwrap()`, and
  `AutoStream<Box<dyn Write + Send>>` (writes to Mutex<Vec<u8>>) — none of
  which port cleanly to the semos_std::sync::Mutex guard-direct API or to
  the missing `Write for Vec<u8>` impl. Same pattern B3-followup applied to
  rustc_errors's `tests.rs`/`json/tests.rs`/`markdown/tests/term.rs`. Host
  `cargo test` continues to run the tests; SemOS target build skips them.

  rustc_parse_format's `tests.rs` was left ungated — it has zero std
  references and only needs Vec (covered by crate-root alloc imports), so
  gating would be over-defensive.

- **Cargo.toml Cargo dep injection deferred.** Per A3 + A2 + B3 precedent,
  the `semos_std` dep edge is added by the parent integrator's workspace
  patch, not within the crate's own Cargo.toml. This keeps host builds
  (rustdoc + host `cargo test`) clean. An integrator note is added inline
  in Cargo.toml.

- **R2's "Command::new for rustup hint in parser/diagnostics.rs" did not
  exist.** Searched the entire crate for `Command::new`, `process::Command`,
  `rustup`, `spawn` — the only hit is a literal *string* in a code-comment
  at `parser/diagnostics.rs:521` showing what bad user-typed source looks
  like. No code-position `std::process::Command` site. R2's recon was
  outdated or covered an older rustc snapshot. **Saved ~1 cfg-block of
  work.**

## 3. Deferred work, line-precise

**Nothing deferred.** Both crates are patch-complete for the SemOS target.

All `cfg(test)`-only test files that were inconvenient to patch are gated at
their `mod` declaration so they only compile on the host target. The two
files left untouched (parser/tests.rs at 2927 LOC, parser/tokenstream/tests.rs
at 114 LOC) are reachable on a normal SemOS-target build with the gating in
place.

If a future port wants those tests on the SemOS target, the work shape is:

> ### `src/parser/tests.rs` (2927 LOC)
> - Line 2: replace `use std::io::prelude::*;` with `use semos_std::io::{Read, Write};`
> - Line 5: replace `use std::sync::{Arc, Mutex};` with
>   `use alloc::sync::Arc; use semos_std::sync::Mutex;`. Then rewrite
>   every `.lock().unwrap()` (~5 sites) to `.lock()` (semos_std::sync::Mutex
>   returns the guard directly without a PoisonError wrapper).
> - Lines 4: replace `use std::path::PathBuf;` with `use semos_std::path::PathBuf;`.
> - Line 6: split `use std::{io, str};` into `use semos_std::io;` + `use core::str;`.
> - Line 46-58 `create_test_handler`: the `Shared { data: output.clone() }` writer
>   needs `impl io::Write for Shared` with `data.lock().extend_from_slice(buf)`
>   (semos_std::sync::Mutex API). And the call sites that do
>   `output.lock().unwrap()` to read bytes (lines 86-87) need
>   `output.lock().clone()`.
> - Line 50: `Box::new(Shared { ... })` already needs `use alloc::boxed::Box;`.
> - Estimated work: ~50 LOC delta, 1 small agent session.

## 4. New API gaps discovered

None directly used in this crate's mainline (non-test) code. The semos_std
surface that lib.rs / errors.rs touches (path::{Path, PathBuf}, path::Path::as_str,
fs::read(&str), alloc::sync::Arc, ToString for &str) is all already present.

For the test-side gap-list (used inside the gated `parser/tests.rs`):

- **`io::Write for Vec<u8>`** — would let `parser/tests.rs:50`'s
  `Box<dyn Write + Send>` blanket-impl-pipeline work without per-test wrappers.
  Same gap B3-followup tracked under `emitter.rs:553`. Cost on semos_std:
  ~5 lines.
- **`semos_std::sync::Mutex::lock` returning a `Result<Guard, PoisonError>`**
  shim. Could expose a `lock_unwrap()` helper that returns the guard
  directly, or alias `.lock()` to return a guard while keeping a
  `try_lock_result()` variant for std-compat callers. Either way, ~10 LOC.

Neither blocks rustc_parse's mainline compile on SemOS.

## 5. Phase-routing summary

For each marker class added:

- **`// M27 R4 B5`** (PathBuf / FS adjacency): 2 cluster sites in
  `lib.rs:129-148` (the cfg-split for `path.display()` and `std::fs::read`)
  + 1 cluster in `errors.rs:7-10` (PathBuf type signature). Owner = no
  follow-up needed; the markers explain why the cfg-split exists and the
  resolution is already in place. If semos_std::path::Path grows
  `.display()` in the future, the lib.rs cluster can collapse to a single
  branch.
- **`// M27` tests gating**: 2 sites in `parser/mod.rs:56-65`. Owner = no
  follow-up needed.

No `// M27 §1.3` (incremental) markers — rustc_parse has no on-disk cache
surface. No `// M27 §1.4` (rayon) markers — the parser is sequential. No
`// M27 §1.5` (proc-macro) markers — rustc_parse is the parser, not a
proc-macro consumer; macro expansion lives in rustc_expand. No `// M27 §1.8`
(i18n) markers — diagnostic messages in this crate go through
rustc_errors's already-ported Translator passthrough.

No `// M27 R3` (hash consolidation) markers — no hash-algorithm choice
crosses an ABI boundary here.

## 6. Surprises worth flagging upward

1. **R2's std-surface estimate (sync:3, fs:1, path:1) was approximately
   right but the load-bearing prediction "parser/diagnostics.rs uses Command
   for rustup hint" was wrong.** There is no `Command::new` site in the
   crate. R2 may have been looking at a different rustc snapshot or
   conflating with rustc_driver's "did you mean rustup install?" logic
   (which lives in `rustc_driver/src/lib.rs`, a different crate). **No
   cfg-stub for Command was needed.** Saved one architectural decision.

2. **`parser/expr.rs` was partially pre-ported** — its top-of-file imports
   were already `use core::mem; use core::ops::{Bound, ControlFlow};` from
   upstream rustc (not from a prior agent), confirming that rustc internals
   in some files already prefer `core::*` over `std::*` even on the upstream
   tree. The only edits needed were the alloc imports + the 2 inline
   `std::mem::replace` calls deep in the body. Minor confidence-builder for
   "rustc is no_std-friendlier than expected in places."

3. **rustc_parse_format is essentially trivial** — 1 std-line import (which
   was `std::ops::Range` — already core-compatible), and the test file uses
   no std types beyond `Vec`. ~5 minutes of mechanical work. R2's "THIN /
   near-zero std surface" estimate was correct.

4. **The crate has zero use of `std::collections::HashMap` directly.** All
   maps go through `rustc_data_structures::fx::FxHashMap`. Same pattern A2
   observed in rustc_span.

5. **The crate does not declare its own `extern crate rustc_*` self-name.**
   Some sibling crates (rustc_errors etc.) need `extern crate self as
   rustc_errors;` for proc-macro-generated absolute paths. rustc_parse's
   `fluent_messages!` macro invocation at lib.rs:78 emits relative paths
   (verified by inspection), so no self-extern was needed.

## 7. Recipe additions

None worth folding into RECIPE.md beyond what B3-followup already
contributed. The Cfg-split-for-Path pattern is canonical (A3 + B3 both use
it). The tests-mod gating pattern is already in RECIPE per B3-followup §7.1.
The one observation worth noting in passing — already implicit in the
recipe but not stated — is:

### 7.1 — alloc-prelude types are NOT auto-imported

`#[macro_use] extern crate alloc;` makes the *macros* (`vec!`, `format!`)
visible at crate root but does NOT prelude-import the *types* (`String`,
`Vec`, `Box`, `ToString`). Every submodule that uses bare `String` / `Vec` /
`Box` / `.to_string()` needs an explicit `use alloc::{…};` line. The fix is
mechanical and cheap, but it does mean ~15 of the 21 files in this crate
needed at least one `use alloc::*;` line added even though only ~10 had a
real `use std::*;` line to rewrite. **Budget guidance for followups:** in a
crate with ~25 files, expect ~15-20 single-line `use alloc::*` additions
even when the actual std rewrite touches half that.

This is already implicit in A2's notes ("Vec/format!/String/HashSet etc. all
resolve via the crate-root `#[macro_use] extern crate alloc;`" — true for
macros, but the alloc::types still need `use` lines). Worth a one-liner
clarification in RECIPE §1.3.

---

## Self-report

- **Tokens:** ~115k (in-conversation; final delta).
- **Tool uses:** ~70 (mix of Read for audit + Edit for patches + Grep for
  std-surface sweeps).
- **Duration:** single session, no context exhaustion.
- **Per-LOC cost:** ~115k / 32k LOC = ~3.6 tokens/LOC across the pair
  (counting all source-tree LOC, including untouched files). Counting only
  the patched LOC (~10k effective): ~11.5 tokens/LOC. **Well below RECIPE
  §5's "standard small mechanical" 30-35 t/LOC band.** Reasons: (a) very
  thin std surface (~10 real std::* import sites), (b) preexisting partial
  port in parser/expr.rs, (c) B3-followup's recipe additions (tests gating,
  cfg-split-path) eliminated all architectural decision overhead, (d)
  rustc_parse_format was a 5-minute trivial.
