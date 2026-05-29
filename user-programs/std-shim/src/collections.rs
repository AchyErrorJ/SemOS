//! `std::collections` re-exports — the same shape standard Rust uses.
//!
//! [`HashMap`] and [`HashSet`] come from `hashbrown` (the same crate
//! std builds on internally). Under no_std + alloc the default hasher
//! is a deterministic seed — fine for non-adversarial use (configs,
//! lookup tables, small caches). If a future caller needs hash-flood
//! resistance, we'd need to wire OS entropy (e.g. via RDRAND through a
//! syscall) into a per-HashMap seeded hasher.
//!
//! [`VecDeque`] just re-exports from `alloc`.

pub use core_alloc::collections::{BTreeMap, BTreeSet, VecDeque};
pub use hashbrown::{HashMap, HashSet};
