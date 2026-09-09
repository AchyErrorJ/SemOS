//! Object Registry
//!
//! The registry provides SUID → SemanticObject lookup, the core addressing
//! mechanism of the Semantic OS. Unlike a filesystem:
//! - Objects are addressed by identity (SUID), not location (path)
//! - Lookup is O(1) via hash table
//! - Objects can be found by semantic similarity (future: vector search)

use super::suid::SUID;
use super::object::{SemanticObject, SecurityTier};

/// Maximum objects in the registry
pub const MAX_OBJECTS: usize = 1024;

/// Registry entry (for hash table)
struct RegistryEntry {
    /// Object SUID (key)
    suid: SUID,
    /// Object data
    object: SemanticObject,
    /// Next entry in hash chain (collision handling)
    next: Option<usize>,
}

/// Hash table bucket
struct Bucket {
    /// First entry index in this bucket
    head: Option<usize>,
}

/// Object Registry - the central SUID → Object mapping
pub struct ObjectRegistry {
    /// Hash table buckets
    buckets: [Bucket; Self::NUM_BUCKETS],
    /// Object storage
    entries: [Option<RegistryEntry>; MAX_OBJECTS],
    /// Free list head
    free_head: Option<usize>,
    /// Number of objects
    count: usize,
}

impl ObjectRegistry {
    /// Number of hash buckets (power of 2 for fast modulo)
    const NUM_BUCKETS: usize = 256;

    /// Create a new empty registry
    pub const fn new() -> Self {
        // Can't use loops in const fn, so we use a workaround
        const EMPTY_BUCKET: Bucket = Bucket { head: None };
        const EMPTY_ENTRY: Option<RegistryEntry> = None;

        Self {
            buckets: [EMPTY_BUCKET; Self::NUM_BUCKETS],
            entries: [EMPTY_ENTRY; MAX_OBJECTS],
            free_head: Some(0),
            count: 0,
        }
    }

    /// Initialize the registry (must be called once at startup)
    pub fn init(&mut self) {
        // Build free list
        for i in 0..MAX_OBJECTS - 1 {
            self.entries[i] = None;
        }
        self.free_head = Some(0);
        self.count = 0;
    }

    /// Hash a SUID to a bucket index
    fn hash(suid: &SUID) -> usize {
        // Simple hash: XOR high and low, then mask
        let hash = (suid.high ^ suid.low) as usize;
        hash & (Self::NUM_BUCKETS - 1)
    }

    /// Allocate an entry slot from free list
    fn alloc_entry(&mut self) -> Option<usize> {
        // Find first empty slot
        for i in 0..MAX_OBJECTS {
            if self.entries[i].is_none() {
                return Some(i);
            }
        }
        None
    }

    /// Insert an object into the registry
    pub fn insert(&mut self, object: SemanticObject) -> bool {
        let suid = object.suid;

        // Check if already exists
        if self.get(&suid).is_some() {
            return false;
        }

        // Allocate entry
        let idx = match self.alloc_entry() {
            Some(i) => i,
            None => return false, // Registry full
        };

        let bucket_idx = Self::hash(&suid);

        // SemFS journal (write-through): append BEFORE making the object
        // visible. On disk error the insert is aborted — durable and
        // visible are the same event. No-op until the journal is mounted
        // (and during mount replay).
        if !super::journal::on_insert(&object) {
            return false;
        }

        // Create entry
        let entry = RegistryEntry {
            suid,
            object,
            next: self.buckets[bucket_idx].head,
        };

        // Insert into hash chain
        self.entries[idx] = Some(entry);
        self.buckets[bucket_idx].head = Some(idx);
        self.count += 1;

        true
    }

    /// Look up an object by SUID
    pub fn get(&self, suid: &SUID) -> Option<&SemanticObject> {
        let bucket_idx = Self::hash(suid);
        let mut current = self.buckets[bucket_idx].head;

        while let Some(idx) = current {
            if let Some(entry) = &self.entries[idx] {
                if entry.suid == *suid {
                    return Some(&entry.object);
                }
                current = entry.next;
            } else {
                break;
            }
        }

        None
    }

    /// Look up an object by SUID (mutable)
    pub fn get_mut(&mut self, suid: &SUID) -> Option<&mut SemanticObject> {
        let bucket_idx = Self::hash(suid);
        let mut current = self.buckets[bucket_idx].head;

        while let Some(idx) = current {
            // Can't use if-let with mutable borrow, so use index
            if self.entries[idx].is_some() {
                let entry_suid = self.entries[idx].as_ref().unwrap().suid;
                if entry_suid == *suid {
                    return Some(&mut self.entries[idx].as_mut().unwrap().object);
                }
                current = self.entries[idx].as_ref().unwrap().next;
            } else {
                break;
            }
        }

        None
    }

    /// Remove an object by SUID
    pub fn remove(&mut self, suid: &SUID) -> Option<SemanticObject> {
        let bucket_idx = Self::hash(suid);
        let mut prev: Option<usize> = None;
        let mut current = self.buckets[bucket_idx].head;

        while let Some(idx) = current {
            if let Some(entry) = &self.entries[idx] {
                if entry.suid == *suid {
                    // SemFS journal (write-through): append the tombstone
                    // BEFORE any in-memory mutation. On disk error, abort
                    // the remove — the registry is untouched.
                    if !super::journal::on_remove(suid) {
                        return None;
                    }

                    // Found it - remove from chain
                    let next = entry.next;

                    if let Some(prev_idx) = prev {
                        if let Some(prev_entry) = &mut self.entries[prev_idx] {
                            prev_entry.next = next;
                        }
                    } else {
                        self.buckets[bucket_idx].head = next;
                    }

                    // Take the object and free the entry
                    let removed = self.entries[idx].take().map(|e| e.object);
                    self.count -= 1;
                    return removed;
                }
                prev = current;
                current = entry.next;
            } else {
                break;
            }
        }

        None
    }

    /// Check if an object exists
    pub fn contains(&self, suid: &SUID) -> bool {
        self.get(suid).is_some()
    }

    /// Get number of objects in registry
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Iterate over all objects (for debugging/scanning)
    pub fn iter(&self) -> impl Iterator<Item = &SemanticObject> {
        self.entries.iter().filter_map(|e| e.as_ref().map(|entry| &entry.object))
    }

    /// Find objects by tier
    pub fn find_by_tier(&self, tier: SecurityTier) -> impl Iterator<Item = &SemanticObject> {
        self.iter().filter(move |obj| obj.tier == tier)
    }

    /// Find objects linked to a given SUID
    pub fn find_linked_to(&self, target: SUID) -> impl Iterator<Item = &SemanticObject> + '_ {
        self.iter().filter(move |obj| {
            obj.get_links().iter().any(|link| {
                link.as_ref().map_or(false, |l| l.target == target)
            })
        })
    }
}

/// Global registry instance, behind the yield-on-contention kernel mutex.
/// The old `static mut` + `&'static mut` pattern raced whenever a syscall
/// handler with interrupts enabled (llm_ask / agent TUI / editor) was
/// preempted mid-borrow (2026-07-17 review, P1).
static GLOBAL_REGISTRY: crate::sync::Mutex<ObjectRegistry> =
    crate::sync::Mutex::new(ObjectRegistry::new());

/// Lock the global registry and return the guard. Derefs to
/// `ObjectRegistry`, so `registry.get(...)` call sites read unchanged.
/// Never hold the guard across a blocking call or a nested `dispatch()`.
pub fn global_registry() -> crate::sync::MutexGuard<'static, ObjectRegistry> {
    GLOBAL_REGISTRY.lock()
}

/// Initialize the global registry
pub fn init_global_registry() {
    GLOBAL_REGISTRY.lock().init();
}
