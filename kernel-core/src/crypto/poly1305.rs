//! Poly1305 Message Authentication Code (RFC 8439).
//!
//! Used as the authentication half of ChaCha20-Poly1305 AEAD in TLS 1.3.
//!
//! # Algorithm
//!
//! This is the "donna 32" approach (Andrew Moon, public domain) — the
//! Poly1305 accumulator and r-key are each stored as **5 × 26-bit limbs**.
//! That layout makes carries cheap on a 32-bit (or 64-bit) CPU without
//! needing 128-bit arithmetic for multiplication, and the modular
//! reduction folds naturally out of `2^130 ≡ 5 (mod p)`.
//!
//! Why this layout and not the obvious 2 × 64-bit limbs:
//! - With 64-bit limbs you need `u128` for every partial product, and
//!   the reduction is brittle (lots of carry chains that are easy to
//!   get wrong by one bit). An earlier version of this file used that
//!   layout and computed wrong tags on inputs longer than a few blocks
//!   — caught when ChaCha20-Poly1305 AEAD KAT against RFC 8439 §2.8.2
//!   diverged from the published tag.
//! - 5 × 26-bit fits in `u64` per product (each limb is ≤ 26 bits, so
//!   `26 + 26 = 52` is well under 64) and the carry handling is simple
//!   row-by-row.
//!
//! # Discipline
//!
//! - `r` is clamped per spec at `new()`. Once set it doesn't change.
//! - `h` starts at 0. Each call to `block()` does `h = (h + msg) * r mod p`.
//! - `finalize()` does the strong reduction (so the result is < p) and
//!   adds `s` mod 2^128 to produce the 16-byte tag.
//!
//! # Tests
//!
//! See [`tests`] for the RFC 8439 §2.5.2 single-block KAT plus the
//! multi-block round-trip via `aead_encrypt` / `aead_decrypt`. The
//! authoritative end-to-end check is `crate::tls::crypto_shim::
//! run_rfc8439_aead_kat`, which exercises the full §2.8.2 ChaCha20-
//! Poly1305 vector through the embedded-tls trait surface.

use super::{CryptoKey, Nonce, CryptoResult, CryptoError, TAG_SIZE};
use super::chacha20::ChaCha20;

/// Poly1305 authentication tag size.
pub const POLY1305_TAG_SIZE: usize = 16;

/// Poly1305 key size (16-byte r || 16-byte s).
pub const POLY1305_KEY_SIZE: usize = 32;

/// Poly1305 authenticator state.
///
/// Internal layout: `r` and `h` as five 26-bit limbs stored in `u64`
/// (the upper 38 bits stay zero during accumulation and absorb
/// partial-product carries during multiply). `s` is held as four
/// 32-bit limbs because finalize adds it as a 128-bit integer mod 2^128.
pub struct Poly1305 {
    /// r-key, clamped per spec, as 5 × 26-bit limbs.
    r: [u64; 5],
    /// (5 * r[1..5]) precomputed for the multiply step. Lets the
    /// schoolbook product fold the over-2^130 limbs back into the low
    /// 130 bits without a separate reduction pass.
    r5: [u64; 4],
    /// Running accumulator, 5 × 26-bit limbs.
    h: [u64; 5],
    /// s-key (added to h mod 2^128 at finalize), 4 × 32-bit limbs.
    s: [u32; 4],
    /// Partial-block buffer (input may not arrive in 16-byte chunks).
    buffer: [u8; 16],
    /// Bytes currently held in `buffer`.
    buffer_len: usize,
}

impl Poly1305 {
    /// Create a new Poly1305 from a 32-byte one-time key. Performs the
    /// spec-mandated clamp on the r half.
    pub fn new(key: &[u8; 32]) -> Self {
        // Read r as a little-endian 128-bit integer, clamp, then split
        // into 5 × 26-bit limbs.
        //
        // The clamp clears specific bits to constrain r so the
        // multiply by h stays inside the safe range:
        //   r &= 0x0ffffffc_0ffffffc_0ffffffc_0fffffff
        // (i.e. top 4 bits of each u32 limb zero; low 2 bits of three
        //  of them zero — matches the RFC 8439 §2.5.1 specification.)
        let r0 = u32::from_le_bytes([key[ 0], key[ 1], key[ 2], key[ 3]]) & 0x0fff_ffff;
        let r1 = u32::from_le_bytes([key[ 4], key[ 5], key[ 6], key[ 7]]) & 0x0fff_fffc;
        let r2 = u32::from_le_bytes([key[ 8], key[ 9], key[10], key[11]]) & 0x0fff_fffc;
        let r3 = u32::from_le_bytes([key[12], key[13], key[14], key[15]]) & 0x0fff_fffc;

        // Pack the 128-bit r value into 5 × 26-bit limbs (little-endian
        // by limb, low limb first).
        //   r_0  bits   0.. 25  = low 26 bits of r0
        //   r_1  bits  26.. 51  = top 6 of r0 || low 20 of r1
        //   r_2  bits  52.. 77  = top 12 of r1 || low 14 of r2
        //   r_3  bits  78..103  = top 18 of r2 || low 8 of r3
        //   r_4  bits 104..129  = top 24 of r3 (we treat r as 130-bit
        //                         with the implied 0 bits at top)
        let lr0 = (r0 & 0x03ff_ffff) as u64;
        let lr1 = (((r0 >> 26) | (r1 << 6)) & 0x03ff_ffff) as u64;
        let lr2 = (((r1 >> 20) | (r2 << 12)) & 0x03ff_ffff) as u64;
        let lr3 = (((r2 >> 14) | (r3 << 18)) & 0x03ff_ffff) as u64;
        let lr4 = ((r3 >> 8) & 0x03ff_ffff) as u64;

        // Precompute 5*r[1..4] for the multiply (folds the over-2^130
        // limbs back via 2^130 ≡ 5 (mod p)).
        Self {
            r: [lr0, lr1, lr2, lr3, lr4],
            r5: [(lr1 * 5), (lr2 * 5), (lr3 * 5), (lr4 * 5)],
            h: [0; 5],
            s: [
                u32::from_le_bytes([key[16], key[17], key[18], key[19]]),
                u32::from_le_bytes([key[20], key[21], key[22], key[23]]),
                u32::from_le_bytes([key[24], key[25], key[26], key[27]]),
                u32::from_le_bytes([key[28], key[29], key[30], key[31]]),
            ],
            buffer: [0u8; 16],
            buffer_len: 0,
        }
    }

    /// Absorb one 16-byte block. `pad_byte` is 0x01 for normal blocks
    /// (becomes the implicit "1" bit appended at bit 128 of the message
    /// representation, per RFC 8439 §2.5.1) and 0x00 only for the
    /// special last-partial-block path which inserts the 0x01 directly
    /// into the buffer.
    fn absorb_block(&mut self, block: &[u8; 16], pad_byte: u8) {
        // Read the block as five 26-bit limbs of the message, with the
        // high pad byte placed at the bit-128 position.
        let m0 = u32::from_le_bytes([block[ 0], block[ 1], block[ 2], block[ 3]]);
        let m1 = u32::from_le_bytes([block[ 4], block[ 5], block[ 6], block[ 7]]);
        let m2 = u32::from_le_bytes([block[ 8], block[ 9], block[10], block[11]]);
        let m3 = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);

        let h0 = self.h[0] + ((m0 & 0x03ff_ffff) as u64);
        let h1 = self.h[1] + (((m0 >> 26) | (m1 << 6)) as u64 & 0x03ff_ffff);
        let h2 = self.h[2] + (((m1 >> 20) | (m2 << 12)) as u64 & 0x03ff_ffff);
        let h3 = self.h[3] + (((m2 >> 14) | (m3 << 18)) as u64 & 0x03ff_ffff);
        // High limb gets the pad byte at bit (128 - 104) = 24 of the limb.
        let h4 = self.h[4] + ((m3 >> 8) as u64 | ((pad_byte as u64) << 24));

        // Multiply h by r mod (2^130 - 5). Each d_i below is the i-th
        // 26-bit limb of (h * r). The "5 * r_j" entries fold limbs
        // h_i * r_j with (i+j) >= 5 back into the low 5 limbs.
        let r = &self.r;
        let s = &self.r5; // s[i] = 5 * r[i+1]
        let d0 = h0 * r[0] + h1 * s[3] + h2 * s[2] + h3 * s[1] + h4 * s[0];
        let d1 = h0 * r[1] + h1 * r[0] + h2 * s[3] + h3 * s[2] + h4 * s[1];
        let d2 = h0 * r[2] + h1 * r[1] + h2 * r[0] + h3 * s[3] + h4 * s[2];
        let d3 = h0 * r[3] + h1 * r[2] + h2 * r[1] + h3 * r[0] + h4 * s[3];
        let d4 = h0 * r[4] + h1 * r[3] + h2 * r[2] + h3 * r[1] + h4 * r[0];

        // Propagate carries between limbs. Each d_i is < 2^64; after
        // carrying out the upper bits each limb fits in 26 bits.
        let mut c: u64;
        let h0 = d0 & 0x03ff_ffff;            c = d0 >> 26;
        let d1 = d1 + c;
        let h1 = d1 & 0x03ff_ffff;            c = d1 >> 26;
        let d2 = d2 + c;
        let h2 = d2 & 0x03ff_ffff;            c = d2 >> 26;
        let d3 = d3 + c;
        let h3 = d3 & 0x03ff_ffff;            c = d3 >> 26;
        let d4 = d4 + c;
        let h4 = d4 & 0x03ff_ffff;            c = d4 >> 26;
        // The carry out of h4 is at bit 130+; fold it back via *5.
        let h0 = h0 + c * 5;
        let h1 = h1 + (h0 >> 26);
        let h0 = h0 & 0x03ff_ffff;

        self.h = [h0, h1, h2, h3, h4];
    }

    /// Absorb arbitrary input. Buffers partial blocks across calls.
    pub fn update(&mut self, mut data: &[u8]) {
        // Top off any leftover from a previous partial call.
        if self.buffer_len > 0 {
            let take = (16 - self.buffer_len).min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take]
                .copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len == 16 {
                let block = self.buffer;
                self.absorb_block(&block, 0x01);
                self.buffer_len = 0;
            }
        }

        // Process full 16-byte blocks straight from the input.
        while data.len() >= 16 {
            let mut block = [0u8; 16];
            block.copy_from_slice(&data[..16]);
            self.absorb_block(&block, 0x01);
            data = &data[16..];
        }

        // Buffer the final partial chunk (if any) for the next call
        // or for finalize.
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    /// Finish the MAC and produce the 16-byte tag.
    pub fn finalize(mut self) -> [u8; POLY1305_TAG_SIZE] {
        // Flush the partial buffer (if any) by appending the 0x01
        // marker AT the end of the message bytes, then zero-padding
        // to 16 bytes. The marker is part of the message value, not
        // a "pad bit" added afterwards, so we call absorb_block with
        // pad_byte=0 (the marker is in the buffer already).
        if self.buffer_len > 0 {
            self.buffer[self.buffer_len] = 0x01;
            for i in self.buffer_len + 1..16 { self.buffer[i] = 0; }
            let block = self.buffer;
            self.absorb_block(&block, 0x00);
        }

        // Final carry propagation: each limb might be slightly over
        // 26 bits from the last accumulation; fix that.
        let mut h0 = self.h[0];
        let mut h1 = self.h[1] + (h0 >> 26); h0 &= 0x03ff_ffff;
        let mut h2 = self.h[2] + (h1 >> 26); h1 &= 0x03ff_ffff;
        let mut h3 = self.h[3] + (h2 >> 26); h2 &= 0x03ff_ffff;
        let mut h4 = self.h[4] + (h3 >> 26); h3 &= 0x03ff_ffff;
        h0 = h0 + 5 * (h4 >> 26);
        h4 &= 0x03ff_ffff;
        h1 = h1 + (h0 >> 26); h0 &= 0x03ff_ffff;

        // Strong reduction: compute g = h + 5; if g >> 130 == 1 use g,
        // else use h. This canonicalises h to be < p exactly.
        let g0 = h0 + 5;
        let g1 = h1 + (g0 >> 26);
        let g2 = h2 + (g1 >> 26);
        let g3 = h3 + (g2 >> 26);
        let g4 = h4.wrapping_add(g3 >> 26).wrapping_sub(1 << 26);
        let (g0, g1, g2, g3, g4) = (g0 & 0x03ff_ffff, g1 & 0x03ff_ffff, g2 & 0x03ff_ffff, g3 & 0x03ff_ffff, g4);

        // Constant-time select: mask = 0 if g4 has the borrow bit (i.e.
        // g overflowed bit 130, meaning h was < p — keep h), all-ones
        // otherwise (use g).
        let mask = (g4 >> 63).wrapping_sub(1);
        let h0 = (h0 & !mask) | (g0 & mask);
        let h1 = (h1 & !mask) | (g1 & mask);
        let h2 = (h2 & !mask) | (g2 & mask);
        let h3 = (h3 & !mask) | (g3 & mask);
        let h4 = (h4 & !mask) | (g4 & mask);

        // Repack the 5 × 26-bit limbs into 4 × 32-bit, then add s mod 2^128.
        let h0_32 = (h0 | (h1 << 26)) as u32;
        let h1_32 = ((h1 >> 6) | (h2 << 20)) as u32;
        let h2_32 = ((h2 >> 12) | (h3 << 14)) as u32;
        let h3_32 = ((h3 >> 18) | (h4 << 8)) as u32;

        let f = (h0_32 as u64) + (self.s[0] as u64);
        let h0_32 = f as u32;
        let f = (h1_32 as u64) + (self.s[1] as u64) + (f >> 32);
        let h1_32 = f as u32;
        let f = (h2_32 as u64) + (self.s[2] as u64) + (f >> 32);
        let h2_32 = f as u32;
        let f = (h3_32 as u64) + (self.s[3] as u64) + (f >> 32);
        let h3_32 = f as u32;

        let mut tag = [0u8; POLY1305_TAG_SIZE];
        tag[ 0.. 4].copy_from_slice(&h0_32.to_le_bytes());
        tag[ 4.. 8].copy_from_slice(&h1_32.to_le_bytes());
        tag[ 8..12].copy_from_slice(&h2_32.to_le_bytes());
        tag[12..16].copy_from_slice(&h3_32.to_le_bytes());
        tag
    }
}

/// One-shot Poly1305 MAC. Equivalent to `Poly1305::new(key).update(msg).finalize()`.
pub fn poly1305_mac(key: &[u8; 32], message: &[u8]) -> [u8; 16] {
    let mut p = Poly1305::new(key);
    p.update(message);
    p.finalize()
}

// ============================================================================
// ChaCha20-Poly1305 AEAD (RFC 8439 §2.8)
// ============================================================================

/// Encrypt `plaintext` into `ciphertext` and write the 16-byte tag.
/// AAD is authenticated but not encrypted.
pub fn aead_encrypt(
    key: &CryptoKey,
    nonce: &Nonce,
    aad: &[u8],
    plaintext: &[u8],
    ciphertext: &mut [u8],
    tag: &mut [u8; TAG_SIZE],
) -> CryptoResult<()> {
    if ciphertext.len() < plaintext.len() {
        return Err(CryptoError::BufferTooSmall);
    }

    // Derive Poly1305 one-time key from ChaCha20 block 0.
    let mut poly_key = [0u8; 32];
    {
        let cipher = ChaCha20::new(key, nonce, 0);
        let block = cipher.block();
        poly_key.copy_from_slice(&block[..32]);
    }

    // Encrypt plaintext starting at ChaCha20 counter 1.
    ciphertext[..plaintext.len()].copy_from_slice(plaintext);
    super::chacha20::chacha20_xor(key, nonce, 1, &mut ciphertext[..plaintext.len()]);

    // Authenticate: aad || pad16, ciphertext || pad16, aad_len_le_u64 || ct_len_le_u64.
    let mut poly = Poly1305::new(&poly_key);
    poly.update(aad);
    if aad.len() % 16 != 0 {
        let pad = [0u8; 16];
        poly.update(&pad[..16 - (aad.len() % 16)]);
    }
    poly.update(&ciphertext[..plaintext.len()]);
    if plaintext.len() % 16 != 0 {
        let pad = [0u8; 16];
        poly.update(&pad[..16 - (plaintext.len() % 16)]);
    }
    let mut lengths = [0u8; 16];
    lengths[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lengths[8..16].copy_from_slice(&(plaintext.len() as u64).to_le_bytes());
    poly.update(&lengths);
    *tag = poly.finalize();

    // Wipe one-time poly key.
    for b in &mut poly_key { unsafe { core::ptr::write_volatile(b, 0); } }
    Ok(())
}

/// Decrypt `ciphertext` into `plaintext` after verifying `tag`. Returns
/// `Err(AuthenticationFailed)` on tag mismatch (constant-time compare).
pub fn aead_decrypt(
    key: &CryptoKey,
    nonce: &Nonce,
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8; TAG_SIZE],
    plaintext: &mut [u8],
) -> CryptoResult<()> {
    if plaintext.len() < ciphertext.len() {
        return Err(CryptoError::BufferTooSmall);
    }

    let mut poly_key = [0u8; 32];
    {
        let cipher = ChaCha20::new(key, nonce, 0);
        let block = cipher.block();
        poly_key.copy_from_slice(&block[..32]);
    }

    let mut poly = Poly1305::new(&poly_key);
    poly.update(aad);
    if aad.len() % 16 != 0 {
        let pad = [0u8; 16];
        poly.update(&pad[..16 - (aad.len() % 16)]);
    }
    poly.update(ciphertext);
    if ciphertext.len() % 16 != 0 {
        let pad = [0u8; 16];
        poly.update(&pad[..16 - (ciphertext.len() % 16)]);
    }
    let mut lengths = [0u8; 16];
    lengths[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lengths[8..16].copy_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    poly.update(&lengths);
    let expected = poly.finalize();

    let mut diff: u8 = 0;
    for i in 0..16 { diff |= expected[i] ^ tag[i]; }
    for b in &mut poly_key { unsafe { core::ptr::write_volatile(b, 0); } }
    if diff != 0 { return Err(CryptoError::AuthenticationFailed); }

    plaintext[..ciphertext.len()].copy_from_slice(ciphertext);
    super::chacha20::chacha20_xor(key, nonce, 1, &mut plaintext[..ciphertext.len()]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8439 §2.5.2 — Poly1305 single-message test vector.
    /// key = sum of clamped r (16 bytes) and s (16 bytes)
    /// msg = "Cryptographic Forum Research Group" (34 bytes)
    /// tag = a8061dc1305136c6c22b8baf0c0127a9
    #[test]
    fn rfc8439_2_5_2_single_block() {
        let key: [u8; 32] = [
            0x85, 0xd6, 0xbe, 0x78, 0x57, 0x55, 0x6d, 0x33,
            0x7f, 0x44, 0x52, 0xfe, 0x42, 0xd5, 0x06, 0xa8,
            0x01, 0x03, 0x80, 0x8a, 0xfb, 0x0d, 0xb2, 0xfd,
            0x4a, 0xbf, 0xf6, 0xaf, 0x41, 0x49, 0xf5, 0x1b,
        ];
        let msg = b"Cryptographic Forum Research Group";
        let expected: [u8; 16] = [
            0xa8, 0x06, 0x1d, 0xc1, 0x30, 0x51, 0x36, 0xc6,
            0xc2, 0x2b, 0x8b, 0xaf, 0x0c, 0x01, 0x27, 0xa9,
        ];
        assert_eq!(poly1305_mac(&key, msg), expected);
    }

    #[test]
    fn test_aead_roundtrip() {
        let key = CryptoKey::from_bytes([0x42u8; 32]);
        let nonce = Nonce::from_bytes([0x07u8; 12]);
        let aad = b"authenticated data";
        let plaintext = b"secret message for Semantic OS";
        let mut ct = [0u8; 64];
        let mut tag = [0u8; 16];
        let mut dec = [0u8; 64];
        aead_encrypt(&key, &nonce, aad, plaintext, &mut ct, &mut tag).unwrap();
        aead_decrypt(&key, &nonce, aad, &ct[..plaintext.len()], &tag, &mut dec).unwrap();
        assert_eq!(&dec[..plaintext.len()], plaintext);
    }

    #[test]
    fn test_aead_tamper_detection() {
        let key = CryptoKey::from_bytes([1u8; 32]);
        let nonce = Nonce::from_bytes([2u8; 12]);
        let aad = b"aad";
        let plaintext = b"data";
        let mut ct = [0u8; 16];
        let mut tag = [0u8; 16];
        let mut dec = [0u8; 16];
        aead_encrypt(&key, &nonce, aad, plaintext, &mut ct, &mut tag).unwrap();
        ct[0] ^= 1;
        let result = aead_decrypt(&key, &nonce, aad, &ct[..plaintext.len()], &tag, &mut dec);
        assert_eq!(result, Err(CryptoError::AuthenticationFailed));
    }
}
