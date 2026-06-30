# Semantic OS — Master Roadmap

**The one roadmap to follow.** This is the index + thesis + active/gated/next view.
Forward detail lives in five themed files under [`roadmap/`](roadmap/); the
historical "what landed, when" log is [`ROADMAP.md`](ROADMAP.md).

| Theme | Owns |
|---|---|
| [roadmap/networking.md](roadmap/map%20-%20networking.md) | USB tether · Layer-4 phone bridge · bare-metal WiFi |
| [roadmap/self-extension.md](roadmap/map%20-%20self-extension.md) | on-device rustc · module loader · package manager · **Phase 22 self-rebuild capstone** |
| [roadmap/phone.md](roadmap/map%20-%20phone.md) | QR pairing · companion capabilities · sensor offload |
| [roadmap/gpu.md](roadmap/map%20-%20gpu.md) | iGPU rendering · dGPU compute (local inference) |
| [roadmap/platform.md](roadmap/map%20-%20platform.md) | ARM port · web/info access · agent infra · Swift bridge · media · utilities |

> **Essential reading for any agent-authored code — read these first:**
> [`semos-security-thesis.md`](semos-security-thesis.md) (the security posture and
> the "Path A: From-Scratch" disciplines) and
> [`provenance-commitment.md`](provenance-commitment.md) (how authorship and trust
> are tracked). Every milestone answers four surface questions before work starts:
> **new syscall? smallest shape? capability check? blast radius?** Supporting docs:
> [`KERNEL_SURFACE.md`](KERNEL_SURFACE.md), [`THREAT_MODEL_AND_PACKAGE_MODEL.md`](THREAT_MODEL_AND_PACKAGE_MODEL.md),
> [`VOUCH_MECHANISM_DESIGN_2026-06-15.md`](VOUCH_MECHANISM_DESIGN_2026-06-15.md).

> **History:** this file supersedes `MASTER_ROADMAP_2026-06-15.md`,
> `semos-post-phase14-roadmap.md`, and `ROADMAP_EXPANSION_PROPOSAL(JUNE26).md` —
> their forward content was folded into the themed files above (originals preserved
> in git history). `IPHONE_SENSOR_OFFLOAD_PLAN.md` remains the Phase-18 sensor
> preview.

---

## The thesis

> **An agent-native, self-extending, sovereign OS.** An LLM agent writes its own
> modules, compiles them *on the machine*, and loads them into the running system —
> with the **security tiers as the capability fence** on agent-written code. Bare
> metal, fully owned, one remote dependency (the model) that the dGPU path can
> eventually remove.

Lineage: **Oberon** (self-extension via dynamically loaded modules) × **Genode**
(capability-scoped component isolation), with an LLM in the author's seat. Self-
hosting alone is old; the novel part is the agent closing the write→compile→load
loop locally, with the tiers as the guardrail that makes agent self-modification
safe.

- **Sovereign** is a *property*, not a separate workstream.
- **Semantic computing** (vector search / semantic objects) is a *tool the agent
  uses*, not a headline.
- **The security tiers already ARE the capability fence** — `current_task_max_tier()`
  gates LLM/semantic/process/namespace syscalls pervasively; a child can never
  exceed its spawner's clearance. Spawn agent modules at tier 0 → auto-sandboxed.

**One sentence:** the first OS where **an AI writes the system, runs on hardware
you fully own, and a human holds the only key that grants it power.**

**Deliberately NOT:** a Linux replacement (no POSIX, no other people's binaries),
a daily driver for normal users, an AAA-games machine (NVIDIA card is compute-only),
or "provably secure" (the claim is smallness + auditability + human-gated trust).

---

## Foundation already shipped

| Area | State |
|---|---|
| Core kernel | GDT/TSS, IDT (atomic IRETQ switch — task#40 family closed), paging + real guard pages, APIC, scheduler, TTF framebuffer console, PCI |
| Storage/FS | VirtIO block + NVMe + AHCI + USB-MSC behind one `BlockDevice` trait, read-only FAT32, sysroot blob, snapshot persistence, path namespace, RTC/wall-clock, FS syscalls |
| Net + crypto | SHA-256/HMAC/HKDF/X25519/ECDSA-P256/ChaCha20-Poly1305, virtio-net, smoltcp, TcpStream, embedded-tls + SPKI pin, **HTTPS round-trip to api.anthropic.com**, DNS, HTTP chunked |
| Userland | `semos-std` (opt-level=0 only), **sem-sh** shell (pipes/redirect/$VAR/$PATH/history/scrollback), TTY + ANSI, modal `edit`or, `agent` TUI |
| Self-extension | hardcoded `/bin` table removed → any namespace ELF runs by name, tier-scoped; tier-0 fence + `SYS_VOUCH`/`SYS_VOUCHES` |
| USB | xHCI (incl. Intel CSZ=1) + standalone EHCI, hubs (1-tier cascade), HID kbd, mass-storage SCSI, **iPhone ipheth tether** |
| Media | HD Audio (DEMO 63), HID gamepad parser |
| Self-hosting (M27) | full rustc + Cranelift ported to `no_std`/target; **on-device compile of /hello.rs → ELF → run, end-to-end (2026-06-15)** |
| ARM | `kernel-aarch64` boots QEMU `virt`: UART→vectors→MMU→GICv2→timer→scheduler running the same kernel-core |

Every boot opens with a **build stamp** (git hash · UTC build time · toolchain,
from `build.rs`) on the first serial line — the exact image is identifiable even if
a later init stage hangs.

---

## Active / gated / next (2026-06-28)

**Boot drives are back** (the 2026-06-15 dead-drive period is over); the T540p
boots on metal again. The three live threads:

### A. Bare-metal networking - USB dongle/tether active; PCI WiFi paused
Native Intel 7260 WiFi made real progress (firmware ALIVE, scan, Phase-A join
plumbing, data-queue TX and `TX_RESP`), but is now pinned at the over-the-air
AP-ACK wall: the AP reports **NO ACK / did not hear us** across protected
session/quota, RX-survival probing, and A/B/A|B + 1M/6M auth-rate sweeps. The
project decision is to **pause native PCI iwlwifi** rather than burn more time on
RF/firmware bring-up.

Near-term online path: use a **USB network dongle / tether** through the existing
`NetDevice` + smoltcp/TLS stack. Prefer class-style USB Ethernet (CDC-ECM/NCM /
RNDIS where practical) or a simple vendor USB NIC (RTL8152/AX88179) before taking
on another opaque USB WiFi firmware stack. Full detail: [networking.md](roadmap/map%20-%20networking.md).

### B. Self-extension loop (the thesis keystone) — ~80%
On-device rustc compiles + runs a program; module loader keystone landed.
Remaining: agent tool to drop a compiled ELF at `/apps/<name>` + spawn at tier 0,
then the "agent adds a `greet` command, works seconds later, kernel never rebuilt"
demo. [self-extension.md](roadmap/map%20-%20self-extension.md).

### C. Phone symbiosis — tether landed, capabilities next
iPhone enumerated, bulk pipe up. Next: QR pairing trust-bootstrap + companion
capabilities (crypto/identity/camera/GPS/mic/push) + LiDAR sensor offload.
[phone.md](roadmap/map%20-%20phone.md).

**Doable without the machine:** self-extension/loader, WPA2 crypto, assoc/EAPOL
builders, dGPU groundwork, ARM HAL (QEMU), browser/HTML parser, agent context mgmt,
security-doc maintenance. **Hard-gated on hardware:** USB dongle/tether traffic,
iGPU/dGPU bring-up, NVMe real-hardware test; native PCI WiFi is paused at the
AP-ACK wall.

---

## Phase map (synthesized)

```
NETWORKING & ONLINE                                   → roadmap/networking.md
  Phase 15  USB tethering ........ ipheth landed; USB dongle/tether traffic validation NEXT
  Phase 17  Layer-4 phone bridge . socket-forward over paired channel (cellular cost solved)
  Phase 20  Bare-metal WiFi ...... PAUSED; 7260 reaches TX_RESP but AP gives no ACK

SELF-EXTENSION (thesis core)                          → roadmap/self-extension.md
  M27       rustc-on-SemOS ....... compile+run works; sysroot polish (~80%)
  Loader    module/loader ........ arbitrary-name spawn DONE; agent /apps drop + tier-0 next
  Phase 22  package manager + SELF-REBUILD CAPSTONE (A/B slots, watchdog, human-vouched promotion)

PHONE SYMBIOSIS                                        → roadmap/phone.md
  Phase 16  QR-code pairing ...... trust bootstrap (Expo prototype)
  Phase 18  companion capabilities crypto/identity/camera/GPS/mic/push + LiDAR offload

GPU (hardware-gated)                                   → roadmap/gpu.md
  Phase 11  iGPU rendering ....... HD 4600 / Iris Xe — CAD view, video, games
  Phase 12  dGPU compute ........ Kepler GK208 (pre-GSP) → local LLM inference → fully sovereign

PLATFORM                                               → roadmap/platform.md
  ARM port .................... kernel-aarch64 runs in QEMU; Ring-3/SVC next
  Info access ................. HTTP client, HTML parser, search, text browser, web_search tool
  Agent infra ................. context mgmt, multi-file edit, cargo integration, parity
  Swift bridge / media / utils  Phase-19 native rewrite; H.264/games/music; `top`
```

---

## Open decisions
1. **dGPU compute vs WiFi-first for "sovereign"** — local inference removes the last
   remote dependency (big lift; offline design proceeds in parallel).
2. **Self-extension demo as the headline** — finish "agent adds a command" end-to-end
   before broadening? Clearest proof of the thesis.
3. **ARM timing** — `kernel-aarch64` runs offline; decision is *when* to sequence
   Ring-3 spawn/SVC relative to WiFi/self-extension.
4. **Phone-as-vault for WiFi** — wire pairing/presence before or after WiFi connects?

---

## Working disciplines (not milestones)
Sustainable pace — pacing for years, not sprints; one thing at a time (WiFi allowed
parallel as background); the OS is for the author first; the from-scratch commitment
holds (no POSIX, no libc, vendored deps patched + audited). A milestone is **done**
when it compiles clean in both crates, a boot DEMO emits `PASS:`/`FAIL:`, the project
memory is updated with lessons, and follow-up work is captured as its own milestone.
