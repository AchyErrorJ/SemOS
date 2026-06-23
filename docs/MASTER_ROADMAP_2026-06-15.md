# Semantic OS — Master Roadmap (combined)

**Dated 2026-06-15.** This document folds seven planning docs into one map so the
whole project fits on one screen:

- `ROADMAP.md` (the canonical changelog + Phases 9–14)
- `semos-post-phase14-roadmap.md` (Phases 15–20: tether → pairing → phone bridge → WiFi)
- `ROADMAP_EXPANSION_PROPOSAL(JUNE26).md` (Phases 15–21: + browser, ARM port, agent infra)
- `IPHONE_SENSOR_OFFLOAD_PLAN.md` (LiDAR/camera-over-tether)
- `M27_DISK_SYSROOT_DESIGN.md` (on-device rustc / self-hosting)
- `KERNEL_SURFACE.md` (firmware/DMA/syscall trust surface)
- `semos-security-thesis.md` (the security posture)

It does **not** replace them — each keeps its detail. This is the index + the
re-framed thesis + the "what's active / what's gated / what's next" view.

---

## The thesis (re-headlined 2026-06-15)

Earlier framing led with the **4-tier LLM security model**. After getting
on-device compilation working, the sharper framing is:

> **An agent-native, self-extending, sovereign OS.** An LLM agent writes its own
> modules, compiles them *on the machine*, and loads them into the running
> system — with the security tiers as the **capability fence** on agent-written
> code. Bare metal, fully owned, one remote dependency (the model) that the dGPU
> path can eventually remove.

Lineage: **Oberon** (self-extension via dynamically loaded modules) × **Genode**
(capability-scoped component isolation), with an LLM in the author's seat. Self-
hosting alone is old (Lisp machines, Smalltalk, Oberon, Unix). The novel part is
the agent closing the write→compile→load loop locally. The security tiers stop
being "security for its own sake" and become the guardrail that makes agent self-
modification safe — which is the only context where they're genuinely interesting.

- **Sovereign** is a *property*, not a separate workstream.
- **Semantic computing** (vector search / semantic objects) is a *tool the agent
  uses*, not a headline.
- **The security tiers already ARE the capability fence** — `current_task_max_tier()`
  gates LLM/semantic/process/namespace syscalls pervasively, and a child can never
  exceed its spawner's clearance. Spawn agent modules at tier 0 → auto-sandboxed.

---

## What you have at the end (the north star)

When the whole map is done, this is the machine — not "an OS with features" but a
kind of computer that doesn't currently exist:

- **Thinks locally** — runs an LLM agent as a first-class operator, with **local
  inference on the dGPU** as the v2 brain, so it can think with no remote API call.
- **Extends itself** — the agent **writes, compiles (on-device), and loads tools
  live** (no reboot; command table is data not code), and **rebuilds its own
  kernel and reboots into it safely** (A/B slots + watchdog rollback + human-
  vouched promotion). Phase 22.
- **Connects independently** — bare-metal **WiFi as primary**, phone as
  peripheral/vault/presence-key/biometric-gate + cellular fallback, full TLS +
  a text-mode browser + search.
- **Senses through the phone** — camera / GPS / mic / Secure-Enclave crypto /
  push, and **LiDAR point-cloud offload** for the design work.
- **Renders + creates** — **iGPU** 3D (the LegibleStudios CAD view), video, games,
  audio; the MarlOS productivity + LegibleStudios design apps on top.
- **Stays safe by construction** — every agent-authored thing is **born powerless
  (tier 0)** until a human vouches it (bytes-bound), from a single tool up to a
  whole kernel; small, from-scratch, auditable; real IOMMU on the ARM targets.
- **Runs where you own it** — ARM port (Apple Silicon) so the portable future is
  M-series Macs with hardware isolation.

**One sentence:** the first OS where **an AI writes the system, runs on hardware
you fully own, and a human holds the only key that grants it power.**

**What it is deliberately NOT:** a Linux replacement (no POSIX, no other people's
binaries — every program is written *for* this OS), a daily driver for normal
users, an AAA-games machine (no Vulkan/Mesa; the NVIDIA card is compute-only), or
"provably secure" (the claim is smallness + auditability + human-gated trust, not
formal proof). These boundaries are the from-scratch commitment, not gaps.

---

## Foundation already shipped (compressed)

| Area | State |
|---|---|
| Core kernel | GDT/TSS, IDT (atomic IRETQ context switch — task#40 family closed), paging + real guard pages, APIC, scheduler, TTF framebuffer console, PCI |
| Storage/FS | VirtIO block, snapshot persistence, path namespace, RTC/wall-clock, FS syscalls, large files (Model A, RAM-resident ≤ ¼ pool) |
| Net + crypto | SHA-256/HMAC/HKDF/X25519/ECDSA-P256/ChaCha20-Poly1305, virtio-net, smoltcp, TcpStream, embedded-tls + SPKI pin, **HTTPS round-trip to api.anthropic.com**, DNS, HTTP chunked |
| Userland | `semos-std` (Vec/String/Box/io/fs/env/threads/Command — opt-level=0 only), **sem-sh** shell (pipes/redirect/$VAR/history/scrollback), TTY + ANSI |
| Agent | Claude Messages API framing + tool dispatch, **live TLS round-trip**, split-pane agent TUI |
| USB | xHCI (incl. Intel CSZ=1) + standalone EHCI, hubs (1-tier cascade), HID kbd, mass-storage SCSI, **iPhone ipheth tether enumerated + bulk data path** |
| Media | HD Audio (DEMO 63), HID gamepad parser |
| Self-hosting (M27) | Full rustc + Cranelift ported to `no_std`/target; **on-device compile of /hello.rs → ELF → run, end-to-end (2026-06-15)**; disk-sysroot staging |

---

## Active threads (2026-06-15)

### A. Bare-metal WiFi (Intel 7260) — Phase B in progress
Online path of choice on the T540p (no ethernet reach; tether blocked). Through
2026-06-15: full firmware bring-up → scan (real SSIDs) → `wifi` shell command →
`wifi connect <n> <pass>` → **Phase A complete** (PHY ctx + MAC ctx + binding +
ADD_STA + time-event all HW-confirmed). **Phase B (first on-air frame TX):** the
`0x90A` off-channel-TX assert is fixed (enable queue before time-event + widen
window). 2026-06-22/23: **association request + WPA2 4-way handshake wired into
`connect()`** — open-auth, assoc-req with RSN IE, data-frame EAPOL extraction, and
EAPOL-Key Msg2/Msg4 TX all implemented; PTK/MIC/RSN-IE crypto already KAT'd.
Current blocker is **`consumed=0` on data queue 1**: the SCD still does not
schedule the `0x1c` TX frame. Attempted fixes: direct-register queue setup
mirroring OpenBSD `iwm_enable_ac_txq`, `SCD_ACT_EN` at bit 19, setting
`SCD_ACTIVE`/`EN_CTRL`, sending `TX_CMD` on data queue 1 (not the host-command
queue), and binding queue 1 to station 0 / TID 8 / FIFO BE via the `SCD_QUEUE_CFG`
(0x1d) host command (response echoes queue 1). Still `SCD_rdptr 0→0`. Next:
verify TFD/TB layout, try a smaller data-queue ring, or inspect the SCD
translation-table entry after `SCD_QUEUE_CFG`. Also re-stubbed the 86 MB
`semos-rustc` include_bytes! because the 102 MB kernel it produced caused
`user_task` page faults and a dead keyboard on boot. After TX flies: auth-resp →
assoc(+RSN IE) → WPA2 4-way → key install → smoltcp NetDevice. NOT QEMU-testable.
See [[semos-wifi]] memory for the blow-by-blow.

### B. Self-extension loop (the thesis keystone) — ~80%
On-device rustc compiles + runs a program (2026-06-15). Module/loader keystone
landed: hardcoded `/bin` spawn table removed → **any ramfs/namespace ELF runs by
name**, tier-scoped. Remaining: agent tool to drop a compiled ELF at `/apps/<name>`
+ spawn at tier 0; then the demo — "ask the agent to add a `greet` command, it
works seconds later, kernel never rebuilt." (Parallel agent finishing the last
20% of the compile path.) See `M27_DISK_SYSROOT_DESIGN.md`, `project_semos_module_loader`.

### C. Phone symbiosis — tether landed, capabilities next
iPhone enumerated, bulk pipe up (`ipheth0`, static 172.20.10.x). Next is the
trust bootstrap (QR pairing) + companion-app capabilities (crypto/identity/
camera/GPS/mic/push) and the **sensor-offload** angle (LiDAR/point-cloud over the
existing byte pipe). Phone-as-vault / phone-as-presence-key for WiFi sign-in fits
here. See `semos-post-phase14-roadmap.md` Phases 16–19, `IPHONE_SENSOR_OFFLOAD_PLAN.md`.

---

## Phase map (synthesized, all sources)

```
NETWORKING & ONLINE
  Phase 15  USB tethering ............ ipheth landed; M53 real-world TLS / M54 first session NEXT
  Phase 20  Bare-metal WiFi .......... ACTIVE (Phase B frame TX); WPA2 crypto + RSN IE built
  Phase 17  Layer-4 phone bridge ..... socket-forward over paired channel (cellular cost solved)

SELF-EXTENSION (thesis core)
  M27       rustc-on-SemOS ........... compile+run works; sysroot/.rlib polish (~80%)
  Loader    module/loader ............ arbitrary-name spawn DONE; agent /apps drop + tier-0 sandbox next
  (v2)      resident service-modules . IPC (SYS_CAP_REGISTER/INVOKE) — deferred until command-modules demo lands

PHONE SYMBIOSIS
  Phase 16  QR-code pairing .......... trust bootstrap
  Phase 18  companion capabilities ... crypto/identity/camera/GPS/mic/push
  Sensor    LiDAR/point-cloud offload  over the existing tether byte pipe
  Phase 19  native Swift bridge ...... when a Mac arrives

GPU  (hardware-gated; the T540p/W540 has Intel iGPU + discrete NVIDIA Quadro)
  Phase 11  M14 iGPU rendering ....... Iris Xe / HD 4600 — CAD view, video, games (i915 reference)
  Phase 12  M18 NVIDIA dGPU COMPUTE .. PTX submission → LOCAL LLM inference (removes the remote dependency → fully sovereign). "tinygrad-NV style", no graphics driver.
            ** The T540p's dGPU is a GeForce GT 740M = Kepler GK208, CUDA compute
               capability 3.0/3.5. This is PRE-GSP (GSP firmware is Turing+, 2018):
               Kepler boots via the older falcon/PMU model that nouveau documents
               well — NO GSP upload needed. So from-scratch compute on THIS card is
               more tractable than a modern card (but SM 3.x, ~384 cores, ~2 GB —
               small-model / single-layer scale, not a big LLM). Offline-doable now:
               PCI/MMIO bring-up design, falcon/PMU boot study, PFIFO channel +
               pushbuffer model, SM 3.x ISA (SASS) / PTX-for-Kepler, the matmul
               kernels a tiny model needs. Bring-up validation needs the machine.

PLATFORM
  Phase 19* information access ....... HTTP client, HTML parser, search, text browser, web_search tool
  Phase 20* ARM port ................ ACTIVE offline: kernel-aarch64 boots in QEMU `-M virt`, kernel-core + scheduler + Platform + page-table allocator + frame allocator all RUN; next = Ring-3 user spawn / SVC
  Phase 21* advanced agent infra .... context-window mgmt, multi-file edit, Claude Code parity
  Media     M17 H.264 decode; M15 HDA✅; M16 gamepad✅

APPS (Path-B convergence with MarlOS productivity + LegibleStudios design)
  utilities / productivity / creativity — kernel surface spec in app-requirements memory

CAPSTONE
  Phase 22  SELF-REBUILD ............. the OS codes/modifies/rebuilds/reboots ITSELF, safely
            M22a self-host full kernel build · M22b A/B slots + watchdog rollback ·
            M22c versioned state-migration + ABI versioning · M22d human-vouched kernel promotion
```
\* numbering differs between the two expansion docs; folded here by theme.

---

## Phase 22 — Self-Rebuild (the capstone) `[  ]`

**The whole project points here:** an OS that codes, modifies, rebuilds, and
reboots *itself* — safely. This is the top of the map; everything else is
foundation for it. Added 2026-06-15 from the live-rebuild design discussion.

**The key architectural split (don't conflate these):**
- **Live userland extension = no reboot.** The agent writes/compiles/runs a tool
  *now*. The command "table" is **data, not code** (resolved from the filesystem
  since the hardcoded `/bin` table was removed 2026-06-15), so it updates live AND
  survives a kernel rebuild for free — a new kernel re-derives it from the
  persisted namespace. **Already working.**
- **Kernel self-rebuild = rebuild image → reboot.** Running kernel code can't be
  hot-swapped for arbitrary structural change (Linux livepatch only does isolated
  functions). The honest, tractable path is rebuild-into-image + reboot, made to
  *feel* live by being fast + stateful (phone-OTA model). This phase is about
  doing that **without bricking the machine.**

### M22a — Self-host the full kernel build on-device `[  ]`
The gate on everything. Today's on-device rustc compiles a 30-byte program;
this is compiling kernel-core + kernel-x86_64 + the dependency tree on the
machine itself. Seed → tree. (Tracks the parallel compiler agent's M27 work.)
**Done when:**
- [ ] on-device rustc compiles `kernel-core` to a `.rlib` on the machine
- [ ] full kernel image rebuilt on-device from its own source tree
- [ ] the rebuilt image is byte-reproducible vs the host-built one (or diff understood)

### M22b — A/B boot slots + watchdog rollback `[  ]`
**The non-negotiable safety mechanism** — a self-modifying OS *will* produce a
broken kernel; this is what makes that survivable instead of fatal. Keep the
last known-good kernel in slot A; write the new one to B; boot B under a
deadline; if B doesn't report healthy, the bootloader reverts to A. Builds on
the existing `idle_with_heartbeat` proof-of-life (the seed of "B is healthy").
**Done when:**
- [ ] two kernel slots on disk (A/B) + a boot selector that prefers the "active" one
- [ ] new image always writes to the INACTIVE slot
- [ ] watchdog: B must write a "healthy" marker within N seconds of boot or the
      next boot falls back to A (and the bad B is marked failed)
- [ ] DEMO: deliberately flash a broken kernel to B → machine auto-recovers to A

### M22c — Versioned state-migration blob + ABI versioning `[  ]`
The "table that needs updating." Most state is filesystem data that just
persists; this is for the kernel-RAM state the new kernel needs from the old.
**Done when:**
- [ ] a versioned `system-state` blob the old kernel writes pre-reboot + the new
      kernel reads/migrates (format version + migration path on bump)
- [ ] **syscall ABI is versioned**: adding a syscall bumps the ABI version; the
      blob records it; userland built against an older ABI still runs (or is
      flagged for recompile) — the syscall numbers become an explicit contract
- [ ] decide per-item what persists: vouch grants likely RESET on a kernel change
      (trust re-earned against new code); installed /apps tools persist (filesystem)

### M22d — Human-vouched kernel promotion `[  ]`
The vouch mechanism, one level up. A self-rebuilt kernel is the ultimate
"tool the agent made" — so the same deny-by-default rule applies: the agent may
build B and boot it *provisionally*, but **promoting B to the permanent default
requires a human reviewing the diff and approving.** Deny-by-default all the way
up to the kernel.
**Done when:**
- [ ] a freshly agent-built kernel boots only provisionally (one-shot / B slot)
- [ ] promotion to default requires an explicit human vouch (review the source diff)
- [ ] the agent has NO path to self-promote a kernel (mirrors `SYS_VOUCH`'s
      console-only authority — see `VOUCH_MECHANISM_DESIGN_2026-06-15.md`)

**Why this is the capstone:** after M22, the loop closes — the OS can extend its
userland live (done), and rebuild its own kernel across a safe, stateful, human-
gated reboot. That is the agent-native self-extending sovereign OS, fully
realized.

---

## What's doable WITHOUT the machine (relevant now — drives dead)

- **Self-extension/module loader** — pure kernel code; boot-test later. ✅ keystone done today.
- **WPA2 4-way crypto** — PTK/EAPOL-MIC + RSN IE, KAT-verified offline. ✅ done today.
- **Assoc/EAPOL frame builders + handshake state machine** — wire `wpa2::ptk`/`eapol_mic` into the connect() Phase B/C path (frame TX still needs HW to validate).
- **dGPU compute groundwork** — PCI/MMIO map design, GSP firmware parsing, PTX/command-submission model, the matmul shapes a small model needs (all designable/testable in pieces offline).
- **ARM HAL** — `kernel-aarch64` now boots, enables MMU, runs `kernel-core`, schedules preemptive kernel tasks via the architecture-independent scheduler, and passes a page-table/allocator self-test in QEMU `-M virt` (no x86 hardware needed). Next offline gate: Ring-3 user spawn + SVC.
- **Browser/HTML parser, agent context-window mgmt** — pure logic, DEMO-testable.
- **Security thesis + KERNEL_SURFACE** doc maintenance.

## What's HARD-gated on the machine (need new boot drives)

- WiFi Phase B TX validation (not QEMU-testable).
- iPhone tether traffic validation on the W540.
- iGPU/dGPU bring-up (real silicon).
- Final metal bring-up / NVMe real-hardware test.

---

## Security posture (KERNEL_SURFACE + thesis, re-stated)

- **Declared trust** for device firmware (iwlwifi 7260, etc.) on x86 without an
  IOMMU — the thesis's stated fallback, not a silent compromise. Real containment
  lands on the ARM targets (Apple DART / ARM SMMU).
- **The tiers are the capability fence** for agent-authored code (see thesis
  reframe above). v1 uses tier-0 sandboxing; a finer per-syscall `CapSet` is a
  later refinement, not required for the first self-extension demo.
- **USB is the contained cross-machine path** (USB devices aren't bus masters) —
  the reason a USB-WiFi dongle is the eventual portable, IOMMU-free answer.

---

## Open decisions (carry into the next sessions)

1. **dGPU compute vs WiFi-first for "sovereign":** local LLM inference on the
   NVIDIA Quadro removes the last remote dependency. Big lift (GSP fw, PTX), but
   it's the most thesis-defining hardware direction. Pursue now (offline design)
   or after WiFi lands?
2. **Self-extension demo as the headline deliverable:** finish the "agent adds a
   command" loop end-to-end before broadening? It's the single clearest proof of
   the re-framed thesis.
3. **ARM port timing:** the `kernel-aarch64` HAL skeleton is now **running offline in QEMU** (boot, MMU, scheduler, allocator, page tables). Decision is no longer "if" but **when to sequence Ring-3 spawn/SVC/syscalls** relative to WiFi/self-extension.
4. **Phone-as-vault for WiFi:** wire the pairing/presence layer before or after
   bare-metal WiFi connects?

See each source doc for the per-phase "done when" checklists.
