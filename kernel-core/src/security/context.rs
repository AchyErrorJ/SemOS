//! Request Context Information
//!
//! Manages context information for policy evaluation.

pub use super::evaluation::{RequestContext, RequestType, EvaluationContext};

/// Context information for requests
pub struct ContextInfo {
    /// Additional context data
    pub data: [u8; 64],
    /// Context data length
    pub len: usize,
}

impl ContextInfo {
    pub const fn new() -> Self {
        Self {
            data: [0; 64],
            len: 0,
        }
    }
}

/// Initialize context subsystem
pub fn init() {
    // Context subsystem initialized
}