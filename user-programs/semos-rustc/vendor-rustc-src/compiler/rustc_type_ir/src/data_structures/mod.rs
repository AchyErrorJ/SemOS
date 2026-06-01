use core::hash::BuildHasherDefault;

// Stage F4: ena is host-only (R3 — pulls log w/ std). On SemOS we
// don't need the unification machinery in rustc_type_ir at this v1
// layer; consumers that need UnifyKey/UnifyValue (rustc_infer) are
// themselves host-only. Re-exports are host-gated.
#[cfg(not(target_os = "none"))]
pub use ena::unify::{NoError, UnifyKey, UnifyValue};
use rustc_hash::{FxBuildHasher, FxHasher};
// `rustc_hash` only exports `FxHashMap`/`FxHashSet` with feature
// `std`; we use df=false here. Alias via hashbrown directly using
// FxBuildHasher (matches the host-feature shape).
#[cfg(not(target_os = "none"))]
pub use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
#[cfg(target_os = "none")]
pub type HashMap<K, V> = hashbrown::HashMap<K, V, FxBuildHasher>;
#[cfg(target_os = "none")]
pub type HashSet<V> = hashbrown::HashSet<V, FxBuildHasher>;

pub type IndexMap<K, V> = indexmap::IndexMap<K, V, BuildHasherDefault<FxHasher>>;
pub type IndexSet<V> = indexmap::IndexSet<V, BuildHasherDefault<FxHasher>>;

mod delayed_map;

#[cfg(feature = "nightly")]
mod impl_ {
    pub use rustc_data_structures::sso::{SsoHashMap, SsoHashSet};
    pub use rustc_data_structures::stack::ensure_sufficient_stack;
}

#[cfg(not(feature = "nightly"))]
mod impl_ {
    pub use hashbrown::{HashMap as SsoHashMap, HashSet as SsoHashSet};

    #[inline]
    pub fn ensure_sufficient_stack<R>(f: impl FnOnce() -> R) -> R {
        f()
    }
}

pub use delayed_map::{DelayedMap, DelayedSet};
pub use impl_::*;
