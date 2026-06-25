# Roadmap — Networking & Online

> Part of the [Master Roadmap](../MASTER_ROADMAP.md). Sibling themes:
> [self-extension](map%20-%20self-extension.md) · [phone](map%20-%20phone.md) · [gpu](map%20-%20gpu.md) ·
> [platform](map%20-%20platform.md). Historical "what landed" log: [ROADMAP.md](../ROADMAP.md).

Getting SemOS **online on real silicon**, then making that connection
independent. Order locked: USB tether (milestone-proving) → Layer-4 phone bridge
(daily-use, no cellular cost) → bare-metal WiFi (untethered). The phone-bridge
*capabilities* live in [phone.md](map%20-%20phone.md); this file owns the network transport.

**Architectural commitments:** phone connects to the world over WiFi when
available, cellular as fallback — the OS sees one logical channel. From-scratch on
the OS side (no POSIX, no libc); firmware blobs (Intel WiFi) are the declared
exception, shipped with attribution.

---

## Phase 15 — USB Tethering (the "online on bare metal" moment) — substantially DONE

**Goal:** phone plugs in over USB, kernel sees a network device, the agent loop
calls the Anthropic API over a real wire.

> **STATUS 2026-06-10 — landed via a different route than drafted.** Reality on
> the W540 diverged, all now in tree (`d11a63e`..`eef5a1f`):
> 1. **iPhone tethering is ipheth, not CDC-ECM** (Apple vendor class
>    0xFF/0xFD/0x01 at alt 1, hidden in USB config 4 — hosts must iterate configs).
>    CDC-ECM driver still exists (DEMO 81) for dongles.
> 2. **Transport is a standalone EHCI driver, not xHCI** — the Lynx Point xHCI
>    never completed a USB-2 port reset; `usb/ehci.rs` owns the USB-2 ports
>    Windows-7-style (per-controller async schedules, QH/qTD control+bulk,
>    Rate-Matching-Hub enum, split transactions, persistent bulk-IN RX QH).
> 3. **DHCP bypassed** — the iPhone tether subnet is fixed (172.20.10.0/28,
>    phone = gateway+DNS), so `net::init_with_ipconfig` statically configures
>    172.20.10.9/28.
>
> Hardware-confirmed: iPhone enumerates → Trust prompt → MAC + carrier 0x04.
> `ipheth0` NetDevice + smoltcp bring-up landed. **Traffic over the tether not
> yet validated on hardware** — next move, then M53/M54 go live.

### M50 — USB device-class network driver `[x — ipheth-over-EHCI 2026-06-10; CDC-ECM variant in tree]`
Recognize a USB network device, enumerate endpoints, expose as a kernel interface.
- [x] USB-class-2 (Communications) device recognized on insertion
- [x] control endpoint parsed (interface descriptors, MAC address)
- [x] data endpoints bound (bulk-in receive, bulk-out transmit)
- [x] `net_interface_register()` called
- [x] DEMO 81: kernel logs device detected, MAC, `usb0` up

### M51 — RNDIS driver `[deferred — no Android requirement on the table]`
Android tethering path (USB 0xE0/0x01/0x03 + management plane, ~1500-2000 LOC).
INIT exchange, OID queries, packet framing on bulk endpoints. DEMO 82.

### M52 — DHCP client `[bypassed 2026-06-10 — tether subnet fixed; optional nicety for non-tether interfaces]`
DHCPDISCOVER → OFFER parse → REQUEST/ACK → apply IP/gw/DNS, T1 renewal. RFC 2131,
~200-300 LOC UDP, v4 client only. DEMO 83.

### M53 — Real-world TLS validation `[NEXT — unblocked once tether traffic validates]`
Hardening pass against real Anthropic servers (cert paths, extensions, edge cases
QEMU never sees).
- [ ] real Anthropic cert chain validates on bare metal
- [ ] SNI sent for `api.anthropic.com`; TLS 1.3 handshake completes
- [ ] HTTP/2 negotiation works or cleanly falls back to HTTP/1.1
- [ ] DEMO 84: full agent turn on bare metal over real network, no QEMU

### M54 — The first usable session `[NEXT after M53]`
- [ ] boot → plug phone → `sem-sh` → `agent` → "time in Tokyo?" → correct answer
- [ ] DEMO 85: ~5-minute video of the full session

**Daily-use note:** Phase 15 proves the architecture end-to-end but burns cellular
data even on WiFi (an iOS limitation). Phase 17 (Layer-4 bridge) is the fix.

---

## Phase 17 — Layer-4 Phone Bridge (cellular cost solved)

**Goal:** replace USB tethering for daily use. The companion app forwards
socket-level operations through iOS networking (WiFi-preferred, cellular
fallback). TLS still runs *in SemOS* — phone handles transport, OS handles crypto.
**Pairing IS authentication** once this is the only network path.

**Why Layer-4 not Layer-3:** L3 needs iOS Network Extension entitlements;
L4 works with standard app capabilities (runs on Expo) and shrinks the kernel
surface (no retransmit/congestion/routing in-OS). Depends on Phase 16 pairing
([phone.md](map%20-%20phone.md)).

### M58 — Layer-4 RPC protocol `[  ]`
`dns_resolve`, `tcp_connect`, `tcp_send`, `tcp_recv`, `tcp_close`, `udp_send`,
`udp_recv` over the paired TLS channel.
- [ ] protocol doc `docs/network-bridge-v1.md`, wire format, concurrent-socket framing
- [ ] async semantics (multiple in-flight), test vectors

### M59 — Kernel socket abstraction `[  ]`
- [ ] `socket(AF_INET, SOCK_STREAM, 0)` returns a bridge-backed socket
- [ ] standard send/recv/close over the bridge; TLS runs unmodified on top
- [ ] mode switch: local TCP/IP (QEMU) vs bridge (daily use)
- [ ] DEMO 88: agent loop over bridge, phone's WiFi, no cellular burn

### M60 — Bridge app gains socket forwarding `[  ]`
App translates RPC → iOS `NWConnection`/`react-native-tcp-socket`; DNS via iOS
APIs; buffered send/recv; concurrent connections; background networking mode.
- [ ] DEMO 89: 100 concurrent HTTPS requests via bridge, all succeed on WiFi

### M61 — Authentication-via-pairing formalized `[  ]`
- [ ] boot flow: no phone = degraded (local only); phone reachable = full operation
- [ ] short-lived token grace period (5-15 min) when phone briefly unavailable
- [ ] no password storage anywhere in SemOS

---

## Phase 20 — Bare-Metal WiFi (Intel 7260) — ACTIVE MAIN TRACK

**Goal:** SemOS connects to WiFi directly, no phone required for network access.
Start with one chip — Intel Wireless 7260 (PCI `8086:08B1`) in the W540/T540p.
WiFi from scratch is 3-6 months; it runs as a long-lived track. Depends only on a
working compiler.

> **STATUS 2026-06-24.** Replacement boot drives in hand; the T540p boots again.
> A live `wifi connect` re-confirmed **Phase A on metal** (PHY ctx + MAC ctx +
> binding + ADD_STA status=1 + time-event all HW-confirmed) and pinned the
> Phase-B blocker precisely: the open-auth TFD is built and the doorbell rings
> (`SCD_wrptr=1`) but **data queue 1 never activates** — global `SCD_ACTIVE` q1
> bit stays 0 even though the per-queue `QUEUE_STATUS` active bit (bit3) reads
> set, so `SCD_rdptr` stays 0 (`consumed=0`). An instrumented activation probe
> (explicit `SCD_ACTIVE` write+readback, then an 8-pass re-assert/poll loop) was
> added to `enable_data_queue` to decide host-writable vs firmware-driven —
> pending the next boot. See the `semos-wifi-scd-q1-stuck` memory for the
> blow-by-blow and the `SCD_GP_CTRL` write-mask probe as the fallback diagnostic.
> Attempts to date: OpenBSD-style `iwm_enable_ac_txq` direct-register setup,
> `SCD_ACT_EN` at bit 19, setting `SCD_ACTIVE`/`EN_CTRL`, `TX_CMD` on data queue 1
> (not host queue 0), `SCD_QUEUE_CFG` (0x1d) binding queue 1 to sta 0 / TID 8 /
> FIFO BE (response echoes q1). NOT QEMU-testable.

### M72 — WiFi chip enumeration + firmware load `[x — DONE 2026-06-13]`
- [x] PCI 7260 detected, firmware blob loaded, chip alive via mailbox
- [x] DEMO 96: "Intel Wireless 7260 detected and initialized"

### M73 — 802.11 management state machine `[x — scan + Phase-A join DONE 2026-06-15; on-air assoc pending Phase-B TX]`
- [x] `wifi_scan()` returns nearby networks (live, real SSIDs)
- [x] `wifi` / `wifi connect <n> <pass>` shell commands
- [ ] on-air association (blocked on Phase-B frame TX)

### M74 — WPA2 / WPA3 authentication `[🔨 — PMK/PTK/EAPOL-MIC + RSN IE built & KAT'd 2026-06-15; 4-way blocked on Phase-B TX]`
PBKDF2 + AES-CCMP for WPA2-PSK; open-auth, assoc-req w/ RSN IE, EAPOL-Key RX parse
and Msg2/Msg4 TX all implemented and wired into `connect()`. WPA3-SAE deferred.
- [ ] WPA2-PSK 4-way completes (gated on `consumed=1`)
- [ ] encrypted data frames send/receive

### M75 — WiFi as primary network interface `[  ]`
- [ ] interface priority: WiFi if connected, phone bridge if not; shell override
- [ ] DEMO 98: boot without phone, join WiFi, agent loop works

---

## Deliberately NOT in this theme
- Ethernet driver — no ethernet reach at the work location; add only if that changes.
- Cloud sync of state — all state local; phone holds keys for remote ops.
