# SemOS Pairing Protocol v1 (M55) — DRAFT

**Status:** draft for review · 2026-07-22
**Milestone:** M55 (Phase 16, [`roadmap/map - phone.md`](roadmap/map%20-%20phone.md))
**Clients:** SemOS (Rust, `kernel-core::crypto`) ↔ companion app (Swift, `CryptoKit`)

Pairing bootstraps mutual trust between one SemOS device and one phone with **no
password and no account** — after pairing, the paired phone *is* the user
identity ([`semos-security-thesis.md`](semos-security-thesis.md)). This spec is the
durable artifact; the Swift app (M57) and the SemOS side (M56) are its two
implementations. It must produce byte-identical results on both — see
[Test vectors](#test-vectors).

---

## 1. Trust model & channels

Two channels exist during pairing:

1. **Visual out-of-band channel** — the phone renders a QR whose text the human
   reads and types/pastes into SemOS (`sem-sh pair <string>`). No camera in v1
   (optical scan is a later camera capability). This channel is **trusted for
   integrity in the phone→SemOS direction**: whatever SemOS receives here really
   came from the phone the human is looking at. It authenticates the phone's
   public key.
2. **Network channel** — a plain TCP connection SemOS opens to the phone's
   advertised `ip:port` on the local network. **Untrusted** (an on-path attacker
   may read/modify/inject).

The phone→SemOS direction is authenticated by (1). The SemOS→phone direction is
authenticated by a **Short Authentication String (SAS)** the human compares on
both screens (§4). Together they defeat MITM in both directions without a
pre-shared secret.

## 2. Cryptographic primitives (reuse SemOS's existing stack)

| Purpose | SemOS (`kernel-core::crypto`) | Swift |
|---|---|---|
| Key agreement | `x25519::x25519` / `x25519_base` | `Curve25519.KeyAgreement` |
| KDF extract | `hkdf::extract(salt, ikm)` | `HKDF<SHA256>` (extract) |
| KDF expand | `hkdf::expand(prk, info, okm)` | `HKDF<SHA256>` (expand) |
| AEAD | `chacha20::aead_encrypt/decrypt` (ChaCha20-Poly1305) | `ChaChaPoly` |
| MAC | `sha256::hmac(key, data)` (HMAC-SHA256) | `HMAC<SHA256>` |
| Transcript hash | `sha256` | `SHA256` |
| Randomness | `platform::random_bytes` (RDRAND) | `SystemRandomNumberGenerator` |

Both sides hold a **static X25519 identity keypair**, generated once and
persisted (SemOS: `/etc/paired-devices/self.key`; phone: Keychain/Secure
Enclave-wrapped). Pairing exchanges and confirms these static public keys; every
later session ([`network-bridge-v1.md`], Phase 17) derives a fresh session key
from them with new nonces and needs no SAS again.

## 3. QR / pairing-string payload (phone → SemOS, trusted channel)

Fixed binary layout, then Crockford **base32** (no padding, uppercase,
hyphen-grouped every 8 chars for typing). All multi-byte integers big-endian.

| Field | Bytes | Notes |
|---|---|---|
| `magic` | 2 | ASCII `"SP"` (SemOS Pair) |
| `version` | 1 | `0x01` |
| `phone_pub` | 32 | phone static X25519 public key |
| `ip` | 4 | phone listener IPv4 (local net) |
| `port` | 2 | phone listener TCP port |
| `nonce_p` | 16 | fresh random, this pairing only |
| `crc` | 2 | CRC-16/CCITT over the above (typo detection, **not** security) |

Total 59 bytes → ~95 base32 chars. Long to hand-type but paste-friendly; the
phone shows both the QR (for the future camera path) and the text string. The
`crc` catches transcription errors before any crypto runs.

## 4. Handshake

```
Human: opens app → app shows QR string → types it into `sem-sh pair <string>`

SemOS decodes payload, validates magic/version/crc, then connects TCP → ip:port.

  SemOS → phone   HELLO   { version, sem_pub(32), nonce_s(16) }        (cleartext)
  phone  → SemOS   ACK     { version }                                  (cleartext)

Both compute:
  shared      = X25519(own_static_priv, peer_static_pub)     # 32 bytes
  transcript  = "SPv1" || version || phone_pub || nonce_p || sem_pub || nonce_s
  th          = SHA256(transcript)
  prk         = HKDF-extract(salt = nonce_p || nonce_s, ikm = shared)
  session_key = HKDF-expand(prk, info = "semos-pair-v1 session" || th, 32)
  sas_bytes   = HKDF-expand(prk, info = "semos-pair-v1 sas"     || th, 4)
  SAS         = decimal(sas_bytes mod 1_000_000)  # zero-padded 6 digits

Both screens display SAS. Human compares; taps "Match" on the phone AND
confirms at the SemOS prompt. (Mismatch ⇒ abort, likely MITM.)

  SemOS → phone   CONFIRM { HMAC(session_key, "confirm-sem"   || th) }
  phone  → SemOS   CONFIRM { HMAC(session_key, "confirm-phone" || th) }

Each verifies the other's MAC (constant-time). On success both persist the
peer static public key + a derived pairing id:
  pairing_id = SHA256("semos-pair-id" || phone_pub || sem_pub)[..8]  (hex)
    SemOS: /etc/paired-devices/<pairing_id>  = { phone_pub, ip_hint, created_at }
    phone: Keychain item keyed by pairing_id = { sem_pub, created_at }
```

The SAS binds `version`, both static keys, and both nonces (all in `th`). Any
MITM substituting a key, or a downgrade of `version`, changes `th` → the two
SAS values differ → the human aborts.

## 5. Wire framing

Every message: `len: u16 (BE)` ‖ `type: u8` ‖ `body`. Types: `HELLO=1`,
`ACK=2`, `CONFIRM=3`, `ABORT=0xFF`. Bodies are the fixed fields above,
concatenated, no TLV. Max message 128 B. A side that reads a bad
length/type/version sends `ABORT` and closes. `ABORT` bodies are advisory text,
never trusted.

## 6. Threat model

| Threat | Mitigation |
|---|---|
| **Passive eavesdrop** on TCP | Handshake reveals only public keys + nonces; `shared`/keys never sent. |
| **MITM during pairing** | SAS comparison (human OOB). A relay/substitution changes `th` ⇒ SAS mismatch. |
| **Replay** of a captured handshake | `nonce_s` fresh per attempt ⇒ different `session_key`/SAS; pairing is one-shot + human-gated; stored pairing needs the CONFIRM MACs. |
| **Downgrade** (`version`, cipher) | `version` is inside `th`; v1 has one ciphersuite. Downgrade ⇒ SAS mismatch. |
| **Malicious QR** (attacker's key typed in) | Then the human is pairing the attacker's phone by choice — out of scope; SAS still authenticates *that* channel. Physical control of the SemOS keyboard is assumed. |
| **CONFIRM forgery** | HMAC under `session_key`, which requires `shared` (⇒ the static privkeys). |
| **Reflection** (same MAC both ways) | Distinct labels `"confirm-sem"` / `"confirm-phone"`. |

Explicitly **out of scope for v1:** forward secrecy of the *pairing* record
(static-key compromise reveals stored pairings — acceptable; session forward
secrecy is Phase 17's job via ephemeral keys), and the camera scan path.

## 7. SemOS-side surface (feeds M56)

- New syscall? **Yes** — `SYS_PAIR` (Ring-3 `sem-sh pair` → kernel). Smallest
  shape: `(qr_ptr, qr_len) -> pairing_id | err`. It performs the whole handshake
  (TCP + crypto + the interactive SAS confirm on the console) in the caller's
  context, mirroring `SYS_AGENT`/`SYS_NETINFO`.
- Capability check: **console-only**, like `SYS_VOUCH` — the agent must never
  reach pairing (it would let agent-written code enroll a device). Gate on the
  interactive-console tier.
- Blast radius: writes one file under `/etc/paired-devices/`. No secret leaves
  the device; the private identity key never appears in any message.

## 8. Test vectors

The canonical vectors live in **[`pairing-v1-test-vectors.md`](pairing-v1-test-vectors.md)**,
generated by `tools/pairing-vectors`. Do not duplicate them here — a second copy
is a second source of truth.

Three independent implementations are pinned to that one set:

| Implementation | How it is checked |
|---|---|
| `tools/pairing-vectors` (host Rust) | generates the vectors |
| `kernel-core::pairing` (SemOS) | boot **DEMO 86** KATs against them |
| `companion-ios` (Swift/CryptoKit) | `PairingCryptoTests` pins them |

DEMO 86 asserts the full set byte-for-byte — `phone_pub`, `transcript_hash`,
`session_key`, `SAS`, `pairing_id`, both CONFIRM MACs, the 59-byte QR payload,
the base32 string (plain **and** hyphenated), the `HELLO` frame — plus CRC typo
rejection and CONFIRM accept/reject. It passed on 2026-07-22, which is the
standing proof that SemOS and the iOS app interoperate.

> **Known caveat in the pinned inputs:** the scalars `01 00…` and `02 00…` both
> clamp to the same value under RFC 7748 (the low 3 bits are cleared), so
> `phone_pub == sem_pub` in these vectors. They remain a valid bit-identical
> KAT, but they do *not* exercise a realistic two-distinct-identity pairing. A
> second, non-degenerate vector set (e.g. scalars `11 00…` / `22 00…`) should be
> added before relying on these alone.

**CryptoKit note:** `session_key` = `HKDF<SHA256>.deriveKey(inputKeyMaterial:
sharedSecret, salt: nonce_p‖nonce_s, info: "semos-pair-v1 session"‖th,
outputByteCount: 32)` — one `deriveKey` call (extract+expand) matches the
reference's `hkdf::extract` then `hkdf::expand`. Same for the 4-byte SAS
material with info `"semos-pair-v1 sas"‖th`.

## 9. Open questions for review

1. **SAS length** — 6 digits (1e6, ~20 bits) is the SRP/Signal norm for
   human-compared strings. Bump to 7–8, or use a word/emoji list instead?
2. **QR ergonomics** — 95 typed chars is rough. Acceptable for v1 (paste), or
   shorten now by dropping `ip`/`port` and discovering the listener via mDNS
   (`_semos-pair._tcp`) so only key+nonce are typed (~63 chars)?
3. **Identity key storage on iOS** — raw X25519 in Keychain, or wrap the private
   key in the Secure Enclave (SE can't do X25519 directly, so this means an
   SE-held P-256 key encrypting the X25519 key at rest). Ties into M62.
