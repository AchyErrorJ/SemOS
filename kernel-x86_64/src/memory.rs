//! Memory Management for x86_64
//!
//! This module implements the 4-tier security memory pool system,
//! ported from the ARM64 version.
//!
//! # Security Tiers
//!
//! | Tier | Name      | LLM Access     | Use Case                  |
//! |------|-----------|----------------|---------------------------|
//! | 0    | Public    | Full verbatim  | Documentation, public data|
//! | 1    | Internal  | Summarized     | Work documents, code      |
//! | 2    | Sensitive | Redacted       | PII, personal data        |
//! | 3    | Secret    | None           | Keys, credentials         |

use bootloader_api::info::MemoryRegionKind;
use bootloader_api::BootInfo;
use spin::Mutex;
use crate::println;

// Use the single SecurityTier definition from kernel-core
pub use kernel_core::memory::SecurityTier;

/// Frame size (4KB)
pub const FRAME_SIZE: usize = 4096;

/// Maximum frames per pool
const MAX_FRAMES: usize = 256;

/// A memory pool for a specific security tier
pub struct MemoryPool {
    /// Base physical address
    base: u64,
    /// Total size in bytes
    size: usize,
    /// Frame allocation bitmap
    bitmap: [u64; MAX_FRAMES / 64],
    /// Number of frames in this pool
    frame_count: usize,
    /// Security tier
    tier: SecurityTier,
    /// Number of allocated frames
    allocated: usize,
}

impl MemoryPool {
    pub const fn new(tier: SecurityTier) -> Self {
        Self {
            base: 0,
            size: 0,
            bitmap: [0; MAX_FRAMES / 64],
            frame_count: 0,
            tier,
            allocated: 0,
        }
    }

    /// Initialize the pool with a memory region
    pub fn init(&mut self, base: u64, size: usize) {
        self.base = base;
        self.size = size;
        self.frame_count = size / FRAME_SIZE;
        self.allocated = 0;

        // Clear bitmap
        for i in 0..self.bitmap.len() {
            self.bitmap[i] = 0;
        }
    }

    /// Allocate a frame from this pool
    pub fn alloc(&mut self) -> Option<u64> {
        // Find a free frame
        for (word_idx, word) in self.bitmap.iter_mut().enumerate() {
            if *word != !0u64 {
                // Find first zero bit
                let bit = (!*word).trailing_zeros() as usize;
                let frame_idx = word_idx * 64 + bit;

                if frame_idx >= self.frame_count {
                    return None;
                }

                // Mark as allocated
                *word |= 1u64 << bit;
                self.allocated += 1;

                let addr = self.base + (frame_idx as u64 * FRAME_SIZE as u64);
                return Some(addr);
            }
        }
        None
    }

    /// Free a frame back to this pool
    pub fn free(&mut self, addr: u64) -> bool {
        if addr < self.base {
            return false;
        }

        let offset = addr - self.base;
        let frame_idx = (offset / FRAME_SIZE as u64) as usize;

        if frame_idx >= self.frame_count {
            return false;
        }

        let word_idx = frame_idx / 64;
        let bit = frame_idx % 64;

        if self.bitmap[word_idx] & (1u64 << bit) == 0 {
            return false; // Already free
        }

        self.bitmap[word_idx] &= !(1u64 << bit);
        self.allocated -= 1;
        true
    }

    /// Get pool statistics
    pub fn stats(&self) -> (usize, usize, usize) {
        (self.frame_count, self.allocated, self.frame_count - self.allocated)
    }

    pub fn tier(&self) -> SecurityTier {
        self.tier
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

/// Global memory pools
static POOLS: Mutex<[MemoryPool; 4]> = Mutex::new([
    MemoryPool::new(SecurityTier::Public),
    MemoryPool::new(SecurityTier::Internal),
    MemoryPool::new(SecurityTier::Sensitive),
    MemoryPool::new(SecurityTier::Secret),
]);

/// Usable memory info from boot
static USABLE_MEMORY: Mutex<Option<(u64, u64)>> = Mutex::new(None);

/// Initialize memory pools from boot info
pub fn init(boot_info: &BootInfo) {
    // Find the largest usable memory region
    let mut best_region: Option<(u64, u64)> = None;

    for region in boot_info.memory_regions.iter() {
        if region.kind == MemoryRegionKind::Usable {
            let size = region.end - region.start;
            match best_region {
                None => best_region = Some((region.start, size)),
                Some((_, best_size)) if size > best_size => {
                    best_region = Some((region.start, size));
                }
                _ => {}
            }
        }
    }

    let (base, total_size) = best_region.expect("No usable memory found!");

    // Store usable memory info
    *USABLE_MEMORY.lock() = Some((base, total_size));

    // Divide memory among pools
    // Public: 25%, Internal: 25%, Sensitive: 25%, Secret: 25%
    let pool_size = (total_size as usize / 4) & !(FRAME_SIZE - 1); // Align to frame

    let mut pools = POOLS.lock();

    pools[0].init(base, pool_size);
    pools[1].init(base + pool_size as u64, pool_size);
    pools[2].init(base + (pool_size * 2) as u64, pool_size);
    pools[3].init(base + (pool_size * 3) as u64, pool_size);

    println!("    Public pool:    0x{:016X} ({} KB)",
        pools[0].base(), pools[0].size() / 1024);
    println!("    Internal pool:  0x{:016X} ({} KB)",
        pools[1].base(), pools[1].size() / 1024);
    println!("    Sensitive pool: 0x{:016X} ({} KB)",
        pools[2].base(), pools[2].size() / 1024);
    println!("    Secret pool:    0x{:016X} ({} KB)",
        pools[3].base(), pools[3].size() / 1024);
}

/// Allocate a frame from a specific security tier
pub fn alloc(tier: SecurityTier) -> Option<u64> {
    let mut pools = POOLS.lock();
    pools[tier as usize].alloc()
}

/// Free a frame (automatically determines the pool)
pub fn free(addr: u64) -> bool {
    let mut pools = POOLS.lock();

    for pool in pools.iter_mut() {
        if addr >= pool.base() && addr < pool.base() + pool.size() as u64 {
            return pool.free(addr);
        }
    }
    false
}

/// Get the security tier for an address
pub fn get_tier(addr: u64) -> Option<SecurityTier> {
    let pools = POOLS.lock();

    for pool in pools.iter() {
        if addr >= pool.base() && addr < pool.base() + pool.size() as u64 {
            return Some(pool.tier());
        }
    }
    None
}

/// Check if a process with max_tier can access an address
pub fn can_access(addr: u64, max_tier: u8) -> bool {
    match get_tier(addr) {
        Some(tier) => (tier as u8) <= max_tier,
        None => false,
    }
}

/// Print memory pool statistics
pub fn print_stats() {
    let pools = POOLS.lock();

    for pool in pools.iter() {
        let (total, used, free) = pool.stats();
        println!("    {} pool: {} frames ({} used, {} free)",
            pool.tier().name(),
            total,
            used,
            free);
    }
}

/// Get pool base addresses and sizes for each tier.
/// Returns [(tier_value, base_addr, size); 4]
pub fn pool_info() -> [(u8, u64, u64); 4] {
    let pools = POOLS.lock();
    [
        (0, pools[0].base(), pools[0].size() as u64),
        (1, pools[1].base(), pools[1].size() as u64),
        (2, pools[2].base(), pools[2].size() as u64),
        (3, pools[3].base(), pools[3].size() as u64),
    ]
}

/// Get total and used memory
pub fn memory_info() -> (u64, u64) {
    let pools = POOLS.lock();
    let mut total = 0u64;
    let mut used = 0u64;

    for pool in pools.iter() {
        let (t, u, _) = pool.stats();
        total += (t * FRAME_SIZE) as u64;
        used += (u * FRAME_SIZE) as u64;
    }

    (total, used)
}
