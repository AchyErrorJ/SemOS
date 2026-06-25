# Roadmap — Phone Symbiosis

> Part of the [Master Roadmap](../MASTER_ROADMAP.md). Sibling themes:
> [networking](map%20-%20networking.md) · [self-extension](map%20-%20self-extension.md) ·
> [gpu](map%20-%20gpu.md) · [platform](map%20-%20platform.md). Historical log: [ROADMAP.md](../ROADMAP.md).

**Phone-as-peripheral.** The phone provides capabilities the OS doesn't have —
crypto, identity, camera, GPS, audio, push, and (via the bridge in
[networking.md](map%20-%20networking.md)) connectivity. **Pairing IS authentication:** no
password, no login screen; the paired phone is the user account. The phone holds
the keys; SemOS is the I/O layer.

**Security discipline (every milestone):** answer before coding — does this need a
new syscall? smallest shape? what capability check guards it? blast radius if the
agent misuses it? See [`semos-security-thesis.md`](../semos-security-thesis.md)
and [`provenance-commitment.md`](../provenance-commitment.md).

---

## Phase 16 — QR-Code Pairing (the trust bootstrap)

**Goal:** pair a SemOS device with a specific phone via QR code, establishing a
TLS-protected channel. Build the companion app as an **Expo (React Native)
prototype** first so it can be developed without a Mac (the native Swift rewrite
is Phase 19, in [platform.md](map%20-%20platform.md)). Expo is acceptable here because the
bridge is protocol + minimal UI with no ARKit dependency. Depends on Phase 15.

### M55 — Pairing protocol design `[  ]`
QR contains: companion app public key, listening IP+port, pairing nonce, protocol
version. No camera in v1 — the QR string is *typed/pasted* into the device
(optical scan is a Phase-18 camera feature).
- [ ] spec `docs/pairing-v1.md`; wire format (Protobuf/Cap'n Proto)
- [ ] threat model: replay, MITM-during-pairing, downgrade
- [ ] test vectors: known run → known shared secret; Rust ↔ TypeScript stay in sync

### M56 — Pairing on the SemOS side `[  ]`
- [ ] `sem-sh pair <qr-string>`; handshake against the Expo app
- [ ] identity persists across reboots (`/etc/paired-devices/`)
- [ ] `paired list` / `unpair <id>`; DEMO 86 (pair, reboot, still paired)

### M57 — Expo bridge app skeleton `[  ]`
TypeScript + Expo SDK, `react-native-tcp-socket`, `react-native-tls`,
`expo-secure-store` (Keychain/Keystore), `react-native-zeroconf`, QR display.
- [ ] shows QR, listens on local TCP, completes pairing server-side, stores peer key
- [ ] runs on iPhone via Expo Go (no TestFlight); DEMO 87 end-to-end pairing

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

---

## Sensor Offload (Phase-18 preview — LiDAR / point-cloud)

Unlocked by the 2026-06-10 tether leapfrog: stream the iPhone's LiDAR point cloud
over the existing tether IP link for the LegibleStudios design work. The iOS side
needs ARKit, which **Expo Go cannot host** — it needs an EAS cloud build or a Mac
(slightly ahead of the Phase 19 native-bridge assumption). Full plan:
[`IPHONE_SENSOR_OFFLOAD_PLAN.md`](../IPHONE_SENSOR_OFFLOAD_PLAN.md).

Phone-as-vault / phone-as-presence-key for WiFi sign-in also belongs here once
pairing lands.
