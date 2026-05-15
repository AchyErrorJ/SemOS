//! Policy Evaluation Engine
//!
//! Core logic for evaluating security policies against requests.
//! Finds matching policies, evaluates rules, and returns access decisions.

use crate::semantic::{SUID, registry::global_registry};
use crate::memory::SecurityTier;
use super::{
    UserId, SecurityError,
    policy::{PolicyObject, PolicyType, PolicyTarget, PolicyAction, RuleCondition},
    policy_suids,
};

/// Maximum policies to evaluate per request
pub const MAX_POLICIES_PER_REQUEST: usize = 32;

/// Policy evaluation context
pub struct EvaluationContext {
    /// Who is making the request
    pub requester_id: UserId,
    /// Requester's current maximum tier
    pub requester_tier: SecurityTier,
    /// Target object SUID
    pub target_suid: SUID,
    /// Target object tier (if known)
    pub target_tier: Option<SecurityTier>,
    /// Target object owner (if known)
    pub target_owner: Option<UserId>,
    /// Request context information
    pub context: RequestContext,
    /// Current time (ticks since boot)
    pub current_time: u64,
}

/// Request context information
#[derive(Clone, Copy)]
pub struct RequestContext {
    /// Request type
    pub request_type: RequestType,
    /// Context flags
    pub flags: u32,
    /// Application context ID
    pub app_context: u32,
}

/// Type of request being made
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RequestType {
    /// Direct semantic object access
    DirectAccess = 0,
    /// LLM context building
    LLMContext = 1,
    /// LLM streaming request
    LLMStream = 2,
    /// Object creation
    ObjectCreate = 3,
    /// Object modification
    ObjectModify = 4,
    /// Administrative operation
    Administrative = 5,
}

/// Result of policy evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyResult {
    /// Access allowed at specified tier
    Allow(SecurityTier),
    /// Access denied
    Deny,
    /// Allow with specific redaction profile
    AllowWithRedaction(super::policy::RedactionProfile),
    /// Requires escalation/approval
    RequireEscalation,
    /// No matching policy found (use default)
    NoMatch,
}

/// Policy evaluation engine
pub struct PolicyEngine {
    /// Is engine initialized?
    initialized: bool,
    /// Default policy for no-match cases
    default_action: PolicyAction,
}

impl PolicyEngine {
    /// Create a new policy engine
    pub const fn new() -> Self {
        Self {
            initialized: false,
            default_action: PolicyAction::Allow(SecurityTier::Public),
        }
    }

    /// Initialize the policy engine
    pub fn init(&mut self) {
        self.initialized = true;
        // Set conservative default - allow public access only
        self.default_action = PolicyAction::Allow(SecurityTier::Public);
    }

    /// Evaluate policies for a request
    pub fn evaluate(&self, context: &EvaluationContext) -> PolicyResult {
        if !self.initialized {
            return PolicyResult::Deny;
        }

        // Find all policies that might apply to this request
        let mut matching_policies = [None; MAX_POLICIES_PER_REQUEST];
        let mut policy_count = 0;

        // Search for policies in the semantic registry
        unsafe {
            let registry = global_registry();

            // Scan policy SUID range for matching policies
            // In production, this would use an index for efficiency
            for policy_id in 0..1000u32 { // Arbitrary scan limit for demo
                let system_suid = policy_suids::new_system_policy(policy_id);
                if let Some(policy_obj) = registry.get(&system_suid) {
                    if let Some(policy_content) = policy_obj.content.as_bytes() {
                        if let Ok(policy) = PolicyObject::deserialize(policy_content) {
                            if self.policy_matches(&policy, context) {
                                if policy_count < MAX_POLICIES_PER_REQUEST {
                                    matching_policies[policy_count] = Some(policy);
                                    policy_count += 1;
                                }
                            }
                        }
                    }
                }

                // Also check user policies for this requester
                if context.requester_id != super::user_ids::SYSTEM {
                    let user_suid = policy_suids::new_user_policy(context.requester_id, policy_id);
                    if let Some(policy_obj) = registry.get(&user_suid) {
                        if let Some(policy_content) = policy_obj.content.as_bytes() {
                            if let Ok(policy) = PolicyObject::deserialize(policy_content) {
                                if self.policy_matches(&policy, context) {
                                    if policy_count < MAX_POLICIES_PER_REQUEST {
                                        matching_policies[policy_count] = Some(policy);
                                        policy_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Sort policies by priority (higher first)
        self.sort_policies_by_priority(&mut matching_policies[..policy_count]);

        // Evaluate policies in priority order
        for policy_opt in &matching_policies[..policy_count] {
            if let Some(policy) = policy_opt {
                if let Some(result) = self.evaluate_policy(policy, context) {
                    return result;
                }
            }
        }

        // No matching policies - apply default
        self.apply_default_action(&self.default_action)
    }

    /// Check if a policy applies to the request context
    fn policy_matches(&self, policy: &PolicyObject, context: &EvaluationContext) -> bool {
        if !policy.is_active() {
            return false;
        }

        // Check policy type matches request type
        match (policy.policy_type, context.context.request_type) {
            (PolicyType::ObjectAccess, RequestType::DirectAccess) => true,
            (PolicyType::ObjectAccess, RequestType::LLMContext) => true,
            (PolicyType::ObjectAccess, RequestType::LLMStream) => true,
            (PolicyType::UserIsolation, _) => true, // User isolation applies to all requests
            (PolicyType::TierEscalation, _) => true, // Tier escalation applies to all requests
            (PolicyType::TimeBasedAccess, _) => true, // Time-based applies to all requests
            (PolicyType::ContextDependent, _) => true, // Context-dependent applies to all
            _ => false,
        }
    }

    /// Check if policy target matches the request
    fn target_matches(&self, target: &PolicyTarget, context: &EvaluationContext) -> bool {
        match target {
            PolicyTarget::AllObjects => true,
            PolicyTarget::Everyone => true,
            PolicyTarget::User(user_id) => *user_id == context.requester_id,
            PolicyTarget::ObjectsByOwner(owner_id) => {
                context.target_owner.map_or(false, |o| o == *owner_id)
            }
            PolicyTarget::ObjectsByTier(tier) => {
                context.target_tier.map_or(false, |t| t == *tier)
            }
            PolicyTarget::ObjectsBySUID(suids, count) => {
                for i in 0..*count {
                    if suids[i] == context.target_suid {
                        return true;
                    }
                }
                false
            }
            PolicyTarget::ObjectsByPattern(_pattern_id) => {
                // Pattern matching not implemented yet
                false
            }
        }
    }

    /// Evaluate a single policy against the context
    fn evaluate_policy(&self, policy: &PolicyObject, context: &EvaluationContext) -> Option<PolicyResult> {
        if !self.target_matches(&policy.target, context) {
            return None;
        }

        // Evaluate rules in priority order
        for rule in policy.rules() {
            if !rule.flags.is_enabled() {
                continue;
            }

            if self.rule_matches(rule, context) {
                let result = self.apply_action(&rule.action);

                if rule.flags.should_audit() {
                    // Log policy evaluation (in production)
                    crate::platform::log("[security] Policy rule matched\n");
                }

                if rule.flags.is_terminal() {
                    return Some(result);
                }

                // Non-terminal rule - continue if action is Continue
                match rule.action {
                    PolicyAction::Continue => continue,
                    _ => return Some(result),
                }
            }
        }

        None // No matching rules in this policy
    }

    /// Check if a rule's conditions match the context
    fn rule_matches(&self, rule: &super::policy::PolicyRule, context: &EvaluationContext) -> bool {
        if rule.condition_count == 0 {
            return false;
        }

        // All conditions must match (AND logic)
        for condition in rule.conditions() {
            if !self.condition_matches(condition, context) {
                return false;
            }
        }

        true
    }

    /// Check if a single condition matches
    fn condition_matches(&self, condition: &RuleCondition, context: &EvaluationContext) -> bool {
        match condition {
            RuleCondition::Always => true,
            RuleCondition::RequesterIs(user_id) => *user_id == context.requester_id,
            RuleCondition::RequesterTierIs(tier) => *tier == context.requester_tier,
            RuleCondition::ObjectTierIs(tier) => {
                context.target_tier.map_or(false, |t| t == *tier)
            }
            RuleCondition::ObjectOwnedBy(owner) => {
                context.target_owner.map_or(false, |o| o == *owner)
            }
            RuleCondition::TimeWindow(start_hour, end_hour) => {
                // Simple time-based matching (would be more sophisticated in production)
                let current_hour = (context.current_time / 3600) % 24; // Assuming ticks are seconds
                current_hour >= *start_hour as u64 && current_hour < *end_hour as u64
            }
            RuleCondition::ContextHasFlag(flag_mask) => {
                (context.context.flags & flag_mask) != 0
            }
            RuleCondition::RequesterInGroup(_group_id) => {
                // Group membership not implemented yet
                false
            }
        }
    }

    /// Convert PolicyAction to PolicyResult
    fn apply_action(&self, action: &PolicyAction) -> PolicyResult {
        match action {
            PolicyAction::Allow(tier) => PolicyResult::Allow(*tier),
            PolicyAction::Deny => PolicyResult::Deny,
            PolicyAction::AllowWithRedaction(profile) => PolicyResult::AllowWithRedaction(*profile),
            PolicyAction::RequireEscalation => PolicyResult::RequireEscalation,
            PolicyAction::LogAndAllow(tier) => {
                // Log the access (in production)
                PolicyResult::Allow(*tier)
            }
            PolicyAction::Continue => PolicyResult::NoMatch, // Should not reach here
        }
    }

    /// Apply default action when no policies match
    fn apply_default_action(&self, action: &PolicyAction) -> PolicyResult {
        self.apply_action(action)
    }

    /// Sort policies by priority (higher first)
    fn sort_policies_by_priority(&self, policies: &mut [Option<PolicyObject>]) {
        // Simple bubble sort for now (would use a proper sort in production)
        for i in 0..policies.len() {
            for j in (i + 1)..policies.len() {
                if let (Some(a), Some(b)) = (&policies[i], &policies[j]) {
                    if b.priority > a.priority {
                        policies.swap(i, j);
                    }
                }
            }
        }
    }

    /// Set default action for no-match cases
    pub fn set_default_action(&mut self, action: PolicyAction) {
        self.default_action = action;
    }
}

/// Global policy engine instance
static mut GLOBAL_POLICY_ENGINE: PolicyEngine = PolicyEngine::new();

/// Get the global policy engine
pub unsafe fn global_policy_engine() -> &'static mut PolicyEngine {
    &mut *core::ptr::addr_of_mut!(GLOBAL_POLICY_ENGINE)
}

/// Initialize the evaluation subsystem
pub fn init() {
    unsafe {
        GLOBAL_POLICY_ENGINE.init();
    }
}

/// Helper function to create evaluation context
pub fn create_evaluation_context(
    requester_id: UserId,
    requester_tier: SecurityTier,
    target_suid: SUID,
    request_type: RequestType,
) -> EvaluationContext {
    // Look up target object info if available
    let (target_tier, target_owner) = unsafe {
        let registry = global_registry();
        if let Some(obj) = registry.get(&target_suid) {
            (Some(obj.tier), Some(obj.owner))
        } else {
            (None, None)
        }
    };

    EvaluationContext {
        requester_id,
        requester_tier,
        target_suid,
        target_tier,
        target_owner,
        context: RequestContext {
            request_type,
            flags: 0,
            app_context: 0,
        },
        current_time: crate::platform::ticks(),
    }
}