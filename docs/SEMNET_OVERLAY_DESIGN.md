# SemNet — SemOS joins the tailnet (design)

**Status:** draft for review · 2026-08-23
**Companion to:** `docs/pairing-v1.md` (identity), `docs/EMBEDDED_TLS_VENDORING_BRIEF.md` (TLS client), `docs/SMOLTCP_VENDORING_BRIEF.md` (TCP/IP stack), `docs/semos-security-thesis.md` (vouch model)

---

## 1. Plain-language summary

We want the T540p (and future SemOS machines) to join the user's **existing
Tailscale network** — the private mesh that already connects their Windows box,
Mac, and other devices — so a SemOS machine can reach them, and be reached,
from anywhere.

We are **not** building a separate SemOS-only overlay. SemOS becomes a normal
member ("node") of the existing tailnet, speaking the same protocols every
other node speaks. Tailscale (the hosted coordination service) keeps doing
what it's good at — key distribution, endpoint discovery, NAT traversal —
and SemOS implements a minimal, read-mostly node client.

Joining decomposes into exactly two conversations:

1. **"Hi, can I join?" (control plane).** SemOS authenticates to
   `login.tailscale.com` with a pre-generated **auth key** (no interactive
   OAuth — a tagged, reusable key created once in the admin console), then
   long-polls for the **netmap**: the list of peers, their WireGuard public
   keys, and their current endpoints.
2. **Encrypted chat (data plane).** Using the netmap, SemOS speaks stock
   **WireGuard** over UDP to the other nodes. All cryptographic primitives
   WireGuard needs are already vendored in `kernel-core::crypto` (built for
   the M55 phone-pairing stack).

What rides on top once connected: the voice-assistant pipeline, `netlog` from
anywhere (not just the LAN), remote install-approval for the self-dev loop
(phone approves over the tailnet instead of a human at the keyboard), and
ordinary DNS/HTTP between nodes.

## 2. Why join instead of build

| | Join existing tailnet | SemOS-native overlay |
|---|---|---|
| New infrastructure | none | rendezvous/relay service to build + host |
| Reaches existing devices | yes (the point) | no — separate network |
| Coordination maturity | Tailscale's, battle-tested | ours, greenfield |
| Crypto to implement | same either way (WireGuard) | same |
| Control-plane code | reverse from Tailscale's OSS Go client | design our own |
| Protocol-drift maintenance | some (versioned protocol) | none |

The WireGuard data plane — the hard, security-critical half — is identical in
both columns. The only extra cost of joining is the control-plane client,
which is bounded and read-mostly. Escape hatch if Tailscale-hosted ever
becomes a problem: **Headscale** (open-source coordination server we host
ourselves) — same client code, different base URL.

## 3. Existing SemOS inventory (what we reuse)

| Need | Have | Where |
|---|---|---|
| X25519 key agreement | ✅ | `kernel-core/src/crypto/x25519.rs` |
| ChaCha20 / Poly1305 AEAD | ✅ | `kernel-core/src/crypto/` (pairing stack) |
| HKDF | ✅ | `kernel-core/src/crypto/` |
| BLAKE2s | ✅ (verify coverage) | `kernel-core/src/crypto/` |
| TCP/IP + UDP stack | ✅ | smoltcp (vendored) |
| NIC driver | ✅ | e1000e, validated on T540p (2026-07-20) |
| TLS 1.3 client | ✅ | embedded-tls; validated end-to-end → Anthropic from the T540p |
| Node identity model | ✅ (design) | pairing-v1: paired phone = user identity; node keys = semantic UIDs |
| UDP send/recv in anger | ✅ | `netlog` (validated e2e 2026-08-17) |

Gaps to build: WireGuard protocol state machine, a virtual network interface
+routing for the overlay, the Tailscale control client, STUN/DERP clients.

## 4. Architecture

```
┌──────────────────────────── SemOS node ───────────────────────────┐
│  sem-sh / voice pipeline / netlog / self-dev approval             │
│         │                                                         │
│  ┌──────┴───────┐        netmap (peers, keys, endpoints)          │
│  │ tailnet ctl  │ ◄──────────────┐                                │
│  │ client       │  HTTPS (embedded-tls) + Noise machine-key chan  │
│  └──────┬───────┘                │                                │
│         │ peer table             ▼                                │
│  ┌──────┴───────┐        login.tailscale.com (coordination)       │
│  │ WireGuard    │                                                │
│  │ data plane   │ ◄═══ encrypted UDP ═══► other tailnet nodes    │
│  └──────┬───────┘        (or via DERP relay over HTTPS)          │
│  smoltcp (UDP sockets) / virtual iface                            │
│  e1000e                                                           │
└───────────────────────────────────────────────────────────────────┘
```

**Identity and keys.** Each SemOS node holds a persistent **machine key**
(X25519 pair, generated at provisioning, stored tier-protected on the
namespace — secrets policy per the threat-model doc). Registration trades the
auth key + machine key for a **node key**; the netmap is keyed to that. A
**tagged, reusable auth key** (e.g. `tag:semos`) is used so nodes never
expire and never need interactive login.

**Two-layer trust story.** Tailnet membership = *connectivity* (who can route
to me). The SemOS vouch chain = *authorization* (what a peer may ask me to
do). The self-dev approval gate and any privileged syscalls keep requiring
vouch-signed requests even over the tailnet — membership alone grants
nothing beyond reachability.

**NAT traversal.** v1 sends and receives through a **DERP relay** (Tailscale's
relays, over HTTPS) — always works, adds latency. v2 adds STUN probing and
direct UDP paths (fast path) with DERP as fallback.

**DNS.** MagicDNS is ordinary DNS served over the tunnel; resolve tailnet
names via the tailnet DNS resolver once the overlay is up.

## 5. Milestones

### S1 — WireGuard data plane (no Tailscale yet)
UDP sockets over smoltcp; Noise_IK handshake; transport encrypt/decrypt;
trivial virtual interface. **Interop test against a stock Linux `wg` peer on
the LAN** — if Linux `wg` can exchange ping/UDP with SemOS, the crypto is
right. Validate against published WireGuard test vectors.
*Acceptance: `ping`-equivalent + UDP echo both ways SemOS ↔ Linux `wg`.*

### S2 — Control client: join the real tailnet
Auth-key registration against `login.tailscale.com`; machine-key Noise
channel (ts2021); netmap fetch + long-poll; netmap → WireGuard peer table.
DERP-only transport.
*Acceptance: from another tailnet device, `ping <semos-node-tailnet-IP>`;
SemOS reaches a tailnet peer (e.g. HTTP fetch from the dev box over the
tailnet).*

### S3 — Direct paths
STUN endpoint discovery, direct UDP send/receive, roaming (endpoint change
→ session keepalive + re-derive), DERP kept as fallback.
*Acceptance: same S2 flows with relays disabled on the far end; netlog over
tailnet from off-LAN.*

### S4 — SemOS integration
`netlog` and the self-dev **approval gate** ride the tailnet; phone-initiated
approval over the overlay signed by the pairing vouch chain (builds on
M55/M56); DNS via MagicDNS; persistent node config in the namespace
(survives reboot once SYS_PERSIST lands).
*Acceptance: DEMO 88-style approval answered from the paired phone off-LAN.*

### Non-goals (for now)
Exit-node / subnet-router duty, Tailscale SSH server, serving MagicDNS,
Funnel. Client-node only.

## 6. Risks & open questions

- **Control-protocol drift.** The ts2021 control protocol is versioned and
  evolves; a minimal client needs occasional catch-up. Mitigation: implement
  the smallest read-mostly subset; Headscale as fallback coordination.
- **DERP transport details.** DERP runs a WebSocket-flavored upgrade over
  HTTPS — embedded-tls is client-only TLS 1.3 (fine), but the WS framing
  layer is new code (small).
- **Auth-key handling.** The reusable auth key is a bearer secret; store it
  tier-protected, never in the repo, rotate if a disk image leaks. Tagged
  nodes avoid re-auth but a leaked key allows rogue nodes → keep ACLs tight
  (`tag:semos` can only reach what it needs).
- **Current tailnet health.** As of 2026-08-23 the Windows box's Tailscale
  interface showed an APIPA address (tailnet apparently down from here).
  Verify `tailscale status` on a healthy device before S2 testing.
- **BLAKE2s coverage.** Confirm the vendored hash covers BLAKE2s (WireGuard
  KDF/chaining) and not only SHA-256; add if missing (small, well-specified).
- **Time.** WireGuard rates/rekeys depend on monotonic time — SemOS has TSC-
  based ticks; ensure a wall-clock-ish source for replay windows and rekey
  timers.

## 7. What this unlocks

- T540p voice assistant reachable from any of the user's devices, anywhere.
- `netlog 100.x.y.z` from off-LAN — metal debugging without the house.
- Self-dev loop (M1–M4) approvals from the phone, not the keyboard.
- A template for SemOS↔SemOS clustering later (same data plane, plus the
  vouch layer on top).
