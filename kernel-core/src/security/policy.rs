//! Policy Object Definitions
//!
//! Defines the structure and serialization of security policies stored
//! as semantic objects in the system.

use crate::semantic::SUID;
use crate::memory::SecurityTier;
use super::{UserId, SecurityError};

/// Maximum number of rules per policy
pub const MAX_RULES_PER_POLICY: usize = 16;

/// Maximum size of policy serialized data
pub const MAX_POLICY_SIZE: usize = 2048;

/// Policy object - stored as a semantic object
#[derive(Clone, Copy)]
pub struct PolicyObject {
    /// Policy type - determines evaluation context
    pub policy_type: PolicyType,
    /// What this policy applies to
    pub target: PolicyTarget,
    /// Policy rules (evaluated in order)
    pub rules: [PolicyRule; MAX_RULES_PER_POLICY],
    /// Number of active rules
    pub rule_count: usize,
    /// Policy priority (higher = evaluated first)
    pub priority: u32,
    /// Owner of this policy (can modify it)
    pub owner: UserId,
    /// When this policy expires (0 = never)
    pub expires_at: u64,
    /// Policy metadata flags
    pub flags: PolicyFlags,
}

/// Type of policy - determines when and how it's evaluated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PolicyType {
    /// Controls access to specific semantic objects
    ObjectAccess = 0,
    /// Per-user data isolation rules
    UserIsolation = 1,
    /// Rules for tier escalation requests
    TierEscalation = 2,
    /// Time-based access control
    TimeBasedAccess = 3,
    /// Context-dependent rules (app-specific)
    ContextDependent = 4,
}

/// What the policy applies to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyTarget {
    /// Apply to all objects/requests
    AllObjects,
    /// Apply to specific objects by SUID
    ObjectsBySUID([SUID; 4], usize), // SUUIDs + count
    /// Apply to all objects owned by a user
    ObjectsByOwner(UserId),
    /// Apply to all objects at a security tier
    ObjectsByTier(SecurityTier),
    /// Apply to a specific user's requests
    User(UserId),
    /// Apply to everyone
    Everyone,
    /// Apply to objects matching a pattern (future)
    ObjectsByPattern(u32), // Pattern ID
}

/// Individual policy rule
#[derive(Clone, Copy)]
pub struct PolicyRule {
    /// Conditions that must be met for this rule to apply
    pub conditions: [RuleCondition; 4],
    /// Number of active conditions
    pub condition_count: usize,
    /// Action to take if conditions match
    pub action: PolicyAction,
    /// Priority within this policy (higher = evaluated first)
    pub rule_priority: u16,
    /// Rule flags
    pub flags: RuleFlags,
}

/// Condition that must be met for a rule to apply
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleCondition {
    /// Always matches (default/fallback rule)
    Always,
    /// Match if requester is a specific user
    RequesterIs(UserId),
    /// Match if requester is in a user group
    RequesterInGroup(u16), // Group ID
    /// Match if target object has specific tier
    ObjectTierIs(SecurityTier),
    /// Match if target object is owned by user
    ObjectOwnedBy(UserId),
    /// Match if request happens during time window
    TimeWindow(u32, u32), // Start hour, end hour (24h format)
    /// Match if request context contains flag
    ContextHasFlag(u32), // Context flag mask
    /// Match if requester's current tier is X
    RequesterTierIs(SecurityTier),
}

/// Action to take when a rule matches
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// Allow access at specified tier
    Allow(SecurityTier),
    /// Deny access completely
    Deny,
    /// Allow with custom redaction rules
    AllowWithRedaction(RedactionProfile),
    /// Require escalation/approval
    RequireEscalation,
    /// Log and allow (audit mode)
    LogAndAllow(SecurityTier),
    /// Defer to next policy (continue evaluation)
    Continue,
}

/// Predefined redaction profiles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RedactionProfile {
    /// Standard PII redaction (emails, SSNs, etc.)
    Standard = 0,
    /// Medical privacy (HIPAA-style)
    Medical = 1,
    /// Financial privacy (PCI-style)
    Financial = 2,
    /// Names and identifiers only
    NamesOnly = 3,
    /// Everything blanked — the MAXIMUM-redaction profile. Renamed from
    /// `Minimal` (2026-07-17 review): the old name read as "lightest touch"
    /// but implemented "replace everything", which hid a tier-inversion bug
    /// in the context redactor.
    Full = 4,
    /// Passthrough — no redaction at all. Only for requesters whose
    /// clearance covers the content outright (Secret).
    None = 5,
    /// Custom redaction rule (by ID)
    Custom(u8) = 255,
}

/// Policy-level flags
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PolicyFlags(pub u16);

impl PolicyFlags {
    pub const ENABLED: u16 = 1 << 0;     // Policy is active
    pub const AUDITED: u16 = 1 << 1;     // Log all evaluations
    pub const IMMUTABLE: u16 = 1 << 2;   // Cannot be modified
    pub const SYSTEM: u16 = 1 << 3;      // System-created policy

    pub fn new() -> Self {
        Self(Self::ENABLED)
    }

    pub fn is_enabled(&self) -> bool {
        self.0 & Self::ENABLED != 0
    }

    pub fn is_audited(&self) -> bool {
        self.0 & Self::AUDITED != 0
    }

    pub fn is_immutable(&self) -> bool {
        self.0 & Self::IMMUTABLE != 0
    }

    pub fn is_system(&self) -> bool {
        self.0 & Self::SYSTEM != 0
    }
}

/// Rule-level flags
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RuleFlags(pub u8);

impl RuleFlags {
    pub const ENABLED: u8 = 1 << 0;      // Rule is active
    pub const TERMINAL: u8 = 1 << 1;     // Stop evaluation if this rule matches
    pub const AUDIT: u8 = 1 << 2;        // Log when this rule matches

    pub fn new() -> Self {
        Self(Self::ENABLED)
    }

    pub fn is_enabled(&self) -> bool {
        self.0 & Self::ENABLED != 0
    }

    pub fn is_terminal(&self) -> bool {
        self.0 & Self::TERMINAL != 0
    }

    pub fn should_audit(&self) -> bool {
        self.0 & Self::AUDIT != 0
    }
}

impl PolicyObject {
    /// Create an empty policy
    pub const fn empty() -> Self {
        Self {
            policy_type: PolicyType::ObjectAccess,
            target: PolicyTarget::Everyone,
            rules: [PolicyRule::empty(); MAX_RULES_PER_POLICY],
            rule_count: 0,
            priority: 0,
            owner: super::user_ids::NOBODY,
            expires_at: 0,
            flags: PolicyFlags(0),
        }
    }

    /// Create a new policy
    pub fn new(
        policy_type: PolicyType,
        target: PolicyTarget,
        owner: UserId,
        priority: u32,
    ) -> Self {
        let mut policy = Self::empty();
        policy.policy_type = policy_type;
        policy.target = target;
        policy.owner = owner;
        policy.priority = priority;
        policy.flags = PolicyFlags::new();
        policy
    }

    /// Add a rule to this policy
    pub fn add_rule(&mut self, rule: PolicyRule) -> Result<(), SecurityError> {
        if self.rule_count >= MAX_RULES_PER_POLICY {
            return Err(SecurityError::InvalidPolicy);
        }
        self.rules[self.rule_count] = rule;
        self.rule_count += 1;
        Ok(())
    }

    /// Get active rules
    pub fn rules(&self) -> &[PolicyRule] {
        &self.rules[..self.rule_count]
    }

    /// Check if policy is active
    pub fn is_active(&self) -> bool {
        self.flags.is_enabled() &&
        (self.expires_at == 0 || self.expires_at > crate::platform::ticks())
    }

    /// Check if user can modify this policy
    pub fn can_modify(&self, user: UserId) -> bool {
        if self.flags.is_immutable() {
            false
        } else {
            user == self.owner || user == super::user_ids::ADMIN
        }
    }

    /// Serialize policy to bytes
    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, SecurityError> {
        if out.len() < 64 { // Minimum needed for demo
            return Err(SecurityError::InvalidPolicy);
        }

        // Simple binary serialization for demo
        let mut offset = 0;

        // Policy header
        out[offset] = self.policy_type as u8;
        offset += 1;
        out[offset] = self.rule_count as u8;
        offset += 1;

        // For demo, just serialize basic info
        out[offset] = self.priority as u8;
        offset += 1;
        out[offset] = self.owner;
        offset += 1;

        Ok(offset)
    }

    /// Deserialize policy from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self, SecurityError> {
        if data.is_empty() {
            return Err(SecurityError::InvalidPolicy);
        }

        // Simple binary deserialization
        // This would be a proper format in production
        let mut policy = Self::empty();
        if let Some(&policy_type_byte) = data.get(0) {
            policy.policy_type = match policy_type_byte {
                0 => PolicyType::ObjectAccess,
                1 => PolicyType::UserIsolation,
                2 => PolicyType::TierEscalation,
                3 => PolicyType::TimeBasedAccess,
                4 => PolicyType::ContextDependent,
                _ => return Err(SecurityError::InvalidPolicy),
            };
        }

        Ok(policy)
    }
}

impl PolicyRule {
    /// Create an empty rule
    pub const fn empty() -> Self {
        Self {
            conditions: [RuleCondition::Always; 4],
            condition_count: 0,
            action: PolicyAction::Deny,
            rule_priority: 0,
            flags: RuleFlags(0),
        }
    }

    /// Create a simple rule with one condition
    pub fn simple(condition: RuleCondition, action: PolicyAction) -> Self {
        let mut rule = Self::empty();
        rule.conditions[0] = condition;
        rule.condition_count = 1;
        rule.action = action;
        rule.flags = RuleFlags::new();
        rule
    }

    /// Add a condition to this rule
    pub fn add_condition(&mut self, condition: RuleCondition) -> Result<(), SecurityError> {
        if self.condition_count >= 4 {
            return Err(SecurityError::InvalidPolicy);
        }
        self.conditions[self.condition_count] = condition;
        self.condition_count += 1;
        Ok(())
    }

    /// Get active conditions
    pub fn conditions(&self) -> &[RuleCondition] {
        &self.conditions[..self.condition_count]
    }
}

/// Initialize policy subsystem
pub fn init() {
    // Policy subsystem initialized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_creation() {
        let mut policy = PolicyObject::new(
            PolicyType::ObjectAccess,
            PolicyTarget::Everyone,
            // Tests live in `security::policy::tests`, so `super::super`
            // is the `security` module where `user_ids` is defined.
            super::super::user_ids::ADMIN,
            100,
        );

        let rule = PolicyRule::simple(
            RuleCondition::Always,
            PolicyAction::Allow(SecurityTier::Public),
        );

        assert!(policy.add_rule(rule).is_ok());
        assert_eq!(policy.rule_count, 1);
        assert!(policy.is_active());
    }
}