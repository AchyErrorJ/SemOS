//! Key Hierarchy Management
//!
//! Derives pool-specific encryption keys from the master key.
//! Each security tier has its own encryption key derived from the master.
//!
//! # Hierarchy
//!
//! ```text
//! Master Key (from passphrase)
//!     │
//!     ├─[HKDF "pool:public"]────► Public Pool Key
//!     ├─[HKDF "pool:internal"]──► Internal Pool Key
//!     ├─[HKDF "pool:sensitive"]─► Sensitive Pool Key
//!     └─[HKDF "pool:secret"]────► Secret Pool Key
//!           │
//!           └─[HKDF "obj:{suid}"]──► Object Key (optional)
//! ```
//!
//! # Key Derivation Function
//!
//! Uses HKDF-SHA256 (RFC 5869) for key derivation:
//! - Extract: PRK = HMAC-SHA256(salt, IKM)
//! - Expand: OKM = HMAC-SHA256(PRK, info || 0x01)

use super::{CryptoKey, CryptoResult, CryptoError, KEY_SIZE, sha256};
use super::master_key::MasterKey;
use crate::memory::SecurityTier;
use core::sync::atomic::{AtomicBool, Ordering};

/// Pool key labels for derivation
const POOL_LABELS: [&[u8]; 4] = [
    b"pool:public",
    b"pool:internal",
    b"pool:sensitive",
    b"pool:secret",
];

/// Pool keys storage
static POOL_KEYS_INITIALIZED: AtomicBool = AtomicBool::new(false);
static mut POOL_KEYS: [CryptoKey; 4] = [
    CryptoKey { bytes: [0u8; KEY_SIZE] },
    CryptoKey { bytes: [0u8; KEY_SIZE] },
    CryptoKey { bytes: [0u8; KEY_SIZE] },
    CryptoKey { bytes: [0u8; KEY_SIZE] },
];

/// Key hierarchy manager
pub struct KeyHierarchy;

impl KeyHierarchy {
    /// Initialize pool keys from master key
    pub fn init() -> CryptoResult<()> {
        let master = MasterKey::global()
            .ok_or(CryptoError::KeyNotInitialized)?;

        unsafe {
            for (i, label) in POOL_LABELS.iter().enumerate() {
                POOL_KEYS[i] = Self::derive_pool_key(master, label);
            }
        }

        POOL_KEYS_INITIALIZED.store(true, Ordering::Release);
        crate::platform::log("  [crypto] Pool keys derived\n");
        Ok(())
    }

    /// Clear all pool keys
    pub fn clear() {
        POOL_KEYS_INITIALIZED.store(false, Ordering::Release);
        unsafe {
            for key in POOL_KEYS.iter_mut() {
                key.zeroize();
            }
        }
        crate::platform::log("  [crypto] Pool keys cleared\n");
    }

    /// Check if pool keys are initialized
    pub fn is_initialized() -> bool {
        POOL_KEYS_INITIALIZED.load(Ordering::Acquire)
    }

    /// Get pool key for a security tier
    pub fn get_pool_key(tier: SecurityTier) -> CryptoResult<&'static CryptoKey> {
        if !Self::is_initialized() {
            return Err(CryptoError::KeyNotInitialized);
        }

        let index = tier as usize;
        if index >= 4 {
            return Err(CryptoError::InvalidKeyLength);
        }

        unsafe { Ok(&POOL_KEYS[index]) }
    }

    /// Derive a pool key from master key using HKDF
    fn derive_pool_key(master: &CryptoKey, label: &[u8]) -> CryptoKey {
        let key_bytes = hkdf_sha256(master.as_bytes(), label);
        CryptoKey::from_bytes(key_bytes)
    }

    /// Derive an object-specific key from pool key
    /// Used for per-object encryption (optional, for extra security)
    pub fn derive_object_key(tier: SecurityTier, suid: u128) -> CryptoResult<CryptoKey> {
        let pool_key = Self::get_pool_key(tier)?;

        // Create label: "obj:" + SUID bytes
        let mut label = [0u8; 20];
        label[0..4].copy_from_slice(b"obj:");
        label[4..20].copy_from_slice(&suid.to_le_bytes());

        let key_bytes = hkdf_sha256(pool_key.as_bytes(), &label);
        Ok(CryptoKey::from_bytes(key_bytes))
    }

    /// Derive a key for specific purpose
    pub fn derive_purpose_key(purpose: &[u8]) -> CryptoResult<CryptoKey> {
        let master = MasterKey::global()
            .ok_or(CryptoError::KeyNotInitialized)?;

        let key_bytes = hkdf_sha256(master.as_bytes(), purpose);
        Ok(CryptoKey::from_bytes(key_bytes))
    }
}

/// HKDF-SHA256 key derivation (simplified, single output block)
///
/// This implements a simplified HKDF that produces exactly 32 bytes.
/// For full HKDF compliance, we'd need to handle arbitrary output lengths.
fn hkdf_sha256(input_key: &[u8], info: &[u8]) -> [u8; KEY_SIZE] {
    // Extract phase: PRK = HMAC-SHA256(salt, IKM)
    // Using a fixed salt for simplicity
    let salt = b"SemanticOS-HKDF-Salt-v1";
    let prk = sha256::hmac(salt, input_key);

    // Expand phase: OKM = HMAC-SHA256(PRK, info || 0x01)
    let mut expand_input = [0u8; 64];
    let info_len = info.len().min(63);
    expand_input[..info_len].copy_from_slice(&info[..info_len]);
    expand_input[info_len] = 0x01;

    sha256::hmac(&prk, &expand_input[..info_len + 1])
}

/// Key rotation support
pub struct KeyRotation;

impl KeyRotation {
    /// Rotate pool key (re-derive from master with new context)
    /// This requires re-encrypting all data in the pool
    pub fn rotate_pool_key(tier: SecurityTier, context: &[u8]) -> CryptoResult<CryptoKey> {
        let master = MasterKey::global()
            .ok_or(CryptoError::KeyNotInitialized)?;

        // Derive new key with rotation context
        let mut label = [0u8; 48];
        let base_label = POOL_LABELS[tier as usize];
        let base_len = base_label.len();
        label[..base_len].copy_from_slice(base_label);
        label[base_len] = b':';

        let ctx_len = context.len().min(48 - base_len - 1);
        label[base_len + 1..base_len + 1 + ctx_len].copy_from_slice(&context[..ctx_len]);

        let key_bytes = hkdf_sha256(master.as_bytes(), &label[..base_len + 1 + ctx_len]);
        Ok(CryptoKey::from_bytes(key_bytes))
    }

    /// Generate a fresh encryption key (for new sessions)
    pub fn generate_session_key(seed: u64) -> CryptoKey {
        // Use seed + master key to derive session key
        let mut input = [0u8; 40];
        input[0..8].copy_from_slice(&seed.to_le_bytes());
        input[8..16].copy_from_slice(b"session:");

        if let Some(master) = MasterKey::global() {
            input[16..40].copy_from_slice(&master.as_bytes()[..24]);
        }

        let key_bytes = sha256::hash(&input);
        CryptoKey::from_bytes(key_bytes)
    }
}

/// Print key hierarchy status (for debugging)
pub fn print_status() {
    crate::platform::log("\n  Key Hierarchy Status:\n");

    if MasterKey::is_initialized() {
        crate::platform::log("    Master Key: [UNLOCKED]\n");
    } else {
        crate::platform::log("    Master Key: [LOCKED]\n");
    }

    if KeyHierarchy::is_initialized() {
        crate::platform::log("    Pool Keys: [DERIVED]\n");
        crate::platform::log("      - Public:    [OK]\n");
        crate::platform::log("      - Internal:  [OK]\n");
        crate::platform::log("      - Sensitive: [OK]\n");
        crate::platform::log("      - Secret:    [OK]\n");
    } else {
        crate::platform::log("    Pool Keys: [NOT INITIALIZED]\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hkdf_deterministic() {
        let key = [1u8; 32];
        let info = b"test info";

        let derived1 = hkdf_sha256(&key, info);
        let derived2 = hkdf_sha256(&key, info);

        assert_eq!(derived1, derived2);
    }

    #[test]
    fn test_hkdf_different_info() {
        let key = [1u8; 32];

        let derived1 = hkdf_sha256(&key, b"info1");
        let derived2 = hkdf_sha256(&key, b"info2");

        assert_ne!(derived1, derived2);
    }
}
