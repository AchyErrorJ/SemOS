//! SPKI-pinning certificate-chain validator.
//!
//! Per `docs/EMBEDDED_TLS_VENDORING_BRIEF.md` §4 + `PHASE_8_ROADMAP.md`:
//! instead of building a full PKIX path validator (which would need a
//! CA store, time source, OCSP/CRL plumbing, ASN.1 robustness, etc.),
//! we pin the SHA-256 of one known intermediate's `SubjectPublicKeyInfo`.
//! Any cert chain whose intermediate's SPKI hashes to our pin is
//! considered trusted; everything else is rejected.
//!
//! For api.anthropic.com, the pinned intermediate is `Google Trust
//! Services WE1` (subject `CN=WE1`, ECDSA P-256). Pin valid through
//! Feb 20 2029 (issuer's `notAfter`).
//!
//! # What this module gives us
//!
//! - [`extract_spki`] — given an X.509 DER cert, returns the
//!   `SubjectPublicKeyInfo` slice within it. The caller hashes that
//!   slice to compare against the pin.
//! - [`extract_ec_p256_point`] — given a `SubjectPublicKeyInfo` slice,
//!   returns the 64-byte uncompressed EC point (X || Y) for ECDSA P-256
//!   keys. Used to obtain the leaf's public key for `verify_signature`
//!   in the TLS 1.3 handshake.
//! - [`verify_pin`] — convenience: extract SPKI, SHA-256, constant-
//!   time compare against [`ANTHROPIC_INTERMEDIATE_PIN`].
//!
//! # What this module is NOT (yet)
//!
//! - Not a [`TlsVerifier`] impl. That requires embedded-tls visibility
//!   patches (the `CertificateRef.entries` field is `pub(crate)`); the
//!   trait wrapper lands in a follow-up commit after we vendor + patch.
//! - Not a full DER scanner. We walk only what we need: SEQUENCE,
//!   context-specific [0], INTEGER skip, AlgorithmIdentifier skip,
//!   Name skip, Validity skip, SubjectPublicKeyInfo (the target),
//!   BIT STRING content for the EC point. No support for indefinite
//!   length encoding (DER doesn't permit it; rejecting it is a feature).
//!
//! # Tests
//!
//! Real fixtures captured from `openssl s_client -connect
//! api.anthropic.com:443 -showcerts` on 2026-05-16:
//!   - `fixtures/anthropic_intermediate_we1.der` — full WE1 cert
//!   - `fixtures/anthropic_intermediate_we1_spki.der` — its SPKI
//!     SHA-256 of which matches [`ANTHROPIC_INTERMEDIATE_PIN`]
//!   - `fixtures/anthropic_leaf_spki.der` — leaf SPKI containing
//!     the EC point used for signature verification

use crate::crypto::sha256;

// ============================================================================
// The pin
// ============================================================================

/// SHA-256 of the `SubjectPublicKeyInfo` DER of the GTS `WE1` intermediate.
///
/// Derived 2026-05-16 via:
/// ```sh
/// openssl s_client -connect api.anthropic.com:443 -showcerts | \
///   awk '/BEGIN/{n++; if(n==2) p=1} p; /END/ && p{exit}' | \
///   openssl x509 -pubkey -noout | \
///   openssl pkey -pubin -outform DER | \
///   openssl dgst -sha256
/// ```
///
/// Intermediate validity: 2023-12-13 → 2029-02-20 (~3-year pin window).
/// If Anthropic re-fronts behind a different CA the handshake will
/// reject — that's the security guarantee, not a bug. Update by re-
/// running the command above and dropping the new value here.
pub const ANTHROPIC_INTERMEDIATE_PIN: [u8; 32] = [
    0x90, 0x87, 0x69, 0xe8, 0xd3, 0x44, 0x77, 0xcc,
    0x2c, 0xba, 0x06, 0x32, 0xc8, 0x86, 0x05, 0xb2,
    0x2d, 0x72, 0x94, 0xc0, 0x84, 0x0f, 0x78, 0x59,
    0x6d, 0x24, 0x7c, 0x64, 0x5b, 0x1a, 0xfc, 0x0e,
];

// ============================================================================
// Minimal DER scanner
// ============================================================================

/// Errors the DER scanner can return. Caller maps these into TLS-layer
/// errors when wired into a `TlsVerifier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerError {
    /// Hit end of input before completing a structure.
    Truncated,
    /// Length field used the indefinite form (DER forbids it).
    IndefiniteLength,
    /// Length field encoded with more than 4 length-of-length bytes
    /// (caps at 2^32-1 — well past any reasonable cert field).
    LengthOverflow,
    /// Tag we don't handle (e.g. we expected SEQUENCE, got something else).
    UnexpectedTag,
    /// Structure layout didn't match what an X.509 cert should look like.
    InvalidStructure,
    /// SubjectPublicKey BIT STRING had a non-zero unused-bits count, or
    /// the EC point isn't in the expected `0x04 || X || Y` form.
    InvalidPublicKey,
}

// DER tag bytes we recognise. Universal class, primitive encoding for
// scalars; constructed for SEQUENCE.
const TAG_INTEGER:        u8 = 0x02;
const TAG_BIT_STRING:     u8 = 0x03;
const TAG_OCTET_STRING:   u8 = 0x04;
const TAG_NULL:           u8 = 0x05;
const TAG_OID:            u8 = 0x06;
const TAG_UTF8_STRING:    u8 = 0x0C;
const TAG_PRINTABLE_STR:  u8 = 0x13;
const TAG_IA5_STRING:     u8 = 0x16;
const TAG_UTC_TIME:       u8 = 0x17;
const TAG_GEN_TIME:       u8 = 0x18;
const TAG_SEQUENCE:       u8 = 0x30; // constructed
const TAG_SET:            u8 = 0x31; // constructed
/// `[0]` context-specific constructed — used by X.509 `Version` and
/// extension wrappers.
const TAG_CTX_0_CONS:     u8 = 0xA0;
const TAG_CTX_3_CONS:     u8 = 0xA3;

/// View into a DER buffer with a cursor. Pure parser state — owns no
/// data, panics nowhere; every advance returns a `Result`.
#[derive(Clone, Copy)]
struct DerCursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> DerCursor<'a> {
    fn new(buf: &'a [u8]) -> Self { Self { buf, pos: 0 } }

    fn remaining(&self) -> usize { self.buf.len() - self.pos }

    fn peek_tag(&self) -> Result<u8, DerError> {
        self.buf.get(self.pos).copied().ok_or(DerError::Truncated)
    }

    /// Read one DER tag-length-value entry. Returns `(tag, value_slice)`.
    /// Advances the cursor past the entry. Does not parse the value.
    fn read_tlv(&mut self) -> Result<(u8, &'a [u8]), DerError> {
        let tag = *self.buf.get(self.pos).ok_or(DerError::Truncated)?;
        self.pos += 1;
        let len = self.read_length()?;
        if self.remaining() < len { return Err(DerError::Truncated); }
        let value = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok((tag, value))
    }

    /// Like `read_tlv` but also returns the byte range of the WHOLE TLV
    /// (tag + length + value) within `self.buf`. Used when we need the
    /// exact wire bytes for a sub-structure (e.g. SubjectPublicKeyInfo
    /// — we hash the entire SEQUENCE including its header).
    fn read_tlv_with_range(&mut self) -> Result<(u8, &'a [u8], (usize, usize)), DerError> {
        let start = self.pos;
        let (tag, value) = self.read_tlv()?;
        Ok((tag, value, (start, self.pos)))
    }

    /// Decode a DER length field per X.690 §8.1.3. Short form: < 128 is
    /// the length directly. Long form: high bit set + N bytes giving
    /// the length big-endian. Indefinite form (0x80) is forbidden by
    /// DER and rejected here.
    fn read_length(&mut self) -> Result<usize, DerError> {
        let first = *self.buf.get(self.pos).ok_or(DerError::Truncated)?;
        self.pos += 1;
        if first < 0x80 {
            return Ok(first as usize);
        }
        if first == 0x80 {
            return Err(DerError::IndefiniteLength);
        }
        let n = (first & 0x7F) as usize;
        if n > 4 { return Err(DerError::LengthOverflow); }
        if self.remaining() < n { return Err(DerError::Truncated); }
        let mut len: usize = 0;
        for _ in 0..n {
            len = (len << 8) | (self.buf[self.pos] as usize);
            self.pos += 1;
        }
        Ok(len)
    }

    /// Expect a SEQUENCE; return a cursor over its contents.
    fn read_sequence(&mut self) -> Result<DerCursor<'a>, DerError> {
        let (tag, value) = self.read_tlv()?;
        if tag != TAG_SEQUENCE { return Err(DerError::UnexpectedTag); }
        Ok(DerCursor::new(value))
    }

    /// Skip one TLV regardless of tag. Used to step over X.509 fields
    /// we don't care about (issuer, serial, validity, …).
    fn skip_any(&mut self) -> Result<(), DerError> {
        let (_tag, _value) = self.read_tlv()?;
        Ok(())
    }
}

// ============================================================================
// X.509 walking — find SubjectPublicKeyInfo within a Certificate
// ============================================================================
//
// Certificate ::= SEQUENCE {
//   tbsCertificate       TBSCertificate,
//   signatureAlgorithm   AlgorithmIdentifier,
//   signatureValue       BIT STRING
// }
//
// TBSCertificate ::= SEQUENCE {
//   version         [0]  EXPLICIT Version DEFAULT v1,    -- optional context-tagged
//   serialNumber         CertificateSerialNumber,        -- INTEGER
//   signature            AlgorithmIdentifier,            -- SEQUENCE
//   issuer               Name,                           -- SEQUENCE
//   validity             Validity,                       -- SEQUENCE
//   subject              Name,                           -- SEQUENCE
//   subjectPublicKeyInfo SubjectPublicKeyInfo,           -- SEQUENCE   <-- TARGET
//   ... extensions ...
// }
//
// We only need to scan deep enough to step over fields up to the SPKI;
// then we return its byte range (including the outer SEQUENCE header)
// because that's what the pin is hashed over.

/// Walk an X.509 DER cert and return a slice referencing its
/// `SubjectPublicKeyInfo` (including the outer SEQUENCE tag + length).
pub fn extract_spki(cert_der: &[u8]) -> Result<&[u8], DerError> {
    let mut outer = DerCursor::new(cert_der);

    // Top-level Certificate SEQUENCE.
    let mut cert = outer.read_sequence()?;

    // tbsCertificate is the first child SEQUENCE.
    let mut tbs = cert.read_sequence()?;

    // Optional [0] EXPLICIT version. If present, skip it. Otherwise the
    // next item is the serialNumber INTEGER directly (Version=v1, omitted).
    let next_tag = tbs.peek_tag()?;
    if next_tag == TAG_CTX_0_CONS {
        tbs.skip_any()?;
    }

    // serialNumber (INTEGER), signature (AlgIdent SEQUENCE), issuer
    // (Name SEQUENCE), validity (SEQUENCE), subject (Name SEQUENCE)
    // — skip the next FIVE TLVs unconditionally.
    for _ in 0..5 {
        tbs.skip_any()?;
    }

    // The next item should be subjectPublicKeyInfo. Capture its byte
    // range INCLUDING the outer SEQUENCE header — that's what the pin
    // hash covers.
    let (tag, _value, (start, end)) = tbs.read_tlv_with_range()?;
    if tag != TAG_SEQUENCE { return Err(DerError::InvalidStructure); }

    // `start` / `end` are positions WITHIN `tbs.buf`. tbs.buf is the
    // value slice of the tbsCertificate SEQUENCE — which itself is a
    // sub-slice of `cert_der`. We don't track the outer offsets here,
    // but the slice we return IS the SPKI bytes verbatim.
    Ok(&tbs.buf[start..end])
}

/// SHA-256 the SPKI of `cert_der`, constant-time-compare to
/// [`ANTHROPIC_INTERMEDIATE_PIN`]. Returns `Ok(())` on match,
/// `Err(DerError)` on parse failure, `Ok(false)` style is not used —
/// any mismatch is a hard error wrapped as `InvalidStructure`.
///
/// This is the "is this intermediate the one we trust" check called
/// once per TLS handshake against the chain's intermediate entry.
pub fn verify_pin(cert_der: &[u8], expected_pin: &[u8; 32]) -> Result<(), DerError> {
    let spki = extract_spki(cert_der)?;
    let digest = sha256::hash(spki);
    // Constant-time compare so a partial-match probe can't measure
    // where the mismatch is. (Belt-and-braces — we're not in an
    // adversarial timing setting yet, but the discipline costs nothing.)
    let mut diff: u8 = 0;
    for i in 0..32 { diff |= digest[i] ^ expected_pin[i]; }
    if diff == 0 { Ok(()) } else { Err(DerError::InvalidStructure) }
}

// ============================================================================
// EC point extraction (P-256 public keys)
// ============================================================================
//
// SubjectPublicKeyInfo ::= SEQUENCE {
//   algorithm        AlgorithmIdentifier,
//   subjectPublicKey BIT STRING
// }
//
// For an ECDSA P-256 SPKI, `subjectPublicKey` is a BIT STRING whose
// content is:
//   - 1 byte: unused-bits count (always 0 for EC keys)
//   - 1 byte: 0x04 marker (uncompressed point per SEC1)
//   - 32 bytes: X coordinate (big-endian)
//   - 32 bytes: Y coordinate (big-endian)
//
// We don't validate the algorithm OID here — the caller is asserting
// "this is an EC P-256 key" by selecting this function. If the slot is
// actually RSA or ECC with another curve, the byte layout won't match
// and we return InvalidPublicKey.

/// EC point bytes: 0x04 marker + X (32) + Y (32) = 65 bytes. Matches
/// SEC1 §2.3.3 uncompressed form, which is what `crypto::p256::verify_p256`
/// accepts as `pubkey_uncompressed`.
pub const EC_P256_UNCOMPRESSED_LEN: usize = 65;

/// Given a `SubjectPublicKeyInfo` DER slice, extract the 65-byte
/// uncompressed EC point. Returns an `InvalidPublicKey` error if the
/// SPKI doesn't look like ECDSA P-256.
pub fn extract_ec_p256_point(spki_der: &[u8]) -> Result<[u8; EC_P256_UNCOMPRESSED_LEN], DerError> {
    let mut spki = DerCursor::new(spki_der);
    let mut inner = spki.read_sequence()?;
    // Step over AlgorithmIdentifier (algorithm OID + parameters).
    inner.skip_any()?;
    // subjectPublicKey BIT STRING.
    let (tag, value) = inner.read_tlv()?;
    if tag != TAG_BIT_STRING { return Err(DerError::UnexpectedTag); }
    // First byte of BIT STRING content is the unused-bits count.
    if value.is_empty() || value[0] != 0 { return Err(DerError::InvalidPublicKey); }
    let point = &value[1..];
    if point.len() != EC_P256_UNCOMPRESSED_LEN { return Err(DerError::InvalidPublicKey); }
    if point[0] != 0x04 { return Err(DerError::InvalidPublicKey); }
    let mut out = [0u8; EC_P256_UNCOMPRESSED_LEN];
    out.copy_from_slice(point);
    Ok(out)
}

// ============================================================================
// Tests — real fixtures from api.anthropic.com (2026-05-16)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Full DER of the GTS `WE1` intermediate cert (675 bytes).
    const INTERMEDIATE_DER: &[u8] =
        include_bytes!("fixtures/anthropic_intermediate_we1.der");

    /// Just the SubjectPublicKeyInfo of the same intermediate (91 bytes).
    /// SHA-256 of these bytes equals `ANTHROPIC_INTERMEDIATE_PIN`.
    const INTERMEDIATE_SPKI: &[u8] =
        include_bytes!("fixtures/anthropic_intermediate_we1_spki.der");

    /// SubjectPublicKeyInfo of the leaf `api.anthropic.com` cert.
    /// Contains the EC P-256 point we'd use for signature verification.
    const LEAF_SPKI: &[u8] =
        include_bytes!("fixtures/anthropic_leaf_spki.der");

    #[test]
    fn fixture_sizes_are_what_we_expect() {
        assert_eq!(INTERMEDIATE_DER.len(), 675);
        assert_eq!(INTERMEDIATE_SPKI.len(), 91);
        assert_eq!(LEAF_SPKI.len(), 91);
    }

    #[test]
    fn extract_spki_from_intermediate_matches_standalone() {
        let extracted = extract_spki(INTERMEDIATE_DER)
            .expect("intermediate cert is well-formed");
        assert_eq!(extracted, INTERMEDIATE_SPKI,
            "SPKI extracted from full cert must equal the standalone SPKI bytes");
    }

    #[test]
    fn intermediate_spki_hash_equals_pin() {
        // Hash the standalone SPKI bytes; check against the hardcoded pin.
        let digest = sha256::hash(INTERMEDIATE_SPKI);
        assert_eq!(digest, ANTHROPIC_INTERMEDIATE_PIN);
    }

    #[test]
    fn verify_pin_accepts_real_intermediate() {
        verify_pin(INTERMEDIATE_DER, &ANTHROPIC_INTERMEDIATE_PIN)
            .expect("real WE1 intermediate must match the pin");
    }

    #[test]
    fn verify_pin_rejects_wrong_pin() {
        let mut wrong = ANTHROPIC_INTERMEDIATE_PIN;
        wrong[0] ^= 0x01; // flip one bit
        let result = verify_pin(INTERMEDIATE_DER, &wrong);
        assert!(result.is_err(), "tampered pin must reject");
    }

    #[test]
    fn verify_pin_rejects_corrupted_cert() {
        // Flip one byte well inside the SPKI region (after the outer
        // SEQUENCE header) so we change what gets hashed. Kept no_alloc
        // by copying into a fixed-size stack buffer — kernel-core can't
        // pull in `alloc::vec::Vec`.
        const N: usize = 675; // size of INTERMEDIATE_DER
        let mut corrupted = [0u8; N];
        corrupted.copy_from_slice(INTERMEDIATE_DER);
        corrupted[N / 2] ^= 0xFF;
        let result = verify_pin(&corrupted, &ANTHROPIC_INTERMEDIATE_PIN);
        assert!(result.is_err(), "corrupted cert must fail the pin");
    }

    #[test]
    fn extract_ec_point_from_leaf_spki() {
        let point = extract_ec_p256_point(LEAF_SPKI)
            .expect("leaf SPKI is ECDSA P-256");
        assert_eq!(point.len(), 65);
        assert_eq!(point[0], 0x04, "must be uncompressed form marker");
        // The leaf's X (bytes 1..33) should start with the known prefix
        // from the openssl dump (0xF1, 0x5B, 0x46, 0xF2, ...).
        assert_eq!(&point[1..5], &[0xF1, 0x5B, 0x46, 0xF2]);
    }

    #[test]
    fn extract_ec_point_from_intermediate_spki() {
        // Intermediate is also EC P-256; should extract a valid point.
        let point = extract_ec_p256_point(INTERMEDIATE_SPKI)
            .expect("intermediate SPKI is ECDSA P-256");
        assert_eq!(point[0], 0x04);
        // Known prefix from the WE1 dump: 6f cd 3a fe 67 57 47 4c ...
        assert_eq!(&point[1..5], &[0x6F, 0xCD, 0x3A, 0xFE]);
    }

    #[test]
    fn der_scanner_rejects_indefinite_length() {
        // Construct a tiny "SEQUENCE with indefinite length" — tag 0x30,
        // length 0x80 (indefinite). DER forbids this; we must reject.
        let bad = [0x30u8, 0x80, 0x00, 0x00];
        assert!(extract_spki(&bad).is_err());
    }

    #[test]
    fn der_scanner_rejects_truncated_input() {
        // SEQUENCE claiming length 100 but only 4 bytes follow.
        let bad = [0x30u8, 0x64, 0x00, 0x00];
        assert!(extract_spki(&bad).is_err());
    }
}
