//! Semantic OS - Platform-Independent Kernel Core
//!
//! This crate contains all architecture-independent kernel logic:
//! - Semantic object system (SUID addressing, registry, vector search)
//! - Security tiers and capability-based access control
//! - Cryptography (ChaCha20-Poly1305, PBKDF2, HKDF)
//! - LLM service integration (context building, redaction, summarization)
//! - Storage persistence (encrypted pool serialization)
//! - RAM filesystem
//! - Driver trait abstractions
//!
//! Platform-specific code (boot, interrupts, MMU, UART, context switch)
//! lives in separate crates: `kernel-arm64`, `kernel-x86_64`, etc.
//!
//! # Platform Abstraction
//!
//! Platform crates implement the [`Platform`] trait and register themselves
//! via [`set_platform`] at boot time. All kernel-core code uses this trait
//! for I/O and timing instead of direct hardware access.

#![no_std]
#![allow(dead_code)]
#![allow(static_mut_refs)]

pub mod platform;
pub mod memory;
pub mod semantic;
pub mod crypto;
pub mod llm;
pub mod storage;
pub mod fs;
pub mod drivers;
pub mod scheduler;
pub mod process;
pub mod ipc;
pub mod syscall;

// Re-exports for convenience
pub use platform::{Platform, set_platform, log, ticks};
pub use memory::{SecurityTier, PoolId};
pub use semantic::{SUID, SemanticObject, ObjectRegistry};
