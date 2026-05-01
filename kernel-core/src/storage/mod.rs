//! Storage Subsystem for Semantic OS
//!
//! Provides persistent storage for encrypted memory pools.
//! Handles serialization, encryption, and boot-time unlock.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    Storage Header                        │
//! │  Magic | Version | Salt | Pool Table Offset | Checksum  │
//! ├─────────────────────────────────────────────────────────┤
//! │                    Pool Table                            │
//! │  [Pool 0 Entry] [Pool 1 Entry] [Pool 2 Entry] [Pool 3]  │
//! ├─────────────────────────────────────────────────────────┤
//! │                    Encrypted Pool Data                   │
//! │  ┌─────────────────┐ ┌─────────────────┐               │
//! │  │ Pool 0 (Public) │ │ Pool 1 (Internal)│ ...          │
//! │  │ Nonce + Tag +   │ │ Nonce + Tag +   │               │
//! │  │ Ciphertext      │ │ Ciphertext      │               │
//! │  └─────────────────┘ └─────────────────┘               │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # Security
//!
//! - Each pool encrypted with its own derived key
//! - ChaCha20-Poly1305 AEAD for confidentiality and integrity
//! - Salt stored in header for key derivation
//! - Master key derived from passphrase at boot

pub mod persistence;
pub mod boot;

use crate::crypto::{Salt, Nonce, TAG_SIZE, NONCE_SIZE, SALT_SIZE};
use crate::memory::SecurityTier;

/// Storage magic number ("SEMO" - Semantic OS)
pub const STORAGE_MAGIC: [u8; 4] = [0x53, 0x45, 0x4D, 0x4F];

/// Current storage format version
pub const STORAGE_VERSION: u32 = 1;

/// Maximum pool data size (per pool)
pub const MAX_POOL_DATA_SIZE: usize = 64 * 1024; // 64KB per pool

/// Number of pools
pub const NUM_POOLS: usize = 4;

/// Storage header (at beginning of storage)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StorageHeader {
    /// Magic number for identification
    pub magic: [u8; 4],
    /// Format version
    pub version: u32,
    /// Salt for master key derivation
    pub salt: [u8; SALT_SIZE],
    /// Offset to pool table
    pub pool_table_offset: u32,
    /// Total data size (header + pools)
    pub total_size: u32,
    /// Header checksum (CRC32)
    pub checksum: u32,
    /// Reserved for future use
    pub reserved: [u8; 20],
}

impl StorageHeader {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Create a new storage header
    pub fn new(salt: Salt, pool_table_offset: u32, total_size: u32) -> Self {
        let mut header = Self {
            magic: STORAGE_MAGIC,
            version: STORAGE_VERSION,
            salt: *salt.as_bytes(),
            pool_table_offset,
            total_size,
            checksum: 0,
            reserved: [0u8; 20],
        };
        header.checksum = header.compute_checksum();
        header
    }

    /// Compute header checksum
    fn compute_checksum(&self) -> u32 {
        // Simple CRC32-like checksum
        let bytes = unsafe {
            core::slice::from_raw_parts(
                self as *const _ as *const u8,
                Self::SIZE - 4 // Exclude checksum field
            )
        };

        let mut crc: u32 = 0xFFFFFFFF;
        for &byte in bytes {
            crc ^= byte as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB88320;
                } else {
                    crc >>= 1;
                }
            }
        }
        !crc
    }

    /// Verify header validity
    pub fn verify(&self) -> bool {
        self.magic == STORAGE_MAGIC
            && self.version == STORAGE_VERSION
            && self.checksum == self.compute_checksum()
    }

    /// Get salt
    pub fn get_salt(&self) -> Salt {
        Salt::from_bytes(self.salt)
    }
}

/// Pool entry in pool table
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PoolEntry {
    /// Security tier (0-3)
    pub tier: u8,
    /// Flags (encrypted, compressed, etc.)
    pub flags: u8,
    /// Reserved
    pub reserved: [u8; 2],
    /// Offset to pool data from start of storage
    pub data_offset: u32,
    /// Size of encrypted data (includes nonce + tag)
    pub data_size: u32,
    /// Original unencrypted size
    pub original_size: u32,
    /// Object count in this pool
    pub object_count: u32,
    /// Pool-specific metadata
    pub metadata: [u8; 8],
}

impl PoolEntry {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Pool data is encrypted
    pub const FLAG_ENCRYPTED: u8 = 0x01;
    /// Pool data is compressed (future use)
    pub const FLAG_COMPRESSED: u8 = 0x02;
    /// Pool contains sensitive data
    pub const FLAG_SENSITIVE: u8 = 0x04;

    /// Create a new pool entry
    pub fn new(tier: SecurityTier, data_offset: u32, data_size: u32, original_size: u32) -> Self {
        let flags = Self::FLAG_ENCRYPTED
            | if tier as u8 >= 2 { Self::FLAG_SENSITIVE } else { 0 };

        Self {
            tier: tier as u8,
            flags,
            reserved: [0; 2],
            data_offset,
            data_size,
            original_size,
            object_count: 0,
            metadata: [0; 8],
        }
    }

    /// Check if encrypted
    pub fn is_encrypted(&self) -> bool {
        self.flags & Self::FLAG_ENCRYPTED != 0
    }
}

/// Pool table (array of pool entries)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PoolTable {
    pub entries: [PoolEntry; NUM_POOLS],
}

impl PoolTable {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    pub fn new() -> Self {
        Self {
            entries: [PoolEntry {
                tier: 0,
                flags: 0,
                reserved: [0; 2],
                data_offset: 0,
                data_size: 0,
                original_size: 0,
                object_count: 0,
                metadata: [0; 8],
            }; NUM_POOLS],
        }
    }
}

/// Encrypted pool data header (prepended to each pool's ciphertext)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EncryptedPoolHeader {
    /// Nonce used for encryption
    pub nonce: [u8; NONCE_SIZE],
    /// Authentication tag
    pub tag: [u8; TAG_SIZE],
}

impl EncryptedPoolHeader {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    pub fn new(nonce: Nonce, tag: [u8; TAG_SIZE]) -> Self {
        Self {
            nonce: *nonce.as_bytes(),
            tag,
        }
    }

    pub fn get_nonce(&self) -> Nonce {
        Nonce::from_bytes(self.nonce)
    }
}

/// Storage error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    /// Invalid storage header
    InvalidHeader,
    /// Version mismatch
    VersionMismatch,
    /// Checksum failed
    ChecksumFailed,
    /// Pool not found
    PoolNotFound,
    /// Encryption failed
    EncryptionFailed,
    /// Decryption failed
    DecryptionFailed,
    /// Authentication failed (tampered data)
    AuthenticationFailed,
    /// Buffer too small
    BufferTooSmall,
    /// Key not initialized
    KeyNotInitialized,
    /// I/O error
    IoError,
    /// Storage full
    StorageFull,
}

/// Result type for storage operations
pub type StorageResult<T> = Result<T, StorageError>;

/// Storage statistics
#[derive(Clone, Copy)]
pub struct StorageStats {
    pub total_size: usize,
    pub used_size: usize,
    pub pool_count: usize,
    pub object_count: usize,
    pub encrypted_pools: usize,
}

impl StorageStats {
    pub fn empty() -> Self {
        Self {
            total_size: 0,
            used_size: 0,
            pool_count: 0,
            object_count: 0,
            encrypted_pools: 0,
        }
    }
}

/// Initialize storage subsystem
pub fn init() {
    crate::platform::log("  [storage] Storage subsystem initialized\n");
    crate::platform::log("    Format: SEMO v1\n");
    crate::platform::log("    Encryption: ChaCha20-Poly1305\n");
    crate::platform::log("    Max pool size: 64KB\n");
}

/// Print storage status
pub fn print_status() {
    crate::platform::log("\n  Storage Status:\n");
    crate::platform::log("    Header size: ");
    crate::platform::log_num(StorageHeader::SIZE as u64);
    crate::platform::log(" bytes\n");
    crate::platform::log("    Pool entry size: ");
    crate::platform::log_num(PoolEntry::SIZE as u64);
    crate::platform::log(" bytes\n");
    crate::platform::log("    Encrypted header size: ");
    crate::platform::log_num(EncryptedPoolHeader::SIZE as u64);
    crate::platform::log(" bytes\n");
}
