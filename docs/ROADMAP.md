# Semantic OS — Roadmap

**No dates here.** This document tracks **what must happen** and **what unblocks what**. Time is a side effect of doing things in the right order, not a thing to plan in.

> **Next agent:** start with the latest handoff — [`HANDOFF_2026-05-22.md`](HANDOFF_2026-05-22.md) — for current state, build/test gotchas, and suggested next steps.

When you finish a milestone, flip its checkbox in this file and update the [Project memory file](../../../Users/jerro/.claude/projects/F--Software-ArmKernel3/memory/project_semantic_os_kernel.md). When a milestone reveals new sub-work, add it to that milestone's checklist or split it into a follow-up milestone.

Phase 8 (network → first remote LLM call) is closed. See [`PHASE_8_ROADMAP.md`](PHASE_8_ROADMAP.md) for that phase's historical detail. The current frontier is Phase 9.

---

## How to read this

```
M0  [✅] = done and committed, exercised by a boot-time DEMO
M0  [🔨] = in progress on a branch
M0  [⏸️] = paused, blocker known and documented
M0  [  ] = pending; checklist below is the contract for "done"
```

A milestone is **done** when:
1. Code compiles clean in both `kernel-core` and `kernel-x86_64`.
2. A boot-time DEMO `N` in `kernel-x86_64/src/main.rs` emits `PASS:`/`FAIL:` lines that grep cleanly out of the QEMU serial log.
3. The project memory file is updated with the lessons learned (not just the fact that it shipped).
4. Follow-up work the milestone uncovered is captured as a separate milestone or task, not left implicit.

---

# Done (status snapshot)

| Phase | What landed |
|---|---|
| 1-6 | GDT/TSS, IDT, paging, APIC, framebuffer console, PCI bus, VirtIO block, snapshot persistence |
| 7 | Streaming LLM syscalls, security policy framework, context-aware redaction, network LLM provider (loopback), user identity + isolation |
| 8 | Crypto stack (SHA-256, HMAC, HKDF, X25519, ECDSA P-256, ChaCha20-Poly1305), virtio-net, smoltcp, TcpStream, RDRAND, embedded-tls vendored + crypto-shim, SPKI-pinning TlsVerifier, TLS-backed NetworkTransport, **first outbound HTTPS round-trip to api.anthropic.com** |
| 9 | Path namespace (M1), RTC + wall_clock (M2), FS Stage 3 syscalls (M4), FS Stage 2 persistence (M5) **with cross-boot vdisk verification**, USB driver (M3) **fully unblocked 2026-05-19 by main-kernel-stack bump (#42 fix)** |
| 14 prep (Tier 1) | Cranelift + cg_clif vendor placeholders + briefs (agent), heap allocator, argv/envp passthrough, per-process env+CWD |
| 14 prep (Tier 2) | SYS_FSYNC, SYS_RENAME, SYS_TRUNCATE, SYS_STATX, FWRITE>256 B via heap-Allocated ObjectContent (M26 "first compile" unblocked) |
| 14 prep (Tier 3) | SYS_THREAD_SPAWN/JOIN (kernel + Ring-3 same-AS), SYS_FUTEX_WAIT/WAKE, SYS_WAITNB, SCHEDULER_TICK_HZ const (parallel/threaded rustc unblocked) |
| 14 (M25 substantial) | `semos-std` crate: `#[global_allocator]` (Vec/String/Box), `io::{Read,Write}`, `fs::File`/`OpenOptions`, `env`, `sync::{Mutex,Once}`, `thread::spawn`+`JoinHandle<T>`, `process::Command` (spawn+wait), argv. hello-std/vec-demo/std-demo/spawn-demo run Ring 3 (DEMO 29–32). **Build at `opt-level=0` only** — any optimization miscompiles the syscall path (#54). Still missing: `net`, full `path`/`time` |
| 9/10 graphics+net | M6 framebuffer drawing API (DEMO 35); M13 HTTP chunked decoder (DEMO 33); M12 DNS resolver (DEMO 34, wall-clock wait + retransmit) |
| Structural | #41 real guard pages between all task stacks; #54 std-shim opt-level workaround; #55 sequential Ring-3 spawn; per-task kernel stack → 128 KiB. **task#40 / #56 FIXED (`8c2cb21`): context_switch was a *torn control transfer* (`popfq; jmp` window where a timer preempted mid-switch) — now an atomic IRETQ. Closes the whole layout-sensitivity / iret-RIP-corruption family.** |
| Cleanup 2026-05-22 | All HANDOFF open issues closed: **#55 re-verified** (`72a002f`, DEMO 28 → 0x2700); **DEMO 27 timing flake de-flaked** (`78ae59e`, poll-not-sleep); **M7/M8 wired into `tty::TtyConsole`** (`78ae59e`, DEMO 39 — the M19 renderer). Suite **132 PASS / 0 FAIL / 0 #DF** with `-netdev`. |
| M19 slice 1 2026-05-22 | **TTY stdin + ANSI** (`716eafd`, DEMO 40): cooked-mode line discipline (`SYS_READ` fd 0, Backspace), `AnsiTty` (SGR color / 2J / K / H) over the TTF console. Suite **135 PASS**. |
| M19 per-process stdio 2026-05-22 | **Full per-process FD-table refactor** (`673d948`+`efd444e`+`21dbd8f`, DEMO 41/42): all FDs (console/pipe/path/ramfs) live in the process `FdTable`; global PATH_FDS/PIPE_FDS deleted; stdio routable (dup2→pipe) + inherited on spawn; slot-keyed resolution + stale-task_id fix. Suite **140 PASS**. |
| M19 DONE 2026-05-22 | **TTY complete** (`9787cb7` line editing + history, `93ca47c` scrollback; DEMO 43/44): in-line cursor + arrow keys (PS/2 0xE0 + USB HID → ESC[ABCD) + 8-line history; TtyConsole scrollback ring. Suite **145 PASS / 0 FAIL / 0 #DF**. M19 ✅ — next is M20 native shell. |
| M20 stage A 2026-05-23 | **sem-sh native shell** (`5398720`, DEMO 45): REPL reading cooked stdin (M19) + script mode, quote-aware parser, builtins (echo/pwd/cd/exit), external ELF exec via Command. Suite **147 PASS**. Gotcha: new user crate must be non-PIE (build.rs+link.ld) or println crashes — see feedback memory. |
| -netdev DEMO 15 hang FIXED 2026-05-23 | `ad540dd`: embedded-io TcpStream read/write now bounded by a 10 s idle deadline. The TLS handshake's ServerHello read spun forever when SLIRP raced port 1 to ESTABLISHED then went silent — hung the boot 350 s+. 4 consecutive -netdev boots clean after. |
| M20 stage B 2026-05-23 | **sem-sh fs builtins + $VAR** (`b81251d`, DEMO 45): cat/ls/which/env builtins + `$VAR` expansion (inherited env). Suite **148 PASS**. |
| M20 DONE 2026-05-23 | **sem-sh redirection + pipes** (`96fbaf9`, DEMO 46): `>`/`<` redirection + `|` pipelines (sequential v1). Kernel: SYS_WRITE→handle_fwrite routing + positional Path writes. Suite **150 PASS / 0 FAIL / 0 #DF**. M20 ✅ — shell complete. |
| M19/M20 hardening 2026-05-23 | **pipe-end refcounting** (`0b4a6bb`) + **true `>>` append** (`763188a`) + **concurrent pipes** (`9d89dbb`: WOULDBLOCK reads + spawn-inherit refcount + exit-time FD cleanup + concurrent shell spawn). Suite **152 PASS / 0 FAIL / 0 #DF**. |
| M22 stage A 2026-05-23 | **Claude agent core** (`34ef9ee`, DEMO 47): `agent.rs` — Messages-API request framing + response parse (text + tool_use) + tool dispatch (read_file/write_file). No network. Suite **157 PASS**. |
| M22 stage B + net fix 2026-05-23 | **agent live TLS round-trip** (`9da1f51`, DEMO 48): build_http_request + send_over_tls → HTTP 401 from api.anthropic.com (proves framing+TLS send/recv). Required **TcpStream reconnect fix** (`efd8c3c`: free smoltcp socket on Drop — a successful connection's close leaked it, hanging the next connect). Also: DEMO 15 stall DIAGNOSED — timeout mechanism sound (ticks advances), residual flake is in net::poll for the bogus port-1 target only; real TLS (16/48) reliable. Suite **139 PASS / 0 FAIL / 0 #DF** (the "158" figure in this row's first draft was a miscount; verified 139 by booting the committed HEAD). Stage C: key + loop + bash/grep/glob + TUI. |
| FS large files — Model A, plan + stage 1 2026-05-27 | **Decision (talked through):** fix the FS for demanding design files via **Model A** — files live in RAM (now up to the 512 MB/pool ceiling), `as_bytes()`'s contiguous-`&[u8]` contract preserved (so the ~11 consumers + `spawn`'s ELF parse don't change), persisted to disk. **Model B** (disk-backed extents, content not resident, multi-GB out-of-core) **deferred until the hardware arrives** — same tier as the deferred GPU. **Per-file size: FIXED CEILING** (not "whatever frames are free"): a single file must never drain a tier pool — that would starve the app's own working set + other files into an OOM cascade; a predictable bound is good hygiene and a one-constant tune. **Stages:** (1) lift the heap-bound cap [done — see next row]; (2a) frame-backed content via a contiguous-frame allocator + `phys_to_virt` (escape the 16 MiB heap → 100s-of-MB in-RAM files, fixed ceiling ~128 MB so one file ≤ ¼ pool); (2b) per-file disk block allocator + persistence so the snapshot is metadata-only and large files persist (today's monolithic snapshot duplicates content into one buffer — the real blocker past a few MB). |
| FS large files — stage 1 2026-05-27 | **8× the per-file cap, persistable.** `MAX_FILE_CONTENT` 256 KiB→2 MiB and `MAX_SNAPSHOT_BYTES` 1 MiB→4 MiB (heap-backed scratch already), staying within the 16 MiB heap. Content is still one heap `Allocated` blob (contiguous → `as_bytes()` unchanged). DEMO 60 extended: installs a synthesized ~1 MiB file (`/apps/bigdoc`, > the old 256 KiB cap *and* the old 1 MiB snapshot) and verifies it survives reboot byte-pattern-intact. Heap-bound (~2 MiB/file) until stage 2a frame-backs content. Two-boot test (non-net): boot 2 → all three files restored incl. **/apps/bigdoc 1 MiB pattern-intact**; DEMO 26 oversize-rejection re-derived from `MAX_FILE_CONTENT+4 KiB` (no longer drifts). **145 PASS / 0 FAIL / 0 #DF.** Known follow-up: DEMO 5 (raw-snapshot demo) shares virtio0 sector 0 with the namespace snapshot and re-seeds it when its small read buffer hits the 1.1 MB namespace header — harmless for a 2-boot test (namespace already in RAM) but clobbers on-disk persistence on a 3rd boot; give DEMO 5 its own region or skip re-seed when a namespace magic is present. |
| Interactive mode 2026-05-27 | **the OS is drivable at the keyboard** (`30c5687` + `32a798c`). Cargo feature `interactive` (default off, so headless CI still runs the 60 demos + idles): boot ends by dropping into the live `sem-sh` shell. Fix that made typing work — the USB HID event ring is only drained when polled, and nothing polled it while we waited, so the shell's `SYS_READ` never saw keystrokes; now the wait loop pumps the ring into the line discipline (edge-detected so held keys don't repeat) with explicit framebuffer echo (`input_push` echoes only to serial). New shell builtins: `help` (lists builtins) and `agent` (launches the split-pane TUI as a Claude chat loop via **SYS_AGENT 112** → `Platform::run_agent_tui` → `agent::run_interactive`; `AGENT_TUI_ACTIVE` pauses the shell pump while the TUI owns the keyboard; `framebuffer::clear()` for overlay teardown). Validated windowed via QEMU `sendkey`: typed commands run, `help`/`agent` work, agent prompt echoes + `exit` clears back to the shell; Backspace at an empty prompt no longer eats the prompt. Default build still 141 PASS / 0 FAIL. **Next: native editor (M21).** |
| xHCI CSZ=1 2026-05-28 | **64-byte context layout for Intel chipsets (`8821df1`).** Previous boot-time REJECT on CSZ=1 is gone. `InputContext`/`DeviceContext` are now raw byte buffers at max stride (2112/2048 B, align(64)); accessors (`input_ctrl_mut`, `slot_mut`, `ep_mut(idx)`, `slot_read`, `ep_read(idx)`) compute offsets via `CTX_SIZE` set once at xhci bring-up (`set_ctx_size(if csz1 {64} else {32})`). 32-byte `SlotContext`/`EndpointContext` data formats unchanged — only their placement varies. **`TIMER_TICKS` is also now `AtomicU64`** (was `spin::Mutex<u64>`, latent ISR-vs-reader deadlock found while building the M10 watchdog). qemu-xhci (CSZ=0) regression-clean: 165 PASS / 0 FAIL / 0 #DF. Unblocks USB enumeration on the T540 HM87 (Intel CSZ=1). |
| M10 watchdog + audit 2026-05-28 | **Pre-flight v1 (`d77ba87`).** Audit: framebuffer-only diagnostics already in place (`serial::_print` mirrors), RTC century byte already handled, panic handler routes through both. New: `idle_with_heartbeat` prints `[heartbeat] kernel reached idle — ticks=N` at end of boot as proof-of-life on metal. Latent bug fixed: `TIMER_TICKS spin::Mutex<u64>` → `AtomicU64`, eliminating the ISR-vs-reader deadlock pattern. **Top M10 follow-up is xHCI CSZ=1 support** — Intel chipsets (incl. the T540 HM87) need 64-byte contexts; current code rejects at bring-up. Blocks USB on real Intel hardware (PS/2 keyboard still works). |
| USB Mass Storage v1 2026-05-28 | **USB stick CBW/CSW + SCSI (DEMO 68, `3a4587b`).** Protocol layer for reading a USB stick on the T540: class IDs 0x08/0x06/0x50 (SCSI BBB), 31-byte CBW build with 'USBC' signature + zero-padded CBWCB, 13-byte CSW parse (tag/residue/status), SCSI CDB builders for INQUIRY / READ CAPACITY (10) / READ (10) / WRITE (10) / TEST UNIT READY, INQUIRY + READ CAPACITY response parsers. Validated against six canned-byte checks. Hardware-ready for live xHCI bulk-endpoint TX/RX (same gating as CDC-ECM). |
| AHCI/SATA 2026-05-28 | **SATA block driver — the T540 internal-disk path (DEMO 67, `ed2630f`).** PCI class-coded discovery (0x01/0x06/0x01), ABAR (BAR5) → MMIO, AHCI-mode enable (no HBA reset; HR severs the SATA PHY in QEMU's ich9-ahci and doesn't auto-relink — real-hardware follow-up adds HR + SCTL.DET cycle + CAP2.BOH handoff), port scan with short DET poll, per-port CL/FB setup, ATA Identify Device for block count, single-LBA READ/WRITE DMA EXT via a one-entry PRDT. Registered as `sata0` BlockDevice. First-boot in QEMU: port 0 SSTS=0x113 SIG=0x101, 131072×512 B (64 MiB), DMA round-trip clean. 159 PASS / 0 FAIL / 0 #DF. |
| CDC-ECM v1 2026-05-28 | **USB Ethernet descriptor parser (DEMO 66, `e79a3a3`).** The M11 fallback path — a USB-to-Ethernet dongle lets TLS run on metal before iwlwifi works. Protocol v1: class/subclass/protocol IDs (0x02/0x06 control, 0x0A data), `parse_config` walks the full configuration blob (skipping Header/Union functional descriptors, picking up CDC Ethernet Functional Descriptor for iMAC/MTU, finding the Data interface alt with bulk EPs), `parse_mac_string` decodes the UTF-16LE 12-hex-digit MAC string (CDC §5.4). Validated against a realistic config blob → iface 0 control, iface 1 alt 1 data, bulk 0x81/0x02 MPS 512, MAC `02:BA:DC:AF:E0:01`, MTU 1514. 157 PASS / 0 FAIL. Live xHCI bulk-endpoint TX/RX is the follow-up on real hardware. |
| M11 v1 (protocol) 2026-05-28 | **802.11 frame builders + iwlwifi PCI scaffolding (DEMO 65, `a0d487b`).** QEMU has no wireless emulation, so v1 = the pieces we'll need on day-1 of metal: `wireless::build_probe_request` / `build_open_auth_request` / `build_association_request` + `build_eapol_msg2` (WPA2 four-way handshake Msg2 with KeyInfo bitflags via bitflags 2.4; MIC left zero for the crypto layer to patch). iwlwifi PCI device-ID table covers T540 (7260/3160 family) and P1 Gen 6 (AX211, 0x51F0/0x51F1/0x54F0). DEMO 65 byte-validates each frame against the IEEE 802.11 layout (Probe Request FC=0x4000 + broadcast addrs + SSID IE, Open Auth algo=0/seq=1, EAPOL KeyInfo=0x010A = MIC+Pairwise+CCMP) and the PCI table. 154 PASS / 0 FAIL. Follow-ups (all hardware-gated): firmware-upload secboot, ALIVE event, PHY init (NVM+PNVM+regulatory+calibration), TX/RX command queues, four-way handshake MIC over the derived PTK. |
| M16 HID parser 2026-05-28 | **HID report descriptor parser + gamepad decode (DEMO 64, `d4b8e2d`).** Pure-module v1 since QEMU has no gamepad: `usb::hid_report::parse` walks a HID 1.11 descriptor (short items, global/local state, Usage Min/Max ranges, multi-usage Input, signed Logical Min/Max, Output/Feature offset advancement) → `ReportLayout` flat field table (no_std, no alloc). `decode_gamepad` extracts standard axes (X/Y/Z/Rx/Ry/Rz/Hat) + first 32 buttons. Validated against a canonical Generic-Desktop Game Pad descriptor + synthetic report `[0x42, 0xFE, 0x0A]` → `x=66, y=-2 (sign-extended), buttons=0b1010`. 150 PASS / 0 FAIL. Follow-ups (hardware-gated): fetch report descriptor via USB control transfer, route input reports in xHCI, expose a Gamepad input device. |
| M15 HD Audio 2026-05-28 | **Intel HDA controller + codec walk + PCM output (DEMO 63, `3f8fed2`).** PCI class-coded discovery (0x04/0x03/0x00), 64-bit MMIO BAR, controller reset, STATESTS-based codec discovery, walk root → AFG → first DAC + first Pin. Codec verbs via the **Immediate Command Interface** (ICI: ICO/IRI/IRS at 0x60/0x64/0x68) — CORB/RIRB-via-DMA was flaky in QEMU after the first verb. Pin: D0 + OUT_EN + EAPD. DAC: 48 kHz 16-bit stereo format, stream tag 1, unmute output amp. BDL with one entry pointing at a page-aligned 4 KiB PCM buffer holding a 440 Hz sine (16-step LUT). Output stream descriptor at MMIO `0x80 + 0x20*ISS`: CBL/LVI/FMT/BDPL/BDPU/CTL+RUN. **Validation:** LPIB sampled twice over a sleep advances (DMA active = playback). 147 PASS / 0 FAIL / 0 #DF. Follow-ups: CORB/RIRB on real metal, MSI-X, capture (ADC), gapless wrap. |
| M9 NVMe 2026-05-27 | **NVMe block driver (DEMO 62, `53cdc1a`).** PCI class-coded discovery (0x01/0x08/0x02), 64-bit MMIO BAR, admin queue bring-up (reset → AQA/ASQ/ACQ → CC.EN → CSTS.RDY), Identify Namespace (NSZE + active LBA format → block_count + block_size), Create-I/O-CQ + Create-I/O-SQ (qid 1), NVM Read/Write via PRP1 (one LBA/cmd, BlockDevice loops). Polled completions with phase-bit tracking. Page-aligned BSS queues/buffers for contiguous DMA. Registered as `nvme0`. First-boot validation in QEMU: PCI 00:04.0, MMIO=0xFEBF0000, 65536 blocks × 512 B, write+read byte-for-byte. 146 PASS / 0 FAIL / 0 #DF. Follow-ups: MSI-X, multi-block PRP lists, real error recovery. |
| M21 editor + console UX 2026-05-27 | **native modal editor + readable console.** `edit <file>` (`94581a8`, DEMO 61) launches a kernel-side vi-style editor (SYS_EDIT → `Platform::run_editor`): Normal/Insert/Command modes, `hjkl`/`0$`/`iaAoO`/`x`/`dd`/`gg`/`G`/`/n`, `:w :q :q! :wq`, Rust syntax highlighting (keywords/strings/comments/numbers) via the M7 TTF renderer, block/bar cursor, status line. Edit logic is pure (testable headlessly — DEMO 61 scripts gg→o→insert→Esc→:w + verifies the FS round-trip). Also: **2× console font** (`6230d97`, ~80×36 cells, readable) and a **scrollback pager** (`78b6bb2`, PageUp/PageDown/End over the byte ring, view freezes while reading). All keyless builds; 144 PASS / 0 FAIL / 0 #DF headless. Search-and-replace, multi-buffer, and the Ring-3 port are follow-ups. |
| Frame allocator 2026-05-27 | **per-app memory ceiling lifted + faster allocator** (toward hosting demanding apps). `MAX_FRAMES` 16384→131072: a tier pool was capped at ~64 MiB *regardless of RAM* (bitmap size), limiting any single app to ~64 MiB even on a big machine; now 512 MiB/pool (`pool_size = RAM/4` still binds in QEMU, so the ceiling shows on real hardware). Allocator rewritten from a from-zero linear bitmap scan (O(n)/alloc, O(n²) to fill) to **next-fit with a `next_word` cursor** (amortized O(1); free() biases the cursor back for prompt reuse). +56 KiB BSS, no layout #DF. Suite 155 PASS / 0 FAIL / 0 #DF. Note: past a few GB/pool a buddy/free-list allocator beats the bitmap. **FS large-file redesign is next** (the other half of the "demanding design app" assessment). |
| Snapshot u32 content_len 2026-05-27 | **large files persist** (lifts the 64 KiB persistence cap). Snapshot per-object `content_len` widened `u16`→`u32` (header +2 B, format VERSION 1→2 so stale snapshots are cleanly rejected as "fresh disk"); per-file check now bounds at `MAX_FILE_CONTENT` (256 KiB). `MAX_SNAPSHOT_BYTES` 64 KiB→1 MiB, and the save/load scratch moved from a stack array to a **heap** buffer (a 1 MiB stack buffer would overflow; a static would shift `.bss` and risk the layout #DF — heap is layout-safe). DEMO 60 extended: installs a 124 KiB ELF (`/apps/big-tool`, which the old u16 limit would have refused) + the small runnable app; two-boot test → boot 2 "loaded 163847 bytes" + both PASS (big file byte-for-byte). Why u32 not u64: a content length is bounded by `MAX_FILE_CONTENT` (256 KiB) and ultimately the 16 MiB heap — u32 (4 GiB) is already orders of magnitude past any reachable value; u64 would just waste header bytes addressing a range physics rules out. Suite 155 PASS / 0 FAIL / 0 #DF. |
| Install persistence 2026-05-26 | **installed apps survive reboot** (DEMO 60). On first boot (fresh disk) it installs `/apps/persistent-tool` + `SYS_FSYNC` (namespace → virtio0); on a later boot the boot-time `Namespace::load` restores it and the demo runs it. Two-boot test (shared vdisk): boot 1 installs, boot 2 → "loaded 39574 bytes from virtio0" + **DEMO 60 PASS: survived reboot and ran**, 0 FAIL. **Bug fixed:** the snapshot *deserialize* reconstructed object content via `from_inline` (256 B cap), so a restored 12 KiB ELF failed the whole load → flipped to `from_bytes` (heap-backed). Also made the install demos (58/59) unlink-before-create so the suite is reboot-safe. Notes: snapshot content_len is `u16`, so persisted files cap at 64 KiB (the 256 KiB in-memory cap can't all persist yet — needs a u32 format bump); QEMU testing needs `cache=writethrough` so writes survive an abrupt kill. Suite 155 PASS / 0 FAIL / 0 #DF (156 on a reboot, where DEMO 60 is a PASS). |
| demos.rs refactor (stage 1) 2026-05-26 | **extracted the recent agent/shell/TUI demos** (DEMO 47-59 era, 14 fns, ~785 lines) from the 6021-LOC `main.rs` into `kernel-x86_64/src/demos.rs` (`pub(crate)`, pulled in via `use crate::demos::*` in `init_loader_task`). main.rs → ~5240 LOC; **new demos now live in demos.rs**. Block was dependency-clean; layout shift didn't re-trigger the stack-guard #DF (256 KiB stack headroom held). Suite 156 PASS / 0 FAIL / 0 #DF. **TODO (deferred):** the older DEMO 0-46 era demos are interleaved with boot/runtime helpers (spawn_named, user_syscall, pump_keyboard, enable_sse, sem_demo_one, StatX/FutexWord) with no clean cut — migrating them is a layout-validated multi-stage job; do it *when it matters or when the kernel stack/layout is being reorganised anyway* (per user). |
| Install anywhere 2026-05-26 | **system-shell vision (4b/4): install anywhere / run anywhere.** `SYS_SPAWN` no longer needs the hardcoded `/bin` table — any absolute path routes to `spawn_namespace_elf`, which resolves the path, **tier-checks the caller against the executable** (a tier-0 agent can't run a higher-tier binary, mirroring the read gate), reads its ELF bytes from the object's heap content, and spawns. "Install" = write an ELF to a namespace path (persists to disk via `SYS_FSYNC`). Enabling changes: `MAX_CONTENT_SIZE` 64→256 KiB (covers the 124 KiB sem-sh; pure validation cap, heap-backed), and directories grew from 16→**64 entries** (dir content moved from 256 B inline to heap `from_bytes`, buffers 256 B→4 KiB) — a work OS needs more than 16 files. DEMO 58 installs a 12,720 B ELF at `/myapp` and runs it from the shell. Suite **155 PASS / 0 FAIL / 0 #DF**. Remaining: `$PATH` bare-name search (type `myapp`, not `/myapp`), per-path task names (vs generic `user-app`), reboot-persistence demo. |
| Shell scripting && / || 2026-05-26 | **system-shell vision (4c/4): conditional chaining.** sem-sh gains `&&` (run next only on success) and `||` (run next only on failure) with short-circuit, quote-aware and distinct from single-`|` pipes (`run_conditional` layer above `run_command`). DEMO 57 validates `true && echo CHAINED ; false && echo NOPE ; false || echo RECOVER` → CHAINED+RECOVER, NOPE skipped. Suite **154 PASS / 0 FAIL / 0 #DF**. **4b (PATH-anywhere) deferred:** `handle_spawn` only spawns `/bin/<name>` via a hardcoded name table; true "apps installed anywhere" needs namespace-stored executables + non-static-name spawn — a substantial kernel feature, not a shell add. `$()` command substitution + glob also remain. |
| Agent shell sandbox 2026-05-26 | **system-shell vision (4a/4): security — the LLM runs sandboxed.** The agent's `bash` tool now spawns sem-sh at **tier 0 (Public)** instead of tier 3 — the LLM is the least-trusted component in the 4-tier model, so its shell gets the lowest clearance. `SYS_OPEN`'s existing tier check (`caller_tier >= object_tier`) then denies the shell ANY Internal/Sensitive/Secret file, for both read and write — no new mechanism, just running the agent where it belongs. DEMO 56 proves it: a kernel-created Secret file is unreadable AND unmodifiable from the agent shell (`echo HACKED > /sec-doc` denied, content intact), while a Public file works. Directly fulfils "the LLM can't see secrets, can't modify protected state." Suite **153 PASS / 0 FAIL / 0 #DF**. (Remaining 4b/4c: PATH-anywhere exec + `&&`/`||`/`$()` scripting.) |
| Shell `fetch` 2026-05-25 | **system-shell vision (3/4): networking**. sem-sh gains `fetch <url>` — an HTTP/1.1 GET over the kernel TCP stack (`semos_std::net::TcpStream`, the same path DEMO 36's net-demo proved), writing the response to stdout so it pipes (`fetch ... | grep`). Pure Ring-3, no kernel change; the agent gets it via `bash`. HTTP only for now — the TLS stack is SPKI-pinned to the agent endpoint, so arbitrary HTTPS can't be validated (a CA-bundle verifier is the follow-up). DEMO 55 validates `fetch http://example.com/` → 837 B HTTP response + HTML. Suite **150 PASS / 0 FAIL / 0 #DF**. |
| recv-stall flake FIXED 2026-05-25 | **the live-agent reliability bug, root-caused and fixed.** A diagnostic (periodic `ticks()` log inside the net read spin) showed `now_tick` frozen across 16M spins: **interrupts were disabled during the recv spin**, so the timer IRQ never fired, `ticks()` never advanced, and the tick-based 30 s idle-timeout could never trip → a slow/stalled peer hung the kernel forever (the multi-session "DEMO 49 stuck at receiving" flake). DEMO 16/34/36 only escaped it by getting data fast. Fix: `enable_interrupts()` (new `Platform` hook) at the top of the `embedded_io` read/write spins in `net/tcp.rs` — a task-level blocking wait must let the timer fire + allow preemption. Validated live: DEMO 49 (agent loop) **and** DEMO 54 (`ask → "Two plus two is four."`) both pass, 0 stall spins, 3657 context switches. This also confirmed the TLS-from-syscall path for `ask` (RSP0 stack is fine). Keyless suite **149 PASS / 0 FAIL / 0 #DF**. |
| Agentic shell 2026-05-25 | **system-shell vision (2/4): the LLM in the shell**. sem-sh gains an `ask` builtin — `ask <question>` or `cmd | ask <question>` (pipes stdin in as context) — that reaches the kernel's network Claude agent via a new `SYS_ASK` (Ring-3 → `Platform::llm_ask` → `agent::ask`, a tool-free single-turn over one keep-alive TLS connection). The platform impl enables interrupts so the network call's wall-clock timeouts advance. Degrades to a clear message, never a hang, with no key / no network. DEMO 54 validates the full bridge **keyless** (Ring-3 `ask` → SYS_ASK → agent → back); live answers need a baked key. Suite **149 PASS / 0 FAIL / 0 #DF** keyless. Open: live `ask` reuses the Session path (so it's subject to the intermittent recv-stall flake), and the TLS-from-syscall stack depth on a Ring-3 RSP0 is still to be validated under load; security follow-up = tier-aware redaction so Secret content can't be sent to the API. |
| Shell-as-OS-interface 2026-05-25 | **system-shell vision (1/4): introspection**. sem-sh gains read-only `ps` (task table with **security tier** per task), `free` (heap usage), `uptime` — backed by new `SYS_PS` + wired `SYS_SYSINFO` (and existing `SYS_TIME`). All read-only and tier-safe: they expose task metadata + heap totals, never secrets or mutable state, so the agent can see the system it runs on but can't change it. DEMO 53 validates via the bash tool. **Spawn sustainability fix:** the agent `bash` tool now reaps its child at exit (`reap_slot`) so a command loop stays flat, and `MAX_PT_FRAMES` 512→2048 gives the non-reaping boot demo cascade headroom (the cumulative leak had drained the pool right at DEMO 52/53). `reclaim_dead_address_spaces` added as scaffolding for the eventual free-on-exit refactor. Suite **148 PASS / 0 FAIL / 0 #DF**. |
| M22 bash tool + grep 2026-05-25 | **agent `bash` tool**: `run_bash` spawns `/bin/sem-sh -c "<cmd>"` from kernel context, dups a pipe onto the child's stdout, and drains it interleaved (4 KiB pipe can't deadlock) — Claude gets the OS's real command surface (builtins, `;`/`|`, redirection, external ELF exec), not a reimplementation. Added a **`grep`** builtin to sem-sh (file + stdin-filter modes) so it's reachable via `bash`; bash tool description now advertises the available builtins to the model. DEMO 52 validates headlessly (write_file → `echo … ; grep NEEDLE /file` → captured + filtered). Suite **145 PASS / 0 FAIL / 0 #DF**. Open: wildcard glob expansion. |
| M22 split panes 2026-05-25 | **side-by-side TUI layout**: the middle row splits into a wider **conversation** pane (left, user+assistant) and a narrower **activity** pane (right, tool_use+tool_result), with a vertical accent divider; status bar + prompt stay full-width. DEMO 50 verifies the split by pixel readback — conversation colours appear only in the left rect and tool colours only in the right, with zero bleed across the divider (mutual exclusion proves the routing). The live agent (DEMO 49) renders into it via the same `push_*` methods. Closes the last M22 Claude-Code-parity item. Suite **144 PASS / 0 FAIL / 0 #DF** keyless. |
| M22 TLS keep-alive 2026-05-25 | **HTTP/1.1 keep-alive session** (`agent::Session`): a multi-turn conversation now rides ONE persistent TLS connection (`Connection: keep-alive` + exact response framing via `http::content_length` / `decode_chunked` as a completeness probe) instead of reconnecting per turn — that removes the inter-turn single-socket reconnect flake entirely AND returns the instant a body is framed (no trailing 30 s idle-timeout recv). Validated live: DEMO 49 ran both turns on one connect (`framed response 2001 B / 1990 B (conn kept alive)`, no reconnect before turn 2). Also gated DEMO 48 (no-key 401 test) to keyless boots so the keyed session opens on the 2nd TLS connect of the boot, not the 3rd (the flake worsens with connect count). `Session::request` reconnects+resends up to 4× to absorb the residual *initial*-connect flake. Suite **144 PASS / 0 FAIL / 0 #DF** keyless. |
| M22 TUI 2026-05-24 | **agent TUI** (DEMO 50 + DEMO 49 live integration): kernel-side three-pane terminal (`tui.rs`) over the M7/M8 `TtyConsole` — status bar / scrollback transcript / prompt, with role-coloured turns (user/assistant/tool_use/tool_result). DEMO 50 verifies every pane + each role's exact colour by pixel readback; DEMO 49's live loop drives the same panes as the conversation unfolds (real UI, not a mock). **Net stack fix:** adding the module overflowed the `init_loader` demo-runner task stack at DEMO 26 (`fs::paths::remove_child` + a timer frame tipped slot 5's 128 KiB `TASK_STACKS` guard → #DF) — the documented layout-sensitivity; bumped `TASK_STACK_SIZE` 128→256 KiB. Suite **142 PASS / 0 FAIL / 0 #DF** keyless. Remaining for full Claude-Code parity: side-by-side split panes + interactive keyboard input. |
| M22 stage C DONE 2026-05-23 | **native agent loop validated against LIVE Claude** (DEMO 49): seeds `/README`, asks Claude (real Anthropic API, `claude-haiku-4-5`) to use the `read_file` tool then summarize. Full loop runs: turn 1 → `tool_use(read_file {"path":"/README"})` → kernel runs the tool → turn 2 replays `assistant tool_use` + `user tool_result` → Claude returns the one-sentence summary. New: `agent::api_key()` (compile-time `option_env!("ANTHROPIC_KEY")`, key lands only in the gitignored binary), `Message::assistant_tool_use`, `decode_body` (chunked-aware), `send_over_tls` **3× retry loop**. **Two net reliability fixes were required and are the real value here:** (1) **rotating ephemeral local port** (`net/tcp.rs`: const `LOCAL_PORT` → `next_local_port()`) — the const port made the 3rd+ TLS reconnect in a boot hang in `poll_to_terminal` because SLIRP/peer hold the prior identical 4-tuple in TIME_WAIT and drop the new SYN; (2) **`IO_IDLE_TIMEOUT_TICKS` 10 s → 30 s** — an LLM's time-to-first-byte legitimately exceeds 10 s when *generating* a reply, which reported a premature EOF and failed the turn. DEMO 49 self-gates on a baked key (skipped in the committed keyless build). Suite still **139 PASS / 0 FAIL / 0 #DF** keyless; DEMO 49 PASS with key. M22 ✅. |

---

# Phase 9 — Bare-metal apps on top of the kernel

Goal: turn the kernel from "boots, makes a TLS call, sandboxes LLM access" into "first real app can run, read+write user files, and survive reboot."

```
                           Phase 9
                              │
            ┌─────────────────┼───────────────────────────┐
            ▼                 ▼                           ▼
       Filesystem      Time + identity                 Graphics
            │                 │                           │
   ┌────────┼─────────┐    ┌──┴───┐               ┌──────┼──────┐
   ▼        ▼         ▼    ▼      ▼               ▼      ▼      ▼
  M1     M4/5       M9    M2     M3              M6     M7     M8
 paths  syscalls  NVMe  RTC   USB-KBD             FB    fonts  vect.
        + persist
```

## M1 — Path namespace (Stage 1) `[✅]`

Hierarchical `/foo/bar` over SUID-addressed semantic objects.
Landed `872cfd2`. DEMO 17 covers it.

## M2 — RTC + wall_clock `[✅]`

MC146818 driver, `Platform::wall_clock()`, kernel-core free function wrapper.
Landed `991928b`. DEMO 19 covers it.

## M3 — USB stack (xHCI + HID keyboard) `[✅]`

Driver landed (`1301bcb`), USB enumeration unblocked by per-task
stack bump (`688a602`: TASK_STACK_SIZE 16 → 64 KiB). DEMO 18 covers it.

The "layout-sensitivity family" recurrences (#36, #40, #42) all
resolved 2026-05-18 to -19:
- #40 (kernel #PF at RIP=0): 258 KiB LlmContext stack overflow → static buffer
- #36 (USB triggers a layout-shift bug): TASK_STACK_SIZE 16→64 KiB
- #42 (small additions hang at "Initializing interrupts..."): the
  bootloader_api default `kernel_stack_size = 80 KiB` was being
  overflowed by `kernel_main`'s frame inflation from minor code
  changes. Fixed by setting `config.kernel_stack_size = 512 * 1024;`
  in BOOTLOADER_CONFIG (commit `b51e22a`). That single change
  unblocked the previously-reverted FWRITE>256 B work too (#44).

#41 — real unmapped guard pages between task stacks — **DONE 2026-05-20**
(`a9fa7d1`). Every TASK_STACK + per-task kernel stack now has an unmapped
guard page below it (2 MiB kernel PDE split into 4 KiB + PTE cleared +
`invlpg`; visible under all CR3s since process address spaces share the
kernel PML4). The whole layout-sensitivity family is now structurally
fixed: an overflow faults precisely instead of smashing the neighbour.
The guard immediately exposed two real latent overflows (per-task kernel
stack 8→64 KiB across #41/#55; TASK_STACK_SIZE 64→128 KiB). #55 (sequential
Ring-3 thread spawn / slot reuse) closed on the same fix (`e750ee8`).

**Status check after the #42 fix:**
- ✅ Root cause identified (main kernel stack default 80 KiB) and fixed
- ✅ `usb::init_and_enumerate()` runs without corrupting state (#36)
- ✅ DEMO 18 passes all 5 sub-checks against `qemu-xhci -device usb-kbd`
- ✅ All other DEMOs still pass in the same boot (28 DEMOs, 74 PASS lines)
- ⏳ ENV_BLOCK_SIZE bumped back to 2 KiB — not yet, but should be fine now
- ⏳ CSZ=1 metal validation on ThinkPad P1 — still pending hardware run

## M4 — FS Stage 3: `SYS_FS_*` syscalls `[✅]`

Path namespace exposed to user space via existing SYS_OPEN/CLOSE/
FREAD/FWRITE/STAT/MKDIR/UNLINK/READDIR numbers. Path-FD range
96..127 sits alongside legacy pipe/ramfs FDs. Tier-aware open
gate via `current_task_max_tier()`.
Landed `dfca48f`. DEMO 20 covers all 8 syscalls from Ring 0.
User-program port (fs-demo) still pending.

## M5 — FS Stage 2: snapshot persistence for the namespace `[✅]`

`Namespace::save(dev)` / `load(dev)` via `storage::snapshot`. Packed
FSNS format, BFS from root, RDRAND-backed `mint_suid` so persisted
SUIDs don't collide across boots, `created_at`/`modified_at` from
`platform::wall_clock()` populated on every mutation.
Landed `920e6da` (in-process roundtrip) + `1f62c08` (cross-boot
auto-load + idempotent DEMOs 17/20/21). Two-QEMU-cycle test
validates byte-exact restore with 450 s timestamp.
**Operational gotcha:** boot-time `Namespace::load(virtio0)` MUST
run AFTER `init_global_registry()` — that call clears the registry;
loading earlier wipes the entries. Verified by the log line
"loaded 643 bytes" still showing even when the data was wiped.
- [ ] Snapshot size limit (64 KiB today) documented as a "namespace
      metadata only" cap; large-object content goes into a separate
      per-object stream when that becomes necessary

## M6 — Framebuffer drawing API `[✅]`

Promote raw `set_pixel` to a real drawing surface.

Landed `6e972a2` (agent). DEMO 35 verified by pixel readback (111 PASS / 0
FAIL combined image). Implemented in `kernel-x86_64/src/framebuffer.rs`
(drawing API added to the existing console module). Detected live format on
QEMU: BGR, stride read from `FrameBufferInfo` (never assumed). `rgb(r,g,b)`
packs to the native order at write time. Only the user-mapped FB region is
deferred (a follow-up syscall) — core API + DEMO are done.

**Done when:**
- [x] `fb_fill_rect(x, y, w, h, color)`, `fb_blit(src, x, y, w, h)`,
      `fb_scroll(dx, dy)`, `fb_present()` as kernel-side functions — all clip
      to framebuffer bounds (no OOB writes)
- [x] Color format documented (BGR vs RGB) — derived from live
      `FrameBufferInfo`, packer switches on `PixelFormat`
- [ ] Shared-memory framebuffer region exposed to user space (mapped
      read/write into the process's address space for direct draw) —
      DEFERRED as follow-up to avoid scope creep; would use a new high
      syscall number (e.g. 60). Core drawing API + DEMO landed first.
- [x] Damage-rect / present model so apps don't tear writes — direct-render
      with accumulated damage rect; `fb_present()` is the commit point
      (back buffer skipped: ~3.5 MiB cost not justified for single surface;
      every pixel write funnels through `FbSurface` so it can be retargeted)
- [x] DEMO 35 draws a checkerboard + rect + blit + scroll, verified by
      reading pixels back from framebuffer memory (headless-safe)

## M7 — Font rasterization `[✅]`

Render text in real fonts, not just the 8x16 bitmap console. Landed
`b059960`. **Used `ttf-parser` instead of fontdue:** fontdue needs an
allocator and isn't available offline, and the kernel has no global
allocator. `ttf-parser` (cached, zero-allocation, no_std) gives glyph
outlines; we rasterize them ourselves.

**Done when:**
- [✅] Outline source: `ttf-parser` 0.25 (`default-features=false`,
      `no-std-float`) — zero-alloc, no_std. (fontdue substituted; see above.)
- [✅] A TTF embedded in the kernel: Noto Sans Regular (SIL OFL 1.1),
      `include_bytes!` in `kernel-x86_64/src/font.rs` (`assets/`).
- [✅] `fb_draw_text(x, baseline_y, str, px, color)` flattens outlines
      (lines + quad/cubic Béziers) into a fixed stack edge buffer and
      scanline-fills (even-odd, 1-bit; AA deferred to M8) via M6's fb_fill_rect.
- [✅] DEMO 37 renders a string at 16/24/40px, verified by pixel readback
      (60+ glyph px, <80% coverage, proportional to size). 114 PASS / 0 DF.
- [✅] Follow-up — routing a *console* through this: `tty::TtyConsole`
      (`78ae59e`, DEMO 39) renders a cursor-managed console (newline, wrap,
      region scroll) via `font::with_face`/`FaceCtx`. NOTE: it's a *region*
      console, not the default `print!` sink (the bitmap stays the boot sink —
      serial is grep truth + the ~16 KiB glyph-raster frame must not run on the
      #41/#55-sensitive interrupt/syscall print path). It's the M19 renderer.
- [ ] Follow-ups still open: kerning/shaping, a glyph cache (re-parses the
      face per `with_face` call today).

## M8 — 2D vector rasterizer (tiny-skia) `[✅]`

Anti-aliased lines/curves/fills for the design apps. Landed `cb6c726`.

**Two things landed together:**
- **Kernel global allocator** — the existing 16 MiB free-list heap arena
  (`kernel_core::memory::heap`, init'd at boot) is now wired as
  `#[global_allocator]` in kernel-x86_64 + `extern crate alloc`. The kernel
  has `Box`/`Vec`/`String` (kernel-core itself stays no-alloc — this is
  binary-side). Unblocks tiny-skia and future kernel work (TTY/shell/agent).
- **tiny-skia 0.11** (cached; `default-features=false` + `no-std-float` →
  no_std + alloc, Apache-2.0). `kernel-x86_64/src/gfx2d.rs` rasterizes paths
  with real AA into an in-heap `Pixmap`, then blits to the M6 framebuffer.

**Done when:**
- [✅] `tiny-skia` as a no_std + alloc dependency (NOT vendored/no_alloc —
      the kernel-allocator route is cleaner and unblocks alloc generally).
- [✅] `gfx2d::aa_scene` (fill + stroke) over M6's `fb_blit`. (Generic
      `fb_stroke_path`/`fb_fill_path` wrappers are the obvious next step.)
- [✅] DEMO 38 draws a filled circle + a stroked cubic Bézier; verified by
      pixel readback — 19748 lit px incl. 974 *blended* AA-edge px (the AA
      signature M7's 1-bit fill lacked). 116 PASS standard / 130 with -netdev.
- [✅] Follow-up — AA text: `gfx2d::aa_draw_text` (`78ae59e`) rasterizes TTF
      glyph outlines through tiny-skia with `anti_alias = true` and blits;
      it's the `Aa::Smooth` mode of `tty::TtyConsole` (DEMO 39: 1661 AA-edge px).
- [ ] Follow-ups still open: grow a real drawing API; gradients/clips.

## M9 — NVMe driver `[✅]`

Block storage on real hardware (**P1 stage only** — the T540 is SATA, not
NVMe, see Phase 10). QEMU's NVMe model proved the bring-up in-tree. v1 landed
`53cdc1a` (DEMO 62).

**Companion:** AHCI/SATA driver landed `ed2630f` (DEMO 67) — the T540 path.
Same `BlockDevice` shape; both NVMe and SATA register at boot, whichever
hardware is present takes effect. See `kernel-x86_64/src/ahci.rs`.

**Done when:**
- [✅] PCI discovery of NVMe controller (class 0x010802) — `find_by_class`
- [✅] Submission/completion queue pair setup (admin + I/O qid 1, polled)
- [✅] Identify Namespace (NSZE + active LBA format) — pulls block_count + block_size
- [✅] Read/Write commands via I/O SQ/CQ (NVM opcodes 0x02/0x01, PRP1)
- [✅] Wired as a `BlockDevice` named `nvme0` (`drivers::registry`)
- [✅] DEMO 62 writes a pattern to LBA 100 + reads it back byte-for-byte;
      first-boot validation 146 PASS / 0 FAIL / 0 #DF
- Follow-ups: MSI-X (interrupts vs polled), multi-block PRP lists, error
  recovery beyond a polled timeout. v1 = one LBA per command (BlockDevice
  layer loops), no interrupts.

---

# Phase 10 — Bare-metal readiness + Wi-Fi

Goal: the kernel boots on real hardware, runs the same DEMOs, and can
reach api.anthropic.com over Wi-Fi (currently TLS works via QEMU SLIRP
forwarding).

**Two-machine bring-up (T540 on the way 2026-05-28):**
- **Stage 1 — ThinkPad T540 (ACQUIRED, on the way).** i7-4600M Haswell,
  8 GB RAM, 256 GB SATA SSD, Win10 preinstalled. Removable mini-PCIe
  Wi-Fi (likely Intel 7260 AC), Intel HD 4600 iGPU only. Validate the
  **bootloader + kernel on real metal** (M10 pre-flight, first-boot,
  USB, task#40 on a real APIC), then **Wi-Fi (M11)** via iwlwifi 7260
  (different firmware blob than AX211 but same driver shape — our M11
  v1 PCI ID table + frame builders cover both already).
  **T540 deltas vs the earlier T440p plan:**
  - **SSD is SATA, not NVMe** (T540-era predates factory NVMe). M9 NVMe
    does NOT exercise here — that waits for the P1. To use the internal
    disk on the T540 we need a new **AHCI/SATA driver**. For initial
    metal bring-up we can avoid that by booting from USB.
  - **USB Mass Storage** class becomes a meaningful goal: boot-from-USB
    needs it on metal, independent of any AHCI work.
  - Windows 10 stays on the disk; we dual-boot off USB. Disable Secure
    Boot in firmware.
- **Stage 2 — ThinkPad P1 Gen 6 (the real target, later).** Only once
  proven on the T540. This is where **GPU work begins** (Phase 11/12,
  Iris Xe + NVIDIA) AND **M9 NVMe gets its first real-hardware test** —
  the T540's SATA SSD means M9 stays QEMU-only until then. HD Audio
  (M15), CDC-ECM, HID parser, 802.11 protocol layer are all QEMU /
  canned-test validated and ready for either machine.

## M10 — Pre-flight checklist for bare-metal boot `[🔨 v1 audit + watchdog]`

Find and fix everything that "passes in QEMU, fails on metal" before
the first real-hardware session. v1 landed `d77ba87` — audit + watchdog
+ one fixed latent bug.

**Done when:**
- [📝] Serial-over-USB plan documented — for now: skip serial entirely on
      the T540 (its serial header is internal and atypical) and rely on
      the framebuffer console as the only output channel. Revisit if a
      USB-serial debug path matters; for the T540 framebuffer is enough.
- [✅] **Framebuffer-only fallback verified** — `serial::_print` already
      mirrors output to `framebuffer::_print`, and the panic handler uses
      `println!`, so panics ARE visible on metal-without-serial. No code
      change needed; documented in code.
- [✅] **xHCI CSZ=1 (Intel 64-byte contexts)** — landed `8821df1`. The
      previous "abort on CSZ=1" branch is gone; `InputContext` /
      `DeviceContext` are raw byte buffers sized for the max (CSZ=1)
      layout, and accessors compute offsets using a runtime `CTX_SIZE`
      set once during xhci bring-up. CSZ=0 regression-clean in qemu-xhci
      (same 165 PASS); CSZ=1 path will exercise on the T540 day-one.
- [✅] **RTC firmware-century-byte assumption** — `rtc.rs:65-226` reads
      the ACPI FADT-set CENTURY register with a 0-fallback. Already
      handles real-BIOS variance.
- [📝] VT-d disabled in BIOS OR identity-IOMMU implemented — BIOS knob;
      no code. Confirm during T540 first-boot.
- [✅] **"Kernel didn't crash" watchdog (one-shot v1)** — `[heartbeat]
      kernel reached idle — ticks=N` printed at end of boot, mirrors to
      framebuffer. Presence + correct N = boot succeeded. **Latent bug
      fixed along the way:** `TIMER_TICKS` was `spin::Mutex<u64>` →
      `AtomicU64`, eliminating an ISR-vs-reader deadlock pattern.
      Continuous beats need a kernel idle-task slot — follow-up.

## M11 — iwlwifi driver `[🔨 v1 protocol layer; device on metal]`

802.11 over Intel WiFi. Two-stage hardware bring-up: T540 (7260/3160
mini-PCIe) first, then P1 Gen 6 (AX211). v1 in-tree protocol scaffolding
landed `a0d487b` (DEMO 65) since QEMU emulates no wireless — everything
else here waits for a T540 in hand.

**Done when:**
- [✅] **802.11 MAC: management frame builders** (Probe Request,
      Open Authentication, Association Request) + EAPOL-Key Msg2 —
      byte-validated against the spec layout in DEMO 65
- [✅] iwlwifi PCI device-ID table (T540 7260 family + P1 AX211)
- [ ] Intel firmware blobs (`iwlwifi-...ucode` + `.pnvm`) embedded
- [ ] Firmware upload + secboot succeeds; ALIVE event received
- [ ] PHY init: NVM + PNVM + regulatory + channel calibration
- [ ] WPA2 four-way handshake in software (MIC over derived PTK),
      CCMP encrypt/decrypt offloaded to firmware after keys installed
- [🔨 protocol] **CDC-ECM USB Ethernet path** as the fallback so the
      TLS stack can be exercised on metal before Wi-Fi works.
      Protocol v1 landed `e79a3a3` (DEMO 66): class constants, config
      descriptor walk (control + Ethernet functional + Data alt with
      bulk pair), MAC string decode. Live xHCI bulk-endpoint TX/RX is
      the follow-up — settled with real hardware in hand.
- [ ] DEMO repeats: associate to a hardcoded SSID, get DHCP, redo the
      Anthropic TLS round-trip over real Wi-Fi

## M12 — DNS resolver `[✅]`

Replace the hardcoded Anthropic IP in DEMO 16.

Landed `f19da16` (agent resolver + integration fix). `kernel-core/src/net/dns.rs`:
A-record query builder, compression-aware response parser, 8-entry cache, UDP
over the shared smoltcp `SocketSet`. The fix that made it work: **wait on
wall-clock (`platform::ticks()`, ~3s) not iteration count, and retransmit
~4×/s** — the agent's 4000-poll loop spent only a few ms, so a warm name
resolved but a cold SLIRP→host lookup timed out; UDP also has no retransmit, so
a datagram dropped pending the 10.0.2.3 ARP was lost. DEMO 34 resolves
example.com + checks the cache; DEMO 16 resolves api.anthropic.com (hardcoded
IP kept as fallback). Skips cleanly without `-netdev`. With network: 121 PASS.

**Done when:**
- [x] UDP socket on top of smoltcp
- [x] DNS request builder (A record, ID + flags + question)
- [x] Response parser (compression pointers handled)
- [x] `dns::resolve(host) -> Option<Ipv4Address>` with cache
- [x] DEMO 16 calls `dns::resolve` first (falls back to hardcoded IP)
- [x] DEMO 34 resolves example.com over SLIRP (10.0.2.3) + cache check

*De-flaked (was "known intermittent, not M12"):* DEMO 27's "sibling Blocked
after sleep" assertion is **FIXED 2026-05-22 (`78ae59e`)** — the fixed-sleep
one-shot now polls (1 tick × up to 200) and succeeds the instant Blocked is
seen ("Blocked after 1 tick").

## M13 — Chunked-transfer-encoding parser `[✅]`

DEMO 16's body preview showed `8d` (the chunk length header) before this.

Landed `d748556` (agent + integration). `kernel-core/src/net/http.rs`:
`decode_chunked(input, out)` (slice-in/slice-out — kernel-core has no
allocator) + `is_chunked(headers)`. Handles multi-chunk, hex/mixed-case
sizes, chunk extensions, trailing headers; errors cleanly on truncated
input. DEMO 33 (4 sub-checks, all green) validates it.

**Done when:**
- [x] `decode_chunked` decoder that produces the unchunked bytes
- [x] NetworkLlmProvider de-chunks the body before JSON extraction
- [x] DEMO 33 validates the decoder against crafted vectors (DEMO 16's live
      body preview will show JSON once a real authenticated call is made)

---

# Phase 11 — Rendering + media (post-Phase-9, post-network)

## M14 — iGPU (Iris Xe) rendering driver `[  ]`

3D rendering for the CAD verification view, video playback, retro
games. Intel docs are public; Linux's `i915` is permissively-licensed
reference material. NVIDIA dGPU does **not** get a graphics driver —
stays compute-only.

**Done when:**
- [ ] PCI discovery + MMIO map
- [ ] Display engine init (modesetting via Type-C eDP)
- [ ] Render engine: command streamer, batch buffer submission
- [ ] Simple test: clear screen to a color via the GPU
- [ ] Texture upload + sampling
- [ ] DEMO 27 draws a rotating textured cube

## M15 — HD Audio driver `[✅]`

Prerequisite for games and video playback. v1 landed `3f8fed2` (DEMO 63).
QEMU's `-device intel-hda -device hda-output` proved the full path in-tree.

**Done when:**
- [✅] Intel HDA controller bring-up (reset, GCTL, STATESTS poll)
- [✅] Codec enumeration (root → AFG → first DAC + first Pin Complex)
- [✅] PCM output stream (48 kHz, 16-bit stereo) — BDL + stream descriptor +
      RUN; verbs via the Immediate Command Interface (ICI) since CORB/RIRB
      via DMA was flaky in QEMU on the second verb
- [✅] DEMO 63 plays a 440 Hz sine through a cyclic 4 KiB BDL and verifies
      LPIB advanced (DMA active), 147 PASS / 0 FAIL / 0 #DF
- Follow-ups: CORB/RIRB path (real-hardware preferred), MSI-X interrupts,
  multi-stream / capture (ADC), gapless start (currently the cyclic loop
  has a small click at the buffer wrap; choose a buffer length that's a
  whole number of 440 Hz periods to fix).

## M16 — USB HID gamepad `[✅ parser; live xHCI wiring on metal]`

v1 landed `d4b8e2d` (DEMO 64). QEMU has no gamepad device, so v1 ships
the actually-hard piece — the report descriptor parser — as a pure
module, validated by canned descriptor + synthetic report. Wiring to a
real gamepad over xHCI is a small extension once T540/P1 is around.

**Done when:**
- [✅] HID report descriptor parser (real one, not boot protocol) —
      `usb::hid_report::parse` handles short items, Usage Min/Max,
      multi-usage Input items, Output/Feature offset, signed extension
- [✅] Gamepad axis + button report parsing —
      `decode_gamepad()` returns `{x,y,z,rx,ry,rz,hat, buttons:u32}`
- [✅] DEMO 64 parses a canonical Game Pad descriptor (X+Y signed 8-bit
      + 4 buttons + padding) and round-trips a synthetic report
      (x=66, y=-2 sign-extended, buttons=0b1010). 150 PASS / 0 FAIL.
- Follow-ups: fetch a HID Report Descriptor over a USB control
  transfer in xHCI; route input reports through the parser; expose a
  Gamepad input device. All hardware-gated.

## M17 — Software video decoder (H.264 minimum) `[  ]`

Playback, not editing. Editing is post-EOY.

**Done when:**
- [ ] H.264 baseline profile decoder (vendored or own)
- [ ] Audio sync via M15
- [ ] DEMO 30 plays a short test clip from ramfs

---

# Phase 12 — Compute-only NVIDIA dGPU path

Local LLM inference as a v2 alternative to remote-via-Wi-Fi.
Tinygrad-NV-style: PTX direct submission, no graphics.

## M18 — NVIDIA dGPU compute driver `[  ]`

**Done when:**
- [ ] PCI discovery + MMIO map
- [ ] GSP firmware upload
- [ ] Channel allocation + DMA buffer mapping
- [ ] PTX kernel submission via host queue
- [ ] CUBLAS-equivalent for the matrix shapes Claude-small needs
- [ ] DEMO 31 runs a single transformer layer forward pass and
      prints the output

---

# Phase 13 — Self-development on the metal

Goal: the user sits at the ThinkPad P1 running Semantic OS, opens a
Claude Code-equivalent agent on the framebuffer, asks Claude to
modify the kernel, sees the change applied to source files on disk,
triggers a build, reboots into the changed kernel. North star — the
moment Semantic OS hosts its own development loop, every subsequent
phase moves faster.

Depends on: Phase 9 done (FS + paths + syscalls), Phase 10 done
(Wi-Fi + DNS, so the agent can reach Anthropic). Framebuffer +
fonts (M6 + M7) are visual prerequisites.

## M19 — TTY layer `[✅]`

The framebuffer console is write-only today. A shell needs bidirectional.
**Done 2026-05-22** — renderer + stdin line-editing/history + ANSI output +
per-process stdio + scrollback all landed and boot-validated (DEMO 39–44,
145 PASS). Remaining nice-to-haves (ANSI scroll-region escapes, raw/cbreak
mode) deferred to when M20/M22 actually need them.

**Renderer (`78ae59e`, DEMO 39):** `tty::TtyConsole` — cursor-managed console
with newline, wrap, region scroll, fg/bg color, M7-sharp / M8-AA glyph modes.

**stdin + ANSI (`716eafd`, DEMO 40):** cooked-mode line discipline + AnsiTty.

**Per-process stdio — full FD-table refactor (`673d948`+`efd444e`+`21dbd8f`,
DEMO 41/42):** every FD (console/pipe/path/ramfs) now lives in the running
process's `FdTable`; the global `PATH_FDS`/`PIPE_FDS` statics are gone. stdio
is routable (`dup2` a pipe onto fd 1) and **inherited across spawn**, so a
parent can redirect a child's stdio. Resolved via the live scheduler slot
(not the stale `current_pid()`), with a stale-`task_id` slot-reuse fix.
Validated 140 PASS / 0 FAIL / 0 #DF.

**Done when:**
- [✅] Buffered stdin with line-editing — cooked-mode line discipline
      (`tty::input_push`/`drain`) with an in-line cursor, mid-line insert/
      Backspace, **arrow keys** (`ESC[A/B/C/D`, emitted by PS/2 0xE0 + USB HID)
      and **8-entry command history** (Up/Down). Surfaced as `SYS_READ` fd 0.
      DEMO 43.
- [✅] ANSI escape sequence handler (`tty::AnsiTty`): SGR color
      (30-37/90-97/39/0), clear screen (`2J`), clear-to-eol (`K`), cursor
      position (`H`/`f`). Cursor positioning uses a nominal cell width (font
      is proportional). Scroll-region escapes not yet parsed.
- [✅] Scrollback — `TtyConsole` line-oriented scrollback ring (64 lines);
      `show_scrollback(top)` re-renders scrolled-off output. DEMO 44.
- [✅] Per-process stdin/stdout/stderr — done via the full per-process
      `FdTable` refactor (DEMO 41 routable stdout, DEMO 42 inherited-on-spawn).
- [✅] DEMOs 40–44: stdin+ANSI (40), pipe-redirected stdout (41), FD
      inheritance across spawn (42), line editing + history (43), scrollback
      (44). (Next free DEMO is 45.)

## M20 — Native shell (`sem-sh`) `[✅]`

Rust shell — no bash compatibility, just what we need. `user-programs/sem-sh`,
built on `semos-std`. **Done 2026-05-23** across stages A (`5398720`), B
(`b81251d`), C (`96fbaf9`); DEMO 45/46; 150 PASS.

**Done when:**
- [✅] Line editor on top of M19 with history (arrows + Up/Down); reads cooked
      lines via `SYS_READ(0)`.
- [✅] Command parser: argv splitting + quoting + `;`/newline + `$VAR` + the
      `< > >> |` metacharacters.
- [✅] Builtins: `echo`/`pwd`/`cd`/`exit`/`true`/`false`/`cat`/`ls`/`which`/`env`
      (`cat` with no args is a stdin filter; `env` prints named vars only).
- [✅] Exec native ELF programs via `process::Command` (`name` → `/bin/name`).
- [✅] Pipes (`|`) and file redirection (`>`, `>>`, `<`) — concurrent (external
      producers spawn under the scheduler; see follow-ups). Exposed two kernel
      fixes: SYS_WRITE now routes through `handle_fwrite` (so a redirected file
      fd 1 actually writes the file), and Path `handle_fwrite` is positional
      (sequential writes accumulate, not overwrite).
- [ ] Job control deferred to a follow-up; not in v1.
- [✅] DEMO 45 (REPL/builtins) + DEMO 46 (`echo > file; cat file; echo | cat`).

**Follow-ups:**
- [✅] per-fd pipe-end refcounting (`0b4a6bb`) — readers/writers counts; dup
      increments, close decrements, EOF at 0. Removed the shell's fragile
      close-ordering dependency.
- [✅] `>>` true-append (`763188a`) — `>` truncates, `>>` seeks to EOF; relies
      on the positional Path writes from stage C.
- [✅] **concurrent pipes** (`9d89dbb`) — external producer stages spawn
      concurrently (Command::spawn, no wait); the consumer blocks in user space
      on a WOULDBLOCK sentinel until EOF. Built on: spawn-inherit pipe
      refcount increment + **exit-time FD cleanup** (a producer's exit drops
      its write-end ref → consumer sees EOF). DEMO 46 `/bin/hello-std | cat`.
- [ ] bare `env` enumeration (needs an enumerate syscall).

**Gotcha (cost time in stage A):** a new user crate builds as PIE (ET_DYN)
unless it copies `build.rs` + `link.ld` + `.cargo/config` (non-PIE EXEC at
0x400000) — the kernel applies no relocations, so `println` crashes while raw
syscalls work. See `feedback_new_user_program_nonpie.md`.

## M21 — Native editor `[✅ v1]`

Edit source files in-place. Not vim-compatible, just usable. v1 landed
`94581a8` (DEMO 61): kernel-side modal editor, launched by sem-sh's `edit
<file>` (SYS_EDIT → `Platform::run_editor` → `editor::run`).

**Done when:**
- [✅] Modal (vi-style) — chosen with the owner; Normal/Insert/Command
- [✅] Open/save against FS Stage 3 syscalls (save = truncate + write)
- [✅] Basic Rust syntax highlighting (keywords + strings + comments +
      numbers) via the M7 TTF renderer; full tree-sitter is later
- [✅] Search (`/term` + `n`); **replace deferred** to a follow-up
- [ ] Multi-file open (tabs or buffers) — deferred; v1 is single-buffer
- [✅] DEMO 61 opens a file, edits a line (gg→o→insert→Esc→:w), saves,
      re-reads to verify; 144 PASS / 0 FAIL / 0 #DF headless
- Follow-ups: search-and-replace, multi-buffer, and the **Ring-3 port**
  (needs a user-space framebuffer surface — open M6 follow-up); v1 is
  kernel-side, reusing the agent-TUI stack.
- Keys: `h j k l`+arrows, `0 $`, `i a A o O`, `x`, `dd`, `gg`, `G`,
  `/`+`n`; `:w :q :q! :wq :x`; Insert: text/Enter/Backspace/Tab/Esc.

## M22 — Claude agent client (native Rust port) `[✅ agent loop live]`

The reason for all of the above. A TUI agent like Claude Code but
written for this kernel, talking to the Anthropic API over the
TLS stack from Phase 8 + Wi-Fi from Phase 10.

**Stage A (`34ef9ee`, DEMO 47):** the agent *core*, no network — lives in
`kernel-x86_64/src/agent.rs` (alloc + kernel syscall/TLS surface; the native
TUI Ring-3 wrapper is a later refactor, needs TLS exposed to Ring-3).
**Stage B (`9da1f51`, DEMO 48):** request over **live TLS** to api.anthropic
.com — `build_http_request` + `send_over_tls` → HTTP 401 round-trip (no key).
Required the TcpStream reconnect fix (`efd8c3c`: free the smoltcp socket on
Drop) so the agent can open a fresh connection per call.

**Stage C (DEMO 49):** the full reasoning loop, validated against the **live
Anthropic API** (`claude-haiku-4-5`). The kernel seeds `/README`, asks Claude
to read it via the `read_file` tool, runs the tool, replays
`assistant tool_use` + `user tool_result`, and gets back the summary. Required
two net-reliability fixes (the real engineering content): a **rotating
ephemeral local port** (the const port hung the 3rd+ reconnect on TIME_WAIT)
and a **30 s IO idle timeout** (10 s was shorter than an LLM's time-to-first-
byte), plus a **3× retry** in `send_over_tls` for the residual single-socket
reconnect flake. The key is supplied at compile time via
`option_env!("ANTHROPIC_KEY")` so it only ever lands in the gitignored binary;
DEMO 49 self-skips in the committed keyless build.

**Stage D (DEMO 50 + DEMO 49 integration):** the **TUI** — a kernel-side
three-pane terminal (`tui.rs`: status bar / scrollback transcript / prompt)
over the M7/M8 `TtyConsole` panes, with role-coloured turns (user / assistant /
tool_use / tool_result). DEMO 50 verifies every pane + role colour headlessly
by pixel readback (Sharp glyphs fill solid colour, so each role's exact colour
is counted). DEMO 49's **live** loop drives the same panes as the conversation
unfolds — `set_status` while it connects/runs a tool/thinks, `push_*` per turn —
so it's the real agent UI, not a mock. (Adding the module overflowed the
`init_loader` task stack at DEMO 26 → bumped `TASK_STACK_SIZE` 128→256 KiB.)

**Stage E (DEMO 51 + DEMO 49 prompt):** **interactive keyboard input**. The
cooked-mode line discipline (`tty::input_push`, fed by the PS/2 ISR and a new
`pump_keyboard` USB-HID poll) → `tty::peek_line` snapshot → `Tui::read_line`,
which echoes the in-progress line into the prompt pane (Backspace + arrow
editing all work) and returns the committed line on Enter. DEMO 51 validates
the path headlessly by injecting keystrokes (incl. an edit + Backspace),
pixel-checking the prompt echo, then confirming `read_line` returns the
assembled line. DEMO 49's live loop now reads its question through `read_line`
(real keyboard on metal; injected headless) → so you type a question and Claude
answers in the TUI.

**Done when:**
- [x] TUI render loop on M19/M20 — **side-by-side split panes** (status bar /
      conversation | activity / prompt): user+assistant turns on the left,
      tool_use+tool_result on the right, role colours, scrollback. DEMO 50
      verifies the split by pixel readback (conversation colours land only in
      the left rect, tool colours only in the right — no bleed across the
      divider). Live loop renders into it (DEMO 49).
- [x] Interactive keyboard input — `Tui::read_line` over the cooked line
      discipline + USB/PS2 pump, prompt echo + editing (DEMO 51); the live agent
      reads its question through it (DEMO 49).
- [x] Agent message loop — full send→parse→tools→resend loop live (DEMO 49).
- [x] Tool use: `read_file`/`write_file`/`bash` all live. `bash` spawns
      `/bin/sem-sh -c` and captures stdout (DEMO 52); `grep` added as a sem-sh
      builtin so it's reachable through `bash` (and `ls` covers directory
      listing). True wildcard `glob` expansion in the tokenizer is still open.
- [x] Multi-turn conversation — message model + multi-turn request building +
      tool_use/tool_result replay validated; context truncation/window still open.
- [~] API key: compile-time `option_env!` works; `/etc/anthropic-api-key`
      runtime load is the remaining persistent mechanism.
- [x] DEMO (stage C, DEMO 49): boots, asks Claude to read README and summarize;
      agent calls `read_file`, returns the summary (live key + net). ✅

**Remaining for a full Claude-Code-equivalent:** the Ring-3 TUI wrapper (needs
TLS exposed to Ring-3), `bash`/`grep`/`glob` tool dispatch, and context-window
management. The core loop — the hard part — is proven.

## M23 — Build pipeline (cross-build over network) `[  ]` — OPTIONAL FALLBACK

User has chosen Phase 14 (self-hosting on the metal) as the
committed build path. M23 stays in the roadmap as a fallback in
case Phase 14 stalls badly enough that we still want the
"changes-edit-reboot" loop working in the meantime via a network
build. **Skip this milestone unless Phase 14 hits a wall.**

If picked up:

**Done when:**
- [ ] Network protocol for "push these files, build, return image"
      (could be: git push → CI webhook → image download; or a
      simpler custom HTTP service)
- [ ] HTTPS POST + GET on top of the TLS transport (currently we
      only do POST in NetworkLlmProvider; need general HTTP)
- [ ] Saved disk image installed to the boot partition
- [ ] DEMO 36 pushes a no-op change, receives the new image, and
      writes it to a staging path (actual reboot is M24)

## M24 — Reboot-into-new-kernel `[  ]`

**Done when:**
- [ ] Replace the running kernel image on the boot device with the
      new one (BIOS/UEFI partition write)
- [ ] Trigger a clean reboot (ACPI / triple-fault / power cycle —
      pick the cleanest available)
- [ ] DEMO 37 (last in this phase): with the agent loop running,
      apply a self-modifying patch (say, change a banner string),
      build, reboot, verify the change is live

## Out of scope for this phase

- **Port rustc + LLVM as-is (Phase 14).** Achievable but bigger
  scope than Phase 13 needs. Moved to its own phase below — the
  realistic shape is **port std + adopt Cranelift**, not "rewrite
  LLVM from scratch."
- **JS runtime port (for running upstream Claude Code as-is).**
  Easier than rustc but still enormous; the native Rust agent
  (M22) bypasses the need.
- **Tree-sitter / LSP** — nice to have but not required for the
  "make a kernel change with Claude's help" loop.

---

# Phase 14 — Self-hosting compilation (COMMITTED PATH, tracked as research)

Goal: rustc + cargo run *on* Semantic OS, building Semantic OS.
**User-chosen committed path** for kernel self-development. The
ThinkPad P1 running Semantic OS hosts its entire dev loop — edit,
build, reboot, no other machine in the loop. Phase 13 M23 (network
build server) is the fallback if this stalls.

Runs in parallel with Phase 13's M19-M22 + M24 (TTY, shell, editor,
agent, reboot-into-new-kernel — all still needed regardless of where
the compiler runs). Independent of Phases 11/12 (rendering / NVIDIA
dGPU).

## Tracked as a research project on AI-assisted porting

User decision: this phase doubles as **research into AI usage** —
how productive is LLM-driven compiler porting, where does iteration
overhead actually come from, what's the real ratio of generated to
kept code on a project this size? **No wall-clock estimates** here
because they'd be guesses; we measure as we go.

Per-session metrics worth tracking (write into the commit body or
a `docs/RESEARCH-LOG.md` as the phase progresses):

- Tokens generated (rough — agent run length × rate)
- LOC added to repo
- LOC deleted (iteration cost)
- LOC kept after the session (net useful delta)
- Build attempts before clean
- Bugs caught at compile vs at test vs at runtime
- Subjective: was this session bottlenecked by agent throughput,
  by iteration cycles, by underlying-bug debugging, or by
  spec/code-reading time?

After 5-10 sessions we should have honest empirical numbers
to replace the LOC-budget guesses below.

## Starting LOC hypotheses (validate during research, don't trust)

| Component | LOC guess | Notes |
|---|---|---|
| std shim over our syscalls (M25) | ~30K | Probably the highest-iteration component — std's surface is broad and tests are unforgiving |
| Spawn/wait + thread/sync syscalls + scheduler upgrade | ~15K | Kernel-side prerequisite for std::process and std::thread |
| Memory allocator (jemalloc-class minimum) | ~8K | Could vendor an existing one; net new work smaller |
| Vendor + integrate Cranelift (M26) | mostly read+review | Cranelift exists (~150K LOC). Integration is the work |
| First rustc build on Semantic OS (M27) | iteration only | The test-suite phase. Open-ended |
| Self-bootstrap (M28) | the moment, not the work | Validation, not coding |

**Why not port LLVM from C++ to Rust?** ~10M LOC of C++. Cranelift
(~150K LOC of Rust, exists today) gives us a Rust-native codegen
backend that's "good enough" for self-hosting. Drop LLVM entirely
on Semantic OS; keep it on the build server (Phase 13 M23) if we
want the full optimizer.

**Why not run upstream Claude Code (Node.js) instead of M22's
native Rust port?** Node.js + V8 is ~5M LOC of C++. The native
Rust agent (M22) is ~4K LOC, ships with Phase 13.

## M25 — std shim over Semantic OS syscalls `[🔨 substantial — `semos-std` lands the core surface]`

Get upstream rustc's std dependencies satisfied on our kernel.
`user-programs/std-shim` (crate `semos-std`) is the implementation;
hello-std / vec-demo / std-demo exercise it as DEMO 29–31.
**Caveat (#54):** shim programs MUST build at `opt-level=0` — any
optimization miscompiles the `asm!`-based syscall wrappers.

**Tier 1 prereqs (all ✅ as of 2026-05-18 — M25 unblocked to start):**
- ✅ Real general-purpose allocator (heap alloc, `9a5850e` — `SYS_HEAP_ALLOC`/`SYS_HEAP_FREE`)
- ✅ argv/envp passthrough in SYS_SPAWN (`8937041` — `setup_user_argv` Platform
  method writes SysV layout to new process's user stack; SpawnArgs struct passed
  via syscall arg3)
- ✅ Per-process env block + CWD (`8a3c29f` — `SYS_GET_CWD`/`SET_CWD`/`GET_ENV`/`SET_ENV`,
  inherit-on-spawn)
- ✅ SYS_FS_* surface (`dfca48f` — already done for std::fs backing)

**Tier 2 prereqs (all ✅ as of 2026-05-19 — M26 "first compile" smoke test unblocked):**
- ✅ SYS_FSYNC (crash-safe writes for cargo) — `9129f19`
- ✅ SYS_RENAME (atomic rename) — `9129f19`
- ✅ SYS_TRUNCATE — `9129f19`
- ✅ FWRITE that handles >256 bytes (heap-Allocated ObjectContent) — `b51e22a`
- ✅ Enriched SYS_STATX (type + mtime + tier + size + suid) — `9129f19`

**Tier 3 prereqs (all ✅ as of 2026-05-19 — parallel/threaded rustc unblocked):**
- ✅ SYS_THREAD_SPAWN / SYS_THREAD_JOIN (kernel-mode `178c96d` + Ring-3 same-AS `5d6e241`)
- ✅ Mutex/Condvar lowering target: SYS_FUTEX_WAIT / SYS_FUTEX_WAKE (`178c96d`)
- ✅ SYS_WAITNB (WNOHANG non-blocking child wait, `178c96d`)
- ✅ SCHEDULER_TICK_HZ const for std::thread::sleep shim (`f6a9824`)
- *Note:* thread-local storage (per-thread `static`s) isn't done — std-shim
  routes TLS through the existing per-process env block for now; revisit if
  upstream std actually requires real TLS for parallel codegen.

**Done when (M25 itself):**
- [✅] `std::fs` routes to `SYS_FS_*` (M4) — `fs::File`/`OpenOptions` + `io::{Read,Write}`, DEMO 31 round-trip
- [✅] `std::process::Command` calls `SYS_SPAWN` / `SYS_WAIT` — `92ccbb5`,
      DEMO 32. Unblocked a broad bug: `AddressSpace::new` now copies the PML4
      from `boot_cr3()` not the live CR3, so a Ring-3 parent spawning a child
      no longer shares (and corrupts) its own page tables. `SYS_WAIT` joins
      the child's scheduler slot (Ring-3 children never hit PROCESS_TABLE
      Zombie). `spawn-demo` validates exit codes 0 and 0x2700 propagate.
- [🔨] `std::thread` over a preemptive scheduler with `std::sync::{Mutex, Condvar, RwLock}` — `thread::spawn`+`JoinHandle<T>`, `Mutex`, `Once` done (DEMO 31); `Condvar`/`RwLock` not yet
- [✅] `std::net::{TcpStream}` over kernel-core::net — `b332cf0`, `f548688`,
      `8c2cb21`, `a708bc8`. Kernel syscalls SYS_DNS_RESOLVE + SYS_TCP_{CONNECT,
      READ,WRITE,CLOSE,STATE} (100-105, one TCP socket at a time,
      **non-blocking**: one net::poll + one try, NET_WOULDBLOCK sentinel) +
      `semos-std::net` (Ipv4Addr, resolve, TcpStream impl io::Read/Write that
      drives the wait in user space). **DEMO 36: net-demo resolves
      example.com, opens a TcpStream, sends an HTTP GET and reads the response
      end-to-end from Ring 3** (125 PASS with -netdev). Unblocked once the
      **task#40 torn-context-switch #DF was fixed (#56, `8c2cb21`)** — that
      was the real blocker, not the net path. UdpSocket not exposed (DNS is a
      one-shot resolve). Fixed en route: address-space GC in
      store_address_space; user stack decoupled from kernel TASK_STACK_SIZE.
- [🔨] `std::env`, `std::path`, `std::time` — `env` done; `path`/`time` minimal
- [✅] A "hello world" program built against this std runs on Semantic OS — hello-std/vec-demo/std-demo (DEMO 29–31)

## M26 — Cranelift backend integration `[🔨 prep done]`

Avoid the LLVM C++ port by adopting the Rust-native codegen.

**Prep already landed (2026-05-18, commit `8ed4aa7`):**
- ✅ Vendor placeholders + VENDOR_NOTEs in
  `kernel-core/vendor/cranelift/` and `vendor/rustc_codegen_cranelift/`
  pinning the versions (cranelift 0.121.0; cg_clif tied to nightly-2026-02-01).
  Sources themselves NOT YET copied — agent's sandbox blocked network +
  cargo execution; documented re-vendoring procedure in each VENDOR_NOTE.
- ✅ `docs/PHASE_14_CRANELIFT_BRIEF.md` (~450 LOC) — sub-crate
  architecture, MIR→CLIF→x86_64 pipeline, integration plan, 10 known-
  unknowns, what LLVM features we give up.
- ✅ `docs/STD_SHIM_SURFACE.md` — 65 std methods catalogued with their
  syscall dependencies, drove the Tier-1/2/3 prereq list above.

**Done when:**
- [ ] Cranelift sources fully vendored (one agent session in a less-restricted
      sandbox, OR manual rsync from the cargo registry cache)
- [ ] `rustc_codegen_cranelift` similarly vendored / patched
- [ ] Smoke test: cranelift compiles a small Rust program to
      x86_64 machine code that runs on Semantic OS

## M27 — First rustc build on Semantic OS `[  ]`

**Done when:**
- [ ] Cargo (built against M25's std) drives a rustc invocation
      that produces a working binary
- [ ] The "hello world" test from M25 compiles and runs end-to-end
      on Semantic OS without the cross-build server

## M28 — Self-bootstrap `[  ]`

The capstone moment for Phase 14.

**Done when:**
- [ ] `cargo build --release` of Semantic OS, run *on* Semantic OS,
      produces a working kernel image
- [ ] That image, when booted, can rebuild itself the same way

---

# Future scope — not yet specced, do not start

These are real eventual requirements but not on any current critical
path. List exists so the work isn't forgotten, not so anyone picks
one up speculatively.

- **Video editing** — software encoders, audio mixing, timeline UI,
  effects pipeline, real-time preview, Iris Xe QuickSync hw encode/decode,
  ICC color management. Triples the AV stack work over playback (M17).
  Depends on M14, M15, M17 solid first.
- **Geometry kernel port** — C++ ArchEngine_kernel → native Rust for
  LegibleStudios CAD app. Brief lives at `F:\Software\LegibleStudios\HANDOFF_2026-05-15_VULKAN_KERNEL_PORT.md`.
- **LegibleStudios full port** — Python+PyQt6 → native Rust for the
  whole app. Brief at `F:\Software\LegibleStudios\HANDOFF_2026-05-15_LEGIBLE_STUDIO_RUST_PORT.md`.
- **MarlOS port** — Tauri typesetter → native Rust on this kernel.
- **Marée, Brise, Claw Pen** — utility apps, design pending.

---

# Out of scope, settled — do not re-propose

These were considered and explicitly rejected. Don't reopen without
new information.

- **Linux ABI compatibility layer.** Rejected on security grounds:
  importing Linux syscalls re-imports the Linux attack surface,
  contradicting the kernel's ring-0 LLM-mediation thesis. Native
  Rust everywhere. C++ apps (including the LegibleStudios Vulkan
  engine) must be Rust-ported. This was reversed once during
  scoping and re-rejected — the security argument is why it stays out.
- **AAA / commercial games.** No Vulkan for NVIDIA, no Mesa-equivalent.
  Retro / native-Rust-port games only (M14 + M17 cover what's needed).
- **Complex text shaping** (Arabic, complex CJK). Latin and simple
  scripts only.

---

# Cross-cutting discipline (apply to every milestone)

Distilled from the lessons captured in the project memory file.
These aren't optional — they're how this codebase stays correct
under modification.

- **Round-trip self-tests are necessary but not sufficient for crypto
  primitives.** Always KAT against the controlling RFC's published
  bytes. See `feedback_crypto_kat_discipline.md` in memory. The
  Phase 8 Poly1305 bug hid behind perfect round-trip tests for weeks.
- **kernel-core can't run `cargo test`.** Boot-time DEMOs in
  `kernel-x86_64/src/main.rs` are the validation path. Each DEMO
  prints `PASS:`/`FAIL:` lines grepped from the QEMU serial log.
  Numbering: next free DEMO N is one past the last one in main.rs.
- **Build order:** if any user binary in `user-programs/<name>/`
  changed, `cargo build --release` there first; then
  `cargo build --release` from `kernel-x86_64/`; then
  `cargo run --release` from `x86_64-runner/` for the disk image.
- **QEMU flags that matter:** `-cpu max` (required — RDRAND probe
  aborts without it). `-rtc base=utc` (required for honest wall
  clock; default is `localtime` and adds host-TZ offset).
  `-device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0` (for M3).
  `-drive format=raw,file=vdisk.img,if=virtio` (for VirtIO block;
  create with `qemu-img create -f raw vdisk.img 16M` once).
- **Agent isolation cuts merge headaches but can hide interaction
  bugs.** If launching an agent on a worktree, either rebase the
  worktree onto current `main` before they start, or budget time
  for combined-image revalidation. (Lesson from the USB agent at
  M3 — its standalone tests passed but the merged image crashed.)
- **Don't `cargo test` kernel-core** — 96 pre-existing test errors
  from `#[cfg(test)]` blocks across `users.rs`, crypto modules, etc.
  Tests that need running get a public function + a boot DEMO.

---

# Maintaining this file

When something ships:
1. Flip its `[ ]` to `[✅]` and add the commit SHA in the description.
2. If the milestone uncovered new sub-work, add it to the next
   milestone's checklist or create a new milestone for it.
3. Update the project memory file (one-line entry in `MEMORY.md`
   index + paragraph or two in `project_semantic_os_kernel.md`).
4. If you learned something that applies to all future work, write
   a `feedback_*.md` memory file for it — that's where the
   cross-cutting discipline above came from.

When you reopen something that was `[✅]`:
1. Mark it `[🔨]` again with a one-line "why reopened".
2. Don't delete the prior entry — append.
