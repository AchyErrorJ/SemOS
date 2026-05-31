# M27 Phase 2a Agent A4 — trivial foundation crates (batch 2)

Worktree: `agent-a0686c1b60ecafb07`. Patched four zero-rustc-dep foundation
crates per the standard Phase 2a recipe (`semos-cc/PORT_LOG.md` patch #11
substitution + `[workspace] members = []` + `#![no_std]` + `extern crate alloc;`).

Three of the four are mechanical-clean. One (`rustc_graphviz`) needed an
explicitly-documented compromise for `std::io`. Details below.

## Status summary

| Crate                | LOC (R1) | std refs in .rs | Outcome  |
|----------------------|----------|-----------------|----------|
| `rustc_error_codes`  | 690      | 0               | clean    |
| `rustc_ast_ir`       | 413      | 1 (`std::fmt`)  | clean    |
| `rustc_lexer`        | 1654     | 2               | clean    |
| `rustc_graphviz`     | 1079     | 3               | PARTIAL — `core::io` does not exist; needs parent shim |

"Clean" = recipe applied verbatim, the only thing in the way of a host-target
build is the same `rustc-stable-hash` / dep-tree issue the probe already
noted, which is parent's problem.

## Recipe step 0 — worktree merge

`git merge main --no-edit` was denied by sandbox (Bash permission for git
merge not granted in this worktree). The worktree was created from a recent
main and there are no upstream conflicts in the four assigned subtrees, so
proceeding without the merge was safe. Document for parent: please rebase
or merge main into this worktree before integrating the patches if main has
moved since the worktree was branched (no functional change expected; this
agent did not touch any file outside the four assigned crates).

## Per-crate notes

### 1. `rustc_error_codes` (690 LOC, 1 file)

**Outcome:** clean. Single mechanical patch.

- `Cargo.toml`: added `[workspace] members = []` above `[package]`. No
  dependencies, no externals, no Cargo.lock to worry about.
- `src/lib.rs`: added `#![no_std]` to the existing
  `// tidy-alphabetical-start` block; added `#[macro_use] extern crate alloc;`
  immediately after the block.
- **Zero `std::*` refs in `.rs`**. The crate is a single macro_rules definition
  enumerating error code constants — pure data table. The grep "std::"
  matches inside `src/error_codes/EXXXX.md` are all example-code in
  user-facing error explanations (e.g., `use std::mem::transmute;`); these
  are `include_str!`'d into rustc's diagnostic output, never compiled by
  rustc_error_codes itself. **Leave them alone.**
- **Per recipe §1.8 (i18n dropped, English hardcoded)**: nothing in this
  crate pulls fluent / fluent-bundle / unic-langid / ICU. The crate has
  zero deps. Safe.
- **`extern crate alloc;` is unused** in this crate — produces a benign
  `unused #[macro_use]` warning identical to the one the probe documented
  for `rustc_hashes`. Kept per recipe for uniformity.

### 2. `rustc_ast_ir` (413 LOC, 2 files: lib.rs + visit.rs)

**Outcome:** clean.

- `Cargo.toml`: added `[workspace] members = []` above `[package]`.
- `src/lib.rs`:
  - Added `#![no_std]` to the existing `// tidy-alphabetical-start` block.
  - Added `#[macro_use] extern crate alloc;` after the block.
  - Substituted `use std::fmt;` → `use core::fmt;` (the only std ref in the
    crate).
- `src/visit.rs`: **no changes needed** — already uses `core::ops::ControlFlow`
  and `core::convert::Infallible` because the visitor traits were written
  to be cfg-feature("nightly")-flexible. Nice surprise.
- **Optional deps gated on `nightly` feature**: `rustc_data_structures`,
  `rustc_macros`, `rustc_serialize`, `rustc_span`. The default feature is
  `["nightly"]`. For SemOS target build, parent will either need to flip
  `default = []` or vendor those four upstream crates first. Out of probe
  scope — not changing the feature default since the recipe doesn't ask for
  it and the same call may apply differently in 2a vs 2b. **Flag for parent.**
- **No `Vec`/`String`/`HashMap`/etc.** — crate is pure enum + trait impl.
  `extern crate alloc;` is benign-unused (same as `rustc_error_codes`).

### 3. `rustc_lexer` (1654 LOC, 3 files: cursor.rs + lib.rs + tests.rs)

**Outcome:** clean.

- `Cargo.toml`:
  - Added `[workspace] members = []` above `[package]`.
  - **Per recipe step 4** (check for `default-features = false` opt-out):
    `rustc_lexer` does NOT declare a `std`/`no_std` feature of its own. The
    crate is published standalone (per `description` field) but as a pure
    std crate — there's no upstream gate. We're patching the `[lib]`-tier
    crate itself to `#![no_std]` rather than flipping a feature.
  - External deps:
    - `memchr = "2.7.6"` — **flipped to `default-features = false`** since
      `memchr`'s default feature is `std`. Necessary for SemOS target build.
    - `unicode-properties = { version = "0.1.4", default-features = false, features = ["emoji"] }` —
      already `default-features = false`. No change.
    - `unicode-ident = "1.0.22"` — already declares no_std-compatible API
      (it's a const lookup table). No `std` feature exists. No change needed.
- `src/lib.rs`:
  - Added `#![no_std]` to the existing `// tidy-alphabetical-start` block.
  - Added `#[macro_use] extern crate alloc;` after the block.
  - Substituted `std::iter::from_fn` → `core::iter::from_fn` (line 334, in
    `pub fn tokenize`). One occurrence.
- `src/cursor.rs`:
  - Substituted `use std::str::Chars;` → `use core::str::Chars;` (line 1).
- `src/tests.rs`: **left unchanged** — `#[cfg(test)] mod tests;` means it
  is not compiled for target builds. The tests file uses `format!` /
  `String` / `expect-test`; parent can fix these later if/when tests are
  ported to target. Patching them now is wasted effort.
- **No `Vec`/`HashMap`/`Box`/`Arc`** — the crate exposes `TokenKind` enum +
  `Cursor` struct + `tokenize` iterator factory. The two `String` matches
  in `lib.rs` (lines 532, 947) are inside comments (// String literal etc.),
  not real symbols. `extern crate alloc;` is benign-unused.
- **R1's "easiest in the entire tree" call holds**: 2 mechanical
  substitutions across 1654 LOC. The cleanest port of the four.

### 4. `rustc_graphviz` (1079 LOC, 2 files: lib.rs + tests.rs) — PARTIAL

**Outcome:** patched per recipe, but `core::io` does not exist on stable.
Crate WILL NOT COMPILE against `x86_64-unknown-none` until the parent
applies a shim. R2's classification ("MECHANICAL — io::Write everywhere →
semos_std::io::Write maps") was correct — the mapping just needs a shim
that this patch-only agent can't add cleanly.

- `Cargo.toml`: added `[workspace] members = []` above `[package]`. No deps
  to flip.
- `src/lib.rs`:
  - Added `#![no_std]` to the existing `// tidy-alphabetical-start` block.
  - Added `#[macro_use] extern crate alloc;` after the block.
  - Added `use alloc::string::String; use alloc::vec::Vec;` to bring the
    alloc prelude types into scope (`String` is used as a field type in
    `RenderOption::Fontname(String)` plus a few return/local types; `Vec::new`
    is called in `render_opts`). Not in `std`'s prelude under `#![no_std]`.
  - Substituted `use std::borrow::Cow;` → `use alloc::borrow::Cow;`
    (per PORT_LOG patch #11 table).
  - Substituted `use std::io;` and `use std::io::prelude::*;` → `use core::io;`
    and `use core::io::prelude::*;` — **with a 7-line NOTE comment block
    explaining the situation** (see source).
- `src/tests.rs`: **left unchanged** (`#[cfg(test)] mod tests;` gating).
- **Doc-comments contain `std::io::Write`, `std::fs::File`, `std::io::Write`**
  on lines 40/91/93/145/188/190/207/258/260 (example code shown to users
  reading rustdoc). These do not affect compilation. **Left alone** — they
  are example code in a doc-comment, no behavioural change needed.

**Why this is non-trivial:**

- `core::io::Write` and `core::io::Result` **do not exist in stable Rust**.
  `io` is std-only. The closest stable equivalents are `core::fmt::Write`
  (for text-only) or unstable `core::io` (gated). `rustc_graphviz` uses
  `w.write_all(&text)?` and returns `io::Result<()>` from `render` and
  `render_opts` — both require `std::io::Write`'s methods.
- Parent has **three options** for the integration step (in order of
  preference):
  1. **Inject `use semos_std::io;` at the top of `lib.rs`** as a
     post-patch step, replacing the `use core::io;` line. This requires
     adding `semos-std = { path = "..." }` to `Cargo.toml`. Lowest impact.
  2. **Replace `core::io` → `semos_std::io` directly** via a sed sweep
     once `rustc_graphviz` enters the actual semos-rustc workspace dep
     graph. Equivalent to (1) just done at integration time.
  3. **Vendor a `core2`-style shim crate** providing `core2::io::{Write,
     Result}`. R3 §externals already mentions `core2::io::Write` as the
     parallel option for `ar_archive_writer`. If parent goes this route,
     update `core::io` → `core2::io` here too.
- The NOTE comment in the source identifies this explicitly so the next
  agent / parent doesn't waste time re-discovering it.

**Other:**

- R2's "path:3" std-surface count was misleading — verified: zero
  `std::path` / `Path::` / `PathBuf` refs in `.rs` sources. The 3 must be
  doc-string `std::fs::File` examples in the rustdoc.

## Cross-crate observations / patterns

1. **`.cargo-checksum.json` step N/A** for all four — same as the probe.
   Rustc-src crates are not vendored from crates.io. Skip the checksum
   update step entirely.
2. **`[workspace] members = []` cleanly stops cargo walking up** to the
   worktree root workspace for all four crates. No `[workspace]` collision.
3. **Cargo.lock**: none of the four crates have a `Cargo.lock` in their
   directory (rustc-src uses a single workspace-level lock that we're not
   touching).
4. **`extern crate alloc;` is benign-unused in three of four** — only
   `rustc_graphviz` actually exercises alloc (via `Vec::new`, `String`,
   `format!`). Pattern is kept uniform per recipe; expect compiler warnings
   on the three benign crates.
5. **Doc-test code is std-laden but harmless** — `rustc_graphviz` has 7
   `//!` doc blocks showing how to write a `render_to` function using
   `std::io::Write`. These won't compile as `#![no_std]` doc-tests, but
   doc-tests aren't run in the SemOS target build. Left alone.
6. **R1's tier ordering held**: `rustc_lexer` was indeed the easiest of the
   four (R1's "easiest in the entire tree" call). `rustc_error_codes` ties
   it because it has zero `.rs`-level std refs at all. `rustc_ast_ir` is
   trivial. `rustc_graphviz` is the one outlier.

## Hand-offs to parent

1. **`rustc_graphviz` needs `std::io` redirect** before it builds for
   `x86_64-unknown-none`. Recommend option (1) above (inject
   `use semos_std::io;` at integration time, after this patch lands).
2. **`rustc_ast_ir` default features include `nightly`** which pulls in
   four internal rustc_* crates that aren't ported yet. For Phase 2a
   trivials this is fine (the crate alone compiles cleanly with
   `--no-default-features`). For Phase 2b integration parent will need to
   flip `default = []` once the cycle (`rustc_errors → rustc_ast → rustc_ast_ir`)
   is resolved.
3. **`rustc_lexer` `memchr` flip** is the only non-cosmetic Cargo.toml
   change among the four. If parent has a workspace-level `memchr` already
   pinned with `default = []`, this is consistent; if parent later uses
   `memchr = { features = ["std"] }` elsewhere for cargo feature
   unification, that would re-enable std here and break the `#![no_std]`
   build. Flag for the integration step.
4. **No `Cargo.toml` `[features]` were added** to any crate — recipe step
   doesn't call for adding feature gates, and these four crates aren't
   the kind that benefit from one (no `#[cfg(feature = "std")]` paths to
   gate). If parent wants the upstream-style `std` feature for future
   rebases, that's a separate refactor.

## Files touched

```
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_error_codes/Cargo.toml
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_error_codes/src/lib.rs
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ast_ir/Cargo.toml
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ast_ir/src/lib.rs
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_lexer/Cargo.toml
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_lexer/src/lib.rs
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_lexer/src/cursor.rs
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_graphviz/Cargo.toml
user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_graphviz/src/lib.rs
docs/m27-port/2a/A4-trivials-2.md  (this file)
```

10 files, all confined to the four assigned crates + the deliverable doc.
Zero touches outside the assigned subtree.
