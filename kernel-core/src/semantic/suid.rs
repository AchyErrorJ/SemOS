//! Semantic Unique Identifier (SUID)
//!
//! SUIDs are 128-bit identifiers that uniquely address semantic objects.
//! Unlike file paths or memory addresses, SUIDs represent the *identity*
//! of content, not its location.
//!
//! # Generation Methods
//!
//! 1. Content-based: Hash of content + metadata (deterministic)
//! 2. Random: Cryptographically random (for new objects)
//! 3. Derived: From parent SUID + relationship (for linked objects)

use core::fmt;

/// Semantic Unique Identifier - a 128-bit content-addressable ID
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct SUID {
    /// High 64 bits (includes version and type info)
    pub high: u64,
    /// Low 64 bits (content hash or random)
    pub low: u64,
}

impl SUID {
    /// SUID version field (bits 124-127 of high)
    pub const VERSION_1: u8 = 1;

    /// Type field values (bits 120-123 of high)
    pub const TYPE_CONTENT: u8 = 0;   // Content-addressed
    pub const TYPE_RANDOM: u8 = 1;    // Random identifier
    pub const TYPE_DERIVED: u8 = 2;   // Derived from parent
    pub const TYPE_SYSTEM: u8 = 15;   // System/kernel object

    /// The null SUID (invalid)
    pub const NULL: SUID = SUID { high: 0, low: 0 };

    /// Create a new SUID from raw values
    pub const fn new(high: u64, low: u64) -> Self {
        Self { high, low }
    }

    /// Create a content-addressed SUID from a hash
    ///
    /// The hash should be computed from:
    /// - Object content
    /// - Creation timestamp
    /// - Security tier
    pub fn from_content_hash(hash: &[u8; 32]) -> Self {
        // Use first 16 bytes of hash, set version and type
        let mut high = u64::from_be_bytes([
            hash[0], hash[1], hash[2], hash[3],
            hash[4], hash[5], hash[6], hash[7],
        ]);
        let low = u64::from_be_bytes([
            hash[8], hash[9], hash[10], hash[11],
            hash[12], hash[13], hash[14], hash[15],
        ]);

        // Set version (1) and type (content) in top nibble
        high = (high & 0x0FFF_FFFF_FFFF_FFFF) | ((Self::VERSION_1 as u64) << 60);
        high = (high & 0xF0FF_FFFF_FFFF_FFFF) | ((Self::TYPE_CONTENT as u64) << 56);

        Self { high, low }
    }

    /// Create a random SUID using a simple LCG PRNG
    /// (In production, use a CSPRNG)
    pub fn random(seed: u64) -> Self {
        // Simple LCG for no_std environment
        // In production, use hardware RNG or better PRNG
        let a: u64 = 6364136223846793005;
        let c: u64 = 1442695040888963407;

        let r1 = seed.wrapping_mul(a).wrapping_add(c);
        let r2 = r1.wrapping_mul(a).wrapping_add(c);

        let mut high = r1;
        let low = r2;

        // Set version (1) and type (random)
        high = (high & 0x0FFF_FFFF_FFFF_FFFF) | ((Self::VERSION_1 as u64) << 60);
        high = (high & 0xF0FF_FFFF_FFFF_FFFF) | ((Self::TYPE_RANDOM as u64) << 56);

        Self { high, low }
    }

    /// Derive a child SUID from a parent and relationship
    pub fn derive(parent: &SUID, relationship: u64) -> Self {
        // XOR parent with relationship, then hash-like mixing
        let mixed_high = parent.high ^ relationship;
        let mixed_low = parent.low ^ relationship.rotate_left(32);

        // Simple mixing function
        let high = mixed_high.wrapping_mul(0x9E3779B97F4A7C15);
        let low = mixed_low.wrapping_mul(0xBF58476D1CE4E5B9);

        let mut result_high = high ^ low.rotate_right(17);

        // Set version (1) and type (derived)
        result_high = (result_high & 0x0FFF_FFFF_FFFF_FFFF) | ((Self::VERSION_1 as u64) << 60);
        result_high = (result_high & 0xF0FF_FFFF_FFFF_FFFF) | ((Self::TYPE_DERIVED as u64) << 56);

        Self { high: result_high, low }
    }

    /// Create a system SUID for kernel objects
    pub const fn system(id: u64) -> Self {
        let high = ((Self::VERSION_1 as u64) << 60) | ((Self::TYPE_SYSTEM as u64) << 56);
        Self { high, low: id }
    }

    /// Check if this SUID is null/invalid
    pub fn is_null(&self) -> bool {
        self.high == 0 && self.low == 0
    }

    /// Get the version number
    pub fn version(&self) -> u8 {
        ((self.high >> 60) & 0xF) as u8
    }

    /// Get the type field
    pub fn suid_type(&self) -> u8 {
        ((self.high >> 56) & 0xF) as u8
    }

    /// Convert to bytes (big-endian)
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&self.high.to_be_bytes());
        bytes[8..16].copy_from_slice(&self.low.to_be_bytes());
        bytes
    }

    /// Create from bytes (big-endian)
    pub fn from_bytes(bytes: &[u8; 16]) -> Self {
        let high = u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let low = u64::from_be_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        Self { high, low }
    }
}

impl fmt::Debug for SUID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SUID({:016X}{:016X})", self.high, self.low)
    }
}

/// Well-known system SUIds
pub mod system_suids {
    use super::SUID;

    /// Root registry object
    pub const ROOT: SUID = SUID::system(0);

    /// Kernel configuration
    pub const KERNEL_CONFIG: SUID = SUID::system(1);

    /// Security policy
    pub const SECURITY_POLICY: SUID = SUID::system(2);

    /// Process table
    pub const PROCESS_TABLE: SUID = SUID::system(3);

    /// Memory map
    pub const MEMORY_MAP: SUID = SUID::system(4);
}

/// SUID generator with state
pub struct SUIDGenerator {
    /// Counter for deterministic generation
    counter: u64,
    /// Seed for random generation
    seed: u64,
}

impl SUIDGenerator {
    /// Create a new generator
    pub const fn new(initial_seed: u64) -> Self {
        Self {
            counter: 0,
            seed: initial_seed,
        }
    }

    /// Generate a new random SUID
    pub fn next_random(&mut self) -> SUID {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        SUID::random(self.seed)
    }

    /// Generate a sequential system SUID
    pub fn next_system(&mut self) -> SUID {
        self.counter += 1;
        SUID::system(self.counter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suid_null() {
        assert!(SUID::NULL.is_null());
        assert!(!SUID::system(1).is_null());
    }

    #[test]
    fn test_suid_version() {
        let suid = SUID::system(42);
        assert_eq!(suid.version(), SUID::VERSION_1);
        assert_eq!(suid.suid_type(), SUID::TYPE_SYSTEM);
    }

    #[test]
    fn test_suid_bytes_roundtrip() {
        let original = SUID::new(0x1234567890ABCDEF, 0xFEDCBA0987654321);
        let bytes = original.to_bytes();
        let recovered = SUID::from_bytes(&bytes);
        assert_eq!(original, recovered);
    }
}
