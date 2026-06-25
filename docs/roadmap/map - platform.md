# Roadmap — Platform (ARM, info access, agent infra, media, utilities)

> Part of the [Master Roadmap](../MASTER_ROADMAP.md). Sibling themes:
> [networking](map%20-%20networking.md) · [self-extension](map%20-%20self-extension.md) ·
> [phone](map%20-%20phone.md) · [gpu](map%20-%20gpu.md). Historical log: [ROADMAP.md](../ROADMAP.md).

Everything that makes the self-hosted OS a daily driver: the ARM port, web/info
access, Claude-Code-parity agent tooling, the native Swift bridge, media, and
system utilities. From-scratch holds — the browser is yours, the package manager
is yours. Honest timelines: the web browser is **2-3 years to "usable for reading
docs,"** not six months; calendar estimates here are optimistic.

---

## ARM Port (kernel-aarch64) — ACTIVE offline

**Goal:** SemOS boots on **Apple Silicon (M2)** via dual-boot alongside macOS.
A *return* to ARM, not a new port — the repo was aarch64 first.

> **STATUS:** `kernel-aarch64` is a standalone crate that **boots in QEMU `virt`**,
> brings up UART (PL011) → exception vectors → MMU → GICv2 → timer → preemptive
> scheduler, and runs the **same `kernel-core`** (sha256 KAT passes), plus a
> page-table/frame-allocator self-test. "Two backends, one portable core." Fully
> **QEMU-testable offline:** `cd kernel-aarch64 && cargo run --release`. Next gate:
> Ring-3 user spawn + SVC syscalls.

**Why M2:** M1/M2 have mature Asahi Linux support (boot chain, drivers); M4 boot
chain (SPTM/GL2) is stalled. Dual-boot keeps macOS as the build host; Asahi's
documented m1n1 → kernel path is followed. Real metal: DART (IOMMU), AIC, ANS,
DCP. **What carries over unchanged:** all of kernel-core, semos-std, user
programs, the agent loop. **What's new:** boot/interrupts/paging/drivers + the
syscall wrapper (`svc #0` vs `syscall`).

### M34 — ARM64 HAL `[🔨 — QEMU virt boot + MMU + GIC + timer + scheduler RUNNING]`
EL2/EL1 transition, exception vectors, ARMv8 4-level paging, GICv2/v3, Generic
Timer, MPIDR/PSCI bring-up, framebuffer. DEMO: boots QEMU `virt`.

### M35 — Device drivers for ARM platforms `[  ]`
DeviceTree parser, PL011 UART, VirtIO over MMIO, GIC v2/v3, PSCI power mgmt.

### M36 — Apple Silicon specifics (M2) `[  ]`
m1n1 chainload, DART (Apple IOMMU), AIC (interrupt controller), Apple NVMe (ANS),
Apple framebuffer (simplefb/DCP). Leverages Asahi's published register docs
("read the docs, write the driver," not reverse engineering).

### M37 — Cross-compilation infrastructure `[  ]`
`cargo build --target aarch64-unknown-none` from x86_64 and vice-versa;
`semos-rustc` multi-backend (cg_clif x86_64 + aarch64).

**exFAT shared partition** (dual-boot file sharing M2): ~2-3 weeks Rust — FAT
traversal, dir entries, cluster alloc, no journaling. A follow-up to FS M4.

---

## Information Access (Web Browser + Search)

A compiler without documentation is a car without roads — this makes self-hosting
usable. The browser is a Ring-3 app, develops in parallel with M27.

### M29 — General HTTP client `[  ]`
Extend the POST-only Anthropic stack to full HTTP/1.1: GET/POST/HEAD/PUT/DELETE,
cookie jar (FS-persistent), redirects, ETag/Last-Modified caching. DEMO 75.

### M30 — HTML parser `[  ]`
Streaming, no DOM engine: tokenizer, minimal tree, `html_to_text` (lynx -dump),
`extract_links`, structural awareness. Vendored `html5gum` (~5K LOC, no_std-ish)
or a ~2K-LOC from-scratch extractor. DEMO 76.

### M31 — Search engine integration `[  ]`
DuckDuckGo HTML API (no JS/key) → `search(q) -> Vec<Result>`; `search` shell
builtin. DEMO 77.

### M32 — Text-mode browser (optional v1) `[  ]`
lynx/w3m-style TUI: render HTML as formatted text, link nav, history. DEMO 78.
(V2 CSS/JS/DOM is a multi-year project — deferred.)

### M33 — Agent tool `web_search` `[  ]`
Wire search into the agent loop; read top-N, extract text, citation tracking. DEMO 79.

---

## Advanced Agent Infrastructure (Claude Code parity)

What makes the agent a daily driver, not a demo. Ring-3 improvements, parallel
with M27.

### M38 — Context window management `[  ]`
`read_file` with line ranges, `grep`/`find`/`list_dir` tools, tree-sitter AST
index for symbol lookup. DEMO 84.

### M39 — Multi-file edit tool `[  ]`
`apply_diff`, precise `edit_file`, `create_file`/`delete_file`. DEMO 85.

### M40 — Cargo integration `[  ]`
`cargo check`/`test`/`build` tools + rustc-JSON error parser. DEMO 86.

### M41 — Persistent agent memory `[  ]`
Conversation history to FS, `conversation_id` across reboots, resume-on-boot. DEMO 87.

### M42 — Security tier for agent tools `[  ]`
Tool-specific tier elevation (write to `/kernel/` needs tier 3), user confirmation
for destructive ops, append-only audit log. DEMO 88. (Ties to the vouch mechanism
in [self-extension.md](map%20-%20self-extension.md).)

---

## Native Swift Bridge Rewrite (when Mac arrives — Phase 19)

Replace the Expo bridge prototype ([phone.md](map%20-%20phone.md)) with production Swift.
The pairing, Layer-4 RPC, and capability **protocols stay** — only the
implementation changes (TypeScript → Swift, react-native → Network framework,
expo-secure-store → Keychain/CryptoKit).

- **M68** Swift app skeleton (Xcode, SwiftUI, TestFlight, pairing in Swift)
- **M69** Layer-4 bridge in Swift (NWConnection, background entitlement, perf vs Expo)
- **M70** capability migration (Keychain/CryptoKit, AVFoundation — parity with M62-67)
- **M71** distribution (Apple Developer Program, TestFlight → App Store)

Start when a used M2 Mac is acquired (after M35 lands on QEMU).

---

## Media + Entertainment (deferred but planned)

Quality-of-life. Depends on iGPU/dGPU ([gpu.md](map%20-%20gpu.md)).
- **M45** video playback — software `dav1d`/`openh264` + QuickSync HW decode + HDA sync. DEMO 91.
- **M46** retro game engine — tiny-skia sprites + HID gamepad (done) + HDA audio + one game. DEMO 92.
- **M47** music player — `symphonia`/`minimp3`, playlists, background playback. DEMO 93.

---

## System Utilities

### M48 — `top` process monitor `[  ]`
First diagnostic any dev reaches for; validates scheduler accounting.
- [ ] kernel: `SYS_PROC_LIST` (PID/PPID/state/name) + `SYS_PROC_STAT` (CPU ticks, RSS, prio, threads) + `SYS_PROC_KILL`
- [ ] `/bin/top` TUI (~500 LOC `semos-std`): sortable by CPU/mem/PID, interactive kill/renice, minimal `ps` mode
- [ ] DEMO 94: run `/bin/top`, see kernel tasks + shell, sort, kill a test process

---

## The real end state

User sits at an M2 Mac running SemOS: boots in seconds → `sem-sh` → `agent`
searches the web (M33), reads docs, multi-file-edits (M39), runs `cargo check`
(M40) → `edit main.rs` native editor → `cargo build` (on-device rustc) → `reboot`
into the new kernel → `semos install ripgrep` (M43) → watch a video while the agent
compiles → reboot to macOS and back, seamless dual-boot. The current frontier gets
to "agent searches + reads docs"; this theme carries the rest.
