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
| M20 stage B 2026-05-23 | **sem-sh fs builtins + $VAR** (`b81251d`, DEMO 45): cat/ls/which/env builtins + `$VAR` expansion (inherited env). Suite **148 PASS**. Stage C (pipes `|` + redirection `>`/`<`) remains. |

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

## M9 — NVMe driver `[  ]`

Block storage on real ThinkPad P1 hardware. VirtIO block (Phase 6)
covers QEMU; real hardware has no SATA, no VirtIO.

**Done when:**
- [ ] PCI discovery of NVMe controller (class 0x010802)
- [ ] Submission/completion queue pair setup
- [ ] Identify Controller + Identify Namespace
- [ ] Read/Write commands via I/O SQ/CQ
- [ ] Wired as a `BlockDevice` named `nvme0` so `storage::snapshot`
      and (eventually) M5 just work on top of it
- [ ] DEMO 25 reads/writes a sector via the BlockDevice trait

---

# Phase 10 — Bare-metal P1 readiness + Wi-Fi

Goal: the kernel boots on the user's ThinkPad P1 Gen 6 (the target
hardware), runs the same DEMOs, and can reach api.anthropic.com over
Wi-Fi (currently TLS works via QEMU SLIRP forwarding).

## M10 — Pre-flight checklist for bare-metal boot `[  ]`

Find and fix everything that "passes in QEMU, fails on metal" before
the first real-hardware session.

**Done when:**
- [ ] Serial-over-USB plan documented (P1 has no native serial port)
- [ ] Framebuffer-only fallback boot path tested (no serial capture)
- [ ] xHCI CSZ=1 branch (Intel 64-byte contexts) re-enabled and
      audited in code review
- [ ] RTC firmware-century-byte assumption verified against real BIOS
- [ ] VT-d disabled in BIOS OR identity-IOMMU implemented
- [ ] "Kernel didn't crash" watchdog: framebuffer last-line banner
      written by the idle loop, so a stalled kernel is visible

## M11 — iwlwifi (AX211) driver `[  ]`

802.11 over Intel AX211.

**Done when:**
- [ ] Intel firmware blobs (`iwlwifi-so-a0-gf-a0-N.ucode` + `.pnvm`)
      embedded in ramfs
- [ ] Firmware upload + secboot succeeds; ALIVE event received
- [ ] PHY init: NVM + PNVM + regulatory + channel calibration
- [ ] 802.11 MAC: management frame builder (Probe/Auth/Assoc Request,
      EAPOL frames)
- [ ] WPA2 four-way handshake in software, CCMP encrypt/decrypt
      offloaded to firmware after keys installed
- [ ] Bring up a CDC-ECM USB Ethernet path FIRST as a fallback so
      the TLS stack can be exercised on metal before Wi-Fi works
- [ ] DEMO 26 associates to a hardcoded SSID, gets DHCP, repeats
      DEMO 16's handshake to api.anthropic.com over real Wi-Fi

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

## M15 — HD Audio driver `[  ]`

Prerequisite for games and video playback.

**Done when:**
- [ ] Intel HDA controller bring-up
- [ ] Codec enumeration
- [ ] PCM output stream (44.1 / 48 kHz, 16-bit stereo minimum)
- [ ] DEMO 28 plays a 440 Hz sine wave for 1 second

## M16 — USB HID gamepad `[  ]`

Extension of M3 (USB keyboard) once that's working.

**Done when:**
- [ ] HID report descriptor parser (real one, not boot protocol)
- [ ] Gamepad axis + button report parsing
- [ ] DEMO 29 reads and prints gamepad input

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

## M20 — Native shell (`sem-sh` or similar) `[🔨 stage A+B landed; pipes/redir left]`

Rust shell — no bash compatibility, just what we need. `user-programs/sem-sh`,
built on `semos-std`. **Stage A (`5398720`) + B (`b81251d`), DEMO 45, 148 PASS.**

**Done when:**
- [✅] Line editor on top of M19 with history — provided by the M19 line
      discipline (arrows + Up/Down history); the shell reads cooked lines via
      `SYS_READ(0)`.
- [✅] Command parser: argv splitting + quoting (`"`/`'`) + `;`/newline
      separation + `$VAR` expansion (env, outside single quotes).
- [✅] Builtins: `echo`/`pwd`/`cd`/`exit`/`true`/`false`/`cat`/`ls`/`which`/`env`.
      (`env` prints named vars only — no enumerate syscall yet.)
- [✅] Exec native ELF programs — non-builtins run via `process::Command`
      (`name` → `/bin/name`), blocking for exit status.
- [ ] Pipes (`|`) and file redirection (`>`, `<`) — **stage C**. `SYS_PIPE`/
      `DUP2` shim wrappers (exposed in arch.rs) + per-process FD inheritance
      (M19) are the substrate; the shell sets up the pipe/redirect on its own
      FDs around each `Command` spawn (children inherit).
- [ ] Job control deferred to a follow-up; not in v1
- [ ] DEMO 46 (stage C): a script that creates a file, cats it back, and pipes
      through another program. (DEMO 45 is the stage A+B REPL/builtins test.)

**Gotcha that cost time in stage A:** a new user crate builds as PIE (ET_DYN)
unless it copies `build.rs` + `link.ld` + `.cargo/config` (non-PIE EXEC at
0x400000) — the kernel applies no relocations, so `println` crashes while raw
syscalls work. See `feedback_new_user_program_nonpie.md`.

## M21 — Native editor `[  ]`

Edit source files in-place. Not vim-compatible, just usable.

**Done when:**
- [ ] Modal or modeless (decide based on M22's agent loop's needs —
      the agent will be the heaviest user)
- [ ] Open/save against FS Stage 3 syscalls
- [ ] Basic syntax highlighting for Rust (just keywords + strings +
      comments; full tree-sitter is later)
- [ ] Search + replace
- [ ] Multi-file open (tabs or buffers)
- [ ] DEMO 34 opens a file, edits a line, saves, re-reads to verify

## M22 — Claude agent client (native Rust port) `[  ]`

The reason for all of the above. A TUI agent like Claude Code but
written for this kernel, talking to the Anthropic API over the
TLS stack from Phase 8 + Wi-Fi from Phase 10.

**Done when:**
- [ ] TUI render loop on M19/M20 (split panes, status line, scrollback)
- [ ] Agent message loop: read user input, send to API, parse
      response, render output, repeat
- [ ] Tool use: at minimum `read_file`, `write_file`, `bash`
      (executes via M20), `grep`, `glob` — the smallest set that
      lets Claude make real edits to this codebase
- [ ] Multi-turn conversation with context management (truncate
      old turns when nearing token limit)
- [ ] Loads API key from a file under `/etc/anthropic-api-key`
      (M5 persistence makes this possible)
- [ ] DEMO 35 boots the agent, asks Claude to read README.md and
      summarize it; agent calls `read_file`, returns the summary

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
