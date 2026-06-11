# Semantic OS — Roadmap Expansion (Draft)

> **SUPERSEDED for Phases 15–18 (2026-06-10):** the active forward plan is
> `docs/semos-post-phase14-roadmap.md` (revised numbering: 15 tether →
> 16 pairing+Expo → 17 Layer-4 bridge → 18 capabilities → 19 Swift →
> 20 WiFi). Phase 15 is substantially DONE as of 2026-06-10 — via ipheth
> over a new standalone EHCI driver, not CDC-ECM-over-xHCI as drafted here
> (the intro paragraph below about the USB-3/xHCI companion path turned out
> wrong on the W540; see the status note in the revised doc).
> **This file remains the only home of the later phases** (web browser, ARM
> port, advanced agent infra, package manager, media, system utilities —
> its Phases 19+); fold those into the revised doc when they get scheduled.

**Proposed additions** to the current roadmap. These are the capabilities that make the self-hosted OS actually *useful* for daily development, not just theoretically complete.

**Sequencing (2026-06-04 update):** Phases 15-18 below — tethering, QR pairing, companion app, WiFi — come first because the OS needs to be **online on real silicon** before web/agent/ARM/package work has anywhere to land. Phases 19+ preserve the original 2026-06-02 expansion proposal (web browser, ARM port, advanced agent infra, package manager, media, system utilities).

The W540 has only USB-3 sockets, but **USB tethering works fine on USB-3**: USB-3 ports are backward-compatible with USB-2 devices via the companion USB-2 PHY (the exact path F12's xHCI work surfaced and validated). Both iPhone (CDC-ECM) and Android (RNDIS/CDC-NCM) tether at USB-2 high-speed regardless of socket version.

## Security thesis disciplines (apply to every milestone)

Per `docs/semos-security-thesis.md` (June 2026, "Path A: From-Scratch"). Every milestone below — every M-number from M50 onward — answers these four questions in its checklist before work starts:

1. **Does this require a new syscall?** If yes, which one(s)?
2. **What's the smallest possible shape for the new syscall(s)?**
3. **What capability check guards it?** (Ring-0 LLM agent is *enabled* by smallness, not by safety theater — every privileged operation is one a maintainer can hold in their head.)
4. **What's the blast radius if the agent misuses it?**

Other disciplines that apply across this whole roadmap:

- **From-scratch holds.** No POSIX libc shim. No vendored Linux software. No "we'll port ffmpeg eventually." Every program against the SemOS syscall surface. The web browser (Phase 19) is yours. The package manager (Phase 22) is yours. Cost: dramatically slower progress. Benefit: architectural coherence.
- **Vendoring is intentional.** When external Rust crates enter the privileged build, they're vendored + patched + audited (the `ena 0.14.4` work in M27 is the model — `vendor-externals/ena` is the pattern). Live crates.io build deps in the privileged path are out.
- **Surface inventory exists.** A `docs/KERNEL_SURFACE.md` document is in scope as a first session-of-work after F13. Every syscall: name, parameters, capability requirements, intended use, audit history. The artifact behind the auditability claim.
- **Capability scoping for agent sessions** is the highest-value security feature to design once the rustc work is closed. Per-session capability tokens enforced inside the kernel; ring-0 placement makes this *easier* to build, not harder.
- **Honest timelines.** From-scratch costs are real: M30 (HTML parser) is a real project; the web browser is **2-3 years to "usable for reading documentation,"** not six months. Phase 19+ calendar estimates below have been written under that pressure — anywhere a phase says "X weeks" or "Y months," it's an *optimistic* number that requires nothing else competing for attention.

---

# Phase 15 — USB Tethering (online on real silicon)

**Goal:** W540 boots Semantic OS, phone plugs in over USB-C → USB-A, kernel sees the phone as a network device, internet works, agent loop calls Anthropic API over a real wire.

**Why this first:** Smallest path to *the OS is real and online*. Two-three weeks of focused work versus three-six months for WiFi. Result is the moment Semantic OS stops being a QEMU demo and becomes a thing you carry around.

**Depends on:** M27 closed (rustc compiles against SemOS), xHCI bringup from F12 (already done), existing TCP/IP/TLS stack (already done).

## M50 — USB CDC-ECM driver `[  ]`

Recognizes a USB device that presents itself as a CDC-ECM (Ethernet over USB) endpoint, enumerates its endpoints, and exposes it to the kernel as a network interface.

**Why CDC-ECM first:** Standard iPhone tethering protocol. iOS exposes a CDC-ECM interface. Less complex than RNDIS — about 500-1000 LOC of Rust.

**Done when:**
- [ ] xHCI recognizes a USB-class-2 (Communications) device on insertion
- [ ] CDC-ECM control endpoint parsed (interface descriptors, MAC address)
- [ ] CDC-ECM data endpoints bound (bulk-in for receive, bulk-out for transmit)
- [ ] `net_interface_register()` called with the new interface
- [ ] DEMO 81: insert tethered iPhone, kernel logs "CDC-ECM device detected, MAC xx:xx:xx:xx:xx:xx, interface usb0 up"

## M51 — RNDIS driver `[  ]`

Same as CDC-ECM but for Microsoft's RNDIS protocol. Android phones default to RNDIS when tethering.

**Why both:** iPhone uses CDC-ECM, Android uses RNDIS. If you want both ecosystems supported, you need both. CDC-ECM is simpler; RNDIS is more layered (has a management plane). About 1500-2000 LOC.

**Done when:**
- [ ] RNDIS detection via USB class/subclass/protocol triple (0xE0/0x01/0x03)
- [ ] RNDIS INIT message exchange completes
- [ ] OID queries handled (link speed, MAC address, link state)
- [ ] Packet framing and de-framing on bulk endpoints
- [ ] DEMO 82: insert tethered Android phone, kernel logs RNDIS init success and interface up

## M52 — DHCP client `[  ]`

When a network interface comes up, sends DHCPDISCOVER, parses DHCPOFFER, sends DHCPREQUEST, applies the leased IP/gateway/DNS configuration.

**Why we need it:** Phone tethering hands out IPs via DHCP. Without a DHCP client, the kernel has a network interface but no IP address.

**Implementation note:** DHCP is a small UDP protocol (~200-300 LOC). RFC 2131 is the spec. v4 client side only; DHCPv6 deferred.

**Done when:**
- [ ] DHCPDISCOVER broadcast on interface bring-up
- [ ] DHCPOFFER parsed (yiaddr, gateway, DNS, lease time)
- [ ] DHCPREQUEST + DHCPACK round trip completes
- [ ] IP/gateway/DNS applied to kernel network state
- [ ] Lease renewal timer (T1 at 50% of lease)
- [ ] DEMO 83: kernel boots, USB-tether attached, gets IP from phone, `ping` works

## M53 — Real-world TLS validation `[  ]`

The TLS stack has been talking to QEMU-emulated networks. Real Anthropic servers exercise certificate paths, TLS extensions, and edge cases QEMU never sees. Hardening pass, not a rewrite.

**Done when:**
- [ ] Real Anthropic API certificate chain validates on bare metal
- [ ] SNI extension correctly sent for `api.anthropic.com`
- [ ] TLS 1.3 handshake completes against real server
- [ ] HTTP/2 negotiation works (or falls back to HTTP/1.1 cleanly)
- [ ] DEMO 84: full agent loop turn on bare metal — boot, type prompt, get response from real Claude, no QEMU

## M54 — The first usable session `[  ]`

Bring the agent loop, the shell, the editor, the TLS stack, and USB tethering together. A user sits at the W540 and does something useful.

**Done when:**
- [ ] Boot Semantic OS on W540
- [ ] Plug in phone for tether
- [ ] Drop into sem-sh
- [ ] Type `agent`, ask "what is the time in Tokyo right now?"
- [ ] Agent responds correctly
- [ ] DEMO 85: video of full session, ~5 minutes, no narration needed

**Phase 15 calendar estimate:** 3-4 weeks of focused work.

---

# Phase 16 — QR-Code Pairing (the trust bootstrap)

**Goal:** Pair the W540 (or any Semantic OS device) with a specific phone via QR code, establishing a TLS-protected channel for everything between them. Both sides know they're talking to each other and nobody else.

**Why this second:** USB tethering gets bytes moving. QR pairing makes the phone an *identified* peer rather than a generic network device. This is the foundation for Phase 17 (companion app capabilities).

**Depends on:** Phase 15 (network on bare metal), camera access (deferred — see implementation note in M55).

## M55 — Pairing protocol design `[  ]`

Define the on-wire format for the initial pairing handshake. Both sides generate keypairs, exchange public keys, establish a shared session key, all without trusting any prior state.

**The QR code contains:**
- Companion app's public key
- Network address (IP + port) where companion app is listening
- Pairing session nonce
- Protocol version

**Implementation note (no camera yet):** The companion app displays the QR code on the phone screen. The W540 *reads* the QR code from text input (typed by the user, or pasted from a phone-relay buffer) for v1, since the W540 doesn't have a working camera driver yet. Future devices (or W540 after a UVC webcam driver lands) can scan optically.

**Done when:**
- [ ] Protocol spec document committed (`docs/pairing-v1.md`)
- [ ] Wire format defined (Cap'n Proto or Protocol Buffers schema)
- [ ] Threat model: replay attacks, MITM during pairing, downgrade attacks all addressed
- [ ] Test vectors: known pairing run produces known shared secret

## M56 — Pairing on the Semantic OS side `[  ]`

Parses the QR code (as typed input), opens a TLS connection to the address, completes the pairing handshake, stores the resulting paired identity persistently.

**Done when:**
- [ ] `sem-sh pair <qr-string>` command works
- [ ] Pairing completes against a test server (initially: a Python script on a laptop)
- [ ] Paired identity persists across reboots (stored in `/etc/paired-devices/`)
- [ ] `sem-sh paired list` shows current pairings
- [ ] `sem-sh unpair <id>` removes a pairing
- [ ] DEMO 86: pair with a test peer, reboot, verify pairing still works

## M57 — Companion app skeleton (iOS) `[  ]`

First version of the iOS companion app. Displays a QR code, accepts incoming pairing connections, persists paired peers in iOS Keychain.

**Tech stack:** Swift, SwiftUI, CryptoKit, Network framework.

**Done when:**
- [ ] App displays a QR code on tap of "Pair new device"
- [ ] App accepts pairing connection on local network
- [ ] Pairing handshake completes server-side
- [ ] Paired peer's public key stored in Keychain
- [ ] App shows list of currently paired devices
- [ ] DEMO 87: end-to-end pairing — W540 boots, app shows QR code, user types QR string into W540, both sides confirm "paired with Jer's iPhone"

## M58 — Companion app skeleton (Android) `[  ]`

Same as M57 but for Android. Lower priority than iOS because initial users are likely on iOS, but worth building for ecosystem reach.

**Done when:**
- [ ] Same capabilities as M57, Kotlin/Compose implementation
- [ ] Uses StrongBox/Keystore for key storage
- [ ] Pairs successfully with a Semantic OS device

**Phase 16 calendar estimate:** 4-6 weeks. Companion apps are real work; the pairing protocol design is the harder intellectual content.

---

# Phase 17 — Companion App Capabilities (phone-as-peripheral payoff)

**Goal:** Each capability the phone provides becomes a Semantic OS syscall. The kernel makes a structured request to the paired phone, the phone does the work, returns the result.

**Why this third:** This unlocks everything. Camera, GPS, biometric auth, audio, push notifications, identity — all become Semantic OS capabilities without Semantic OS having to implement them. The kernel stays small; the user gets a useful system. Design principle: **the phone holds the keys; SemOS is the I/O layer.**

**Depends on:** Phase 16 (pairing exists, channel is secure).

**Implementation note:** Each capability is one milestone. They can be implemented independently in any order based on which is needed first.

## M59 — Request/response protocol over paired channel `[  ]`

The structured message-passing layer that capabilities are built on. JSON or Cap'n Proto messages flowing over the TLS-protected paired connection.

**Done when:**
- [ ] Capability request message format defined
- [ ] Capability response message format defined
- [ ] Async dispatch: kernel can have multiple outstanding requests
- [ ] Timeout handling for unresponsive phone
- [ ] Versioning: protocol can evolve without breaking paired devices
- [ ] DEMO 88: kernel sends `request_echo` with payload, phone responds with same payload

## M60 — Crypto capability (Secure Enclave / StrongBox) `[  ]`

Semantic OS requests crypto operations from the phone's hardware-backed key store. Private keys never leave the phone.

**Done when:**
- [ ] `request_generate_keypair(label)` → phone creates keypair in Secure Enclave, returns public key
- [ ] `request_sign(label, data)` → phone signs data with named key, returns signature
- [ ] `request_decrypt(label, ciphertext)` → phone decrypts with named key
- [ ] User authentication (Face ID / Touch ID) required for use, configurable per-key
- [ ] DEMO 89: Semantic OS generates a keypair stored in iPhone Secure Enclave, signs a test message, verifies signature on-device

## M61 — Identity capability `[  ]`

The phone is the user's identity. Sign in to Anthropic, GitHub, anything that needs auth — happens via the phone, with the phone's accounts.

**Done when:**
- [ ] `request_identity()` → returns user's primary identity from phone
- [ ] OAuth flow proxied through phone (phone opens browser, completes flow, returns token)
- [ ] Token storage in phone's Keychain, not on Semantic OS disk
- [ ] DEMO 90: Semantic OS authenticates to Anthropic API using a token held in iPhone Keychain, no API key on disk

## M62 — Camera capability `[  ]`

Semantic OS asks the phone to capture a photo. Phone opens camera UI, user takes photo, photo returns to Semantic OS over paired channel.

**Done when:**
- [ ] `request_camera_capture(mode)` opens camera app on phone
- [ ] User takes photo or cancels; result returned over channel
- [ ] Photo metadata stripped of GPS by default, optional inclusion
- [ ] DEMO 91: Semantic OS requests a photo, phone captures it, photo appears in `/tmp/capture.jpg` on W540

## M63 — GPS capability `[  ]`

Semantic OS asks the phone for location. Phone returns coordinate with accuracy.

**Done when:**
- [ ] `request_location()` returns lat/lng/accuracy/timestamp
- [ ] User authorization required per session (iOS prompts the first time)
- [ ] DEMO 92: agent answers "where am I" by requesting location and reverse-geocoding

## M64 — Microphone capability `[  ]`

Semantic OS asks the phone to record audio. For voice prompts to the agent, or any audio capture use case.

**Done when:**
- [ ] `request_audio_capture(duration)` records audio on phone
- [ ] Audio returned as WAV or compressed format
- [ ] DEMO 93: speak a question into phone, agent receives transcription, responds

## M65 — Push notification capability `[  ]`

Semantic OS sends a notification to be displayed on the phone. For long-running operations, alerts, agent results when the user is away from the laptop.

**Done when:**
- [ ] `request_notification(title, body, action)` shows notification on phone
- [ ] User tap-to-action sends a callback back to Semantic OS
- [ ] DEMO 94B: long compile finishes on Semantic OS, sends "build done" notification to phone

**Phase 17 calendar estimate:** Each capability is 1-2 weeks. Crypto + identity first (M60, M61) since they unlock secure operation; camera (M62) second since it's the most demoable; the rest as needed.

---

# Phase 18 — Bare-Metal WiFi (background work)

**Goal:** Semantic OS connects to WiFi networks directly, without requiring a tethered phone. The phone remains useful for everything else; networking becomes independent.

**Why this in the background:** WiFi from scratch is 3-6 months of focused work. It shouldn't block other useful capabilities. Run it as the long-running side project that lands when it lands. Can start in parallel with Phase 15 work, though it shouldn't compete for attention.

**Depends on:** M27 (self-hosting), nothing else.

**Implementation note:** Start with one specific chip — the **Intel Wireless 7260** in the W540 (PCI device `8086:08B1`). Worry about other chips later or never. The from-scratch commitment doesn't require supporting every WiFi chip; it requires that whatever you support, you wrote.

## M66 — WiFi chip enumeration and firmware load `[  ]`

Identifies the Intel WiFi chip on PCIe, loads the firmware blob into the chip, brings the chip out of reset.

**Done when:**
- [ ] PCI device 8086:08B1 detected
- [ ] Firmware blob loaded from `/lib/firmware/` (this is an exception to the from-scratch commitment — you don't write Intel WiFi firmware, you ship Intel's blob with attribution)
- [ ] Chip reports itself as alive via mailbox interface
- [ ] DEMO 95: kernel boots, logs "Intel Wireless 7260 detected and initialized"

## M67 — 802.11 management state machine `[  ]`

Scans for networks, joins one, handles association/disassociation, manages beacon timing.

**Done when:**
- [ ] `wifi_scan()` returns list of nearby networks (SSID, BSSID, signal strength)
- [ ] `wifi_connect(ssid, password)` joins a network
- [ ] Roaming between APs handled (or explicitly not — single-AP support is fine for v1)
- [ ] DEMO 96: `sem-sh wifi list` shows nearby networks, `sem-sh wifi connect` joins one

## M68 — WPA2 / WPA3 authentication `[  ]`

Performs the cryptographic handshake with a protected network. PBKDF2 + AES-CCMP for WPA2-PSK; SAE handshake for WPA3.

**Done when:**
- [ ] WPA2-PSK 4-way handshake completes
- [ ] Encrypted data frames send/receive correctly
- [ ] WPA3-SAE deferred to a follow-up; WPA2 sufficient for v1
- [ ] DEMO 97: connect to a WPA2 network, ping the gateway

## M69 — WiFi as primary network interface `[  ]`

Semantic OS uses WiFi by default when available. Phone tethering becomes optional/fallback.

**Done when:**
- [ ] Network interface priority: WiFi if connected, USB tether if WiFi not available, none if both absent
- [ ] User can override priority in shell
- [ ] DEMO 98: boot Semantic OS without phone connected, join WiFi, agent loop works

**Phase 18 calendar estimate:** 3-6 months of background work. Don't push it. Let it land when it lands.

---

# Phase 19 — Information Access (Web Browser + Search)

**Goal:** Claude (and the user) can search the web, read documentation, and browse crates.io/docs.rs without leaving the OS.

**Why this is a hard requirement, not a nice-to-have:** A developer who can't look up API docs or search StackOverflow is a developer who can't ship. The current `fetch` builtin (M20) does HTTP GET, but parsing HTML and navigating links is the gap.

**Depends on:** Phase 8 (TLS), Phase 9 (FS + paths), Phase 10 (Wi-Fi/DNS), M22 (agent loop). Can run in parallel with Phase 14 (self-hosting) — the browser is a Ring-3 app, not a kernel feature.

## M29 — General HTTP client `[  ]`

The current HTTP stack is POST-only (tailored for the Anthropic API). Extend to full HTTP/1.1 client.

**Done when:**
- [ ] GET, POST, HEAD, PUT, DELETE methods
- [ ] Cookie jar (persistent to FS)
- [ ] Redirect following (301/302/307)
- [ ] Basic caching (ETag, Last-Modified)
- [ ] User-agent string configurable
- [ ] DEMO 75: `fetch https://docs.rs/tokio/latest/tokio/` → HTML response saved to `/tmp/docs.html`

## M30 — HTML parser `[  ]`

No need for a full DOM engine. A streaming parser that extracts text, links, and form fields.

**Done when:**
- [ ] Tokenizer (tags, attributes, text, comments, CDATA)
- [ ] Tree builder to minimal DOM (no CSS, no JS, no layout)
- [ ] Text extraction: `html_to_text(html) -> String` (what `lynx -dump` does)
- [ ] Link extraction: `extract_links(html) -> Vec<Url>`
- [ ] Table/structural awareness (headers, lists, paragraphs)
- [ ] DEMO 76: parses `docs.rs` page, extracts all `href` links, prints text content

**Approach:** Vendored `html5ever` (Rust, Servo project, permissive license) is ~20K LOC. A lighter option: `html5gum` (streaming, no_std-friendly, ~5K LOC). Could also write a minimal parser from scratch (~2K LOC for just the extraction use case).

## M31 — Search engine integration `[  ]`

The browser doesn't need to render pages — it needs to *find* them.

**Done when:**
- [ ] DuckDuckGo HTML search API (no JS, no API key needed) — `search("rust error E0308") -> Vec<Result>`
- [ ] Or: Bing Web Search API (requires key, better results)
- [ ] Result ranking snippet (title, URL, 200-char excerpt)
- [ ] `search` builtin in sem-sh: `search "rust async traits" | grep "blog"`
- [ ] DEMO 77: `search "rust no_std tcp"` returns 5 results with titles and URLs

## M32 — Text-mode browser (optional v1) `[  ]`

Like `lynx` or `w3m` — navigate links, scroll, read text-heavy pages.

**Done when:**
- [ ] `browse <url>` command launches TUI browser
- [ ] Render HTML as formatted text (headers, bold, lists, links)
- [ ] Navigation: Up/Down/Enter to follow link, Backspace to go back, `q` to quit
- [ ] History stack (persistent to FS?)
- [ ] DEMO 78: browse `https://doc.rust-lang.org/book/`, navigate to chapter 1, read text

**V2 (deferred):** CSS-aware rendering, JavaScript engine, actual DOM. This is a multi-year project (see: Servo, Ladybird). V1 is "text extraction + link navigation" which is sufficient for documentation reading.

## M33 — Agent tool: `web_search` `[  ]`

Wire the search capability into the Claude agent loop (M22) as a new tool.

**Done when:**
- [ ] `web_search` tool added to agent tool list
- [ ] Search → read top-N results → extract text → feed into context
- [ ] Citation tracking (URL per fact, so Claude can say "per docs.rs...")
- [ ] DEMO 79: agent answers "What's the latest Rust edition?" by searching + reading rust-lang.org

---

# Phase 20 — ARM Architecture Port (Apple Silicon M2)

**Goal:** Semantic OS boots on Apple Silicon Macs (**M2 specifically**) via **dual-boot** alongside macOS. The kernel started as aarch64; this is a *return* to ARM, not a new port.

**Why M2:**
- M1/M2 have **full Asahi Linux support today** — installer works, drivers exist, community is active
- M4 support is **stalled** — Apple changed the boot chain (SPTM/GL2), Asahi has no clear path, no installer, no driver support as of June 2026
- M2 is the **sweet spot** — mature enough to "just work," cheap enough on the used market, powerful enough for daily use
- **User decision: June 2, 2026 — "M2 mac it is"**

**Why dual-boot changes the strategy:**
- No need to replace macOS — Semantic OS lives on its own APFS partition or USB/Thunderbolt drive
- macOS stays as the **build host** — compile the ARM kernel on macOS, copy to the Semantic OS partition, reboot to test
- Asahi Linux has already proven the boot chain (m1n1 → custom kernel); we follow their documented path
- Full hardware access: DART (IOMMU), AIC (interrupts), ANS (NVMe), DCP (display) — not a VM, real metal
- The W540 (x86_64) and the M2 Mac (ARM) become **parallel daily-driver targets**, not sequential

**Boot strategy (documented by Asahi Linux):**
1. m1n1 (open-source bootloader) chainloads from the Asahi partition
2. m1n1 hands off to our kernel at EL2 (same as QEMU `virt` but with real device trees)
3. Kernel boots, mounts its own root fs, drops into sem-sh
4. Reboot back to macOS via `reboot` command → macOS picker or default boot

**Depends on:** Phase 14 (self-hosting) — we need a working compiler before we can compile for ARM. The ARM port itself is a kernel-level effort that can start in parallel with M27 rustc work.

**Parallel development strategy (enabled by dual-boot):**
- x86_64 (W540): Primary dev machine, full kernel + userland development
- ARM64 (M2 Mac): Secondary target, boot via m1n1 + dual-boot, test on real hardware weekly
- Cross-compile: x86_64 rustc (cg_clif) compiles `kernel-aarch64` → copy to M2 partition → boot → iterate
- Native compile: Once M27 lands on ARM, the M2 Mac compiles its own kernel natively

**Two-machine bring-up:**
- **Stage 1 — W540 (x86_64, NOW):** Continue as primary development machine. All kernel-core work, all userspace, all agent infrastructure.
- **Stage 2 — M2 Mac (ARM64, parallel):** Install Asahi Linux's m1n1 + UEFI environment. Boot a minimal kernel-aarch64 (M34) as soon as it compiles. Daily driver testing starts the moment sem-sh + framebuffer work on ARM.
- **Stage 3 — W540 retired or secondary:** Once the M2 Mac runs the full Semantic OS stack (shell, editor, agent, web browser, self-hosting compiler), the W540 becomes backup/secondary.

**Boot chain on Apple Silicon (documented by Asahi Linux):**
```
iBoot (Apple firmware) → m1n1 (open-source bootloader, Asahi)
  → UEFI environment (optional, for standard boot)
  → Semantic OS kernel (EL2 → EL1)
  → sem-sh on framebuffer
```
- m1n1 is installed via the Asahi Linux installer (runs from macOS recovery)
- Semantic OS kernel is a PE/COFF or raw binary on the EFI partition
- Reboot to macOS: hold power button at boot → macOS picker → select macOS

## M34 — ARM64 HAL (`kernel-aarch64`) `[  ]`

Create the ARM equivalent of `kernel-x86_64`.

**Done when:**
- [ ] Boot entry (EL2/EL1 transition, exception vectors)
- [ ] MMU: ARMv8 paging (4-level, 4K granules), TTBR0/TTBR1, TLB maintenance
- [ ] Interrupts: GIC v2/v3 (distributor + redistributor + CPU interface), not x86 APIC/IOAPIC
- [ ] Timer: ARM Generic Timer (CNTVCT_EL0, CNTP_CTL_EL0), not PIT/HPET/LAPIC
- [ ] Per-CPU state: MPIDR_EL1, spin-table or PSCI bring-up
- [ ] Framebuffer: same as x86_64 (UEFI/ACPI provides it, or DeviceTree)
- [ ] DEMO 80: boots on QEMU `virt` machine, prints "Hello ARM64" to framebuffer

**What carries over unchanged:**
- kernel-core (all of it — memory, scheduler, FS, net, agent, crypto)
- semos-std (pure Rust, no asm — except syscall wrappers)
- user programs (sem-sh, editor, demos — recompiled for aarch64)
- The agent loop (pure Rust, no ISA dependency)

**What needs rewriting:**
- `kernel-x86_64/src/` → `kernel-aarch64/src/` (boot, interrupts, paging, PCI, device drivers)
- Syscall wrappers in `semos-std` (`asm!("svc #0")` instead of `asm!("syscall")`)
- Bootloader: UEFI on ARM (edk2/QEMU) or custom bootloader

## M35 — Device drivers for ARM platforms `[  ]`

ARM has no PCI bus (well, PCIe via SBSA). Device enumeration is different.

**Done when:**
- [ ] DeviceTree parser (ARM's equivalent of ACPI PCI enumeration)
- [ ] PL011 UART (QEMU `virt` console)
- [ ] VirtIO block/net on ARM (same VirtIO protocol, different transport — MMIO vs PCI)
- [ ] Generic Interrupt Controller (GIC) v2 and v3
- [ ] PSCI power management (shutdown, reboot)
- [ ] DEMO 81: boots on QEMU `virt`, enumerates VirtIO block + net, runs FS + net demos

## M36 — Apple Silicon specifics (M2) `[  ]`

The real target. Apple's hardware is proprietary but well-documented by the Asahi Linux project.

**Done when:**
- [ ] Apple Silicon boot (m1n1 or direct iBoot chain loading)
- [ ] Apple IOMMU (DART) — no VT-d, different page table format
- [ ] Apple interrupt controller (AIC) — not GIC, custom
- [ ] Apple GPIO/pinctrl
- [ ] Apple NVMe (ANS/APM) — same NVMe protocol, different transport
- [ ] Apple framebuffer (simplefb or DCP display controller)
- [ ] DEMO 82: boots on M2 Mac Mini or M2 MacBook Air, framebuffer visible

**Approach:** Leverage Asahi Linux's work (GPL, but the hardware register docs are factual — can be reimplemented). The Asahi team has published all the register-level details needed. This is "read the docs, write the driver" — not reverse engineering.

**M2 specifics:**
- M2 is ARM64 (aarch64) — same ISA as M1/M3, so kernel-core needs no changes
- Asahi Linux support for M2 is **mature and upstreamed** — all core drivers work (DART, AIC, ANS, USB, Wi-Fi)
- Fedora Asahi Remix 43 runs on M2 with native 120Hz display, full hardware support
- The core drivers (DART, AIC, ANS, USB) are identical across M1/M2/M3 generations

## M37 — Cross-compilation infrastructure `[  ]`

Build x86_64 from ARM, build ARM from x86_64.

**Done when:**
- [ ] `cargo build --target aarch64-unknown-none` from x86_64 dev machine (W540)
- [ ] `cargo build --target x86_64-unknown-none` from ARM dev machine (requires x86_64 rustc backend)
- [ ] `semos-rustc` supports multiple backends (cg_clif x86_64 + cg_clif aarch64)
- [ ] DEMO 83: build kernel-aarch64 on W540, copy to M2 Mac, boots

---

# Phase 21 — Advanced Agent Infrastructure (Claude Code Parity)

**Goal:** The agent on Semantic OS is as capable as Claude Code on macOS/VS Code. This is what makes the OS a daily driver, not a demo.

**Current state (M22):** Single-turn or basic multi-turn, `read_file`/`write_file`/`bash` tools, no context management, no code navigation, no `cargo check` integration.

**Depends on:** Phase 14 (self-hosting), Phase 15 (web search). Can start in parallel with Phase 14 (self-hosting) — these are Ring-3 agent improvements.

## M38 — Context window management `[  ]`

Claude Code's superpower: it keeps the whole codebase in context. We need the same.

**Done when:**
- [ ] `read_file` with line ranges (not whole file)
- [ ] `grep` tool (fast project-wide search)
- [ ] `find` tool (list files matching pattern)
- [ ] `list_dir` tool (tree view of directory)
- [ ] Automatic codebase indexing: build a tree-sitter AST index for fast symbol lookup
- [ ] DEMO 84: agent reads `main.rs` lines 100-150, searches for `fn handle_`, lists all files in `src/`

## M39 — Multi-file edit tool `[  ]`

Claude Code can refactor across files. Our agent needs the same.

**Done when:**
- [ ] `apply_diff` tool — apply a unified diff patch to multiple files
- [ ] `edit_file` tool — replace specific lines (more precise than `write_file`)
- [ ] `create_file` / `delete_file` tools
- [ ] DEMO 85: agent renames a function across 3 files using `apply_diff`

## M40 — Cargo integration `[  ]`

The agent needs to compile and test the code it edits.

**Done when:**
- [ ] `cargo check` tool — run compiler, return errors
- [ ] `cargo test` tool — run tests, return results
- [ ] `cargo build` tool — build binary
- [ ] Error parser: extract file/line/message from rustc JSON output
- [ ] DEMO 86: agent edits `main.rs`, runs `cargo check`, sees error, fixes it, checks again

## M41 — Persistent agent memory `[  ]`

Claude Code remembers conversations across sessions. We need the same.

**Done when:**
- [ ] Conversation history saved to FS (`/home/agent/history/`)
- [ ] `conversation_id` across reboots
- [ ] "Resume last conversation" on boot
- [ ] DEMO 87: reboot, agent resumes previous conversation about kernel bug

## M42 — Security tier for agent tools `[  ]`

The agent runs at tier 0 (Public) today. Some tools need higher tier access.

**Done when:**
- [ ] Tool-specific tier elevation: `write_file` to `/kernel/` requires tier 3 (Secret)
- [ ] User confirmation for destructive ops (delete, chmod, etc.)
- [ ] Audit log of all agent actions (append-only, tier 3)
- [ ] DEMO 88: agent tries to write to `/sec/` file, gets denied; user confirms via keyboard, succeeds

---

# Phase 22 — Package Manager + Ecosystem

**Goal:** You can `cargo install` or `semos install` tools on the OS without manual ELF copying.

**Depends on:** Phase 14 (self-hosting), Phase 15 (web search for crate discovery).

## M43 — Package manager (`semos-pkg`) `[  ]`

**Done when:**
- [ ] `install <crate>` — download from crates.io mirror, compile, install to `/apps/`
- [ ] `remove <crate>`
- [ ] `update <crate>`
- [ ] Dependency resolution (DAG, not full cargo resolver in v1)
- [ ] DEMO 89: `semos install ripgrep` → downloads, compiles, installs `/bin/rg`

## M44 — crates.io mirror / cache `[  ]`

**Done when:**
- [ ] Local registry index (crates.io-index clone)
- [ ] Crate tarball caching (`/var/cache/crates/`)
- [ ] Offline mode (install from cache only)
- [ ] DEMO 90: install crate with no network (cached)

---

# Phase 23 — Media + Entertainment (Deferred but Planned)

**Goal:** The OS can play video, run retro games, and be pleasant to use for non-dev tasks. This is "quality of life" that makes the OS a daily driver, not just a dev box.

**Depends on:** Phase 11 (iGPU), Phase 12 (NVIDIA compute), Phase 15 (web browser for streaming).

## M45 — Video playback (H.264/AV1) `[  ]`

**Done when:**
- [ ] Software decoder (vendored `dav1d` or `openh264`)
- [ ] Iris Xe QuickSync hardware decode (M14 prerequisite)
- [ ] Audio sync via M15 HD Audio
- [ ] Simple media player UI (TUI or minimal framebuffer)
- [ ] DEMO 91: play 1080p H.264 MP4 from USB stick

## M46 — Retro game engine `[  ]`

**Done when:**
- [ ] 2D sprite engine (tiny-skia based)
- [ ] Gamepad input (M16 HID)
- [ ] Audio output (M15 HD Audio)
- [ ] One ported game (e.g., Pong, Tetris, or a Doom-like)
- [ ] DEMO 92: play a game with gamepad + sound

## M47 — Music player `[  ]`

**Done when:**
- [ ] MP3/FLAC decoder (vendored `minimp3` or `symphonia`)
- [ ] Playlist management
- [ ] Background playback (audio continues while in shell)
- [ ] DEMO 93: play FLAC album from USB stick, skip tracks with keyboard

---

# Reordering Assessment: What Changes?

## Current order (what's in the file today):
```
Phase 9  → FS, time, USB, NVMe
Phase 10 → Wi-Fi, metal boot
Phase 11 → iGPU, HD Audio, HID
Phase 12 → NVIDIA compute
Phase 13 → TTY, shell, editor, agent, reboot
Phase 14 → Self-hosting (rustc, Cranelift, cargo)
```

## Proposed new order:
```
Phase 9  → FS, time, USB, NVMe
Phase 10 → Wi-Fi, metal boot
Phase 11 → iGPU, HD Audio, HID
Phase 12 → NVIDIA compute
Phase 13 → TTY, shell, editor, agent, reboot
Phase 14 → Self-hosting (rustc, Cranelift, cargo) ← CURRENT END
Phase 15 → Web browser + search (M29-M33) ← NEW
Phase 16 → ARM port (M34-M37) ← NEW
Phase 17 → Advanced agent (M38-M42) ← NEW
Phase 18 → Package manager (M43-M44) ← NEW
Phase 19 → Media + entertainment (M45-M47) ← NEW
```

## What stays the same:
- Phase 14 is still the "capstone" moment — the OS builds itself
- Phase 15-18 can all run in parallel with Phase 14 (they're Ring-3 apps)
- Phase 16 (ARM) is a kernel-level effort that can start once kernel-core is stable (now)

## What the user might be right about:
> "once we get here everything in my head is easier. but that could be wrong"

**You're right about self-hosting, but you're missing the "information access" prerequisite.** A compiler without documentation is like a car without roads. Phase 15 (web browser) is what makes Phase 14 usable. Without it, you're editing code blind.

**My recommendation:** Phase 15 and Phase 14 should be developed in parallel. The browser is a Ring-3 app that needs the same rustc that Phase 14 is building. Start the browser on the cross-build path (M23) while the self-hosting compiler is still being ported.

---

# Risk Assessment: ARM Port (Phase 16)

## What makes ARM hard:
| Risk | Mitigation |
|---|---|
| No PCI on ARM → DeviceTree instead of ACPI | Parser is ~1K LOC; Asahi docs are excellent |
| GIC vs APIC → different interrupt model | GIC is actually simpler than IOAPIC + LAPIC |
| Apple Silicon boot chain is proprietary | m1n1 (Asahi) is open source and documented |
| DART (Apple IOMMU) is custom | Asahi Linux has full DART driver; reimplementable |
| `semos-std` syscall wrappers are x86_64 asm | One file: `src/syscall.rs` — change `syscall` to `svc #0` |
| Cranelift aarch64 backend | Exists and is maintained; same backend, different target |

## What makes ARM easier than expected:
- **kernel-core is already architecture-agnostic.** The hard part (scheduler, FS, net, agent) is ISA-independent.
- **The ARM port is a *return*, not a new port.** The repo was aarch64 before it was x86_64. The boot code exists in git history.
- **Apple Silicon is the best-documented proprietary hardware ever.** Asahi Linux has published every register, every firmware format, every boot step.
- **QEMU `virt` is a perfect test target.** Boots UEFI, has VirtIO, no proprietary hardware needed for development.

## Timeline estimate (revised with dual-boot and M2 target):

| Phase | Work | Calendar estimate |
|---|---|---|
| M34 (ARM64 HAL) | Boot + MMU + interrupts + timer on QEMU `virt` | 2-3 months |
| M35 (ARM drivers) | DeviceTree + VirtIO + GIC on QEMU | 1 month |
| M36 (Apple Silicon M2) | m1n1 chainloading + DART + AIC + ANS + DCP | 3-4 months |
| M37 (cross-compile) | x86_64 rustc → aarch64 target | 1 month |
| **M2 Mac daily driver** | **sem-sh + editor + agent + Wi-Fi on real hardware** | **~6-8 months from M34 start** |
| M28 (self-host on ARM) | rustc-on-SemOS compiling itself on M2 | 6-12 months after M37 |

**Critical path:** M34 can start **now**, in parallel with M27 (rustc port on x86_64). The QEMU `virt` target needs no Apple hardware — it's pure ARM64 architecture bring-up. Once M34/M35 work on QEMU, an **M2 Mac Mini** is acquired (used market, ~$400-500) for M36.

**Dual-boot enables weekly testing cadence:** Build on x86_64 (W540), copy kernel to M2 partition, reboot, test, reboot back to macOS for the work week. No dedicated ARM dev machine needed until M36.

**Why this is faster than expected:**
- No need to reverse-engineer boot chain — Asahi Linux published it all
- macOS is the safety net — if Semantic OS panics, reboot to macOS and debug
- Cross-compile from W540 or macOS host — no waiting for self-hosted ARM compiler
- kernel-core is already architecture-agnostic — the HAL is the only new code

---

# Hardware Plan (Updated June 2, 2026)

| Machine | Role | Specs | Timeline |
|---|---|---|---|
| **W540** (current) | Primary development — x86_64 kernel bring-up, daily dev, cross-compiles for ARM | x86_64, 32GB RAM, SSD | Now |
| **M2 Mac** (future) | ARM test target — dual-boot Semantic OS, hardware testing, eventual daily driver | M2 (8/8 or 8/10 core), 16GB+ RAM | Acquire after M35 completes on QEMU |
| **M4/M5 Mac** (future) | Upgrade path — when Asahi Linux catches up or when you no longer need dual-boot | Latest Apple Silicon | 2+ years, or when dual-boot is no longer needed |

**No P1:** The ThinkPad P1 was considered but not acquired. W540 is the current and planned x86_64 machine.

---

# The Real End State

After all phases, the user sits at an M2 Mac running Semantic OS:

1. **Boots in 3 seconds** — no UEFI bloat, no systemd
2. **Drops into sem-sh** — native shell, familiar commands
3. **Types `agent`** — launches the Claude TUI
4. **Asks Claude:** "Find me the Rust docs for async traits and add an example to my project"
5. **Claude searches the web** (Phase 15), reads docs.rs, finds the example
6. **Claude edits the file** (Phase 17 multi-file edit), runs `cargo check` (Phase 17 cargo integration)
7. **Claude says:** "Compiles clean. Want me to run the tests?"
8. **User types `edit main.rs`** — native editor opens
9. **User makes a change, saves, runs `cargo build`** — rustc on SemOS compiles (Phase 14)
10. **User types `reboot`** — boots into new kernel (Phase 13 M24)
11. **User installs a new tool:** `semos install ripgrep` — package manager works (Phase 18)
12. **User watches a video** while the agent compiles in background (Phase 19)
13. **User reboots to macOS** to use a proprietary app, then back to Semantic OS — seamless dual-boot

That's the vision. The current roadmap gets to step 6. The expansion gets to step 13.

---

# exFAT Shared Partition (Dual-Boot File Sharing)

For cross-OS file sharing between macOS and Semantic OS on the M2 Mac:

**Disk layout:**
```
Disk layout on M2 Mac:
┌─────────────────┬─────────────────┬─────────────────┐
│   APFS (macOS)  │   Your FS       │   exFAT         │
│   ~70% of disk  │   (Semantic OS) │   (shared)      │
│                 │   ~20% of disk  │   ~10% of disk  │
└─────────────────┴─────────────────┴─────────────────┘
```

- **APFS:** macOS lives here, untouched
- **Your FS:** Semantic OS root partition — kernel, binaries, agent history
- **exFAT:** Shared workspace — `~/workspace/`, `~/downloads/`, anything cross-OS

**How it works in practice:**
1. Boot macOS → mount exFAT partition → edit code in VS Code → save
2. Reboot to Semantic OS → exFAT auto-mounts on boot → `cat /shared/project/main.rs`
3. Agent edits the same file → saves → reboot to macOS → changes visible

**What Semantic OS needs:**
- exFAT driver (~2-3 weeks of work in Rust)
- FAT table traversal, directory entries, cluster allocation
- No journaling, no complex metadata — much simpler than FS Stage 3
- Or: network-based sharing (SMB/NFS) if exFAT is deferred

**My recommendation:** Add exFAT driver to the FS roadmap as a follow-up to M4. It's the right intersection of "useful for dual-boot" and "not a research project."

---

# Next Steps

1. **Validate this expansion** — does the user agree with the phases and ordering?
2. **Add to the main ROADMAP.md** — merge these milestones into the existing file
3. **Pick the first new milestone** — M29 (HTTP client) is the natural next step after M22 (agent live)
4. **ARM port decision** — does the user want to start M34 now (in parallel with M27), or after M28?
5. **Acquire M2 Mac** — after M35 completes on QEMU (estimated 3-4 months), buy a used M2 Mac Mini (~$400-500)
6. **System utilities** — `top` process monitor (M48) should be built as a Ring-3 app once the kernel exposes process query syscalls

---

# Phase 24 — System Utilities

Small but essential tools that make the OS inspectable and debuggable. These are Ring-3 apps that run in the shell and exercise kernel introspection interfaces.

## M48 — `top` process monitor `[  ]`

A real-time process and resource monitor, like `top` or `htop` on Linux. This is the first thing you run when the OS feels slow or something is hanging.

**What the kernel needs:**
- `SYS_PROC_LIST` — enumerate all processes (PID, parent PID, state, name)
- `SYS_PROC_STAT` — per-process CPU time (user + system ticks), memory usage (resident bytes), priority, thread count
- `SYS_PROC_KILL` — send signal to process (already exists as part of process management, but may need exposure)
- Or: extend existing `SYS_PS` / `SYS_PROC_INFO` if they exist

**What the TUI app does:**
- [ ] Query process list every N seconds (configurable, default 1s)
- [ ] Sort by CPU% (default), memory, PID, name
- [ ] Display: PID | USER | CPU% | MEM% | TIME | COMMAND
- [ ] Interactive keys: `q` quit, `P` sort by CPU, `M` sort by memory, `k` kill (prompt for PID), `r` renice (prompt for PID + priority)
- [ ] Minimal mode: just the process list, no TUI overhead (like `ps`)
- [ ] Built against `semos-std` (no_std-friendly if we vendor it, or std if available)

**Approach:**
- Kernel side: add a `SYS_PROC_LIST` syscall that fills a user-provided buffer with process descriptors. If this already exists as part of the scheduler's debug interface, expose it.
- User side: ~500 LOC Rust app using the TUI infrastructure (M7/M8 framebuffer or M19 TTY). Can reuse `TtyConsole` drawing primitives from the editor.
- If the kernel doesn't have per-process CPU accounting yet, this unblocks adding it (useful for scheduler debugging anyway).

**Done when:**
- [ ] Kernel exposes process query interface (or confirms existing one works)
- [ ] `/bin/top` binary built against `semos-std`
- [ ] TUI display: process list, sortable, refreshes in real time
- [ ] Interactive kill and renice
- [ ] DEMO 94: boot OS, run `/bin/top`, see kernel tasks + shell + any running processes, sort by CPU, kill a test process, verify it dies

**Depends on:** Phase 13 (M20 shell, M19 TTY), M25 stdlib. Can be built in parallel with Phase 14.

**Why it matters:** Without a process monitor, debugging a hung system means guessing. This is the first diagnostic tool any developer reaches for. It's also a validation that the kernel's scheduler accounting is correct.

---

*Drafted by Kimi Claw, 2026-06-02. Based on the existing ROADMAP.md, user decision to target M2 Mac for dual-boot, and updated hardware plan (W540 as primary dev machine, no P1).*
*Updated 2026-06-02: Added Phase 20 — System Utilities with M48 (`top` process monitor) per user request.*
*Updated 2026-06-04: Inserted Phases 15-18 (USB tethering, QR pairing, companion app, bare-metal WiFi) at the front per user direction; renumbered the original web/ARM/agent/package/media/utilities phases to 19-24. USB tethering confirmed to work on the W540's USB-3-only sockets via the companion USB-2 PHY (xHCI bringup landed in F12). Phases 15-17 sequenced ahead of WiFi because tether → online in 3-4 weeks; full WiFi is 3-6 months background work.*
*Updated 2026-06-04 (later): Added the "Security thesis disciplines" preamble after reading `docs/semos-security-thesis.md`. Every milestone from M50 onward should answer the four surface questions (new syscall? smallest shape? capability check? blast radius?) before work starts. Honest-timelines note added — the web browser is 2-3 years, not 6 months; calendar estimates throughout are optimistic.*
