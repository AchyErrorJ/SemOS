//! LLM Provider Interface
//!
//! Defines the interface for LLM providers and manages provider state.
//! In the bare-metal kernel, actual LLM inference isn't performed;
//! instead, requests are queued for external processing or a future
//! local model.
//!
//! # Provider Types
//!
//! - **Local**: Future embedded model (e.g., small LLaMA variant)
//! - **Remote**: External API (requires network driver)
//! - **Mock**: For testing, returns predefined responses
//!
//! # Request Flow
//!
//! ```text
//! Application
//!     │
//!     ▼
//! LlmRequest (query + context)
//!     │
//!     ▼
//! Provider.process()
//!     │
//!     ├──► Local Model (future)
//!     │
//!     ├──► Remote API (requires network)
//!     │
//!     └──► Mock Response (testing)
//!     │
//!     ▼
//! LlmResponse (answer + metadata)
//! ```

use super::{LlmContext, LlmError, MAX_CONTEXT_SIZE};

/// Maximum request prompt length
pub const MAX_PROMPT_LEN: usize = 1024;

/// Maximum response length
pub const MAX_RESPONSE_LEN: usize = 4096;

/// Maximum queued requests
pub const MAX_QUEUED_REQUESTS: usize = 16;

/// Provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProviderType {
    /// No provider configured
    None = 0,
    /// Mock provider for testing
    Mock = 1,
    /// Local inference (future)
    Local = 2,
    /// Remote API (requires network)
    Remote = 3,
}

/// Request status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RequestState {
    /// Request is being prepared
    Preparing = 0,
    /// Request is queued
    Queued = 1,
    /// Request is being processed
    Processing = 2,
    /// Request completed successfully
    Completed = 3,
    /// Request failed
    Failed = 4,
    /// Request was cancelled
    Cancelled = 5,
}

/// LLM request ID
pub type LlmRequestId = u64;

/// An LLM request
#[derive(Clone, Copy)]
pub struct LlmRequest {
    /// Unique request ID
    pub id: LlmRequestId,
    /// Requester task ID
    pub requester_id: u8,
    /// Requester's max tier
    pub requester_tier: u8,
    /// Request state
    pub state: RequestState,
    /// Prompt/query
    prompt: [u8; MAX_PROMPT_LEN],
    prompt_len: usize,
    /// Context entry count
    pub context_entries: usize,
    /// Total context size
    pub context_size: usize,
    /// Created timestamp
    pub created_at: u64,
    /// Completed timestamp
    pub completed_at: u64,
}

impl LlmRequest {
    /// Create an empty request
    pub const fn empty() -> Self {
        Self {
            id: 0,
            requester_id: 0,
            requester_tier: 0,
            state: RequestState::Preparing,
            prompt: [0u8; MAX_PROMPT_LEN],
            prompt_len: 0,
            context_entries: 0,
            context_size: 0,
            created_at: 0,
            completed_at: 0,
        }
    }

    /// Create a new request
    pub fn new(requester_id: u8, requester_tier: u8, prompt: &[u8]) -> Self {
        let mut req = Self::empty();
        req.requester_id = requester_id;
        req.requester_tier = requester_tier;

        let copy_len = prompt.len().min(MAX_PROMPT_LEN);
        req.prompt[..copy_len].copy_from_slice(&prompt[..copy_len]);
        req.prompt_len = copy_len;

        req
    }

    /// Get the prompt
    pub fn prompt(&self) -> &[u8] {
        &self.prompt[..self.prompt_len]
    }

    /// Set context metadata
    pub fn set_context_info(&mut self, entries: usize, size: usize) {
        self.context_entries = entries;
        self.context_size = size;
    }

    /// Check if request is pending
    pub fn is_pending(&self) -> bool {
        matches!(
            self.state,
            RequestState::Queued | RequestState::Processing | RequestState::Preparing
        )
    }
}

/// An LLM response
#[derive(Clone, Copy)]
pub struct LlmResponse {
    /// Request ID this responds to
    pub request_id: LlmRequestId,
    /// Response content
    content: [u8; MAX_RESPONSE_LEN],
    content_len: usize,
    /// Was response truncated?
    pub truncated: bool,
    /// Number of context entries used
    pub context_used: usize,
    /// Processing time (ticks)
    pub processing_time: u64,
    /// Error code (0 = success)
    pub error_code: i64,
}

impl LlmResponse {
    /// Create an empty response
    pub const fn empty() -> Self {
        Self {
            request_id: 0,
            content: [0u8; MAX_RESPONSE_LEN],
            content_len: 0,
            truncated: false,
            context_used: 0,
            processing_time: 0,
            error_code: 0,
        }
    }

    /// Create a successful response
    pub fn success(request_id: LlmRequestId, content: &[u8], context_used: usize) -> Self {
        let mut resp = Self::empty();
        resp.request_id = request_id;
        resp.context_used = context_used;

        let copy_len = content.len().min(MAX_RESPONSE_LEN);
        resp.content[..copy_len].copy_from_slice(&content[..copy_len]);
        resp.content_len = copy_len;
        resp.truncated = content.len() > MAX_RESPONSE_LEN;

        resp
    }

    /// Create an error response
    pub fn error(request_id: LlmRequestId, error_code: i64) -> Self {
        let mut resp = Self::empty();
        resp.request_id = request_id;
        resp.error_code = error_code;
        resp
    }

    /// Get response content
    pub fn content(&self) -> &[u8] {
        &self.content[..self.content_len]
    }

    /// Check if response indicates success
    pub fn is_success(&self) -> bool {
        self.error_code == 0
    }
}

/// LLM provider interface
pub struct LlmProvider {
    /// Provider type
    provider_type: ProviderType,
    /// Is provider available?
    available: bool,
    /// Initialized flag
    initialized: bool,
    /// Request queue
    requests: [LlmRequest; MAX_QUEUED_REQUESTS],
    /// Response buffer (most recent)
    last_response: LlmResponse,
    /// Next request ID
    next_id: LlmRequestId,
    /// Pending request count
    pending_count: usize,
}

impl LlmProvider {
    /// Create a new provider
    pub const fn new() -> Self {
        Self {
            provider_type: ProviderType::Mock,
            available: false,
            initialized: false,
            requests: [LlmRequest::empty(); MAX_QUEUED_REQUESTS],
            last_response: LlmResponse::empty(),
            next_id: 1,
            pending_count: 0,
        }
    }

    /// Initialize the provider
    pub fn init(&mut self) {
        self.initialized = true;
        self.available = true;
        self.provider_type = ProviderType::Mock;
    }

    /// Set provider type
    pub fn set_type(&mut self, provider_type: ProviderType) {
        self.provider_type = provider_type;
        self.available = provider_type != ProviderType::None;
    }

    /// Check if provider is available
    pub fn is_available(&self) -> bool {
        self.initialized && self.available
    }

    /// Get provider type
    pub fn provider_type(&self) -> ProviderType {
        self.provider_type
    }

    /// Submit a request
    pub fn submit(&mut self, mut request: LlmRequest) -> Result<LlmRequestId, LlmError> {
        if !self.is_available() {
            return Err(LlmError::ProviderUnavailable);
        }

        // Find free slot
        let slot = self
            .requests
            .iter()
            .position(|r| r.id == 0)
            .ok_or(LlmError::InternalError)?;

        request.id = self.next_id;
        request.state = RequestState::Queued;
        request.created_at = crate::platform::ticks();

        self.requests[slot] = request;
        self.next_id += 1;
        self.pending_count += 1;

        Ok(request.id)
    }

    /// Process pending requests (called from kernel loop)
    pub fn process_pending(&mut self) {
        if !self.is_available() {
            return;
        }

        // Find index of first queued request
        let mut request_index = None;
        let mut request_id = 0u64;
        let mut context_entries = 0usize;
        let mut prompt_copy = [0u8; MAX_PROMPT_LEN];
        let mut prompt_len = 0usize;

        for (i, request) in self.requests.iter_mut().enumerate() {
            if request.id != 0 && request.state == RequestState::Queued {
                request.state = RequestState::Processing;
                request_index = Some(i);
                request_id = request.id;
                context_entries = request.context_entries;
                prompt_len = request.prompt().len().min(MAX_PROMPT_LEN);
                prompt_copy[..prompt_len].copy_from_slice(&request.prompt()[..prompt_len]);
                break;
            }
        }

        if let Some(idx) = request_index {
            // Process based on provider type (using copied data)
            let response = match self.provider_type {
                ProviderType::Mock => self.mock_process_prompt(&prompt_copy[..prompt_len], request_id, context_entries),
                ProviderType::Local => LlmResponse::error(request_id, LlmError::ProviderUnavailable as i64),
                ProviderType::Remote => self.remote_process_prompt(&prompt_copy[..prompt_len], request_id, context_entries),
                ProviderType::None => LlmResponse::error(request_id, LlmError::ProviderUnavailable as i64),
            };

            // Update request state
            self.requests[idx].state = if response.is_success() {
                RequestState::Completed
            } else {
                RequestState::Failed
            };
            self.requests[idx].completed_at = crate::platform::ticks();

            self.last_response = response;
            self.pending_count = self.pending_count.saturating_sub(1);
        }
    }

    /// Mock processing using copied prompt data
    fn mock_process_prompt(&self, prompt: &[u8], request_id: u64, context_entries: usize) -> LlmResponse {
        let response_text: &[u8] = if prompt.starts_with(b"search") || prompt.starts_with(b"find") {
            b"[MOCK] Found 3 relevant documents matching your query."
        } else if prompt.starts_with(b"summarize") || prompt.starts_with(b"summary") {
            b"[MOCK] Summary: Key concepts in semantic memory."
        } else if prompt.starts_with(b"explain") {
            b"[MOCK] Explanation: Semantic OS uses content-addressable memory."
        } else {
            b"[MOCK] Request processed. Use actual LLM in production."
        };

        LlmResponse::success(request_id, response_text, context_entries)
    }

    /// Mock processing for testing
    fn mock_process(&self, request: &LlmRequest) -> LlmResponse {
        // Generate a mock response based on the prompt
        let prompt = request.prompt();

        let response_text: &[u8] = if prompt.starts_with(b"search") || prompt.starts_with(b"find") {
            b"[MOCK] Found 3 relevant documents matching your query."
        } else if prompt.starts_with(b"summarize") || prompt.starts_with(b"summary") {
            b"[MOCK] Summary: Key concepts in semantic memory."
        } else if prompt.starts_with(b"explain") {
            b"[MOCK] Explanation: Semantic OS uses content-addressable memory."
        } else {
            b"[MOCK] Request processed. Use actual LLM in production."
        };

        LlmResponse::success(request.id, response_text, request.context_entries)
    }

    /// Local inference (placeholder)
    fn local_process(&self, request: &LlmRequest) -> LlmResponse {
        // Local model not yet implemented
        LlmResponse::error(
            request.id,
            LlmError::ProviderUnavailable as i64,
        )
    }

    /// Remote API call (placeholder, called via the deprecated `LlmRequest`
    /// path — the live dispatch goes through [`remote_process_prompt`] below).
    fn remote_process(&self, request: &LlmRequest) -> LlmResponse {
        LlmResponse::error(
            request.id,
            LlmError::ProviderUnavailable as i64,
        )
    }

    /// Remote-API processing using a copied prompt slice. Routes through
    /// [`crate::llm::net_provider::NetworkLlmProvider`], which frames an
    /// HTTP/1.1 request and pushes it down whatever transport is configured
    /// (today: loopback; tomorrow: TCP over e1000).
    fn remote_process_prompt(&self, prompt: &[u8], request_id: u64, context_entries: usize) -> LlmResponse {
        let mut completion = [0u8; super::net_provider::MAX_RESPONSE_BODY];
        let result = unsafe {
            super::net_provider::global_net_provider().complete(prompt, &mut completion)
        };
        match result {
            Ok(n) => LlmResponse::success(request_id, &completion[..n], context_entries),
            Err(e) => LlmResponse::error(request_id, e.to_error_code()),
        }
    }

    /// Get response for a request
    pub fn get_response(&self, request_id: LlmRequestId) -> Option<&LlmResponse> {
        if self.last_response.request_id == request_id {
            Some(&self.last_response)
        } else {
            None
        }
    }

    /// Get request status
    pub fn get_status(&self, request_id: LlmRequestId) -> Option<RequestState> {
        self.requests
            .iter()
            .find(|r| r.id == request_id)
            .map(|r| r.state)
    }

    /// Cancel a pending request
    pub fn cancel(&mut self, request_id: LlmRequestId, requester_id: u8) -> Result<(), LlmError> {
        let request = self
            .requests
            .iter_mut()
            .find(|r| r.id == request_id)
            .ok_or(LlmError::InvalidRequest)?;

        if request.requester_id != requester_id {
            return Err(LlmError::AccessDenied);
        }

        if !request.is_pending() {
            return Err(LlmError::InvalidRequest);
        }

        request.state = RequestState::Cancelled;
        self.pending_count = self.pending_count.saturating_sub(1);

        Ok(())
    }

    /// Cleanup completed requests
    pub fn cleanup(&mut self) {
        for request in self.requests.iter_mut() {
            if request.id != 0
                && matches!(
                    request.state,
                    RequestState::Completed | RequestState::Failed | RequestState::Cancelled
                )
            {
                *request = LlmRequest::empty();
            }
        }
    }

    /// Get number of pending requests
    pub fn pending_count(&self) -> usize {
        self.pending_count
    }
}

// Global provider instance
static mut GLOBAL_LLM_PROVIDER: LlmProvider = LlmProvider::new();

/// Get the global LLM provider
pub unsafe fn global_provider() -> &'static mut LlmProvider {
    &mut *core::ptr::addr_of_mut!(GLOBAL_LLM_PROVIDER)
}

/// Initialize the provider subsystem
pub fn init() {
    unsafe {
        GLOBAL_LLM_PROVIDER.init();
    }
}
