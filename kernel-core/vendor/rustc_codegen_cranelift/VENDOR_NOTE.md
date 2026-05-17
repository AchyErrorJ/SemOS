# Vendored rustc_codegen_cranelift (Phase 14 prep — SOURCES NOT YET POPULATED)

This directory is a **placeholder created during Phase 14 prep work**
(see `docs/PHASE_14_CRANELIFT_BRIEF.md`). The actual crate sources
have not been copied in yet — same sandbox restrictions as the
sister cranelift vendor dir. This note documents intended layout,
version pin, and the procedure to populate it later.

## Target version pin

- **Crate:** `rustc_codegen_cranelift` (a.k.a. `cg_clif`)
- **Recommended source:** the `rustc-codegen-cranelift` subtree
  bundled with `nightly-2026-02-01` (the kernel's pinned toolchain;
  see `kernel-x86_64/rust-toolchain.toml`). Using the bundled
  version guarantees the MIR ABI matches our rustc.
- **Upstream repo:** <https://github.com/rust-lang/rustc_codegen_cranelift>
  (subtree-synced from there into rust-lang/rust)
- **Crates.io fallback:** there is no current published crate
  release; cg_clif ships via the rustup component
  `rustc-codegen-cranelift-preview`. For our prep purposes the
  in-tree source IS the source of truth.

## Why the bundled version, not a separate release?

cg_clif is tightly coupled to rustc internals — it depends on
`rustc_codegen_ssa`, `rustc_middle`, `rustc_session`, etc. as
unstable compiler-internal crates. Pulling a "current crates.io
release" without matching the rustc it was developed against is a
recipe for compile errors that take a day to diagnose. Pinning to
the bundled-with-our-toolchain version makes this a non-issue.

The downside: re-vendoring requires installing the matching nightly
toolchain first. The procedure below assumes nightly-2026-02-01.

## Procedure to populate this directory

```bash
# 1. Ensure the kernel's nightly is installed
rustup install nightly-2026-02-01
rustup component add --toolchain nightly-2026-02-01 rust-src \
                                                   rustc-codegen-cranelift-preview

# 2. Locate the cg_clif source in the rustup sysroot
SYSROOT=$(rustc +nightly-2026-02-01 --print sysroot)
CG_CLIF_SRC=$SYSROOT/lib/rustlib/rustc-src/rust/compiler/rustc_codegen_cranelift

# 3. Copy into our vendor tree, preserving the layout
rsync -a --delete \
    "$CG_CLIF_SRC/" \
    F:/Software/ArmKernel3/kernel-core/vendor/rustc_codegen_cranelift/

# 4. The sister cranelift sub-crates already vendored should match
#    versions with what cg_clif's Cargo.toml expects. Reconcile by
#    comparing this dir's Cargo.toml `cranelift-* = ...` lines to
#    the version pin in vendor/cranelift/VENDOR_NOTE.md. If they
#    don't match, prefer cg_clif's expected version and re-vendor
#    cranelift to match.
```

## Directory layout once populated

```
vendor/rustc_codegen_cranelift/
├── Cargo.toml          ← workspace root (the cg_clif crate)
├── src/
│   ├── lib.rs          ← codegen backend entry point
│   ├── driver/         ← invoked by rustc when -Zcodegen-backend=cranelift
│   ├── intrinsics/     ← Rust-language intrinsic lowerings
│   ├── abi/            ← System-V / Windows calling convention impls
│   ├── value_and_place.rs  ← MIR Place lowering
│   ├── num.rs          ← integer / float ops
│   └── ...
├── build_system/       ← Python+shell that drives the bootstrap
├── patches/            ← libstd patches to make it cg_clif-friendly
└── example/            ← tiny standalone programs used in CI
```

## Patches we already KNOW we'll need (forecast, do not pre-apply)

1. **`Cargo.toml` dep redirects** — change every `cranelift-* = "X.Y"`
   crates.io ref to `cranelift-* = { path = "../cranelift/cranelift-*" }`.
   That's a mechanical edit per sub-crate.

2. **`src/driver/mod.rs`** — invokes `std::process::Command` to call
   `rustc` for sysroot bootstrap. On Semantic OS that goes through
   our M25 `std::process` shim, which routes to `SYS_SPAWN`. The
   driver itself doesn't need source changes; the std shim is the
   work.

3. **`build_system/`** — Python-based scripts to bootstrap a sysroot.
   On Semantic OS we don't have Python. Either:
   - Port the bootstrap to a Rust binary (estimated ~500 LOC), or
   - Skip the bootstrap step entirely for the first M27 milestone
     and use a pre-built sysroot copied in from the cross-build
     server.

   First-milestone path is the pre-built sysroot. The bootstrap port
   is a separate follow-up.

4. **`src/intrinsics/llvm.rs`** — implements LLVM-specific intrinsics
   (the `core::intrinsics::llvm_*` family). We're targeting x86_64
   only, so the LLVM-named-but-actually-x86 intrinsics (atomics,
   prefetch, etc.) all still apply. Audit + cull anything that
   names a non-x86 ISA.

5. **`patches/0001-Disable-not-compiling-tests.patch`** etc. —
   cg_clif ships patches it applies to `libstd` before bootstrapping
   it through itself. These patches assume `std` compiles. We can't
   use them as-is because our `std` shim (M25) is structurally
   different from upstream std (it's a translation layer, not a port
   of upstream sources). Plan: re-derive the patches against the M25
   shim once that lands; track each upstream patch's intent as a
   row in the M26 worksheet.

## What "vendored" means for this crate

Same as cranelift sister vendor: source-present, version-pinned,
NOT yet wired into the build graph. cg_clif requires `std` to
compile (it links into rustc, which is a std consumer); we don't
have std on x86_64-unknown-none yet. Adding it to
`kernel-core/Cargo.toml` now would only produce errors.

## Status as of Phase 14 prep

| Item | Status |
|---|---|
| Vendor dir created | yes |
| VENDOR_NOTE.md written | yes (this file) |
| Sources copied | NO (sandbox-blocked; procedure documented above) |
| Patches applied | NO |
| Bootstrap script ported | NO (Python → Rust port deferred) |
| `LICENSE` file present | NO (waiting on sources) |

## License

rustc_codegen_cranelift is licensed under
**MIT OR Apache-2.0** (the standard rust-lang dual license).
Compatible with our codebase's posture. The full `LICENSE-MIT` and
`LICENSE-APACHE` files will land in this directory once sources
are populated.
