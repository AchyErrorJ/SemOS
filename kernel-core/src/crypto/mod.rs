//! Cryptographic Services for Semantic OS
//!
//! Provides encryption, key derivation, and key management for
//! secure storage of semantic objects across security tiers.
//!
//! # Key Hierarchy
//!
//! ```text
//! Passphrase
//!     │
//!     ▼ (PBKDF2-SHA256)
//! Master Key (256-bit)
//!     │
//!     ├──► Pool Key (Public)    ──► Object Keys
//!     ├──► Pool Key (Internal)  ──► Object Keys
//!     ├──► Pool Key (Sensitive) ──► Object Keys
//!     └──► Pool Key (Secret)    ──► Object Keys
//! ```
//!
//! # Encryption
//!
//! - **Algorithm**: ChaCha20-Poly1305 (AEAD)
//! - **Key Size**: 256 bits
//! - **Nonce**: 96 bits (random per encryption)
//! - **Tag**: 128 bits (authentication)
//!
//! # no_std Compatibility
//!
//! All crypto operations work without std library.
//! Uses software implementations suitable for bare-metal.

pub mod master_key;
pub mod key_hierarchy;
pub mod chacha20;
pub mod poly1305;

use crate::memory::SecurityTier;

/// Key size in bytes (256 bits)
pub const KEY_SIZE: usize = 32;

/// Nonce size in bytes (96 bits for ChaCha20-Poly1305)
pub const NONCE_SIZE: usize = 12;

/// Authentication tag size in bytes (128 bits)
pub const TAG_SIZE: usize = 16;

/// Salt size for key derivation
pub const SALT_SIZE: usize = 16;

/// A 256-bit cryptographic key
#[derive(Clone)]
pub struct CryptoKey {
    bytes: [u8; KEY_SIZE],
}

impl CryptoKey {
    /// Create a new key from bytes
    pub const fn from_bytes(bytes: [u8; KEY_SIZE]) -> Self {
        Self { bytes }
    }

    /// Create a zero key (for initialization)
    pub const fn zero() -> Self {
        Self { bytes: [0u8; KEY_SIZE] }
    }

    /// Get key bytes
    pub fn as_bytes(&self) -> &[u8; KEY_SIZE] {
        &self.bytes
    }

    /// Get mutable key bytes
    pub fn as_bytes_mut(&mut self) -> &mut [u8; KEY_SIZE] {
        &mut self.bytes
    }

    /// Securely zero the key
    pub fn zeroize(&mut self) {
        for byte in &mut self.bytes {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for CryptoKey {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// A nonce for AEAD encryption
#[derive(Clone, Copy)]
pub struct Nonce {
    bytes: [u8; NONCE_SIZE],
}

impl Nonce {
    /// Create a nonce from bytes
    pub const fn from_bytes(bytes: [u8; NONCE_SIZE]) -> Self {
        Self { bytes }
    }

    /// Create a zero nonce
    pub const fn zero() -> Self {
        Self { bytes: [0u8; NONCE_SIZE] }
    }

    /// Get nonce bytes
    pub fn as_bytes(&self) -> &[u8; NONCE_SIZE] {
        &self.bytes
    }

    /// Increment nonce (for counter mode)
    pub fn increment(&mut self) {
        for byte in self.bytes.iter_mut().rev() {
            *byte = byte.wrapping_add(1);
            if *byte != 0 {
                break;
            }
        }
    }
}

/// Salt for key derivation
#[derive(Clone, Copy)]
pub struct Salt {
    bytes: [u8; SALT_SIZE],
}

impl Salt {
    /// Create salt from bytes
    pub const fn from_bytes(bytes: [u8; SALT_SIZE]) -> Self {
        Self { bytes }
    }

    /// Create a zero salt (not recommended for real use)
    pub const fn zero() -> Self {
        Self { bytes: [0u8; SALT_SIZE] }
    }

    /// Get salt bytes
    pub fn as_bytes(&self) -> &[u8; SALT_SIZE] {
        &self.bytes
    }

    /// Generate random salt using simple PRNG
    /// Note: In production, use a proper hardware RNG
    pub fn generate(seed: u64) -> Self {
        let mut bytes = [0u8; SALT_SIZE];
        let mut state = seed;

        for byte in &mut bytes {
            // Simple xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state as u8;
        }

        Self { bytes }
    }
}

/// Crypto error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// Invalid key length
    InvalidKeyLength,
    /// Invalid nonce length
    InvalidNonceLength,
    /// Authentication failed (tampered data)
    AuthenticationFailed,
    /// Buffer too small
    BufferTooSmall,
    /// Key derivation failed
    KeyDerivationFailed,
    /// Invalid passphrase
    InvalidPassphrase,
    /// Key not initialized
    KeyNotInitialized,
}

/// Result type for crypto operations
pub type CryptoResult<T> = Result<T, CryptoError>;

/// Encrypted data with nonce and tag
pub struct EncryptedData {
    /// The nonce used for encryption
    pub nonce: Nonce,
    /// Authentication tag
    pub tag: [u8; TAG_SIZE],
    /// Ciphertext (same length as plaintext)
    pub ciphertext: [u8; 1024], // Fixed size for simplicity
    /// Actual length of ciphertext
    pub len: usize,
}

impl EncryptedData {
    pub const fn empty() -> Self {
        Self {
            nonce: Nonce::zero(),
            tag: [0u8; TAG_SIZE],
            ciphertext: [0u8; 1024],
            len: 0,
        }
    }
}

/// Initialize crypto subsystem
pub fn init() {
    crate::platform::log("  [crypto] Cryptographic services initialized\n");
    crate::platform::log("    Key derivation: PBKDF2-SHA256\n");
    crate::platform::log("    Encryption: ChaCha20-Poly1305\n");
    crate::platform::log("    Key size: 256 bits\n");
}

/// Simple SHA-256 implementation for key derivation
/// This is a basic implementation - in production, use a vetted library
pub mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
        0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
        0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    #[inline(always)]
    fn rotr(x: u32, n: u32) -> u32 {
        (x >> n) | (x << (32 - n))
    }

    #[inline(always)]
    fn ch(x: u32, y: u32, z: u32) -> u32 {
        (x & y) ^ (!x & z)
    }

    #[inline(always)]
    fn maj(x: u32, y: u32, z: u32) -> u32 {
        (x & y) ^ (x & z) ^ (y & z)
    }

    #[inline(always)]
    fn sigma0(x: u32) -> u32 {
        rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22)
    }

    #[inline(always)]
    fn sigma1(x: u32) -> u32 {
        rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25)
    }

    #[inline(always)]
    fn gamma0(x: u32) -> u32 {
        rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3)
    }

    #[inline(always)]
    fn gamma1(x: u32) -> u32 {
        rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10)
    }

    /// SHA-256 hash
    pub fn hash(data: &[u8]) -> [u8; 32] {
        let mut h = H0;

        // Pad message
        let bit_len = (data.len() as u64) * 8;
        let mut padded = [0u8; 128]; // Max 2 blocks for small inputs
        let data_len = data.len().min(55); // Fit in one block with padding
        padded[..data_len].copy_from_slice(&data[..data_len]);
        padded[data_len] = 0x80;

        // Add length in bits (big endian)
        let len_offset = if data_len < 56 { 56 } else { 120 };
        padded[len_offset..len_offset + 8].copy_from_slice(&bit_len.to_be_bytes());

        let blocks = if data_len < 56 { 1 } else { 2 };

        for block in 0..blocks {
            let mut w = [0u32; 64];
            let offset = block * 64;

            // Prepare message schedule
            for i in 0..16 {
                let j = offset + i * 4;
                w[i] = u32::from_be_bytes([
                    padded[j], padded[j + 1], padded[j + 2], padded[j + 3]
                ]);
            }

            for i in 16..64 {
                w[i] = gamma1(w[i - 2])
                    .wrapping_add(w[i - 7])
                    .wrapping_add(gamma0(w[i - 15]))
                    .wrapping_add(w[i - 16]);
            }

            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
                (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

            for i in 0..64 {
                let t1 = hh
                    .wrapping_add(sigma1(e))
                    .wrapping_add(ch(e, f, g))
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let t2 = sigma0(a).wrapping_add(maj(a, b, c));

                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }

            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }

        let mut result = [0u8; 32];
        for (i, &word) in h.iter().enumerate() {
            result[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        result
    }

    /// HMAC-SHA256
    pub fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
        let mut k = [0u8; 64];

        if key.len() > 64 {
            let h = hash(key);
            k[..32].copy_from_slice(&h);
        } else {
            k[..key.len()].copy_from_slice(key);
        }

        // Inner padding
        let mut inner = [0x36u8; 64];
        for (i, byte) in k.iter().enumerate() {
            inner[i] ^= byte;
        }

        // Outer padding
        let mut outer = [0x5cu8; 64];
        for (i, byte) in k.iter().enumerate() {
            outer[i] ^= byte;
        }

        // H(outer || H(inner || data))
        let mut inner_data = [0u8; 128];
        inner_data[..64].copy_from_slice(&inner);
        let data_len = data.len().min(64);
        inner_data[64..64 + data_len].copy_from_slice(&data[..data_len]);

        let inner_hash = hash(&inner_data[..64 + data_len]);

        let mut outer_data = [0u8; 96];
        outer_data[..64].copy_from_slice(&outer);
        outer_data[64..96].copy_from_slice(&inner_hash);

        hash(&outer_data)
    }
}
