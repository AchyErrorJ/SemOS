# Self-hosting plan — M25 → M26 → M27

Goal: **rustc runs on SemOS and rebuilds the kernel.** That's the long pole;
this doc breaks it into sessions that each ship a falsifiable next-thing.

The path: M25 (`semos-std` Rust standard-library shim) → M26 (Cranelift
backend → produces working ELFs) → M27 (a `rustc` binary that produces a
runnable hello-world from source).

---

## Where we are (2026-05-28)

### M25 — `semos-std` already lands the core surface

Already wired (DEMOs 29–32 + the editor + sem-sh all build against this):

| Surface | Module | Notes |
|---|---|---|
| `#[global_allocator]` + `Vec`/`String`/`Box` | `alloc_impl.rs` | bump → kernel `SYS_HEAP_ALLOC` |
| `io::{Read, Write}` | `io.rs` | over `SYS_READ` / `SYS_WRITE` |
| `fs::File` + `OpenOptions` | `fs.rs` | path-FD-keyed `SYS_OPEN` family |
| `env` | `env.rs` | inherited from spawn, per-process |
| `sync::{Mutex, Once}` | `sync.rs` | spin-based; `Condvar`/`RwLock` not yet |
| `thread::spawn` + `JoinHandle<T>` | `thread.rs` | `SYS_THREAD_SPAWN`/`JOIN` |
| `process::Command` (spawn + wait) | `process.rs` | `SYS_SPAWN` + `SYS_WAIT` |
| `net::TcpStream` | `net.rs` | over kernel smoltcp |
| **`time::{Instant, Duration}`** | **`time.rs`** | **new this session — over `SYS_TIME`** |

### Still missing in `semos-std`
- `sync::Condvar`, `sync::RwLock` — futex-based; `SYS_FUTEX_WAIT/WAKE`
  exist, just needs wiring.
- `std::path::Path`/`PathBuf` lexical impl.
- `std::collections::HashMap` (need a portable hasher — `ahash` no_std works).
- `std::sync::mpsc` channels — built on Mutex + Condvar.

### M26 — Cranelift
Per ROADMAP: prep done (placeholders + briefs); vendoring not yet.

### M27 — rustc on SemOS
Not started. Depends on M26 + a more complete `semos-std`.

---

## Constraint we have to plan around

**opt-level=0 only.** Every `semos-std` consumer must build with
`opt-level=0` — any optimization miscompiles the syscall path (#54). The
underlying codegen bug is still open. This affects M27 directly: a release
rustc build won't run on SemOS until #54 is fixed. **Tier-1 work item.**

---

## Concrete next sessions

### Session A — finish the `semos-std` surface (~1–2 sessions)
Adds the last pieces of stdlib breadth so a real Rust crate's `use std::*` is
mostly satisfied. Each is a single file in `user-programs/std-shim/src/`:

1. **`std::sync::Condvar`** + **`std::sync::RwLock`** — futex-backed.
   - Condvar: `wait(MutexGuard)` releases the mutex and parks on `SYS_FUTEX_WAIT`;
     `notify_one`/`notify_all` issue `SYS_FUTEX_WAKE`.
   - RwLock: u32 atom (high bit = writer; low bits = reader count);
     contention falls through to futex.
2. **`std::path::{Path, PathBuf}`** (lexical only). Don't touch fs metadata;
   `Path::is_file` etc. should defer to `fs::metadata`, which we don't yet
   have — skip those for now.
3. **`std::collections::HashMap`** — either pull the `hashbrown` crate
   (no_std-capable; it's what std uses under the hood) or write a tiny
   linear-probe map. `hashbrown` is the right choice; it just needs a fixed
   hasher (it ships `DefaultHashBuilder`).
4. **`std::sync::mpsc`** — `channel()` returns `(Sender, Receiver)` over a
   Mutex<VecDeque<T>> + Condvar.

DEMO per piece is mechanical: spawn two threads, one produces, one consumes,
verify ordering / wakeups / shared-state correctness.

### Session B — Cranelift vendoring (~1 session)
Two crates to drop into `vendor/`:
- `cranelift-codegen` + `cranelift-frontend` (the JIT/AOT compiler core).
- `cranelift-object` (writes .o files; needed for AOT).

These build on stable Rust with `no_std` + alloc. The wrinkle: they expect
`std::collections::HashMap` — so Session A's HashMap is a prereq.

Set up: `cargo new vendor/cranelift-codegen`-style placeholder is already
there (per the ROADMAP "Cranelift sources fully vendored — one agent session
in a less-restricted env"). Actually do the vendoring.

### Session C — `cg_clif` (rustc Cranelift backend) (~1 session)
`rustc_codegen_cranelift` is rustc's drop-in alternative to cg_llvm. It's
maintained, smaller than LLVM, and `no_std`-friendly. Vendor it into
`vendor/cg_clif`; patch its build script to find our vendored Cranelift.

### Session D — a tiny rustc-on-SemOS proof-of-concept (~2 sessions)
Build a **minimal** rustc binary (or skip rustc entirely and use a simpler
front-end) that does just `fn main() { let x = 1 + 2; }` → ELF via
cg_clif → ELF runs on SemOS via the existing `process::Command` path.

This is **enormously simpler than full rustc**:
- No proc-macros, no incremental, no parallelism, no LTO.
- Single-file compilation. No external crates.
- Single target: x86_64-unknown-none with our linker script.

The goal is "the toolchain pipeline works end-to-end on SemOS." Full
rustc-builds-rustc is M27 proper and waits for the toolchain to firm up.

### Session E — #54 codegen bug (medium-hard)
The opt-level=0 workaround blocks the eventual self-hosted rustc release
build. Open work: figure out which codegen pass mis-handles the syscall
sequence, file upstream / patch our copy. Could be a session on its own;
could be a multi-session rabbit hole. Worth scoping properly before
committing.

---

## Sequencing relative to hardware

None of M25/26/27 are hardware-gated. They proceed in QEMU. But two
practical hedges:

- **Cranelift on a 32 MB heap.** Our kernel heap is 16 MiB. Cranelift +
  cg_clif may want more. We can either: (a) bump the kernel heap, (b) move
  heap to a `mmap_anon` per-process model where each compile gets its own
  large pool, (c) keep heap small and accept compilations failing on big
  inputs.
- **Disk space.** Self-hosting writes `.o` and `.elf` files. With the FS
  Model A stage-1 cap of 2 MiB per file, big crates won't fit. Stage 2a
  (frame-backed content) lifts this to ~128 MiB per file — likely needed
  before rustc-on-SemOS is realistic. **Cross-cutting prereq.**

---

## What I would actually do next

If we're committing to this track, **Session A** (finish `semos-std`) is the
right next step. Each piece is bounded, each ships a DEMO, and the surface
is what every subsequent session will depend on. Roughly:

1. **`Condvar` + `RwLock`** + DEMOs 69/70.
2. **`PathBuf`** + DEMO 71 (lexical join/parent/extension).
3. **`HashMap`** via vendoring `hashbrown` + DEMO 72.
4. **`mpsc`** + DEMO 73 (producer/consumer ordering).

If we don't immediately do M26/27, this work has independent value: the
editor, agent, and any future native apps all benefit from a richer stdlib.

---

## Note: the typesetter port is a parallel track

MarlOS's typesetter (per the user 2026-05-27) is already built on Windows.
Its port to SemOS depends on a `semos-std` rich enough that it doesn't need
extensive shim work, AND on the framebuffer/font/layout APIs we already
shipped. The Session A list above is essentially the dependency the
typesetter port needs to be tractable. So **finishing `semos-std` unblocks
both self-hosting and the typesetter port simultaneously** — high leverage.
