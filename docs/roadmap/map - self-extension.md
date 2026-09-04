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

**M2 bug fix — DONE 2026-08-19 (QEMU):** same `autocompile` boot now runs `demo83_bugfix` after DEMO 80: seeds `/tmp/agentgen/m2/` with a bug report + buggy `calc.rs`, reproduces the failing selftest, writes the fix, recompiles, verifies byte-exact, then asks the human on serial (`Install /apps/calc? [y/N]`, fail-fast on n/timeout) before an atomic `/apps/.staging` rename install and a bare-name tier-0 smoke run. Approve and deny paths both PASS.

**M3 feature add — DONE 2026-08-21 (QEMU):** `demo87_featureadd` runs after DEMO 83 in the same `autocompile` boot: seeds `/tmp/agentgen/m3/` with a feature spec, `wc.rs`, and sample data; compiles `wc` on-device; verifies it in isolation (byte-exact `3 15 79` against kernel-computed counts — the first guest program to read a namespace file, via new `sys_open`/`sys_fread`/`sys_close` stubs in aot_semos's built-in stub table); then the serial approval gate (`Install /apps/wc? [y/N]`, fail-fast) and the same atomic `/apps/.staging` install + bare-name tier-0 smoke as M2. Approve and deny paths both PASS. Getting there surfaced two real bugs, both fixed: (1) `syscall_entry` destroyed the guest's callee-saved `r15` on EVERY syscall — it stashed the user RSP in r15 before saving it, so every syscall returned r15 = user RSP (cg_clif guests keep live values in r15; LLVM std-shim binaries rarely did, which is why it hid for months); fix stashes user RSP in a memory scratch slot instead. (2) the hand-assembled `sys_fread` stub was 7 bytes — one missing imm32 zero turned `mov eax, 12` into `mov eax, 0x0F00000C`.

**M4 self-repair — DONE 2026-08-21 (QEMU):** `demo88_selfrepair` runs after DEMO 87 in the same `autocompile` boot: seeds a "previously approved" `/apps/head1` v1 that traps when its data file is empty, then truncates `/apps/data/motd.txt` to zero bytes — the data change that starts the crashes. The health check sees the kernel fault sentinel `0xFA01FA17` from `kill_current_task` (the crash signal), the agent writes a `crash.log` panic log, reads it back plus the tool source, writes v2 (empty input exits 0 quietly), recompiles on-device, and verifies in isolation: empty input exits 0, live input prints the first line byte-exact, and the installed v1 re-run still crashes (so the repair demonstrably replaces something broken). Then the same fail-fast serial gate as M2/M3 (`Install /apps/head1 (repaired v2)? [y/N]`) — the only human step — and the atomic `/apps/.staging` rename repair + post-repair health check. Approve and deny paths both PASS. Trap engineering: a deliberately-crashing test guest cannot use any raw pointer deref (`read_volatile` or bare `*ptr` pull `panic_fmt`/`panic_null_pointer_dereference` via ub/precondition checks — unlinkable in aot_semos); the working trap is `transmute(1usize)` to `fn()` + call — an indirect call has no instrumentation, so the instruction-fetch #PF kills the task with the sentinel. Also learned: sem-sh propagates the child exit status through an i32, so a 32-bit sentinel comes back sign-extended — compare the low 32 bits.



**Headline demo — DONE 2026-09-05 (QEMU):** `demo93_greet` + the `greet93-test`
two-boot feeder deliver "ask the agent to add a `greet` command, it works seconds
later, the kernel never rebuilt" — with the SemFS persistence beat: the
agent-added command **survives a hard power cycle**. Boot 1: the feeder types
`greet` at sem-sh (`command not found` — the unknown-command beat), then
`selfdev 93`; the demo seeds `/tmp/agentgen/m93/` (feature spec + `greet.rs`),
compiles on-device, verifies byte-exact in isolation, waits at the same fail-fast
serial/TTY approval gate (`Install /apps/greet? [y/N]`), then installs via the
atomic `/apps/.staging` rename and smokes bare `greet` fenced at tier 0. Because
the SemFS journal is write-through, the install is durable the moment the rename
returns; the harness then **hard-kills QEMU mid-session** (no clean shutdown).
Boot 2: the journal replays, `/apps/greet` resolves, the feeder runs it by bare
name and byte-exact checks the greeting (`[DEMO 93] PASS: greet persisted across
hard-kill reboot`), then types `selfdev 80` as a coexistence smoke (`[DEMO 80]
PASS`). Harness: `run-greet93-qemu.sh` (answers the approval gate 'y' over the
serial pipe). Guest source: `user-programs/semos-rustc/test-sources/greet.rs` —
fixed greeting compiled in (no argv: cg_clif lacks the rsp-grab trampoline);
`GREET_EXPECTED` in main.rs must stay byte-identical to its `GREETING`. sem-sh's
`selfdev` builtin now accepts 93.

The tier-0 fence + `SYS_VOUCH`/`SYS_VOUCHES` (console-only elevation, bytes-bound)
are already in the kernel. See `project_semos_module_loader`,
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
