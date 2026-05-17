# Phase 14 Cranelift integration brief

**Status:** prep work for [Phase 14 M26](ROADMAP.md#m26--cranelift-backend-integration).
Companion to [`STD_SHIM_SURFACE.md`](STD_SHIM_SURFACE.md), which covers
the M25 std-shim prerequisite. This document is a planning brief, not
a tutorial. It cites upstream source by file path so a future agent
(or human) can read the actual code, not just our retelling.

Sister vendor notes (placeholder, awaiting source population):
- `kernel-core/vendor/cranelift/VENDOR_NOTE.md`
- `kernel-core/vendor/rustc_codegen_cranelift/VENDOR_NOTE.md`

---

## 1. Why Cranelift at all

We need a Rust compiler that runs **on** Semantic OS, producing
native x86_64 code that runs **on** Semantic OS. Two choices:

1. **Port LLVM as-is.** LLVM is ~10M LOC of C++. Semantic OS has no
   C++ runtime and a deliberate Rust-only policy (`ROADMAP.md`
   "Out of scope, settled" section forbids reintroducing C++).
2. **Adopt Cranelift.** ~150K LOC of pure Rust. Designed from the
   start as a library, not a monolithic toolchain. Has a
   rustc-compatible backend (`rustc_codegen_cranelift`, aka cg_clif)
   that's been shipping in nightly as a rustup component since
   ~2021. Trades peak codegen quality for compile-time speed and
   embeddability — perfectly aligned with our "self-hosting on a
   laptop, not a build farm" use case.

We go with Cranelift. LLVM stays available for cross-builds on the
dev machine (the existing kernel-x86_64 build path); on the device
itself, only Cranelift exists.

### What we give up by not having LLVM on the device

| LLVM optimization | What we lose | Severity for our workload |
|---|---|---|
| Aggressive inlining | Code-size + perf | Medium — Cranelift inlines, just less |
| Loop vectorization (SIMD widening) | Throughput on hot loops | Low — kernel doesn't run hot user loops; rustc itself isn't SIMD-bound |
| Polly polyhedral / loop transforms | Numerical perf | Negligible for our workload |
| GVN, LICM, mem2reg quality | General codegen quality | Cranelift has all three, just less aggressive |
| LTO / ThinLTO | Cross-crate inlining | High — our `lto = true` profile assumes this |
| Profile-guided optimization | Branch hints | Negligible (we never had PGO in our build anyway) |
| Sanitizer passes (ASan, MSan, TSan) | Dynamic memory-safety | High if you use them, but they're host-side only — we'd keep them on the cross-build server |

Honest accounting: **our self-built binaries will be measurably slower
than our cross-built ones**, probably 20-40% slower on rustc-style
workloads (per published cg_clif benchmarks). For self-hosting that's
acceptable: the dev loop runs on the device, performance-critical
release builds still go through the cross-build server (Phase 13 M23
fallback) if we ever want LLVM-level quality.

---

## 2. Cranelift architecture, the relevant slice

Cranelift is a workspace of ~14 sub-crates. For our purposes the
ones that matter are:

```
                  +-------------------------+
                  |   rustc                 |
                  |   (frontend: parse/HIR  |
                  |    /MIR/borrowck)       |
                  +-----------+-------------+
                              | MIR
                              v
              +---------------+--------------+
              |  rustc_codegen_cranelift     |   <-- VENDORED at
              |  ("cg_clif")                 |       vendor/rustc_codegen_cranelift/
              |   - MIR -> Cranelift IR      |
              |   - intrinsic lowerings      |
              |   - ABI handling             |
              +---------------+--------------+
                              | Cranelift IR
                              v
              +---------------+--------------+
              |  cranelift-frontend          |   <-- vendor/cranelift/cranelift-frontend/
              |   (FunctionBuilder API)      |
              +---------------+--------------+
                              v
              +---------------+--------------+
              |  cranelift-codegen           |   <-- vendor/cranelift/cranelift-codegen/
              |   - IR optimization passes   |
              |   - register allocation      |
              |   - x86_64 backend (ISLE)    |
              |   - machine code emission    |
              +---------------+--------------+
                              | raw bytes + relocations
                              v
              +---------------+--------------+
              |  cranelift-module +          |   <-- vendor/cranelift/cranelift-module/
              |  cranelift-object            |   <-- vendor/cranelift/cranelift-object/
              |   (collect into an ELF       |
              |    object file)              |
              +---------------+--------------+
                              | .o file
                              v
              +---------------+--------------+
              |  Our linker (TBD; see        |
              |  section 7) -> ELF binary    |
              |  -> loaded by                |
              |     kernel-core/process/elf  |
              +------------------------------+
```

### What each crate actually does

**`cranelift` (the wrapper crate)** — re-exports `cranelift_codegen`
and `cranelift_frontend`. Pure convenience. No code of our own
touches this; we depend on the sub-crates directly.

**`cranelift-codegen`** — the core. Defines the IR
(`cranelift_codegen::ir`), runs optimization passes
(`cranelift_codegen::opts`), runs register allocation
(`cranelift_codegen::regalloc`), runs the target backend
(`cranelift_codegen::isa::x64`), and emits machine code. The x86_64
backend is itself ~30K LOC of ISLE-generated rules + handwritten
glue.

**`cranelift-frontend`** — `FunctionBuilder` API. Hides the
detail of converting "the user thought in terms of variables" into
SSA's "values flow between basic blocks via block params." cg_clif
uses this heavily; we never write to it directly.

**`cranelift-module`** — abstracts over "I'm building something
that will eventually become a loadable artifact." Three implementations
exist: `cranelift-jit` (in-process mmap+exec), `cranelift-object`
(write an ELF/Mach-O/COFF object), and a `cranelift-faerie` that's
been removed in recent versions. cg_clif uses `cranelift-object` for
AOT compilation (our path); the JIT path is for cg_clif's
experimental --jit mode (not our path).

**`cranelift-entity`, `cranelift-bforest`** — utility crates for
typed integer indices and B+tree-backed data structures used inside
the IR. Pure data structures, no platform deps.

**`cranelift-native`** — host CPU feature detection (CPUID query +
feature flag construction). We don't want this; we feed the feature
set statically for our known target (x86_64 with whatever's safe to
assume for ThinkPad P1 Gen 6's 13th-gen Intel: AVX2 yes, AVX-512 no,
BMI2 yes, etc.). Documented patch in
`vendor/cranelift/VENDOR_NOTE.md`.

**`cranelift-isle`** — the **I**nstruction **S**election **L**owering
**E**xpression DSL. cranelift-codegen's backend pattern matching is
written in ISLE and compiled to Rust source ahead of time. The
generated `.rs` files ship committed in upstream; we use them as-is.
The `isle` crate itself only runs at build time IF you modify a
`.isle` file — we won't. Vendor it for completeness but it stays
inert.

**`cranelift-control`** — fuzzing hooks. Disable the feature; don't
need it.

### Not in scope for our vendor

- **`cranelift-wasm`** — wasm-to-CLIF frontend. We're not a wasm
  runtime; our frontend is cg_clif, which goes MIR → CLIF directly.
- **`wasmtime`** — the VM that consumes cranelift output for wasm
  execution. Irrelevant.
- **`cranelift-jit`** — likely dropped (vendor note explains).

---

## 3. rustc_codegen_cranelift's relationship to rustc

This trips people up: cg_clif is **not a rustc fork**. It's a
"codegen backend" crate that's loaded by rustc as a dynamic library
when you pass `-Zcodegen-backend=cranelift`. The relationship:

```
   rustc binary
        |
        | dynamically loads:
        v
   librustc_codegen_cranelift.so  (or .dll)
        |
        | implements trait:
        v
   rustc_codegen_ssa::traits::CodegenBackend
```

rustc itself is the frontend (parse → HIR → MIR → borrowck → trans
preparation). After MIR, rustc hands off to whichever
`CodegenBackend` impl is loaded. The default is the LLVM backend
(`rustc_codegen_llvm`, in-tree). Setting
`-Zcodegen-backend=cranelift` swaps cg_clif in instead.

Consequences:

1. **cg_clif has the same MIR ABI as the rustc it shipped with.**
   The version pin in
   `vendor/rustc_codegen_cranelift/VENDOR_NOTE.md` (the
   nightly-2026-02-01 bundled version) matches our kernel's
   `rust-toolchain.toml`. If we upgrade the toolchain, we re-vendor
   cg_clif at the same time. **Hard rule.**

2. **cg_clif depends on unstable rustc-internal crates.** Concretely:
   `rustc_codegen_ssa`, `rustc_middle`, `rustc_session`, `rustc_span`,
   `rustc_target`, `rustc_metadata`, `rustc_data_structures`,
   `rustc_index`, `rustc_errors`, and several more. These are
   **not** crates.io crates — they live in `compiler/` in
   rust-lang/rust and have unstable APIs. The only way to compile
   cg_clif is with the matching nightly rustc available; if we want
   to compile cg_clif **on Semantic OS**, we need rustc itself on
   Semantic OS first.

3. **Chicken-and-egg.** M27 (first rustc build on Semantic OS) needs
   a rustc binary on Semantic OS. The bootstrap path is:
   - Cross-build a rustc binary on the dev machine (host's LLVM
     produces it).
   - Cross-build cg_clif against that rustc (also on the dev machine).
   - Copy both binaries into the Semantic OS disk image.
   - Boot Semantic OS. Run the cross-built rustc + cg_clif under our
     std shim to compile a "hello world".
   - Iterate until it works.
   - Then: re-compile rustc itself, using the cross-built rustc, with
     cg_clif as the codegen backend. The output rustc binary is our
     first **self-hosted** rustc. The capstone of M28 is this
     rebuild producing a working binary.

   Note that the rustc binary itself is cross-built; we're not
   trying to compile rustc-the-source on Semantic OS until M28 (and
   even then we're using a working rustc that was itself cross-built
   — no chicken-and-egg in the bootstrap, just in the
   "what's the first thing we run" question).

---

## 4. How a Rust program actually flows through the pipeline

A concrete walk-through of compiling a single `hello.rs`:

```rust
// hello.rs
fn main() { println!("hello"); }
```

1. **Driver: cargo or rustc directly.** Let's assume rustc directly:
   `rustc -Zcodegen-backend=cranelift hello.rs -o hello`
2. **rustc frontend.** Parse → AST → HIR → MIR. MIR is rustc's
   control-flow-graph IR with basic blocks, places, rvalues. Stable
   inside a rustc version, unstable across versions.
3. **rustc dispatches each MIR body to the codegen backend.** The
   backend trait method called is roughly
   `CodegenBackend::codegen_crate`. Inside cg_clif this lives in
   `src/driver/mod.rs` (will be at
   `vendor/rustc_codegen_cranelift/src/driver/mod.rs` once vendored).
4. **cg_clif: MIR → Cranelift IR.** Per-function translation in
   `src/base.rs` (and the abi/ + intrinsics/ subdirs). Lowers
   MIR's `Place`, `Operand`, `Rvalue`, `Terminator` into
   `cranelift_frontend::FunctionBuilder` calls that produce a
   `cranelift_codegen::ir::Function`.
5. **Cranelift IR optimization passes.** `cranelift_codegen::opts`
   runs egraph-based simplification, GVN, LICM. Not as aggressive
   as LLVM but does the obvious wins.
6. **Register allocation.** `cranelift_codegen::regalloc` (the
   `regalloc2` algorithm; tree-spilling SSA-aware allocator).
7. **x86_64 backend lowering.** `cranelift_codegen::isa::x64`. The
   bulk of the rules are ISLE-generated from
   `cranelift-codegen/src/isa/x64/lower.isle`.
8. **Machine code emission.** Per-function machine bytes go into
   a `cranelift_codegen::CompiledCode`.
9. **Object file assembly.** `cranelift-object` collects all the
   compiled functions + their relocations into an ELF `.o` file
   via the `object` crate.
10. **Linking.** rustc invokes the system linker (`lld` or `link.exe`
    today) to combine the `.o` + `libstd.rlib` + crt startup files
    into a final `hello` binary. **On Semantic OS we don't have a
    system linker yet** — see section 7.
11. **Load & run.** Our `kernel-core/src/process/elf.rs` loader takes
    over.

---

## 5. What "no LLVM on the device" means in practice

### Things that stay identical
- Source compatibility — any Rust code that works under
  `rustc -Zcodegen-backend=cranelift` works under our pipeline.
- The borrow checker, type checker, MIR optimizer — all rustc
  frontend, all unaffected.
- `#[inline]`, `#[cold]`, `#[no_mangle]` attributes work (cg_clif
  honors them).
- Atomics — cg_clif emits the same x86_64 atomic instructions.
- TLS (thread-local storage) — cg_clif supports it on x86_64.
- `extern "C"` ABIs — works.

### Things that change
- **Compile-time speed.** Cranelift compiles ~3x faster than LLVM
  -O0 and ~10x faster than LLVM -O2 on rustc's own source. Win for
  self-hosting.
- **Generated-code performance.** Roughly 20-40% slower than LLVM
  -O2 on rustc-style workloads (chasing pointers, recursion,
  hashmap lookups). Less of a gap on straight-line code. No SIMD
  widening, no PGO, no LTO — the three biggest LLVM wins are absent.
- **Debug info.** cg_clif emits DWARF; quality is lower (line
  numbers and parameter types yes, full inlined-function expansion
  no). Backtraces work; rust-gdb pretty-printing works.
- **Some unstable features are unimplemented.** As of the bundled
  cg_clif version: no `#[link_section]` for some sections, no
  `core::arch::x86_64::_pdep_u64` software fallback (we'd hit a
  bug on AMD CPUs without BMI2 — not relevant since our target is
  Intel-only). Audit list lives in cg_clif's `Readme.md` under
  "Known limitations."
- **No LLVM intrinsics.** `core::intrinsics::llvm_*` calls become
  `unimplemented!()` panics at compile time. Rust code that depends
  on those (rare; mostly compiler-test crates) won't compile.

### Things that REMAIN required even with cg_clif
- A linker. cg_clif produces `.o`; the link step is the same as
  with the LLVM backend. We need an on-device linker. Section 7.
- A complete `libstd`. cg_clif compiles `libstd` like any other
  crate — but we need to provide a `libstd` that lowers to OUR
  syscalls. That's M25 (`docs/STD_SHIM_SURFACE.md`).
- A `libcore` and `liballoc`. These exist as rustlibs; cg_clif
  builds them from the rust-src component. No platform work needed
  beyond making sure rust-src is on the disk.

---

## 6. Concrete integration plan (high-level)

The work splits into five chunks. Order matters: each depends on
the previous.

### Chunk 1: source vendoring (this prep + a follow-up "do the
actual `rsync`" agent)

- Vendor `cranelift` + sub-crates per
  `vendor/cranelift/VENDOR_NOTE.md`.
- Vendor `rustc_codegen_cranelift` per its VENDOR_NOTE.
- Capture host-target test baselines for both.
- Outcome: source present, version pinned, no integration yet.
  Repo grows by ~250 MB of vendored Rust.

### Chunk 2: build cranelift against `x86_64-unknown-none` (NOT against
our std shim yet — just get the no-std build going)

The cranelift sub-crates use `std::collections::HashMap`,
`std::time::Instant`, `std::sync::Mutex`, and friends. To compile
against `x86_64-unknown-none` we need either:

- (a) Provide stubs for those types via our std shim — but the std
  shim itself needs cranelift to compile, so this is circular.
- (b) Vendor the relevant pieces of `hashbrown`, `spin`, etc., and
  patch cranelift to use them via cfg-gates — the existing no-std
  story for cranelift. This is what wasmtime does for its embedded
  builds; the patches are known and small.

Path (b). Expected patch surface: ~30 cfg-gate additions across
cranelift-codegen, cranelift-frontend, cranelift-module. Track each
in `vendor/cranelift/VENDOR_NOTE.md`'s patch section as it lands.

### Chunk 3: build cg_clif against our std shim

cg_clif depends on `rustc_codegen_ssa` and friends. Those are
themselves std consumers. The order is:

1. Build a libstd that compiles via the rustc-on-host path (this is
   M25 — output is a libstd rlib that targets `x86_64-unknown-semos`,
   the new target triple we'll need to register).
2. Build cg_clif against that libstd, on the host. Output is a
   `librustc_codegen_cranelift.so` that runs on Semantic OS when
   loaded by a rustc-on-Semantic-OS.
3. Build a rustc binary (host LLVM produces it) that's targeted to
   run on `x86_64-unknown-semos`. This is "rustc binary, but linked
   against our libstd." rustc itself isn't huge to retarget — the
   compiler is mostly target-independent, and the bits that aren't
   (the `rustc_target` crate's target-tuple definitions) just need
   a `x86_64-unknown-semos` entry added.

That's the cross-build of rustc + cg_clif from the host. End state:
the disk image carries `rustc`, `librustc_codegen_cranelift.so`,
`libstd.rlib`, and `rust-src/`. Booting and running `rustc --version`
in the framebuffer console is the first M27 milestone.

### Chunk 4: target tuple `x86_64-unknown-semos`

The Rust compiler maintains target specs as JSON files (or built-in
target definitions in `rustc_target/src/spec/`). For our target:

```json
{
  "arch": "x86_64",
  "cpu": "x86-64",
  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128",
  "executables": true,
  "linker": "rust-lld",
  "linker-flavor": "ld.lld",
  "llvm-target": "x86_64-unknown-none",
  "max-atomic-width": 64,
  "os": "semos",
  "panic-strategy": "abort",
  "position-independent-executables": false,
  "relocation-model": "static",
  "target-pointer-width": "64",
  "vendor": "unknown",
  "code-model": "kernel"
}
```

Add to `rustc_target/src/spec/targets/x86_64_unknown_semos.rs`. Also
add to `rustc_target/src/spec/mod.rs`'s `supported_targets!` list.

This is a ~50 LOC change to rustc itself; ships as part of our
"cross-built rustc for Semantic OS" artifact.

### Chunk 5: cargo

cargo is a separate crate from rustc (rust-lang/cargo). It builds
against `std` just like any other Rust program. Once M25's std shim
is in place, cargo's port reduces to:
- Cross-build cargo for `x86_64-unknown-semos`.
- Solve the network gap: cargo expects to talk to crates.io. For
  the first M27, hardcode a local-only registry (cargo's `[source]`
  config supports this). Crates come pre-fetched into a directory
  on the disk image; cargo never makes a network call.
- Network-fetching cargo is a follow-up (depends on Tier 4 prereqs
  in STD_SHIM_SURFACE: TCP syscall surface + DNS).

### Chunk 6: the linker

Mentioned separately because it's not Cranelift's problem but it
blocks the end-to-end pipeline.

Two options:

- **`rust-lld`** ships with rustc as a wrapped LLD binary. It's a
  C++ binary; that's a non-starter on Semantic OS (no C++ runtime).
- **`mold`** is C++ too. Same issue.
- **`wild`** is a pure-Rust linker (~50K LOC, alpha-stage as of
  early 2026). Targets ELF only, which is what we want. Recommended
  path: vendor `wild` alongside cranelift; cross-build a `wild`
  binary for `x86_64-unknown-semos`; rustc invokes it via the
  target spec's `linker` field.
- **Write our own.** Plausible at the scale we need (single-output
  ELF, no shared-library support, no symbol versioning) — probably
  ~5K LOC of Rust. Tracked as a possible follow-up if wild proves
  too unstable.

Recommended: try `wild`. Fall back to a homegrown linker only if
wild blocks.

---

## 7. Known unknowns

Questions a future agent (or human) should resolve before starting
M26 proper. Each is non-trivial; some might change the integration
plan.

1. **Does cranelift's no_std posture actually work against our
   target as of the pinned version?** wasmtime ships embedded-mode
   builds, but their target is "embedded with std-like features
   stubbed by the host" — not literally `x86_64-unknown-none`. The
   gap between "compiles with `no_std`" and "compiles without
   `alloc`" is significant. Verification: a follow-up agent should
   try `cargo build --target x86_64-unknown-none --no-default-features`
   inside each vendored cranelift sub-crate and report the error
   surface.

2. **How many of cg_clif's MIR lowerings depend on rustc-internal
   APIs that changed in the nightly we pinned vs the nightly when
   cg_clif was last verified to work in this configuration?** The
   ABI changes inside `rustc_middle` are usually small but
   occasionally break cg_clif for a week or two until the
   contributors catch up. Risk: the bundled cg_clif in
   nightly-2026-02-01 might itself be in a "broken, fix incoming"
   state. Verification: build cg_clif on host against the pinned
   nightly; if `cargo build` fails, look at upstream
   bytecodealliance/wasmtime issues + rust-lang/rust issues for the
   relevant timeframe.

3. **How big does the disk image get when we add rust-src + libstd
   sources + cranelift vendor + cg_clif vendor?** Today our vdisk
   is 16 MiB (created via `qemu-img create -f raw vdisk.img 16M`
   per `ROADMAP.md`). rust-src alone is ~200 MiB. cranelift's
   vendor tree is ~80 MiB unpacked. cg_clif's is ~20 MiB. Total
   ~300 MiB just for sources. We're going to want a multi-GiB
   image with a real filesystem before any of this is useful on
   bare metal. ThinkPad P1's NVMe handles it trivially; QEMU
   testing just needs `qemu-img create -f raw vdisk.img 2G` (or
   compressed `qcow2`).

4. **How do we ship rustc binaries between the host build and the
   Semantic OS image?** Today we have no installer, no package
   manager, no "drop a file in the disk image and have it appear
   in /usr/bin." Options:
   - Embed the rustc binary in the kernel's ramfs (today's
     `user-programs/` mechanism). Works for tiny binaries; rustc is
     ~50 MiB stripped — that's a 50 MiB kernel image. Painful but
     viable for first M27 attempt.
   - Implement a real on-disk FS that supports large file content
     and the rustc binary lives there. This is M5's promise
     ("snapshot persistence for the namespace") extended with
     large-content support. Probably the right answer long-term.
   - Boot-time copy: kernel reads rustc from a partition table on
     the disk and stages it into the ramfs at boot. Compromise.

5. **Calling convention compatibility between cg_clif output and
   our existing user-programs that were built by the host's LLVM
   rustc.** Both are System V x86_64 by default; should be ABI-
   compatible. Risk: tiny edge cases in how `repr(simd)` or
   `repr(packed)` lowers might differ. Probably won't matter for
   compiling Rust source on the device (everything is built by
   cg_clif end-to-end) but is a footgun for "I have a `.rlib` from
   the host, can I link it against `.rlibs` from the device."
   Recommendation: don't try; always rebuild from source on the
   device.

6. **Inline assembly support.** rustc's `asm!` macro is lowered by
   the codegen backend. cg_clif has partial `asm!` support
   (improving over time). Our kernel uses `asm!` extensively
   (`kernel-x86_64/src/context.rs` etc.) — but the kernel is
   cross-built, not self-built, so it's not blocked. User programs
   that want `asm!` and would be self-built on Semantic OS might
   hit limits. Audit: when cg_clif is up, try compiling our
   `user-programs/` crates with it; the ones that use `asm!`
   (probably none currently) would be canaries.

7. **Test infrastructure for the M26 smoke test.** We have boot-time
   DEMOs (numbered, `PASS:`/`FAIL:`) but no way to invoke `rustc`
   from a DEMO yet. Likely workflow: M26 ships a DEMO 38 that
   spawns `rustc` against a hardcoded source file in ramfs, waits
   for completion via `SYS_WAIT`, and verifies the output ELF
   loads. Compositional with the std shim.

8. **License-compatibility on the LLVM-exception line in
   Apache-2.0.** Cranelift's "Apache-2.0 WITH LLVM-exception" is
   stricter than vanilla Apache for downstream modifications.
   Effectively a patent grant + special permission to combine with
   GPL code. We're not GPL-licensing anything; the modification
   restrictions are well within what we'd do anyway. Document in
   the project root LICENSE summary when we eventually write one.
   Not blocking.

9. **Bootstrap chain audit.** Eventually someone is going to ask
   "how do we know the rustc binary we shipped wasn't tampered
   with?" The full bootstrap chain is: host rustc (binary from
   Rust Foundation) → cross-build rustc-for-semos → put on disk →
   boot Semantic OS → use rustc-for-semos + cg_clif to rebuild
   itself. Each step is verifiable if we publish hashes; nothing
   needs new work for M26. Worth noting in the M28 acceptance
   criteria though.

10. **What about a debugger?** rustc emits DWARF (lower quality
    under cg_clif but present). A useful debugger on Semantic OS
    needs symbol lookup + breakpoint + step. None exist today. Not
    M26's problem but worth surfacing — without a debugger,
    debugging a self-built rustc when it crashes is going to mean
    serial-console printk archaeology.

---

## 8. Estimated work pipeline (NOT timelines)

Per ROADMAP's "tracked as a research project on AI-assisted porting":

| Chunk | Iteration shape |
|---|---|
| Source vendoring | One-shot. Mechanical. ~1 agent session if network/cargo accessible. |
| Cranelift no-std build | Iterative. Each cfg-gate patch is small but discovery is "try to build → see error → patch → repeat." Order of 5-15 build attempts before clean. |
| cg_clif build | Mostly cross-build infrastructure. Probably hits 1-2 hard rustc-API-changed walls; budget for 1-2 sessions of upstream-issue archaeology. |
| Target tuple addition | Small. ~50 LOC of rustc fork-and-PR-style work. |
| Linker (wild) | Unknown. wild itself is alpha. If it doesn't work first try, switching to homegrown linker is multi-session. |
| First end-to-end "hello world" | Iterative debugging across the std shim ↔ cg_clif ↔ wild interaction surface. Probably the longest single phase. |

The biggest unknowns are the no-std build effort (chunk 2) and the
linker (chunk 6). Both could complete quickly or could each be
their own week-long detour. The "no timelines" framing in ROADMAP
exists exactly because of this.

---

## 9. References

- Cranelift main docs: <https://docs.rs/cranelift/>
- Cranelift IR reference: <https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/ir.md>
- ISLE language guide: <https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/isle-language-reference.md>
- cg_clif README: <https://github.com/rust-lang/rustc_codegen_cranelift/blob/master/Readme.md>
- cg_clif design doc: <https://github.com/rust-lang/rustc_codegen_cranelift/blob/master/docs/design.md>
- regalloc2 paper / repo: <https://github.com/bytecodealliance/regalloc2>
- rustc dev guide on codegen backends: <https://rustc-dev-guide.rust-lang.org/backend/backend-agnostic.html>
- Our roadmap entry: [`ROADMAP.md` § Phase 14 M26](ROADMAP.md#m26--cranelift-backend-integration)
- Companion std shim spec: [`STD_SHIM_SURFACE.md`](STD_SHIM_SURFACE.md)
- Vendor notes (placeholders for now):
  - `kernel-core/vendor/cranelift/VENDOR_NOTE.md`
  - `kernel-core/vendor/rustc_codegen_cranelift/VENDOR_NOTE.md`
