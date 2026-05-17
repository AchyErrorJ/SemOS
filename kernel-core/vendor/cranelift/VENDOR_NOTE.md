# Vendored cranelift (Phase 14 prep — SOURCES NOT YET POPULATED)

This directory is a **placeholder created during Phase 14 prep work**
(see `docs/PHASE_14_CRANELIFT_BRIEF.md`). The actual crate sources
have not been copied in yet — the prep-session sandbox blocked both
network access and `cargo` execution, so the vendoring step was
deferred. This note documents the **intended** layout, version pin,
and the exact procedure a follow-up agent (or human) should run to
finish populating it.

## Target version pin

- **Crate:** `cranelift` (and its sister sub-crates listed below)
- **Version:** `0.121.0` (released as part of the wasmtime 38.0.0
  cycle, end of Q1 2026 — verify on crates.io before vendoring; if
  a newer stable is out adjust this note and the dependency stanzas
  to match)
- **Source URL:** <https://crates.io/crates/cranelift>
- **Upstream repo:** <https://github.com/bytecodealliance/wasmtime>
  (cranelift lives in `cranelift/` inside the wasmtime workspace)
- **Git tag for source pinning:** `cranelift-v0.121.0`

### Sister sub-crates (all share the version)

The `cranelift` crate is a thin re-export wrapper. The real work is
done by these sub-crates, all of which we need vendored together so
their internal `path =` deps resolve without crates.io:

| Sub-crate | Purpose |
|---|---|
| `cranelift-codegen` | The IR + register allocator + x86_64 backend |
| `cranelift-codegen-meta` | Build-time IR/ISA metadata generator |
| `cranelift-codegen-shared` | Types shared between codegen and codegen-meta |
| `cranelift-frontend` | Builder API on top of `cranelift-codegen` |
| `cranelift-entity` | Typed integer indices used everywhere in the IR |
| `cranelift-bforest` | B+tree backing for some IR data structures |
| `cranelift-control` | Fuzzing hooks (we can drop the feature) |
| `cranelift-isle` | Compiler for the ISLE pattern-match DSL the backend uses |
| `cranelift-srcgen` | Source generator used by `codegen-meta` |
| `cranelift-native` | Host CPU feature detection (we WILL patch this out) |
| `cranelift-jit` | JIT entrypoint that mmaps + executes code (LIKELY DROPPED — see below) |
| `cranelift-module` | The "compile a function, get a callable pointer back" API |
| `cranelift-object` | Emits ELF object files via the `object` crate (likely how we'll consume codegen output) |

## What "vendored" means for this crate

We are NOT trying to build cranelift against `x86_64-unknown-none`
during this prep step. That requires `std`, which we don't have yet
(it's the M25 deliverable, scheduled to land BEFORE the codegen
integration in M26). What we are trying to achieve in prep is:

1. Source tree present in the repo so reviewers can read it without
   a network round-trip
2. Version pinned in writing so the M26 agent has a known starting
   point and doesn't have to re-research "which version of cranelift
   was current when we started"
3. Test baseline established on the HOST (`x86_64-pc-windows-msvc`
   or `x86_64-unknown-linux-gnu`) so we know what "passing" looks
   like before our patches start cutting things out

The crate IS **excluded from `kernel-core/Cargo.toml`'s dep tree
for now** — it would not build (it requires `std`), and adding it
to the build graph just to see it fail wastes CI cycles. The M26
work re-adds it as a `path = "vendor/cranelift"` dep at the same
time the std shim becomes available.

## Procedure to populate this directory

Run this once, on a machine with network + cargo, BEFORE starting
M26 proper:

```bash
# 1. Download fresh source (replace 0.121.0 with the pinned version)
cargo new --bin /tmp/cranelift-fetch
cd /tmp/cranelift-fetch
cargo add cranelift@=0.121.0 \
            cranelift-codegen@=0.121.0 \
            cranelift-frontend@=0.121.0 \
            cranelift-module@=0.121.0 \
            cranelift-object@=0.121.0 \
            cranelift-jit@=0.121.0
cargo fetch

# 2. Find the cached sources
ls ~/.cargo/registry/src/index.crates.io-*/cranelift-*

# 3. For each sub-crate, copy into our vendor tree. Preserve the
#    crate's internal Cargo.toml verbatim; we'll patch path =
#    references on the kernel-core side later.
for CRATE in cranelift cranelift-codegen cranelift-codegen-meta \
             cranelift-codegen-shared cranelift-frontend \
             cranelift-entity cranelift-bforest cranelift-control \
             cranelift-isle cranelift-srcgen cranelift-native \
             cranelift-jit cranelift-module cranelift-object; do
    SRC=$(ls -d ~/.cargo/registry/src/index.crates.io-*/${CRATE}-* | head -1)
    DST=F:/Software/ArmKernel3/kernel-core/vendor/cranelift/${CRATE}
    mkdir -p "$DST"
    rsync -a --delete "$SRC/" "$DST/"
done

# 4. Run the upstream test suite ON THE HOST TARGET to get baseline
#    numbers. This is "before any of our patches" baseline.
cd F:/Software/ArmKernel3/kernel-core/vendor/cranelift/cranelift-codegen
cargo test --release --no-fail-fast 2>&1 | tee \
    F:/Software/ArmKernel3/kernel-core/vendor/cranelift/TEST_BASELINE.txt
```

Record the test totals in `TEST_BASELINE.txt`:
- N tests in cranelift-codegen
- N tests in cranelift-frontend
- N tests in cranelift-module
- N pass / N fail on the host

## Patches we already KNOW we'll need (forecast, do not pre-apply)

These come from reading the source on crates.io and matching it to
our `no_std` / `no_alloc` posture. The actual diff lands during M26.

1. **`cranelift-codegen/Cargo.toml`** — disable default features.
   The default pulls `host-arch` (auto-detects via cpuid + std).
   We compile for a fixed target (x86_64-unknown-none) and don't
   need autodetection. Cut: `default-features = false`, add
   `features = ["x86", "unwind"]`.

2. **`cranelift-native`** — entire crate gets `#[cfg(feature = "std")]`
   gated out. It does runtime CPU feature detection via the standard
   library; we feed the feature set statically. Replacement is a
   ~50 LOC stub that returns our known target's feature set.

3. **`cranelift-codegen/src/timing.rs`** — uses `std::time::Instant`.
   Either:
   - Cfg-gate the timing module behind `std`, or
   - Wire it to `crate::platform::ticks()` from kernel-core (gives us
     timing in QEMU ticks instead of wallclock; fine for self-profiling)

4. **`cranelift-jit`** — likely DROPPED entirely. It mmaps anonymous
   memory and marks it executable; that's a `libc` dependency we
   don't satisfy. Our use case is "produce an object file, hand to
   our own ELF loader" (already in `kernel-core/src/process/elf.rs`).
   `cranelift-module` + `cranelift-object` is the path we want.

5. **`cranelift-codegen` internal `Vec`/`HashMap` usage** — survey
   for any code paths that assume the global allocator works (vs
   our tier-aware allocator). The IR builder allocates heavily;
   that's fine once we have a real allocator (Phase 14 prerequisite,
   tracked separately).

6. **ISLE-generated code** — `cranelift-codegen` ships pre-generated
   ISLE output as committed `.rs` files. We can use them as-is.
   The `cranelift-isle` crate itself only runs at build time (only
   needed if we modify a `.isle` file). For Phase 14 we likely
   never regenerate — pin the committed `.rs` and forget `isle`
   exists.

## What's NOT in scope for this vendor

- **WebAssembly frontend (`cranelift-wasm`)** — we are NOT a wasm
  runtime. The MIR → cranelift-IR translator is what rustc_codegen_cranelift
  provides (see the sister vendor dir). We never go through wasm.
- **`wasmtime` crate itself** — we are taking cranelift the
  codegen library, not the wasmtime VM that's built on top of it.
- **`peepmatic`** — superseded by ISLE in current cranelift versions.
  Not present in 0.121.

## License

cranelift is licensed under **Apache-2.0 WITH LLVM-exception**.
Compatible with our codebase's MIT-or-Apache dual-license posture
(Apache is the relevant side; the LLVM exception is a permissive
patent grant that's stricter than vanilla Apache only for downstream
modification, which we accept).

The full license text will land at `LICENSE` in this directory once
sources are populated. The LLVM-exception text comes alongside the
Apache-2.0 license file in the upstream tree.

## Status as of Phase 14 prep

| Item | Status |
|---|---|
| Vendor dir created | yes |
| VENDOR_NOTE.md written | yes (this file) |
| Sub-crate sources copied | NO (sandbox-blocked) |
| Patches applied | NO (waiting on sources) |
| `kernel-core/Cargo.toml` dep added | NO (would fail to build — see brief) |
| Host test baseline captured | NO (cargo execution blocked) |
| `LICENSE` file present | NO (waiting on sources) |
