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
    /// Serialize the full policy into a fixed-layout little-endian blob.
    ///
    /// Format (v2 — the v1 "demo" format was a 4-byte stub whose
    /// deserialize dropped rules/flags/owner/target, silently breaking
    /// every install round-trip; found by the privilege-separation
    /// acceptance harness, DEMO 12):
    ///
    /// ```text
    /// header (84 B):
    ///   [0]      policy_type u8      [1]  rule_count u8
    ///   [2..6]   priority u32 LE     [6]  owner u8
    ///   [7..9]   flags u16 LE        [9..17] expires_at u64 LE
    ///   [17]     target tag u8       [18] target suid_count u8 (tag 1 only)
    ///   [19..]   target payload (u8/u32 inline, or count×16B SUIDs)
    /// per rule (60 B):
    ///   [0] condition_count u8  [1..3] rule_priority u16 LE
    ///   [3] action tag u8       [4..8] action payload (u8/u32)
    ///   [8] rule flags u8       [9] reserved
    ///   [10..58] 4 × condition (tag u8 + 8B payload, 12B each... 12×4=48)
    /// ```
    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, SecurityError> {
        const HEADER: usize = 84;
        const RULE: usize = 60;
        let need = HEADER + RULE * self.rule_count;
        if out.len() < need {
            return Err(SecurityError::InvalidPolicy);
        }
        out[..need].fill(0);

        out[0] = self.policy_type as u8;
        out[1] = self.rule_count as u8;
        out[2..6].copy_from_slice(&self.priority.to_le_bytes());
        out[6] = self.owner;
        out[7..9].copy_from_slice(&self.flags.0.to_le_bytes());
        out[9..17].copy_from_slice(&self.expires_at.to_le_bytes());

        // Target.
        match &self.target {
            PolicyTarget::AllObjects => out[17] = 0,
            PolicyTarget::ObjectsBySUID(suids, count) => {
                out[17] = 1;
                let n = (*count).min(4);
                out[18] = n as u8;
                for i in 0..n {
                    let s = suids[i];
                    out[19 + i * 16..19 + i * 16 + 8].copy_from_slice(&s.high.to_le_bytes());
                    out[19 + i * 16 + 8..19 + i * 16 + 16].copy_from_slice(&s.low.to_le_bytes());
                }
            }
            PolicyTarget::ObjectsByOwner(u) => {
                out[17] = 2;
                out[18] = *u;
            }
            PolicyTarget::ObjectsByTier(t) => {
                out[17] = 3;
                out[18] = *t as u8;
            }
            PolicyTarget::User(u) => {
                out[17] = 4;
                out[18] = *u;
            }
            PolicyTarget::Everyone => out[17] = 5,
            PolicyTarget::ObjectsByPattern(id) => {
                out[17] = 6;
                out[18..22].copy_from_slice(&id.to_le_bytes());
            }
        }

        // Rules.
        for (ri, rule) in self.rules().iter().enumerate() {
            let base = HEADER + RULE * ri;
            out[base] = rule.condition_count as u8;
            out[base + 1..base + 3].copy_from_slice(&rule.rule_priority.to_le_bytes());
            let (atag, apayload) = match rule.action {
                PolicyAction::Allow(t) => (0u8, t as u32),
                PolicyAction::Deny => (1, 0),
                PolicyAction::AllowWithRedaction(p) => (2, redaction_to_u32(p)),
                PolicyAction::RequireEscalation => (3, 0),
                PolicyAction::LogAndAllow(t) => (4, t as u32),
                PolicyAction::Continue => (5, 0),
            };
            out[base + 3] = atag;
            out[base + 4..base + 8].copy_from_slice(&apayload.to_le_bytes());
            out[base + 8] = rule.flags.0;
            let cc = rule.condition_count.min(4);
            for ci in 0..cc {
                let cb = base + 10 + ci * 12;
                let (ctag, cpayload) = match rule.conditions[ci] {
                    RuleCondition::Always => (0u8, [0u8; 8]),
                    RuleCondition::RequesterIs(u) => (1, payload_u64(u as u64)),
                    RuleCondition::RequesterInGroup(g) => (2, payload_u64(g as u64)),
                    RuleCondition::ObjectTierIs(t) => (3, payload_u64(t as u64)),
                    RuleCondition::ObjectOwnedBy(u) => (4, payload_u64(u as u64)),
                    RuleCondition::TimeWindow(a, b) => (5, payload_u64_2(a, b)),
                    RuleCondition::ContextHasFlag(f) => (6, payload_u64(f as u64)),
                    RuleCondition::RequesterTierIs(t) => (7, payload_u64(t as u64)),
                };
                out[cb] = ctag;
                out[cb + 1..cb + 9].copy_from_slice(&cpayload);
            }
        }
        Ok(need)
    }

    /// Inverse of `serialize` (v2 fixed layout). Rejects truncated blobs,
    /// unknown tags, and rule counts that would overflow the blob.
    pub fn deserialize(data: &[u8]) -> Result<Self, SecurityError> {
        const HEADER: usize = 84;
        const RULE: usize = 60;
        if data.len() < HEADER {
            return Err(SecurityError::InvalidPolicy);
        }
        let rule_count = data[1] as usize;
        if rule_count > MAX_RULES_PER_POLICY || data.len() < HEADER + RULE * rule_count {
            return Err(SecurityError::InvalidPolicy);
        }

        let mut policy = Self::empty();
        policy.policy_type = match data[0] {
            0 => PolicyType::ObjectAccess,
            1 => PolicyType::UserIsolation,
            2 => PolicyType::TierEscalation,
            3 => PolicyType::TimeBasedAccess,
            4 => PolicyType::ContextDependent,
            _ => return Err(SecurityError::InvalidPolicy),
        };
        policy.rule_count = rule_count;
        policy.priority = u32::from_le_bytes(data[2..6].try_into().map_err(|_| SecurityError::InvalidPolicy)?);
        policy.owner = data[6];
        policy.flags = PolicyFlags(u16::from_le_bytes(data[7..9].try_into().map_err(|_| SecurityError::InvalidPolicy)?));
        policy.expires_at = u64::from_le_bytes(data[9..17].try_into().map_err(|_| SecurityError::InvalidPolicy)?);

        policy.target = match data[17] {
            0 => PolicyTarget::AllObjects,
            1 => {
                let n = (data[18] as usize).min(4);
                let mut suids = [crate::semantic::SUID::new(0, 0); 4];
                for i in 0..n {
                    let b = 19 + i * 16;
                    let high = u64::from_le_bytes(data[b..b + 8].try_into().map_err(|_| SecurityError::InvalidPolicy)?);
                    let low = u64::from_le_bytes(data[b + 8..b + 16].try_into().map_err(|_| SecurityError::InvalidPolicy)?);
                    suids[i] = crate::semantic::SUID::new(high, low);
                }
                PolicyTarget::ObjectsBySUID(suids, n)
            }
            2 => PolicyTarget::ObjectsByOwner(data[18]),
            3 => PolicyTarget::ObjectsByTier(tier_from_u8(data[18])?),
            4 => PolicyTarget::User(data[18]),
            5 => PolicyTarget::Everyone,
            6 => PolicyTarget::ObjectsByPattern(u32::from_le_bytes(data[18..22].try_into().map_err(|_| SecurityError::InvalidPolicy)?)),
            _ => return Err(SecurityError::InvalidPolicy),
        };

        for ri in 0..rule_count {
            let base = HEADER + RULE * ri;
            let mut rule = PolicyRule::empty();
            let cc = (data[base] as usize).min(4);
            rule.condition_count = cc;
            rule.rule_priority = u16::from_le_bytes(data[base + 1..base + 3].try_into().map_err(|_| SecurityError::InvalidPolicy)?);
            let apayload = u32::from_le_bytes(data[base + 4..base + 8].try_into().map_err(|_| SecurityError::InvalidPolicy)?);
            rule.action = match data[base + 3] {
                0 => PolicyAction::Allow(tier_from_u8(apayload as u8)?),
                1 => PolicyAction::Deny,
                2 => PolicyAction::AllowWithRedaction(redaction_from_u32(apayload)?),
                3 => PolicyAction::RequireEscalation,
                4 => PolicyAction::LogAndAllow(tier_from_u8(apayload as u8)?),
                5 => PolicyAction::Continue,
                _ => return Err(SecurityError::InvalidPolicy),
            };
            rule.flags = RuleFlags(data[base + 8]);
            for ci in 0..cc {
                let cb = base + 10 + ci * 12;
                let p8: [u8; 8] = data[cb + 1..cb + 9].try_into().map_err(|_| SecurityError::InvalidPolicy)?;
                rule.conditions[ci] = match data[cb] {
                    0 => RuleCondition::Always,
                    1 => RuleCondition::RequesterIs(u64::from_le_bytes(p8) as u8),
                    2 => RuleCondition::RequesterInGroup(u64::from_le_bytes(p8) as u16),
                    3 => RuleCondition::ObjectTierIs(tier_from_u8(u64::from_le_bytes(p8) as u8)?),
                    4 => RuleCondition::ObjectOwnedBy(u64::from_le_bytes(p8) as u8),
                    5 => {
                        let a = u32::from_le_bytes(p8[0..4].try_into().map_err(|_| SecurityError::InvalidPolicy)?);
                        let b = u32::from_le_bytes(p8[4..8].try_into().map_err(|_| SecurityError::InvalidPolicy)?);
                        RuleCondition::TimeWindow(a, b)
                    }
                    6 => RuleCondition::ContextHasFlag(u64::from_le_bytes(p8) as u32),
                    7 => RuleCondition::RequesterTierIs(tier_from_u8(u64::from_le_bytes(p8) as u8)?),
                    _ => return Err(SecurityError::InvalidPolicy),
                };
            }
            policy.rules[ri] = rule;
        }
        Ok(policy)
    }
}

fn payload_u64(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

fn payload_u64_2(a: u32, b: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&a.to_le_bytes());
    out[4..8].copy_from_slice(&b.to_le_bytes());
    out
}

fn tier_from_u8(v: u8) -> Result<crate::memory::SecurityTier, SecurityError> {
    match v {
        0 => Ok(crate::memory::SecurityTier::Public),
        1 => Ok(crate::memory::SecurityTier::Internal),
        2 => Ok(crate::memory::SecurityTier::Sensitive),
        3 => Ok(crate::memory::SecurityTier::Secret),
        _ => Err(SecurityError::InvalidPolicy),
    }
}

fn redaction_to_u32(p: RedactionProfile) -> u32 {
    match p {
        RedactionProfile::Standard => 0,
        RedactionProfile::Medical => 1,
        RedactionProfile::Financial => 2,
        RedactionProfile::NamesOnly => 3,
        RedactionProfile::Full => 4,
        RedactionProfile::None => 5,
        RedactionProfile::Custom(id) => 0xFFFF_FF00 | id as u32,
    }
}

fn redaction_from_u32(v: u32) -> Result<RedactionProfile, SecurityError> {
    if v & 0xFFFF_FF00 == 0xFFFF_FF00 {
        return Ok(RedactionProfile::Custom((v & 0xFF) as u8));
    }
    redaction_from_u8(v as u8)
}

fn redaction_from_u8(v: u8) -> Result<RedactionProfile, SecurityError> {
    match v {
        0 => Ok(RedactionProfile::Standard),
        1 => Ok(RedactionProfile::Medical),
        2 => Ok(RedactionProfile::Financial),
        3 => Ok(RedactionProfile::NamesOnly),
        4 => Ok(RedactionProfile::Full),
        5 => Ok(RedactionProfile::None),
        255 => Ok(RedactionProfile::Custom(255)),
        _ => Err(SecurityError::InvalidPolicy),
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
#[cfg(test)]
mod roundtrip_debug {
    use super::*;

    #[test]
    fn serialize_roundtrip_keeps_rules() {
        let mut pol = PolicyObject::new(PolicyType::ObjectAccess, PolicyTarget::Everyone, 254, 200);
        let rule = PolicyRule::simple(RuleCondition::Always, PolicyAction::Allow(crate::memory::SecurityTier::Public));
        pol.add_rule(rule).unwrap();
        let mut buf = [0u8; 256];
        let n = pol.serialize(&mut buf).unwrap();
        let back = PolicyObject::deserialize(&buf[..n]).unwrap();
        assert!(back.is_active(), "deserialized policy not active");
        assert_eq!(back.rule_count, 1);
    }
}
