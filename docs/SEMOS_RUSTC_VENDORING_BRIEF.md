# semos-rustc Vendoring Brief

**Date:** 2026-07-18 (written retroactively per the 2026-07-17 code review)
**Status:** active record of upstream state for the vendored rustc tree
**Companion to:** `docs/VENDORING.md` (index), `docs/M27_RUSTC_PORT_PLAN.md`,
`docs/M27_DISK_SYSROOT_DESIGN.md`, `docs/CRANELIFT_VENDORING_BRIEF.md`

The 2026-07-17 external review flagged `user-programs/semos-rustc`
(~1.13M LoC) as the largest vendored tree with no upstream record. This
brief covers it.

---

## 1. What is vendored, and where

`user-programs/semos-rustc/` is a **workspace root** that owns an entire
vendored rustc source tree and builds it into a single **Ring-3 ELF that
runs rustc on SemOS itself** (DEMO 80; the M27 milestone). Layout:

| Subtree | Contents | Notes |
|---------|----------|-------|
| `vendor-rustc-src/` | `rust-lang/rust` compiler tree (`compiler/rustc_*` crates) | Co-pinned to the project toolchain, **nightly-2026-02-01** |
| `vendor-externals/` | 27 support crates, incl. the Cranelift `no_std` port subset | See `CRANELIFT_VENDORING_BRIEF.md` + the in-tree `CRANELIFT_VENDOR_NOTES.md` |
| `src/`, `build.rs`, `link.ld` | The SemOS-side glue: driver, linker script (ET_EXEC at the SemOS user base), build wiring | First-party |
| `test-sources/` | Input programs for on-device compiles (`hello.rs`) | First-party |

Tree size ~1.3 GB (build artifacts included); the 2026-07-17 review
measured ~1.13M LoC of vendored source.

## 2. Upstream state

- **Source**: `github.com/rust-lang/rust`, matching **nightly-2026-02-01**
  (the same pin as the repo-root `rust-toolchain.toml`; the
  `semos-rustc/rust-toolchain.toml` re-pins it deliberately so the
  vendored compiler and the toolchain building it never drift apart —
  rustc internals only compile against their own nightly).
- **License**: MIT OR Apache-2.0 (standard rust-lang dual license).
- **Codegen backends**: LLVM and GCC backends, the LLVM C++ bindings,
  ICU data baking, and sanitizer support are **excluded from the
  workspace** (see the comment block in `semos-rustc/Cargo.toml`).
  `rustc_codegen_cranelift` is the only backend — it was **un-excluded**
  in Phase 5c Stage G so the on-device compiler emits code through the
  vendored Cranelift 0.122.0 line.

## 3. Local modifications

This is a **fork**, not a pristine vendor drop. The M27 port (see
`M27_RUSTC_PORT_PLAN.md` for the phase-by-phase history) carried:

- `no_std`/SemOS-platform patches across the `rustc_*` crates (arena /
  hashbrown / jobserver / fs / process shims against the SemOS syscall
  surface and semos-std).
- Workspace surgery: per-crate `[workspace] members = []` opt-outs,
  `resolver = "2"` at the root to stop host-only dep features leaking
  into the `x86_64-unknown-none` resolution (the iter 7b "tracing's std
  turns on workspace-wide" class).
- cg_clif's Cranelift dep repinned 0.127.0 → 0.122.0 (see the Cranelift
  brief).
- Per-crate patch headers were added during Phases 2–4; treat each
  crate's header comment as its patch log.

## 4. Update policy

1. **The rustc pin follows the repo toolchain pin.** Do not move one
   without the other: bump `rust-toolchain.toml` (root + semos-rustc)
   and re-vendor `vendor-rustc-src` in the same milestone.
2. A re-vendor is a **big-bang, milestone-gated** event: replay or
   re-derive the `no_std` patches crate-by-crate (Phase 2–4 patch
   headers are the checklist), then re-run the acceptance gates —
   `cargo check` for `x86_64-unknown-none`, a full image build, and the
   DEMO 80 on-device `hello.rs` compile + spawn smoke on metal or UEFI.
3. **Never bump Cranelift independently of cg_clif's declared line** —
   the two move together (see `CRANELIFT_VENDORING_BRIEF.md` §4).
4. Rustc security advisories rarely apply directly (we run the compiler
   as a tier-fenced Ring-3 tool, not as a network service); a miscompile
   fix relevant to Cranelift codegen *does* apply and follows §2.

## 5. Known gaps

- No recorded upstream **git SHA** for the nightly-2026-02-01 rustc
  snapshot — recoverable from the nightly tag, but write it down at the
  next re-vendor.
- The no_std patch set is documented in-plan (`M27_RUSTC_PORT_PLAN.md`)
  and in-crate, but not as standalone `.patch` files; a re-vendor is a
  manual replay.
- 1.3 GB of `target/` artifacts sit inside the tree boundary on dev
  machines — keep them out of any future packaging story.
