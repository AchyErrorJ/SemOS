//! Master Key Derivation
//!
//! Derives the master encryption key from a user passphrase using PBKDF2-SHA256.
//! The master key is used to derive all pool-specific keys.
//!
//! # Security Properties
//!
//! - **Key Stretching**: 100,000 iterations of PBKDF2
//! - **Salt**: 128-bit random salt (stored alongside encrypted data)
//! - **Output**: 256-bit master key
//!
//! # Usage
//!
//! ```ignore
//! let salt = Salt::generate(get_random_seed());
//! let master = MasterKey::derive("my passphrase", &salt)?;
//! ```

use super::{CryptoKey, CryptoResult, CryptoError, Salt, KEY_SIZE, sha256};
use core::sync::atomic::{AtomicBool, Ordering};

/// Number of PBKDF2 iterations
/// 100,000 is a reasonable balance between security and performance
/// On bare-metal ARM, this takes ~1-2 seconds
pub const PBKDF2_ITERATIONS: u32 = 100_000;

/// Master key state
static MASTER_KEY_INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut MASTER_KEY: CryptoKey = CryptoKey { bytes: [0u8; KEY_SIZE] };
static mut MASTER_SALT: Salt = Salt { bytes: [0u8; 16] };

/// Master key wrapper with additional security features
pub struct MasterKey {
    key: CryptoKey,
    salt: Salt,
}

impl MasterKey {
    /// Derive master key from passphrase using PBKDF2-SHA256
    pub fn derive(passphrase: &str, salt: &Salt) -> CryptoResult<Self> {
        if passphrase.is_empty() {
            return Err(CryptoError::InvalidPassphrase);
        }

        let key = pbkdf2_sha256(
            passphrase.as_bytes(),
            salt.as_bytes(),
            PBKDF2_ITERATIONS,
        );

        Ok(Self {
            key: CryptoKey::from_bytes(key),
            salt: *salt,
        })
    }

    /// Get the derived key
    pub fn key(&self) -> &CryptoKey {
        &self.key
    }

    /// Get the salt used for derivation
    pub fn salt(&self) -> &Salt {
        &self.salt
    }

    /// Verify a passphrase against a known key
    pub fn verify(passphrase: &str, salt: &Salt, expected: &CryptoKey) -> bool {
        if let Ok(derived) = Self::derive(passphrase, salt) {
            // Constant-time comparison
            let mut diff = 0u8;
            for (a, b) in derived.key.as_bytes().iter().zip(expected.as_bytes().iter()) {
                diff |= a ^ b;
            }
            diff == 0
        } else {
            false
        }
    }

    /// Store as global master key
    pub fn set_global(self) {
        unsafe {
            MASTER_KEY = self.key.clone();
            MASTER_SALT = self.salt;
        }
        MASTER_KEY_INITIALIZED.store(true, Ordering::Release);
    }

    /// Clear the global master key
    pub fn clear_global() {
        MASTER_KEY_INITIALIZED.store(false, Ordering::Release);
        unsafe {
            MASTER_KEY.zeroize();
            MASTER_SALT = Salt::zero();
        }
    }

    /// Check if global master key is initialized
    pub fn is_initialized() -> bool {
        MASTER_KEY_INITIALIZED.load(Ordering::Acquire)
    }

    /// Get reference to global master key (if initialized)
    pub fn global() -> Option<&'static CryptoKey> {
        if Self::is_initialized() {
            unsafe { Some(&MASTER_KEY) }
        } else {
            None
        }
    }

    /// Get reference to global salt
    pub fn global_salt() -> Option<&'static Salt> {
        if Self::is_initialized() {
            unsafe { Some(&MASTER_SALT) }
        } else {
            None
        }
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// PBKDF2-SHA256 key derivation
/// Derives a key from password and salt using HMAC-SHA256
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; KEY_SIZE] {
    let mut result = [0u8; KEY_SIZE];

    // PBKDF2 with dkLen = 32 needs only 1 block (i = 1)
    let mut block = pbkdf2_f(password, salt, iterations, 1);
    result.copy_from_slice(&block);

    // Zeroize intermediate value
    for byte in &mut block {
        *byte = 0;
    }

    result
}

/// PBKDF2 F function: F(Password, Salt, c, i) = U1 ^ U2 ^ ... ^ Uc
fn pbkdf2_f(password: &[u8], salt: &[u8], iterations: u32, block_num: u32) -> [u8; 32] {
    // U1 = PRF(Password, Salt || INT(i))
    let mut salt_i = [0u8; 64];
    let salt_len = salt.len().min(60);
    salt_i[..salt_len].copy_from_slice(&salt[..salt_len]);
    salt_i[salt_len..salt_len + 4].copy_from_slice(&block_num.to_be_bytes());

    let mut u = sha256::hmac(password, &salt_i[..salt_len + 4]);
    let mut result = u;

    // U2..Uc
    for _ in 1..iterations {
        u = sha256::hmac(password, &u);
        for (r, b) in result.iter_mut().zip(u.iter()) {
            *r ^= b;
        }
    }

    result
}

/// Unlock the master key from passphrase
/// Called at boot time to decrypt stored pools
pub fn unlock(passphrase: &str, salt: &Salt) -> CryptoResult<()> {
    let master = MasterKey::derive(passphrase, salt)?;
    master.set_global();

    crate::platform::log("  [crypto] Master key unlocked\n");
    Ok(())
}

/// Lock (clear) the master key
/// Called at shutdown or when security is compromised
pub fn lock() {
    MasterKey::clear_global();
    crate::platform::log("  [crypto] Master key locked\n");
}

/// Check if master key is available
pub fn is_unlocked() -> bool {
    MasterKey::is_initialized()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_derivation() {
        let salt = Salt::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let master = MasterKey::derive("test passphrase", &salt).unwrap();

        // Verify same passphrase + salt = same key
        let master2 = MasterKey::derive("test passphrase", &salt).unwrap();
        assert_eq!(master.key.as_bytes(), master2.key.as_bytes());

        // Different passphrase = different key
        let master3 = MasterKey::derive("different passphrase", &salt).unwrap();
        assert_ne!(master.key.as_bytes(), master3.key.as_bytes());
    }

    #[test]
    fn test_verify() {
        let salt = Salt::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let master = MasterKey::derive("test", &salt).unwrap();

        assert!(MasterKey::verify("test", &salt, master.key()));
        assert!(!MasterKey::verify("wrong", &salt, master.key()));
    }
}
