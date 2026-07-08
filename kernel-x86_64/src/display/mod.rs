//! Native display subsystem (M14-F and beyond).
//!
//! This module provides safe abstractions over Intel Haswell display MMIO
//! and, eventually, native modesetting and compositor surfaces. All
//! register access is gated by device whitelist and feature flags.

pub mod mmio;

pub mod modeset;
