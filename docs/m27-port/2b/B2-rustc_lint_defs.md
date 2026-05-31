# B2 — rustc_lint_defs

**Date:** 2026-05-30
**Phase:** 2b
**Assigned crates / files:** `compiler/rustc_lint_defs/` (Cargo.toml,
src/lib.rs, src/builtin.rs)
**Status:** COMPLETE
**Token cost (self-report):** ~20k tokens / ~30 tool uses / ~12 min
**Source LOC patched:** 1042 (lib.rs) + 5409 (builtin.rs) = 6451 LOC.
Actual diff is ~12 lines in lib.rs + 3 lines in Cargo.toml. builtin.rs
required zero edits.

## 1. Per-file diff summary

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `Cargo.toml` | 17 → 21 | `[workspace] members = []` header; added `semos_std = { path = "../../../../std-shim" }` dep for the lone PathBuf site | `# M27 R4 B5` comment in deps |
| `src/lib.rs` | 1042 | `#![no_std]` + `#[macro_use] extern crate alloc;` prelude; explicit `use alloc::{borrow::Cow, string::{String, ToString}, vec::Vec};`; `use core::fmt::Display;`; 7 `std::*` → `core::*` substitutions; 1 `std::path::PathBuf` → `semos_std::path::PathBuf` (trait impl signature, line 308) | None — recipe-only |
| `src/builtin.rs` | 5409 | None — file is entirely `declare_lint!` + `declare_lint_pass!` macro invocations + doc comments. `std::*` mentions in this file are 100% inside `///` doc comments (HashMap example code, ptr::null, env::var examples — 14 sites). The crate-root `#![no_std]` covers it. | None |

### Detailed substitutions in lib.rs

- L1–2 (header): `use std::borrow::Cow; use std::fmt::Display;` →
  `#![no_std]` + `#[macro_use] extern crate alloc;` +
  `use alloc::borrow::Cow; use alloc::string::{String, ToString};
  use alloc::vec::Vec; use core::fmt::Display;`.
  `ToString` is needed for `release_fcw.to_string()` / `edition_fcw.to_string()`
  (lib.rs:530, 534) since `Display::to_string` lives on `ToString`.
  `String` and `Vec` need explicit prelude imports under no_std.
- L301 (in `impl IntoDiagArg for Level`):
  `_: &mut Option<std::path::PathBuf>` →
  `_: &mut Option<semos_std::path::PathBuf>`. The trait signature is
  owned by `rustc_error_messages`; this impl must match whichever
  PathBuf type that crate ends up using. semos_std::path::PathBuf is
  the canonical choice per RECIPE §1.6 and A2/A3 precedent.
- L534, L541 (two `Display::fmt` impls): `std::fmt::Formatter` /
  `std::fmt::Result` → `core::fmt::*`. Used `replace_all` since both
  signatures are identical.
- L595 (PartialEq for LintId): `std::ptr::eq` → `core::ptr::eq`.
- L601-602 (`impl std::hash::Hash for LintId`):
  `std::hash::Hash` → `core::hash::Hash`,
  `std::hash::Hasher` → `core::hash::Hasher`.
- L643 (`impl StableCompare for LintId`): `std::cmp::Ordering` →
  `core::cmp::Ordering`.

### Macros (all `#[macro_export]`, expand in downstream crates)

`pluralize!`, `declare_lint!`, `declare_tool_lint!`, `impl_lint_pass!`,
`declare_lint_pass!`, `fcw!`. None emit `::std::*` tokens — checked via
`grep -n "::std" lib.rs` (zero matches). `impl_lint_pass!` emits a
`vec![...]` call; downstream crates with `#![no_std] #[macro_use] extern
crate alloc;` get the `vec!` macro in scope. No macro-body patches
needed.

## 2. Decisions made (architectural)

- **semos_std as a direct dep for PathBuf in the IntoDiagArg impl.**
  Considered three options:
  1. Leave a `// M27 R4 B5 TODO` marker without substituting → trait
     impl wouldn't typecheck against `rustc_error_messages`'s
     `into_diag_arg(&mut Option<PathBuf>)` signature.
  2. Use `core::ptr::null::<()>` as a placeholder → breaks the impl.
  3. Add semos_std as a dep + use `semos_std::path::PathBuf` directly.
  Chose (3) because the trait method signature must match the trait
  declaration site (in rustc_error_messages). Per RECIPE §1.6, basic
  PathBuf uses substitute directly. This is the simplest of all basic
  uses — a single type position in an unused parameter. Parent may
  prefer to route through a `rustc_error_messages` re-export; the
  Cargo.toml comment flags this for review.

## 3. Deferred work, line-precise

Nothing deferred. The crate has only two source files; both reach a
compilable state under `#![no_std]` after the substitutions above.

## 4. New API gaps discovered

None new — semos_std::path::PathBuf is already on the surface inventory
in RECIPE §2. The lone PathBuf use here is a phantom (type-only,
unused-parameter) so it doesn't exercise any of the gap-listed
operations (Cow<Path>, components, strip_prefix, etc.).

## 5. Phase-routing summary

- **`// M27 R4 B5`** (in Cargo.toml only): one site — the semos_std dep
  comment. Parent (Phase 5 integration) should verify the path target
  and decide whether to consolidate through a `rustc_error_messages`
  re-export.

No other markers added in source. This crate is mostly pure data
(enums, structs, trait impls) — no IO/FS/path-walking surface.

## 6. Surprises worth flagging upward

- **builtin.rs is a pure declarative-macro file.** 5409 LOC, zero
  required edits. Every `std::*` reference is inside `///` doc-comment
  fenced examples. The `target/LOC ratio` for this crate is therefore
  dominated by lib.rs's small surface; effective LOC patched is ~1042,
  not 6451. The 7-reverse-dep blast radius is on enums + macros, not
  code.
- **The `IntoDiagArg` impl signature couples this crate to whatever
  PathBuf type rustc_error_messages chooses.** This is a B1/B2-style
  hidden dep: lint_defs has no other path-handling surface but ships
  this trait impl. Phase 5 integration should sanity-check that the
  PathBuf paths line up after rustc_error_messages also lands.
- **`Level::from_str` uses `"expect" | _ => None` arm.** Pre-existing
  upstream code, but mildly surprising — the `"expect" | _ => None`
  match arm is a clippy-worthy redundant-pattern. Not patching;
  upstream behavior preserved verbatim.

## 7. Recipe additions

No new recipe shape discovered. This crate is the canonical "small
mechanical port" — exactly the A4/A5 shape with no `cfg(target_os =
"none")` split, no host/target body separation, no `// M27 R4 Bx`
markers besides the inevitable PathBuf-in-trait-impl one.

Worth noting in RECIPE if not already: **declarative-macro-heavy
crates (5000+ LOC of `declare_lint!` / `declare_X!` invocations) may
need zero source edits in the data file.** The crate-root `#![no_std]`
+ alloc prelude is sufficient when the file is pure macro invocations
+ doc comments. This makes per-LOC token-cost estimates misleading for
this class of crate: count the *logic* LOC, not the *declaration* LOC.
