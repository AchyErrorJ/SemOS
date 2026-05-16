# embedded-tls Vendoring Brief

**Date:** 2026-05-16
**Status:** scoping document; integration not yet started
**Companion to:** `docs/PHASE_8_ROADMAP.md` (Track B step 6) and `docs/SMOLTCP_VENDORING_BRIEF.md`

Source: agent brief produced 2026-05-16. Saved here because the agent task
output files are session-scoped.

---

## 1. embedded-tls at a glance

- **Crate**: `embedded-tls` (was `drogue-tls` pre-0.10). Repo: `github.com/drogue-iot/embedded-tls`.
- **Maintainer**: Drogue IoT (Red Hat); also picked up by the embassy-rs community.
- **License**: Apache-2.0.
- **Version to target**: **0.17.x**. First line where the sync/async story is cleanly factored and the cipher-suite trait is general enough to register a single AEAD.
- **Scope**:
  - TLS **1.3 client only**.
  - No TLS 1.2 fallback (dropped in 0.14).
  - No server mode. No `TlsAcceptor`.
  - mTLS partially wired; we don't need it for Anthropic (bearer-token over TLS).
  - Session resumption types exist but documentation explicitly warns it's not feature-complete. **Disable resumption for v1**, single-use connections per request.
- **LOC**: ~6500-8000. After stripping AES suites, webpki verifier, alternative AEADs: **~4500 LOC** vendored.

## 2. Architecture

### Main types (0.17 surface)

- `TlsConfig<'a, CipherSuite>` — borrowed config: server name (SNI), CA store/verifier, optional client cert, optional PSK, cipher-suite type tag.
- `TlsContext<'a, CipherSuite, RNG>` — config + `&mut dyn CryptoRngCore`. Persists between calls.
- `TlsConnection<'a, Socket, CipherSuite>` — the connection. Owns read+write record buffers, handshake state, traffic key schedule, underlying socket.
- `TlsError` — flat enum (`InvalidRecord`, `DecryptError`, `InvalidCertificate`, `InvalidApplicationData`, `MissingHandshake`, `CipherSuiteNotSupported`, `Io(e)`). Maps cleanly to our `LlmError`.

### Handshake state machine

Standard TLS 1.3 1-RTT client. No 0-RTT in public API.

```
ClientHello              ─►
                         ◄─  ServerHello
                              {ChangeCipherSpec — ignored}
                              EncryptedExtensions
                              [CertificateRequest]    (we ignore)
                              Certificate
                              CertificateVerify
                              Finished
Finished                 ─►
                              <Application Data both ways>
```

### Crypto-swap trait surface

embedded-tls does **not** have a single clean "CryptoProvider" trait. Instead uses a **`TlsCipherSuite` trait** bundling AEAD + hash + HKDF, plus separate associated types for key-exchange and signature verifier.

```rust
pub trait TlsCipherSuite {
    const CODE_POINT: u16;
    type Hash: digest::Digest + digest::OutputSizeUser;
    type Hkdf:  hkdf::HkdfExtract<Self::Hash>;
    type Cipher: aead::AeadInPlace + aead::KeyInit;
    type KeyLen: ArrayLength<u8>;
    type IvLen:  ArrayLength<u8>;
}
```

X25519 goes through `x25519-dalek::EphemeralSecret`/`PublicKey`. ECDSA-P256 via `p256::ecdsa::VerifyingKey` (which uses RustCrypto `signature::Verifier`).

**Implication**: cleanest swap is **provide our own `TlsCipherSuite` impl** whose associated types are thin RustCrypto-trait wrappers around `kernel-core::crypto`. Concentrate patches at: (a) the X.509 verifier (§4), (b) the no-`getrandom` story for `EphemeralSecret::random_from_rng`.

## 3. Crypto-swap plan

| embedded-tls expects | Substitute from `kernel-core::crypto` | Wrapper LOC |
|---|---|---|
| ChaCha20-Poly1305 (`aead::AeadInPlace + KeyInit`) | `crypto::chacha20` + `crypto::poly1305` | ~80 |
| SHA-256 (`digest::Digest`) | `crypto::sha256::Sha256` | ~60 |
| HMAC-SHA256 (`hmac::Mac`) | `crypto::sha256::HmacSha256` | ~40 |
| HKDF-Extract/Expand (`hkdf::Hkdf`) | `crypto::hkdf::{extract, expand}` | ~60 |
| HKDF-Expand-Label / Derive-Secret (inlined inside key schedule) | `crypto::hkdf::{expand_label, derive_secret}` | replace inline calls, ~30 LOC patch |
| X25519 keygen | our RNG + `crypto::x25519::x25519_base` | ~50 |
| X25519 shared secret | `crypto::x25519::x25519(scalar, peer_u)` | (above) |
| ECDSA-P256 verify (`signature::Verifier`) | `crypto::p256::verify_p256` | ~70 |
| Random bytes (`RngCore + CryptoRng`) | `KernelRng` (RDRAND + TSC jitter) | ~40 |

**Total wrapper LOC: ~430**, single file `kernel-core/src/tls/crypto_shim.rs`. Nothing else in kernel-core changes.

### Primitives embedded-tls uses that we DON'T have

- **AES-128/256-GCM**: do NOT pull in. Strip from vendored tree; only register `0x1303` (ChaCha20-Poly1305).
- **HKDF-SHA384**: used by `Aes256GcmSha384`. Not reached.
- **RSA / RSA-PSS**: **OPEN QUESTION**. Confirm Anthropic's leaf cert is ECDSA-P256-signed; if RSA-PSS, that's +1400 LOC.
- **Ed25519**: not needed by public web PKI today.
- **`getrandom`**: explicitly disable feature in every transitive crate. Pass `KernelRng` instead.

### Cipher-suite restriction

Two safe places to enforce ChaCha20-Poly1305-only:
1. `TlsConfig::new()` is generic; instantiate only with `Chacha20Poly1305Sha256`.
2. Patch the offered-suites list in vendored ClientHello builder to one entry (`0x1303`).

Do both. Belt-and-braces is cheap.

## 4. X.509 swap (most invasive patch)

embedded-tls 0.17 has internal `CertVerifier` (or `TlsVerifier`, name varies by patch) trait:

```rust
pub trait TlsVerifier<'a, CipherSuite> {
    fn verify_certificate(
        &mut self,
        transcript: &<CipherSuite::Hash as Digest>::Clone,
        ca: &Option<Certificate<'a>>,
        cert: CertificateRef<'_>,
    ) -> Result<(), TlsError>;

    fn verify_signature(
        &mut self,
        verify: &CertificateVerify<'_>,
    ) -> Result<(), TlsError>;
}
```

Default impl chains to `webpki` if feature enabled; else `NoVerify` (accepts any cert). **Do not ship with `NoVerify`** — that's a known soundness footgun.

### Our SPKI-pinning verifier

`kernel-core/src/tls/spki_pin.rs`, **~650 LOC**:

1. Walk server's `CertificateMessage`.
2. Deliberately-limited DER scan to find `SubjectPublicKeyInfo` (no general ASN.1 parser).
3. SHA-256 hash the SPKI bytes, compare against hardcoded pin list (intermediate + leaf, recommend intermediate to survive leaf rotation).
4. `verify_signature`: extract `(r, s)` from `CertificateVerify`, assemble the TLS 1.3 signature transcript (`prefix || context || 0x00 || handshake_transcript_hash`), SHA-256, call `crypto::p256::verify_p256`.

Breakdown:
- 200 LOC: minimal DER tag-walker
- 200 LOC: SPKI extraction + SHA-256 + constant-time pin compare
- 100 LOC: leaf-cert public-key extraction
- 100 LOC: signature-transcript construction per RFC 8446 §4.4.3
- 50 LOC: pin table + tests with captured real certs

**Skip notAfter entirely**. No wall clock; pin substitutes.

## 5. Transport layer

embedded-tls 0.17 wants either:
- Sync: `embedded_io::Read + Write`
- Async: `embedded_io_async::Read + Write` (only if `async` feature enabled)

**Use sync** — we have no async runtime. Same shape as our existing `NetworkTransport` trait.

Adapter (~50-80 LOC), `kernel-core/src/tls/transport_adapter.rs`:

```rust
pub struct SmoltcpTlsSocket<'a> {
    sock:   &'a mut smoltcp::socket::tcp::Socket<'a>,
    iface:  &'a mut smoltcp::iface::Interface,
    device: &'a mut dyn smoltcp::phy::Device,
}

impl embedded_io::Read for SmoltcpTlsSocket<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        loop {
            self.poll();
            let s = self.sockets.get_mut::<tcp::Socket>(self.handle);
            if s.can_recv() { return Ok(s.recv_slice(buf).map_err(|_| Closed)?); }
            if !s.is_active() { return Err(Closed); }
            crate::scheduler::yield_now();
        }
    }
}
impl embedded_io::Write for SmoltcpTlsSocket<'_> { /* symmetric */ }
```

## 6. alloc / no_std

**embedded-tls compiles without alloc.** Designed for Cortex-M without allocator.

### Record buffers

TLS 1.3 max encrypted application-data record: 2^14 + 256 = 16640 bytes.

Practical sizing:
- `TLS_RX_BUF: [u8; 16640]` — exact max-record size (certs blow past 8 KB).
- `TLS_TX_BUF: [u8; 4096]` — outbound is small (HTTP request, ClientHello).

Total per-connection: ~20 KB static. Same `static mut TLS_CTX` pattern as `LlmContext` post-task-#40.

## 7. Time source

- **Monotonic clock**: smoltcp `Instant` — convert from our tick count to µs. Fine.
- **Wall clock for X.509 notBefore/notAfter**: we don't have one (RTC deferred to Phase 9).

### How to skip cert time validation

Since we replace the entire verifier with SPKI-pinning, we **simply don't call any time function**. embedded-tls's `TlsVerifier` impl owes only `verify_certificate` and `verify_signature`; what it does inside is its business.

Leave a `TODO(phase-9)` and a `#[cfg(feature = "rtc")]`-gated `assert!` so a future RTC enable prods the implementer to wire up validity-period checks.

## 8. Recommended integration

LOC estimates are for **integration glue only** (not embedded-tls itself).

1. **Vendor** `embedded-tls` 0.17.x into `vendor/embedded-tls/` with upstream tag in `VENDOR.md`. Modify in place (we own this fork). Strip: `examples/`, `tests/` needing std, the `webpki` cert-verifier module, AES suite modules, `getrandom` plumbing.
2. **Cargo features**: `default = []`. Disable: `webpki`, `async`, `defmt`, `log`.
3. **Crypto shim** — `kernel-core/src/tls/crypto_shim.rs`, **~430 LOC**.
4. **Cipher-suite registration** — one `TlsCipherSuite` impl, **~80 LOC**.
5. **Kernel RNG** — `kernel-x86_64/src/rng.rs`, **~40 LOC**. RDRAND-required, panic at boot if absent.
6. **SPKI-pinning verifier** — `kernel-core/src/tls/spki_pin.rs`, **~650 LOC**.
7. **Transport adapter** — `kernel-core/src/tls/transport_adapter.rs`, **~50-80 LOC**.
8. **TLS-backed `NetworkTransport`** — `kernel-core/src/llm/transport_tls.rs`, **~150 LOC**. Implements existing `NetworkTransport` by wrapping `TlsConnection`.
9. **Wire into `NetworkLlmProvider`** — extend `TransportKind` to route `Tcp` to new TLS transport. **~20 LOC patch**.
10. **Endpoint config** — `~30 LOC` if we add a syscall to set host/port/key at runtime.

**Total new code: ~1450 LOC.** Plus vendored embedded-tls (~4500 LOC post-strip).

## 9. Known issues / pitfalls

- **Cert-verification soundness**: recurring "we don't check X" defaults. `NoVerify` is the default if `webpki` feature off and you don't configure your own — **handshakes complete with any cert**. **Mitigation**: replace verifier entirely; audit call site so `verify_certificate` returning `Ok(())` is the ONLY path that proceeds. Don't rely on `verify_signature` as backstop.
- **Session resumption**: incomplete in 0.16, partially fixed in 0.17. Issues like "resumed sessions fail with InvalidApplicationData on the first record". **Mitigation**: disable resumption.
- **CertificateRequest handling**: minimal. Server demanding mTLS gets an empty Certificate which most reject with `bad_certificate`. Anthropic doesn't request client certs. Log clearly if seen.
- **Record-layer fragmentation**: handshake messages can span multiple records. 0.15 had bugs with split Certificate; 0.17 has fix. Add regression test with deliberately-fragmented handshake from captured pcap.
- **`ChangeCipherSpec` handling**: TLS 1.3 middlebox-compat — must silently ignore. Recent versions do.
- **Alert handling**: minimal. Many alerts become `TlsError::InvalidRecord`. Patch the alert path to surface level/description for debugging.

## 10. Open questions

1. **Which patch version of 0.17 to pin?** Inspect changelog for any post-0.17.0 `TlsVerifier` shape changes. Lock exact git SHA in `VENDOR.md`.
2. **Confirm sync `embedded-io` is right** vs vendoring older pre-async-split version. **Recommendation**: 0.17 with `async` disabled. Confirm strip compiles.
3. **Does the async path leak through with feature off?** Do `cargo expand` with `default-features = false` and inspect.
4. ~~**Anthropic edge signature algorithm**~~ **— RESOLVED 2026-05-16.** Inspection via `openssl s_client -connect api.anthropic.com:443 -showcerts` shows:
   - **Leaf**: `CN=api.anthropic.com`, EC P-256, signed `ecdsa-with-SHA256`. Issuer: Google Trust Services `WE1`.
   - **Intermediate**: `WE1`, EC P-256, signed `ecdsa-with-SHA384` (irrelevant — we pin its SPKI, not its signature).
   - **TLS 1.3 handshake**: `Peer signature type: ecdsa_secp256r1_sha256`.
   - **Conclusion**: every primitive we need (ECDSA-P256-verify, SHA-256, X25519, HKDF, ChaCha20-Poly1305) is already in `kernel-core/src/crypto/`. **RSA-PSS-verify NOT required.** The +1400 LOC contingency is off the table.
5. **Pin the intermediate or leaf?** **Decided: intermediate.** Leaf rotates ~quarterly (Mar→Jun 2026 in the snapshot); intermediate is valid until Feb 20, 2029.
   - **SPKI pin (SHA-256 of the intermediate's `SubjectPublicKeyInfo`, DER-encoded):**
     ```
     908769e8d34477cc2cba0632c88605b22d7294c0840f78596d247c645b1afc0e
     ```
   - This is the const to hardcode in `kernel-core/src/tls/spki_pin.rs`.
   - Re-verify before the verifier lands (issuance churn is rare but possible).
6. **`defmt` integration**: we have our own logger. Patch `defmt::trace!` / `log::debug!` call sites in vendored tree.
7. **RNG quality**: RDRAND-required-or-panic recommended over TSC-jitter fallback.
8. **Distinct `LlmError::TlsHandshakeFailed`** vs generic `ProviderUnavailable` — ~1 line + a mapping in `transport_to_llm`.

---

**Recommendation**: Vendor 0.17.x. Strip AES suites, webpki verifier, async, getrandom. Author ~1450 LOC integration glue. Run RFC 8448 vectors against key schedule before touching network. Pin intermediate CA SPKI. Skip all wall-clock cert validation.

Biggest unknown: Anthropic edge signature algorithm (ECDSA-P256 vs RSA-PSS).
