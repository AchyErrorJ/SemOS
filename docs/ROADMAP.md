# Semantic OS — Roadmap

**No dates here.** This document tracks **what must happen** and **what unblocks what**. Time is a side effect of doing things in the right order, not a thing to plan in.

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

## M3 — USB stack (xHCI + HID keyboard) `[⏸️]`

Driver code landed (`1301bcb`), init gated behind a comment because it
triggers a delayed kernel #GP fault during DEMO 8.

**Done when:**
- [ ] `usb::init_and_enumerate()` runs without corrupting kernel/user memory
- [ ] DEMO 18 passes all 5 sub-checks against `qemu-xhci -device usb-kbd`
- [ ] All DEMOs 0-19 still pass in the same boot
- [ ] CSZ=1 (Intel 64-byte contexts) branch noted as not-validated-in-QEMU,
      so the metal test on ThinkPad P1 surfaces it as a known gap

**Blocker:** `usb::init_and_enumerate()` corrupts memory; surfaces as
#GP at non-canonical RIP `0x500010000044800` during the next
user-program execution cycle. Bisect plan in task #36.

## M4 — FS Stage 3: `SYS_FS_*` syscalls `[  ]`

Expose the path namespace to user space.

**Done when:**
- [ ] `SYS_FS_OPEN(path, flags) → fd` allocates a per-process FD
      against the path-namespace SUID
- [ ] `SYS_FS_READ(fd, buf, n) → n` and `SYS_FS_WRITE(fd, buf, n) → n`
      read/write the underlying SemanticObject
- [ ] `SYS_FS_CREATE(path, tier) → fd`, `SYS_FS_MKDIR(path)`,
      `SYS_FS_UNLINK(path)`, `SYS_FS_READDIR(fd, buf)`, `SYS_FS_STAT(path)`
- [ ] FD table extended (today `fs::ramfs::FdTable` is read-only embedded
      files) to hold path-namespace handles
- [ ] Security check: callers can only `open` objects whose `tier` the
      calling task's `max_tier` covers; `readdir` filters out
      inaccessible children
- [ ] A user-space program (`fs-demo` or extension of `sem-demo`)
      uses the syscalls end-to-end and prints what it sees
- [ ] DEMO 20 covers it from Ring 0; the user-space test covers it
      from Ring 3

## M5 — FS Stage 2: snapshot persistence for the namespace `[  ]`

Survive reboot.

**Done when:**
- [ ] Each `Namespace` mutation marks the affected SUIDs dirty
- [ ] A kernel-side flush walks the dirty set and writes through
      `storage::snapshot` to VirtIO block
- [ ] Boot path: if a snapshot exists, replay it into `global_registry`
      before `Namespace::init` runs; init becomes idempotent (already is)
- [ ] DEMO 21: create files → flush → reboot (in the same QEMU run via
      a soft re-init path, or two QEMU runs against the same vdisk
      image) → read same content back
- [ ] `mint_suid()` switched from atomic counter + boot-tick scramble
      to RDRAND so persisted SUIDs don't collide across boots
- [ ] SemanticObject `created_at`/`modified_at` populated from
      `platform::wall_clock()` (M2)
- [ ] Snapshot size limit (64 KiB today) documented as a "namespace
      metadata only" cap; large-object content goes into a separate
      per-object stream when that becomes necessary

## M6 — Framebuffer drawing API `[  ]`

Promote raw `set_pixel` to a real drawing surface.

**Done when:**
- [ ] `fb_fill_rect(x, y, w, h, color)`, `fb_blit(src, x, y, w, h)`,
      `fb_scroll(dx, dy)`, `fb_present()` as kernel-side functions
- [ ] Color format documented (BGR vs RGB; bootloader-0.11 framebuffers
      vary by hardware)
- [ ] Shared-memory framebuffer region exposed to user space (mapped
      read/write into the process's address space for direct draw)
- [ ] Damage-rect / present model so apps don't tear writes
- [ ] DEMO 22 draws a checkerboard + a moving rect proving each
      primitive

## M7 — Font rasterization (fontdue port) `[  ]`

Render text in real fonts, not just the 8x16 bitmap console.

**Done when:**
- [ ] `fontdue` (MIT, ~7k LOC) vendored under `kernel-core/vendor/`
      with no_std + no_alloc cuts
- [ ] A TTF/OTF font file embedded in the kernel ramfs (start with one
      open-source font, e.g. Inter or Source Sans 3)
- [ ] `fb_draw_text(x, y, str, size, color)` rasterizes glyphs via
      fontdue and blits them through M6's API
- [ ] DEMO 23 renders text at 3 different sizes; visible in the QEMU
      framebuffer

## M8 — 2D vector rasterizer (tiny-skia port) `[  ]`

Anti-aliased lines/curves/fills for the design apps.

**Done when:**
- [ ] `tiny-skia` (Apache-2.0, ~25k LOC) vendored under
      `kernel-core/vendor/` with no_std + no_alloc cuts
- [ ] `fb_stroke_path` / `fb_fill_path` over M6
- [ ] DEMO 24 draws a few anti-aliased Bézier curves; visible in QEMU

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

## M12 — DNS resolver `[  ]`

Replace the hardcoded Anthropic IP in DEMO 16.

**Done when:**
- [ ] UDP socket on top of smoltcp
- [ ] DNS request builder (A record, ID + flags + question)
- [ ] Response parser
- [ ] `dns::resolve(host: &str) -> Option<Ipv4Address>` with a small
      cache
- [ ] DEMO 16 stops hardcoding the IP and calls `dns::resolve` first

## M13 — Chunked-transfer-encoding parser `[  ]`

DEMO 16's body preview today shows `8d` (the chunk length header).
Once apps actually consume the response we need real chunked parsing.

**Done when:**
- [ ] `http::ChunkedBody` decoder that produces the unchunked bytes
- [ ] NetworkLlmProvider uses it for Anthropic responses
- [ ] DEMO 16's body preview shows actual JSON, not the chunk header

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

## M19 — TTY layer `[  ]`

The framebuffer console is write-only today. A shell needs bidirectional.

**Done when:**
- [ ] Buffered stdin sourced from the USB keyboard driver (M3) with
      line-editing primitives (Backspace, arrow keys)
- [ ] ANSI escape sequence handler in the framebuffer output path
      (cursor positioning, color, screen clear, scroll region) — the
      minimum subset any TUI program assumes
- [ ] Scrollback buffer (~100 lines) so output isn't lost on scroll
- [ ] Per-process stdin/stdout/stderr (today there's just one global
      println!) so multiple programs can read/write independently
- [ ] DEMO 32 echoes typed characters back through ANSI-coloured output

## M20 — Native shell (`sem-sh` or similar) `[  ]`

Rust shell — no bash compatibility, just what we need.

**Done when:**
- [ ] Line editor on top of M19 with history (Up/Down recall)
- [ ] Command parser: argv splitting, quoting, env-var substitution
- [ ] Builtins: `cd`, `ls`, `cat`, `echo`, `pwd`, `exit`, `env`, `which`
- [ ] Exec native ELF programs (extension of today's `user-programs/`
      mechanism — currently only kernel-launched, needs runtime spawn)
- [ ] Pipes (`|`) and file redirection (`>`, `<`)
- [ ] Job control deferred to a follow-up; not in v1
- [ ] DEMO 33 launches `sem-sh` and runs a script that creates a
      file, cats it back, and pipes through another program

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

## M23 — Build pipeline (Road A: cross-build over the network) `[  ]`

Compilation happens on a build server reachable over Wi-Fi. The
agent on Semantic OS pushes source changes, kicks off the build,
pulls back the new kernel image. This is the *achievable* path —
Road B (port rustc + LLVM) is multi-year scope and deliberately
parked.

**Done when:**
- [ ] Network protocol for "push these files, build, return image"
      (could be: git push → CI webhook → image download; or a
      simpler custom HTTP service; pick whichever the user runs
      on the build server)
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

- **Port rustc + LLVM (Road B).** Reopened only if Road A proves
  insufficient. Estimated multi-year; almost no hobby kernel
  attempts this. Even mature Rust-native kernels like Redox
  haven't done it.
- **JS runtime port (for running upstream Claude Code as-is).**
  Easier than rustc but still enormous; the native Rust agent
  (M22) bypasses the need.
- **Tree-sitter / LSP** — nice to have but not required for the
  "make a kernel change with Claude's help" loop.

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
