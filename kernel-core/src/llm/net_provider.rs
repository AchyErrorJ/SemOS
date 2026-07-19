//! Network-Backed LLM Provider
//!
//! Client half of the remote-LLM path. Given a configured endpoint and a
//! [`NetworkTransport`](super::transport::NetworkTransport), it:
//!
//! 1. Frames an HTTP/1.1 POST request to a chat-completion endpoint
//!    (request body shaped after the Anthropic Messages API).
//! 2. Pushes it through the transport.
//! 3. Reads the response back, validates the status line, locates the
//!    body, and extracts the assistant completion from the JSON.
//!
//! All state is statically sized — there is no heap in kernel-core. The
//! response body is bounded by [`MAX_RESPONSE_BODY`], which is large
//! enough for a single chat turn's worth of output.
//!
//! # Configuration
//!
//! Currently a single global instance, set up at boot with sensible defaults
//! pointing at the loopback transport. A future syscall (`SYS_LLM_SET_ENDPOINT`)
//! can rewrite the host/port/path so user space chooses where requests go.

use super::transport::{NetworkTransport, TransportError, MAX_HOST_LEN, global_loopback_transport};
use super::LlmError;
use crate::tls::transport_tls::global_tls_transport;

/// Max length of the API path component (e.g. `/v1/messages`).
pub const MAX_PATH_LEN: usize = 64;

/// Max length of the model identifier.
pub const MAX_MODEL_LEN: usize = 64;

/// Max length of the bearer / API key.
pub const MAX_API_KEY_LEN: usize = 128;

/// Buffer used to build the outgoing HTTP request (headers + body).
pub const MAX_REQUEST_SIZE: usize = 4096;

/// Buffer used to receive the raw HTTP response.
pub const MAX_RESPONSE_SIZE: usize = 4096;

/// Max bytes of completion text we extract into the caller's response slot.
pub const MAX_RESPONSE_BODY: usize = 2048;

/// Which transport this provider is wired to.
///
/// - [`Loopback`](TransportKind::Loopback) — in-kernel mock peer used
///   by DEMO 10 (no NIC, no network). Synthesises an Anthropic-shaped
///   response so the parser path is exercised end-to-end.
/// - [`TlsTcp`](TransportKind::TlsTcp) — TLS 1.3 over TCP, the real
///   remote path. The transport is configured at boot via
///   [`crate::tls::transport_tls::configure_global`] with a target IP
///   (no DNS yet); the host string from the endpoint config becomes
///   the SNI name and feeds the SPKI verifier. See
///   `kernel-core/src/tls/transport_tls.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TransportKind {
    Loopback = 0,
    TlsTcp   = 1,
}

/// Wire-format flavour for the chat completion API.
///
/// Right now the loopback always answers in Anthropic shape, so that's our
/// default. The enum exists so the parser can branch when an OpenAI-style
/// `/v1/chat/completions` endpoint is configured later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApiFormat {
    Anthropic = 0,
    OpenAi = 1,
}

/// Endpoint configuration. Plain bytes so the whole thing is `Copy`.
#[derive(Clone, Copy)]
pub struct Endpoint {
    host: [u8; MAX_HOST_LEN],
    host_len: usize,
    port: u16,
    path: [u8; MAX_PATH_LEN],
    path_len: usize,
    model: [u8; MAX_MODEL_LEN],
    model_len: usize,
    api_key: [u8; MAX_API_KEY_LEN],
    api_key_len: usize,
    pub transport: TransportKind,
    pub format: ApiFormat,
    pub max_tokens: u32,
}

impl Endpoint {
    pub const fn empty() -> Self {
        Self {
            host: [0u8; MAX_HOST_LEN],
            host_len: 0,
            port: 0,
            path: [0u8; MAX_PATH_LEN],
            path_len: 0,
            model: [0u8; MAX_MODEL_LEN],
            model_len: 0,
            api_key: [0u8; MAX_API_KEY_LEN],
            api_key_len: 0,
            transport: TransportKind::Loopback,
            format: ApiFormat::Anthropic,
            max_tokens: 1024,
        }
    }

    /// Default endpoint used at boot — points at the in-kernel loopback
    /// peer with a stand-in model name. Real deployments rewrite this at
    /// runtime once a network stack and API credentials are available.
    pub fn loopback_default() -> Self {
        let mut e = Self::empty();
        e.set_host("loopback.local").ok();
        e.port = 443;
        e.set_path("/v1/messages").ok();
        e.set_model("claude-opus-4-7").ok();
        // No key required against the loopback peer; left empty.
        e.transport = TransportKind::Loopback;
        e.format = ApiFormat::Anthropic;
        e.max_tokens = 1024;
        e
    }

    pub fn set_host(&mut self, s: &str) -> Result<(), LlmError> {
        let b = s.as_bytes();
        if b.is_empty() || b.len() > MAX_HOST_LEN { return Err(LlmError::InvalidRequest); }
        self.host[..b.len()].copy_from_slice(b);
        self.host_len = b.len();
        Ok(())
    }
    pub fn set_path(&mut self, s: &str) -> Result<(), LlmError> {
        let b = s.as_bytes();
        if b.is_empty() || b.len() > MAX_PATH_LEN { return Err(LlmError::InvalidRequest); }
        self.path[..b.len()].copy_from_slice(b);
        self.path_len = b.len();
        Ok(())
    }
    pub fn set_model(&mut self, s: &str) -> Result<(), LlmError> {
        let b = s.as_bytes();
        if b.is_empty() || b.len() > MAX_MODEL_LEN { return Err(LlmError::InvalidRequest); }
        self.model[..b.len()].copy_from_slice(b);
        self.model_len = b.len();
        Ok(())
    }
    pub fn set_api_key(&mut self, s: &str) -> Result<(), LlmError> {
        let b = s.as_bytes();
        if b.len() > MAX_API_KEY_LEN { return Err(LlmError::InvalidRequest); }
        self.api_key[..b.len()].copy_from_slice(b);
        self.api_key_len = b.len();
        Ok(())
    }
    pub fn host(&self) -> &[u8]   { &self.host[..self.host_len] }
    pub fn path(&self) -> &[u8]   { &self.path[..self.path_len] }
    pub fn model(&self) -> &[u8]  { &self.model[..self.model_len] }
    pub fn api_key(&self) -> &[u8]{ &self.api_key[..self.api_key_len] }
    pub fn port(&self) -> u16     { self.port }
}

// ============================================================================
// NetworkLlmProvider
// ============================================================================

/// Network-backed LLM provider. One per kernel; holds the endpoint config
/// and per-call scratch buffers.
pub struct NetworkLlmProvider {
    initialized: bool,
    endpoint: Endpoint,
    /// Scratch for the outgoing request. Reused across calls.
    req_buf: [u8; MAX_REQUEST_SIZE],
    /// Scratch for the raw response. Reused across calls.
    resp_buf: [u8; MAX_RESPONSE_SIZE],
    /// Scratch for a de-chunked body (M13). Only used when the response
    /// carries `Transfer-Encoding: chunked`; otherwise untouched.
    chunk_buf: [u8; MAX_RESPONSE_SIZE],
    /// Last extracted completion text. Held so callers can inspect it after
    /// a `complete()` returns — useful for logging and the demo.
    last_completion: [u8; MAX_RESPONSE_BODY],
    last_completion_len: usize,
    /// Total successful round-trips since boot. Cheap health metric.
    success_count: u64,
    /// Total failed round-trips since boot.
    failure_count: u64,
}

impl NetworkLlmProvider {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            endpoint: Endpoint::empty(),
            req_buf: [0u8; MAX_REQUEST_SIZE],
            resp_buf: [0u8; MAX_RESPONSE_SIZE],
            chunk_buf: [0u8; MAX_RESPONSE_SIZE],
            last_completion: [0u8; MAX_RESPONSE_BODY],
            last_completion_len: 0,
            success_count: 0,
            failure_count: 0,
        }
    }

    pub fn init(&mut self) {
        self.endpoint = Endpoint::loopback_default();
        self.initialized = true;
    }

    pub fn is_initialized(&self) -> bool { self.initialized }
    pub fn endpoint(&self) -> &Endpoint { &self.endpoint }
    pub fn endpoint_mut(&mut self) -> &mut Endpoint { &mut self.endpoint }
    pub fn success_count(&self) -> u64 { self.success_count }
    pub fn failure_count(&self) -> u64 { self.failure_count }

    /// Last completion text the provider extracted. Empty if none yet.
    pub fn last_completion(&self) -> &[u8] {
        &self.last_completion[..self.last_completion_len]
    }

    /// Run a single chat-completion round-trip. Writes the assistant's reply
    /// (text only) into `response_out` and returns its length in bytes.
    ///
    /// On failure increments the failure counter and returns an `LlmError`
    /// that maps cleanly to the existing syscall error codes.
    pub fn complete(&mut self, prompt: &[u8], response_out: &mut [u8]) -> Result<usize, LlmError> {
        if !self.initialized {
            return Err(LlmError::NotInitialized);
        }
        if prompt.is_empty() || prompt.len() > 4096 {
            return Err(LlmError::InvalidRequest);
        }

        // 1. Build the request bytes.
        let req_len = self.build_request(prompt)?;

        // Snapshot the endpoint host as a stack-local string so we don't
        // hold a borrow on `self.endpoint` across the transport calls below.
        let host_bytes = self.endpoint.host().to_vec_inplace();
        let host_str = match core::str::from_utf8(host_bytes.as_slice()) {
            Ok(s) => s,
            Err(_) => return self.fail(LlmError::InvalidRequest),
        };
        let port = self.endpoint.port;
        let transport_kind = self.endpoint.transport;

        // 2. Drive the transport. Either backend is a global singleton
        // re-locked between phases — the `dyn NetworkTransport` coercion
        // lets the send/recv loops below not care which backend they're
        // driving. The guard holds the backend's kernel mutex for the
        // phase, so a preempted peer task can't interleave bytes into the
        // same connection (2026-07-17 review, P1).
        {
            let mut t = lock_transport(transport_kind);
            if !t.is_connected() {
                if let Err(e) = t.connect(host_str, port) {
                    return self.fail(transport_to_llm(e));
                }
            }
            // Send phase.
            let mut sent = 0;
            while sent < req_len {
                match t.send(&self.req_buf[sent..req_len]) {
                    Ok(0) => return self.fail(LlmError::InternalError),
                    Ok(n) => sent += n,
                    Err(e) => return self.fail(transport_to_llm(e)),
                }
            }
        }

        // Receive phase. Each iteration we re-lock the transport so the
        // guard is held only for the duration of one recv call.
        let mut total_resp = 0;
        loop {
            if total_resp >= self.resp_buf.len() {
                return self.fail(LlmError::ContextTooLarge);
            }
            let n_result = {
                let mut t = lock_transport(transport_kind);
                t.recv(&mut self.resp_buf[total_resp..])
            };
            match n_result {
                Ok(0) => break, // EOF
                Ok(n) => total_resp += n,
                Err(e) => return self.fail(transport_to_llm(e)),
            }
        }

        // 3. Close the transport (best-effort; ignore any error).
        {
            let mut t = lock_transport(transport_kind);
            t.close();
        }

        // 4. Parse the HTTP response, then the JSON body.
        //
        // M13: if the server framed the body with chunked transfer encoding
        // we must de-chunk before the JSON extractor sees it — otherwise the
        // hex chunk-length lines (e.g. "8d\r\n") leak into the parsed body.
        // De-chunk into `chunk_buf` and point `body` at the reassembled
        // bytes. All of this happens inside one borrow scope that touches
        // only `resp_buf`, `chunk_buf` and the caller's `response_out`, so we
        // can re-borrow `self` mutably afterward to update stats.
        let format = self.endpoint.format;
        let extract_result = {
            let Self { resp_buf, chunk_buf, .. } = self;
            let raw = &resp_buf[..total_resp];
            let body = match parse_http_body(raw) {
                Some(b) => {
                    // `b` is the body slice; the bytes before it are headers.
                    let header_len = raw.len() - b.len();
                    if crate::net::is_chunked(&raw[..header_len]) {
                        match crate::net::decode_chunked(b, &mut chunk_buf[..]) {
                            Ok(n) => Some(&chunk_buf[..n]),
                            Err(_) => None,
                        }
                    } else {
                        Some(b)
                    }
                }
                None => None,
            };
            body.and_then(|body| match format {
                ApiFormat::Anthropic => extract_anthropic_completion(body, response_out),
                ApiFormat::OpenAi => extract_openai_completion(body, response_out),
            })
        };
        let completion_len = match extract_result {
            Some(n) => n,
            None => return self.fail(LlmError::InternalError),
        };

        // 5. Cache for inspection and bump stats.
        let cache_len = completion_len.min(self.last_completion.len());
        self.last_completion[..cache_len].copy_from_slice(&response_out[..cache_len]);
        self.last_completion_len = cache_len;
        self.success_count = self.success_count.wrapping_add(1);

        Ok(completion_len)
    }

    /// Bump the failure counter and propagate the error.
    fn fail(&mut self, e: LlmError) -> Result<usize, LlmError> {
        self.failure_count = self.failure_count.wrapping_add(1);
        Err(e)
    }

    /// Build the HTTP/1.1 request into `self.req_buf`. Returns its length.
    fn build_request(&mut self, prompt: &[u8]) -> Result<usize, LlmError> {
        // First build the JSON body into a scratch region, then prepend
        // headers (Content-Length needs the body length).
        //
        // Layout: [headers ........... CRLFCRLF | body ............]
        //
        // We build body at the *end* of req_buf, then shift it after writing
        // headers. To avoid the shift, build into a separate stack buffer.
        let mut body = [0u8; 2048];
        let body_len = build_chat_body(self.endpoint.format,
                                       self.endpoint.model(),
                                       self.endpoint.max_tokens,
                                       prompt,
                                       &mut body)
            .ok_or(LlmError::InvalidRequest)?;

        let mut p = 0;
        // Request line.
        p += copy_into(&mut self.req_buf[p..], b"POST ")
            .ok_or(LlmError::InternalError)?;
        p += copy_into(&mut self.req_buf[p..], self.endpoint.path())
            .ok_or(LlmError::InternalError)?;
        p += copy_into(&mut self.req_buf[p..], b" HTTP/1.1\r\n")
            .ok_or(LlmError::InternalError)?;

        // Host.
        p += copy_into(&mut self.req_buf[p..], b"Host: ")
            .ok_or(LlmError::InternalError)?;
        p += copy_into(&mut self.req_buf[p..], self.endpoint.host())
            .ok_or(LlmError::InternalError)?;
        p += copy_into(&mut self.req_buf[p..], b"\r\n")
            .ok_or(LlmError::InternalError)?;

        // Standard headers.
        p += copy_into(&mut self.req_buf[p..],
            b"Content-Type: application/json\r\n")
            .ok_or(LlmError::InternalError)?;
        p += copy_into(&mut self.req_buf[p..],
            b"Accept: application/json\r\n")
            .ok_or(LlmError::InternalError)?;
        p += copy_into(&mut self.req_buf[p..],
            b"Connection: close\r\n")
            .ok_or(LlmError::InternalError)?;

        // Auth headers, per API format.
        match self.endpoint.format {
            ApiFormat::Anthropic => {
                if self.endpoint.api_key_len > 0 {
                    p += copy_into(&mut self.req_buf[p..], b"x-api-key: ")
                        .ok_or(LlmError::InternalError)?;
                    p += copy_into(&mut self.req_buf[p..], self.endpoint.api_key())
                        .ok_or(LlmError::InternalError)?;
                    p += copy_into(&mut self.req_buf[p..], b"\r\n")
                        .ok_or(LlmError::InternalError)?;
                }
                p += copy_into(&mut self.req_buf[p..],
                    b"anthropic-version: 2023-06-01\r\n")
                    .ok_or(LlmError::InternalError)?;
            }
            ApiFormat::OpenAi => {
                if self.endpoint.api_key_len > 0 {
                    p += copy_into(&mut self.req_buf[p..], b"Authorization: Bearer ")
                        .ok_or(LlmError::InternalError)?;
                    p += copy_into(&mut self.req_buf[p..], self.endpoint.api_key())
                        .ok_or(LlmError::InternalError)?;
                    p += copy_into(&mut self.req_buf[p..], b"\r\n")
                        .ok_or(LlmError::InternalError)?;
                }
            }
        }

        // Content-Length + blank line + body.
        p += copy_into(&mut self.req_buf[p..], b"Content-Length: ")
            .ok_or(LlmError::InternalError)?;
        let dec_len = write_decimal(&mut self.req_buf[p..], body_len as u64);
        if dec_len == 0 { return Err(LlmError::InternalError); }
        p += dec_len;
        p += copy_into(&mut self.req_buf[p..], b"\r\n\r\n")
            .ok_or(LlmError::InternalError)?;
        p += copy_into(&mut self.req_buf[p..], &body[..body_len])
            .ok_or(LlmError::ContextTooLarge)?;

        Ok(p)
    }

}

// ============================================================================
// Free helpers for body building and response parsing.
// ============================================================================

/// Locked view of whichever global transport backend an `Endpoint`
/// selects. Holds the backend's kernel-mutex guard and derefs straight to
/// `dyn NetworkTransport`, replacing the old `&'static mut` match arms.
enum TransportGuard {
    Loopback(crate::sync::MutexGuard<'static, super::transport::LoopbackTransport>),
    Tls(crate::sync::MutexGuard<'static, crate::tls::transport_tls::TlsTransport>),
}

/// Lock the global transport backend for `kind`.
fn lock_transport(kind: TransportKind) -> TransportGuard {
    match kind {
        TransportKind::Loopback => TransportGuard::Loopback(global_loopback_transport()),
        TransportKind::TlsTcp   => TransportGuard::Tls(global_tls_transport()),
    }
}

impl core::ops::Deref for TransportGuard {
    type Target = dyn NetworkTransport + 'static;
    fn deref(&self) -> &(dyn NetworkTransport + 'static) {
        match self {
            TransportGuard::Loopback(g) => &**g,
            TransportGuard::Tls(g) => &**g,
        }
    }
}

impl core::ops::DerefMut for TransportGuard {
    fn deref_mut(&mut self) -> &mut (dyn NetworkTransport + 'static) {
        match self {
            TransportGuard::Loopback(g) => &mut **g,
            TransportGuard::Tls(g) => &mut **g,
        }
    }
}

/// Convert a `TransportError` into the matching `LlmError`.
fn transport_to_llm(e: TransportError) -> LlmError {
    match e {
        TransportError::InvalidHost
        | TransportError::InvalidState => LlmError::InvalidRequest,
        TransportError::Closed         => LlmError::ProviderUnavailable,
        TransportError::BufferTooSmall
        | TransportError::NoBuffers    => LlmError::ContextTooLarge,
        TransportError::Timeout
        | TransportError::Io           => LlmError::InternalError,
    }
}

/// Build the JSON request body for the configured API format.
/// Returns the length on success, or `None` if the body wouldn't fit.
fn build_chat_body(
    format: ApiFormat,
    model: &[u8],
    max_tokens: u32,
    prompt: &[u8],
    out: &mut [u8],
) -> Option<usize> {
    let mut p = 0;
    p += copy_into(&mut out[p..], b"{\"model\":\"")?;
    p += copy_into(&mut out[p..], model)?;
    p += copy_into(&mut out[p..], b"\",\"max_tokens\":")?;
    let dl = write_decimal(&mut out[p..], max_tokens as u64);
    if dl == 0 { return None; }
    p += dl;
    p += copy_into(&mut out[p..], b",\"messages\":[{\"role\":\"user\",\"content\":\"")?;
    p += copy_into_escaped(&mut out[p..], prompt)?;
    p += copy_into(&mut out[p..], b"\"}]}")?;
    // For OpenAI we'd put `model` + `messages` at the top level too — the
    // shape we emit is compatible with both for the fields the loopback
    // looks at. Format-specific quirks (e.g. `system` prompts) belong in a
    // follow-up.
    let _ = format;
    Some(p)
}

/// Find the body of an HTTP response: the bytes after the first `\r\n\r\n`.
/// Validates a `2xx` status line. Returns `None` on malformed responses.
fn parse_http_body(resp: &[u8]) -> Option<&[u8]> {
    // Status line: "HTTP/1.1 <code> ..."
    if resp.len() < 12 || !resp.starts_with(b"HTTP/1.") {
        return None;
    }
    // Find space after the version.
    let space = resp.iter().position(|&b| b == b' ')?;
    let after = &resp[space + 1..];
    if after.len() < 3 { return None; }
    if after[0] != b'2' { return None; } // accept any 2xx
    if !after[1].is_ascii_digit() || !after[2].is_ascii_digit() {
        return None;
    }

    // Header/body split.
    let sep = find_subslice(resp, b"\r\n\r\n")?;
    Some(&resp[sep + 4..])
}

/// Extract the assistant's reply from an Anthropic Messages response.
/// Looks for `"text":"..."` inside `"content":[...]` and decodes it into
/// `out`, returning the byte length.
fn extract_anthropic_completion(body: &[u8], out: &mut [u8]) -> Option<usize> {
    // We deliberately use the LAST `"text":"..."` we find, on the theory
    // that any earlier matches would be inside metadata we don't care about.
    // For a single-turn response that's exactly the assistant's message.
    extract_last_json_string(body, b"text", out)
}

/// Extract the assistant's reply from an OpenAI chat-completion response.
/// Looks for `"content":"..."` inside `choices[0].message`.
fn extract_openai_completion(body: &[u8], out: &mut [u8]) -> Option<usize> {
    extract_last_json_string(body, b"content", out)
}

/// Find the LAST `"key":"value"` in `body` and write `value` (unescaped)
/// into `out`. Returns the number of bytes written, or `None` if no match.
fn extract_last_json_string(body: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut marker = [0u8; 32];
    if key.len() + 2 > marker.len() { return None; }
    marker[0] = b'"';
    marker[1..1 + key.len()].copy_from_slice(key);
    marker[1 + key.len()] = b'"';
    let marker = &marker[..key.len() + 2];

    let mut search_start = 0;
    let mut last: Option<(usize, usize)> = None;
    while let Some(rel) = find_subslice(&body[search_start..], marker) {
        let abs = search_start + rel;
        let mut p = abs + marker.len();
        while p < body.len() && (body[p] == b' ' || body[p] == b'\t') { p += 1; }
        if p >= body.len() || body[p] != b':' {
            search_start = abs + marker.len();
            continue;
        }
        p += 1;
        while p < body.len() && (body[p] == b' ' || body[p] == b'\t') { p += 1; }
        if p >= body.len() || body[p] != b'"' {
            search_start = abs + marker.len();
            continue;
        }
        p += 1;
        let vs = p;
        while p < body.len() && body[p] != b'"' {
            if body[p] == b'\\' && p + 1 < body.len() { p += 2; } else { p += 1; }
        }
        if p >= body.len() { return None; }
        last = Some((vs, p));
        search_start = p + 1;
    }

    let (vs, ve) = last?;
    let mut written = 0;
    let mut i = vs;
    while i < ve && written < out.len() {
        if body[i] == b'\\' && i + 1 < ve {
            let c = match body[i + 1] {
                b'"' => b'"',
                b'\\' => b'\\',
                b'/' => b'/',
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'b' => 0x08,
                b'f' => 0x0C,
                other => other,
            };
            out[written] = c;
            written += 1;
            i += 2;
        } else {
            out[written] = body[i];
            written += 1;
            i += 1;
        }
    }
    Some(written)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() { return None; }
    let last = haystack.len() - needle.len();
    let mut i = 0;
    while i <= last {
        if &haystack[i..i + needle.len()] == needle { return Some(i); }
        i += 1;
    }
    None
}

/// Copy as many bytes from `src` into `dst` as fit. Returns the count, or
/// `None` if `dst` was too small to take the full `src` (caller wants the
/// whole thing on success, not a partial write).
fn copy_into(dst: &mut [u8], src: &[u8]) -> Option<usize> {
    if src.len() > dst.len() { return None; }
    dst[..src.len()].copy_from_slice(src);
    Some(src.len())
}

/// Copy `src` into `dst`, JSON-escaping characters as needed. Returns the
/// number of output bytes written, or `None` if `dst` was too small.
fn copy_into_escaped(dst: &mut [u8], src: &[u8]) -> Option<usize> {
    let mut p = 0;
    for &b in src {
        match b {
            b'"'  => p += copy_into(&mut dst[p..], b"\\\"")?,
            b'\\' => p += copy_into(&mut dst[p..], b"\\\\")?,
            b'\n' => p += copy_into(&mut dst[p..], b"\\n")?,
            b'\r' => p += copy_into(&mut dst[p..], b"\\r")?,
            b'\t' => p += copy_into(&mut dst[p..], b"\\t")?,
            0x20..=0x7E => {
                if p >= dst.len() { return None; }
                dst[p] = b;
                p += 1;
            }
            _ => {
                // Drop or replace non-printable bytes. The prompt is meant
                // to be UTF-8 text; anything outside ASCII printable is
                // currently replaced with '?' to keep the body strictly
                // ASCII (no unicode escape table needed).
                if p >= dst.len() { return None; }
                dst[p] = b'?';
                p += 1;
            }
        }
    }
    Some(p)
}

/// Format `n` as decimal into `dst`. Returns bytes written, 0 on failure.
fn write_decimal(dst: &mut [u8], n: u64) -> usize {
    if n == 0 {
        if dst.is_empty() { return 0; }
        dst[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut k = 0;
    let mut v = n;
    while v > 0 && k < tmp.len() {
        tmp[k] = b'0' + (v % 10) as u8;
        v /= 10;
        k += 1;
    }
    if dst.len() < k { return 0; }
    for i in 0..k {
        dst[i] = tmp[k - 1 - i];
    }
    k
}

// ============================================================================
// Tiny "to_vec" replacement to dodge the no-alloc constraint when we just
// need to keep a short string of known max length on the stack.
// ============================================================================

/// A stack-allocated byte buffer used for short host names.
pub struct StackBytes {
    data: [u8; MAX_HOST_LEN],
    len: usize,
}
impl StackBytes {
    pub fn as_slice(&self) -> &[u8] { &self.data[..self.len] }
}

trait ToStackBytes {
    fn to_vec_inplace(&self) -> StackBytes;
}
impl ToStackBytes for [u8] {
    fn to_vec_inplace(&self) -> StackBytes {
        let mut sb = StackBytes { data: [0u8; MAX_HOST_LEN], len: 0 };
        let n = self.len().min(MAX_HOST_LEN);
        sb.data[..n].copy_from_slice(&self[..n]);
        sb.len = n;
        sb
    }
}

// ============================================================================
// Global instance
// ============================================================================

static GLOBAL_NET_PROVIDER: crate::sync::Mutex<NetworkLlmProvider> =
    crate::sync::Mutex::new(NetworkLlmProvider::new());

/// Lock the global network provider (2026-07-17 review, P1 — was a bare
/// `static mut` under the "single-threaded kernel" contract).
pub fn global_net_provider() -> crate::sync::MutexGuard<'static, NetworkLlmProvider> {
    GLOBAL_NET_PROVIDER.lock()
}

/// Initialise the global network provider (sets defaults pointing at the
/// loopback transport).
pub fn init() {
    GLOBAL_NET_PROVIDER.lock().init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_via_loopback() {
        super::super::transport::init();
        init();
        let prompt = b"hello, world";
        let mut out = [0u8; 512];
        {
            let mut net = global_net_provider();
            let n = net.complete(prompt, &mut out).unwrap();
            let s = core::str::from_utf8(&out[..n]).unwrap();
            assert!(s.contains("[loopback] echo: hello, world"));
            assert_eq!(net.success_count(), 1);
            assert_eq!(net.failure_count(), 0);
        }
    }

    #[test]
    fn parse_http_body_rejects_non_2xx() {
        let resp = b"HTTP/1.1 500 Internal Server Error\r\n\r\nboom";
        assert!(parse_http_body(resp).is_none());
    }
}
