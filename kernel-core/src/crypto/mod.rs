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
pub mod sha256;

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

// SHA-256 + HMAC-SHA256 used to live inline here. They were moved to
// `crypto/sha256.rs` 2026-05-16 — see that file for: streaming Sha256
// struct, one-shot `hash()` (now arbitrary-length, was capped at 55 bytes),
// and HMAC-SHA256 (now arbitrary-length, was capped at 64 bytes), with
// RFC 6234 + RFC 4231 test vectors. API path `crypto::sha256::{hash, hmac}`
// is unchanged — call sites need no edits.
