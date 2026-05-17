//! Memory management types and pool abstractions.
//!
//! Platform-independent: SecurityTier, PoolId, frame allocation logic.
//! Physical address layout and hardware init live in platform crates.

pub mod types;
pub mod pools;
pub mod frames;
pub mod secure_alloc;
pub mod heap;

pub use types::{Frame, FrameNumber, PhysicalAddress, Bytes};
pub use frames::FrameAllocator;
pub use pools::{SecurityTier, PoolAllocator, PoolId, MemoryPool};
pub use secure_alloc::{secure_alloc, secure_free, AllocResult, FreeResult};
