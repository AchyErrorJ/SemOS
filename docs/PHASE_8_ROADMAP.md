# Phase 8 Roadmap — From QEMU-bound Kernel to "Remote LLM Call from Bare Metal"

**Date:** 2026-05-15
**Inputs:** three agent scoping briefs from 2026-05-15 (AX211/iwlwifi, xHCI+USB HID, Minimal TLS 1.3 client).
**Status of inputs:** complete; archived in agent task outputs.

This document is the synthesised roadmap. It does not repeat the briefs; refer back to them for the per-component depth.

---

## Goal

End state: the kernel boots on the ThinkPad P1 Gen 6 (or QEMU), associates with a Wi-Fi network over the AX211 NIC, opens a TCP connection to `api.anthropic.com:443`, completes a TLS 1.3 handshake against a pinned intermediate CA, sends a chat-completion HTTP request through the existing `NetworkLlmProvider`, and surfaces the assistant's reply through `SYS_LLM_STREAM_READ`.

What changes from today: everything in the layer cake below that's currently `❌` becomes `✅`.

---

## Layer cake (current state)

```
    APP LAYER         user-program calls SYS_LLM_STREAM_START
                                       │
                      LlmProvider queue ───► remote_process_prompt   ✅
                                       │
                      NetworkLlmProvider ────► complete()             ✅
                                       │
                      HTTP/1.1 framing + JSON parse                   ✅
                                       │
    TRANSPORT         NetworkTransport (trait, pluggable)             ✅
                                       │
                      ┌──────────────────────────────┐
                      │ TLS 1.3 client (pinned cert) │                ❌
                      └──────────────────────────────┘
                                       │
    L4                TCP                                              ❌
    L3                IPv4 + ARP                                       ❌
    L2/Wi-Fi          802.11 MAC + WPA2-PSK 4-way handshake            ❌
    LINK DRIVER       iwlwifi (AX211)                                  ❌
                      MSI-X (prerequisite for iwlwifi + NVMe)          ❌

    USB (parallel)    xHCI controller + enumeration + HID kbd          ❌

    SIDE QUESTS       VT-d / IOMMU handling                            ❌
                      RTC driver (only required if TLS notAfter)       ❌
                      Framebuffer scrollback                           🔨

    PHASE 7 GIVENS    ChaCha20-Poly1305, semantic objects, identity,
                      LLM provider, loopback transport, mock peer      ✅
```

Today's loopback path proves the upper half (HTTP framing, JSON parsing, provider queue, syscall ABI) end-to-end. Phase 8 is the lower half plus TLS, with USB as a parallel deliverable that unblocks interactive demos on metal.

---

## Dependency graph

The good news: most of Phase 8 is parallelisable. The bad news: TLS is the longest single linear task and it gates the end-state demo.

```
xHCI ─────────────────────────────────────────────►  (independent; unblocks input on metal)

MSI-X ──► iwlwifi ──► 802.11 MAC ──► WPA2 ────────┐
                                                  ▼
                                            ARP+IPv4+TCP ──► TLS 1.3 ──► (provider wires in)
                                                  ▲
virtio-net (already-easy QEMU path) ──────────────┘
```

Key insight: **the network stack (ARP + IPv4 + TCP + TLS) can be developed entirely against virtio-net in QEMU, in parallel with iwlwifi.** When the Wi-Fi driver is ready, swap the link layer underneath. virtio-net is structurally similar to virtio-block (which already works), so it's a much shorter path to "TCP working" than waiting for iwlwifi to come up on metal.

---

## Three parallel tracks

### Track A — USB stack (xHCI + HID keyboard)

**Why it's independent:** doesn't share infrastructure with the network path. Doesn't gate it either.
**Why it matters:** without keyboard input there's no interactive demo on metal. Also a prerequisite for any future USB Ethernet fallback if Wi-Fi proves intractable.
**Scope (from brief):** ~3000 LOC, 8-12 weeks part-time.
**Test path:** QEMU `-device qemu-xhci -device usb-kbd` end-to-end. ~80% of the work is QEMU-testable. The remaining 20% (BIOS handoff, scratchpad buffer counts, Intel-specific quirks) is metal-only.
**Approach:** vendor the `xhci` crate for the register layer; hand-write everything above. Polling mode v1 (no MSI-X). See `kernel-core/src/llm/net_provider.rs` for the pattern of "vendor the small layer, hand-write the integration."
**First milestone:** "kernel receives a USB keypress event in `INPUT_RING`."

### Track B — Network stack on virtio-net (ARP + IPv4 + TCP + TLS)

**Why it's the smart development path:** virtio-net in QEMU works today (you already have the virtio infrastructure). Lets you build, test, and validate the entire upper network stack — including TLS against real HTTPS servers — without waiting for iwlwifi. When Wi-Fi comes online, replace the link layer.

**Sub-sequence:**

1. **virtio-net driver** — ~500 LOC. Copy the virtio-block pattern: PCIe enum, queue setup, packet TX/RX. Adds NetDevice trait impl, registers as `"virtio-net0"`. **First milestone:** "kernel receives an Ethernet frame in QEMU."

2. **ARP + IPv4** — ~800-1200 LOC combined. ARP table (~16 entries), IPv4 framing with header checksum, fragment reassembly (or hard-fail on fragmented input for v1). **First milestone:** "kernel sends an ICMP echo and gets a reply."

3. **TCP** — ~1500-2000 LOC. State machine (CLOSED → SYN_SENT → ESTABLISHED → … → CLOSED), sequence numbers, sliding window, retransmission, RTT estimation. The sliding-window correctness is the hard part. **First milestone:** "kernel opens a TCP connection to a known server and receives bytes."

4. **DNS (or skip)** — agent brief recommends hardcoding the IP for v1. If you take that, ~0 LOC. If you implement a minimal DNS A-record resolver, ~400 LOC.

5. **DHCP (or skip)** — similar. Hardcode IP + gateway + DNS server for v1; implement DHCP later. ~0 LOC if hardcoded.

6. **TLS 1.3 client** — ~5300 LOC including crypto. **The single largest item in Phase 8 and the most likely to dominate the schedule.** Strategy from brief:
   - Vendor `embedded-tls` as a starting point.
   - Replace its crypto deps with in-kernel ChaCha20-Poly1305 + hand-rolled SHA-256 + X25519 + ECDSA-P256-verify (~1500 LOC of crypto extension to existing `kernel-core/src/crypto/`).
   - Replace its X.509 with a ~650 LOC pinned-SPKI scanner (no full ASN.1 parser).
   - Skip `notAfter` validation for v1 (defer to Phase 9 RTC driver).
   - Cipher suite: `TLS_CHACHA20_POLY1305_SHA256` (already have the AEAD).
   - **Discipline: run RFC 8448 test vectors against the key schedule BEFORE touching the network.** This single practice catches ~80% of from-scratch TLS bugs.

7. **Wire into NetworkLlmProvider** — add a `TcpTransport` impl of the `NetworkTransport` trait, configure the global net provider to use it instead of loopback. ~1 day.

**Scope summary:** ~9000-10000 LOC across the whole track. TLS is the bulk.
**Critical pre-step:** none — virtio-net is the simplest first move.
**First end-to-end milestone:** "kernel boots in QEMU with -netdev user, opens TCP to api.anthropic.com:443, completes TLS handshake, sends HTTP request, parses JSON response, surfaces it through SYS_LLM_STREAM_READ."

Once that works in QEMU, the upper stack is proven. iwlwifi (Track C) then only has to make the link layer functional on metal.

### Track C — iwlwifi (AX211) driver

**Why it's separate:** ONLY testable on metal (QEMU does not emulate iwlwifi — agent brief corrected an earlier wrong assumption). Independent of Tracks A and B.
**Scope (from brief):** ~6600 LOC, 5-7 months part-time.
**Pre-requisite:** MSI-X support in the kernel. Per agent brief, iwlwifi context-info-gen3 devices effectively require MSI-X. Plain MSI fails to ALIVE on modern firmware. So MSI-X is a Track C side quest (~150-300 LOC) before the driver itself.
**Sub-sequence:**

1. **MSI-X support** — ~200 LOC. Parse the MSI-X capability in PCI config space, map the table, write LAPIC-targeted entries, mask/unmask. Side quest, also needed for NVMe later.
2. **Firmware blob embedding** — ~400 LOC including TLV parser. Two files needed: `iwlwifi-so-a0-gf-a0-N.ucode` and `iwlwifi-so-a0-gf-a0.pnvm`. Both Intel-license-redistributable; ship a `LICENCE.iwlwifi_firmware` alongside.
3. **PCIe + MSI-X bring-up** — ~600 LOC. BAR mapping, capability traversal, MSI-X table programming, DMA-coherent buffer setup.
4. **Context info gen3 + firmware load + ALIVE** — ~1200 LOC. **Biggest single hurdle in Track C.** Without serial output or framebuffer scrollback, debugging an ALIVE failure on metal is brutal.
5. **Command/notification ABI + ring infra** — ~800 LOC. Once ALIVE arrives, this is the round-trip path for every later command.
6. **802.11 frame builders/parsers + auth/assoc + WPA2 4-way handshake** — ~2200 LOC. WPA needs HMAC-SHA1, PBKDF2-SHA1, AES Key Wrap (RFC 3394). Plumbing into the same crypto module as TLS.
7. **TX/RX data path with gen2 TFDs + RB pool** — ~900 LOC. Once this works the link is up and ARP/IPv4 from Track B run over it.
8. **Integration** — replace virtio-net's role in the network stack with iwlwifi. ~1 day if Track B was designed against the `NetDevice` trait.

**Critical pitfalls (from brief):**
- VT-d / IOMMU enabled by default in ThinkPad BIOS. DMA fails silently or traps if ignored. Disable in BIOS or implement identity-IOMMU.
- PNVM missing causes silent TX drops that look like upper-layer bugs.
- Set up a debug path BEFORE writing line one (framebuffer scrollback minimum).
- Use 5 GHz channel 36/40/44/48 for v1 — skip 6 GHz / Wi-Fi 6E to avoid regulatory complexity.

**First milestone:** "ALIVE notification received from the AX211 firmware on metal." (Everything else builds on this.)

---

## Cross-cutting prerequisites

These aren't in any single brief but are required for Phase 8 to land:

1. **MSI-X** — Track C requires it; NVMe (Phase 9+) will too. Track A doesn't need it. Build during Track C.
2. **VT-d / IOMMU handling** — affects any DMA-using device. Easiest: disable in BIOS. Real solution: parse ACPI DMAR table, program identity mappings. Defer to "needed for metal bring-up" phase.
3. **Framebuffer scrollback ring** — was in-progress per uncommitted +95 lines in `kernel-x86_64/src/framebuffer.rs` as of 2026-05-13. Verify with `git log`. Required for any metal debugging where the kernel scrolls past the screen.
4. **RTC driver / wall clock** — only required if TLS does `notAfter` validation. Per brief, skip for v1 and defer to Phase 9. Pinned-SPKI is the substitute.
5. **A no_std JSON parser** — already needed by Brise/Claw Pen apps per `project_semantic_os_app_requirements`. Hand-roll ~300 LOC; do not pull in serde. Probably falls out of app work, not kernel work.

---

## What's QEMU-testable vs metal-only

| Component | QEMU | Metal-only |
|---|---|---|
| xHCI + USB HID kbd | ~80% (qemu-xhci is faithful) | BIOS handoff, scratchpad count, Intel quirks |
| virtio-net driver | 100% | — |
| ARP + IPv4 + TCP | 100% against virtio-net + Linux peer | — |
| DHCP + DNS (if implemented) | 100% with QEMU `-netdev user` | — |
| TLS 1.3 crypto + key schedule | 100% via RFC 8448 vectors as unit tests | — |
| TLS 1.3 against real server | yes, with QEMU usermode networking | — |
| iwlwifi (AX211) | 0% (no emulation) | 100% |
| 802.11 frame builders/parsers | 100% as unit tests with captured packets | — |
| WPA2 handshake logic | 100% as unit tests | Real-AP timing edge cases only |

**Implication:** the entire network stack including TLS can be brought all the way to "remote LLM call completes" in QEMU before any metal Wi-Fi work. That's the lowest-risk path to validating Phase 7's design.

---

## Risk inventory (top 5)

1. **TLS X.509 parsing** — historically the largest source of TLS CVEs. The brief recommends a deliberately-limited DER scanner over a general ASN.1 parser, plus SPKI pinning. Stick to that. Anything that says "we'll need a real ASN.1 parser later" is a red flag.

2. **iwlwifi ALIVE on metal** — first wall of Track C. Without a debug path, you'll lose days. Frame buffer scrollback is the minimum; USB-serial requires the USB stack (which is Track A, parallel — fine if it's ready first).

3. **TCP correctness** — sliding window, retransmission, RTT, fast retransmit. Easy to write something that "kinda works" against a forgiving peer and fails against real-world conditions. Test against multiple peers (Linux, OpenBSD, the Anthropic edge directly).

4. **VT-d** — silent DMA failures. Confirm BIOS state on the P1 Gen 6 before assuming any DMA-using driver will work.

5. **Crypto from scratch** — X25519 and ECDSA-P256 implemented from scratch by a non-cryptographer are CVE bait. The brief recommends Monocypher-style ~250 LOC X25519 and treating it as deeply unit-tested. If you find yourself "optimising" any crypto primitive, stop.

---

## Suggested first concrete step

Given the dependency graph, the highest-leverage starting point depends on which kind of work you'd rather block-out time for. Three honest options:

- **Crypto extension** (smallest, most isolated): add SHA-256, HMAC, HKDF, X25519, ECDSA-P256 to `kernel-core/src/crypto/`. All testable as pure-Rust unit tests with no kernel changes. RFC 8448 test-vector harness in `cargo test`. This is the cheapest first move toward TLS, and it's risk-free because everything is offline-verifiable.

- **virtio-net driver**: ~500 LOC, structurally similar to virtio-block. Unblocks the whole network track in QEMU.

- **xHCI**: independent track, completely parallel. ~2-4 weeks to first keypress in QEMU.

If forced to pick one: **crypto extension first**. It's the smallest, lowest-risk, immediately useful (RFC 8448 vectors give exact correctness criteria), and it's a strict prerequisite for the schedule-dominating item (TLS). Then virtio-net to start the network track. xHCI in parallel whenever you want input on metal.

---

## Open questions surfaced by the briefs

- ~~Confirm Anthropic's leaf cert signature algorithm~~ **— RESOLVED 2026-05-16.** `openssl s_client` showed leaf = ECDSA P-256 / SHA-256, TLS handshake CertificateVerify = `ecdsa_secp256r1_sha256`. **RSA-PSS NOT needed.** Pin the intermediate (WE1) SPKI: SHA-256 = `908769e8d34477cc2cba0632c88605b22d7294c0840f78596d247c645b1afc0e`. Intermediate valid through Feb 20, 2029. Detail in `EMBEDDED_TLS_VENDORING_BRIEF.md` §10.
- **MAC address source for AX211** — comes from OTP via `NVM_ACCESS_CMD`. Decision: read from OTP or hardcode a locally-administered MAC for v1?
- **AP RSN capabilities for the target Wi-Fi network** — does it require PMF (802.11w)? If yes, +500 LOC for IGTK + management-frame protection.
- **VT-d BIOS state on the target machine** — verify on Linux first (`dmesg | grep -i dmar`). Determines whether IOMMU code is on Track C critical path.
- **Polling vs interrupt-driven xHCI** — polling is simpler for v1 (no MSI-X dependency). Decide whether to revisit when MSI-X exists for Track C.

---

## References

Full agent briefs (in agent task output files; one-shot, may not be re-readable in future sessions — copy into kernel docs/ if you want them preserved):

- AX211 / iwlwifi scoping brief — 2026-05-15, ~3000 words
- xHCI + USB HID scoping brief — 2026-05-15, ~3000 words
- Minimal TLS 1.3 client survey — 2026-05-15, ~4500 words

Related kernel/project docs:

- `F:\Software\ArmKernel3\docs\architecture.md` — kernel architecture
- `C:\Users\jerro\.claude\projects\F--Software-ArmKernel3\memory\project_semantic_os_kernel.md` — kernel project memory (Phase 7 done, Phase 8/9 planned)
- `C:\Users\jerro\.claude\projects\F--Software-ArmKernel3\memory\project_semantic_os_app_requirements.md` — what apps need from the kernel; informs WHY each Phase 8 piece matters
