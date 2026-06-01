// Stage F8: odht (open-addressing hash table) is host-only — pulls
// stable_deref_trait/log via incompatible features. The SemOS-target
// rustc never emits / loads the rmeta-side def-path hash map (no
// incremental cache per §1.3), so we replace it with a simple
// alloc::collections::BTreeMap<Hash64, DefIndex> stub here.

use rustc_hashes::Hash64;
use rustc_span::def_id::DefIndex;

#[derive(Clone, Default)]
pub struct Config;

#[cfg(not(target_os = "none"))]
impl odht::Config for Config {
    // This hash-map is single-crate, so we only need to key by the local hash.
    type Key = Hash64;
    type Value = DefIndex;

    type EncodedKey = [u8; 8];
    type EncodedValue = [u8; 4];

    type H = odht::UnHashFn;

    #[inline]
    fn encode_key(k: &Hash64) -> [u8; 8] {
        k.as_u64().to_le_bytes()
    }

    #[inline]
    fn encode_value(v: &DefIndex) -> [u8; 4] {
        v.as_u32().to_le_bytes()
    }

    #[inline]
    fn decode_key(k: &[u8; 8]) -> Hash64 {
        Hash64::new(u64::from_le_bytes(*k))
    }

    #[inline]
    fn decode_value(v: &[u8; 4]) -> DefIndex {
        DefIndex::from_u32(u32::from_le_bytes(*v))
    }
}

#[cfg(not(target_os = "none"))]
pub type DefPathHashMap = odht::HashTableOwned<Config>;
#[cfg(target_os = "none")]
pub type DefPathHashMap = alloc::collections::BTreeMap<Hash64, DefIndex>;
