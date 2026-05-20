# What is Semantic OS?

A one-page orientation. The [README](../README.md) argues the *why*,
[architecture.md](architecture.md) maps the *what-is-where*, and
[ROADMAP.md](ROADMAP.md) tracks the *what's-next*. This file is the
"explain it to me from scratch" version.

## In one sentence

Semantic OS (SemOS) is a from-scratch, bare-metal **x86_64 kernel
written in Rust** that moves LLM data-leak protection out of fragile
user-space sandboxes and **enforces it in Ring 0**, at the same boundary
that enforces memory protection.

## The core thesis

Conventional OSes hand a program raw bytes and trust it not to leak them
to an LLM. SemOS inverts that. It replaces the file abstraction with
**semantic objects**:

- Addressed by a **SUID** (a 128-bit semantic unique ID), not a path.
- Tagged with a **security tier**: `Public | Internal | Sensitive | Secret`.
- Accessed through syscalls that distinguish *who's asking* from *what
  the bytes are for*.

When a task asks for an object's bytes directly it gets them verbatim
(subject to tier clearance). When it asks for an **LLM-bound view** of
the *same* object, the kernel applies tier-based redaction *before* the
bytes ever leave Ring 0:

```
DIRECT READ:  Sensitive: email=alice@example.com card=4111-1111-1111-1111
LLM CONTEXT:  Sensitive: email=[EMAIL] card=[CARD]
```

Same caller, same buffer, two views — chosen by the kernel from the
declared downstream use, not from the caller's capabilities. User code
can't bypass it because the policy lives in privileged code.

## What it actually is, concretely

- A real kernel that boots on QEMU (BIOS + UEFI) via the
  `bootloader-0.11` crate: real→protected→long mode, GDT/TSS, IDT,
  4-level paging, Local APIC timer, PCI/VirtIO, a framebuffer console.
- `no_std`, no host OS underneath. ~Two Cargo crates:
  `kernel-core` (platform-independent policy) talks to hardware only
  through a `Platform` trait implemented by `kernel-x86_64`.
- Ring 3 user programs are **real Rust ELF binaries** (in
  `user-programs/`), loaded by the kernel's own ELF loader and run
  unprivileged — they reach the kernel only through `SYSCALL`.
- Correctness is demonstrated by a battery of **boot-time DEMOs**
  (31 as of this writing) printed to the serial console; a green run is
  the regression signal.

## What works today (capability tour)

- **Semantic-object security model** — 4 tiers, per-tier memory pools,
  context-aware redaction, capability + user-identity checks. The
  headline direct-vs-LLM-view demo runs both in-kernel and from a Ring 3
  binary.
- **Crypto + networking + TLS** — hand-verified SHA-256, HMAC, HKDF,
  X25519, ECDSA-P256, ChaCha20-Poly1305 (all KAT'd against spec
  vectors); virtio-net + smoltcp; embedded-tls vendored; SPKI cert
  pinning. Milestone: a real **outbound HTTPS round-trip to
  api.anthropic.com** from bare metal.
- **Persistent filesystem** — hierarchical `/paths` over SUID-addressed
  objects, snapshot persistence to a virtio block device with verified
  cross-boot survival, and a `SYS_FS_*` syscall surface
  (open/read/write/mkdir/unlink/readdir/rename/truncate/statx/fsync).
- **USB** — xHCI controller bring-up + HID keyboard enumeration.
- **RTC / wall-clock**, hardware RNG (RDRAND), a free-list kernel heap,
  per-process env + CWD, argv/envp passthrough.
- **Threading & sync** — `SYS_THREAD_SPAWN`/`JOIN` (kernel-mode and
  Ring-3 same-address-space), futex (`SYS_FUTEX_WAIT`/`WAKE`),
  non-blocking child wait.
- **A `std` shim** (`user-programs/std-shim`, crate `semos-std`): a
  growing subset of Rust's `std` mapped onto SemOS syscalls — `print!`,
  a `#[global_allocator]` (Vec/String/Box), `io::{Read,Write}`,
  `fs::File`, `env`, `sync::{Mutex,Once}`, `thread::spawn`/`JoinHandle`.

## Where it's going

The project is organized in phases (see ROADMAP). Phases 1–9 (kernel
foundations, security model, crypto/TLS, filesystem, USB) are done.
Phase 14 — **self-hosting compilation** — is the current frontier: get
Rust's `std` satisfied on SemOS (the std-shim above) and adopt
**Cranelift** as the codegen backend, so the OS can eventually compile
code on the metal without an LLVM C++ port. The aspiration is a system
where the security boundary and the development environment are the
same machine.

## How to read the codebase

| You want… | Go to |
|---|---|
| Why it exists | [`README.md`](../README.md) |
| Module/crate map, the Platform trait | [`docs/architecture.md`](architecture.md) |
| Phase-by-phase status & next steps | [`docs/ROADMAP.md`](ROADMAP.md) |
| The std-shim surface & syscall map | [`docs/STD_SHIM_SURFACE.md`](STD_SHIM_SURFACE.md) |
| A full captured boot | [`docs/boot-demo.log`](boot-demo.log) |
| The security policy itself | `kernel-core/src/` (semantic objects, tiers, redaction) |
| Hardware bring-up | `kernel-x86_64/src/` |
| Ring 3 programs | `user-programs/` |

## Status caveat

This is a research kernel, not production software. It targets QEMU
(metal validation on a ThinkPad P1 is a separate track). There are
known structural rough edges being worked — chiefly per-task stack
guard pages and a `-Oz` codegen sensitivity in user binaries — tracked
in the roadmap and task list. The value is in the thesis and the
working end-to-end demonstrations of it, not in hardening.
