//! Driver trait abstractions and device registry.
//!
//! Traits define what a driver must do (BlockDevice, CharDevice, etc.).
//! Concrete implementations live in platform crates.

pub mod traits;
pub mod registry;

pub use traits::*;
