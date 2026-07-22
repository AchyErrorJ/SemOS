# M53 — Real-World TLS Validation (T540p, native Ethernet)

**Prepared:** 2026-07-21
**Status:** READY TO RUN — no code changes identified as required; this is a
hardware validation pass over an already-wired path.
**Target:** Lenovo T540p, Intel I217-LM (`e1000e0`), same cable/router as the
2026-07-21 Ethernet validation.
**Depends on:** [`ETHERNET_T540P_VALIDATION_2026-07-20.md`](ETHERNET_T540P_VALIDATION_2026-07-20.md)
(DHCP/DNS/TCP/HTTP already proven on this NIC).

## Why this is "just run it"

The TLS-to-Anthropic path was built and boot-tested against QEMU SLIRP (DEMO
16/48/49, M22), and none of it is QEMU-specific:

- `kernel_core::net::resolve()` does real DNS (already proven working over
  `e1000e0` — `fetch http://example.com/` resolved and connected on 2026-07-21).
- `agent::send_over_tls` / `agent::Session` call `net::resolve("api.anthropic.com")`
  with a hardcoded fallback IP (`160.79.104.10`) only used if DNS fails —
  see `kernel-x86_64/src/agent.rs:439,520`.
- `TlsTransport` (`kernel-core/src/tls/transport_tls.rs`) is built on the same
  `TcpStream`/smoltcp stack already proven over `e1000e0`; it has no NIC-specific
  code.
- The SPKI pin (`kernel-core/src/tls/spki_pin.rs`) targets the GTS `WE1`
  **intermediate**, valid through **2029-02-20** — not the leaf, so normal leaf
  cert rotation won't break it.
- DEMO 48/49 already run automatically at boot whenever `net::is_initialized()`
  is true (`kernel-x86_64/src/main.rs:1307-1328`) — no extra wiring needed to
  trigger the test, just boot with the cable plugged in.

What QEMU never exercised: a real internet path's RTT/jitter, real MTU/
fragmentation behavior, and whatever edge cases Anthropic's edge (Cloudflare in
front of GTS-issued certs) presents outside a SLIRP NAT. That's the actual
target of M53.

## Two-stage plan

### Stage 1 — keyless (DEMO 48): proves TLS itself

No `ANTHROPIC_KEY` baked in. Cheapest, lowest-risk first test — proves DNS +
TCP + TLS 1.3 handshake + cert-chain/SPKI-pin validation + HTTP framing all
work against the real service, without ever putting a real API key on the
machine.

```sh
bash tools/esp-install/build-and-flash.sh
```

(no `ANTHROPIC_KEY` env var set — default keyless build)

Boot with the Ethernet cable connected. Expected serial evidence:

```text
================================================================
  SemOS DEMO 48: agent live TLS round-trip (M22 stage B)
================================================================
  [DEMO 48] sending NNN-byte agent request over TLS (no key → expect 401)...
  [DEMO 48] received NNN bytes, HTTP status 401
  [DEMO 48] PASS: agent request reached Anthropic over TLS — 401 (auth) as expected, no key
```

If DEMO 48 instead prints `SKIPPED: transport error (...)`, that's a real
finding — capture the error string, it's the same triage class as the e1000e
TX bug (something in the handshake path chokes on real-world conditions QEMU
didn't produce).

### Stage 2 — keyed (DEMO 49): proves the full agent loop

Only after Stage 1 passes. Bake a real key in:

```sh
ANTHROPIC_KEY=sk-ant-... bash tools/esp-install/build-and-flash.sh
```

Expected serial evidence:

```text
================================================================
  SemOS DEMO 49: agent tool loop w/ live Claude (M22 stage C)
================================================================
  ...
  [DEMO 49] PASS: ...
```

This is also the core of **M54** ("first usable session") — if Stage 2 passes
cleanly with a representative real question, M54's acceptance gate is
essentially satisfied by the same boot.

## Failure triage

- **DEMO 48 `SKIPPED: transport error`** — inspect which layer: `netinfo`
  first (link/DHCP still healthy?), then `TlsTransport::last_tcp_state()` /
  `TlsTransport::last_handshake_error()` (exposed as diagnostics, not yet wired
  into `netinfo` — add a print if this triggers) to tell TCP-connect failure
  apart from a TLS handshake failure.
- **TCP connects, handshake fails** — check `TlsTransport::last_handshake_error()`
  for the embedded-tls `TlsError` variant. A cert-chain/pin mismatch here would
  mean Anthropic re-fronted behind a different intermediate since the
  2026-05-16 pin capture — re-run the `openssl s_client` capture in
  `spki_pin.rs`'s doc comment to check before assuming a bug.
- **Handshake succeeds, HTTP status not 401/parseable** — framing/parsing bug,
  same class as pre-existing DEMO 47 coverage; not network-specific.
- **Everything hangs past the ~30s idle timeout** — likely a real-RTT issue
  QEMU's SLIRP loopback never surfaced (e.g. `TCP_CONNECT_TIMEOUT_TICKS` /
  `IO_IDLE_TIMEOUT_TICKS` too tight for a real WAN path, or MTU/fragmentation
  on the real router). Capture `netinfo` TX/RX counters at the hang.

## Acceptance gate

- [ ] DEMO 48 boots and prints `PASS` (TLS round-trip, HTTP 401, no key)
- [ ] DEMO 49 (keyed) boots and prints `PASS` (full live Claude round-trip)
- [ ] no `bad_desc`/timeout growth in `netinfo` across the exchange
- [ ] result and representative boot log appended below

## Result log

_(append boot evidence here after each attempt, same discipline as the
Ethernet validation doc)_
