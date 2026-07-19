//! Network Transport Abstraction for LLM Remote Providers
//!
//! Defines a generic byte-stream transport used by [`net_provider`] to talk
//! to a remote chat-completion endpoint. The trait deliberately mirrors the
//! shape of a TCP connection (`connect`/`send`/`recv`/`close`), so a future
//! `TcpTransport` backed by an e1000 driver + minimal TCP stack can drop in
//! with zero changes to the provider layer above.
//!
//! For now we ship one concrete transport:
//!
//! - [`LoopbackTransport`]: a software loopback that parses the outgoing
//!   HTTP request, extracts the prompt from the JSON body, and synthesises a
//!   well-formed Anthropic-shaped JSON response. This exercises the full
//!   framing/parsing path end-to-end inside the kernel, with no NIC.
//!
//! # Layering
//!
//! ```text
//!   LlmProvider::remote_process()
//!         │
//!         ▼
//!   NetworkLlmProvider  (net_provider.rs)   ← builds HTTP, parses JSON
//!         │
//!         ▼
//!   NetworkTransport trait                  ← this file
//!         │
//!         ├── LoopbackTransport             (here, today)
//!         └── TcpTransport                  (future, on top of e1000+TCP)
//! ```

/// Errors a transport can raise.
///
/// Kept small and `Copy` so callers can match without lifetime gymnastics —
/// matches the style of `LlmError` and `DriverError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// `connect` called twice without `close`, or send/recv before connect.
    InvalidState,
    /// Host string was malformed or longer than [`MAX_HOST_LEN`].
    InvalidHost,
    /// Peer dropped the connection / loopback rejected the request.
    Closed,
    /// Caller's buffer was too small to hold a single chunk.
    BufferTooSmall,
    /// The transport's internal buffer is full.
    NoBuffers,
    /// Transport-level timeout (unused by loopback, reserved for TCP).
    Timeout,
    /// Generic I/O failure.
    Io,
}

/// Max length of a host string accepted by [`NetworkTransport::connect`].
/// Sized for typical API hostnames (`api.anthropic.com`, `api.openai.com`).
pub const MAX_HOST_LEN: usize = 64;

/// Trait implemented by anything that can ferry bytes for the LLM provider.
///
/// One connection at a time per transport instance. Reads and writes are
/// best-effort — `send` returns how many bytes were accepted, `recv` how
/// many were delivered; both can be called in a loop until done.
pub trait NetworkTransport {
    /// Open a connection to `host:port`. Closes any previous one first.
    fn connect(&mut self, host: &str, port: u16) -> Result<(), TransportError>;

    /// Push bytes toward the peer. Returns bytes accepted (may be < `data.len()`).
    fn send(&mut self, data: &[u8]) -> Result<usize, TransportError>;

    /// Pull bytes from the peer. Returns bytes written (0 = peer half-closed).
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError>;

    /// Tear the connection down. Safe to call when not connected.
    fn close(&mut self);

    /// Is a connection currently open?
    fn is_connected(&self) -> bool;

    /// Human-readable transport identifier, for logging.
    fn name(&self) -> &'static str;
}

// ============================================================================
// LoopbackTransport — in-kernel mock peer
// ============================================================================

/// Size of each direction's buffer. Big enough for the headers + a small
/// JSON body. Real TCP will not use these — it carries its own ring buffers.
pub const LOOPBACK_BUF_SIZE: usize = 4096;

/// In-kernel mock that pretends to be a remote chat-completion endpoint.
///
/// Lifecycle:
/// 1. `connect`           — clears buffers, marks connected.
/// 2. `send(http_request)` — accumulates into `tx_buf` until headers + body
///                           complete, then synthesises a JSON response into
///                           `rx_buf`.
/// 3. `recv(out)`         — drains `rx_buf`.
/// 4. `close`              — resets state.
pub struct LoopbackTransport {
    connected: bool,
    /// Bytes the caller has pushed. Once we see a complete HTTP request
    /// (`\r\n\r\n` + Content-Length body) we synthesise a response.
    tx_buf: [u8; LOOPBACK_BUF_SIZE],
    tx_len: usize,
    /// Bytes ready to be read back as the "response".
    rx_buf: [u8; LOOPBACK_BUF_SIZE],
    rx_len: usize,
    rx_pos: usize,
    /// True once we've synthesised the response for the current request.
    response_ready: bool,
    /// Number of round-trips serviced since boot — useful for the demo.
    request_count: u64,
}

impl LoopbackTransport {
    pub const fn new() -> Self {
        Self {
            connected: false,
            tx_buf: [0u8; LOOPBACK_BUF_SIZE],
            tx_len: 0,
            rx_buf: [0u8; LOOPBACK_BUF_SIZE],
            rx_len: 0,
            rx_pos: 0,
            response_ready: false,
            request_count: 0,
        }
    }

    /// How many requests this loopback has serviced.
    pub fn request_count(&self) -> u64 {
        self.request_count
    }

    /// Reset all per-connection state.
    fn reset(&mut self) {
        self.tx_len = 0;
        self.rx_len = 0;
        self.rx_pos = 0;
        self.response_ready = false;
    }

    /// Check whether `tx_buf` now holds a complete HTTP/1.1 request
    /// (headers terminated by `\r\n\r\n` plus a body of `Content-Length`
    /// bytes). If so, synthesise the response.
    fn try_complete_request(&mut self) {
        if self.response_ready {
            return;
        }

        // Find the header/body boundary.
        let headers_end = match find_subslice(&self.tx_buf[..self.tx_len], b"\r\n\r\n") {
            Some(idx) => idx,
            None => return,
        };
        let body_start = headers_end + 4;

        // Pull Content-Length out of the headers. Default to 0 if absent.
        let content_length =
            parse_content_length(&self.tx_buf[..headers_end]).unwrap_or(0);
        if self.tx_len < body_start + content_length {
            return; // body not yet fully received
        }

        // Extract the user prompt from the JSON body. We look for the last
        // `"content":"..."` string — naive but sufficient for a single-turn
        // chat request. If we can't find one, fall back to a generic reply.
        let body = &self.tx_buf[body_start..body_start + content_length];
        let mut prompt_buf = [0u8; 512];
        let prompt_len = extract_json_string(body, b"content", &mut prompt_buf).unwrap_or(0);
        let prompt = &prompt_buf[..prompt_len];

        // Build the synthetic chat completion. Shape matches the Anthropic
        // Messages API so the upstream parser is exercised against real-ish
        // bytes, not a degenerate one-field blob.
        self.rx_len = build_loopback_response(prompt, &mut self.rx_buf);
        self.rx_pos = 0;
        self.response_ready = true;
        self.request_count = self.request_count.wrapping_add(1);
    }
}

impl NetworkTransport for LoopbackTransport {
    fn connect(&mut self, host: &str, _port: u16) -> Result<(), TransportError> {
        if host.is_empty() || host.len() > MAX_HOST_LEN {
            return Err(TransportError::InvalidHost);
        }
        self.reset();
        self.connected = true;
        Ok(())
    }

    fn send(&mut self, data: &[u8]) -> Result<usize, TransportError> {
        if !self.connected {
            return Err(TransportError::InvalidState);
        }
        let space = LOOPBACK_BUF_SIZE - self.tx_len;
        if space == 0 {
            return Err(TransportError::NoBuffers);
        }
        let n = data.len().min(space);
        self.tx_buf[self.tx_len..self.tx_len + n].copy_from_slice(&data[..n]);
        self.tx_len += n;
        self.try_complete_request();
        Ok(n)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        if !self.connected {
            return Err(TransportError::InvalidState);
        }
        // If the request hasn't been fully delivered yet, give the caller a
        // chance to keep sending. Returning 0 here would look like EOF, so
        // we report it as a transient buffer-empty condition.
        if !self.response_ready {
            return Ok(0);
        }
        let remaining = self.rx_len - self.rx_pos;
        if remaining == 0 {
            return Ok(0); // legitimate end-of-stream
        }
        let n = buf.len().min(remaining);
        buf[..n].copy_from_slice(&self.rx_buf[self.rx_pos..self.rx_pos + n]);
        self.rx_pos += n;
        Ok(n)
    }

    fn close(&mut self) {
        self.connected = false;
        self.reset();
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn name(&self) -> &'static str {
        "loopback"
    }
}

// ============================================================================
// Helpers — tiny header / JSON utilities used by the loopback mock.
// Kept here (not in net_provider) because they're transport-private: the
// loopback peer is the *server* side; the provider is the *client* side.
// ============================================================================

/// Find the first occurrence of `needle` in `haystack`. Returns the byte
/// offset of the match, or `None` if absent.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    let mut i = 0;
    while i <= last {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a `Content-Length: N` header out of an HTTP header block. Returns
/// the decoded value, or `None` if not present / malformed.
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    // Case-insensitive header lookup, but we accept canonical casing only.
    // Real HTTP normalisation belongs in the client; the loopback is allowed
    // to be picky.
    let key = b"Content-Length:";
    let idx = find_subslice(headers, key)?;
    let mut p = idx + key.len();
    // Skip whitespace.
    while p < headers.len() && (headers[p] == b' ' || headers[p] == b'\t') {
        p += 1;
    }
    let mut n: usize = 0;
    let mut saw_digit = false;
    while p < headers.len() && headers[p].is_ascii_digit() {
        n = n.checked_mul(10)?.checked_add((headers[p] - b'0') as usize)?;
        p += 1;
        saw_digit = true;
    }
    if saw_digit { Some(n) } else { None }
}

/// Pull the value of a JSON string field named `key` out of `body`. Writes
/// the (unescaped) value into `out` and returns its byte length, or `None`
/// if the field wasn't found.
///
/// This is intentionally minimal — it handles `"key":"value"` with simple
/// backslash-escapes (`\"`, `\\`, `\n`, `\r`, `\t`). No unicode escapes, no
/// nested object lookup. Good enough for "find `content` in a flat message
/// object", which is all the loopback needs.
fn extract_json_string(body: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    // Build the `"key"` marker on the stack so we can find it directly.
    let mut marker = [0u8; 32];
    if key.len() + 2 > marker.len() {
        return None;
    }
    marker[0] = b'"';
    marker[1..1 + key.len()].copy_from_slice(key);
    marker[1 + key.len()] = b'"';
    let marker = &marker[..key.len() + 2];

    let mut search_start = 0;
    let mut last_value_end: Option<(usize, usize)> = None;

    // Scan all occurrences; remember the last one (so e.g. `"content":[{"type":"text","text":...,"content":"..."}]`
    // resolves to the innermost match — fine for our flat case).
    while let Some(rel) = find_subslice(&body[search_start..], marker) {
        let abs = search_start + rel;
        let mut p = abs + marker.len();
        // Expect `:` (with optional whitespace).
        while p < body.len() && (body[p] == b' ' || body[p] == b'\t') {
            p += 1;
        }
        if p >= body.len() || body[p] != b':' {
            search_start = abs + marker.len();
            continue;
        }
        p += 1;
        while p < body.len() && (body[p] == b' ' || body[p] == b'\t') {
            p += 1;
        }
        // Expect opening quote.
        if p >= body.len() || body[p] != b'"' {
            search_start = abs + marker.len();
            continue;
        }
        p += 1;
        let value_start = p;
        // Walk until unescaped closing quote.
        while p < body.len() && body[p] != b'"' {
            if body[p] == b'\\' && p + 1 < body.len() {
                p += 2;
            } else {
                p += 1;
            }
        }
        if p >= body.len() {
            return None; // unterminated string
        }
        last_value_end = Some((value_start, p));
        search_start = p + 1;
    }

    let (vs, ve) = last_value_end?;
    // Copy with simple escape decoding.
    let mut written = 0;
    let mut i = vs;
    while i < ve && written < out.len() {
        if body[i] == b'\\' && i + 1 < ve {
            let escaped = match body[i + 1] {
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
            out[written] = escaped;
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

/// Construct an Anthropic-shaped HTTP/1.1 response into `out`. Echoes (a
/// trimmed view of) the prompt back as the assistant's `content[0].text`,
/// so the demo can verify round-trip semantics without an actual LLM.
fn build_loopback_response(prompt: &[u8], out: &mut [u8]) -> usize {
    // First build the JSON body in a scratch buffer so we know Content-Length.
    let mut body = [0u8; 1024];
    let mut bp = 0;

    let head = b"{\"id\":\"msg_loopback\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"semantic-os-loopback\",\"content\":[{\"type\":\"text\",\"text\":\"[loopback] echo: ";
    bp += copy_into(&mut body[bp..], head);

    // Re-escape the prompt as we copy it into the JSON string. Keep it short
    // so we don't blow the body buffer.
    let prompt_cap = (body.len() - bp).saturating_sub(64); // leave room for the tail
    let mut i = 0;
    while i < prompt.len() && bp < body.len() && (bp + 2) < body.len() && i < prompt_cap {
        match prompt[i] {
            b'"' => {
                bp += copy_into(&mut body[bp..], b"\\\"");
            }
            b'\\' => {
                bp += copy_into(&mut body[bp..], b"\\\\");
            }
            b'\n' => {
                bp += copy_into(&mut body[bp..], b"\\n");
            }
            b'\r' => {
                bp += copy_into(&mut body[bp..], b"\\r");
            }
            0x20..=0x7E => {
                body[bp] = prompt[i];
                bp += 1;
            }
            _ => {
                // Drop non-printable bytes; keeps the mock JSON valid without
                // dragging in a full unicode escape table.
                body[bp] = b'?';
                bp += 1;
            }
        }
        i += 1;
    }

    let tail = b"\"}],\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}";
    bp += copy_into(&mut body[bp..], tail);
    let body_len = bp;

    // Now write the HTTP response header + body into `out`.
    let mut op = 0;
    op += copy_into(&mut out[op..], b"HTTP/1.1 200 OK\r\n");
    op += copy_into(&mut out[op..], b"Content-Type: application/json\r\n");
    op += copy_into(&mut out[op..], b"Content-Length: ");
    op += write_decimal(&mut out[op..], body_len as u64);
    op += copy_into(&mut out[op..], b"\r\n\r\n");
    op += copy_into(&mut out[op..], &body[..body_len]);
    op
}

/// Copy as many bytes from `src` into `dst` as fit. Returns the number copied.
fn copy_into(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

/// Format `n` as decimal into `dst`. Returns bytes written.
fn write_decimal(dst: &mut [u8], n: u64) -> usize {
    if n == 0 {
        if dst.is_empty() { return 0; }
        dst[0] = b'0';
        return 1;
    }
    // Build digits LSB-first, then reverse into dst.
    let mut tmp = [0u8; 20]; // u64::MAX fits in 20 digits
    let mut k = 0;
    let mut v = n;
    while v > 0 && k < tmp.len() {
        tmp[k] = b'0' + (v % 10) as u8;
        v /= 10;
        k += 1;
    }
    let n_written = k.min(dst.len());
    for i in 0..n_written {
        dst[i] = tmp[k - 1 - i];
    }
    n_written
}

// ============================================================================
// Global transport singleton
// ============================================================================

// Behind the yield-on-contention kernel mutex (was `static mut` +
// `&'static mut` under the false "syscalls are serialized" assumption —
// 2026-07-17 review, P1).
static GLOBAL_LOOPBACK: crate::sync::Mutex<LoopbackTransport> =
    crate::sync::Mutex::new(LoopbackTransport::new());

/// Lock the global loopback transport. Used by `NetworkLlmProvider` when
/// its configured transport kind is `Loopback`.
pub fn global_loopback_transport() -> crate::sync::MutexGuard<'static, LoopbackTransport> {
    GLOBAL_LOOPBACK.lock()
}

/// Initialise the transport subsystem. Currently a no-op (statics are
/// zero-initialised), but provided so `llm::init` can hold a slot for it.
pub fn init() {
    // Nothing to do today.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_round_trip() {
        let mut t = LoopbackTransport::new();
        t.connect("api.example.com", 443).unwrap();

        let req = b"POST /v1/messages HTTP/1.1\r\n\
                    Host: api.example.com\r\n\
                    Content-Type: application/json\r\n\
                    Content-Length: 47\r\n\
                    \r\n\
                    {\"model\":\"x\",\"content\":\"hello world\"}";
        // Content-Length above is the exact byte count of the body.
        let mut sent = 0;
        while sent < req.len() {
            sent += t.send(&req[sent..]).unwrap();
        }

        let mut out = [0u8; 1024];
        let mut total = 0;
        loop {
            let n = t.recv(&mut out[total..]).unwrap();
            if n == 0 { break; }
            total += n;
        }
        let response = core::str::from_utf8(&out[..total]).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("[loopback] echo: hello world"));
    }
}
