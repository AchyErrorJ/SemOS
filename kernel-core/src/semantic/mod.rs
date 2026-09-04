//! Semantic Object System
//!
//! This module implements the core semantic addressing model where objects
//! are identified by Semantic Unique Identifiers (SUIDs) rather than paths
//! or virtual addresses.
//!
//! # Architecture
//!
//! ```text
//! Traditional OS:              Semantic OS:
//! ┌─────────────┐             ┌─────────────────────┐
//! │ /home/doc.txt│            │ SUID: 0x3A7F...     │
//! │ (path-based) │            │ (content-addressed) │
//! └──────┬──────┘             └──────────┬──────────┘
//!        │                               │
//!        v                               v
//! ┌─────────────┐             ┌─────────────────────┐
//! │ inode table │             │ Object Registry     │
//! │ (location)  │             │ (semantic identity) │
//! └─────────────┘             └─────────────────────┘
//! ```
//!
//! # Security Model
//!
//! Objects are stored in security-tiered pools:
//! - PUBLIC (0): Full LLM access
//! - INTERNAL (1): Summarized LLM access
//! - SENSITIVE (2): Redacted LLM access
//! - SECRET (3): No LLM access

pub mod suid;
pub mod object;
pub mod registry;
pub mod journal;
pub mod capability;
pub mod vector;
pub mod search;

pub use suid::SUID;
pub use object::{SemanticObject, ObjectContent};
pub use registry::ObjectRegistry;
pub use capability::{Capability, CapabilitySet};
pub use vector::{Vector, DefaultVector, VectorIndex, SearchResult};
pub use search::SemanticSearch;
