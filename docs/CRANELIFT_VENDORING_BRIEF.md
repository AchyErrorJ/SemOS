# Cranelift Vendoring Brief

**Date:** 2026-07-18 (written retroactively per the 2026-07-17 code review)
**Status:** active record of upstream state for the Cranelift vendored trees
**Companion to:** `docs/EMBEDDED_TLS_VENDORING_BRIEF.md`,
`docs/SMOLTCP_VENDORING_BRIEF.md`, `docs/VENDORING.md` (index),
`docs/PHASE_14_CRANELIFT_BRIEF.md`, `docs/M27_RUSTC_PORT_PLAN.md`,
`user-programs/semos-rustc/vendor-externals/CRANELIFT_VENDOR_NOTES.md`

The 2026-07-17 external review noted that embedded-tls and smoltcp have
vendoring briefs but the two biggest trees do not. This brief covers the
**Cranelift** half: `compiler/vendor/` (~550k LoC) and the Cranelift subset
inside `user-programs/semos-rustc/vendor-externals/`.

---

## 1. What is vendored, and where

| Tree | Contents | Size | Consumer |
|------|----------|------|----------|
| `compiler/vendor/` | 44 crates: `cranelift-*` 0.122.0 (codegen, frontend, module, object, isle, entity, control, bitset, bforest, assembler-x64, srcgen, …) plus support crates (`anyhow` 1.0.102, `arbitrary` 1.4.2, `bumpalo` 3.20.3, `cfg-if`, `crc32fast` 1.5.0, `serde` 1.0.228, `quote` 1.0.45, `target-lexicon`, …) | ~26 MB | `compiler/` (semos-compiler, M26) |
| `user-programs/semos-rustc/vendor-externals/` | 27 crates: the Cranelift `no_std` port subset (same 0.122.0 line) + `regalloc2`, `gimli`, `hashbrown`-adjacent deps, `libm`, `memchr`, `regex`/`aho-corasick`, `tracing-core`, … | (in the 1.3 GB tree) | `semos-rustc` via `rustc_codegen_cranelift` (M27) |

`compiler/vendor/` was produced by `cargo vendor` (each crate dir carries
`Cargo.toml.orig`); `vendor-externals/` was copied file-by-file from the
same M26 Session B crates.io download (see `CRANELIFT_VENDOR_NOTES.md` —
the sandbox blocked recursive copies).

## 2. Upstream state

- **Crate family**: Cranelift (`github.com/bytecodealliance/wasmtime`,
  `cranelift/*`), **0.122.0** — the wasmtime release-38 cycle.
- **License**: Apache-2.0 WITH LLVM-exception (per-crate `LICENSE` files
  ride along in the vendored dirs).
- **Pin rationale** (from `CRANELIFT_VENDOR_NOTES.md`):
  1. 0.122.0 was already in-tree from M26 Session B.
  2. `rustc_codegen_cranelift` (cg_clif) is **co-pinned to the rustc
     nightly** (`nightly-2026-02-01`), so we own the API-compat decision;
     cg_clif's Cargo.toml was edited down from its upstream 0.127.0
     declaration to 0.122.0.
  3. 0.127 → 0.122 spans ~5 minor versions (the usual breaking-API
     window); if type-checking against the rustc internal surface fails,
     we bump or fork the rustc-side API, not silently drift.

## 3. Local modifications

- `compiler/vendor/`: pristine `cargo vendor` output; SemOS consumes it
  through `compiler/Cargo.toml` (cranelift-codegen/-frontend/-module/
  -object 0.122 + target-lexicon 0.13).
- `vendor-externals/`: **modified** — this is the `no_std` port staging
  area. Crates were patched crate-by-crate for
  `x86_64-unknown-none` (std-gated features off, `hashbrown`/`libm`
  substitutions) to make `cargo check -p rustc_codegen_cranelift
  --target x86_64-unknown-none` pass. `CRANELIFT_VENDOR_NOTES.md` is the
  running log of that port and is authoritative for *what was patched*.
- cg_clif itself lives in `vendor-rustc-src/compiler/
  rustc_codegen_cranelift` (see `SEMOS_RUSTC_VENDORING_BRIEF.md`); its
  Cranelift dependency was repointed at these vendored crates.

## 4. Update policy

1. **Bump the whole 0.122 line together.** The `cranelift-*` crates share
   internal types; a partial bump is a build break by construction.
2. Any bump must re-validate three gates, in order:
   `compiler/` build → `cargo check -p rustc_codegen_cranelift
   --target x86_64-unknown-none` → a full `semos-rustc` image build plus
   the DEMO 80 on-device compile smoke.
3. Before bumping, re-check cg_clif's upstream declared Cranelift version
   for the *current* pinned nightly — if upstream has moved past the API
   delta we're carrying, prefer realigning with upstream over deepening
   the local diff.
4. Security-relevant Cranelift advisories (miscompiles are codegen
   correctness bugs for us, not just crashes) justify an out-of-band
   bump; note them in this file when applied.

## 5. Known gaps

- No per-crate patch log for `compiler/vendor/` (it is believed pristine
  but was never diffed against crates.io after the fact — worth a
  one-time `cargo vendor --sync` style verification).
- `vendor-externals/` patches are narrated in `CRANELIFT_VENDOR_NOTES.md`
  but not split out as `.patch` files; a re-vendor would have to replay
  them by hand.
