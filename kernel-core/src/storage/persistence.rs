//! Pool Persistence
//!
//! Handles serialization and encryption of memory pools for persistent storage.
//!
//! # Serialization Format
//!
//! Each pool is serialized as:
//! 1. Pool metadata (frame count, allocation bitmap, etc.)
//! 2. Frame data (raw bytes)
//!
//! The serialized data is then encrypted with the pool's derived key.

use super::{
    StorageHeader, PoolTable, PoolEntry, EncryptedPoolHeader,
    StorageResult, StorageError, MAX_POOL_DATA_SIZE,
};
use crate::crypto::{
    Nonce, Salt, TAG_SIZE, NONCE_SIZE,
    key_hierarchy::KeyHierarchy,
    poly1305::{aead_encrypt, aead_decrypt},
};
use crate::memory::{SecurityTier, MemoryPool};

/// Serialized pool metadata
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PoolMetadata {
    /// Pool security tier
    pub tier: u8,
    /// Number of frames in pool
    pub frame_count: u8,
    /// Number of allocated frames
    pub allocated_count: u8,
    /// Frame size (in 4KB units)
    pub frame_size_4k: u8,
    /// Base address of pool
    pub base_addr: u64,
    /// Allocation bitmap (64 bits for up to 64 frames)
    pub allocation_bitmap: u64,
    /// Reserved for future use
    pub reserved: [u8; 8],
}

impl PoolMetadata {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Create metadata from a memory pool
    pub fn from_pool(pool: &MemoryPool, tier: SecurityTier) -> Self {
        let mut bitmap: u64 = 0;
        let mut allocated_count = 0u8;

        // Build allocation bitmap
        for i in 0..pool.frame_count().min(64) {
            if pool.is_frame_allocated(i) {
                bitmap |= 1u64 << i;
                allocated_count += 1;
            }
        }

        Self {
            tier: tier as u8,
            frame_count: pool.frame_count() as u8,
            allocated_count,
            frame_size_4k: 1, // 4KB frames
            base_addr: pool.base_address() as u64,
            allocation_bitmap: bitmap,
            reserved: [0; 8],
        }
    }
}

/// Pool serializer
pub struct PoolSerializer {
    /// Buffer for serialized data
    buffer: [u8; MAX_POOL_DATA_SIZE],
    /// Current position in buffer
    pos: usize,
}

impl PoolSerializer {
    /// Create a new serializer
    pub fn new() -> Self {
        Self {
            buffer: [0u8; MAX_POOL_DATA_SIZE],
            pos: 0,
        }
    }

    /// Reset serializer for reuse
    pub fn reset(&mut self) {
        self.pos = 0;
    }

    /// Serialize a memory pool
    pub fn serialize_pool(&mut self, pool: &MemoryPool, tier: SecurityTier) -> StorageResult<usize> {
        self.reset();

        // Write pool metadata
        let metadata = PoolMetadata::from_pool(pool, tier);
        self.write_struct(&metadata)?;

        // Write frame data for allocated frames
        for i in 0..pool.frame_count() {
            if pool.is_frame_allocated(i) {
                let frame_data = pool.get_frame_data(i);
                self.write_bytes(frame_data)?;
            }
        }

        Ok(self.pos)
    }

    /// Write a struct to buffer
    fn write_struct<T>(&mut self, data: &T) -> StorageResult<()> {
        let size = core::mem::size_of::<T>();
        if self.pos + size > MAX_POOL_DATA_SIZE {
            return Err(StorageError::BufferTooSmall);
        }

        let bytes = unsafe {
            core::slice::from_raw_parts(data as *const T as *const u8, size)
        };
        self.buffer[self.pos..self.pos + size].copy_from_slice(bytes);
        self.pos += size;
        Ok(())
    }

    /// Write bytes to buffer
    fn write_bytes(&mut self, data: &[u8]) -> StorageResult<()> {
        if self.pos + data.len() > MAX_POOL_DATA_SIZE {
            return Err(StorageError::BufferTooSmall);
        }
        self.buffer[self.pos..self.pos + data.len()].copy_from_slice(data);
        self.pos += data.len();
        Ok(())
    }

    /// Get serialized data
    pub fn data(&self) -> &[u8] {
        &self.buffer[..self.pos]
    }

    /// Get serialized length
    pub fn len(&self) -> usize {
        self.pos
    }
}

/// Pool deserializer
pub struct PoolDeserializer<'a> {
    /// Data buffer
    data: &'a [u8],
    /// Current position
    pos: usize,
}

impl<'a> PoolDeserializer<'a> {
    /// Create a new deserializer
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Read pool metadata
    pub fn read_metadata(&mut self) -> StorageResult<PoolMetadata> {
        if self.pos + PoolMetadata::SIZE > self.data.len() {
            return Err(StorageError::BufferTooSmall);
        }

        let metadata = unsafe {
            core::ptr::read(self.data.as_ptr().add(self.pos) as *const PoolMetadata)
        };
        self.pos += PoolMetadata::SIZE;
        Ok(metadata)
    }

    /// Read frame data
    pub fn read_frame(&mut self, size: usize) -> StorageResult<&'a [u8]> {
        if self.pos + size > self.data.len() {
            return Err(StorageError::BufferTooSmall);
        }

        let frame = &self.data[self.pos..self.pos + size];
        self.pos += size;
        Ok(frame)
    }

    /// Remaining bytes
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
}

/// Encrypt serialized pool data
pub fn encrypt_pool(
    tier: SecurityTier,
    plaintext: &[u8],
    nonce: &Nonce,
    output: &mut [u8],
) -> StorageResult<usize> {
    // Get pool-specific encryption key
    let key = KeyHierarchy::get_pool_key(tier)
        .map_err(|_| StorageError::KeyNotInitialized)?;

    // Ensure output buffer is large enough
    let total_size = EncryptedPoolHeader::SIZE + plaintext.len();
    if output.len() < total_size {
        return Err(StorageError::BufferTooSmall);
    }

    // Encrypt data
    let mut tag = [0u8; TAG_SIZE];
    let aad = &[tier as u8]; // Authenticate the tier

    aead_encrypt(
        key,
        nonce,
        aad,
        plaintext,
        &mut output[EncryptedPoolHeader::SIZE..],
        &mut tag,
    ).map_err(|_| StorageError::EncryptionFailed)?;

    // Write encrypted header
    let header = EncryptedPoolHeader::new(*nonce, tag);
    let header_bytes = unsafe {
        core::slice::from_raw_parts(
            &header as *const _ as *const u8,
            EncryptedPoolHeader::SIZE
        )
    };
    output[..EncryptedPoolHeader::SIZE].copy_from_slice(header_bytes);

    Ok(total_size)
}

/// Decrypt pool data
pub fn decrypt_pool(
    tier: SecurityTier,
    encrypted: &[u8],
    output: &mut [u8],
) -> StorageResult<usize> {
    if encrypted.len() < EncryptedPoolHeader::SIZE {
        return Err(StorageError::BufferTooSmall);
    }

    // Read encrypted header
    let header = unsafe {
        core::ptr::read(encrypted.as_ptr() as *const EncryptedPoolHeader)
    };

    let ciphertext = &encrypted[EncryptedPoolHeader::SIZE..];
    if output.len() < ciphertext.len() {
        return Err(StorageError::BufferTooSmall);
    }

    // Get pool-specific decryption key
    let key = KeyHierarchy::get_pool_key(tier)
        .map_err(|_| StorageError::KeyNotInitialized)?;

    // Decrypt and verify
    let nonce = header.get_nonce();
    let aad = &[tier as u8];

    aead_decrypt(
        key,
        &nonce,
        aad,
        ciphertext,
        &header.tag,
        output,
    ).map_err(|_| StorageError::AuthenticationFailed)?;

    Ok(ciphertext.len())
}

/// Save all pools to storage buffer
pub fn save_pools(
    pools: &crate::memory::PoolAllocator,
    salt: &Salt,
    output: &mut [u8],
) -> StorageResult<usize> {
    let _offset = 0;

    // Write storage header
    let header_offset = StorageHeader::SIZE;
    let table_offset = header_offset;
    let data_offset = table_offset + PoolTable::SIZE;

    // Initialize pool table
    let mut pool_table = PoolTable::new();
    let mut current_data_offset = data_offset;

    // Serialize and encrypt each pool
    let mut serializer = PoolSerializer::new();
    let mut encrypted_buffer = [0u8; MAX_POOL_DATA_SIZE + EncryptedPoolHeader::SIZE];

    let pool_refs: [(&MemoryPool, SecurityTier); 4] = [
        (&pools.public, SecurityTier::Public),
        (&pools.internal, SecurityTier::Internal),
        (&pools.sensitive, SecurityTier::Sensitive),
        (&pools.secret, SecurityTier::Secret),
    ];

    for (i, (pool, tier)) in pool_refs.iter().enumerate() {
        // Serialize pool
        let plain_len = serializer.serialize_pool(pool, *tier)?;

        // Generate unique nonce for this pool
        // In production, use proper random nonce generation
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        nonce_bytes[0] = i as u8;
        nonce_bytes[4..12].copy_from_slice(&(current_data_offset as u64).to_le_bytes());
        let nonce = Nonce::from_bytes(nonce_bytes);

        // Encrypt pool data
        let encrypted_len = encrypt_pool(
            *tier,
            serializer.data(),
            &nonce,
            &mut encrypted_buffer,
        )?;

        // Update pool table entry
        pool_table.entries[i] = PoolEntry::new(
            *tier,
            current_data_offset as u32,
            encrypted_len as u32,
            plain_len as u32,
        );
        pool_table.entries[i].object_count = pool.allocated_frames() as u32;

        // Copy encrypted data to output
        if current_data_offset + encrypted_len > output.len() {
            return Err(StorageError::StorageFull);
        }
        output[current_data_offset..current_data_offset + encrypted_len]
            .copy_from_slice(&encrypted_buffer[..encrypted_len]);

        current_data_offset += encrypted_len;
        serializer.reset();
    }

    // Write pool table
    let table_bytes = unsafe {
        core::slice::from_raw_parts(
            &pool_table as *const _ as *const u8,
            PoolTable::SIZE
        )
    };
    output[table_offset..table_offset + PoolTable::SIZE].copy_from_slice(table_bytes);

    // Write storage header
    let header = StorageHeader::new(*salt, table_offset as u32, current_data_offset as u32);
    let header_bytes = unsafe {
        core::slice::from_raw_parts(
            &header as *const _ as *const u8,
            StorageHeader::SIZE
        )
    };
    output[..StorageHeader::SIZE].copy_from_slice(header_bytes);

    Ok(current_data_offset)
}

/// Load pools from storage buffer
pub fn load_pools(
    data: &[u8],
    pools: &mut crate::memory::PoolAllocator,
) -> StorageResult<()> {
    if data.len() < StorageHeader::SIZE {
        return Err(StorageError::InvalidHeader);
    }

    // Read and verify header
    let header = unsafe {
        core::ptr::read(data.as_ptr() as *const StorageHeader)
    };

    if !header.verify() {
        return Err(StorageError::ChecksumFailed);
    }

    // Read pool table
    let table_offset = header.pool_table_offset as usize;
    if table_offset + PoolTable::SIZE > data.len() {
        return Err(StorageError::InvalidHeader);
    }

    let pool_table = unsafe {
        core::ptr::read(data.as_ptr().add(table_offset) as *const PoolTable)
    };

    // Decrypt and restore each pool
    let mut decrypted_buffer = [0u8; MAX_POOL_DATA_SIZE];

    for entry in &pool_table.entries {
        if entry.data_size == 0 {
            continue;
        }

        let tier = match entry.tier {
            0 => SecurityTier::Public,
            1 => SecurityTier::Internal,
            2 => SecurityTier::Sensitive,
            3 => SecurityTier::Secret,
            _ => continue,
        };

        let data_start = entry.data_offset as usize;
        let data_end = data_start + entry.data_size as usize;

        if data_end > data.len() {
            return Err(StorageError::InvalidHeader);
        }

        // Decrypt pool data
        let decrypted_len = decrypt_pool(
            tier,
            &data[data_start..data_end],
            &mut decrypted_buffer,
        )?;

        // Deserialize and restore pool
        let mut deserializer = PoolDeserializer::new(&decrypted_buffer[..decrypted_len]);
        let metadata = deserializer.read_metadata()?;

        // Get mutable reference to the appropriate pool
        let pool = match tier {
            SecurityTier::Public => &mut pools.public,
            SecurityTier::Internal => &mut pools.internal,
            SecurityTier::Sensitive => &mut pools.sensitive,
            SecurityTier::Secret => &mut pools.secret,
        };

        // Restore frame allocations based on bitmap
        for i in 0..metadata.frame_count.min(64) {
            if metadata.allocation_bitmap & (1u64 << i) != 0 {
                // Read frame data and restore
                if let Ok(frame_data) = deserializer.read_frame(4096) {
                    pool.restore_frame(i as usize, frame_data);
                }
            }
        }
    }

    Ok(())
}

/// Generate unique nonce from pool tier and counter
pub fn generate_nonce(tier: SecurityTier, counter: u64) -> Nonce {
    let mut bytes = [0u8; NONCE_SIZE];
    bytes[0] = tier as u8;
    bytes[1] = 0x01; // Version
    bytes[2] = 0x00;
    bytes[3] = 0x00;
    bytes[4..12].copy_from_slice(&counter.to_le_bytes());
    Nonce::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_metadata_size() {
        assert_eq!(PoolMetadata::SIZE, 32);
    }
}
