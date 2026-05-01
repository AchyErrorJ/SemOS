//! Secure Memory Allocation with Tier Enforcement
//!
//! Tracks all allocations with security tier metadata and enforces
//! that tasks can only allocate from tiers they have access to.
//!
//! Note: This module doesn't directly access scheduler or Uart - caller must
//! provide task context and handle printing.

use super::pools::{PoolAllocator, SecurityTier};
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of tracked allocations
pub const MAX_ALLOCATIONS: usize = 64;

/// Allocation record
#[derive(Clone, Copy)]
pub struct AllocationRecord {
    /// Physical address of allocation (0 = unused slot)
    pub addr: u64,
    /// Size in bytes
    pub size: u64,
    /// Security tier
    pub tier: u8,
    /// Owning task ID
    pub owner: u8,
    /// Flags (reserved)
    pub flags: u8,
}

impl AllocationRecord {
    pub const fn empty() -> Self {
        Self {
            addr: 0,
            size: 0,
            tier: 0,
            owner: 0,
            flags: 0,
        }
    }

    pub fn is_free(&self) -> bool {
        self.addr == 0
    }
}

/// Allocation table for tracking all secure allocations
pub struct AllocationTable {
    records: [AllocationRecord; MAX_ALLOCATIONS],
    count: AtomicU64,
}

impl AllocationTable {
    pub const fn new() -> Self {
        Self {
            records: [AllocationRecord::empty(); MAX_ALLOCATIONS],
            count: AtomicU64::new(0),
        }
    }

    /// Find a free slot
    fn find_free_slot(&mut self) -> Option<usize> {
        for i in 0..MAX_ALLOCATIONS {
            if self.records[i].is_free() {
                return Some(i);
            }
        }
        None
    }

    /// Find allocation by address
    fn find_by_addr(&self, addr: u64) -> Option<usize> {
        for i in 0..MAX_ALLOCATIONS {
            if self.records[i].addr == addr && !self.records[i].is_free() {
                return Some(i);
            }
        }
        None
    }

    /// Record a new allocation
    pub fn record(&mut self, addr: u64, size: u64, tier: u8, owner: u8) -> bool {
        if let Some(slot) = self.find_free_slot() {
            self.records[slot] = AllocationRecord {
                addr,
                size,
                tier,
                owner,
                flags: 0,
            };
            self.count.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Remove an allocation record
    pub fn remove(&mut self, addr: u64) -> Option<AllocationRecord> {
        if let Some(slot) = self.find_by_addr(addr) {
            let record = self.records[slot];
            self.records[slot] = AllocationRecord::empty();
            self.count.fetch_sub(1, Ordering::Relaxed);
            Some(record)
        } else {
            None
        }
    }

    /// Get allocation info by address
    pub fn get(&self, addr: u64) -> Option<&AllocationRecord> {
        if let Some(slot) = self.find_by_addr(addr) {
            Some(&self.records[slot])
        } else {
            None
        }
    }

    /// Get allocation count
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Get all active allocation records (for printing elsewhere)
    pub fn get_active_records(&self) -> impl Iterator<Item = (usize, &AllocationRecord)> {
        self.records.iter().enumerate().filter(|(_, rec)| !rec.is_free())
    }
}

/// Static allocation table
static mut ALLOC_TABLE: AllocationTable = AllocationTable::new();

/// Secure allocation result
#[derive(Debug, Clone, Copy)]
pub enum AllocResult {
    Success(u64),
    PermissionDenied,
    OutOfMemory,
    TableFull,
    InvalidTier,
}

/// Secure free result
#[derive(Debug, Clone, Copy)]
pub enum FreeResult {
    Success,
    NotFound,
    NotOwner,
}

/// Allocate memory with tier enforcement
///
/// task_tier: maximum tier the current task can access
/// task_id: current task ID (for ownership tracking)
/// Returns the physical address if successful, or an error
pub fn secure_alloc(
    allocator: &mut PoolAllocator,
    tier: u8,
    size: usize,
    task_tier: u8,
    task_id: u8,
) -> AllocResult {
    // Check tier permission
    if tier > task_tier {
        return AllocResult::PermissionDenied;
    }

    // Convert tier number to SecurityTier
    let security_tier = match tier {
        0 => SecurityTier::Public,
        1 => SecurityTier::Internal,
        2 => SecurityTier::Sensitive,
        3 => SecurityTier::Secret,
        _ => return AllocResult::InvalidTier,
    };

    // Attempt allocation from the pool
    match allocator.allocate(security_tier, size) {
        Some(addr) => {
            let addr_val = addr.0 as u64;

            // Record the allocation
            unsafe {
                let table = &raw mut ALLOC_TABLE;
                if (*table).record(addr_val, size as u64, tier, task_id) {
                    AllocResult::Success(addr_val)
                } else {
                    // Table full - should deallocate, but for now just report error
                    AllocResult::TableFull
                }
            }
        }
        None => AllocResult::OutOfMemory,
    }
}

/// Free memory with ownership check
/// task_id: current task ID (for ownership verification)
pub fn secure_free(addr: u64, task_id: u8) -> FreeResult {
    unsafe {
        let table = &raw mut ALLOC_TABLE;

        // Look up the allocation
        if let Some(record) = (*table).get(addr) {
            // Check ownership (kernel task 0 can free anything)
            if record.owner != task_id && task_id != 0 {
                return FreeResult::NotOwner;
            }

            // Remove from table
            (*table).remove(addr);
            // Note: actual memory deallocation would happen here
            // For now we just track it
            FreeResult::Success
        } else {
            FreeResult::NotFound
        }
    }
}

/// Check if a task with given tier can access an address
/// task_tier: maximum tier the current task can access
pub fn can_access(addr: u64, task_tier: u8) -> bool {
    unsafe {
        let table = &raw const ALLOC_TABLE;

        if let Some(record) = (*table).get(addr) {
            record.tier <= task_tier
        } else {
            // Unknown address - deny by default
            false
        }
    }
}

/// Get the tier of an allocation
pub fn get_tier(addr: u64) -> Option<u8> {
    unsafe {
        let table = &raw const ALLOC_TABLE;
        (*table).get(addr).map(|r| r.tier)
    }
}

/// Get allocation count
pub fn get_allocation_count() -> u64 {
    unsafe {
        let table = &raw const ALLOC_TABLE;
        (*table).count()
    }
}

/// Iterate over all active allocations (for printing in caller)
/// Returns (index, addr, size, tier, owner) for each active allocation
pub fn iter_allocations<F>(mut f: F)
where
    F: FnMut(usize, u64, u64, u8, u8),
{
    unsafe {
        let table = &raw const ALLOC_TABLE;
        for (i, rec) in (*table).get_active_records() {
            f(i, rec.addr, rec.size, rec.tier, rec.owner);
        }
    }
}

/// Tier names for display
pub fn tier_name(tier: u8) -> &'static str {
    match tier {
        0 => "Public",
        1 => "Internal",
        2 => "Sensitive",
        3 => "Secret",
        _ => "Unknown",
    }
}
