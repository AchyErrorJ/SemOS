# M27 — Disk-resident sysroot for semos-rustc (DEMO 80)

**Status:** design (chosen over "stage host core + loosen check" and "build core
with vendored rustc" — we design the disk path first, deliberately).

**Goal:** let the SemOS-resident `semos-rustc` find and load `core` (and
`compiler_builtins`) when compiling `/hello.rs`, getting past the
`metadata_cannot_find_crate` wall reached 2026-06-11.

---

## 1. The constraint that shapes everything

`/hello.rs` is `#![no_std] #![no_main]` and references `core::panic::PanicInfo`,
so rustc must load `core`'s metadata during resolution. The numbers:

| crate | `.rlib` | `.rmeta` (metadata only) |
|---|---|---|
| **core** | 58.0 MB | **56.7 MB** |
| alloc | 5.6 MB | 5.1 MB |
| compiler_builtins | 3.9 MB | 1.8 MB |

`core`'s metadata is essentially the whole rlib (generic/inline MIR serialized,
built unoptimized+debug via `build-std`). So **there is no "small rmeta" escape
hatch for core** — we are committed to serving a ~57 MB file.

That collides with three current limits:

- **`include_bytes!` is out.** Kernel image is already 104 MB and at the
  hardware load edge; +57 MB would not boot ([[project_m27_kernel_size_boot_blocker]]).
- **RAM file store is out.** `MAX_FILE_CONTENT` = 2 MiB; files are contiguous
  heap-`Allocated` blobs in a 16 MiB kernel heap. 57 MB is 29× the file cap and
  3.6× the whole kernel heap.
- **The SemOS metadata read path is a stub.** `get_rmeta_metadata_section`
  (target_os="none") returns `Err("not supported yet")`; `get_rlib_metadata`
  for SemOS isn't implemented; `MetadataBlob` backing assumes `Mmap`.

## 2. The key insight (what makes this tractable)

The 57 MB is read by the **user process** (`semos-rustc`), not held in kernel
RAM. `get_metadata_section` → `loader.get_rlib_metadata(filename)` → reads the
file via `semos_std::fs` (`SYS_OPEN`/`SYS_READ`) into the **process heap** —
which now lives at **4 GiB with ~508 GB of headroom** (the iter-8 heap-base fix).
So a 57 MB `Vec<u8>` in `semos-rustc` is fine.

Therefore the kernel never needs 57 MB of contiguous heap. It only needs to
**serve the bytes of a large read-only file** through `SYS_READ`. That reduces
the problem to: store the rlibs somewhere the kernel can stream from.

## 3. Architecture (three layers + a host staging step)

```
HOST (build/flash time)                 SemOS (boot/run time)
────────────────────                    ─────────────────────
core.rlib, compiler_builtins.rlib  ──►  [A] sysroot blob on disk
  packed into a sysroot blob              (raw region of the boot disk)
                                              │  kernel reads sectors
                                              ▼
                                        [B] /sysroot/.../lib*.rlib
                                            served as read-only
                                            disk-backed namespace files
                                              │  SYS_OPEN / SYS_READ
                                              ▼
                                        [C] semos-rustc metadata loader
                                            get_rlib_metadata: read file,
                                            parse ar, extract lib.rmeta,
                                            MetadataBlob::new(Vec), and a
                                            loosened version check
```

### Layer A — physical storage & host staging

The rlibs live in a **dedicated read-only region of the boot disk image** (one
USB stick keeps the T540p single-boot workflow). Decision point inside this
layer (see §6 Q1): a custom GPT partition vs. a raw blob appended after the
kernel partition.

- **Blob format:** a tiny header table — magic, count, then
  `(name, offset, len)` records, then concatenated file bytes, all
  sector-aligned. No filesystem; the kernel maps names → (offset,len).
- **Host build step:** a script (or extend `x86_64-runner`) that takes the
  freshly-built `core.rlib` + `compiler_builtins.rlib` from the build tree and
  writes the blob region into the `.img` after the bootloader wraps the kernel.
- **Sysroot layout the blob represents:** rustc searches
  `<sysroot>/lib/rustlib/<target>/lib/lib<crate>-<hash>.rlib`. `default_sysroot()`
  already returns `/sysroot`, so the namespace files must appear at
  `/sysroot/lib/rustlib/x86_64-unknown-none/lib/libcore-<hash>.rlib` etc.
  (`x86_64-unknown-none` is a *built-in* target — no target-spec JSON needed.)

### Layer B — kernel: serve large read-only disk-backed files

Add a read-only, disk-backed file kind so `/sysroot/...` reads stream from the
blob region instead of RAM. Two implementation shapes (see §6 Q2):

- **B1 (stream from disk):** a new `ObjectContent::DiskBlob { lba, len }` (or a
  namespace entry kind) whose `SYS_READ` copies sectors on demand from the boot
  block device. No large RAM cost. Closest to "Model B" but read-only only.
- **B2 (frame-backed RAM):** at boot, read the blob into a new
  `ObjectContent::FrameBacked` (contiguous PT-pool frames, ~14K frames for
  57 MB; pool is 131K). This is the roadmap's deferred "FS stage 2a". Simpler
  to bolt onto the existing `as_bytes()`/`SYS_READ` path, costs 57 MB RAM.

Either way: register the three files in the path namespace at boot pointing at
their blob slices, and make `handle_open_path` + `handle_fread` resolve them.

### Layer C — semos-rustc metadata loader (target_os="none")

1. **`get_rlib_metadata` (SemOS impl):** `semos_std::fs::read(filename)` the
   whole rlib into a `Vec<u8>`, parse the `ar` archive (the rlib is a Unix `ar`;
   first member is `lib.rmeta`), slice out the `lib.rmeta` bytes, wrap as the
   `MetadataBlob` backing (`OwnedSlice`/`Mmap`-from-`Vec`). Needs
   `rustc_data_structures::memmap::Mmap`-from-`Vec` support on SemOS.
2. **`get_rmeta_metadata_section` (SemOS):** same, minus the ar step (read whole
   file). Replaces the current `Err` stub.
3. **Loosen `MetadataBlob::check_compatibility` (decoder.rs:730):** on SemOS,
   skip the rustc-version-string comparison (still require the `METADATA_HEADER`
   magic, which guards schema). Required because the staged `core.rlib` was built
   by the host nightly, whose version string ≠ `semos-rustc`'s `CFG_VERSION`.
   **Risk (Q3):** this only works if the metadata *schema* matches — i.e. the
   vendored rustc source == the nightly-2025-12-23 that built `core`. If schemas
   differ, decoding produces garbage; fallback is to build `core` with the
   vendored rustc as a host tool (the option-B path).
4. Verify `semos_std::fs::read` loops `SYS_READ` to EOF into a heap `Vec` and
   handles a 57 MB file (it allocates on the 4 GiB heap — fine).

## 4. Build & flash flow (target end state)

```
1. build user-programs/semos-rustc        (unchanged)
2. build kernel-x86_64                     (bakes semos-rustc)
3. x86_64-runner: wrap kernel → .img
4. NEW: pack core.rlib + compiler_builtins.rlib → sysroot blob,
        write into the .img's sysroot region
5. flash .img → boot → semos-rustc /hello.rs -o /tmp/hello.elf
```

## 5. Implementation order (each a bootable checkpoint)

1. **C3 first (cheap, de-risks everything):** loosen `check_compatibility` and
   confirm — by staging *just* the 1.8 MB `compiler_builtins.rlib` through the
   *existing* 2 MiB RAM file path (`include_bytes!` is fine at 1.8 MB) — that
   the SemOS `get_rlib_metadata` + ar-parse + version-skip + **metadata decode**
   actually works end-to-end on a small crate. **This proves the schema-compat
   risk (Q3) before we build any disk plumbing.** If a 1.8 MB host-built
   compiler_builtins decodes in semos-rustc, core will too.
2. **Layer B** (disk-backed or frame-backed large file) once C3 proves decode.
3. **Layer A** host staging + the .img sysroot region.
4. Wire `/sysroot/...` namespace registration at boot; run the full compile.

## 6. Open decisions

- **Q1 — disk region shape:** custom GPT partition vs. raw appended region.
  Leaning raw appended region read via the boot block device (less GPT plumbing).
- **Q2 — B1 stream-from-disk vs. B2 frame-backed RAM.** Leaning **B2** for the
  first green compile (simpler, reuses `as_bytes()`/`SYS_READ`; 57 MB RAM is
  acceptable), revisit B1 if RAM pressure shows up.
- **Q3 — metadata schema compatibility** of host-built `core` vs. vendored
  rustc. De-risked first via step C3. If it fails, pivot to building `core`
  with the vendored rustc as a host tool.
- **Q4 — shrink core?** Rebuild `core` via `build-std` with `debug=0` /
  release to cut the 57 MB (metadata still carries MIR, so savings are
  bounded). Worth measuring once C3 passes; not on the critical path.

## 6b. C3 RESULT (2026-06-11) — host rlibs are INCOMPATIBLE; pivot to host-tool build

Ran the C3 probe (`semos-rustc --c3-selftest`, embedded 1.8 MB
`compiler_builtins.rmeta`). Outcome:

- `MetadataBlob::new` OK, version-string `String` decoded:
  `found="rustc 1.95.0-nightly (905b92696 2026-01-31)"` vs
  `expected="rustc 1.84.0-semos-m27 …"`.
- **`get_header` decoded `name="concat"`** (should be `compiler_builtins`),
  triple correct (`x86_64-unknown-none`), bools correct.

**Diagnosis (confirmed via decoder.rs:383–403):** symbols are decoded by three
tags; `SYMBOL_PREDEFINED → Symbol::new(index)` resolves a **pre-interned symbol
by table index**. `compiler_builtins` was encoded as `SYMBOL_PREDEFINED(N)`;
index N in semos-rustc's vendored pre-interned table is `concat`. So the
**pre-interned symbol table differs** between the rmeta's compiler and the
vendored source — even though both build *under* nightly-2026-02-01, the vendored
rustc *source* is a different rustc version than the toolchain's bundled rustc.
`core`'s metadata is saturated with predefined symbols → host rlibs are unusable,
and there is no decode-side fix (index→symbol is lossy without the encoder table).

**Decision:** build `core`/`compiler_builtins` with a rustc that shares
semos-rustc's exact symbol table = **build the vendored rustc source as a HOST
tool** (option B). This is also the project's real cross-compiler.

## 6c. Option B plan — vendored rustc as a host tool

1. **Host driver crate** (`user-programs/rustc-host` or a host bin/feature):
   standard `fn main()` → `rustc_driver_impl::run_compiler(args)`, host target
   (`x86_64-pc-windows-msvc`), real std (cfg(target_os="none") gates OFF), its
   own `.cargo/config` (NOT semos-rustc's target-none one), `RUSTC_BOOTSTRAP=1`
   + the same `CFG_*` envs so symbol tables/versions match the SemOS build.
2. **Feasibility gate:** the vendored crates have only ever been built for
   target-none; the `cfg(not(target_os="none"))` (host) paths may have bitrotted.
   Probe foundational crates for host first; fix cfg/host-path breakage as it
   surfaces (expected to be the bulk of the work).
3. **Codegen backend:** reuse cg_clif on host (same `make_codegen_backend` trick
   as semos-rustc) — avoids needing rustc_codegen_llvm in the vendored tree.
4. **Produce the rlibs:** drive the host rustc (directly or via
   `RUSTC=… cargo build -Z build-std=core,compiler_builtins --target
   x86_64-unknown-none`) against the rust-src core source → `libcore.rlib` +
   `libcompiler_builtins.rlib` with **matching** metadata.
5. Stage those on disk and resume Layers A/B (now safe — schema guaranteed).

## 7. What we are NOT doing (scope guard)

- No general writable disk filesystem (this is read-only sysroot only).
- No proc-macros, no dylib crates, no multi-target sysroot.
- No linking of a real `core` into the output — `/hello.rs` calls nothing in
  `core`, so only metadata (for name resolution) is needed; codegen/link is the
  next milestone after this wall falls.

## 8. IMPLEMENTED — disk-staged c3-selftest (2026-06-12)

Layers A + B are wired for the c3-selftest. Decision (Q1/Q2): **raw blob on a
SATA/AHCI disk**, kernel reads it via the existing `Sata` `BlockDevice`. We pack
the host-built `.rmeta` (not `.rlib`) — the c3 probe only needs the metadata
blob, which avoids no_std `ar` parsing.

**Layer A — `tools/pack-sysroot-blob.py`:** writes a sector-aligned raw image:
header sector (`SEMSYSR1` magic, count, then `(name[64], lba u64, len u64)`
records) + each file's bytes at its LBA.

**Layer B — `kernel-core/src/sysroot_blob.rs`:** at boot (after AHCI registers)
`probe()` reads LBA 0 of `sata0`; on a magic match it caches the file table.
Two syscalls stream files without holding them in kernel RAM:
`SYS_SYSROOT_INFO=120` (idx → name + len) and `SYS_SYSROOT_READ=121`
(idx, offset, buf, len → bytes; loops a 32 KiB static scratch over `read_blocks`).

**c3-selftest (`semos-rustc --c3-selftest`):** probes the embedded
compiler_builtins.rmeta (RAM path) AND enumerates the disk blob, streaming each
`.rmeta` (incl. the ~57 MB `libcore`) in 64 KiB chunks → `semos_c3_probe`.

### Build + run

```sh
# 1. build the sysroot (host): see user-programs/rustc-host/build-core.sh
#    → libcore-<hash>.rmeta + libcompiler_builtins-<hash>.rmeta
# 2. pack the blob
python tools/pack-sysroot-blob.py sysroot.img \
  libcore-<hash>.rmeta=<deps>/libcore-<hash>.rmeta \
  libcompiler_builtins-<hash>.rmeta=<deps>/libcompiler_builtins-<hash>.rmeta
# 3. build kernel image (bakes semos-rustc): kernel-x86_64 build, then x86_64-runner
```

**QEMU** (attach the blob as a 2nd AHCI disk; needs `-cpu max` for RDRAND — the
default CPU aborts at the TLS RNG check). Note the *full* image is too big to
boot under QEMU-BIOS, but a semos-rustc-stubbed kernel boots and validates
`probe()`:

```sh
qemu-system-x86_64 -cpu max -m 2048 \
  -drive format=raw,file=<kernel-bios.img> \
  -drive id=sysdisk,file=sysroot.img,if=none,format=raw \
  -device ich9-ahci,id=ahci -device ide-hd,drive=sysdisk,bus=ahci.0 \
  -serial stdio -display none
```

**Hardware (T540p):** flash the kernel image to the boot USB; write `sysroot.img`
to the internal SATA disk (LBA 0). The kernel's AHCI reads `sata0`; expect
`[sysroot] blob found: 2 file(s)` then, from `--c3-selftest`,
`[c3] get_header DECODED: name="compiler_builtins"` and `name="core"`.

**Smoke-tested 2026-06-12 (QEMU, stubbed kernel):** `probe()` found the AHCI
blob, parsed both records with exact LBAs/sizes. The streaming `read()` + the
~57 MB core decode run only in the full image → validate on hardware.

**Next (DEMO 80 proper):** pack `.rlib` (+ no_std `ar` parse) and register the
files in the `/sysroot/...` namespace so the rustc crate loader finds them, then
compile `/hello.rs`.

## 9. IMPLEMENTED — generated + flashed on Pop!_OS (2026-06-30)

The whole Layer-A staging flow now runs natively on the T540p Pop!_OS
workstation (previously Windows-only). Steps:

1. Build the host driver for Linux:
   `cd user-programs/rustc-host && cargo build --release --target x86_64-unknown-linux-gnu`
   (uses the repo-pinned `nightly-2026-02-01`; the vendored rustc source is what
   matters, not the stock toolchain).
2. Fetch the base library source once:
   `rustup toolchain install nightly-2025-12-23 --component rust-src`.
3. Regenerate the patched sysroot source:
   `bash user-programs/rustc-host/prepare-sysroot-src.sh nightly-2025-12-23-x86_64-unknown-linux-gnu`.
4. Build `core` + the `compiler_builtins` stub into `.rmeta`:
   `bash user-programs/rustc-host/build-core-linux.sh`
   (Linux port of `build-core.sh`; the original still documents the Windows paths).
5. Pack the blob:
   `python3 tools/pack-sysroot-blob.py out/sysroot.img libcore-<hash>.rmeta=<deps>/libcore-<hash>.rmeta libcompiler_builtins-<hash>.rmeta=<deps>/libcompiler_builtins-<hash>.rmeta`.
6. Flash to the named GPT partition (NOT LBA 0 of the OS disk):
   `sudo dd if=out/sysroot.img of=/dev/disk/by-partlabel/SEMOS_SYSROOT bs=4M conv=fsync` after
   confirming `PARTLABEL == SEMOS_SYSROOT`.

Result on 2026-06-30: 59,579,904-byte blob (`SEMSYSR1`, 2 files —
`libcore-53344cc650ffcdf9.rmeta` + `libcompiler_builtins-fb74582bf62b1baa.rmeta`)
written to `/dev/sda5` (`SEMOS_SYSROOT`, 4 GiB). The self-test
(`rustc-host` compiles a no_std prelude snippet against the produced core +
compiler_builtins) passed.

### Linux host-build fixes required (this session)

The vendored rustc tree had only ever been built for the SemOS target or
Windows host; a few host-path bits broke on Linux and were fixed:

- `rustc_data_structures::temp_dir`: re-export `TempDir` (`pub use tempfile::TempDir`)
  so `rustc_metadata` can import it on the std host arm.
- `rustc_interface` + `rustc_driver_impl`: dropped dead `#[cfg(target_os = "none")]`
  attributes that were left attached to disabled `PROBE-disabled` comment lines;
  on a host build they cfg-ed out the following real statement.
- `rustc-host/.cargo/config.toml`: dropped the MSVC-only `/STACK` + `/HEAP`
  link-args (they were passed to Linux `cc` and failed the link).
