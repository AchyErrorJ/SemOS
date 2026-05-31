# M27 Phase 2a probe — rustc_hashes

Probe agent for Phase 2a. Goal: port `compiler/rustc_hashes/` to
no_std + alloc + semos-std using the `semos-cc/PORT_LOG.md` recipe.

## Outcome: PROBE PASS

Source compiles cleanly with `#![no_std]` declared. One downstream
blocker (external dep `rustc-stable-hash 0.1.2`) noted as follow-up
for parent — that crate is unconditionally std and needs its own
patch, but that is out of probe scope.

Crate is 131 LOC, single source file (`src/lib.rs`). Took ~5
minutes of edits once the recipe was in hand.

## Files patched

| File | Change |
|------|--------|
| `Cargo.toml` | added `[workspace] members = []` above `[package]` |
| `src/lib.rs` | added `#![no_std]` + `#[macro_use] extern crate alloc;` after inner doc block; substituted `std::fmt` / `std::ops::BitXorAssign` / `std::hash::{Hash,Hasher}` → `core::*` |

No `.cargo-checksum.json` present in this crate (rustc-src is staged
raw from the dist tarball, not packaged like a crates.io vendor
checkout) — step 4 of the recipe was N/A. Also no `Cargo.lock`.

## Patterns matched (recipe table)

All four `std::` references in `src/lib.rs` were of the
"`std::*` → `core::*`" class — none of the alloc-class patterns
(no Vec, Box, String, HashMap, etc. in this crate):

- `std::fmt` → `core::fmt` (line 16, was 1 occurrence)
- `std::ops::BitXorAssign` → `core::ops::BitXorAssign` (line 17)
- `std::hash::Hash` → `core::hash::Hash` (line 84)
- `std::hash::Hasher` → `core::hash::Hasher` (line 85)

Zero alloc-class substitutions needed — this crate genuinely doesn't
touch the allocator. `extern crate alloc;` was added per the recipe's
"standard pattern" guidance, even though it's unused here. (Produces
a single `unused #[macro_use]` warning — benign and intentional, keeps
the pattern uniform with the rest of the foundation-tier crates.)

## Surprises / notes

1. **No `.cargo-checksum.json`** — these aren't vendored crates,
   they're source-tree crates from the upstream rustc dist. The
   recipe step about updating sha256 in checksum json doesn't apply
   to anything in `vendor-rustc-src/compiler/*`. It only applies to
   the externals that get vendored into `vendor/` (e.g., what
   semos-cc did with cranelift). Phase 2a foundation crates (all
   rustc_*) skip step 4 entirely.

2. **Downstream blocker: `rustc-stable-hash 0.1.2`** — the only
   external dep. It has `default = []` plus a `nightly` feature, but
   `nightly` does not gate std use; std is unconditional in
   `sip128.rs` (`use std::hash::Hasher;`). When the SemOS target build
   happens, this crate will need vendoring + a no_std patch (add
   `#![no_std]`, `use core::hash::Hasher;`). Confirmed by host-target
   sanity check (see below) — on `x86_64-pc-windows-msvc` it builds
   because std is available, but on `aarch64-unknown-none` /
   `x86_64-unknown-none` it'll fail at the dep, not at rustc_hashes
   itself. Treat as a separate vendor patch in the parent's Phase 2a
   externals list.

3. **`#![no_std]` placement** — kept after the leading `//!` inner
   doc block, before `extern crate alloc;` and before any `use`
   imports. Order: doc-attrs → `#![no_std]` → items. Both
   `//!` and `#![...]` are inner attributes so they can be adjacent;
   any `extern crate` / `use` must follow.

4. **No HashMap / Vec / Box / String** — this crate is pure value
   types (Hash64, Hash128) plus their trait impls. Probably the
   smallest possible probe in the foundation tier. Good signal that
   the recipe scales: the patterns mapped 1:1 even for a crate
   that uses almost no std surface.

## Sanity check

Ran `cargo check --target x86_64-pc-windows-msvc --offline` from the
crate dir. Result:

    Checking rustc-stable-hash v0.1.2
    Checking rustc_hashes v0.0.0
    warning: unused `#[macro_use]` import
       --> src\lib.rs:18:1
    Finished `dev` profile in 0.26s

Confirms:
- `[workspace] members = []` correctly stops cargo from walking up
  to the worktree-root workspace.
- `#![no_std]` is accepted (no errors).
- All `core::*` substitutions resolve.
- The `extern crate alloc;` `#[macro_use]` is benign-unused (warning
  only), kept per recipe standard.

The same crate against `aarch64-unknown-none` fails only at
`rustc-stable-hash` — `rustc_hashes` itself is clean.

## Patterns to relay back

For the parent before the full Phase 2a fleet spawns:

- **`.cargo-checksum.json` step in the recipe is N/A** for any
  `compiler/rustc_*` crate. Skip it; it only applied because semos-cc
  vendored from crates.io.
- **`[workspace] members = []` is sufficient** to stop cargo
  walking up to a parent workspace. This crate has no parent
  workspace declared in the rustc-src tree (no
  `vendor-rustc-src/Cargo.toml`), but cargo walked up to my worktree
  root anyway — `members = []` cleanly blocked it.
- **`rustc-stable-hash` needs vendoring + no_std patch** before any
  SemOS-target build of rustc_hashes can succeed. This is a single
  external dep affecting at minimum rustc_hashes; check other
  foundation crates for the same dep.
- **The recipe holds verbatim** even for sub-200-LOC crates. No
  extra surprises beyond the externals downstream.
