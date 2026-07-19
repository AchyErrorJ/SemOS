//! Context-Aware Redaction Engine
//!
//! Advanced redactor that applies different redaction rules based on:
//! - Security policies for the target object
//! - Requester identity and security tier
//! - Request context (time, purpose, etc.)
//! - Custom redaction profiles (Medical, Financial, etc.)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Redaction Request                             │
//! │    (content, target_suid, requester_id, context)               │
//! └─────────────────────┬───────────────────────────────────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │               Policy Evaluation                                 │
//! │  Check security policies for redaction requirements             │
//! └─────────────────────┬───────────────────────────────────────────┘
//!                       │
//!          ┌────────────┼────────────┐
//!          ▼            ▼            ▼
//!    ┌─────────┐  ┌─────────┐  ┌─────────┐
//!    │ MEDICAL │  │FINANCIAL│  │ CUSTOM  │
//!    │ Profile │  │ Profile │  │ Profile │
//!    └────┬────┘  └────┬────┘  └────┬────┘
//!         │            │            │
//!         └────────────┴────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │              Pattern-Based Redaction                            │
//! │  Apply specific patterns based on selected profile              │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use crate::semantic::SUID;
use crate::memory::SecurityTier;
use crate::security::{
    UserId,
    evaluation::{create_evaluation_context, RequestType, PolicyResult, global_policy_engine},
    policy::RedactionProfile,
};
use super::redact::{Redactor, RedactionLevel, tokens};

/// Context information for redaction decisions
#[derive(Clone, Copy)]
pub struct RedactionContext {
    /// Who is requesting access
    pub requester_id: UserId,
    /// Requester's maximum tier
    pub requester_tier: SecurityTier,
    /// Target object SUID
    pub target_suid: SUID,
    /// Type of request (LLM context, direct access, etc.)
    pub request_type: RequestType,
    /// Additional context flags
    pub context_flags: u32,
    /// Application-specific context
    pub app_context: u32,
}

/// Context-aware redaction engine
pub struct ContextAwareRedactor {
    /// Base redactor for standard patterns
    base_redactor: Redactor,
    /// Is engine initialized
    initialized: bool,
    /// Default redaction profile
    default_profile: RedactionProfile,
}

impl ContextAwareRedactor {
    /// Create a new context-aware redactor
    pub const fn new() -> Self {
        Self {
            base_redactor: Redactor::new(),
            initialized: false,
            default_profile: RedactionProfile::Standard,
        }
    }

    /// Initialize the context-aware redactor
    pub fn init(&mut self) {
        self.base_redactor.init();
        self.initialized = true;
    }

    /// Set default redaction profile
    pub fn set_default_profile(&mut self, profile: RedactionProfile) {
        self.default_profile = profile;
    }

    /// Redact content using context-aware policies
    pub fn redact_with_context(
        &self,
        content: &[u8],
        context: &RedactionContext,
        output: &mut [u8],
    ) -> usize {
        if !self.initialized {
            return 0;
        }

        // Evaluate security policies to determine redaction requirements
        let redaction_profile = self.determine_redaction_profile(context);

        // Apply profile-specific redaction
        self.apply_redaction_profile(content, redaction_profile, output)
    }

    /// Determine which redaction profile to use based on context and policies
    fn determine_redaction_profile(&self, context: &RedactionContext) -> RedactionProfile {
        // Create evaluation context for policy engine
        let eval_context = create_evaluation_context(
            context.requester_id,
            context.requester_tier,
            context.target_suid,
            context.request_type,
        );

        // Check if security policies specify a redaction profile
        let policy_engine = global_policy_engine();
        let policy_result = policy_engine.evaluate(&eval_context);
        map_policy_result(policy_result, context.requester_tier, self.default_profile)
    }

    /// Apply specific redaction profile to content
    fn apply_redaction_profile(
        &self,
        content: &[u8],
        profile: RedactionProfile,
        output: &mut [u8],
    ) -> usize {
        match profile {
            RedactionProfile::None => {
                // Passthrough: requester is cleared for the content outright.
                let copy_len = content.len().min(output.len());
                output[..copy_len].copy_from_slice(&content[..copy_len]);
                copy_len
            }
            RedactionProfile::Standard => {
                // Standard PII redaction
                self.base_redactor.redact(content, output)
            }
            RedactionProfile::Medical => {
                self.apply_medical_redaction(content, output)
            }
            RedactionProfile::Financial => {
                self.apply_financial_redaction(content, output)
            }
            RedactionProfile::NamesOnly => {
                self.apply_names_only_redaction(content, output)
            }
            RedactionProfile::Full => {
                self.apply_full_redaction(content, output)
            }
            RedactionProfile::Custom(profile_id) => {
                self.apply_custom_redaction(content, output, profile_id)
            }
        }
    }

    /// Apply medical privacy redaction (HIPAA-style)
    fn apply_medical_redaction(&self, content: &[u8], output: &mut [u8]) -> usize {
        // Start with standard redaction
        let mut temp_buffer = [0u8; super::MAX_ENTRY_SIZE];
        let base_len = self.base_redactor.redact(content, &mut temp_buffer);

        // Apply additional medical-specific patterns
        let medical_redacted = self.apply_medical_patterns(&temp_buffer[..base_len], output);
        medical_redacted
    }

    /// Apply financial privacy redaction (PCI-style)
    fn apply_financial_redaction(&self, content: &[u8], output: &mut [u8]) -> usize {
        // Start with standard redaction (includes credit cards)
        let mut temp_buffer = [0u8; super::MAX_ENTRY_SIZE];
        let base_len = self.base_redactor.redact(content, &mut temp_buffer);

        // Apply additional financial-specific patterns
        let financial_redacted = self.apply_financial_patterns(&temp_buffer[..base_len], output);
        financial_redacted
    }

    /// Apply names-only redaction
    fn apply_names_only_redaction(&self, content: &[u8], output: &mut [u8]) -> usize {
        // Only redact names and leave other data intact
        self.apply_name_patterns(content, output)
    }

    /// Apply full redaction (most restrictive)
    fn apply_full_redaction(&self, _content: &[u8], output: &mut [u8]) -> usize {
        // Very aggressive redaction - only keep basic structure
        let summary = b"[CONTENT REDACTED - INSUFFICIENT PRIVILEGE]";
        let copy_len = summary.len().min(output.len());
        output[..copy_len].copy_from_slice(&summary[..copy_len]);
        copy_len
    }

    /// Apply custom redaction profile by ID
    fn apply_custom_redaction(&self, content: &[u8], output: &mut [u8], _profile_id: u8) -> usize {
        // For now, fall back to standard redaction
        // In production, this would load custom patterns from a configuration
        self.base_redactor.redact(content, output)
    }

    /// Apply medical-specific redaction patterns
    fn apply_medical_patterns(&self, content: &[u8], output: &mut [u8]) -> usize {
        let mut pos = 0;
        let mut out_pos = 0;

        while pos < content.len() && out_pos < output.len() {
            // Look for medical-specific patterns
            if let Some((pattern_len, replacement)) = self.find_medical_pattern(&content[pos..]) {
                // Copy replacement
                let copy_len = replacement.len().min(output.len() - out_pos);
                output[out_pos..out_pos + copy_len].copy_from_slice(&replacement[..copy_len]);
                out_pos += copy_len;
                pos += pattern_len;
            } else {
                // Copy single byte
                if out_pos < output.len() {
                    output[out_pos] = content[pos];
                    out_pos += 1;
                }
                pos += 1;
            }
        }

        out_pos
    }

    /// Apply financial-specific redaction patterns
    fn apply_financial_patterns(&self, content: &[u8], output: &mut [u8]) -> usize {
        let mut pos = 0;
        let mut out_pos = 0;

        while pos < content.len() && out_pos < output.len() {
            // Look for financial-specific patterns
            if let Some((pattern_len, replacement)) = self.find_financial_pattern(&content[pos..]) {
                // Copy replacement
                let copy_len = replacement.len().min(output.len() - out_pos);
                output[out_pos..out_pos + copy_len].copy_from_slice(&replacement[..copy_len]);
                out_pos += copy_len;
                pos += pattern_len;
            } else {
                // Copy single byte
                if out_pos < output.len() {
                    output[out_pos] = content[pos];
                    out_pos += 1;
                }
                pos += 1;
            }
        }

        out_pos
    }

    /// Apply name-specific redaction patterns
    fn apply_name_patterns(&self, content: &[u8], output: &mut [u8]) -> usize {
        let mut pos = 0;
        let mut out_pos = 0;

        while pos < content.len() && out_pos < output.len() {
            // Look for name patterns (simplified - just look for capitalized words)
            if let Some((pattern_len, replacement)) = self.find_name_pattern(&content[pos..]) {
                // Copy replacement
                let copy_len = replacement.len().min(output.len() - out_pos);
                output[out_pos..out_pos + copy_len].copy_from_slice(&replacement[..copy_len]);
                out_pos += copy_len;
                pos += pattern_len;
            } else {
                // Copy single byte
                if out_pos < output.len() {
                    output[out_pos] = content[pos];
                    out_pos += 1;
                }
                pos += 1;
            }
        }

        out_pos
    }

    /// Find medical-specific patterns
    fn find_medical_pattern(&self, content: &[u8]) -> Option<(usize, &'static [u8])> {
        // Medical record numbers: MRN followed by digits
        if content.len() >= 7 && &content[..3] == b"MRN" {
            // Look for MRN followed by number
            let mut end = 3;
            while end < content.len() && (content[end].is_ascii_digit() || content[end] == b'-' || content[end] == b' ') {
                end += 1;
                if end - 3 > 15 { break; } // Reasonable limit
            }
            if end > 5 { // At least "MRN" + some digits
                return Some((end, b"[MRN]"));
            }
        }

        // DOB patterns: MM/DD/YYYY or MM-DD-YYYY
        if content.len() >= 10 {
            if self.is_date_pattern(&content[..10]) {
                return Some((10, b"[DOB]"));
            }
        }

        // Patient ID patterns (simplified)
        if content.len() >= 8 && content.starts_with(b"PATIENT") {
            return Some((7, b"[PATIENT]"));
        }

        None
    }

    /// Find financial-specific patterns
    fn find_financial_pattern(&self, content: &[u8]) -> Option<(usize, &'static [u8])> {
        // Account numbers: ACCT followed by digits
        if content.len() >= 8 && &content[..4] == b"ACCT" {
            let mut end = 4;
            while end < content.len() && (content[end].is_ascii_digit() || content[end] == b'-' || content[end] == b' ') {
                end += 1;
                if end - 4 > 20 { break; }
            }
            if end > 6 {
                return Some((end, b"[ACCOUNT]"));
            }
        }

        // Routing numbers: 9 consecutive digits
        if content.len() >= 9 {
            let mut digit_count = 0;
            for &byte in &content[..9] {
                if byte.is_ascii_digit() {
                    digit_count += 1;
                } else {
                    break;
                }
            }
            if digit_count == 9 {
                return Some((9, b"[ROUTING]"));
            }
        }

        None
    }

    /// Find name patterns (simplified)
    fn find_name_pattern(&self, content: &[u8]) -> Option<(usize, &'static [u8])> {
        // Look for capitalized words (simplified name detection)
        if content.is_empty() || !content[0].is_ascii_uppercase() {
            return None;
        }

        let mut end = 1;
        while end < content.len() && content[end].is_ascii_alphabetic() {
            end += 1;
        }

        // Consider it a name if it's 2-20 characters
        if end >= 2 && end <= 20 {
            return Some((end, tokens::NAME));
        }

        None
    }

    /// Check if bytes match a date pattern MM/DD/YYYY or MM-DD-YYYY
    fn is_date_pattern(&self, bytes: &[u8]) -> bool {
        if bytes.len() != 10 {
            return false;
        }

        // Check MM/DD/YYYY pattern
        bytes[0].is_ascii_digit() &&
        bytes[1].is_ascii_digit() &&
        (bytes[2] == b'/' || bytes[2] == b'-') &&
        bytes[3].is_ascii_digit() &&
        bytes[4].is_ascii_digit() &&
        (bytes[5] == b'/' || bytes[5] == b'-') &&
        bytes[6].is_ascii_digit() &&
        bytes[7].is_ascii_digit() &&
        bytes[8].is_ascii_digit() &&
        bytes[9].is_ascii_digit()
    }
}

/// Map a policy-engine result + requester tier to a redaction profile.
/// Pure function (no globals) so the tier→profile contract is unit-testable
/// without standing up the policy engine (2026-07-17 review, high #4).
///
/// Rules:
/// - `AllowWithRedaction(p)` → `p` (policy says exactly what to do).
/// - `Allow(_)`: Secret requester → `None` (passthrough — this was the
///   inversion bug: Secret used to land on the *maximum*-redaction profile
///   because it was misleadingly named `Minimal`). Everyone else → Standard.
/// - `Deny` → `Full` (blank everything).
/// - Anything else (`RequireEscalation`, `NoMatch`, future variants) →
///   `Full`: unknown outcomes fail CLOSED, never to the default profile.
fn map_policy_result(
    result: PolicyResult,
    requester_tier: SecurityTier,
    _default_profile: RedactionProfile,
) -> RedactionProfile {
    match result {
        PolicyResult::AllowWithRedaction(profile) => profile,
        PolicyResult::Allow(_) => match requester_tier {
            SecurityTier::Secret => RedactionProfile::None,
            _ => RedactionProfile::Standard,
        },
        PolicyResult::Deny => RedactionProfile::Full,
        _ => RedactionProfile::Full,
    }
}

/// Global context-aware redactor instance, behind the kernel mutex (same
/// preemption race as the registry — 2026-07-17 review, P1).
static GLOBAL_CONTEXT_REDACTOR: crate::sync::Mutex<ContextAwareRedactor> =
    crate::sync::Mutex::new(ContextAwareRedactor::new());

/// Lock the global context-aware redactor.
pub fn global_context_redactor() -> crate::sync::MutexGuard<'static, ContextAwareRedactor> {
    GLOBAL_CONTEXT_REDACTOR.lock()
}

/// Initialize the context-aware redaction subsystem
pub fn init() {
    GLOBAL_CONTEXT_REDACTOR.lock().init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::user_ids;

    #[test]
    fn test_medical_redaction() {
        let mut redactor = ContextAwareRedactor::new();
        redactor.init();

        let content = b"Patient MRN123456 DOB 01/01/1990 has condition X";
        let mut output = [0u8; 256];

        let len = redactor.apply_medical_redaction(content, &mut output);
        let result = core::str::from_utf8(&output[..len]).unwrap();

        assert!(result.contains("[MRN]"));
        assert!(result.contains("[DOB]"));
    }

    #[test]
    fn test_context_redaction() {
        let mut redactor = ContextAwareRedactor::new();
        redactor.init();
        // The policy engine must be initialized or evaluate() returns Deny
        // (fail-closed) and every request comes back fully blanked.
        crate::security::evaluation::init();

        let context = RedactionContext {
            requester_id: user_ids::GUEST,
            requester_tier: SecurityTier::Public,
            target_suid: SUID::new(0x1234, 0x5678),
            request_type: RequestType::LLMContext,
            context_flags: 0,
            app_context: 0,
        };

        let content = b"Sensitive data: email=user@example.com";
        let mut output = [0u8; 256];

        let len = redactor.redact_with_context(content, &context, &mut output);
        let result = core::str::from_utf8(&output[..len]).unwrap();

        assert!(result.contains("[EMAIL]"));
    }

    // --- Tier→profile contract tests (2026-07-17 review, high #4) ---
    // Pin the mapping so the tier-inversion bug (Secret → maximum redaction)
    // cannot come back.

    #[test]
    fn test_allow_secret_requester_gets_passthrough() {
        let p = map_policy_result(
            PolicyResult::Allow(SecurityTier::Secret),
            SecurityTier::Secret,
            RedactionProfile::Standard,
        );
        assert_eq!(p, RedactionProfile::None);
    }

    #[test]
    fn test_allow_lower_tiers_get_standard() {
        for tier in [SecurityTier::Sensitive, SecurityTier::Internal, SecurityTier::Public] {
            let p = map_policy_result(
                PolicyResult::Allow(tier),
                tier,
                RedactionProfile::Standard,
            );
            assert_eq!(p, RedactionProfile::Standard);
        }
    }

    #[test]
    fn test_deny_gets_full_redaction() {
        for tier in [SecurityTier::Secret, SecurityTier::Sensitive, SecurityTier::Internal, SecurityTier::Public] {
            let p = map_policy_result(PolicyResult::Deny, tier, RedactionProfile::Standard);
            assert_eq!(p, RedactionProfile::Full);
        }
    }

    #[test]
    fn test_unknown_policy_results_fail_closed() {
        // RequireEscalation and NoMatch must NOT fall back to the default
        // profile — unknown outcomes redact maximally.
        for result in [PolicyResult::RequireEscalation, PolicyResult::NoMatch] {
            let p = map_policy_result(result, SecurityTier::Secret, RedactionProfile::Standard);
            assert_eq!(p, RedactionProfile::Full);
        }
    }

    #[test]
    fn test_policy_specified_profile_passes_through() {
        let p = map_policy_result(
            PolicyResult::AllowWithRedaction(RedactionProfile::Medical),
            SecurityTier::Public,
            RedactionProfile::Standard,
        );
        assert_eq!(p, RedactionProfile::Medical);
    }

    #[test]
    fn test_full_profile_blanks_content() {
        let mut redactor = ContextAwareRedactor::new();
        redactor.init();
        let content = b"top secret payload";
        let mut output = [0u8; 256];
        let len = redactor.apply_redaction_profile(content, RedactionProfile::Full, &mut output);
        assert_eq!(&output[..len], b"[CONTENT REDACTED - INSUFFICIENT PRIVILEGE]");
    }

    #[test]
    fn test_none_profile_passes_content_verbatim() {
        let mut redactor = ContextAwareRedactor::new();
        redactor.init();
        let content = b"top secret payload";
        let mut output = [0u8; 256];
        let len = redactor.apply_redaction_profile(content, RedactionProfile::None, &mut output);
        assert_eq!(&output[..len], content);
    }
}