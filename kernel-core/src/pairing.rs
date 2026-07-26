//! SemOS Pairing Protocol v1 — crypto + encoding core (M56).
//!
//! Spec: `docs/pairing-v1.md`. These are **pure, no-I/O** functions so that the
//! real `SYS_PAIR` handshake, the loopback self-test, and the companion app all
//! agree bit-for-bit. Nothing here touches the network, the console, or the
//! filesystem — the syscall layer orchestrates those around this core.
//!
//! Primitives are SemOS's own: X25519 key agreement, HKDF-SHA256, HMAC-SHA256,
//! SHA-256. No heap: every function works in fixed-size buffers.

use crate::crypto::crc16::crc16_ccitt;
use crate::crypto::crockford;
use crate::crypto::{hkdf, sha256, x25519};

/// Protocol version carried in the QR payload and bound into the transcript.
pub const VERSION: u8 = 0x01;
/// Payload magic (`"SP"` = SemOS Pair).
pub const MAGIC: [u8; 2] = *b"SP";
/// Pairing nonce length (per side).
pub const NONCE_LEN: usize = 16;
/// Decoded QR payload size in bytes: magic(2) ver(1) pub(32) ip(4) port(2)
/// nonce(16) crc(2).
pub const PAYLOAD_LEN: usize = 59;
/// Max base32 characters the encoded payload occupies (excludes hyphens).
pub const MAX_ENCODED_LEN: usize = 96;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// A character outside the Crockford base32 alphabet.
    BadChar,
    /// Decoded length was not exactly `PAYLOAD_LEN`.
    BadLength,
    /// Magic bytes were not `"SP"`.
    BadMagic,
    /// Version byte did not match `VERSION`.
    BadVersion,
    /// CRC-16 mismatch — a transcription typo, not an attack.
    BadCrc,
}

// ============================================================================
// The decoded QR payload
// ============================================================================

/// The phone-supplied pairing parameters, received over the trusted visual
/// channel (QR string typed/pasted into `sem-sh pair`).
#[derive(Clone, Copy)]
pub struct PairingPayload {
    pub version: u8,
    pub phone_pub: [u8; 32],
    pub ip: [u8; 4],
    pub port: u16,
    pub nonce_p: [u8; NONCE_LEN],
}

impl PairingPayload {
    /// Serialize into the fixed 59-byte wire layout (with CRC appended).
    pub fn to_bytes(&self) -> [u8; PAYLOAD_LEN] {
        let mut b = [0u8; PAYLOAD_LEN];
        b[0..2].copy_from_slice(&MAGIC);
        b[2] = self.version;
        b[3..35].copy_from_slice(&self.phone_pub);
        b[35..39].copy_from_slice(&self.ip);
        b[39..41].copy_from_slice(&self.port.to_be_bytes());
        b[41..57].copy_from_slice(&self.nonce_p);
        let crc = crc16_ccitt(&b[0..57]);
        b[57..59].copy_from_slice(&crc.to_be_bytes());
        b
    }

    /// Parse from the fixed 59-byte layout, validating magic, version, and CRC.
    pub fn from_bytes(b: &[u8]) -> Result<Self, PairError> {
        if b.len() != PAYLOAD_LEN {
            return Err(PairError::BadLength);
        }
        if b[0..2] != MAGIC {
            return Err(PairError::BadMagic);
        }
        if b[2] != VERSION {
            return Err(PairError::BadVersion);
        }
        let crc = u16::from_be_bytes([b[57], b[58]]);
        if crc != crc16_ccitt(&b[0..57]) {
            return Err(PairError::BadCrc);
        }
        let mut phone_pub = [0u8; 32];
        phone_pub.copy_from_slice(&b[3..35]);
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&b[35..39]);
        let port = u16::from_be_bytes([b[39], b[40]]);
        let mut nonce_p = [0u8; NONCE_LEN];
        nonce_p.copy_from_slice(&b[41..57]);
        Ok(Self { version: b[2], phone_pub, ip, port, nonce_p })
    }
}

/// Encode a payload as a Crockford base32 string into `out` (ASCII bytes),
/// returning the character count. `out` must be at least `MAX_ENCODED_LEN`.
pub fn encode_payload(p: &PairingPayload, out: &mut [u8]) -> usize {
    crockford::encode_into(&p.to_bytes(), out).unwrap_or(0)
}

/// Decode a pairing string (hyphens and case ignored) back into a payload.
pub fn decode_payload(s: &str) -> Result<PairingPayload, PairError> {
    let mut raw = [0u8; PAYLOAD_LEN + 2]; // slack; exact length checked below
    let n = crockford::decode_into(s.as_bytes(), &mut raw).ok_or(PairError::BadChar)?;
    PairingPayload::from_bytes(&raw[..n])
}

// ============================================================================
// Handshake key schedule
// ============================================================================

/// Result of the pairing key agreement — identical on both sides.
#[derive(Clone, Copy)]
pub struct Session {
    /// Symmetric key for the CONFIRM MACs (and, later, the first bridge
    /// session bootstrap).
    pub session_key: [u8; 32],
    /// Transcript hash: `SHA256("SPv1" || version || phone_pub || nonce_p ||
    /// sem_pub || nonce_s)`. Everything the SAS commits to.
    pub transcript_hash: [u8; 32],
    /// Short Authentication String the human compares on both screens:
    /// a zero-padded 6-digit decimal.
    pub sas: u32,
}

/// Derive the shared session from both sides' static keys and nonces.
///
/// `own_priv` is *this* device's static X25519 private key; `peer_pub` is the
/// *other* device's static public key. The four transcript fields
/// (`phone_pub`, `nonce_p`, `sem_pub`, `nonce_s`, `version`) are identical on
/// both sides, so both compute the same `Session` despite feeding their own
/// private key into the DH.
pub fn derive_session(
    version: u8,
    phone_pub: &[u8; 32],
    nonce_p: &[u8; NONCE_LEN],
    sem_pub: &[u8; 32],
    nonce_s: &[u8; NONCE_LEN],
    own_priv: &[u8; 32],
    peer_pub: &[u8; 32],
) -> Session {
    let shared = x25519::x25519(own_priv, peer_pub);

    // transcript = "SPv1" || version || phone_pub || nonce_p || sem_pub || nonce_s
    let mut transcript = [0u8; 4 + 1 + 32 + NONCE_LEN + 32 + NONCE_LEN];
    let mut o = 0;
    transcript[o..o + 4].copy_from_slice(b"SPv1");
    o += 4;
    transcript[o] = version;
    o += 1;
    transcript[o..o + 32].copy_from_slice(phone_pub);
    o += 32;
    transcript[o..o + NONCE_LEN].copy_from_slice(nonce_p);
    o += NONCE_LEN;
    transcript[o..o + 32].copy_from_slice(sem_pub);
    o += 32;
    transcript[o..o + NONCE_LEN].copy_from_slice(nonce_s);
    let th = sha256::hash(&transcript);

    // prk = HKDF-extract(salt = nonce_p || nonce_s, ikm = shared)
    let mut salt = [0u8; NONCE_LEN * 2];
    salt[..NONCE_LEN].copy_from_slice(nonce_p);
    salt[NONCE_LEN..].copy_from_slice(nonce_s);
    let prk = hkdf::extract(&salt, &shared);

    // session_key = HKDF-expand(prk, "semos-pair-v1 session" || th, 32)
    let mut session_key = [0u8; 32];
    expand_labeled(&prk, b"semos-pair-v1 session", &th, &mut session_key);

    // sas_bytes = HKDF-expand(prk, "semos-pair-v1 sas" || th, 4); SAS = mod 1e6
    let mut sas_bytes = [0u8; 4];
    expand_labeled(&prk, b"semos-pair-v1 sas", &th, &mut sas_bytes);
    let sas = u32::from_be_bytes(sas_bytes) % 1_000_000;

    Session { session_key, transcript_hash: th, sas }
}

/// `HKDF-expand(prk, info = label || th, okm)` with the info passed as an
/// EXACT-length slice — no padding, so the Swift `HKDF<SHA256>` side (which
/// follows the spec's `label || th`) derives identical output.
fn expand_labeled(prk: &[u8; 32], label: &[u8], th: &[u8; 32], okm: &mut [u8]) {
    let mut ib = [0u8; 64];
    let l = label.len();
    ib[..l].copy_from_slice(label);
    ib[l..l + 32].copy_from_slice(th);
    let _ = hkdf::expand(prk, &ib[..l + 32], okm);
}

/// The CONFIRM MAC for one direction: `HMAC(session_key, label || th)`.
/// Distinct labels per direction prevent a reflection attack.
pub fn confirm_mac(session_key: &[u8; 32], th: &[u8; 32], from_sem: bool) -> [u8; 32] {
    let label: &[u8] = if from_sem { b"confirm-sem" } else { b"confirm-phone" };
    let mut msg = [0u8; 16 + 32];
    let l = label.len();
    msg[..l].copy_from_slice(label);
    msg[l..l + 32].copy_from_slice(th);
    sha256::hmac(session_key, &msg[..l + 32])
}

/// Stable pairing id: first 8 bytes of `SHA256("semos-pair-id" || phone_pub ||
/// sem_pub)`, rendered as 16 lowercase hex chars into `out` (must be >= 16).
pub fn pairing_id(phone_pub: &[u8; 32], sem_pub: &[u8; 32], out: &mut [u8]) -> usize {
    let mut m = [0u8; 13 + 32 + 32];
    m[..13].copy_from_slice(b"semos-pair-id");
    m[13..45].copy_from_slice(phone_pub);
    m[45..77].copy_from_slice(sem_pub);
    let h = sha256::hash(&m);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for i in 0..8 {
        out[i * 2] = HEX[(h[i] >> 4) as usize];
        out[i * 2 + 1] = HEX[(h[i] & 0xF) as usize];
    }
    16
}

// ============================================================================
// Wire framing
// ============================================================================
//
// Every message is `len: u16 (BE) || type: u8 || body`, where **`len` counts
// the type byte plus the body** (so a 32-byte CONFIRM body has len = 33). This
// matches the companion app's `PairingListener` reader, which pulls a 3-byte
// header then `len - 1` body bytes.

/// SemOS → phone: `version || sem_pub(32) || nonce_s(16)`.
pub const FRAME_HELLO: u8 = 0x01;
/// phone → SemOS: `version`.
pub const FRAME_ACK: u8 = 0x02;
/// Either direction: the 32-byte CONFIRM MAC.
pub const FRAME_CONFIRM: u8 = 0x03;
/// Either direction: advisory abort. Body text is never trusted.
pub const FRAME_ABORT: u8 = 0xFF;

/// Frame header size (`len` + `type`).
pub const FRAME_HEADER_LEN: usize = 3;
/// Largest frame body we will accept (spec caps messages at 128 B).
pub const MAX_FRAME_BODY: usize = 125;

/// Build the HELLO frame into `out`, returning its total length (53).
pub fn build_hello(sem_pub: &[u8; 32], nonce_s: &[u8; NONCE_LEN], out: &mut [u8]) -> usize {
    let body_len = 1 + 32 + NONCE_LEN; // 49
    let total = FRAME_HEADER_LEN + body_len;
    out[0..2].copy_from_slice(&((body_len + 1) as u16).to_be_bytes());
    out[2] = FRAME_HELLO;
    out[3] = VERSION;
    out[4..36].copy_from_slice(sem_pub);
    out[36..36 + NONCE_LEN].copy_from_slice(nonce_s);
    total
}

/// Build a CONFIRM frame carrying `mac`, returning its total length (35).
pub fn build_confirm(mac: &[u8; 32], out: &mut [u8]) -> usize {
    out[0..2].copy_from_slice(&33u16.to_be_bytes());
    out[2] = FRAME_CONFIRM;
    out[3..35].copy_from_slice(mac);
    FRAME_HEADER_LEN + 32
}

/// Parse a frame header, returning `(type, body_len)`. `None` if the declared
/// length is nonsensical or oversized.
pub fn parse_frame_header(hdr: &[u8]) -> Option<(u8, usize)> {
    if hdr.len() < FRAME_HEADER_LEN {
        return None;
    }
    let len = u16::from_be_bytes([hdr[0], hdr[1]]) as usize;
    if len == 0 || len - 1 > MAX_FRAME_BODY {
        return None;
    }
    Some((hdr[2], len - 1))
}

/// Verify a received CONFIRM body against the expected MAC for `from_sem`,
/// in constant time.
pub fn verify_confirm(
    session_key: &[u8; 32],
    th: &[u8; 32],
    from_sem: bool,
    body: &[u8],
) -> bool {
    if body.len() != 32 {
        return false;
    }
    let expected = confirm_mac(session_key, th, from_sem);
    crate::crypto::ct_eq(&expected, body)
}

// ============================================================================
// Stored pairing record (pure byte layout; persistence lives in the platform
// layer so this module stays I/O-free)
// ============================================================================

/// Bytes of a persisted pairing record. magic(2)="PR" ver(1) phone_pub(32)
/// ip(4) port(2) created_at(8, BE) = 49 bytes.
pub const RECORD_LEN: usize = 49;
const RECORD_MAGIC: [u8; 2] = *b"PR";

/// A paired device as stored under `/etc/paired-devices/<id>`.
#[derive(Clone, Copy)]
pub struct PairRecord {
    pub phone_pub: [u8; 32],
    pub ip: [u8; 4],
    pub port: u16,
    pub created_at: u64,
}

impl PairRecord {
    pub fn to_bytes(&self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        b[0..2].copy_from_slice(&RECORD_MAGIC);
        b[2] = VERSION;
        b[3..35].copy_from_slice(&self.phone_pub);
        b[35..39].copy_from_slice(&self.ip);
        b[39..41].copy_from_slice(&self.port.to_be_bytes());
        b[41..49].copy_from_slice(&self.created_at.to_be_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() != RECORD_LEN || b[0..2] != RECORD_MAGIC || b[2] != VERSION {
            return None;
        }
        let mut phone_pub = [0u8; 32];
        phone_pub.copy_from_slice(&b[3..35]);
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&b[35..39]);
        let port = u16::from_be_bytes([b[39], b[40]]);
        let mut ca = [0u8; 8];
        ca.copy_from_slice(&b[41..49]);
        Some(Self { phone_pub, ip, port, created_at: u64::from_be_bytes(ca) })
    }
}
