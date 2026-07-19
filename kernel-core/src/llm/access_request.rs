//! Access Request & Escalation
//!
//! Handles requests for elevated tier access.
//! When an LLM or process needs access to a higher tier than
//! currently allowed, it can submit an escalation request.
//!
//! # Escalation Flow
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │ LLM needs access to Tier 2 object                       │
//! │ Current access: Tier 1                                  │
//! └─────────────────────┬───────────────────────────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │ Submit AccessRequest                                    │
//! │ - Requester ID                                          │
//! │ - Target SUID                                           │
//! │ - Requested tier                                        │
//! │ - Justification                                         │
//! └─────────────────────┬───────────────────────────────────┘
//!                       │
//!                       ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │ EscalationQueue                                         │
//! │ - Stores pending requests                               │
//! │ - Notifies approvers                                    │
//! │ - Tracks decisions                                      │
//! └─────────────────────┬───────────────────────────────────┘
//!                       │
//!         ┌─────────────┼─────────────┐
//!         ▼             ▼             ▼
//!    ┌─────────┐  ┌─────────┐  ┌─────────┐
//!    │ Approve │  │  Deny   │  │ Timeout │
//!    └────┬────┘  └────┬────┘  └────┬────┘
//!         │            │            │
//!         ▼            ▼            ▼
//!    Grant temp    Return     Return
//!    access        denied     timeout
//! ```

use crate::semantic::SUID;

/// Maximum pending access requests
pub const MAX_PENDING_REQUESTS: usize = 32;

/// Maximum justification length
pub const MAX_JUSTIFICATION_LEN: usize = 256;

/// Request ID type
pub type RequestId = u64;

/// Access request status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RequestStatus {
    /// Request is pending review
    Pending = 0,
    /// Request was approved
    Approved = 1,
    /// Request was denied
    Denied = 2,
    /// Request timed out
    Expired = 3,
    /// Request was cancelled
    Cancelled = 4,
}

/// Decision on an access request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AccessDecision {
    /// Grant access (with optional time limit)
    Grant = 0,
    /// Deny access
    Deny = 1,
    /// Grant with redaction applied
    GrantRedacted = 2,
    /// Grant with summary only
    GrantSummary = 3,
}

/// An access escalation request
#[derive(Clone, Copy)]
pub struct AccessRequest {
    /// Unique request ID
    pub id: RequestId,
    /// Requester task/process ID
    pub requester_id: u8,
    /// Requester's current max tier
    pub current_tier: u8,
    /// Requested tier level
    pub requested_tier: u8,
    /// Target object SUID (if specific)
    pub target_suid_high: u64,
    pub target_suid_low: u64,
    /// Is this for a specific object or general escalation?
    pub is_specific: bool,
    /// Request status
    pub status: RequestStatus,
    /// Justification text
    justification: [u8; MAX_JUSTIFICATION_LEN],
    justification_len: usize,
    /// Timestamp (ticks) when created
    pub created_at: u64,
    /// Timestamp when decided (0 if pending)
    pub decided_at: u64,
    /// Decision (valid only if status is Approved/Denied)
    pub decision: AccessDecision,
    /// Time-limited grant duration (0 = permanent)
    pub grant_duration: u64,
}

impl AccessRequest {
    /// Create an empty request
    pub const fn empty() -> Self {
        Self {
            id: 0,
            requester_id: 0,
            current_tier: 0,
            requested_tier: 0,
            target_suid_high: 0,
            target_suid_low: 0,
            is_specific: false,
            status: RequestStatus::Pending,
            justification: [0u8; MAX_JUSTIFICATION_LEN],
            justification_len: 0,
            created_at: 0,
            decided_at: 0,
            decision: AccessDecision::Deny,
            grant_duration: 0,
        }
    }

    /// Create a new access request for a specific object
    pub fn for_object(
        requester_id: u8,
        current_tier: u8,
        requested_tier: u8,
        suid: &SUID,
        justification: &[u8],
    ) -> Self {
        let mut req = Self::empty();
        req.requester_id = requester_id;
        req.current_tier = current_tier;
        req.requested_tier = requested_tier;
        req.target_suid_high = suid.high;
        req.target_suid_low = suid.low;
        req.is_specific = true;

        let just_len = justification.len().min(MAX_JUSTIFICATION_LEN);
        req.justification[..just_len].copy_from_slice(&justification[..just_len]);
        req.justification_len = just_len;

        req
    }

    /// Create a new access request for general tier escalation
    pub fn for_tier(
        requester_id: u8,
        current_tier: u8,
        requested_tier: u8,
        justification: &[u8],
    ) -> Self {
        let mut req = Self::empty();
        req.requester_id = requester_id;
        req.current_tier = current_tier;
        req.requested_tier = requested_tier;
        req.is_specific = false;

        let just_len = justification.len().min(MAX_JUSTIFICATION_LEN);
        req.justification[..just_len].copy_from_slice(&justification[..just_len]);
        req.justification_len = just_len;

        req
    }

    /// Get justification text
    pub fn justification(&self) -> &[u8] {
        &self.justification[..self.justification_len]
    }

    /// Check if request is still pending
    pub fn is_pending(&self) -> bool {
        self.status == RequestStatus::Pending
    }

    /// Check if request was granted
    pub fn is_granted(&self) -> bool {
        self.status == RequestStatus::Approved
            && (self.decision == AccessDecision::Grant
                || self.decision == AccessDecision::GrantRedacted
                || self.decision == AccessDecision::GrantSummary)
    }

    /// Get target SUID if specific request
    pub fn target_suid(&self) -> Option<SUID> {
        if self.is_specific {
            Some(SUID::new(self.target_suid_high, self.target_suid_low))
        } else {
            None
        }
    }
}

/// Queue of pending escalation requests
pub struct EscalationQueue {
    /// Pending requests
    requests: [AccessRequest; MAX_PENDING_REQUESTS],
    /// Number of requests
    count: usize,
    /// Next request ID
    next_id: RequestId,
    /// Initialized flag
    initialized: bool,
}

impl EscalationQueue {
    /// Create a new escalation queue
    pub const fn new() -> Self {
        Self {
            requests: [AccessRequest::empty(); MAX_PENDING_REQUESTS],
            count: 0,
            next_id: 1,
            initialized: false,
        }
    }

    /// Initialize the queue
    pub fn init(&mut self) {
        self.initialized = true;
    }

    /// Submit a new access request
    /// Returns the request ID on success
    pub fn submit(&mut self, mut request: AccessRequest) -> Result<RequestId, super::LlmError> {
        if !self.initialized {
            return Err(super::LlmError::NotInitialized);
        }

        // Find a free slot
        let slot = self.find_free_slot().ok_or(super::LlmError::InternalError)?;

        // Assign ID and timestamp
        request.id = self.next_id;
        request.created_at = crate::platform::ticks();
        request.status = RequestStatus::Pending;

        self.requests[slot] = request;
        self.count += 1;
        self.next_id += 1;

        Ok(request.id)
    }

    /// Get a pending request by ID
    pub fn get(&self, id: RequestId) -> Option<&AccessRequest> {
        self.requests.iter().find(|r| r.id == id && r.id != 0)
    }

    /// Get a mutable reference to a request
    pub fn get_mut(&mut self, id: RequestId) -> Option<&mut AccessRequest> {
        self.requests.iter_mut().find(|r| r.id == id && r.id != 0)
    }

    /// Approve a request
    pub fn approve(
        &mut self,
        id: RequestId,
        decision: AccessDecision,
        duration: u64,
    ) -> Result<(), super::LlmError> {
        let request = self.get_mut(id).ok_or(super::LlmError::InvalidRequest)?;

        if request.status != RequestStatus::Pending {
            return Err(super::LlmError::InvalidRequest);
        }

        request.status = RequestStatus::Approved;
        request.decision = decision;
        request.grant_duration = duration;
        request.decided_at = crate::platform::ticks();

        Ok(())
    }

    /// Deny a request
    pub fn deny(&mut self, id: RequestId) -> Result<(), super::LlmError> {
        let request = self.get_mut(id).ok_or(super::LlmError::InvalidRequest)?;

        if request.status != RequestStatus::Pending {
            return Err(super::LlmError::InvalidRequest);
        }

        request.status = RequestStatus::Denied;
        request.decision = AccessDecision::Deny;
        request.decided_at = crate::platform::ticks();

        Ok(())
    }

    /// Cancel a pending request
    pub fn cancel(&mut self, id: RequestId, requester_id: u8) -> Result<(), super::LlmError> {
        let request = self.get_mut(id).ok_or(super::LlmError::InvalidRequest)?;

        // Only requester can cancel
        if request.requester_id != requester_id {
            return Err(super::LlmError::AccessDenied);
        }

        if request.status != RequestStatus::Pending {
            return Err(super::LlmError::InvalidRequest);
        }

        request.status = RequestStatus::Cancelled;
        request.decided_at = crate::platform::ticks();

        Ok(())
    }

    /// Expire old pending requests
    pub fn expire_old(&mut self, max_age_ticks: u64) {
        let now = crate::platform::ticks();

        for request in self.requests.iter_mut() {
            if request.id != 0
                && request.status == RequestStatus::Pending
                && now - request.created_at > max_age_ticks
            {
                request.status = RequestStatus::Expired;
                request.decided_at = now;
            }
        }
    }

    /// Get all pending requests for a requester
    pub fn pending_for_requester(&self, requester_id: u8) -> impl Iterator<Item = &AccessRequest> {
        self.requests.iter().filter(move |r| {
            r.id != 0 && r.status == RequestStatus::Pending && r.requester_id == requester_id
        })
    }

    /// Get all pending requests (for approval UI)
    pub fn all_pending(&self) -> impl Iterator<Item = &AccessRequest> {
        self.requests
            .iter()
            .filter(|r| r.id != 0 && r.status == RequestStatus::Pending)
    }

    /// Remove completed/cancelled requests to free slots
    pub fn cleanup(&mut self) {
        for request in self.requests.iter_mut() {
            if request.id != 0
                && (request.status == RequestStatus::Cancelled
                    || request.status == RequestStatus::Expired
                    || request.status == RequestStatus::Denied)
            {
                *request = AccessRequest::empty();
                self.count = self.count.saturating_sub(1);
            }
        }
    }

    /// Find a free slot
    fn find_free_slot(&self) -> Option<usize> {
        self.requests.iter().position(|r| r.id == 0)
    }

    /// Get number of pending requests
    pub fn pending_count(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| r.id != 0 && r.status == RequestStatus::Pending)
            .count()
    }
}

// Global escalation queue, behind the yield-on-contention kernel mutex
// (2026-07-17 review, P1).
static GLOBAL_ESCALATION_QUEUE: crate::sync::Mutex<EscalationQueue> =
    crate::sync::Mutex::new(EscalationQueue::new());

/// Lock the global escalation queue.
pub fn global_escalation_queue() -> crate::sync::MutexGuard<'static, EscalationQueue> {
    GLOBAL_ESCALATION_QUEUE.lock()
}

/// Initialize the escalation subsystem
pub fn init() {
    GLOBAL_ESCALATION_QUEUE.lock().init();
}
