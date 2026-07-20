//! Pool allocator for security-tiered memory
//!
//! Manages 4 isolated memory pools for the security tiers.

use crate::memory::types::{Bytes, Frame, PhysicalAddress};
use crate::memory::frames::FrameAllocator;
use core::sync::atomic::{AtomicU64, Ordering};

/// Security tier for memory pools
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SecurityTier {
    /// Full LLM access, shareable externally
    Public = 0,
    /// Summarized LLM access, organization-internal
    Internal = 1,
    /// Redacted LLM access, PII & proprietary docs
    Sensitive = 2,
    /// No LLM access, credentials & keys
    Secret = 3,
}

impl SecurityTier {
    /// Get tier from value
    pub const fn from_value(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Public),
            1 => Some(Self::Internal),
            2 => Some(Self::Sensitive),
            3 => Some(Self::Secret),
            _ => None,
        }
    }

    /// Get numeric value
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// Alias for from_value (compatibility with semantic::object code)
    pub fn from_u8(val: u8) -> Option<Self> {
        Self::from_value(val)
    }

    /// Get human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Public => "Public",
            Self::Internal => "Internal",
            Self::Sensitive => "Sensitive",
            Self::Secret => "Secret",
        }
    }

    /// Check if this tier can access another tier
    /// Rule: Can only access equal or lower tiers
    pub fn can_access(&self, target: SecurityTier) -> bool {
        *self as u8 >= target as u8
    }
}

/// Pool identifier (based on security tier)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PoolId(pub u64);

impl PoolId {
    /// Create pool ID from security tier
    pub fn from_tier(tier: SecurityTier) -> Self {
        Self(tier as u64)
    }

    /// Get the tier from this pool ID
    pub fn tier(self) -> SecurityTier {
        match self.0 {
            0 => SecurityTier::Public,
            1 => SecurityTier::Internal,
            2 => SecurityTier::Sensitive,
            3 => SecurityTier::Secret,
            _ => SecurityTier::Public, // Default fallback
        }
    }
}

/// Memory pool for a security tier
pub struct MemoryPool {
    /// Security tier
    pub tier: SecurityTier,
    /// Pool identifier
    pub id: PoolId,
    /// Base physical address
    pub base_addr: PhysicalAddress,
    /// Pool size in bytes
    pub size: Bytes,
    /// Currently used bytes
    pub used: AtomicU64,
    /// Frame allocator for this pool
    pub allocator: FrameAllocator,
}

impl MemoryPool {
    /// Initialize a memory pool in-place to avoid stack allocation
    ///
    /// # Safety
    /// - `ptr` must point to valid, uninitialized memory for MemoryPool
    /// - The memory region must be valid and not overlap other pools
    #[inline(never)]
    pub unsafe fn init_in_place(
        ptr: *mut Self,
        tier: SecurityTier,
        base_addr: PhysicalAddress,
        size: usize,
    ) {
        use core::ptr::addr_of_mut;

        // Write each field directly to avoid creating intermediate Self on stack
        addr_of_mut!((*ptr).tier).write(tier);
        addr_of_mut!((*ptr).id).write(PoolId::from_tier(tier));
        addr_of_mut!((*ptr).base_addr).write(base_addr);
        addr_of_mut!((*ptr).size).write(Bytes::new(size));
        addr_of_mut!((*ptr).used).write(AtomicU64::new(0));

        // Initialize the frame allocator in place
        FrameAllocator::init_in_place(addr_of_mut!((*ptr).allocator), base_addr, size);
    }

    /// Create a new memory pool (WARNING: uses stack space)
    ///
    /// # Safety
    /// The memory region must be valid and not overlap other pools
    #[allow(dead_code)]
    pub unsafe fn new(tier: SecurityTier, base_addr: PhysicalAddress, size: usize) -> Self {
        Self {
            tier,
            id: PoolId::from_tier(tier),
            base_addr,
            size: Bytes::new(size),
            used: AtomicU64::new(0),
            allocator: FrameAllocator::new(base_addr, size),
        }
    }

    /// Allocate memory from this pool
    pub fn allocate(&mut self, size: usize, alignment: usize) -> Option<PhysicalAddress> {
        let aligned_size = (size + alignment - 1) & !(alignment - 1);
        let frames_needed = aligned_size / 4096 + if aligned_size % 4096 != 0 { 1 } else { 0 };

        let start_frame = self.allocator.allocate_frames(frames_needed)?;

        // Calculate absolute address: pool base + frame offset
        let frame_offset = start_frame.number.0 * 4096;
        let addr = PhysicalAddress::new(self.base_addr.0 + frame_offset);

        // NOTE: Skip atomic update to used counter due to LLVM bug with fetch_add
        // self.used.fetch_add(aligned_size as u64, Ordering::AcqRel);

        Some(addr)
    }

    /// Free memory back to pool
    pub fn deallocate(&mut self, addr: PhysicalAddress, size: usize) {
        let frames_needed = size / 4096 + if size % 4096 != 0 { 1 } else { 0 };
        let start_frame = Frame::containing(addr);

        for i in 0..frames_needed {
            self.allocator.deallocate_frame(Frame::new(crate::memory::types::FrameNumber::new(
                start_frame.number.0 + i,
            )));
        }

        self.used.fetch_sub(size as u64, Ordering::AcqRel);
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            tier: self.tier,
            total_bytes: self.size.0,
            used_bytes: self.used.load(Ordering::Acquire) as usize,
            free_frames: self.allocator.free_frames(),
        }
    }

    // ========== Persistence Support Methods ==========

    /// Get total number of frames in this pool
    pub fn frame_count(&self) -> usize {
        self.allocator.total_frames()
    }

    /// Check if a specific frame is allocated
    pub fn is_frame_allocated(&self, frame_index: usize) -> bool {
        self.allocator.is_frame_allocated_public(frame_index)
    }

    /// Get the raw data of a frame (for serialization)
    ///
    /// # Safety
    /// The frame must be within this pool's range
    pub fn get_frame_data(&self, frame_index: usize) -> &[u8] {
        let frame_addr = self.base_addr.0 + (frame_index * 4096);
        unsafe {
            core::slice::from_raw_parts(frame_addr as *const u8, 4096)
        }
    }

    /// Restore frame data (for deserialization)
    ///
    /// # Safety
    /// The frame must be within this pool's range and data must be 4096 bytes
    pub fn restore_frame(&mut self, frame_index: usize, data: &[u8]) {
        if data.len() != 4096 || frame_index >= self.frame_count() {
            return;
        }

        let frame_addr = self.base_addr.0 + (frame_index * 4096);
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                frame_addr as *mut u8,
                4096,
            );
        }

        // Mark frame as allocated
        self.allocator.mark_frame_allocated(frame_index);
    }

    /// Get number of allocated frames
    pub fn allocated_frames(&self) -> usize {
        self.allocator.total_frames() - self.allocator.free_frames()
    }

    /// Get base address
    pub fn base_address(&self) -> usize {
        self.base_addr.0
    }
}

/// Pool statistics
#[derive(Debug, Clone, Copy)]
pub struct PoolStats {
    pub tier: SecurityTier,
    pub total_bytes: usize,
    pub used_bytes: usize,
    pub free_frames: usize,
}

/// Manages all 4 security pools
pub struct PoolAllocator {
    /// Public pool
    pub public: MemoryPool,
    /// Internal pool
    pub internal: MemoryPool,
    /// Sensitive pool
    pub sensitive: MemoryPool,
    /// Secret pool
    pub secret: MemoryPool,
}

impl PoolAllocator {
    /// Memory layout constants for QEMU virt
    /// Pools start at 0x42000000 (well after kernel at 0x40200000)
    const PUBLIC_BASE: usize = 0x42000000;
    const PUBLIC_SIZE: usize = 4 * 1024; // 4KB (1 page)

    const INTERNAL_BASE: usize = Self::PUBLIC_BASE + Self::PUBLIC_SIZE;
    const INTERNAL_SIZE: usize = 4 * 1024; // 4KB (1 page)

    const SENSITIVE_BASE: usize = Self::INTERNAL_BASE + Self::INTERNAL_SIZE;
    const SENSITIVE_SIZE: usize = 8 * 1024; // 8KB (2 pages)

    const SECRET_BASE: usize = Self::SENSITIVE_BASE + Self::SENSITIVE_SIZE;
    const SECRET_SIZE: usize = 16 * 1024; // 16KB (4 pages)

    /// Initialize pool allocator in-place to avoid stack allocation
    ///
    /// # Safety
    /// - `ptr` must point to valid, uninitialized memory for PoolAllocator
    /// - Must be called exactly once per allocation
    #[inline(never)]
    pub unsafe fn init_in_place(ptr: *mut Self) {
        use core::ptr::addr_of_mut;

        // Initialize each pool field directly without intermediate stack allocation
        MemoryPool::init_in_place(
            addr_of_mut!((*ptr).public),
            SecurityTier::Public,
            PhysicalAddress::new(Self::PUBLIC_BASE),
            Self::PUBLIC_SIZE,
        );

        MemoryPool::init_in_place(
            addr_of_mut!((*ptr).internal),
            SecurityTier::Internal,
            PhysicalAddress::new(Self::INTERNAL_BASE),
            Self::INTERNAL_SIZE,
        );

        MemoryPool::init_in_place(
            addr_of_mut!((*ptr).sensitive),
            SecurityTier::Sensitive,
            PhysicalAddress::new(Self::SENSITIVE_BASE),
            Self::SENSITIVE_SIZE,
        );

        MemoryPool::init_in_place(
            addr_of_mut!((*ptr).secret),
            SecurityTier::Secret,
            PhysicalAddress::new(Self::SECRET_BASE),
            Self::SECRET_SIZE,
        );
    }

    /// Create the 4 isolated security pools (WARNING: uses stack space)
    ///
    /// # Safety
    /// Must be called once during kernel init
    #[allow(dead_code)]
    pub unsafe fn new() -> Self {
        Self {
            public: MemoryPool::new(
                SecurityTier::Public,
                PhysicalAddress::new(Self::PUBLIC_BASE),
                Self::PUBLIC_SIZE,
            ),
            internal: MemoryPool::new(
                SecurityTier::Internal,
                PhysicalAddress::new(Self::INTERNAL_BASE),
                Self::INTERNAL_SIZE,
            ),
            sensitive: MemoryPool::new(
                SecurityTier::Sensitive,
                PhysicalAddress::new(Self::SENSITIVE_BASE),
                Self::SENSITIVE_SIZE,
            ),
            secret: MemoryPool::new(
                SecurityTier::Secret,
                PhysicalAddress::new(Self::SECRET_BASE),
                Self::SECRET_SIZE,
            ),
        }
    }

    /// Get a pool by tier
    pub fn get_pool(&self, tier: SecurityTier) -> &MemoryPool {
        match tier {
            SecurityTier::Public => &self.public,
            SecurityTier::Internal => &self.internal,
            SecurityTier::Sensitive => &self.sensitive,
            SecurityTier::Secret => &self.secret,
        }
    }

    /// Allocate from a specific tier
    pub fn allocate(&mut self, tier: SecurityTier, size: usize) -> Option<PhysicalAddress> {
        match tier {
            SecurityTier::Public => self.public.allocate(size, 4096),
            SecurityTier::Internal => self.internal.allocate(size, 4096),
            SecurityTier::Sensitive => self.sensitive.allocate(size, 4096),
            SecurityTier::Secret => self.secret.allocate(size, 4096),
        }
    }

    /// Print pool statistics (for debugging)
    pub fn print_stats(&self) {

        crate::platform::log("\n=== Memory Pool Statistics ===\n");

        let pools = [
            ("Public", self.public.stats()),
            ("Internal", self.internal.stats()),
            ("Sensitive", self.sensitive.stats()),
            ("Secret", self.secret.stats()),
        ];

        for (name, _stats) in pools {
            crate::platform::log(name);
            crate::platform::log(" pool: ");
            // Print used/total (simplified)
            crate::platform::log("allocated\n");
        }

        crate::platform::log("============================\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_values() {
        assert_eq!(SecurityTier::Public.value(), 0);
        assert_eq!(SecurityTier::Secret.value(), 3);
    }

    #[test]
    fn test_pool_allocate() {
        let mut pool = unsafe { MemoryPool::new(SecurityTier::Public, PhysicalAddress::new(0x10000000), 4096 * 16) };

        let addr1 = pool.allocate(4096, 4096);
        assert!(addr1.is_some());

        let addr2 = pool.allocate(4096, 4096);
        assert!(addr2.is_some());

        assert_ne!(addr1.unwrap().0, addr2.unwrap().0);
    }
}
