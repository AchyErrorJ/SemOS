# SemOS Status Report — for OpenClaw agent handoff

**Date:** 2026-08-16 · **Repo head:** `dab28f4` (+ local WSL-merge WIP) · **Scope:** where development stands, what's working, and direct answers to the audio/Whisper scoping questions.

---

## 1. What SemOS is, in one paragraph

A from-scratch, bare-metal x86_64 OS written in Rust (`no_std`, no host OS, no POSIX, no libc). Two kernel crates: `kernel-core` (platform-agnostic policy) over a `Platform` trait implemented by `kernel-x86_64`. Boots on QEMU (BIOS+UEFI) and on the ThinkPad T540p dev rig. The headline thesis: **LLM data-leak protection enforced in Ring 0** via 4-tier semantic objects, and an **agent-native self-extension loop** — an LLM writes modules, compiles them on the machine, and loads them into the running system with security tiers as the capability fence.

---

## 2. What's working today (verified in-tree)

| Area | State |
|---|---|
| Kernel core | GDT/TSS, IDT, 4-level paging + guard pages, APIC/IOAPIC, scheduler, TTF framebuffer console, PCI |
| Storage/FS | VirtIO-blk + NVMe + AHCI + USB-MSC behind one `BlockDevice` trait, read-only FAT32, disk sysroot blob, snapshot persistence (cross-boot verified), path namespace, RTC/wall-clock, full FS syscall set |
| Crypto/TLS/net | SHA-256/HMAC/HKDF/X25519/ECDSA-P256/ChaCha20-Poly1305 (all KAT-verified), virtio-net + smoltcp, embedded-tls + SPKI pinning, **HTTPS round-trip to api.anthropic.com from bare metal**, DNS, HTTP chunked |
| Userland | `semos-std` shim (std subset, opt-level=0 only), **sem-sh** shell (pipes, redirect, $VAR, $PATH, history, scrollback), TTY+ANSI, modal text editor, `agent` TUI |
| Self-hosting (M27, ~80%) | **Full rustc ported to SemOS (`semos-rustc`) + Cranelift; on-device compile of hello.rs → ELF → run works end-to-end** (2026-06-15). `semos-cc` Cranelift emitter runs in Ring 3 (runtime heap tuning open) |
| USB | xHCI (incl. Intel CSZ=1 quirk) + standalone EHCI, hubs (1-tier cascade), HID keyboard, mass-storage SCSI, CDC-ECM/NCM, **iPhone ipheth tether** |
| Media | Intel HD Audio output (DEMO 63: codec walk + sine wave playback), HID gamepad parser |
| ARM | `kernel-aarch64` boots QEMU `virt` through to scheduler on the same kernel-core |
| Agent loop | LLM syscalls (`SYS_LLM_*`), streaming, policy get/set, redaction; Kimi/Anthropic keys via `.kimi-key` (gitignored) |
| Display (M14, active) | Haswell iGPU modeset work in progress; FB syscalls (meta/blit/modeset/vblank/backlight) landed upstream |
| Netlog (new) | `SYS_NETLOG` (132): kernel log ring → UDP to a LAN listener; built into current image, **boot-test pending** |

**Build state (today):** full WSL2 toolchain migration done — kernel-core, kernel-x86_64, and all 17 embedded user-program ELFs (incl. the 90 MB `semos-rustc`) build green in WSL. Boot-image packaging (`x86_64-runner`) is the last step being fixed (bootloader crate's nested builds hit a nightly-rustc lint ICE on Linux hosts; workaround in flight).

---

## 3. Answers to the scoping questions

### 3.1 Syscall / IPC surface — what a Rust program actually calls

Not POSIX. ~90 syscalls (numbered 0–131 with gaps), ABI defined in `kernel-core/src/syscall/mod.rs`, entered via `SYSCALL` instruction from Ring 3. A userland Rust program almost never raw-dogs these — it links **`semos-std`** (`user-programs/std-shim`), which maps a growing `std` subset onto them: `print!`/`println!`, `fs::File`, `io::{Read,Write}`, `env`, `process`, `thread::spawn`/`JoinHandle`, `sync::{Mutex,Once}`, `mpsc`, `time`, `net`, `path`, `collections`, plus a `#[global_allocator]` (Vec/String/Box work).

Surface by group (canonical numbers):

| Group | Syscalls |
|---|---|
| I/O | `SYS_WRITE`(0), `SYS_READ`(1) |
| Process | `EXIT`(2), `YIELD`(3), `GETPID`(4), `SLEEP`(5), `SPAWN`(40), `WAIT`(41), `KILL`(42), `EXEC`(43), `DUP`/`DUP2`(44/45), `PIPE`(46), `THREAD_SPAWN`(92), futex wait/wake |
| Files | `OPEN`(10) … `FREAD`(12), `FWRITE`(13), `SEEK`, `STAT`, `MKDIR`, `UNLINK`, `READDIR`, `FSYNC`, `RENAME`(36), `TRUNCATE`(37), `STATX`(38) |
| Memory | `ALLOC`(30), `FREE`(31), `BRK`(33), `HEAP_ALLOC`/`HEAP_FREE`, `MMAP_ANON`(39) — byte-granular user heap exists; per-process heap currently 16 MiB |
| Semantic objects | `SEM_CREATE/READ/WRITE/DELETE/LINK/QUERY/SEARCH/META` (SUID-addressed, tier-tagged objects — the FS sits on these) |
| LLM | `LLM_QUERY/CONTEXT/REDACT/SUMMARIZE/ACCESS/STREAM_START/STREAM_READ/SET_POLICY/GET_POLICY` (50–58) |
| Crypto/persist | `ENCRYPT`/`DECRYPT`/`HASH` (60–62), `PERSIST`/`RESTORE` (63/64) |
| Display | `FBINFO`(118), `BACKLIGHT`(119), `FB_META`(128), `FB_BLIT`(129), `MODESET`(130), `FB_WAIT_VBLANK`(131) |
| Misc | `TIME`/`UPTIME`/`REBOOT` (70–72), `SYSINFO`(73), env/CWD get/set (74–77), `WIFI_CONNECT`(125), sysroot info/read/flash (120–122), `NETLOG`(132), pairing (`PAIR`/`PAIRED`/`UNPAIR`) |

**Answering directly:**
- **Read audio:** *nothing today.* There is no audio-capture syscall and no `/dev` layer — the HDA driver is output-only (demo sine wave). The audio spec's `/dev/audio0` char-device shape doesn't match SemOS (no devfs); the SemOS-native shape is either a small `SYS_AUDIO_*` set or an audio endpoint exposed as a semantic object read via `SYS_SEM_READ`-style streaming.
- **Allocate memory:** `Vec`/`Box`/`String` via the semos-std global allocator → `SYS_HEAP_ALLOC`/`SYS_BRK`. Raw pages via `SYS_ALLOC`/`SYS_MMAP_ANON`.
- **Spawn tasks:** `SYS_SPAWN` by name — any ELF in ramfs `/bin/<name>` or the namespace `/apps/<name>` is runnable, tier-gated (see 3.2). Threads: `SYS_THREAD_SPAWN` (same-address-space, Ring-3 capable). IPC: `SYS_PIPE`, kernel semaphore objects (`SYS_SEM_*`), futex.

### 3.2 Security thesis

- **4-tier model** (`Public | Internal | Sensitive | Secret`) on every semantic object; a task's `max_tier` gates object, LLM, process, and namespace syscalls pervasively. **`SYS_SPAWN` is the capability fence:** a child's tier = `min(requested, caller_tier)` — spawned code can never exceed its spawner's clearance. Agent/LLM-authored code runs at **tier 0**, auto-sandboxed.
- **Ring-0 redaction:** LLM-bound views of an object get tier-based redaction before bytes leave Ring 0; direct reads get verbatim bytes. The policy lives in privileged code; user code can't bypass it.
- **Vouch mechanism** (`SYS_VOUCH`/`SYS_VOUCHES`): human-granted privilege elevation — "a human holds the only key that grants the agent power."
- **No dynamic linking.** Programs are static ELFs: compiled into the boot image, or dropped at `/apps/<name>` (e.g. by the on-device compiler) and spawned by name. **No JIT.** Panic = abort.
- **Package model (designed, not yet implemented):** tier-aware installs, **no transitive trust** (deps inherit the caller's tier ceiling), content-addressed registry (`sha256(tarball)`, not `name@version`), source-vetting above tier 0. Today nothing loads at runtime that wasn't compiled into the image or explicitly placed — no install syscall exists yet.
- **Honest weaknesses:** no IOMMU on the T540p (no DMAR table → no VT-d), so every bus-master device (xHCI, EHCI, AHCI, NVMe, NIC) DMAs unconstrained; the iwlwifi firmware blob is a documented declared-trust exception (native WiFi currently paused). Full per-syscall audit is open work (`KERNEL_SURFACE.md` §3).

**Consequence for Whisper/ONNX:** any inference runtime is allowed *in principle* — it would be a tier-0 (or vouched-higher) Ring-3 static ELF. But it must be **compiled to a SemOS ELF**: no dynamic libraries, no runtime code loading, no JIT. That rules out linking prebuilt C++ runtimes (whisper.cpp, ONNX Runtime) unless they're source-ported through a SemOS-targeting C/C++ toolchain — which barely exists (see 3.3). The realistic path is **pure-Rust inference** (`candle` / `tract` / `burn`) cross-compiled against semos-std, with the runtime's `std` needs shimmed. openWakeWord (Python/ONNX) has the same answer: CPython is a documented "hopeless" port; MicroPython is the designated embedded-Python path if a scripting runtime is ever wanted.

### 3.3 Rust toolchain state

- **Host side (build machine):** pinned `nightly-2026-02-01` via `rust-toolchain.toml` (`rust-src`, `llvm-tools`, cranelift-preview). Target `x86_64-unknown-none` with `build-std = [core, compiler_builtins, alloc]` + `compiler-builtins-mem`. So: **yes — no_std Rust with `core` + `alloc` is the native programming model**, plus `semos-std` for the std-shaped subset. Known wart: user binaries must build at **opt-level=0** (a codegen sensitivity at higher opts is an open bug).
- **On device (self-hosting):** `semos-rustc` — the full rustc, vendored and ported to no_std/SemOS — **compiles and runs programs on SemOS today** (hello.rs → ELF → spawn, end-to-end verified). `semos-cc` is a from-scratch Cranelift-based ELF emitter (Ring-3, DEMO 73: emits+runs its own ET_EXEC; has a runtime heap-exhaustion bug under tuning). `compiler/` is the host-side version of the same emitter.
- **C/C++:** there is no general C/C++ toolchain for SemOS, and by policy there never will be a libc. `semos-cc` is not a C compiler — it's an IR→ELF experiment. Anything C++ (whisper.cpp) would need a full source port through Cranelift with a C++ stdlib — not realistic near-term.

### 3.4 Userland story — where would Five live?

**SemOS has a real userspace.** Ring-3 ELF binaries, own address space per process, entered only through `SYSCALL`; same-address-space Ring-3 threads via `SYS_THREAD_SPAWN`. Programs come from three places: ELFs baked into the boot image (17 today, incl. `sem-sh`), the on-disk/ramfs namespace (`/bin`, `/apps`), or emitted on-device by the ported compilers. `sem-sh` is the interactive shell; the `agent` TUI is the LLM front-end.

**Five would be a Ring-3 user program** (a daemon spawned from `/apps/five`), not a kernel module — SemOS has no kernel-module concept for user code; in-kernel extension is the *module loader* thesis (Oberon-style), which is tier-fenced and human-vouched, not a free-for-all.

### 3.5 USB stack state

**100% Rust, from scratch, in-kernel** (`kernel-x86_64/src/usb/`, ~8,900 LoC):

| File | LoC | What it does |
|---|---|---|
| `xhci.rs` | 3,955 | Full xHCI host controller: bring-up, command/transfer/event TRB rings, slot/endpoint contexts, Intel CSZ=1 quirk, port enumeration |
| `ehci.rs` | 2,134 | Standalone EHCI (Lynx Point companion-controller routing handled) |
| `device.rs` / `hub.rs` | 459 / 378 | Descriptors, enumeration, 1-tier hub cascade |
| `hid.rs` / `hid_report.rs` | 403 | Boot keyboard + report-descriptor parsing |
| `mass_storage.rs` | 186 | USB-MSC / SCSI — flash drives work (behind the `BlockDevice` trait) |
| `cdc_ecm/ncm`, `iphone*` | ~700 | USB Ethernet (CDC-ECM/NCM) + iPhone ipheth tethering |

**Depth:** control transfers, bulk, interrupt-IN (HID) all work on metal. **The two gaps that matter for audio:**
1. **No isochronous transfers** — zero occurrences in the tree. Isoch TRB/TD scheduling is the audio spec's Phase 2 and is genuinely new work (the ring/TRB machinery is endpoint-agnostic, so it's an extension, not a rewrite).
2. **Completion is polled, not interrupt-driven** — xHCI interrupts are deliberately left disabled (`IMAN.IE = 0`); the stack polls event rings. Fine for keyboard/storage/tether; **continuous 1 ms isochronous audio wants real interrupt-driven completion** (MSI-X or IOAPIC routing — APIC/IOAPIC infrastructure already exists elsewhere in the kernel).

---

## 4. What that means for the USB-audio + Whisper plan

The spec's Phase 0 checklist is **mostly already done** (PCI, xHCI+EHCI init, enumeration, EP0 control, bulk/interrupt scheduling — all Rust, all working on the T540p). The real critical path:

1. **Isochronous IN on xHCI** (new; ~2–3 days est. — the spec's Phase 2 is the right shape).
2. **Interrupt-driven completion** for the audio endpoint (new; poll-first MVP is acceptable — the stack already works that way).
3. **UAC descriptor parsing + format negotiation** (new driver, `usb/uac.rs`; descriptor-walking patterns exist in `hid_report.rs`/`cdc_ecm.rs` to copy).
4. **Kernel audio ring + userland surface** — *not* `/dev/audio0`; a small `SYS_AUDIO_*` surface or a semantic-object stream (fits the tier model: mic = Sensitive by default).
5. **Five daemon in Ring 3** reading PCM via that surface.
6. **Inference:** pure-Rust only. Whisper via `candle` (gguf quantized, e.g. tiny/base) or `tract` (ONNX) cross-compiled to semos-std; wake-word via tract-hosted ONNX or a simpler VAD+energy gate first. Watch the 16 MiB per-process heap — whisper-tiny quantized wants tens of MB; heap bump required (precedent: `MAX_PT_FRAMES` was bumped for the 5.4 MiB semos-cc ELF).

## 5. What we need answered / decided

1. **Mic in hand?** Blue Yeti vs Snowball vs Logitech — need the actual VID/PID and whether it natively does 16 kHz mono (else we capture 48 kHz and downsample; naive decimation is fine for MVP).
2. **xHCI or EHCI port** for the mic on the T540p (xHCI strongly preferred — isoch on EHCI is siTD pain).
3. **Runtime pick:** candle vs tract vs burn for the first inference port (decides which `std` gaps semos-std must grow — file mmap? f32 transcendentals via libm? threading?).
4. **Wake-word scope:** is openWakeWord mandatory, or is energy-VAD + push-to-talk acceptable for the first Five milestone? (Drops the ONNX dependency entirely.)
5. **Audio security tier:** confirm mic capture lands at Sensitive (tier 2) by default, with five-daemon vouched to tier 2 — or is ambient audio tier-0-readable by design?
6. **Interrupt budget:** approve enabling xHCI interrupts (MSI-X) — touches the platform interrupt path beyond USB.

---

*Sources: `docs/WHAT_IS_SEMOS.md`, `docs/MASTER_ROADMAP.md`, `docs/THREAT_MODEL_AND_PACKAGE_MODEL.md`, `docs/KERNEL_SURFACE.md`, `docs/STD_SHIM_SURFACE.md`, `kernel-core/src/syscall/mod.rs`, `kernel-x86_64/src/usb/`, `user-programs/std-shim/`, `user-programs/semos-cc/PORT_LOG.md` — all at repo HEAD `dab28f4` (+WSL-merge WIP).*
