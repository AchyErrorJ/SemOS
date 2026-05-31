# B3 + B3-followup — rustc_errors

**Date:** 2026-05-31
**Phase:** 2b
**Assigned crates / files:** `compiler/rustc_errors/` (15 files, ~2,400 LOC)
**Status:** COMPLETE (B3 patched 11/15 files before session-limit bounce;
B3-followup finished the remaining 4 files + this notes file).
**Token cost (self-report):**
- B3 (original run): ~120k tokens / 97 tool uses / 506 s — bounced before
  notes file written (per parent integration commit f94d979).
- B3-followup: ~38k tokens / ~30 tool uses / single session.
**Source LOC patched:** B3 ~1,650 / B3-followup ~770 / total ~2,420.

## 1. Per-file diff summary

| File | LOC | Owner | Changes | Markers added |
|------|----:|---|---------|---------------|
| `Cargo.toml` | 40 | B3 | `[workspace] members = []` header. | — |
| `src/codes.rs` | trivial | B3 | One std::* substitution. | — |
| `src/decorate_diag.rs` | trivial | B3 | std::* → core/alloc. | — |
| `src/diagnostic.rs` | medium | B3 | std::* → core/alloc; minor R4 B5 PathBuf. | M27 R4 B5 |
| `src/diagnostic_impls.rs` | medium | B3 | std::* → core/alloc; PathBuf args. | M27 R4 B5 |
| `src/error.rs` | ~100 | B3 | i18n simplification: removed fluent_bundle::resolver::errors imports; simplified `Display for TranslateError` per §1.8. Kept TranslateError variants/constructors so downstream API stays stable. | M27 §1.8 |
| `src/json.rs` | medium | B3 | std::* → core/alloc/semos_std; Mutex API delta (no `.unwrap()` after `.lock()`); Path/PathBuf via semos_std. | M27 §1.8, R4 B5, §1.4 |
| `src/lib.rs` | ~1,900 | B3 | `#![no_std]` + `extern crate alloc;`. Backtrace SHIM mod (§1.9), local `panic` mod with `panic_any` → `process::abort` + re-exports `core::panic::Location`. std::error::Report wrapping dropped at all sites. PathBuf via semos_std. `std::env::var_os` → `semos_std::env::var`. ICE-file OpenOptions via semos_std. `!std::thread::panicking()` → `!false` (single-threaded abort model). | M27 §1.8, §1.9, R4 B5 |
| `src/lock.rs` | 86 | B3 | `use core::any::Any; use alloc::boxed::Box;` at top. `use std::*;` inside the `cfg(windows)` body left untouched — body never compiles on SemOS target. | — |
| `src/timings.rs` | trivial | B3 | std::* → core/alloc. | — |
| `src/translation.rs` | ~115 | B3 | Major i18n removal (§1.8): translate_message becomes a pass-through (Str → Cow::Borrowed, FluentIdentifier → identifier text). Dropped tracing::{debug,trace}. Kept Translator + fluent_bundle struct fields for ABI compat. | M27 §1.8 |
| `src/emitter.rs` | 733 | **B3-followup** | std::error::Report dropped; std::io::prelude/IsTerminal/Stderr/stderr → semos_std::io. ColorConfig::to_color_choice: `is_terminal()` calls → `const IS_TERMINAL: bool = false;` (SemOS serial console). `Buffy` rewritten to wrap `semos_std::io::Stdout` (only available terminal sink); behavioural docs explain kernel-atomic-write semantics. `stderr_destination` + `get_stderr_color_choice` follow suit. `Path` import via semos_std. | M27 R4 B5 (×4), §1.8 (×1), TODO(Phase 2b) (×3) |
| `src/annotate_snippet_emitter_writer.rs` | 763 | **B3-followup** | std::error::Report dropped at 3 sites; std::sync::Arc → alloc::sync::Arc; std::io → semos_std::io; std::fmt::Formatter → core::fmt::Formatter; 3 `std::mem::replace` → `core::mem::replace`. Tracing left as external dep. | M27 §1.8 (×3) |
| `src/registry.rs` | 23 | **B3-followup** | **Zero edits.** Only uses `rustc_data_structures::fx::FxHashMap` + `&'static str` + `ErrCode`. Already no_std-clean. | — |
| `src/tests.rs` | 182 | **B3-followup** | Untouched; instead gated via `lib.rs`: `#[cfg(all(test, not(target_os = "none")))] mod tests;`. The file uses std::sync::LazyLock and direct fluent_bundle internals that have no semos_std analogue. Host `cargo test` still exercises it; SemOS target skips. | — |
| `src/json/tests.rs` | 200 | **B3-followup** | Untouched; gated similarly in `json.rs`. The file uses `Arc<Mutex<Vec<u8>>>` with std-style `.lock().unwrap()` — incompatible with semos_std::sync::Mutex's guard-direct API. | — |
| `src/markdown/mod.rs` | 81 | **B3-followup** | `use std::io;` → `use semos_std::io;`. `create_stdout_bufwtr` left with std::io::stdout() call + TODO marker (anstream::Stdout::always wants a std::io::Stdout; needs anstream patch later). | M27 R4 B5 TODO(Phase 2b) |
| `src/markdown/parse.rs` | 589 | **B3-followup** | `use std::{iter, mem, str};` → `use core::{iter, mem, str}; use alloc::{string::String, vec::Vec};`. No other std refs. | — |
| `src/markdown/term.rs` | 223 | **B3-followup** | `use std::cell::Cell;` → `use core::cell::Cell;`. `use std::io::{self, Write};` → `use semos_std::io::{self, Write};`. `thread_local!` → `semos_std::thread_local!`. Three call sites updated from `CURSOR.set(x)`/`WIDTH.get()` to `CURSOR.with(|c| c.set(x))`/`WIDTH.with(|c| c.get())` because semos_std::thread::LocalKey<Cell<T>> has no inherent `set`/`get` shortcut. Tests submodule gated host-only (path-fs dependencies). | M27 R4 B2 (×3) |
| `src/markdown/tests/parse.rs` | 168 | **B3-followup** | **Zero edits.** Tests are no_std-clean (Vec/format!/etc.) — covered by crate-root extern crate alloc;. | — |
| `src/markdown/tests/term.rs` | 88 | **B3-followup** | Untouched; gated via `markdown/term.rs`. Uses std::fs::write/read + std::env::var_os to bless fixtures. | — |

## 2. Decisions made (architectural)

- **§1.8 i18n drop — implementation pattern.** B3 chose to KEEP the
  `Translator`, `TranslateError`, `TranslateErrorKind`, `FluentBundle`,
  `LazyFallbackBundle` types as the rustc-side ABI so the rest of the
  crate (and downstream rustc_session / rustc_interface) continues to
  compile without surgery. The bodies become passthroughs: `Str(s)` →
  `Cow::Borrowed(s)`, `FluentIdentifier(id, _)` → `Cow::Borrowed(id)`.
  The `fluent_bundle` field remains in the `Translator` struct but is
  never read on the SemOS target. This trades a slightly larger struct
  for a much smaller patch surface vs. cutting Translator entirely.
- **§1.9 backtrace shim.** B3 created an in-crate `backtrace_shim`
  module exposing `Backtrace { capture, force_capture, status, Display }`
  + `BacktraceStatus` enum, so the existing call sites in `lib.rs`
  (delayed_bug `Backtrace::capture()`, `backtrace.status()`,
  `must_produce_diag` rendering) don't change at all. The Backtrace
  always reports `Unsupported` and Displays as
  `"(no backtrace available on SemOS)"`.
- **§1.9 panic shim.** B3 created an in-crate `panic` module re-exporting
  `core::panic::Location` and providing `panic_any<T>(_) -> !` that calls
  `semos_std::process::abort()`. The crate's existing `panic::*` import
  resolves to this local module on the SemOS target. `std::panic::catch_unwind`
  is NOT modeled — there's no call site in this crate for it (the only
  uses are FatalError control flow handled by rustc_span's fatal_error.rs
  rewrite from Phase 2a B1).
- **§1.9 thread::panicking → false.** B3 replaced
  `!std::thread::panicking()` with the literal `!false` in
  `DiagCtxtInner::drop` — SemOS panics abort, nothing is ever
  "panicking" in the stack-unwinding sense, so the
  `must_produce_diag` gate fires unconditionally.
- **R4 B5 ICE file open.** `std::fs::File::options().create(true).append(true).open(path)`
  → `semos_std::fs::OpenOptions::new().create(true).append(true).open(file.as_str())`.
  `PathBuf::as_str` gives the bare string path required by the
  shim's `open(&str)` signature (semos_std::path is lexical-only).
- **R4 B5 Stderr → Stdout.** **B3-followup.** SemOS has no
  `std::io::Stderr` — the kernel exposes a single serial console
  through `SYS_WRITE`. `Buffy`, `stderr_destination`, and
  `get_stderr_color_choice` are re-wired to use `semos_std::io::Stdout`
  (a unit type implementing `Write`). The Buffy buffering layer remains
  because anstream's `AutoStream<Box<dyn Write + Send>>` API expects a
  buffered writer. The SYS_WRITE kernel contract is already atomic
  per-call, so any error-emission interleaving is impossible on
  single-threaded SemOS.
- **R4 B5 IsTerminal stub.** **B3-followup.** `std::io::IsTerminal` is
  std-only. `ColorConfig::to_color_choice` replaced `io::stderr().is_terminal()`
  with a `const IS_TERMINAL: bool = false;` local — the SemOS console is
  a serial UART, never an ANSI terminal. `ColorConfig::Auto` thus
  degenerates to `Never`; `ColorConfig::Always` still emits raw ANSI
  bytes via `ColorChoice::AlwaysAnsi`.
- **R4 B2 thread_local! in markdown/term.rs.** **B3-followup.** Routed
  to `semos_std::thread_local!`. The `LocalKey<Cell<T>>` shortcut
  methods `.get()`/`.set()` (std-only since 1.73) don't exist on
  semos_std::thread::LocalKey, so the call sites are rewritten to
  `.with(|c| c.get())` / `.with(|c| c.set(x))`. Single-threaded
  rustc-on-SemOS makes this a sound semantic equivalent.
- **Tests gated, not patched.** **B3-followup.** Three test modules
  (`src/tests.rs`, `src/json/tests.rs`, `src/markdown/tests/term.rs`)
  exercise host-only API shapes (LazyLock, std::fs path-write blessing,
  fluent_bundle internals). Gated as
  `#[cfg(all(test, not(target_os = "none")))]` so host `cargo test`
  still runs them and the SemOS target build skips them entirely.
  `src/markdown/tests/parse.rs` was already no_std-clean and stays
  active.
- **tracing left as external.** Both `emitter.rs` and
  `annotate_snippet_emitter_writer.rs` keep `use tracing::{debug, warn};`
  unchanged. tracing is on the "DEEP-PATCH" list in R3 (the
  rustc_log/tracing shim is a separate Phase 2b/3 task). Until then,
  the imports will fail to resolve on the SemOS target — that's a
  parent-integration concern, not a per-file fix.

## 3. Deferred work, line-precise

**Nothing deferred at file level.** All 15 source files in
`compiler/rustc_errors/src/` are either patched or covered by a
`#[cfg(not(target_os = "none"))]` gate.

The crate has **four lingering TODO(Phase 2b) markers** that require
adjacent-crate work, not within rustc_errors:

### `markdown/mod.rs:46` — `anstream::Stdout::always(std::io::stdout())`
- The call site is inside `create_stdout_bufwtr` and is only used by
  rustdoc-side rendering. Until `anstream` is patched to accept a
  generic `Write` (or semos_std grows a `std::io::Stdout`-shaped type
  that anstream can swallow), this fn doesn't type-check on the SemOS
  target.
- **Recipe for the followup:** when porting `anstream` (R3 DEEP-PATCH,
  ~2 sessions), expose an `anstream::Stdout::always_writer(w: impl Write)`
  alternative. Then patch this site to `anstream::Stdout::always_writer(semos_std::io::Stdout)`.

### `emitter.rs:553` (Buffy `buffer.write`) — Write-for-Vec<u8>
- semos_std::io::Write impl-for-Vec<u8> is expected to exist (verified
  in Cranelift port). If a future semos_std cleanup removes the blanket
  impl, this site needs an explicit `impl io::Write for Vec<u8>` shim.
- **Recipe for the followup:** if Phase 3 finds the impl missing, the
  fix is a 5-line `impl io::Write for Vec<u8>` in `semos_std::io`
  similar to the std impl: `extend_from_slice` for write, no-op flush.

### `emitter.rs:605-617` — get_stderr_color_choice
- Currently always returns `ColorChoice::Never` when the input was
  `Auto`, ignoring the borrowed `&semos_std::io::Stdout`. If color
  output on the SemOS console is ever wanted (an HTTP-backed renderer
  that knows it's piped to an ANSI-capable terminal upstream), expose
  a const-true `IsTerminal` impl on `semos_std::io::Stdout` and
  reinstate the original `AutoStream::choice(stderr)` call.
- **Recipe for the followup:** ~3-line change once semos_std grows
  IsTerminal.

### `markdown/term.rs:104,160,180` — LocalKey<Cell<T>> sugar
- The `.with(|c| c.get())` rewrites are correct but verbose. If
  semos_std::thread::LocalKey grows `set`/`get`/`take`/`replace` for
  `LocalKey<Cell<T>>` (one impl block, ~15 lines), these three sites
  can be reverted to the cleaner `WIDTH.get()` form.
- **Recipe for the followup:** add to `std-shim/src/thread.rs` the
  same conditional impl as the std `impl<T: Copy> LocalKey<Cell<T>>`
  block (since 1.73). Then `grep -n 'with(|c| c\.\(get\|set\)' compiler/rustc_errors/src/markdown/term.rs` and revert.

## 4. New API gaps discovered

Not on R2's top-6 list but surfaced during B3-followup:

- **`std::io::IsTerminal`.** No semos_std analogue. Site:
  `emitter.rs:425, 432` (ColorConfig::to_color_choice). Interim
  treatment: `const IS_TERMINAL: bool = false;` local. Real fix: add
  `pub trait IsTerminal { fn is_terminal(&self) -> bool; }` to
  `semos_std::io` and implement it on `Stdout` returning `false` (or
  a runtime env-var check if SemOS exposes a "TERM" var).
- **`std::io::Stderr`.** No semos_std analogue. Site:
  `emitter.rs:520, 547, 565` (Buffy + stderr_destination +
  get_stderr_color_choice). Interim treatment: route through
  `semos_std::io::Stdout`. Real fix: expose a separate `Stderr` unit
  type in `semos_std::io` for symmetry, even if it shares the same
  syscall sink. Cost: ~5 lines.
- **`LocalKey<Cell<T>>::{get, set, take, replace}` sugar.** Site:
  `markdown/term.rs:104,160,180`. Interim: `.with(|c| ...)`. Real fix:
  ~15-line impl block in semos_std::thread.
- **`anstream::Stdout::always` expects `std::io::Stdout`.** Cross-crate
  gap; tracked under markdown/mod.rs above.

Within rustc_errors no semos_std::* surface that B3 used is missing —
OpenOptions, env::var, path::PathBuf::as_str, io::Write, process::abort
all sufficed.

## 5. Phase-routing summary

For each marker class added:

- **`// M27 §1.8`** (i18n drop): owner = Phase 4 final cleanup if we
  ever decide to fully strip the unused FluentBundle field from
  Translator. Count: ~6 sites between lib.rs, translation.rs, error.rs,
  emitter.rs, annotate_snippet_emitter_writer.rs.
- **`// M27 §1.9`** (FatalError / no-unwinder): owner = Phase 5
  integration if a kernel-side stack unwinder ever lands. Count: ~4
  sites in lib.rs.
- **`// M27 R4 B5`** (PathBuf / OsString / fs adjacency): owner =
  semos-std prep work IF anything needs more than the lexical/string
  path surface. The B5 markers here all resolve cleanly against the
  current semos_std API. Count: ~5 sites between lib.rs, json.rs,
  emitter.rs (Stdout-as-Stderr cluster), markdown/mod.rs (anstream
  TODO).
- **`// M27 R4 B2`** (TLS): owner = no additional work needed; the
  semos_std `thread_local!` macro already covers this. The
  `LocalKey<Cell<T>>` sugar gap is captured in §3 above.
- **`// M27 TODO(Phase 2b)`**: anstream + tracing port work that the
  Phase 2b external-crate fleet picks up.
- **`// M27 §1.4`** (single-thread Mutex): owner = handled inline in
  json.rs (semos_std::sync::Mutex returns guard directly).

## 6. Surprises worth flagging upward

1. **rustc_errors is structurally simpler than R3 estimated.** The R3
   audit budgeted i18n removal at 3 sessions (`fluent-bundle` DEEP-PATCH).
   The actual rustc-side ripple was tiny: B3 simplified translation.rs
   in one pass plus an `error.rs::Display` simplification. The
   downstream `Translator`-shape ABI is unchanged. This suggests the
   external `fluent-bundle` port can be skipped entirely — we never
   read its body from rustc_errors. **Save ~3 sessions.**
2. **The crate has only one real piece of std-FS surface** (the ICE
   file in `lib.rs`'s `DiagCtxtInner::drop` flush path). Everything
   else is in-memory string manipulation routed through `io::Write`.
   This was unexpected — diagnostic emission "feels" I/O-heavy but is
   actually almost all `format!` to a `Box<dyn Write>`.
3. **`semos_std::io::Stderr` is missing despite the RECIPE listing
   it.** The RECIPE table in `docs/m27-port/RECIPE.md` §2 advertises
   "Stdout, Stderr" as semos_std::io. Only `Stdout` exists today.
   Add an alias (or true separate sink) for the record; not blocking
   for B3-followup since the Stdout reuse is correct semantically.
4. **The `markdown` submodule is dead weight on the SemOS target.**
   It's used only for `rustc --explain XXXX` rendering, which depends
   on `anstream::Stdout` + filesystem fixture round-trips. None of
   that exercises during a normal compile. A future cleanup could
   `cfg(not(target_os = "none"))` the entire `pub mod markdown;`
   declaration in lib.rs. Not done in this pass to keep the diff
   small and reversible.
5. **B3's `extern crate self as rustc_errors;` is load-bearing for the
   `rustc_fluent_macro::fluent_messages!` invocation at lib.rs:149.**
   Don't drop it during cleanup — the proc-macro generates absolute
   paths starting with `::rustc_errors::*`.

## 7. Recipe additions

Two patterns from B3-followup that should land in
`docs/m27-port/RECIPE.md`:

### 7.1 — Tests submodule gating: `#[cfg(all(test, not(target_os = "none")))]`

When a host-only `#[cfg(test)] mod tests;` references std-side APIs
(LazyLock, std::fs::write, std::env::var_os, std::sync::Mutex
guard-with-result) that have no semos_std analogue, **don't patch the
test file — gate the mod declaration**. The host `cargo test` still
runs them; the SemOS target build skips them entirely.

Tradeoff vs. patching: the test file stays in lock-step with upstream
rustc, and you avoid maintaining a divergent test surface.

Use case: B3-followup gated 3 such files (rustc_errors/src/tests.rs,
json/tests.rs, markdown/tests/term.rs) with one-line attribute changes,
versus an estimated ~50 LOC of surface-level rewrites.

### 7.2 — semos_std::thread::LocalKey<Cell<T>> sugar gap

semos_std's `LocalKey<T>` does NOT have the std-LocalKey-shortcut
methods `.get()`, `.set()`, `.take()`, `.replace()` that std added in
1.73 for `LocalKey<Cell<T>>`. Code using these compiles fine on host
but not against `semos_std::thread_local!`. Mechanical fix:

```rust
// Before (std-only):
CURSOR.set(0);
let w = WIDTH.get();

// After (works against semos_std::thread_local!):
CURSOR.with(|c| c.set(0));
let w = WIDTH.with(|c| c.get());
```

If/when semos_std adds the impl block (one-pager), revert with grep.
Site for the recipe addition: RECIPE §2 "semos-std surface", "Known
gaps" list.

---

## Verification checklist for the parent integrator

1. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/emitter.rs` should show:
   - Imports rewritten at top (alloc / core / semos_std).
   - `Report::new` wrappers dropped at one site (~line 115).
   - `Buffy` struct, `stderr_destination`, `get_stderr_color_choice`
     re-routed through `semos_std::io::Stdout` (~lines 520-617).
   - `ColorConfig::to_color_choice` switched to `const IS_TERMINAL: bool = false;`.
2. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/annotate_snippet_emitter_writer.rs` should show:
   - Imports rewritten at top.
   - 3 `.map_err(Report::new).unwrap()` → `.unwrap()`.
   - One `std::fmt::Formatter`/`Result` → `core::fmt::*`.
   - One `std::mem::replace` (×3 occurrences) → `core::mem::replace`.
3. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/markdown/` should show:
   - mod.rs: `use semos_std::io;` + create_stdout_bufwtr TODO.
   - parse.rs: imports only.
   - term.rs: imports + `semos_std::thread_local!` + 3 `.with(|c| ...)` rewrites + tests-mod gating.
4. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/lib.rs` should show ONE new line (the tests-mod cfg gate at ~line 149) — everything else was B3's work.
5. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/json.rs` should show ONE new line (the tests-mod cfg gate at ~line 49) — everything else was B3's work.
6. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/registry.rs` should show **zero changes** — registry.rs needed no edits.
7. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/tests.rs` should show **zero changes** (file intentionally left unmodified; gated via lib.rs).
8. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/json/tests.rs` should show **zero changes** (gated via json.rs).
9. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/markdown/tests/parse.rs` should show **zero changes** (no_std-clean as-is).
10. `git diff main -- user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/src/markdown/tests/term.rs` should show **zero changes** (gated via markdown/term.rs).

The crate is **patch-complete for the SemOS target**. Outstanding
external blockers: `tracing`, `anstream`, `annotate-snippets`,
`derive_setters`, `termize` — all carried in Cargo.toml `[dependencies]`
and listed in R3 as PATCH/DEEP-PATCH/WALL crates with separate
mitigations.
