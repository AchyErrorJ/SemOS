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
        unsafe {
            let policy_engine = global_policy_engine();
            let policy_result = policy_engine.evaluate(&eval_context);

            match policy_result {
                PolicyResult::AllowWithRedaction(profile) => profile,
                PolicyResult::Allow(_) => {
                    // No specific redaction required, but apply default for sensitive data
                    match context.requester_tier {
                        SecurityTier::Secret => RedactionProfile::Minimal,
                        SecurityTier::Sensitive => RedactionProfile::Standard,
                        SecurityTier::Internal => RedactionProfile::Standard,
                        SecurityTier::Public => RedactionProfile::Standard,
                    }
                }
                PolicyResult::Deny => {
                    // Access denied - redact everything
                    RedactionProfile::Minimal
                }
                _ => self.default_profile,
            }
        }
    }

    /// Apply specific redaction profile to content
    fn apply_redaction_profile(
        &self,
        content: &[u8],
        profile: RedactionProfile,
        output: &mut [u8],
    ) -> usize {
        match profile {
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
            RedactionProfile::Minimal => {
                self.apply_minimal_redaction(content, output)
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

    /// Apply minimal redaction (most restrictive)
    fn apply_minimal_redaction(&self, content: &[u8], output: &mut [u8]) -> usize {
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

/// Global context-aware redactor instance
static mut GLOBAL_CONTEXT_REDACTOR: ContextAwareRedactor = ContextAwareRedactor::new();

/// Get the global context-aware redactor
pub unsafe fn global_context_redactor() -> &'static mut ContextAwareRedactor {
    &mut *core::ptr::addr_of_mut!(GLOBAL_CONTEXT_REDACTOR)
}

/// Initialize the context-aware redaction subsystem
pub fn init() {
    unsafe {
        GLOBAL_CONTEXT_REDACTOR.init();
    }
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
}