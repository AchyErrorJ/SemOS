//! Capability-Based Access Control
//!
//! Capabilities are unforgeable tokens that grant specific access rights
//! to semantic objects. Unlike traditional ACLs:
//! - Capabilities are held by processes, not attached to files
//! - Access is checked by capability possession
//! - Capabilities can be delegated (with restrictions)

use super::suid::SUID;

/// Individual capability types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CapabilityType {
    /// Read object content
    Read = 0,
    /// Write/modify object content
    Write = 1,
    /// Execute (for code objects)
    Execute = 2,
    /// Delete object
    Delete = 3,
    /// Modify object links
    Link = 4,
    /// Change object tier (requires higher privilege)
    ChangeTier = 5,
    /// Delegate capabilities to other processes
    Delegate = 6,
    /// Full control (owner-level)
    Owner = 7,
}

impl CapabilityType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Read),
            1 => Some(Self::Write),
            2 => Some(Self::Execute),
            3 => Some(Self::Delete),
            4 => Some(Self::Link),
            5 => Some(Self::ChangeTier),
            6 => Some(Self::Delegate),
            7 => Some(Self::Owner),
            _ => None,
        }
    }

    /// Get the bit mask for this capability
    pub fn mask(&self) -> u8 {
        1 << (*self as u8)
    }
}

/// A set of capabilities (bit flags)
#[derive(Clone, Copy, Default)]
#[repr(transparent)]
pub struct CapabilitySet {
    bits: u8,
}

impl CapabilitySet {
    /// Empty capability set (no permissions)
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Full capability set (all permissions)
    pub const fn all() -> Self {
        Self { bits: 0xFF }
    }

    /// Read-only capability
    pub const fn read_only() -> Self {
        Self { bits: 1 << CapabilityType::Read as u8 }
    }

    /// Read-write capability
    pub const fn read_write() -> Self {
        Self {
            bits: (1 << CapabilityType::Read as u8) | (1 << CapabilityType::Write as u8)
        }
    }

    /// Create from a single capability
    pub const fn from_cap(cap: CapabilityType) -> Self {
        Self { bits: 1 << (cap as u8) }
    }

    /// Check if this set contains a capability
    pub fn has(&self, cap: CapabilityType) -> bool {
        self.bits & cap.mask() != 0
    }

    /// Add a capability to the set
    pub fn grant(&mut self, cap: CapabilityType) {
        self.bits |= cap.mask();
    }

    /// Remove a capability from the set
    pub fn revoke(&mut self, cap: CapabilityType) {
        self.bits &= !cap.mask();
    }

    /// Combine with another set (union)
    pub fn union(&self, other: &Self) -> Self {
        Self { bits: self.bits | other.bits }
    }

    /// Intersect with another set
    pub fn intersect(&self, other: &Self) -> Self {
        Self { bits: self.bits & other.bits }
    }

    /// Check if this set is a superset of another
    pub fn contains_all(&self, other: &Self) -> bool {
        (self.bits & other.bits) == other.bits
    }

    /// Check if this set is empty
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Get raw bits
    pub fn bits(&self) -> u8 {
        self.bits
    }
}

/// A capability token for a specific object
#[derive(Clone, Copy)]
pub struct Capability {
    /// Target object SUID
    pub target: SUID,
    /// Granted capabilities
    pub caps: CapabilitySet,
    /// Expiration time (0 = never expires)
    pub expires_at: u64,
    /// Can this capability be delegated?
    pub delegatable: bool,
}

impl Capability {
    /// Create a new capability
    pub fn new(target: SUID, caps: CapabilitySet) -> Self {
        Self {
            target,
            caps,
            expires_at: 0,
            delegatable: false,
        }
    }

    /// Create a capability with expiration
    pub fn with_expiry(target: SUID, caps: CapabilitySet, expires_at: u64) -> Self {
        Self {
            target,
            caps,
            expires_at,
            delegatable: false,
        }
    }

    /// Check if this capability grants a specific permission
    pub fn allows(&self, cap: CapabilityType) -> bool {
        self.caps.has(cap)
    }

    /// Check if capability has expired
    pub fn is_expired(&self, current_time: u64) -> bool {
        self.expires_at != 0 && current_time > self.expires_at
    }

    /// Create a delegated (possibly restricted) capability
    pub fn delegate(&self, new_caps: CapabilitySet) -> Option<Self> {
        if !self.delegatable || !self.caps.has(CapabilityType::Delegate) {
            return None;
        }

        // Can only delegate capabilities we have
        let delegated_caps = self.caps.intersect(&new_caps);

        Some(Self {
            target: self.target,
            caps: delegated_caps,
            expires_at: self.expires_at, // Inherit expiration
            delegatable: false, // Delegated caps cannot be re-delegated by default
        })
    }
}

/// Process capability table
/// Each process holds a table of capabilities for objects it can access
pub struct CapabilityTable {
    /// Capabilities held by this process
    entries: [Option<Capability>; Self::MAX_CAPS],
    /// Number of active entries
    count: usize,
}

impl CapabilityTable {
    /// Maximum capabilities per process
    pub const MAX_CAPS: usize = 64;

    /// Create an empty capability table
    pub const fn new() -> Self {
        Self {
            entries: [None; Self::MAX_CAPS],
            count: 0,
        }
    }

    /// Add a capability to the table
    pub fn insert(&mut self, cap: Capability) -> bool {
        // Check if we already have a capability for this object
        for entry in self.entries.iter_mut() {
            if let Some(existing) = entry {
                if existing.target == cap.target {
                    // Merge capabilities
                    existing.caps = existing.caps.union(&cap.caps);
                    return true;
                }
            }
        }

        // Find empty slot
        for entry in self.entries.iter_mut() {
            if entry.is_none() {
                *entry = Some(cap);
                self.count += 1;
                return true;
            }
        }

        false // Table full
    }

    /// Look up capability for an object
    pub fn lookup(&self, target: &SUID) -> Option<&Capability> {
        for entry in self.entries.iter() {
            if let Some(cap) = entry {
                if cap.target == *target {
                    return Some(cap);
                }
            }
        }
        None
    }

    /// Check if process has a specific capability for an object
    pub fn check(&self, target: &SUID, required: CapabilityType, current_time: u64) -> bool {
        if let Some(cap) = self.lookup(target) {
            !cap.is_expired(current_time) && cap.allows(required)
        } else {
            false
        }
    }

    /// Remove a capability
    pub fn remove(&mut self, target: &SUID) -> bool {
        for entry in self.entries.iter_mut() {
            if let Some(cap) = entry {
                if cap.target == *target {
                    *entry = None;
                    self.count -= 1;
                    return true;
                }
            }
        }
        false
    }

    /// Remove expired capabilities
    pub fn cleanup_expired(&mut self, current_time: u64) {
        for entry in self.entries.iter_mut() {
            if let Some(cap) = entry {
                if cap.is_expired(current_time) {
                    *entry = None;
                    self.count -= 1;
                }
            }
        }
    }

    /// Get number of active capabilities
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if table is empty
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}
