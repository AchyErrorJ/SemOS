//! Process Capability System
//!
//! Provides fine-grained access control based on capabilities.
//! Each capability grants permission to perform specific operations.
//!
//! # Design
//!
//! Capabilities are stored as a bitset for efficiency.
//! Processes inherit a subset of their parent's capabilities.
//!
//! # Capability Categories
//!
//! - **Memory**: Access to memory management operations
//! - **Process**: Process creation and control
//! - **File**: Filesystem operations
//! - **Device**: Hardware device access
//! - **Network**: Network operations (future)
//! - **System**: System administration
//! - **Semantic**: Semantic addressing features

/// Individual capability flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Capability {
    // Memory capabilities (0-7)
    /// Allocate memory
    MemAlloc = 0,
    /// Free memory
    MemFree = 1,
    /// Map memory pages
    MemMap = 2,
    /// Access DMA regions
    MemDma = 3,
    /// Access kernel memory (dangerous)
    MemKernel = 4,

    // Process capabilities (8-15)
    /// Spawn new processes
    ProcSpawn = 8,
    /// Kill other processes
    ProcKill = 9,
    /// Change process priority
    ProcPriority = 10,
    /// Debug other processes
    ProcDebug = 11,
    /// Set capabilities (requires careful handling)
    ProcSetCap = 12,

    // File capabilities (16-23)
    /// Read files
    FileRead = 16,
    /// Write files
    FileWrite = 17,
    /// Execute files
    FileExec = 18,
    /// Create files
    FileCreate = 19,
    /// Delete files
    FileDelete = 20,
    /// Change file permissions
    FileChmod = 21,
    /// Access any file (bypass permissions)
    FileBypass = 22,

    // Device capabilities (24-31)
    /// Access serial/console
    DevConsole = 24,
    /// Access block devices
    DevBlock = 25,
    /// Access framebuffer
    DevFramebuffer = 26,
    /// Access network devices
    DevNetwork = 27,
    /// Access any device
    DevAll = 28,

    // System capabilities (32-39)
    /// Reboot/shutdown system
    SysReboot = 32,
    /// Modify system time
    SysTime = 33,
    /// Load kernel modules
    SysModule = 34,
    /// Mount filesystems
    SysMount = 35,
    /// Access raw I/O ports
    SysRawIo = 36,

    // Semantic capabilities (40-47)
    /// Query semantic objects
    SemQuery = 40,
    /// Create semantic objects
    SemCreate = 41,
    /// Modify semantic objects
    SemModify = 42,
    /// Delete semantic objects
    SemDelete = 43,
    /// Access Public tier objects
    SemTierPublic = 44,
    /// Access Internal tier objects
    SemTierInternal = 45,
    /// Access Sensitive tier objects
    SemTierSensitive = 46,
    /// Access Secret tier objects
    SemTierSecret = 47,
}

impl Capability {
    /// Convert to bit index
    pub fn bit(self) -> u64 {
        1u64 << (self as u32)
    }

    /// Get all capabilities in a category
    pub fn category_mask(category: u32) -> u64 {
        // Each category has 8 capabilities
        0xFF << (category * 8)
    }
}

/// Set of capabilities (bitset)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitySet {
    bits: u64,
}

impl CapabilitySet {
    /// Empty capability set (no permissions)
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Full capability set (all permissions)
    pub const fn all() -> Self {
        Self { bits: !0 }
    }

    /// Check if a capability is present
    pub const fn has(&self, cap: Capability) -> bool {
        (self.bits & (1u64 << (cap as u32))) != 0
    }

    /// Add a capability
    pub fn add(&mut self, cap: Capability) {
        self.bits |= 1u64 << (cap as u32);
    }

    /// Remove a capability
    pub fn remove(&mut self, cap: Capability) {
        self.bits &= !(1u64 << (cap as u32));
    }

    /// Check if set contains all capabilities from another set
    pub const fn contains(&self, other: &CapabilitySet) -> bool {
        (self.bits & other.bits) == other.bits
    }

    /// Intersect with another set
    pub const fn intersect(&self, other: &CapabilitySet) -> CapabilitySet {
        CapabilitySet { bits: self.bits & other.bits }
    }

    /// Union with another set
    pub const fn union(&self, other: &CapabilitySet) -> CapabilitySet {
        CapabilitySet { bits: self.bits | other.bits }
    }

    /// Create a set from raw bits
    pub const fn from_bits(bits: u64) -> Self {
        Self { bits }
    }

    /// Get raw bits
    pub const fn bits(&self) -> u64 {
        self.bits
    }

    /// Create capability set for a typical user process
    pub fn default_user() -> Self {
        let mut set = Self::empty();
        // Memory
        set.add(Capability::MemAlloc);
        set.add(Capability::MemFree);
        // Process
        set.add(Capability::ProcSpawn);
        // File
        set.add(Capability::FileRead);
        set.add(Capability::FileWrite);
        set.add(Capability::FileExec);
        set.add(Capability::FileCreate);
        // Device
        set.add(Capability::DevConsole);
        // Semantic
        set.add(Capability::SemQuery);
        set.add(Capability::SemTierPublic);
        set
    }

    /// Create minimal capability set (sandboxed process)
    pub fn minimal() -> Self {
        let mut set = Self::empty();
        set.add(Capability::MemAlloc);
        set.add(Capability::MemFree);
        set.add(Capability::FileRead);
        set.add(Capability::DevConsole);
        set.add(Capability::SemQuery);
        set.add(Capability::SemTierPublic);
        set
    }

    /// Capabilities that can be inherited by child processes
    pub fn inheritable() -> Self {
        Self::from_bits(
            // Most capabilities can be inherited, except dangerous ones
            Self::all().bits
                & !(Capability::MemKernel.bit())
                & !(Capability::ProcSetCap.bit())
                & !(Capability::FileBypass.bit())
                & !(Capability::DevAll.bit())
                & !(Capability::SysRawIo.bit())
                & !(Capability::SysModule.bit())
        )
    }

    /// Calculate inherited capabilities (child gets subset of parent)
    pub fn inherit(&self) -> Self {
        self.intersect(&Self::inheritable())
    }

    /// Count number of capabilities
    pub fn count(&self) -> u32 {
        self.bits.count_ones()
    }
}

/// Capability check result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// Missing required capability
    MissingCapability(Capability),
    /// Operation not permitted
    NotPermitted,
}

/// Check if current process has a capability
pub fn check(cap: Capability) -> Result<(), CapError> {
    if let Some(proc) = super::current() {
        if proc.has_capability(cap) {
            Ok(())
        } else {
            Err(CapError::MissingCapability(cap))
        }
    } else {
        // No current process (early boot), assume kernel
        Ok(())
    }
}

/// Check if current process has all of multiple capabilities
pub fn check_all(caps: &[Capability]) -> Result<(), CapError> {
    for &cap in caps {
        check(cap)?;
    }
    Ok(())
}

/// Check if current process can access a security tier
pub fn check_tier(tier: crate::memory::SecurityTier) -> Result<(), CapError> {
    use crate::memory::SecurityTier;

    let required_cap = match tier {
        SecurityTier::Public => Capability::SemTierPublic,
        SecurityTier::Internal => Capability::SemTierInternal,
        SecurityTier::Sensitive => Capability::SemTierSensitive,
        SecurityTier::Secret => Capability::SemTierSecret,
    };

    check(required_cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_set() {
        let mut set = CapabilitySet::empty();
        assert!(!set.has(Capability::MemAlloc));

        set.add(Capability::MemAlloc);
        assert!(set.has(Capability::MemAlloc));

        set.remove(Capability::MemAlloc);
        assert!(!set.has(Capability::MemAlloc));
    }

    #[test]
    fn test_inheritance() {
        let parent = CapabilitySet::all();
        let child = parent.inherit();

        // Child should not have dangerous capabilities
        assert!(!child.has(Capability::MemKernel));
        assert!(!child.has(Capability::SysRawIo));
    }
}
