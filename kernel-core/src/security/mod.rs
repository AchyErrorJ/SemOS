//! Security Policy Framework
//!
//! Hybrid security model: kernel-enforced framework + runtime-updateable policies
//! stored as semantic objects. Enables fine-grained, context-aware access control
//! for LLM interactions and semantic object access.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    LLM/Semantic Request                         │
//! │        (requester_id, target_suid, context)                     │
//! └─────────────────────┬───────────────────────────────────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                 Policy Engine                                   │
//! │  1. Find matching policies by target + requester               │
//! │  2. Evaluate rules in priority order                           │
//! │  3. Apply first matching rule                                  │
//! └─────────────────────┬───────────────────────────────────────────┘
//!                       │
//!          ┌────────────┼────────────┐
//!          ▼            ▼            ▼
//!    ┌─────────┐  ┌─────────┐  ┌─────────┐
//!    │ ALLOW   │  │ REDACT  │  │  DENY   │
//!    │(tier X) │  │(custom) │  │ (error) │
//!    └────┬────┘  └────┬────┘  └────┬────┘
//!         │            │            │
//!         └────────────┴────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │              Context-Aware Redactor                             │
//! │  Applies rules based on requester + object + policy            │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Policy Storage
//!
//! Policies are stored as semantic objects with reserved SUID ranges:
//! - Policy SUUIDs: `0x7000_0000_0000_0000` to `0x7FFF_FFFF_FFFF_FFFF`
//! - System policies (kernel-created): `0x7000_xxxx_xxxx_xxxx`
//! - User policies: `0x7100_xxxx_xxxx_xxxx`
//! - App-specific policies: `0x7200_xxxx_xxxx_xxxx`

pub mod policy;
pub mod evaluation;
pub mod rules;
pub mod context;

pub use policy::{PolicyObject, PolicyType, PolicyTarget, PolicyAction};
pub use evaluation::{PolicyEngine, EvaluationContext, PolicyResult};
pub use rules::{PolicyRule, RuleCondition};
pub use context::{RequestContext, ContextInfo};

use crate::semantic::SUID;
use crate::memory::SecurityTier;

/// Policy SUID ranges - policies are semantic objects with special addressing
pub mod policy_suids {
    use crate::semantic::SUID;

    /// Base for all policy SUUIDs
    pub const POLICY_BASE: u64 = 0x7000_0000_0000_0000;

    /// System policies (created by kernel, high privilege)
    pub const SYSTEM_POLICY_BASE: u64 = 0x7000_0000_0000_0000;
    pub const SYSTEM_POLICY_MASK: u64 = 0x7000_FFFF_FFFF_FFFF;

    /// User-created policies
    pub const USER_POLICY_BASE: u64 = 0x7100_0000_0000_0000;
    pub const USER_POLICY_MASK: u64 = 0x71FF_FFFF_FFFF_FFFF;

    /// Application-specific policies
    pub const APP_POLICY_BASE: u64 = 0x7200_0000_0000_0000;
    pub const APP_POLICY_MASK: u64 = 0x72FF_FFFF_FFFF_FFFF;

    /// Check if a SUID is a policy object
    pub fn is_policy_suid(suid: &SUID) -> bool {
        suid.high >= POLICY_BASE && suid.high < 0x8000_0000_0000_0000
    }

    /// Check if a policy SUID is system-level
    pub fn is_system_policy(suid: &SUID) -> bool {
        suid.high >= SYSTEM_POLICY_BASE && suid.high <= SYSTEM_POLICY_MASK
    }

    /// Generate a new system policy SUID
    pub fn new_system_policy(policy_id: u32) -> SUID {
        SUID::new(SYSTEM_POLICY_BASE | (policy_id as u64), 0)
    }

    /// Generate a new user policy SUID
    pub fn new_user_policy(user_id: u8, policy_id: u32) -> SUID {
        let high = USER_POLICY_BASE | ((user_id as u64) << 32) | (policy_id as u64);
        SUID::new(high, 0)
    }
}

/// Security framework error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityError {
    /// Policy not found
    PolicyNotFound,
    /// Access denied by policy
    AccessDenied,
    /// Policy malformed or invalid
    InvalidPolicy,
    /// Policy engine not initialized
    NotInitialized,
    /// Requester lacks permission to modify policy
    InsufficientPrivilege,
    /// Policy evaluation failed
    EvaluationError,
    /// Context information missing
    MissingContext,
}

impl SecurityError {
    /// Convert to syscall error code
    pub fn to_error_code(self) -> i64 {
        match self {
            SecurityError::PolicyNotFound => -70,
            SecurityError::AccessDenied => -71,
            SecurityError::InvalidPolicy => -72,
            SecurityError::NotInitialized => -73,
            SecurityError::InsufficientPrivilege => -74,
            SecurityError::EvaluationError => -75,
            SecurityError::MissingContext => -76,
        }
    }
}

/// User ID type for multi-user support
pub type UserId = u8;

/// Special user IDs
pub mod user_ids {
    use super::UserId;

    pub const SYSTEM: UserId = 0;      // Kernel/system processes
    pub const ADMIN: UserId = 1;       // Administrator
    pub const GUEST: UserId = 254;     // Guest/anonymous
    pub const NOBODY: UserId = 255;    // Invalid/no user
}

/// Global security framework state
static mut SECURITY_INITIALIZED: bool = false;

/// Initialize the security policy framework
pub fn init() {
    crate::platform::log("[*] Initializing security policy framework...\n");

    // Initialize submodules
    policy::init();
    evaluation::init();
    rules::init();
    context::init();

    unsafe {
        SECURITY_INITIALIZED = true;
    }

    crate::platform::log("[OK] Security policy framework initialized\n");
    crate::platform::log("  Policy SUUIDs: 0x7000_0000_0000_0000 - 0x7FFF_FFFF_FFFF_FFFF\n");
    crate::platform::log("  System policies: 0x7000_xxxx_xxxx_xxxx\n");
    crate::platform::log("  User policies:   0x7100_xxxx_xxxx_xxxx\n");
}

/// Check if security framework is initialized
pub fn is_initialized() -> bool {
    unsafe { SECURITY_INITIALIZED }
}