# Roadmap — Phone Symbiosis

> Part of the [Master Roadmap](../MASTER_ROADMAP.md). Sibling themes:
> [networking](map%20-%20networking.md) · [self-extension](map%20-%20self-extension.md) ·
> [gpu](map%20-%20gpu.md) · [platform](map%20-%20platform.md). Historical log: [ROADMAP.md](../ROADMAP.md).

**Phone-as-peripheral.** The phone provides capabilities the OS doesn't have —
crypto, identity, camera, GPS, audio, push, rendered web pages, and (via the
bridge in [networking.md](map%20-%20networking.md)) connectivity. **Pairing IS authentication:** no
password, no login screen; the paired phone is the user account. The phone holds
the keys; SemOS is the I/O layer.

**Security discipline (every milestone):** answer before coding — does this need a
new syscall? smallest shape? what capability check guards it? blast radius if the
agent misuses it? See [`semos-security-thesis.md`](../semos-security-thesis.md)
and [`provenance-commitment.md`](../provenance-commitment.md).

---

## Phase 16 — QR-Code Pairing (the trust bootstrap)

**Goal:** pair a SemOS device with a specific phone via QR code, establishing a
TLS-protected channel.

> **DECISION 2026-07-22 — build the companion app in native Swift directly.**
> The original plan sequenced an Expo (React Native) prototype first *because it
> could be built without a Mac*. A Mac is now available, so we skip the Expo
> detour and its throwaway Expo→Swift rewrite: **Phase 19 (M68–M71 in
> [platform.md](map%20-%20platform.md)) folds into this phase.** Native from the start also
> unblocks the capabilities that Expo Go structurally cannot host — ARKit sensor
> offload and the M76 WKWebView render capability. Depends on Phase 15.

### M55 — Pairing protocol design `[🔨 — draft spec landed 2026-07-22]`
QR contains: companion app public key, listening IP+port, pairing nonce, protocol
version. No camera in v1 — the QR string is *typed/pasted* into the device
(optical scan is a Phase-18 camera feature). Design: X25519 key agreement +
HKDF + a human-compared **SAS** to authenticate the untrusted TCP direction,
reusing SemOS's existing crypto (`kernel-core::crypto`). Full spec:
[`docs/pairing-v1.md`](../pairing-v1.md).
- [x] spec `docs/pairing-v1.md`: binary wire format + base32 string, handshake
- [x] threat model: replay, MITM-during-pairing (SAS), downgrade
- [ ] resolve open questions (SAS length, mDNS vs typed ip:port, iOS key storage)
- [ ] test vectors: fill from the Rust reference impl; Rust ↔ Swift stay in sync

### M56 — Pairing on the SemOS side `[  ]`
- [ ] `sem-sh pair <qr-string>`; handshake against the Expo app
- [ ] identity persists across reboots (`/etc/paired-devices/`)
- [ ] `paired list` / `unpair <id>`; DEMO 86 (pair, reboot, still paired)

### M57 — Native Swift app skeleton `[  ]`
Xcode + SwiftUI app (replaces the former Expo skeleton). `Network.framework`
(`NWListener`/`NWConnection`) for the local TCP socket, `CryptoKit` for the
X25519/HKDF handshake, Keychain for the stored peer key, `CoreImage` to render
the QR. Runs on a real iPhone via a free dev-signed build (7-day) or TestFlight.
- [ ] shows QR, listens on local TCP, completes pairing server-side, stores peer key
- [ ] DEMO 87: end-to-end pairing (SemOS `pair` ↔ Swift app) on a real iPhone

---

## Phase 18 — Companion App Capabilities (phone-as-peripheral payoff)

**Goal:** each phone capability becomes a SemOS request over the paired channel.
Independent milestones, any order by need. Crypto + identity first (unlock secure
operation), camera second (most demoable). Depends on Phase 17 paired channel
([networking.md](map%20-%20networking.md)).

### M62 — Crypto capability (Secure Enclave / StrongBox) `[  ]`
Private keys never leave the phone.
- [ ] `request_generate_keypair(label)` → public key from Secure Enclave
- [ ] `request_sign` / `request_decrypt`; Face/Touch ID gated, per-key configurable
- [ ] DEMO 90: keypair in iPhone Secure Enclave, sign, verify on-device

### M63 — Identity capability `[  ]`
- [ ] `request_identity()`; OAuth proxied through phone; tokens in phone Keychain
- [ ] DEMO 91: authenticate to Anthropic with a token held on the phone, none on disk

### M64 — Camera capability `[  ]`
- [ ] `request_camera_capture(mode)` / `request_qr_scan()`; GPS metadata stripped by default
- [ ] DEMO 92: requested photo lands at `/tmp/capture.jpg`

### M65 — GPS capability `[  ]`
- [ ] `request_location()` → lat/lng/accuracy/timestamp, per-session authz
- [ ] DEMO 93: agent answers "where am I" via location + reverse-geocode

### M66 — Microphone capability `[  ]`
- [ ] `request_audio_capture(duration)` → WAV/compressed; DEMO 94 (voice prompt → agent)

### M67 — Push notification capability `[  ]`
- [ ] `request_notification(title, body, action)` + tap-to-action callback
- [ ] DEMO 95: long compile finishes → "build done" notification on phone

### M76 — Render capability (JS-rendered / anti-bot fallback) `[  ]`
For pages the native HTTP+HTML-to-text path ([platform.md](map%20-%20platform.md)
M29-M32) can't handle — JS-only SPAs, bot-check gates (Cloudflare, CAPTCHA), or
sites that key off a real Safari-class TLS/UA fingerprint. The phone loads the
URL in a hidden `WKWebView`, waits for load/JS settle, and returns extracted
content over the paired channel. **Tiered fallback, not a replacement:** the
agent always tries the native fetch + `html_to_text` path first; this only
fires on empty/failed extraction. Keeps the sovereign from-scratch path as the
default and confines the phone's rendering engine to the cases that structurally
require it.
- [ ] `request_render(url, wait_for_selector?)` → extracted text + links, or a
  raw `outerHTML` snapshot for cases that need structure
- [ ] authorization model decided at design time: reuse M62's Face/Touch-ID
  gate, or a lighter per-session grant given the payload isn't a secret
- [ ] rendered HTML/text returned to SemOS is treated as untrusted input, same
  discipline as any other fetch response
- [ ] DEMO 99: agent hits a JS-only doc page that fails native extraction,
  falls back to phone render, gets usable text

Depends on Phase 17's paired channel (M58-M61) — not startable before then.

---

## Sensor Offload (Phase-18 preview — LiDAR / point-cloud)

Unlocked by the 2026-06-10 tether leapfrog: stream the iPhone's LiDAR point cloud
over the existing tether IP link for the LegibleStudios design work. The iOS side
needs ARKit, which **Expo Go cannot host** — it needs an EAS cloud build or a Mac
(slightly ahead of the Phase 19 native-bridge assumption). Full plan:
[`IPHONE_SENSOR_OFFLOAD_PLAN.md`](../IPHONE_SENSOR_OFFLOAD_PLAN.md).

Phone-as-vault / phone-as-presence-key for WiFi sign-in also belongs here once
pairing lands.
