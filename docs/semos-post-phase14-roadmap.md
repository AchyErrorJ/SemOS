# Semantic OS — Post-Phase-14 Roadmap (Revised)

> **Roadmap family:** `ROADMAP.md` = historical session log (canonical record
> of what landed). **This file = the active forward plan.**
> `ROADMAP_EXPANSION_PROPOSAL(JUNE26).md` = superseded June-4 draft, kept for
> its Phases 19+ content (browser, ARM port, packages, media) not yet folded
> in here. `IPHONE_SENSOR_OFFLOAD_PLAN.md` = the Phase-18 preview (LiDAR over
> the tether) unlocked by the 2026-06-10 Phase-15 leapfrog.

**Scope:** What happens after rustc_lint closes, F13 ships, DEMO 80 lands, and Phase 14 (self-hosting) is complete.

**Principle:** Each milestone produces a demoable artifact and moves the OS toward being *useful*, not just *complete*. The OS becomes a tool people could actually pick up before it tries to do everything.

**Core architectural commitments (locked):**

- **Phone-as-peripheral.** The phone provides capabilities the OS doesn't have (crypto, camera, GPS, identity, network connectivity).
- **Pairing IS authentication.** No password on the laptop. No login screen. The paired phone is the user account. Identity, credentials, and authorization all flow from the pairing.
- **From-scratch on the OS side.** No POSIX, no libc, no compatibility shims. The OS stays small because the surface stays small.
- **Phone connects to the world over WiFi when available, cellular as fallback.** The OS doesn't care which; it sees one logical network channel via the paired phone.

**Sequence:**

1. **Phase 15 — USB tethering** (first internet on bare metal, validates the milestone using existing OS network stack)
2. **Phase 16 — Pairing + Expo bridge app** (prototype the protocol on a phone using cross-platform tools, no Mac required)
3. **Phase 17 — Layer-4 phone bridge** (replaces tethering for daily use; uses phone's WiFi via the bridge app, no cellular cost)
4. **Phase 18 — Companion app capabilities** (crypto, identity, camera, GPS, audio, notifications — each as protocol-level RPC)
5. **Phase 19 — Native Swift bridge rewrite** (when a Mac is available; production-quality bridge app, then ship to other users)
6. **Phase 20 — Bare-metal WiFi** (long-running background work; untethered operation eventually)

Phases 15-18 are sequential and run in 2026. Phase 19 begins when a Mac is acquired. Phase 20 runs in parallel as background work whenever there's an evening for it.

---

## Phase 15 — USB Tethering (the "online on bare metal" moment)

**Goal:** W540 boots Semantic OS, phone plugs in over USB-C with Personal Hotspot on, kernel sees the phone as a network device, internet works (over cellular), agent calls Anthropic API over real wire.

**Why this first:** It's the smallest path to *the OS is real and online*. The kernel's existing TCP/IP and TLS stack works — only USB-class drivers and a DHCP client need to be added. Cellular data cost is real but bounded; this phase exists to validate the *milestone*, not to be the daily-use mode.

**Honest constraint:** USB tethering on iOS forces traffic over cellular even when the phone has WiFi available. That's an Apple platform limitation. Phase 17 (Layer-4 bridge) is the architectural fix for this; Phase 15 is the milestone-proving step that comes first.

**Depends on:** Phase 14 closed (rustc self-hosts), xHCI work from F12 (already done), existing TCP/IP/TLS stack (already done).

> **STATUS 2026-06-10 — Phase 15 substantially complete, via a different route
> than planned below.** Reality on the W540 diverged from the plan in three
> ways, all now landed (commits `d11a63e`..`eef5a1f`):
>
> 1. **iPhone tethering is ipheth, not CDC-ECM** (Apple vendor class
>    0xFF/0xFD/0x01 at alt 1, hidden in USB configuration 4 — hosts must
>    iterate configs). CDC-ECM driver still exists (DEMO 81) for dongles.
> 2. **The transport is a new standalone EHCI driver, not xHCI.** The Lynx
>    Point xHCI never completed a USB-2 port reset (~30 builds); `usb/ehci.rs`
>    owns the USB-2 ports Windows-7-style: per-controller async schedules,
>    QH/qTD control+bulk, Rate-Matching-Hub enumeration, split transactions,
>    persistent bulk-IN RX QH for ipheth.
> 3. **DHCP was bypassed**: the iPhone tether subnet is fixed
>    (172.20.10.0/28, phone = gateway+DNS), so `net::init_with_ipconfig`
>    statically configures 172.20.10.9/28. M52 stays as an optional nicety.
>
> Hardware-confirmed: iPhone enumerates → Trust prompt → MAC + carrier 0x04.
> `ipheth0` NetDevice + smoltcp bring-up landed, **traffic over the tether not
> yet validated on hardware** — that's the next session's first move, then
> M53/M54 are live. See `docs/IPHONE_SENSOR_OFFLOAD_PLAN.md` for the Phase-18
> preview this unlocked (LiDAR point-cloud streaming over the tether IP link —
> note its iOS app needs ARKit, which Expo Go cannot host; it needs an EAS
> cloud build or a Mac, slightly ahead of the Phase 19 assumption).

### M50 — USB device-class network driver `[x — landed as ipheth-over-EHCI 2026-06-10; CDC-ECM variant from DEMO 81 also in tree]`

**What it does:** Recognizes a USB device that presents itself as a CDC-ECM (Ethernet over USB) endpoint, enumerates its endpoints, exposes it to the kernel as a network interface.

**Why CDC-ECM first:** Standard iPhone tethering protocol. Less complex than RNDIS. About 500-1000 lines of Rust.

**Done when:**
- [ ] xHCI recognizes a USB-class-2 (Communications) device on insertion
- [ ] CDC-ECM control endpoint parsed (interface descriptors, MAC address)
- [ ] CDC-ECM data endpoints bound (bulk-in for receive, bulk-out for transmit)
- [ ] `net_interface_register()` called with the new interface
- [ ] DEMO 81: insert tethered iPhone with Personal Hotspot on, kernel logs "CDC-ECM device detected, MAC xx:xx:xx:xx:xx:xx, interface usb0 up"

### M51 — RNDIS driver `[deferred — no Android requirement on the table; iPhone path landed via ipheth]`

**What it does:** Same as CDC-ECM but for Microsoft's RNDIS protocol. Android phones default to RNDIS when tethering.

**Why both:** iPhone uses CDC-ECM, Android uses RNDIS. Supporting both means the OS doesn't care which phone you have. About 1500-2000 lines of Rust because RNDIS has a management plane.

**Done when:**
- [ ] RNDIS detection via USB class/subclass/protocol triple (0xE0/0x01/0x03)
- [ ] RNDIS INIT message exchange completes
- [ ] OID queries handled (link speed, MAC address, link state)
- [ ] Packet framing and de-framing on bulk endpoints
- [ ] DEMO 82: insert tethered Android phone, kernel logs RNDIS init success and interface up

### M52 — DHCP client `[bypassed 2026-06-10 — tether subnet is fixed, static config landed via init_with_ipconfig; DHCP remains an optional nicety for non-tether interfaces]`

**What it does:** When a network interface comes up, sends DHCPDISCOVER, parses DHCPOFFER, sends DHCPREQUEST, applies the leased IP/gateway/DNS configuration.

**Implementation note:** DHCP is small UDP (~200-300 lines). RFC 2131. Client side only. DHCPv6 deferred.

**Done when:**
- [ ] DHCPDISCOVER broadcast on interface bring-up
- [ ] DHCPOFFER parsed (yiaddr, gateway, DNS, lease time)
- [ ] DHCPREQUEST + DHCPACK round trip completes
- [ ] IP/gateway/DNS applied to kernel network state
- [ ] Lease renewal timer (T1 at 50% of lease)
- [ ] DEMO 83: kernel boots, USB-tether attached with Personal Hotspot on, gets IP from phone, `ping` works

### M53 — Real-world TLS validation `[NEXT — unblocked once tether traffic validates on the W540]`

**What it does:** Your TLS stack has been talking to QEMU-emulated networks. Real Anthropic servers exercise certificate paths, TLS extensions, and edge cases QEMU never sees. Hardening pass, not a rewrite.

**Done when:**
- [ ] Real Anthropic API certificate chain validates successfully on bare metal
- [ ] SNI extension correctly sent for `api.anthropic.com`
- [ ] TLS 1.3 handshake completes against real server
- [ ] HTTP/2 negotiation works (or fall back to HTTP/1.1 cleanly)
- [ ] DEMO 84: full agent loop turn on bare metal — boot, type prompt, get response from real Claude, over real cellular network, no QEMU

### M54 — The milestone session `[NEXT after M53]`

**What it does:** Bring the agent loop, the shell, the editor, the TLS stack, and USB tethering together. A user can sit at the W540 with their phone plugged in and do something useful.

**Done when:**
- [ ] Boot Semantic OS on W540
- [ ] Plug in phone, enable Personal Hotspot
- [ ] Drop into sem-sh
- [ ] Type `agent`, ask "what is the time in Tokyo right now?"
- [ ] Agent responds correctly via cellular network
- [ ] DEMO 85: video of full session, ~5 minutes, no narration needed

**Phase 15 calendar estimate:** 3-4 weeks of focused work.

**Daily-use note:** Phase 15 establishes that the network architecture works end-to-end on real hardware. It is NOT the long-term daily-use mode because it burns cellular data even when the phone has WiFi. Phase 17 fixes this.

---

## Phase 16 — Pairing + Expo Bridge App (the trust bootstrap)

**Goal:** Pair the W540 with a specific phone via QR code. Establish a TLS-protected channel between them. Build the companion app as an Expo (React Native) prototype so it can be developed without a Mac.

**Why Expo:** Native iOS development requires Xcode requires a Mac. No Mac currently. The bridge app is networking-and-protocol code, not AR or other deeply-native iOS features — Expo's surface is sufficient for a prototype. Native Swift rewrite happens in Phase 19 when a Mac arrives.

**Tradeoff:** Expo's networking primitives are rougher than native iOS networking. Background execution is limited. The Expo version is explicitly a prototype that validates the protocol and unblocks Phase 17 — it is not the production app.

**Why this is the right place for Expo (not for LegiView):**
- The bridge app is protocol + minimal UI. Expo handles protocol fine.
- No AR component. (ARKit was the reason LegiView couldn't be Expo.)
- Iteration happens locally between W540 and phone via Expo Go. No App Store, no TestFlight, no Mac needed for the prototype.
- Throwaway-friendly. When Phase 19 rewrites in Swift, no real work is lost — protocol stays, only the implementation changes.

**Depends on:** Phase 15 (network on bare metal working at least over tethering).

### M55 — Pairing protocol design `[  ]`

**What it does:** Define the on-wire format for the pairing handshake. Both sides generate keypairs, exchange public keys, establish a shared session key, all without trusting any prior state.

**The QR code contains:**
- Companion app's public key
- Network address (IP + port) where companion app is listening on the local network
- Pairing session nonce
- Protocol version

**Implementation note:** The W540 has no camera in Phase 16. The QR code is *typed* into the W540 as a base64 or base58 string. Optical scanning is a Phase 18 feature when the camera capability exists.

**Done when:**
- [ ] Protocol spec document committed (`docs/pairing-v1.md`)
- [ ] Wire format defined (Protobuf or Cap'n Proto schema)
- [ ] Threat model addressed: replay attacks, MITM during pairing, downgrade attacks
- [ ] Test vectors: known pairing run produces known shared secret
- [ ] Protocol is language-neutral — Rust and TypeScript implementations stay in sync via the schema

### M56 — Pairing on the Semantic OS side `[  ]`

**What it does:** Parses the QR code (as typed input), opens a TLS connection to the local network address, completes the pairing handshake, stores the resulting paired identity persistently.

**Done when:**
- [ ] `sem-sh pair <qr-string>` command works
- [ ] Pairing completes against the Expo companion app
- [ ] Paired identity persists across reboots (stored in `/etc/paired-devices/`)
- [ ] `sem-sh paired list` shows current pairings
- [ ] `sem-sh unpair <id>` removes a pairing
- [ ] DEMO 86: pair with Expo app on phone, reboot W540, verify pairing still works

### M57 — Expo bridge app skeleton `[  ]`

**What it does:** First version of the bridge app, written in TypeScript using Expo + React Native. Displays a QR code, accepts incoming pairing connections, persists paired peers in `expo-secure-store`.

**Tech stack:**
- Expo SDK (latest stable)
- TypeScript
- `react-native-tcp-socket` for TCP listener
- `react-native-tls` for TLS handshake
- `expo-secure-store` for paired-peer storage (uses iOS Keychain / Android Keystore under the hood)
- `react-native-zeroconf` for local network discovery
- `expo-qrcode-svg` or similar for QR code display
- Cap'n Proto or Protobuf for message serialization

**Done when:**
- [ ] App displays a QR code on tap of "Pair new device"
- [ ] App listens on a local-network TCP port
- [ ] Pairing handshake completes server-side
- [ ] Paired peer's public key stored in secure storage
- [ ] App shows list of currently paired devices
- [ ] Runs on iPhone via Expo Go (no TestFlight needed for personal use)
- [ ] DEMO 87: end-to-end pairing — W540 boots with Phase 15 networking, app shows QR code, user types QR string into W540, both sides confirm "paired with Jer's iPhone"

**Honest scope note:** The Expo bridge app is *yours*, on *your phone*. Not shipping to other users yet. That's a Phase 19 problem. For now, the app exists to make Semantic OS development self-sufficient.

**Phase 16 calendar estimate:** 4-6 weeks. The pairing protocol design is the harder intellectual content; the Expo app is mostly wiring known libraries together.

---

## Phase 17 — Layer-4 Phone Bridge (cellular cost solved)

**Goal:** Replace USB tethering as the daily-use networking mode. The companion app forwards network operations from Semantic OS through iOS's standard networking APIs (which use WiFi when available, cellular when not). The Personal-Hotspot-required, cellular-only constraint of Phase 15 goes away.

**Architecture:** Layer-4 (socket-level) bridging.

- Semantic OS makes structured RPC calls over the paired channel: `connect(host, port) -> socket_id`, `send(socket_id, bytes)`, `recv(socket_id) -> bytes`, `close(socket_id)`
- The bridge app receives these calls, translates them to iOS `NWConnection` / `URLSession` operations
- iOS handles the actual network connectivity using whatever path is best (WiFi preferred, cellular fallback)
- Results flow back over the paired channel

**Why Layer-4 instead of Layer-3 packet forwarding:**
- Layer-3 requires iOS Network Extensions entitlements, which are real bureaucratic friction with Apple
- Layer-4 works with standard app capabilities, runs on Expo
- The kernel surface shrinks: no TCP retransmits, no congestion control, no packet routing inside the OS. Phone handles all that.
- TLS still runs in Semantic OS — the security thesis stays intact. The phone handles transport, the OS handles crypto.

**Authentication side-effect:** Once the paired channel is the only network path, *being paired IS being authenticated*. No login screen on the OS. The phone holds the user identity; the bridge enforces that the OS is acting on the paired user's behalf.

**Depends on:** Phase 16 (pairing protocol + Expo bridge app exist).

### M58 — Layer-4 RPC protocol `[  ]`

**What it does:** Defines the wire format for socket operations between Semantic OS and the bridge app, over the paired TLS channel.

**Operations:**
- `dns_resolve(hostname) -> ip_address`
- `tcp_connect(host, port) -> socket_id`
- `tcp_send(socket_id, bytes) -> bytes_written`
- `tcp_recv(socket_id, max_bytes) -> bytes`
- `tcp_close(socket_id)`
- `udp_send(host, port, bytes)`
- `udp_recv() -> (host, port, bytes)`

**Done when:**
- [ ] Protocol document committed (`docs/network-bridge-v1.md`)
- [ ] Wire format defined
- [ ] Message framing handles concurrent sockets cleanly
- [ ] Async semantics defined (multiple in-flight requests)
- [ ] Test vectors: known sequence of operations produces known wire bytes

### M59 — Kernel socket abstraction `[  ]`

**What it does:** Replaces (or sits alongside) the existing TCP/IP stack in Semantic OS. When configured to use the bridge, network calls go through the paired channel instead of through the kernel's own TCP/IP stack.

**Done when:**
- [ ] `socket(AF_INET, SOCK_STREAM, 0)` returns a bridge-backed socket
- [ ] Standard send/recv/close semantics work over the bridge
- [ ] TLS stack runs unmodified on top of bridge-backed sockets
- [ ] Mode switch: kernel can use either local TCP/IP (Phase 15 / QEMU) or bridge (Phase 17 / daily use)
- [ ] DEMO 88: agent loop runs over bridge, uses phone's WiFi connection, no cellular burn

### M60 — Bridge app gains socket forwarding `[  ]`

**What it does:** The Expo app receives RPC requests from Semantic OS, translates them to iOS network operations.

**Done when:**
- [ ] App handles `dns_resolve` via iOS DNS APIs
- [ ] App handles `tcp_connect` via `NWConnection` or `react-native-tcp-socket`
- [ ] App handles `tcp_send/recv` with proper buffering
- [ ] Connection lifecycle handled cleanly (multiple concurrent connections supported)
- [ ] Background networking mode allows the app to keep the bridge alive during use
- [ ] DEMO 89: W540 makes 100 concurrent HTTPS requests via bridge, all succeed, all use phone's WiFi when available

### M61 — Authentication-via-pairing formalized `[  ]`

**What it does:** Documents and enforces that the paired phone IS the user account. Removes any latent "user account" concept from Semantic OS.

**Done when:**
- [ ] Boot flow documented: no paired phone = degraded mode (local work only, no network, no identity)
- [ ] Boot flow documented: paired phone reachable = full operation as paired user
- [ ] Short-lived token mechanism for graceful degradation when phone briefly unavailable (5-15 minute grace period)
- [ ] First-boot setup flow: user runs `pair`, sees prompt, completes pairing
- [ ] Unpair flow: `sem-sh unpair` removes a pairing, future operations require re-pairing
- [ ] No password storage anywhere in Semantic OS

**Phase 17 calendar estimate:** 4-6 weeks. After this phase, the daily-use story is clean: phone is your identity, phone is your network connection, no passwords, no cellular cost when WiFi is available.

---

## Phase 18 — Companion App Capabilities

**Goal:** Each thing the phone provides becomes a Semantic OS capability accessible via the paired channel. The kernel makes structured requests; the bridge app does the work using iOS APIs; results return.

**Why this third:** This is the phone-as-peripheral payoff. Camera, GPS, biometric auth, audio, push notifications, identity — all become Semantic OS capabilities without Semantic OS having to implement them.

**Implementation note:** Each capability is one milestone. They can be implemented independently in any order based on which is needed first.

**Depends on:** Phase 17 (paired channel with RPC protocol exists).

### M62 — Crypto capability (Secure Enclave / StrongBox) `[  ]`

**What it does:** Semantic OS requests crypto operations from the phone's hardware-backed key store. Private keys never leave the phone.

**Done when:**
- [ ] `request_generate_keypair(label)` → phone creates keypair in Secure Enclave, returns public key
- [ ] `request_sign(label, data)` → phone signs data with named key
- [ ] `request_decrypt(label, ciphertext)` → phone decrypts with named key
- [ ] Face ID / Touch ID required for use, configurable per-key
- [ ] DEMO 90: Semantic OS generates a keypair stored in iPhone Secure Enclave, signs a test message, verifies signature on-device

### M63 — Identity capability `[  ]`

**What it does:** The phone is the user's identity. Sign in to Anthropic, GitHub, anything that needs auth — happens via the phone.

**Done when:**
- [ ] `request_identity()` returns user's primary identity from phone
- [ ] OAuth flow proxied through phone (phone opens browser, completes flow, returns token)
- [ ] Token storage in phone's Keychain, not on Semantic OS disk
- [ ] DEMO 91: Semantic OS authenticates to Anthropic API using a token held in iPhone Keychain, no API key on disk

### M64 — Camera capability `[  ]`

**What it does:** Semantic OS asks the phone to capture a photo or scan a QR code. Phone opens camera UI, user takes photo or scans, result returns over paired channel.

**Done when:**
- [ ] `request_camera_capture(mode)` opens camera app on phone
- [ ] User takes photo or cancels; result returned over channel
- [ ] `request_qr_scan()` opens camera with QR detection; returns scanned string
- [ ] Photo metadata stripped of GPS by default, optional inclusion
- [ ] DEMO 92: Semantic OS requests a photo, phone captures it, photo appears in `/tmp/capture.jpg` on W540

### M65 — GPS capability `[  ]`

**What it does:** Semantic OS asks the phone for location. Phone returns coordinate with accuracy.

**Done when:**
- [ ] `request_location()` returns lat/lng/accuracy/timestamp
- [ ] User authorization required per session (iOS prompts the first time)
- [ ] DEMO 93: agent answers "where am I" by requesting location and reverse-geocoding

### M66 — Microphone capability `[  ]`

**What it does:** Semantic OS asks the phone to record audio. For voice prompts to the agent, transcription, any audio use case.

**Done when:**
- [ ] `request_audio_capture(duration)` records audio on phone
- [ ] Audio returned as WAV or compressed format
- [ ] DEMO 94: speak a question into phone, agent receives transcription, responds

### M67 — Push notification capability `[  ]`

**What it does:** Semantic OS sends a notification to be displayed on the phone. For long-running operations, alerts, agent results when the user is away from the laptop.

**Done when:**
- [ ] `request_notification(title, body, action)` shows notification on phone
- [ ] User tap-to-action sends a callback back to Semantic OS
- [ ] DEMO 95: long compile finishes on Semantic OS, sends "build done" notification to phone

**Phase 18 calendar estimate:** Each capability is 1-2 weeks. Picking which ones first depends on which Semantic OS applications need them. Probably crypto + identity first (M62, M63) — they unlock secure operation. Camera second (M64) — most demoable. The rest as needed.

---

## Phase 19 — Native Swift Bridge Rewrite (when Mac arrives)

**Goal:** Replace the Expo bridge app with a native Swift implementation. Production-quality, distributable via TestFlight or App Store, suitable for shipping to other users.

**Why rewrite:** Expo got the protocol validated and made Semantic OS self-sufficient for personal use. But Expo has real limitations for production:
- Background execution is fragile; iOS kills backgrounded RN apps more aggressively
- Network Extensions framework (true VPN-style bridging) requires native code
- Some iOS capabilities (deep Keychain features, advanced LiveActivity-style notifications) have no Expo wrapper
- Performance and battery: native Swift is meaningfully better for always-on background networking

**When to start:** As soon as a Mac arrives. Used Mac mini M1 ($400-600 CAD) when summer income clears.

**What stays the same:**
- The pairing protocol (M55)
- The Layer-4 RPC protocol (M58)
- The capability protocols (M62-M67)
- All the design work done in Phases 16-18

**What changes:**
- Implementation language: TypeScript → Swift
- iOS APIs: react-native-tcp-socket → NWConnection / Network framework
- Storage: expo-secure-store → direct Keychain via CryptoKit
- Background networking: standard Expo modes → proper iOS background tasks + Network Extensions if needed

### M68 — Swift app skeleton `[  ]`

**What it does:** New Xcode project, basic SwiftUI shell, same UX as Expo version. Pairing, paired devices list, status.

**Done when:**
- [ ] Xcode project builds and runs on iPhone via TestFlight
- [ ] UX feature-parity with Expo version
- [ ] Pairing protocol implementation in Swift

### M69 — Layer-4 bridge in Swift `[  ]`

**What it does:** Re-implement the socket forwarding using native iOS network APIs.

**Done when:**
- [ ] NWConnection-based TCP forwarding
- [ ] Background networking entitlement working
- [ ] Performance benchmarks vs Expo version (Swift should win meaningfully on latency and battery)
- [ ] Feature parity with M60

### M70 — Capability migration `[  ]`

**What it does:** Re-implement all the capabilities (crypto, identity, camera, GPS, audio, notifications) in Swift, using native iOS APIs directly.

**Done when:**
- [ ] All Phase 18 capabilities work on the Swift version
- [ ] Native Keychain + CryptoKit for crypto operations
- [ ] Native AVFoundation for audio/camera
- [ ] Feature parity with M62-M67

### M71 — Distribution `[  ]`

**What it does:** Ship the Swift app to other users via TestFlight initially, App Store eventually.

**Done when:**
- [ ] $99/year Apple Developer Program enrollment
- [ ] TestFlight build distributed to test users
- [ ] App Store submission and approval

**Phase 19 calendar estimate:** 6-10 weeks once Mac is acquired. Faster because the protocols are already designed and the Expo version is a working reference implementation.

---

## Phase 20 — Bare-Metal WiFi (NOW A MAIN TRACK — active 2026-06)

> **Status update 2026-06-15:** No longer background — this is the active online
> path on the T540p. **M72 DONE** (7260 enumerated, INIT+RUNTIME firmware ALIVE,
> calibration forwarded, real MAC from NVM). **M73 DONE** (LMAC scan returns real
> SSIDs; `wifi` + `wifi connect <n> <pass>` shell commands work; full Phase-A host-
> command join: PHY ctx + MAC ctx + binding + ADD_STA + time-event all HW-confirmed).
> **M74 in progress** — WPA2 PMK derivation working on live input (PBKDF2-SHA1, KAT'd);
> PTK + EAPOL-MIC crypto + the RSN IE built & KAT-verified offline (2026-06-15);
> blocked on **Phase-B first on-air frame TX** (the `0x1c` TX path: queue-enable +
> off-channel-assert fixed, currently `consumed=0` SCD-scheduling fix is built but
> unbooted — boot drives dead). Detail in the `semos-wifi` project memory; see also
> `MASTER_ROADMAP_2026-06-15.md`.

**Goal:** Semantic OS connects to WiFi networks directly, without requiring a paired phone for network access. The phone remains useful for everything else (identity, crypto, camera, etc.); networking becomes independent.

**Why this in the background:** WiFi from scratch is 3-6 months of focused work. It shouldn't block the other useful capabilities. Run it as the long-running side project that lands when it lands.

**Depends on:** Phase 14 (self-hosting), nothing else. Can start in parallel with Phase 15+ work, though it shouldn't compete for attention.

**Implementation note:** Start with one specific chip — the Intel Wireless 7260 in the W540. Worry about other chips later or never.

### M72 — WiFi chip enumeration and firmware load `[x — DONE 2026-06-13]`

**What it does:** Identifies the Intel WiFi chip on PCIe, loads the firmware blob, brings the chip out of reset.

**Done when:**
- [ ] PCI device detected, firmware loaded from `/lib/firmware/`
- [ ] Chip reports itself as alive via mailbox interface
- [ ] DEMO 96: kernel boots, logs "Intel Wireless 7260 detected and initialized"

### M73 — 802.11 management state machine `[x — scan + Phase-A join DONE 2026-06-15; on-air assoc pending Phase-B TX]`

**What it does:** Scans for networks, joins one, handles association/disassociation, manages beacon timing.

**Done when:**
- [ ] `wifi_scan()` returns list of nearby networks
- [ ] `wifi_connect(ssid, password)` joins a network
- [ ] DEMO 97: `sem-sh wifi list` shows nearby networks, `sem-sh wifi connect` joins one

### M74 — WPA2 / WPA3 authentication `[🔨 — PMK/PTK/EAPOL-MIC crypto + RSN IE built & KAT'd 2026-06-15; 4-way handshake blocked on Phase-B frame TX]`

**What it does:** Performs the cryptographic handshake with a protected network.

**Done when:**
- [ ] WPA2-PSK 4-way handshake completes
- [ ] Encrypted data frames send/receive correctly
- [ ] WPA3-SAE deferred to a follow-up

### M75 — WiFi as primary network interface `[  ]`

**What it does:** Semantic OS uses WiFi by default when available. Phone bridge becomes optional fallback.

**Done when:**
- [ ] Network interface priority: WiFi if connected, phone bridge if WiFi not available
- [ ] User can override priority in shell
- [ ] DEMO 98: boot Semantic OS without phone connected, join WiFi, agent loop works

**Phase 20 calendar estimate:** 3-6 months of background work. Don't push it.

---

## What this roadmap commits to

**1. The agent loop on bare metal is the next milestone after Phase 14.** Not "completeness" — usefulness. The OS becomes something you'd actually use.

**2. The phone is treated as a first-class peripheral.** Network connectivity, crypto, identity, camera, GPS, audio, notifications all live on the phone. Semantic OS borrows them via protocol. The kernel stays small.

**3. Pairing IS authentication.** No password on the laptop. No login screen. The paired phone is the user account. Identity, credentials, and authorization all flow from the pairing. This is a design property, not a feature — it emerges naturally from Phases 16-17 done correctly.

**4. USB tethering is the milestone-proving step, not the daily-use mode.** It validates "OS works on bare metal with real internet." But it burns cellular data. Phase 17 fixes that.

**5. Expo is acceptable for the bridge app prototype, not for LegiView.** The bridge app is protocol + minimal UI; Expo handles that. LegiView needs ARKit which is iOS-native only. Different projects, different stack choices.

**6. The Mac is needed for Phase 19 (native bridge) and for LegiView native development.** Not for Phases 15-18. The OS can become useful without a Mac via the Expo bridge.

**7. The from-scratch commitment holds.** No POSIX. No libc. No vendored compatibility layers in the kernel. The Expo app uses iOS APIs because that's how iOS works, but the wire protocol is yours and the OS side is from-scratch.

**8. Each phase ends with a demoable artifact.** USB tether → live agent loop on bare metal. Pairing → "paired with iPhone" state. Layer-4 bridge → WiFi-cost network. Capabilities → photos, locations, biometric auth. Bare-metal WiFi → standalone operation. Every phase produces a video you could show someone.

---

## What this roadmap deliberately does NOT include

- **Web browser.** Deferred. Too big for the next year.
- **Package manager / crates.io install.** Same — deferred.
- **Video playback / games.** Same — deferred.
- **Linux compatibility layer.** Forbidden by the from-scratch commitment.
- **Multiple-user support.** Single-user OS. Each device pairs with one phone at a time.
- **Cloud sync of state.** All state local; phone holds keys for any remote operations.
- **ethernet driver.** No ethernet at the current work location, so no reason to build it. If circumstances change, can be added later.

---

## Order of work (the practical schedule)

**Now → next 1-2 weeks:** Finish Stage F12 cleanly (rustc_lint already closed — bring along borrowck and any tail crates).

**Next 1-2 weeks:** Stage F13 (cg_clif wiring) + DEMO 80. Phase 14 closes.

**Then 3-4 weeks:** Phase 15 (USB tethering on W540). First internet on bare metal. Milestone achieved.

**Then 4-6 weeks:** Phase 16 (pairing protocol + Expo bridge app). Bridge prototype works.

**Then 4-6 weeks:** Phase 17 (Layer-4 bridge). Daily-use networking via WiFi, no cellular burn. Authentication-via-pairing formalized.

**Then ongoing:** Phase 18 capabilities, picked by need. Phase 20 (WiFi) runs in parallel from this point if there's interest.

**When Mac arrives (likely Q3-Q4 2026):** Phase 19 begins. Native Swift bridge rewrite. LegiView iOS app development becomes possible in parallel.

**Calendar estimate to "useful Semantic OS on the W540 via Expo bridge":** 4-5 months from rustc_lint close. That gets you to: pair phone → WiFi-based internet via bridge → agent loop and OS capabilities available.

**Calendar estimate to "Semantic OS shippable to other users":** Add Phase 19 after Mac arrives. Probably late 2026 or early 2027 for first distributable build.

---

## What stays in your head, not on this list

The big things that aren't milestones:

- **Sustainable pace.** Pacing for 10 years. Days off. Slow-pitch nights. Summer job that pays the bills. Supply teaching days.
- **Don't push for compressed timelines under pressure.** The pattern of "the long timeline feels intolerable so let me make it shorter" — that's the thing to notice and resist.
- **One thing at a time, mostly.** Phase 14 close → Phase 15 → etc. Phase 20 (WiFi) is allowed to be parallel because it's background work.
- **The OS is for you first.** The Phase 19 production rewrite and distribution to other users is downstream of "the OS is useful enough that I use it daily." If you never get there, that's fine — the OS still exists, the work still has value.
- **The Mac purchase is a soft milestone.** It enables Phase 19 and LegiView native. Don't force it; let it happen when summer income clears or when there's a clear reason.

---

## Living document

Update when milestones close. When something turns out harder than expected, note it. When a capability gets added, document the protocol message. The roadmap is a guide, not a contract.
