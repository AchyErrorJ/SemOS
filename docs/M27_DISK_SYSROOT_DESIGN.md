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

## 7. What we are NOT doing (scope guard)

- No general writable disk filesystem (this is read-only sysroot only).
- No proc-macros, no dylib crates, no multi-target sysroot.
- No linking of a real `core` into the output — `/hello.rs` calls nothing in
  `core`, so only metadata (for name resolution) is needed; codegen/link is the
  next milestone after this wall falls.
