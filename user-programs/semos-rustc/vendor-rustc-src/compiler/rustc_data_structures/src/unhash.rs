use core::hash::{BuildHasherDefault, Hasher};

// M27: on host the upstream uses std::collections::{HashMap, HashSet}.
// On SemOS we substitute hashbrown — same shape, no_std-compatible.
#[cfg(not(target_os = "none"))]
use std::collections::{HashMap, HashSet};
#[cfg(target_os = "none")]
use hashbrown::{HashMap, HashSet};

pub type UnhashMap<K, V> = HashMap<K, V, BuildHasherDefault<Unhasher>>;
pub type UnhashSet<V> = HashSet<V, BuildHasherDefault<Unhasher>>;
pub type UnindexMap<K, V> = indexmap::IndexMap<K, V, BuildHasherDefault<Unhasher>>;

/// This no-op hasher expects only a single `write_u64` call. It's intended for
/// map keys that already have hash-like quality, like `Fingerprint`.
#[derive(Default)]
pub struct Unhasher {
    value: u64,
}

impl Hasher for Unhasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.value
    }

    fn write(&mut self, _bytes: &[u8]) {
        unimplemented!("use write_u64");
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        debug_assert_eq!(0, self.value, "Unhasher doesn't mix values!");
        self.value = value;
    }
}
