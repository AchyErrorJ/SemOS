//! ChaCha20 Stream Cipher Implementation
//!
//! Implements the ChaCha20 stream cipher as defined in RFC 8439.
//! Used as the encryption component of ChaCha20-Poly1305 AEAD.
//!
//! # Security
//!
//! - 256-bit key
//! - 96-bit nonce (unique per encryption)
//! - 32-bit block counter
//! - Resistant to timing attacks (constant-time operations)

use super::{CryptoKey, Nonce};

/// ChaCha20 block size in bytes
pub const BLOCK_SIZE: usize = 64;

/// ChaCha20 state (16 x 32-bit words)
#[derive(Clone)]
pub struct ChaCha20 {
    state: [u32; 16],
}

impl ChaCha20 {
    /// ChaCha20 constants ("expand 32-byte k")
    const CONSTANTS: [u32; 4] = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574];

    /// Create a new ChaCha20 cipher instance
    pub fn new(key: &CryptoKey, nonce: &Nonce, counter: u32) -> Self {
        let key_bytes = key.as_bytes();
        let nonce_bytes = nonce.as_bytes();

        let state = [
            Self::CONSTANTS[0],
            Self::CONSTANTS[1],
            Self::CONSTANTS[2],
            Self::CONSTANTS[3],
            u32::from_le_bytes([key_bytes[0], key_bytes[1], key_bytes[2], key_bytes[3]]),
            u32::from_le_bytes([key_bytes[4], key_bytes[5], key_bytes[6], key_bytes[7]]),
            u32::from_le_bytes([key_bytes[8], key_bytes[9], key_bytes[10], key_bytes[11]]),
            u32::from_le_bytes([key_bytes[12], key_bytes[13], key_bytes[14], key_bytes[15]]),
            u32::from_le_bytes([key_bytes[16], key_bytes[17], key_bytes[18], key_bytes[19]]),
            u32::from_le_bytes([key_bytes[20], key_bytes[21], key_bytes[22], key_bytes[23]]),
            u32::from_le_bytes([key_bytes[24], key_bytes[25], key_bytes[26], key_bytes[27]]),
            u32::from_le_bytes([key_bytes[28], key_bytes[29], key_bytes[30], key_bytes[31]]),
            counter,
            u32::from_le_bytes([nonce_bytes[0], nonce_bytes[1], nonce_bytes[2], nonce_bytes[3]]),
            u32::from_le_bytes([nonce_bytes[4], nonce_bytes[5], nonce_bytes[6], nonce_bytes[7]]),
            u32::from_le_bytes([nonce_bytes[8], nonce_bytes[9], nonce_bytes[10], nonce_bytes[11]]),
        ];

        Self { state }
    }

    /// Generate a keystream block
    pub fn block(&self) -> [u8; BLOCK_SIZE] {
        let mut working = self.state;

        // 20 rounds (10 double-rounds)
        for _ in 0..10 {
            // Column rounds
            Self::quarter_round(&mut working, 0, 4, 8, 12);
            Self::quarter_round(&mut working, 1, 5, 9, 13);
            Self::quarter_round(&mut working, 2, 6, 10, 14);
            Self::quarter_round(&mut working, 3, 7, 11, 15);
            // Diagonal rounds
            Self::quarter_round(&mut working, 0, 5, 10, 15);
            Self::quarter_round(&mut working, 1, 6, 11, 12);
            Self::quarter_round(&mut working, 2, 7, 8, 13);
            Self::quarter_round(&mut working, 3, 4, 9, 14);
        }

        // Add original state
        for (w, s) in working.iter_mut().zip(self.state.iter()) {
            *w = w.wrapping_add(*s);
        }

        // Serialize to bytes
        let mut output = [0u8; BLOCK_SIZE];
        for (i, word) in working.iter().enumerate() {
            output[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
        }

        output
    }

    /// ChaCha20 quarter round
    #[inline(always)]
    fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        state[a] = state[a].wrapping_add(state[b]);
        state[d] ^= state[a];
        state[d] = state[d].rotate_left(16);

        state[c] = state[c].wrapping_add(state[d]);
        state[b] ^= state[c];
        state[b] = state[b].rotate_left(12);

        state[a] = state[a].wrapping_add(state[b]);
        state[d] ^= state[a];
        state[d] = state[d].rotate_left(8);

        state[c] = state[c].wrapping_add(state[d]);
        state[b] ^= state[c];
        state[b] = state[b].rotate_left(7);
    }

    /// Increment the counter
    pub fn increment_counter(&mut self) {
        self.state[12] = self.state[12].wrapping_add(1);
    }

    /// Set counter value
    pub fn set_counter(&mut self, counter: u32) {
        self.state[12] = counter;
    }

    /// Get current counter value
    pub fn counter(&self) -> u32 {
        self.state[12]
    }
}

/// Encrypt/decrypt data using ChaCha20
/// (XOR with keystream - encryption and decryption are the same operation)
pub fn chacha20_xor(
    key: &CryptoKey,
    nonce: &Nonce,
    counter: u32,
    data: &mut [u8],
) {
    let mut cipher = ChaCha20::new(key, nonce, counter);
    let mut pos = 0;

    while pos < data.len() {
        let block = cipher.block();
        let remaining = data.len() - pos;
        let to_process = remaining.min(BLOCK_SIZE);

        for i in 0..to_process {
            data[pos + i] ^= block[i];
        }

        pos += to_process;
        cipher.increment_counter();
    }
}

/// Encrypt data in place
pub fn encrypt(key: &CryptoKey, nonce: &Nonce, data: &mut [u8]) {
    chacha20_xor(key, nonce, 1, data);
}

/// Decrypt data in place
pub fn decrypt(key: &CryptoKey, nonce: &Nonce, data: &mut [u8]) {
    chacha20_xor(key, nonce, 1, data);
}

/// Generate keystream (for Poly1305 key derivation)
pub fn keystream(key: &CryptoKey, nonce: &Nonce, counter: u32, len: usize) -> [u8; 64] {
    let cipher = ChaCha20::new(key, nonce, counter);
    let mut output = cipher.block();

    // Zero out any bytes beyond requested length
    for byte in output.iter_mut().skip(len) {
        *byte = 0;
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let key = CryptoKey::from_bytes([
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
            0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ]);
        let nonce = Nonce::from_bytes([0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00]);

        let plaintext = b"Hello, Semantic OS!";
        let mut data = *plaintext;

        // Encrypt
        encrypt(&key, &nonce, &mut data);
        assert_ne!(&data, plaintext);

        // Decrypt
        decrypt(&key, &nonce, &mut data);
        assert_eq!(&data, plaintext);
    }
}
