//! HKDF-SHA256 (RFC 5869) plus the TLS 1.3 helpers from RFC 8446 §7.1.
//!
//! Surface:
//! - [`extract`] — `HKDF-Extract(salt, IKM)` → 32-byte PRK
//! - [`expand`]  — `HKDF-Expand(PRK, info, L)` writing L bytes into the caller's buffer
//! - [`expand_label`] — TLS 1.3 `HKDF-Expand-Label(secret, label, context, L)`
//! - [`derive_secret`] — TLS 1.3 `Derive-Secret(secret, label, messages)` (32 bytes out)
//!
//! No `alloc`. No heap. The HKDF-Expand loop streams into the underlying
//! [`HmacSha256`], so `info` length is bounded only by what callers want
//! to pass — not by a hard-coded stack buffer size. The TLS 1.3
//! [`expand_label`] does use a 514-byte stack buffer because the
//! `HkdfLabel` wire format has tight bounds (label ≤ 255, context ≤ 255).
//!
//! Tests cover RFC 5869 §A.1–A.3 plus the early-secret and Derive-Secret
//! derivations from RFC 8448 §3, which is the canonical TLS 1.3 key-
//! schedule sanity check called out in the Phase 8 roadmap as "the single
//! discipline that catches ~80% of from-scratch TLS impl bugs."

use super::sha256::{HmacSha256, OUTPUT_SIZE};

/// HKDF-Extract per RFC 5869 §2.2.
///
/// Returns a 32-byte pseudorandom key (PRK). Per the RFC, if `salt` is
/// empty (`&[]`) it's treated as a string of HashLen zero bytes — we
/// materialise that explicitly so HMAC sees the right zero-padded key.
pub fn extract(salt: &[u8], ikm: &[u8]) -> [u8; OUTPUT_SIZE] {
    let zeros = [0u8; OUTPUT_SIZE];
    let actual_salt: &[u8] = if salt.is_empty() { &zeros } else { salt };
    let mut h = HmacSha256::new(actual_salt);
    h.update(ikm);
    h.finalize()
}

/// HKDF-Expand per RFC 5869 §2.3.
///
/// Writes exactly `okm.len()` bytes into `okm`. Returns `false` if the
/// requested length exceeds `255 * HashLen` (the RFC's hard ceiling); on
/// `false` the buffer is left in an indeterminate state — caller must
/// treat it as undefined.
///
/// The loop body is `T(i) = HMAC(PRK, T(i-1) || info || i)`, which we
/// drive via streaming HMAC so `info` has no fixed-size bound.
pub fn expand(prk: &[u8], info: &[u8], okm: &mut [u8]) -> bool {
    if okm.len() > 255 * OUTPUT_SIZE { return false; }
    if okm.is_empty() { return true; }

    let n = (okm.len() + OUTPUT_SIZE - 1) / OUTPUT_SIZE;
    let mut t_prev = [0u8; OUTPUT_SIZE];
    let mut t_prev_len = 0usize; // T(0) is the empty string
    let mut written = 0;

    for i in 1..=n {
        let mut h = HmacSha256::new(prk);
        h.update(&t_prev[..t_prev_len]);
        h.update(info);
        h.update(&[i as u8]); // counter is 1 byte; n ≤ 255 holds because of the upper bound above
        let t_i = h.finalize();

        let want = (okm.len() - written).min(OUTPUT_SIZE);
        okm[written..written + want].copy_from_slice(&t_i[..want]);
        written += want;

        t_prev = t_i;
        t_prev_len = OUTPUT_SIZE;
    }

    true
}

/// `HKDF-Expand-Label(secret, label, context, Length)` per RFC 8446 §7.1.
///
/// `label` is the suffix; this function prepends the literal `"tls13 "`
/// (note the trailing space — easy to forget and breaks every key in
/// the schedule when omitted). `context` is the per-call data, often a
/// transcript hash; pass `&[]` when the spec calls for "" context.
///
/// Returns `false` if the constructed HkdfLabel would exceed RFC 8446's
/// bounds: label suffix > 249 bytes (because "tls13 " + label ≤ 255),
/// context > 255 bytes, or okm.len() > 65535.
pub fn expand_label(
    secret: &[u8],
    label: &[u8],
    context: &[u8],
    okm: &mut [u8],
) -> bool {
    // HkdfLabel wire format (RFC 8446 §7.1):
    //   struct {
    //       uint16 length;                       // 2 bytes BE
    //       opaque label<7..255> = "tls13 " + Label;
    //       opaque context<0..255>;
    //   } HkdfLabel;
    const TLS13_PREFIX: &[u8] = b"tls13 ";
    let full_label_len = TLS13_PREFIX.len() + label.len();

    if full_label_len > 255 { return false; }
    if context.len() > 255 { return false; }
    if okm.len() > 0xFFFF { return false; }

    // Max HkdfLabel size: 2 + 1 + 255 + 1 + 255 = 514.
    let mut info = [0u8; 514];
    let mut pos = 0;

    // uint16 length (the OKM length the caller wants), big-endian.
    let len_be = (okm.len() as u16).to_be_bytes();
    info[pos] = len_be[0]; pos += 1;
    info[pos] = len_be[1]; pos += 1;

    // opaque label<7..255>: 1-byte length prefix + "tls13 " + label
    info[pos] = full_label_len as u8; pos += 1;
    info[pos..pos + TLS13_PREFIX.len()].copy_from_slice(TLS13_PREFIX);
    pos += TLS13_PREFIX.len();
    info[pos..pos + label.len()].copy_from_slice(label);
    pos += label.len();

    // opaque context<0..255>: 1-byte length prefix + context
    info[pos] = context.len() as u8; pos += 1;
    info[pos..pos + context.len()].copy_from_slice(context);
    pos += context.len();

    expand(secret, &info[..pos], okm)
}

/// `Derive-Secret(Secret, Label, Messages)` per RFC 8446 §7.1.
///
/// Returns `HKDF-Expand-Label(Secret, Label, Transcript-Hash(Messages), HashLen)`.
/// Here the caller supplies the already-computed transcript hash directly
/// — this module knows nothing about TLS handshake structure, just the
/// key-schedule math.
pub fn derive_secret(secret: &[u8], label: &[u8], transcript_hash: &[u8]) -> [u8; OUTPUT_SIZE] {
    let mut out = [0u8; OUTPUT_SIZE];
    // Bounds are statically satisfied for sane TLS labels + a SHA-256 hash:
    //   label ≤ ~13 bytes for any spec'd value, transcript = 32 bytes.
    // `expand_label` will only return false on much larger inputs.
    let ok = expand_label(secret, label, transcript_hash, &mut out);
    debug_assert!(ok, "derive_secret arguments exceeded HKDF bounds");
    out
}

// ============================================================================
// Tests — RFC 5869 §A.1–A.3 + RFC 8448 §3 (TLS 1.3 reference handshake).
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::sha256;

    /// Hex-encode `d` into `out`, return number of bytes written.
    /// Big buffer because RFC 5869 §A.2 OKM is 82 bytes → 164 hex chars.
    fn hex(d: &[u8], out: &mut [u8; 256]) -> usize {
        let mut p = 0;
        for &b in d {
            let high = b >> 4;
            let low = b & 0xF;
            out[p]     = if high < 10 { b'0' + high } else { b'a' + high - 10 };
            out[p + 1] = if low  < 10 { b'0' + low  } else { b'a' + low  - 10 };
            p += 2;
        }
        p
    }

    fn assert_hex(actual: &[u8], expected: &[u8]) {
        let mut buf = [0u8; 256];
        let n = hex(actual, &mut buf);
        assert_eq!(&buf[..n], expected,
            "expected {} bytes of hex, got {} bytes of digest",
            expected.len(), actual.len());
    }

    // --- RFC 5869 §A.1: Basic test case with SHA-256 ---
    #[test]
    fn rfc5869_tc1() {
        let ikm = [0x0b; 22];
        let salt: [u8; 13] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        ];
        let info: [u8; 10] = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];

        let prk = extract(&salt, &ikm);
        assert_hex(&prk,
            b"077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");

        let mut okm = [0u8; 42];
        assert!(expand(&prk, &info, &mut okm));
        assert_hex(&okm,
            b"3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865");
    }

    // --- RFC 5869 §A.2: SHA-256 with longer inputs/outputs ---
    #[test]
    fn rfc5869_tc2() {
        let ikm:  [u8; 80] = core::array::from_fn(|i| i as u8);          // 0x00..0x4f
        let salt: [u8; 80] = core::array::from_fn(|i| (0x60 + i) as u8); // 0x60..0xaf
        let info: [u8; 80] = core::array::from_fn(|i| (0xb0 + i) as u8); // 0xb0..0xff

        let prk = extract(&salt, &ikm);
        assert_hex(&prk,
            b"06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244");

        let mut okm = [0u8; 82];
        assert!(expand(&prk, &info, &mut okm));
        assert_hex(&okm,
            b"b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87");
    }

    // --- RFC 5869 §A.3: zero-length salt and info ---
    #[test]
    fn rfc5869_tc3() {
        let ikm = [0x0b; 22];

        let prk = extract(&[], &ikm);
        assert_hex(&prk,
            b"19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04");

        let mut okm = [0u8; 42];
        assert!(expand(&prk, &[], &mut okm));
        assert_hex(&okm,
            b"8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8");
    }

    // --- RFC 8448 §3: TLS 1.3 reference handshake, no-PSK ---
    //
    // Early-Secret = HKDF-Extract(salt = 0..0, IKM = 0..0)
    //              = 33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a
    #[test]
    fn rfc8448_early_secret() {
        let zeros = [0u8; 32];
        let early = extract(&zeros, &zeros);
        assert_hex(&early,
            b"33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a");
    }

    // RFC 8448 §3: derived = Derive-Secret(Early-Secret, "derived", "")
    //             = 6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba
    //
    // This is the canonical end-to-end test of the TLS 1.3 key-schedule
    // helpers: it exercises Extract, Derive-Secret, Expand-Label, Expand,
    // the HkdfLabel wire encoding, the "tls13 " prefix, the transcript
    // hash plumbing, and SHA-256 of the empty string — all at once.
    // If this one passes, the math is right.
    #[test]
    fn rfc8448_derived_from_early_secret() {
        let zeros = [0u8; 32];
        let early = extract(&zeros, &zeros);
        let empty_transcript = sha256::hash(b"");
        let derived = derive_secret(&early, b"derived", &empty_transcript);
        assert_hex(&derived,
            b"6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba");
    }

    // Sanity: empty OKM is a no-op success.
    #[test]
    fn expand_empty_okm_is_ok() {
        let prk = extract(&[0u8; 32], &[0u8; 32]);
        let mut okm: [u8; 0] = [];
        assert!(expand(&prk, b"some info", &mut okm));
    }

    // Bounds: 255 * 32 = 8160 is the largest L `expand` accepts.
    #[test]
    fn expand_at_and_over_max_length() {
        let prk = [0x42u8; 32];
        let mut max_ok = [0u8; 255 * OUTPUT_SIZE];
        assert!(expand(&prk, b"i", &mut max_ok));

        let mut too_big = [0u8; 255 * OUTPUT_SIZE + 1];
        assert!(!expand(&prk, b"i", &mut too_big));
    }

    // Bounds: expand_label rejects oversized label / context / okm.
    #[test]
    fn expand_label_rejects_oversized_inputs() {
        let prk = [0u8; 32];
        let mut okm = [0u8; 32];
        let mut big_okm = [0u8; 0x1_0000];

        // Label suffix that makes "tls13 " + label exceed 255.
        let bad_label = [b'x'; 250]; // 6 + 250 = 256 > 255
        assert!(!expand_label(&prk, &bad_label, &[], &mut okm));

        // Context that exceeds 255.
        let bad_context = [b'c'; 256];
        assert!(!expand_label(&prk, b"valid", &bad_context, &mut okm));

        // okm.len() > 0xFFFF.
        assert!(!expand_label(&prk, b"valid", &[], &mut big_okm));
    }
}
