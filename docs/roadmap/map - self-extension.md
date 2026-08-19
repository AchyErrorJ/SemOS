# Roadmap — Self-Extension (the thesis core)

> Part of the [Master Roadmap](../MASTER_ROADMAP.md). Sibling themes:
> [networking](map%20-%20networking.md) · [phone](map%20-%20phone.md) · [gpu](map%20-%20gpu.md) ·
> [platform](map%20-%20platform.md). Historical log: [ROADMAP.md](../ROADMAP.md).

The headline: **an LLM agent writes its own modules, compiles them on the machine,
and loads them into the running system — with the security tiers as the capability
fence on agent-written code.** This theme owns on-device compilation, the module
loader, the package manager, and the self-rebuild capstone.

**Essential reading for any agent-authored code:**
[`semos-security-thesis.md`](../semos-security-thesis.md) (the security posture)
and [`provenance-commitment.md`](../provenance-commitment.md) (how authorship /
trust is tracked). Every new milestone answers the four surface questions before
work starts: new syscall? smallest shape? capability check? blast radius?

---

## M27 — rustc on SemOS (self-hosting) — ~80%, compile+run WORKS

The full upstream `rustc` (incl. the **Cranelift codegen backend**, ported to
`no_std`/the SemOS target) builds for SemOS and runs in Ring 3. On bare metal
(2026-06-15, DEMO 80) it took `hello.rs` through the **entire** pipeline — parse →
expand → typeck/borrowck → Cranelift codegen → ELF, reading `*.rlib` from a
disk-staged sysroot blob via `SYS_SYSROOT_READ` — and the program ran, `[exit]
code=0`, **control returned to sem-sh** (the post-compile freeze is resolved).

**Remaining:** sysroot/`.rlib` polish; the 86 MB `semos-rustc` `include_bytes!` is
**re-stubbed** because the 102 MB kernel it produced never boots (see the
kernel-size memory) — DEMO 80 needs the disk/ramdisk load path, not a baked-in
blob. Design notes: [`SELF_HOSTING_PLAN.md`](../SELF_HOSTING_PLAN.md),
[`M27_DISK_SYSROOT_DESIGN.md`](../M27_DISK_SYSROOT_DESIGN.md).

`semos-std` surface: `#[global_allocator]`, `io::{Read,Write,Seek}`,
`fs::{File,rename}`, `env`, `sync::{Mutex,Once}`, `thread::spawn + JoinHandle<T>`,
`process::Command`, `net::TcpStream`, `time::{Instant,Duration}`,
`path::{Path,PathBuf}`. **Build at opt-level=0 only** — any optimization
miscompiles the syscall path (underlying codegen bug still open).

---

## Module / Loader keystone — DONE 2026-06-15

The hardcoded `/bin` spawn table is **removed** → any ramfs/namespace ELF runs by
name (`spawn_namespace_elf` + `$PATH`), tier-scoped. The command "table" is **data,
not code**, so it updates live AND survives a kernel rebuild for free.

**M1 hello loop — DONE 2026-08-19 (QEMU):** `--features autocompile` boots to `demo80_autocompile`, which compiles `/hello.rs` with on-device semos-rustc (disk-blob sysroot, .rlib), spawns the ELF via `sem-sh -c` **fenced at tier 0**, and verifies its captured stdout byte-for-byte (`[DEMO 80] PASS: M1 hello loop`).

**Remaining for the headline demo:** an agent tool that drops a compiled ELF at
`/apps/<name>` and spawns it at **tier 0**; then the demo — "ask the agent to add a
`greet` command, it works seconds later, the kernel never rebuilt." The tier-0
fence + `SYS_VOUCH`/`SYS_VOUCHES` (console-only elevation, bytes-bound) are already
in the kernel. See `project_semos_module_loader`,
[`VOUCH_MECHANISM_DESIGN_2026-06-15.md`](../VOUCH_MECHANISM_DESIGN_2026-06-15.md).

---

## Package Manager + Ecosystem (Phase 22 of the expansion)

`semos install` tools without manual ELF copying. From-scratch: the package
manager is yours; vendored deps are patched + audited.

### M43 — Package manager (`semos-pkg`) `[  ]`
- [ ] `install <crate>` → download from a crates.io mirror, compile, install to `/apps/`
- [ ] `remove` / `update`; dependency resolution (DAG, not full cargo resolver in v1)
- [ ] DEMO 89: `semos install ripgrep` → downloads, compiles, installs `/bin/rg`

### M44 — crates.io mirror / cache `[  ]`
- [ ] local registry index clone; tarball cache (`/var/cache/crates/`); offline mode
- [ ] DEMO 90: install a cached crate with no network

---

## CAPSTONE — Phase 22: SELF-REBUILD `[  ]`

The whole project points here: an OS that codes, modifies, rebuilds, and reboots
*itself* — safely. **The key split:** live userland extension = no reboot (done,
above); kernel self-rebuild = rebuild image → reboot, made to *feel* live by being
fast + stateful (phone-OTA model), done **without bricking the machine**.

### M22a — Self-host the full kernel build on-device `[  ]`
- [ ] on-device rustc compiles `kernel-core` to a `.rlib` on the machine
- [ ] full kernel image rebuilt on-device from its own source tree
- [ ] rebuilt image byte-reproducible vs the host-built one (or diff understood)

### M22b — A/B boot slots + watchdog rollback `[  ]`
The non-negotiable safety net — a self-modifying OS *will* produce a broken kernel.
- [ ] two slots (A/B) + boot selector preferring the active one; new image → INACTIVE slot
- [ ] watchdog: B must write a "healthy" marker within N seconds or next boot reverts to A
- [ ] DEMO: flash a deliberately broken kernel to B → machine auto-recovers to A

### M22c — Versioned state-migration blob + ABI versioning `[  ]`
- [ ] versioned `system-state` blob the old kernel writes / new kernel migrates
- [ ] **syscall ABI versioned** — adding a syscall bumps the version; older-ABI userland still runs (or is flagged)
- [ ] per-item persistence decided: vouch grants likely RESET on kernel change; `/apps` tools persist

### M22d — Human-vouched kernel promotion `[  ]`
A self-rebuilt kernel is the ultimate "tool the agent made" — same deny-by-default.
- [ ] agent-built kernel boots only provisionally (one-shot / B slot)
- [ ] promotion to default requires an explicit human vouch (review the diff)
- [ ] the agent has NO path to self-promote (mirrors `SYS_VOUCH`'s console-only authority)

**Why this is the capstone:** after M22 the loop closes — live userland extension
(done) + safe, stateful, human-gated kernel rebuild = the agent-native
self-extending sovereign OS, fully realized.
