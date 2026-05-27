//! Semantic Object
//!
//! A SemanticObject is the fundamental unit of storage in the Semantic OS.
//! Unlike traditional files, objects are:
//! - Addressed by SUID (not path)
//! - Security-tiered (not user-owned)
//! - Linked to related objects (graph structure)
//! - Optionally encrypted per-tier

use super::suid::SUID;
use super::capability::CapabilitySet;

// Re-export SecurityTier from memory module (single source of truth)
pub use crate::memory::SecurityTier;

/// Maximum number of links per object
pub const MAX_LINKS: usize = 16;

/// Maximum content size — the per-file ceiling (FIXED, not "all free frames":
/// a single file must never be able to drain memory and starve everything
/// else). 2 MiB for FS large-files Model A, stage 1: content is still one
/// heap-`Allocated` blob (contiguous, so `as_bytes()` is unchanged), so the
/// real bound is the 16 MiB heap — this is the heap-safe step. Stage 2a
/// frame-backs content (contiguous frames via `phys_to_virt`) to lift this to
/// ~128 MiB without touching the heap; see ROADMAP.
pub const MAX_CONTENT_SIZE: usize = 2 * 1024 * 1024;

/// Object content type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentType {
    /// Raw bytes
    Binary = 0,
    /// UTF-8 text
    Text = 1,
    /// Vector embedding (f32 array)
    Vector = 2,
    /// Structured data (semantic graph node)
    Structured = 3,
    /// Reference to external resource
    Reference = 4,
}

/// Maximum size for a heap-Allocated object's content. Bounded so a
/// runaway FWRITE can't drain the 16 MiB kernel heap on a single file.
/// Larger than MAX_CONTENT_SIZE would be inconsistent; keep them aligned.
pub const MAX_FILE_CONTENT: usize = MAX_CONTENT_SIZE; // 2 MiB (stage 1)

/// Object content storage.
///
/// `Allocated` owns its heap block. The Drop impl frees it via
/// [`crate::memory::heap::deallocate`]. We deliberately do NOT derive
/// `Clone` — accidental bitwise copies of an `Allocated` would alias the
/// same heap pointer and double-free at drop time. If a deep copy is
/// needed in the future, add an explicit `try_clone()` that allocates a
/// fresh buffer.
pub enum ObjectContent {
    /// Inline content (small objects — ≤256 B, fits in this enum).
    Inline {
        data: [u8; 256],
        len: usize,
    },
    /// Heap-Allocated content (>256 B). `ptr` is from
    /// `kernel-core::memory::heap::allocate(capacity, 8)`.
    Allocated {
        ptr: usize,
        len: usize,
        capacity: usize,
    },
    /// Vector embedding (fixed 384 dimensions for MiniLM).
    /// NOTE: `data_ptr` is currently not owned (no Drop) — vectors are
    /// produced by static fixtures today, not heap. Revisit when LLM
    /// embeddings actually land in this enum at runtime.
    Vector {
        dimensions: u16,
        /// Pointer to f32 array
        data_ptr: usize,
    },
    /// Empty/null content
    Empty,
}

impl Default for ObjectContent {
    fn default() -> Self {
        Self::Empty
    }
}

impl Drop for ObjectContent {
    fn drop(&mut self) {
        // Only Allocated owns memory we must free. Vector's data_ptr is
        // not owned by us (see field doc above). Inline / Empty are
        // self-contained.
        if let Self::Allocated { ptr, capacity, .. } = *self {
            if ptr != 0 && capacity != 0 {
                crate::memory::heap::deallocate(ptr as *mut u8, capacity, 8);
            }
        }
    }
}

impl ObjectContent {
    /// Create inline content from bytes (legacy — ≤256 B only).
    /// Prefer [`from_bytes`] for new code so the caller doesn't have to
    /// know the inline threshold.
    pub fn from_inline(data: &[u8]) -> Option<Self> {
        if data.len() > 256 {
            return None;
        }
        let mut buf = [0u8; 256];
        buf[..data.len()].copy_from_slice(data);
        Some(Self::Inline { data: buf, len: data.len() })
    }

    /// Create content from bytes, choosing the right storage variant:
    /// - ≤ 256 B → `Inline`
    /// - 257 .. MAX_FILE_CONTENT B → heap-`Allocated`
    /// - > MAX_FILE_CONTENT → `None` (file too large)
    ///
    /// Returns `None` on OOM too.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() <= 256 {
            return Self::from_inline(data);
        }
        if data.len() > MAX_FILE_CONTENT {
            return None;
        }
        let cap = data.len();
        let ptr = crate::memory::heap::allocate(cap, 8);
        if ptr.is_null() {
            return None;
        }
        // SAFETY: heap::allocate returned a fresh, unaliased, `cap`-byte
        // block. We initialise it fully before exposing it.
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), ptr, cap);
        }
        Some(Self::Allocated { ptr: ptr as usize, len: cap, capacity: cap })
    }

    /// Get content as bytes (if available)
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Inline { data, len } => Some(&data[..*len]),
            Self::Allocated { ptr, len, .. } => {
                // SAFETY: `Allocated` owns the block; the lifetime is
                // tied to `&self`, so the slice can't outlive the
                // ObjectContent that holds the ptr.
                unsafe {
                    Some(core::slice::from_raw_parts(*ptr as *const u8, *len))
                }
            }
            _ => None,
        }
    }

    /// Get content length
    pub fn len(&self) -> usize {
        match self {
            Self::Inline { len, .. } => *len,
            Self::Allocated { len, .. } => *len,
            Self::Vector { dimensions, .. } => *dimensions as usize * 4,
            Self::Empty => 0,
        }
    }

    /// Check if content is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A link to another semantic object
#[derive(Clone, Copy)]
pub struct ObjectLink {
    /// Target object SUID
    pub target: SUID,
    /// Relationship type (semantic meaning of the link)
    pub relationship: RelationType,
}

/// Types of relationships between objects
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RelationType {
    /// Parent-child (containment)
    Contains = 0,
    /// Reference (non-owning link)
    References = 1,
    /// Derived from (transformation)
    DerivedFrom = 2,
    /// Similar to (semantic similarity)
    SimilarTo = 3,
    /// Supersedes (version relationship)
    Supersedes = 4,
    /// Custom relationship
    Custom = 255,
}

/// Object state flags
#[derive(Clone, Copy)]
pub struct ObjectFlags {
    bits: u32,
}

impl ObjectFlags {
    pub const NONE: Self = Self { bits: 0 };
    pub const ENCRYPTED: Self = Self { bits: 1 << 0 };
    pub const IMMUTABLE: Self = Self { bits: 1 << 1 };
    pub const SYSTEM: Self = Self { bits: 1 << 2 };
    pub const DELETED: Self = Self { bits: 1 << 3 };

    pub fn is_encrypted(&self) -> bool { self.bits & Self::ENCRYPTED.bits != 0 }
    pub fn is_immutable(&self) -> bool { self.bits & Self::IMMUTABLE.bits != 0 }
    pub fn is_system(&self) -> bool { self.bits & Self::SYSTEM.bits != 0 }
    pub fn is_deleted(&self) -> bool { self.bits & Self::DELETED.bits != 0 }

    pub fn set(&mut self, flag: Self) { self.bits |= flag.bits; }
    pub fn clear(&mut self, flag: Self) { self.bits &= !flag.bits; }
    pub fn as_u32(&self) -> u32 { self.bits }
}

impl Default for ObjectFlags {
    fn default() -> Self {
        Self::NONE
    }
}

/// Semantic Object - the fundamental storage unit
#[repr(C)]
pub struct SemanticObject {
    /// Unique identifier
    pub suid: SUID,

    /// Security tier (determines pool and access)
    pub tier: SecurityTier,

    /// Content type
    pub content_type: ContentType,

    /// Object flags
    pub flags: ObjectFlags,

    /// Creation timestamp (ticks since boot)
    pub created_at: u64,

    /// Last modified timestamp
    pub modified_at: u64,

    /// Owner task ID (for capability checks)
    pub owner: u8,

    /// Capabilities required to access
    pub capabilities: CapabilitySet,

    /// Object content
    pub content: ObjectContent,

    /// Links to related objects
    pub links: [Option<ObjectLink>; MAX_LINKS],

    /// Number of active links
    pub link_count: u8,
}

impl SemanticObject {
    /// Create a new empty object
    pub fn new(suid: SUID, tier: SecurityTier, owner: u8) -> Self {
        Self {
            suid,
            tier,
            content_type: ContentType::Binary,
            flags: ObjectFlags::NONE,
            created_at: 0, // TODO: Get current ticks
            modified_at: 0,
            owner,
            capabilities: CapabilitySet::empty(),
            content: ObjectContent::Empty,
            links: [None; MAX_LINKS],
            link_count: 0,
        }
    }

    /// Create an object with inline content
    pub fn with_content(suid: SUID, tier: SecurityTier, owner: u8, content: &[u8]) -> Option<Self> {
        let mut obj = Self::new(suid, tier, owner);
        obj.content = ObjectContent::from_inline(content)?;
        Some(obj)
    }

    /// Add a link to another object
    pub fn add_link(&mut self, target: SUID, relationship: RelationType) -> bool {
        if self.link_count as usize >= MAX_LINKS {
            return false;
        }
        let idx = self.link_count as usize;
        self.links[idx] = Some(ObjectLink { target, relationship });
        self.link_count += 1;
        true
    }

    /// Remove a link by target SUID
    pub fn remove_link(&mut self, target: &SUID) -> bool {
        for i in 0..self.link_count as usize {
            if let Some(link) = &self.links[i] {
                if link.target == *target {
                    // Shift remaining links
                    for j in i..self.link_count as usize - 1 {
                        self.links[j] = self.links[j + 1];
                    }
                    self.links[self.link_count as usize - 1] = None;
                    self.link_count -= 1;
                    return true;
                }
            }
        }
        false
    }

    /// Get all links
    pub fn get_links(&self) -> &[Option<ObjectLink>] {
        &self.links[..self.link_count as usize]
    }

    /// Check if this object can be accessed by a task with given tier
    pub fn can_access(&self, task_tier: SecurityTier) -> bool {
        task_tier.can_access(self.tier)
    }
}
