//! Poly1305 Message Authentication Code
//!
//! Implements Poly1305 MAC as defined in RFC 8439.
//! Used as the authentication component of ChaCha20-Poly1305 AEAD.
//!
//! # Security
//!
//! - 128-bit one-time key (derived from ChaCha20 keystream)
//! - 128-bit authentication tag
//! - Key MUST be unique for each message (nonce-derived)

use super::{CryptoKey, Nonce, CryptoResult, CryptoError, TAG_SIZE};
use super::chacha20::{ChaCha20, BLOCK_SIZE};

/// Poly1305 authentication tag size
pub const POLY1305_TAG_SIZE: usize = 16;

/// Poly1305 key size (r || s)
pub const POLY1305_KEY_SIZE: usize = 32;

/// Poly1305 authenticator state
pub struct Poly1305 {
    /// Accumulator (h)
    h: [u64; 3],
    /// r key (clamped)
    r: [u64; 2],
    /// s key (for final addition)
    s: [u64; 2],
    /// Partial block buffer
    buffer: [u8; 16],
    /// Bytes in buffer
    buffer_len: usize,
}

impl Poly1305 {
    /// Create a new Poly1305 instance with 32-byte key
    pub fn new(key: &[u8; 32]) -> Self {
        // Split key into r (first 16 bytes) and s (last 16 bytes)
        let mut r0 = u64::from_le_bytes([
            key[0], key[1], key[2], key[3],
            key[4], key[5], key[6], key[7],
        ]);
        let mut r1 = u64::from_le_bytes([
            key[8], key[9], key[10], key[11],
            key[12], key[13], key[14], key[15],
        ]);

        // Clamp r (required by spec)
        // Clear bits 4, 6, 7 of r[3], r[7], r[11], r[15]
        // Clear bits 0, 1 of r[4], r[8], r[12]
        r0 &= 0x0ffffffc0fffffff;
        r1 &= 0x0ffffffc0ffffffc;

        let s0 = u64::from_le_bytes([
            key[16], key[17], key[18], key[19],
            key[20], key[21], key[22], key[23],
        ]);
        let s1 = u64::from_le_bytes([
            key[24], key[25], key[26], key[27],
            key[28], key[29], key[30], key[31],
        ]);

        Self {
            h: [0, 0, 0],
            r: [r0, r1],
            s: [s0, s1],
            buffer: [0u8; 16],
            buffer_len: 0,
        }
    }

    /// Process a 16-byte block
    fn block(&mut self, data: &[u8], is_final_block: bool) {
        // Read block as two 64-bit little-endian values
        let mut n = [0u64; 2];

        if data.len() >= 8 {
            n[0] = u64::from_le_bytes([
                data[0], data[1], data[2], data[3],
                data[4], data[5], data[6], data[7],
            ]);
        } else {
            let mut bytes = [0u8; 8];
            bytes[..data.len().min(8)].copy_from_slice(&data[..data.len().min(8)]);
            n[0] = u64::from_le_bytes(bytes);
        }

        if data.len() >= 16 {
            n[1] = u64::from_le_bytes([
                data[8], data[9], data[10], data[11],
                data[12], data[13], data[14], data[15],
            ]);
        } else if data.len() > 8 {
            let mut bytes = [0u8; 8];
            let remaining = data.len() - 8;
            bytes[..remaining].copy_from_slice(&data[8..]);
            n[1] = u64::from_le_bytes(bytes);
        }

        // Add high bit if not final partial block
        let hibit: u64 = if is_final_block && data.len() < 16 { 0 } else { 1 };

        // h += m (with high bit)
        let h0 = self.h[0].wrapping_add(n[0]);
        let h1 = self.h[1].wrapping_add(n[1]);
        let h2 = self.h[2].wrapping_add(hibit);

        // Use 128-bit arithmetic for h *= r mod 2^130 - 5
        // This is a simplified implementation using 64-bit operations
        self.multiply_and_reduce(h0, h1, h2);
    }

    /// Multiply h by r and reduce mod 2^130 - 5
    /// Uses schoolbook multiplication with 64-bit limbs
    fn multiply_and_reduce(&mut self, h0: u64, h1: u64, h2: u64) {
        let r0 = self.r[0];
        let r1 = self.r[1];

        // Compute h * r using 128-bit products
        // h = h0 + h1*2^64 + h2*2^128
        // r = r0 + r1*2^64

        // We need to compute (h0 + h1*2^64 + h2*2^128) * (r0 + r1*2^64) mod 2^130-5

        // Use 64-bit limbs with carry propagation
        let (d0, c0) = mul64(h0, r0);
        let (d1a, c1a) = mul64(h0, r1);
        let (d1b, c1b) = mul64(h1, r0);
        let (d2a, c2a) = mul64(h1, r1);

        // h2 is at most 4 bits, so these won't overflow
        let d2b = h2.wrapping_mul(r0);
        let d3 = h2.wrapping_mul(r1);

        // Accumulate limbs
        let (d1, carry1) = d1a.overflowing_add(d1b);
        let (d1, carry1b) = d1.overflowing_add(c0);

        let (d2, carry2) = d2a.overflowing_add(d2b);
        let (d2, carry2b) = d2.overflowing_add(c1a);
        let (d2, carry2c) = d2.overflowing_add(c1b);
        let (d2, carry2d) = d2.overflowing_add(if carry1 { 1 } else { 0 });
        let (d2, carry2e) = d2.overflowing_add(if carry1b { 1 } else { 0 });

        let d3 = d3
            .wrapping_add(c2a)
            .wrapping_add(if carry2 { 1 } else { 0 })
            .wrapping_add(if carry2b { 1 } else { 0 })
            .wrapping_add(if carry2c { 1 } else { 0 })
            .wrapping_add(if carry2d { 1 } else { 0 })
            .wrapping_add(if carry2e { 1 } else { 0 });

        // Reduce mod 2^130 - 5
        // d = d0 + d1*2^64 + d2*2^128 + d3*2^192
        // 2^130 = 5 mod (2^130 - 5)
        // So d2*2^128 + d3*2^192 = (d2 + d3*2^64) * 2^128
        //                        = (d2 + d3*2^64) * 4 * 2^126
        //                        = (d2 + d3*2^64) * 4 * 5 * 2^(-4) (approximately)

        // Simplified reduction:
        // h2 = d2 & 3 (keep 2 bits)
        // carry = d2 >> 2 (bits above 130)
        // Add carry * 5 back to low bits

        let h2_new = d2 & 3;
        let carry = (d2 >> 2) | (d3 << 62);
        let carry5 = carry.wrapping_mul(5);

        let (h0_new, c) = d0.overflowing_add(carry5);
        let h1_new = d1.wrapping_add(if c { 1 } else { 0 });
        let h2_final = h2_new.wrapping_add((d3 >> 2).wrapping_mul(5));

        self.h[0] = h0_new;
        self.h[1] = h1_new;
        self.h[2] = h2_final & 7; // Keep only 3 bits for h2
    }

    /// Update with additional data
    pub fn update(&mut self, data: &[u8]) {
        let mut offset = 0;

        // Process any buffered data first
        if self.buffer_len > 0 {
            let to_copy = (16 - self.buffer_len).min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + to_copy]
                .copy_from_slice(&data[..to_copy]);
            self.buffer_len += to_copy;
            offset = to_copy;

            if self.buffer_len == 16 {
                let block = self.buffer;
                self.block(&block, false);
                self.buffer_len = 0;
            }
        }

        // Process full blocks
        while offset + 16 <= data.len() {
            self.block(&data[offset..offset + 16], false);
            offset += 16;
        }

        // Buffer remaining bytes
        if offset < data.len() {
            let remaining = data.len() - offset;
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    /// Finalize and produce the authentication tag
    pub fn finalize(mut self) -> [u8; POLY1305_TAG_SIZE] {
        // Process final partial block if any
        if self.buffer_len > 0 {
            // Pad with 0x01 and zeros
            self.buffer[self.buffer_len] = 0x01;
            for i in self.buffer_len + 1..16 {
                self.buffer[i] = 0;
            }
            let block = self.buffer;
            self.block(&block[..self.buffer_len], true);
        }

        // Final reduction
        // Compute h mod 2^130 - 5 (fully reduced)
        let mut h0 = self.h[0];
        let mut h1 = self.h[1];
        let mut h2 = self.h[2];

        // Propagate carries from h2 (bits above 128)
        // h2 can have bits above position 2 that need to be folded back
        let c = h2 >> 2;
        h2 &= 3;

        // Add c * 5 to h0, using 128-bit arithmetic to handle overflow
        let sum = h0 as u128 + (c.wrapping_mul(5)) as u128;
        h0 = sum as u64;
        let carry = (sum >> 64) as u64;
        h1 = h1.wrapping_add(carry);

        // Compute h + s
        let (t0, c) = h0.overflowing_add(self.s[0]);
        let t1 = h1.wrapping_add(self.s[1]).wrapping_add(if c { 1 } else { 0 });

        // Output tag
        let mut tag = [0u8; 16];
        tag[0..8].copy_from_slice(&t0.to_le_bytes());
        tag[8..16].copy_from_slice(&t1.to_le_bytes());

        tag
    }
}

/// 64-bit multiplication returning 128-bit result as (low, high)
#[inline(always)]
fn mul64(a: u64, b: u64) -> (u64, u64) {
    let result = (a as u128) * (b as u128);
    (result as u64, (result >> 64) as u64)
}

/// One-shot Poly1305 MAC
pub fn poly1305_mac(key: &[u8; 32], message: &[u8]) -> [u8; 16] {
    let mut poly = Poly1305::new(key);
    poly.update(message);
    poly.finalize()
}

/// ChaCha20-Poly1305 AEAD encryption
///
/// Encrypts plaintext and produces authentication tag.
/// Additional authenticated data (AAD) is authenticated but not encrypted.
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

    // Generate Poly1305 key from ChaCha20 block 0
    let mut poly_key = [0u8; 32];
    let cipher = ChaCha20::new(key, nonce, 0);
    let block = cipher.block();
    poly_key.copy_from_slice(&block[..32]);

    // Encrypt plaintext with ChaCha20 starting at block 1
    ciphertext[..plaintext.len()].copy_from_slice(plaintext);
    super::chacha20::chacha20_xor(key, nonce, 1, &mut ciphertext[..plaintext.len()]);

    // Compute Poly1305 tag over AAD and ciphertext
    let mut poly = Poly1305::new(&poly_key);

    // Authenticate AAD with padding
    poly.update(aad);
    if aad.len() % 16 != 0 {
        let padding = [0u8; 16];
        poly.update(&padding[..16 - (aad.len() % 16)]);
    }

    // Authenticate ciphertext with padding
    poly.update(&ciphertext[..plaintext.len()]);
    if plaintext.len() % 16 != 0 {
        let padding = [0u8; 16];
        poly.update(&padding[..16 - (plaintext.len() % 16)]);
    }

    // Authenticate lengths (as 64-bit little-endian)
    let mut lengths = [0u8; 16];
    lengths[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lengths[8..16].copy_from_slice(&(plaintext.len() as u64).to_le_bytes());
    poly.update(&lengths);

    // Get tag
    *tag = poly.finalize();

    // Zeroize poly key
    for b in &mut poly_key {
        unsafe { core::ptr::write_volatile(b, 0); }
    }

    Ok(())
}

/// ChaCha20-Poly1305 AEAD decryption
///
/// Decrypts ciphertext and verifies authentication tag.
/// Returns error if authentication fails (tampered data).
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

    // Generate Poly1305 key from ChaCha20 block 0
    let mut poly_key = [0u8; 32];
    let cipher = ChaCha20::new(key, nonce, 0);
    let block = cipher.block();
    poly_key.copy_from_slice(&block[..32]);

    // Compute expected tag
    let mut poly = Poly1305::new(&poly_key);

    // Authenticate AAD with padding
    poly.update(aad);
    if aad.len() % 16 != 0 {
        let padding = [0u8; 16];
        poly.update(&padding[..16 - (aad.len() % 16)]);
    }

    // Authenticate ciphertext with padding
    poly.update(ciphertext);
    if ciphertext.len() % 16 != 0 {
        let padding = [0u8; 16];
        poly.update(&padding[..16 - (ciphertext.len() % 16)]);
    }

    // Authenticate lengths
    let mut lengths = [0u8; 16];
    lengths[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lengths[8..16].copy_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    poly.update(&lengths);

    let expected_tag = poly.finalize();

    // Constant-time tag comparison
    let mut diff = 0u8;
    for (a, b) in expected_tag.iter().zip(tag.iter()) {
        diff |= a ^ b;
    }

    // Zeroize poly key
    for b in &mut poly_key {
        unsafe { core::ptr::write_volatile(b, 0); }
    }

    if diff != 0 {
        return Err(CryptoError::AuthenticationFailed);
    }

    // Decrypt ciphertext
    plaintext[..ciphertext.len()].copy_from_slice(ciphertext);
    super::chacha20::chacha20_xor(key, nonce, 1, &mut plaintext[..ciphertext.len()]);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aead_roundtrip() {
        let key = CryptoKey::from_bytes([
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
            0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f,
            0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
            0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f,
        ]);
        let nonce = Nonce::from_bytes([
            0x07, 0x00, 0x00, 0x00, 0x40, 0x41, 0x42, 0x43,
            0x44, 0x45, 0x46, 0x47,
        ]);

        let aad = b"authenticated data";
        let plaintext = b"secret message for Semantic OS";

        let mut ciphertext = [0u8; 64];
        let mut tag = [0u8; 16];
        let mut decrypted = [0u8; 64];

        // Encrypt
        aead_encrypt(&key, &nonce, aad, plaintext, &mut ciphertext, &mut tag).unwrap();

        // Decrypt
        aead_decrypt(&key, &nonce, aad, &ciphertext[..plaintext.len()], &tag, &mut decrypted).unwrap();

        assert_eq!(&decrypted[..plaintext.len()], plaintext);
    }

    #[test]
    fn test_aead_tamper_detection() {
        let key = CryptoKey::from_bytes([1u8; 32]);
        let nonce = Nonce::from_bytes([2u8; 12]);

        let aad = b"aad";
        let plaintext = b"data";

        let mut ciphertext = [0u8; 16];
        let mut tag = [0u8; 16];
        let mut decrypted = [0u8; 16];

        aead_encrypt(&key, &nonce, aad, plaintext, &mut ciphertext, &mut tag).unwrap();

        // Tamper with ciphertext
        ciphertext[0] ^= 1;

        // Decryption should fail
        let result = aead_decrypt(&key, &nonce, aad, &ciphertext[..plaintext.len()], &tag, &mut decrypted);
        assert_eq!(result, Err(CryptoError::AuthenticationFailed));
    }
}
