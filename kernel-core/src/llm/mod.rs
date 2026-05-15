//! LLM Service Integration
//!
//! Provides LLM access control with tiered context building.
//! The LLM service respects security tiers and applies appropriate
//! filtering (redaction, summarization) based on access level.
//!
//! # Security Model
//!
//! ```text
//! Tier 0 (PUBLIC):    Full LLM access - content included verbatim
//! Tier 1 (INTERNAL):  Summarized access - LLM sees summaries only
//! Tier 2 (SENSITIVE): Redacted access - PII and sensitive data masked
//! Tier 3 (SECRET):    No LLM access - completely excluded
//! ```
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    LLM Request                          │
//! │  "Find documents about project X"                       │
//! └─────────────────────┬───────────────────────────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │               Context Builder                           │
//! │  - Query semantic objects                               │
//! │  - Filter by requester's max tier                       │
//! │  - Apply tier-specific transformations                  │
//! └─────────────────────┬───────────────────────────────────┘
//!                       │
//!         ┌─────────────┼─────────────┐
//!         ▼             ▼             ▼
//!    ┌─────────┐  ┌─────────┐  ┌─────────┐
//!    │ Tier 0  │  │ Tier 1  │  │ Tier 2  │
//!    │ Verbatim│  │ Summary │  │ Redacted│
//!    └────┬────┘  └────┬────┘  └────┬────┘
//!         │            │            │
//!         └────────────┴────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │               LLM Provider                              │
//! │  - Local inference (future)                             │
//! │  - Remote API call (future)                             │
//! └─────────────────────────────────────────────────────────┘
//! ```

pub mod context_builder;
pub mod redact;
pub mod context_redact;
pub mod summarize;
pub mod access_request;
pub mod provider;
pub mod transport;
pub mod net_provider;

pub use context_builder::{ContextBuilder, ContextEntry, LlmContext};
pub use redact::{Redactor, RedactionLevel};
pub use context_redact::{ContextAwareRedactor, RedactionContext, init as init_context_redactor};
pub use summarize::{Summarizer, Summary};
pub use access_request::{AccessRequest, AccessDecision, EscalationQueue};
pub use provider::{LlmProvider, LlmRequest, LlmResponse};
pub use transport::{NetworkTransport, TransportError, LoopbackTransport};
pub use net_provider::{NetworkLlmProvider, Endpoint, TransportKind, ApiFormat};

use crate::memory::SecurityTier;

/// Maximum context entries for LLM
pub const MAX_CONTEXT_ENTRIES: usize = 64;

/// Maximum content size per entry (bytes)
pub const MAX_ENTRY_SIZE: usize = 4096;

/// Maximum total context size (bytes)
pub const MAX_CONTEXT_SIZE: usize = 32768;

/// LLM service error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmError {
    /// Service not initialized
    NotInitialized,
    /// Access denied due to tier restriction
    AccessDenied,
    /// Context too large
    ContextTooLarge,
    /// No matching objects found
    NoResults,
    /// Provider not available
    ProviderUnavailable,
    /// Request pending escalation
    PendingEscalation,
    /// Invalid request format
    InvalidRequest,
    /// Internal error
    InternalError,
}

impl LlmError {
    /// Convert to syscall error code
    pub fn to_error_code(self) -> i64 {
        match self {
            LlmError::NotInitialized => -50,
            LlmError::AccessDenied => -51,
            LlmError::ContextTooLarge => -52,
            LlmError::NoResults => -53,
            LlmError::ProviderUnavailable => -54,
            LlmError::PendingEscalation => -55,
            LlmError::InvalidRequest => -56,
            LlmError::InternalError => -57,
        }
    }
}

/// Determine the effective tier for LLM access
/// This maps security tiers to LLM access levels
pub fn effective_llm_tier(object_tier: SecurityTier, requester_max_tier: u8) -> Option<SecurityTier> {
    let obj_tier_val = object_tier as u8;

    // Requester can't access objects above their tier
    if obj_tier_val > requester_max_tier {
        return None;
    }

    // Return the object's tier (determines how content is processed)
    Some(object_tier)
}

/// Global LLM service state
static mut LLM_SERVICE_INITIALIZED: bool = false;

/// Initialize the LLM service
pub fn init() {
    crate::platform::log("[*] Initializing LLM service...\n");

    // Initialize submodules
    context_builder::init();
    redact::init();
    context_redact::init();
    summarize::init();
    access_request::init();
    provider::init();
    transport::init();
    net_provider::init();

    unsafe {
        LLM_SERVICE_INITIALIZED = true;
    }

    crate::platform::log("[OK] LLM service initialized\n");
    crate::platform::log("  Tier 0: Full access (verbatim)\n");
    crate::platform::log("  Tier 1: Summarized access\n");
    crate::platform::log("  Tier 2: Redacted access\n");
    crate::platform::log("  Tier 3: No LLM access\n");
}

/// Check if LLM service is initialized
pub fn is_initialized() -> bool {
    unsafe { LLM_SERVICE_INITIALIZED }
}
