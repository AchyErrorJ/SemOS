//! M22 — Claude agent core.
//!
//! The reasoning loop for a native Claude Code-style agent: frame an Anthropic
//! Messages-API request (model + system + multi-turn messages + tool defs),
//! parse the response (assistant text and/or a `tool_use` block), run the
//! requested tool against the kernel (FS, shell), and feed the result back as
//! a `tool_result` for the next turn.
//!
//! This module is the *protocol + tools* — independent of the network. The
//! live HTTPS round-trip (Stage B) reuses the Phase-8 TLS transport; the
//! conversation loop + TUI (Stage C) drive this. It lives in the binary crate
//! because it leans on the global allocator for JSON string work and on the
//! kernel-side syscall surface for tools.

use alloc::format;
use alloc::string::String;

use kernel_core::syscall::{dispatch, numbers::*};

/// A conversation turn. `content` is already-formatted JSON for the content
/// array of one message (text block, or a tool_result block).
pub struct Message {
    pub role: &'static str, // "user" | "assistant"
    pub content_json: String,
}

impl Message {
    /// A plain user/assistant text message.
    pub fn text(role: &'static str, text: &str) -> Self {
        Self {
            role,
            content_json: format!("[{{\"type\":\"text\",\"text\":\"{}\"}}]", json_escape(text)),
        }
    }

    /// A user message carrying a tool_result for `tool_use_id`.
    pub fn tool_result(tool_use_id: &str, result: &str) -> Self {
        Self {
            role: "user",
            content_json: format!(
                "[{{\"type\":\"tool_result\",\"tool_use_id\":\"{}\",\"content\":\"{}\"}}]",
                json_escape(tool_use_id),
                json_escape(result)
            ),
        }
    }

    /// The assistant turn that issued a tool_use — must be replayed in the
    /// message history before the matching tool_result (Anthropic requirement).
    /// `input_json` is the raw input object, inserted verbatim.
    pub fn assistant_tool_use(tu: &ToolUse) -> Self {
        Self {
            role: "assistant",
            content_json: format!(
                "[{{\"type\":\"tool_use\",\"id\":\"{}\",\"name\":\"{}\",\"input\":{}}}]",
                json_escape(&tu.id),
                json_escape(&tu.name),
                tu.input_json
            ),
        }
    }
}

/// The Anthropic API key for outbound requests. Compile-time only via the
/// ANTHROPIC_KEY env var (so it lands in the gitignored binary, never in
/// source/git); empty if not set. The persistent mechanism is a future
/// `/etc/anthropic-api-key` read.
pub fn api_key() -> &'static str {
    option_env!("ANTHROPIC_KEY").unwrap_or("")
}

/// What the model asked for in a response: free text and/or one tool call.
#[derive(Default)]
pub struct AgentResponse {
    pub text: Option<String>,
    pub tool_use: Option<ToolUse>,
}

pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input_json: String, // the raw `input` object, e.g. {"path":"/x"}
}

/// Escape a string for embedding in a JSON string literal (the subset we
/// produce: quotes, backslashes, control chars → \uXXXX or \n etc.).
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The agent's tool definitions, as the JSON array the Messages API expects.
/// Kept small and string-literal for now (no per-tool schema builder).
pub fn tools_json() -> &'static str {
    concat!(
        "[",
        "{\"name\":\"read_file\",\"description\":\"Read a file's contents.\",",
        "\"input_schema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}},\"required\":[\"path\"]}},",
        "{\"name\":\"write_file\",\"description\":\"Write contents to a file (creates/overwrites).\",",
        "\"input_schema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"},\"content\":{\"type\":\"string\"}},\"required\":[\"path\",\"content\"]}},",
        "{\"name\":\"bash\",\"description\":\"Run a command in the sem-sh shell and return its stdout. Supports ; sequencing, | pipes, < > >> redirection, $VAR, and builtins: echo, pwd, cd, ls, cat, grep PATTERN [file], which, env, true, false, ps (tasks+tiers), free (heap), uptime, fetch URL (HTTP GET), ask QUESTION. External programs run from /bin.\",",
        "\"input_schema\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}},\"required\":[\"command\"]}}",
        "]"
    )
}

/// Build the JSON body for POST /v1/messages with the full conversation +
/// tool definitions. `system` may be empty.
pub fn build_request(model: &str, max_tokens: u32, system: &str, messages: &[Message]) -> String {
    let mut body = String::with_capacity(1024);
    body.push_str("{\"model\":\"");
    body.push_str(&json_escape(model));
    body.push_str("\",\"max_tokens\":");
    body.push_str(&format!("{}", max_tokens));
    if !system.is_empty() {
        body.push_str(",\"system\":\"");
        body.push_str(&json_escape(system));
        body.push('"');
    }
    body.push_str(",\"tools\":");
    body.push_str(tools_json());
    body.push_str(",\"messages\":[");
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str("{\"role\":\"");
        body.push_str(m.role);
        body.push_str("\",\"content\":");
        body.push_str(&m.content_json);
        body.push('}');
    }
    body.push_str("]}");
    body
}

/// Build a plain (tool-free) Messages request — for the shell `ask` builtin,
/// which wants a direct text answer, not a tool-use turn.
pub fn build_query(model: &str, max_tokens: u32, system: &str, messages: &[Message]) -> String {
    let mut body = String::with_capacity(512);
    body.push_str("{\"model\":\"");
    body.push_str(&json_escape(model));
    body.push_str("\",\"max_tokens\":");
    body.push_str(&format!("{}", max_tokens));
    if !system.is_empty() {
        body.push_str(",\"system\":\"");
        body.push_str(&json_escape(system));
        body.push('"');
    }
    body.push_str(",\"messages\":[");
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str("{\"role\":\"");
        body.push_str(m.role);
        body.push_str("\",\"content\":");
        body.push_str(&m.content_json);
        body.push('}');
    }
    body.push_str("]}");
    body
}

// ============================================================================
// Response parsing — a minimal scanner for the Anthropic response shape.
// ============================================================================
//
// We don't carry a full JSON parser; the response is machine-generated and
// regular, so we scan for the specific blocks we care about inside
// `"content":[ ... ]`: a `"type":"text"` block (→ assistant text) and a
// `"type":"tool_use"` block (→ id/name/input).

/// Find the value of a JSON string field `"key":"value"` starting from `from`.
/// Returns (value, index just past the closing quote). Handles `\"` escapes.
fn scan_string_field(s: &[u8], key: &str, from: usize) -> Option<(String, usize)> {
    let pat = format!("\"{}\"", key);
    let pat = pat.as_bytes();
    let mut i = from;
    while i + pat.len() < s.len() {
        if &s[i..i + pat.len()] == pat {
            // Skip to the colon, optional spaces, then the opening quote.
            let mut j = i + pat.len();
            while j < s.len() && (s[j] == b' ' || s[j] == b':') {
                j += 1;
            }
            if j >= s.len() || s[j] != b'"' {
                i += pat.len();
                continue;
            }
            j += 1; // past opening quote
            let mut out = String::new();
            while j < s.len() {
                match s[j] {
                    b'\\' if j + 1 < s.len() => {
                        let e = s[j + 1];
                        match e {
                            b'n' => out.push('\n'),
                            b'r' => out.push('\r'),
                            b't' => out.push('\t'),
                            b'"' => out.push('"'),
                            b'\\' => out.push('\\'),
                            b'/' => out.push('/'),
                            other => out.push(other as char),
                        }
                        j += 2;
                    }
                    b'"' => return Some((out, j + 1)),
                    c => {
                        out.push(c as char);
                        j += 1;
                    }
                }
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Find `"key":{ ... }` and return the raw object text (including braces).
fn scan_object_field(s: &[u8], key: &str, from: usize) -> Option<(String, usize)> {
    let pat = format!("\"{}\"", key);
    let pat = pat.as_bytes();
    let mut i = from;
    while i + pat.len() < s.len() {
        if &s[i..i + pat.len()] == pat {
            let mut j = i + pat.len();
            while j < s.len() && (s[j] == b' ' || s[j] == b':') {
                j += 1;
            }
            if j >= s.len() || s[j] != b'{' {
                i += pat.len();
                continue;
            }
            let start = j;
            let mut depth = 0i32;
            let mut in_str = false;
            let mut esc = false;
            while j < s.len() {
                let c = s[j];
                if in_str {
                    if esc {
                        esc = false;
                    } else if c == b'\\' {
                        esc = true;
                    } else if c == b'"' {
                        in_str = false;
                    }
                } else {
                    match c {
                        b'"' => in_str = true,
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                let obj = core::str::from_utf8(&s[start..=j]).ok()?.into();
                                return Some((obj, j + 1));
                            }
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Parse an Anthropic Messages-API response body. Extracts the first text
/// block and the first tool_use block (either may be absent).
pub fn parse_response(body: &str) -> AgentResponse {
    let s = body.as_bytes();
    let mut resp = AgentResponse::default();

    // tool_use: locate the block, then read its name/id/input after that point.
    if let Some(tu_at) = find(s, b"\"type\":\"tool_use\"") {
        let id = scan_string_field(s, "id", tu_at).map(|(v, _)| v);
        let name = scan_string_field(s, "name", tu_at).map(|(v, _)| v);
        let input = scan_object_field(s, "input", tu_at).map(|(v, _)| v);
        if let (Some(id), Some(name)) = (id, name) {
            resp.tool_use = Some(ToolUse {
                id,
                name,
                input_json: input.unwrap_or_else(|| String::from("{}")),
            });
        }
    }

    // text block.
    if let Some(t_at) = find(s, b"\"type\":\"text\"") {
        if let Some((txt, _)) = scan_string_field(s, "text", t_at) {
            resp.text = Some(txt);
        }
    }
    resp
}

/// First index of `needle` in `hay`, or None.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ============================================================================
// Live transport — POST the request to api.anthropic.com over the TLS stack.
// ============================================================================

/// Wrap a JSON body in a full HTTP/1.1 POST to /v1/messages. `api_key` is
/// added as `x-api-key` when non-empty (empty → expect a 401, which still
/// proves the round-trip). When `keep_alive`, request `Connection: keep-alive`
/// so the TLS connection survives the response and the next turn reuses it
/// (see `Session`); otherwise `Connection: close` (one-shot, read-until-EOF).
pub fn build_http_request(body: &str, api_key: &str, keep_alive: bool) -> String {
    let mut req = String::with_capacity(body.len() + 256);
    req.push_str("POST /v1/messages HTTP/1.1\r\n");
    req.push_str("Host: api.anthropic.com\r\n");
    req.push_str("User-Agent: semantic-os/0.1\r\n");
    req.push_str("Content-Type: application/json\r\n");
    req.push_str("anthropic-version: 2023-06-01\r\n");
    if !api_key.is_empty() {
        req.push_str("x-api-key: ");
        req.push_str(api_key);
        req.push_str("\r\n");
    }
    req.push_str(if keep_alive {
        "Connection: keep-alive\r\n"
    } else {
        "Connection: close\r\n"
    });
    req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    req.push_str(body);
    req
}

/// Is `resp` a complete HTTP response? On a keep-alive connection the server
/// does NOT close after the body, so we can't read-until-EOF — we must frame
/// each response exactly. Returns `true` once the body is fully present:
///   - chunked: `decode_chunked` succeeds (it returns `Ok` only at the
///     terminating zero-chunk, `Err` while truncated) — exact, not a substring
///     search for `0\r\n\r\n`.
///   - Content-Length: the body has reached the declared length.
/// Returns `false` (need more bytes) until then. A response with neither
/// framing header can only be delimited by close, so it never reports complete
/// here — the read loop falls back to EOF for that case.
fn response_complete(resp: &[u8]) -> bool {
    let sep = b"\r\n\r\n";
    let hdr_end = match find(resp, sep) {
        Some(i) => i,
        None => return false, // headers not fully received yet
    };
    let headers = &resp[..hdr_end];
    let body = &resp[hdr_end + sep.len()..];
    if kernel_core::net::http::is_chunked(headers) {
        let mut scratch = [0u8; 8192];
        kernel_core::net::http::decode_chunked(body, &mut scratch).is_ok()
    } else if let Some(clen) = kernel_core::net::http::content_length(headers) {
        body.len() >= clen
    } else {
        false
    }
}

/// Send `request` to api.anthropic.com:443 over the Phase-8 TLS transport and
/// read the response into `resp_out`. Resolves the host (DNS, hardcoded
/// fallback), connects (TLS 1.3 handshake + cert pin), sends, reads until the
/// server closes (we send `Connection: close`), and closes. Returns bytes read.
///
/// Wrapped in a small retry loop: the single-static-socket model occasionally
/// races the prior connection's teardown (SLIRP/smoltcp) on back-to-back
/// connects, so a fresh connection's first read can come back empty. Rather
/// than fail the whole agent turn on a transient, we tear down and reconnect.
/// Our requests are read-only/idempotent for this purpose, so a resend is safe.
pub fn send_over_tls(request: &[u8], resp_out: &mut [u8]) -> Result<usize, &'static str> {
    // 3 attempts: absorbs the common single-socket reconnect flake (empty first
    // read) without grinding — a *failing* attempt can sit on the 30 s recv idle
    // timeout, so a high cap turns a bad-luck streak into a multi-minute stall.
    // The residual flake (rare total failure) is the documented single-socket
    // limitation; the real fix is a socket pool / HTTP keep-alive.
    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = "no attempt";
    for attempt in 1..=MAX_ATTEMPTS {
        match send_over_tls_once(request, resp_out, attempt) {
            Ok(n) if n > 0 => return Ok(n),
            Ok(_) => {
                last_err = "empty response";
                crate::println!("    [tls] attempt {} got 0 bytes — retrying", attempt);
            }
            Err(e) => {
                last_err = e;
                crate::println!("    [tls] attempt {} failed ({}) — retrying", attempt, e);
            }
        }
        // Let the failed connection's teardown drain before reconnecting.
        for _ in 0..200 {
            kernel_core::net::poll();
        }
    }
    Err(last_err)
}

/// One connect → send → recv → close cycle. See `send_over_tls`.
fn send_over_tls_once(
    request: &[u8],
    resp_out: &mut [u8],
    attempt: u32,
) -> Result<usize, &'static str> {
    use kernel_core::llm::transport::NetworkTransport;
    use kernel_core::net::Ipv4Address;
    use kernel_core::tls::transport_tls::{configure_global, global_tls_transport};

    const SNI: &str = "api.anthropic.com";
    const PORT: u16 = 443;
    const FALLBACK: Ipv4Address = Ipv4Address::new(160, 79, 104, 10);

    let ip = kernel_core::net::resolve(SNI).unwrap_or(FALLBACK);
    crate::println!("    [tls] attempt {}: resolved, connecting...", attempt);
    configure_global(ip, PORT);

    unsafe {
        let t = global_tls_transport();
        if t.connect(SNI, PORT).is_err() {
            t.close();
            return Err("tls connect failed");
        }
        crate::println!("    [tls] connected, sending {} B...", request.len());
        // Send the whole request.
        let mut sent = 0;
        while sent < request.len() {
            match t.send(&request[sent..]) {
                Ok(0) => break,
                Ok(n) => sent += n,
                Err(_) => {
                    t.close();
                    return Err("send failed");
                }
            }
        }
        crate::println!("    [tls] sent {} B, receiving...", sent);
        // Read until EOF (server closes after the response) or buffer full.
        let mut got = 0;
        for _ in 0..64 {
            if got == resp_out.len() {
                break;
            }
            match t.recv(&mut resp_out[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(_) => break,
            }
        }
        crate::println!("    [tls] recv done: {} B", got);
        t.close();
        Ok(got)
    }
}

/// A keep-alive HTTP/1.1 session over ONE persistent TLS connection to
/// api.anthropic.com. A multi-turn agent conversation opens a single session
/// and issues several `request`s over it — one TLS handshake for the whole
/// conversation instead of a fresh connect per turn. That's the fix for the
/// single-socket reconnect flake: the flake is in *reconnecting*, so we stop
/// reconnecting between turns. Responses are framed exactly (chunked terminator
/// / Content-Length via `response_complete`) so the connection stays open and
/// we never wait out a trailing idle timeout. If the peer drops the connection
/// mid-conversation, `request` transparently reconnects once and resends.
pub struct Session {
    connected: bool,
}

impl Session {
    const SNI: &'static str = "api.anthropic.com";
    const PORT: u16 = 443;

    /// Open the session: resolve + TLS handshake, retried a few times to absorb
    /// the rotating-port reconnect flake on the very first connect.
    pub fn open() -> Result<Self, &'static str> {
        let mut s = Session { connected: false };
        for attempt in 1..=3 {
            crate::println!("    [tls] session connect attempt {}...", attempt);
            if s.connect() {
                crate::println!("    [tls] session established (keep-alive)");
                return Ok(s);
            }
            for _ in 0..200 {
                kernel_core::net::poll();
            }
        }
        Err("session connect failed")
    }

    fn connect(&mut self) -> bool {
        use kernel_core::llm::transport::NetworkTransport;
        use kernel_core::net::Ipv4Address;
        use kernel_core::tls::transport_tls::{configure_global, global_tls_transport};
        const FALLBACK: Ipv4Address = Ipv4Address::new(160, 79, 104, 10);
        let ip = kernel_core::net::resolve(Self::SNI).unwrap_or(FALLBACK);
        configure_global(ip, Self::PORT);
        unsafe {
            let t = global_tls_transport();
            if t.connect(Self::SNI, Self::PORT).is_err() {
                t.close();
                self.connected = false;
                return false;
            }
        }
        self.connected = true;
        true
    }

    fn force_close(&mut self) {
        use kernel_core::llm::transport::NetworkTransport;
        use kernel_core::tls::transport_tls::global_tls_transport;
        unsafe {
            global_tls_transport().close();
        }
        self.connected = false;
        for _ in 0..200 {
            kernel_core::net::poll();
        }
    }

    /// Send one request and read one complete response over the live
    /// connection. On a transport error or a dropped connection, reconnect once
    /// and resend (our requests are idempotent for this purpose).
    pub fn request(&mut self, request: &[u8], resp_out: &mut [u8]) -> Result<usize, &'static str> {
        let mut last_err = "no attempt";
        // Up to 4 tries: the *initial* connect of a session is still a
        // reconnect-within-the-boot and can hit the single-socket flake (a fast
        // recv error, not a slow timeout), so retrying is cheap. Once a turn
        // succeeds, subsequent turns reuse the live connection with no reconnect.
        for attempt in 1..=4 {
            if !self.connected && !self.connect() {
                last_err = "reconnect failed";
                for _ in 0..200 {
                    kernel_core::net::poll();
                }
                continue;
            }
            match self.send_and_read(request, resp_out) {
                Ok(n) if n > 0 => return Ok(n),
                Ok(_) => last_err = "empty response",
                Err(e) => last_err = e,
            }
            crate::println!("    [tls] request attempt {} failed ({}) — reconnecting", attempt, last_err);
            self.force_close();
        }
        Err(last_err)
    }

    fn send_and_read(&mut self, request: &[u8], resp_out: &mut [u8]) -> Result<usize, &'static str> {
        use kernel_core::llm::transport::NetworkTransport;
        use kernel_core::tls::transport_tls::global_tls_transport;
        unsafe {
            let t = global_tls_transport();
            // Send the whole request on the live connection.
            let mut sent = 0;
            while sent < request.len() {
                match t.send(&request[sent..]) {
                    Ok(0) => return Err("send returned 0"),
                    Ok(n) => sent += n,
                    Err(_) => {
                        self.connected = false;
                        return Err("send failed");
                    }
                }
            }
            crate::println!("    [tls] sent {} B (keep-alive), receiving framed...", sent);
            // Read exactly one framed response; the connection stays OPEN. We
            // return the instant the body is complete — no trailing recv that
            // would block on the idle timeout (the read-until-EOF path's cost).
            let mut got = 0;
            for _ in 0..256 {
                if response_complete(&resp_out[..got]) {
                    crate::println!("    [tls] framed response: {} B (conn kept alive)", got);
                    return Ok(got);
                }
                if got == resp_out.len() {
                    return Ok(got); // buffer full — best effort
                }
                match t.recv(&mut resp_out[got..]) {
                    Ok(0) => {
                        // Peer closed. Complete only if framing says so.
                        self.connected = false;
                        return if response_complete(&resp_out[..got]) {
                            Ok(got)
                        } else {
                            Err("eof before complete")
                        };
                    }
                    Ok(n) => got += n,
                    Err(_) => {
                        self.connected = false;
                        return Err("recv failed");
                    }
                }
            }
            Err("response did not complete")
        }
    }

    /// Tear the session down (close_notify + socket release). Idempotent.
    pub fn close(&mut self) {
        if self.connected {
            self.force_close();
        }
    }
}

/// Copy `s` into `out` (truncating to fit) and return the byte count.
fn write_out(out: &mut [u8], s: &str) -> usize {
    let b = s.as_bytes();
    let n = b.len().min(out.len());
    out[..n].copy_from_slice(&b[..n]);
    n
}

/// The shell `ask` builtin's engine: send `prompt` to Claude as a single
/// tool-free turn over one keep-alive TLS connection, and write the plain-text
/// answer into `out` (returning its length). This is the OS's LLM, reachable
/// from the shell — `ask "question"` or `cmd | ask "..."`. Degrades to a clear
/// message (not a hang) when there's no key or the network is unavailable.
///
/// NOTE (security): the prompt is sent verbatim to the external API. Whatever
/// the caller pipes in is disclosed — tier-aware redaction (so the LLM can't be
/// fed Secret-tier content) is the planned guard, not yet enforced here.
pub fn ask(prompt: &str, out: &mut [u8]) -> usize {
    let key = api_key();
    if key.is_empty() {
        return write_out(out, "ask: no ANTHROPIC_KEY configured in this build");
    }
    if prompt.is_empty() {
        return write_out(out, "ask: empty prompt");
    }

    let model = "claude-haiku-4-5-20251001";
    let sys = "You are a terse assistant embedded in the Semantic OS shell. Answer in one or two plain sentences, no preamble.";
    let msgs = [Message::text("user", prompt)];
    let req = build_query(model, 512, sys, &msgs);
    let http = build_http_request(&req, key, true);

    let mut session = match Session::open() {
        Ok(s) => s,
        Err(e) => return write_out(out, &format!("ask: connection failed ({})", e)),
    };
    let mut resp = [0u8; 8192];
    let n = match session.request(http.as_bytes(), &mut resp) {
        Ok(n) => n,
        Err(e) => {
            session.close();
            return write_out(out, &format!("ask: request failed ({})", e));
        }
    };
    session.close();

    let mut body = [0u8; 8192];
    let bn = decode_body(&resp[..n], &mut body);
    let parsed = parse_response(&String::from_utf8_lossy(&body[..bn]));
    match parsed.text {
        Some(t) if !t.trim().is_empty() => write_out(out, t.trim()),
        _ => {
            let status = http_status(&resp[..n]).unwrap_or(0);
            write_out(out, &format!("ask: no answer (HTTP {})", status))
        }
    }
}

/// SYS_AGENT — the interactive split-pane agent terminal launched by the
/// shell's `agent` builtin. Runs a chat loop over the framebuffer TUI: each
/// line the user types is sent to Claude via the one-shot `ask` path and the
/// reply lands in the conversation pane, until they type `exit`/`quit`. Reuses
/// the tested `ask` query, so each turn is independent for now (multi-turn
/// memory + the read_file/bash tools in the loop are a follow-up). Without a
/// baked-in key it still runs — you can see the UI and type — and reports that
/// chatting needs a key. Headless (no framebuffer) → nothing to show, returns 1.
///
/// While this runs, the shell is blocked in the syscall and the interactive
/// wait loop must not pump the HID ring (it would race our `read_line` pump),
/// so we hold `AGENT_TUI_ACTIVE` for the duration and clear the screen on exit.
pub fn run_interactive(_flags: u64) -> u64 {
    use crate::tui::Tui;
    use core::sync::atomic::Ordering;

    // Clear the boot console first so the TUI sits on a clean screen instead of
    // overlaying leftover demo/shell scrollback.
    crate::framebuffer::clear();
    let mut tui = match Tui::new("claude-haiku-4-5") {
        Some(t) => t,
        None => return 1, // headless — no framebuffer to draw the TUI
    };
    tui.push_assistant("Agent terminal — type a question, Enter to send. 'exit' returns to the shell.");

    let have_key = !api_key().is_empty();
    if !have_key {
        tui.push_error("(no ANTHROPIC_KEY in this build — you can type, but chatting needs a key)");
    }

    crate::AGENT_TUI_ACTIVE.store(true, Ordering::Relaxed);

    let mut out = [0u8; 8192];
    loop {
        tui.set_status("ready");
        let mut qbuf = [0u8; 512];
        let n = tui.read_line(&mut qbuf);
        let q = core::str::from_utf8(&qbuf[..n]).unwrap_or("").trim();
        if q.is_empty() {
            continue;
        }
        if q.eq_ignore_ascii_case("exit") || q.eq_ignore_ascii_case("quit") {
            break;
        }
        tui.push_user(q);
        if !have_key {
            tui.push_error("can't reach Claude: no ANTHROPIC_KEY baked into this build");
            continue;
        }
        tui.set_status("thinking");
        let m = ask(q, &mut out).min(out.len());
        let answer = core::str::from_utf8(&out[..m]).unwrap_or("(decode error)");
        tui.push_assistant(answer);
    }

    crate::AGENT_TUI_ACTIVE.store(false, Ordering::Relaxed);
    // Tear down the overlay so the shell prompt resumes on a clean screen.
    crate::framebuffer::clear();
    0
}

/// Parse the numeric status code out of an HTTP response (`HTTP/1.1 NNN ...`).
pub fn http_status(resp: &[u8]) -> Option<u32> {
    if resp.len() < 12 || &resp[..5] != b"HTTP/" {
        return None;
    }
    let sp = resp.iter().position(|&b| b == b' ')?;
    let mut code = 0u32;
    let mut i = sp + 1;
    while i < resp.len() && resp[i].is_ascii_digit() {
        code = code * 10 + (resp[i] - b'0') as u32;
        i += 1;
    }
    if code > 0 {
        Some(code)
    } else {
        None
    }
}

/// Return the body slice of an HTTP response (everything after the blank line).
pub fn http_body(resp: &[u8]) -> &[u8] {
    let sep = b"\r\n\r\n";
    if let Some(i) = find(resp, sep) {
        &resp[i + sep.len()..]
    } else {
        &[]
    }
}

/// Decode the HTTP body into `out`, de-chunking if the response used
/// Transfer-Encoding: chunked (real Claude responses do). Returns the decoded
/// length. For a Content-Length body it's a straight copy.
pub fn decode_body(resp: &[u8], out: &mut [u8]) -> usize {
    let sep = b"\r\n\r\n";
    let (headers, body) = match find(resp, sep) {
        Some(i) => (&resp[..i], &resp[i + sep.len()..]),
        None => return 0,
    };
    if kernel_core::net::http::is_chunked(headers) {
        kernel_core::net::http::decode_chunked(body, out).unwrap_or(0)
    } else {
        let n = body.len().min(out.len());
        out[..n].copy_from_slice(&body[..n]);
        n
    }
}

// ============================================================================
// Tool dispatch — run a tool against the kernel, return the tool_result text.
// ============================================================================

/// Run the named tool with the given raw `input` JSON object. Returns the
/// textual result to feed back as a tool_result.
pub fn run_tool(name: &str, input_json: &str) -> String {
    match name {
        "read_file" => match field_str(input_json, "path") {
            Some(path) => match read_file(&path) {
                Ok(s) => s,
                Err(e) => format!("error: {}", e),
            },
            None => String::from("error: missing 'path'"),
        },
        "write_file" => {
            let path = field_str(input_json, "path");
            let content = field_str(input_json, "content");
            match (path, content) {
                (Some(p), Some(c)) => match write_file(&p, c.as_bytes()) {
                    Ok(()) => format!("wrote {} bytes to {}", c.len(), p),
                    Err(e) => format!("error: {}", e),
                },
                _ => String::from("error: missing 'path' or 'content'"),
            }
        }
        "bash" => match field_str(input_json, "command") {
            Some(cmd) => run_bash(&cmd),
            None => String::from("error: missing 'command'"),
        },
        other => format!("error: unknown tool '{}'", other),
    }
}

/// Pull a string field out of a small JSON object (the tool input).
fn field_str(obj_json: &str, key: &str) -> Option<String> {
    scan_string_field(obj_json.as_bytes(), key, 0).map(|(v, _)| v)
}

/// The agent's `bash` tool: run `cmd` through the real M20 shell
/// (`/bin/sem-sh -c "<cmd>"`) and return its captured stdout. This is a *real*
/// shell — builtins, `;` sequencing, `|` pipes, `</>/>>` redirection, external
/// ELF exec — not a kernel-side reimplementation. The agent therefore gets the
/// OS's actual command surface, and anything we add to sem-sh (e.g. `grep`)
/// becomes available to Claude for free.
///
/// Mechanics (we run in kernel context, on the `init_loader` task): pin the
/// kernel task to the current scheduler slot so per-process FD ops resolve to
/// us; open a pipe and dup its write end onto fd 1 so the spawned shell
/// inherits it as stdout; spawn with argv `["/bin/sem-sh","-c",cmd]`; then poll
/// the child's slot while draining the pipe *interleaved* (the pipe is 4 KiB —
/// draining as we go means a chatty command can't fill it and deadlock).
fn run_bash(cmd: &str) -> String {
    use alloc::vec::Vec;
    use kernel_core::scheduler::{self, TaskState};
    use kernel_core::syscall::SpawnArgs;

    // argv blob: [count: u32][len: u32][bytes]... for ["/bin/sem-sh","-c",cmd].
    let items: [&str; 3] = ["/bin/sem-sh", "-c", cmd];
    let mut blob: Vec<u8> = Vec::new();
    blob.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for it in items {
        blob.extend_from_slice(&(it.len() as u32).to_le_bytes());
        blob.extend_from_slice(it.as_bytes());
    }

    let saved = kernel_core::process::kernel_task_id();
    kernel_core::process::set_kernel_task_id(Some(scheduler::current_task_index()));

    let mut fds = [0u64; 2];
    if dispatch(SYS_PIPE, fds.as_mut_ptr() as u64, 0, 0, 0) != 0 {
        kernel_core::process::set_kernel_task_id(saved);
        return String::from("error: pipe failed");
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    dispatch(SYS_DUP2, write_fd, 1, 0, 0);

    let path = "/bin/sem-sh";
    let spawn_args = SpawnArgs {
        argv_blob_ptr: blob.as_ptr() as u64,
        argv_blob_len: blob.len() as u32,
        envp_blob_ptr: 0,
        envp_blob_len: 0,
    };
    let pid = dispatch(
        SYS_SPAWN,
        path.as_ptr() as u64,
        path.len() as u64,
        // Sandbox the agent's shell at tier 0 (Public). The LLM is the
        // least-trusted component in the 4-tier model, so its shell runs with
        // the lowest clearance: SYS_OPEN's tier check then denies it any
        // Internal/Sensitive/Secret file — it can neither read secrets nor
        // modify protected (higher-tier) state, only touch Public files.
        0,
        &spawn_args as *const SpawnArgs as u64,
    );
    // Restore our own stdout; the child already inherited the pipe at spawn.
    dispatch(SYS_DUP2, 0, 1, 0, 0);

    if pid == u64::MAX {
        dispatch(SYS_CLOSE, read_fd, 0, 0, 0);
        dispatch(SYS_CLOSE, write_fd, 0, 0, 0);
        kernel_core::process::set_kernel_task_id(saved);
        return String::from("error: failed to spawn /bin/sem-sh");
    }

    let child = kernel_core::process::ProcessId(pid as u32);
    let cs = kernel_core::process::get(child).and_then(|p| p.task_id);

    let mut out = String::new();
    let mut buf = [0u8; 512];
    let mut polled = 0u64;
    const OUT_CAP: usize = 8192;
    loop {
        // The scheduler ran the child during the last sleep, so the current
        // slot is ours again — re-pin so FD ops keep resolving to us.
        kernel_core::process::set_kernel_task_id(Some(scheduler::current_task_index()));

        let r = dispatch(SYS_READ, read_fd, buf.as_mut_ptr() as u64, buf.len() as u64, 0);
        let got = if r == u64::MAX || r == u64::MAX - 1 { 0 } else { (r as usize).min(buf.len()) };
        if got > 0 {
            out.push_str(&String::from_utf8_lossy(&buf[..got]));
            if out.len() >= OUT_CAP {
                break;
            }
            continue; // drain greedily before sleeping
        }

        let exited = match cs {
            Some(slot) => scheduler::task_state(slot) == TaskState::Exited,
            None => true,
        };
        if exited {
            // Final drain of whatever is still buffered after exit.
            loop {
                let r2 = dispatch(SYS_READ, read_fd, buf.as_mut_ptr() as u64, buf.len() as u64, 0);
                let g2 = if r2 == u64::MAX || r2 == u64::MAX - 1 { 0 } else { (r2 as usize).min(buf.len()) };
                if g2 == 0 || out.len() >= OUT_CAP {
                    break;
                }
                out.push_str(&String::from_utf8_lossy(&buf[..g2]));
            }
            break;
        }

        let _ = dispatch(SYS_SLEEP, 1, 0, 0, 0);
        polled += 1;
        if polled > 2000 {
            break; // safety: never hang the boot on a stuck child
        }
    }

    dispatch(SYS_CLOSE, read_fd, 0, 0, 0);
    dispatch(SYS_CLOSE, write_fd, 0, 0, 0);

    // Reap the child immediately so its address-space PT frames return to the
    // pool now, not whenever some future spawn happens to reuse the slot. We
    // are the child's waiter and it has exited, so this is safe — and it's what
    // keeps a session that runs many shell commands (this tool, in a loop)
    // sustainable instead of exhausting the frame pool after ~MAX_TASKS spawns.
    if let Some(slot) = cs {
        if scheduler::task_state(slot) == TaskState::Exited {
            kernel_core::platform::get().reap_slot(slot);
        }
    }

    kernel_core::process::set_kernel_task_id(saved);

    if out.is_empty() {
        String::from("(no output)")
    } else {
        out
    }
}

/// Read a path-namespace file via SYS_OPEN + SYS_FREAD.
fn read_file(path: &str) -> Result<String, &'static str> {
    let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0);
    if fd == u64::MAX {
        return Err("open failed");
    }
    let mut out = String::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = dispatch(SYS_FREAD, fd, buf.as_mut_ptr() as u64, buf.len() as u64, 0);
        if n == u64::MAX || n == 0 {
            break;
        }
        let n = (n as usize).min(buf.len());
        out.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    dispatch(SYS_CLOSE, fd, 0, 0, 0);
    Ok(out)
}

/// Write a file (create/overwrite) via SYS_OPEN(CREATE) + SYS_FWRITE.
fn write_file(path: &str, data: &[u8]) -> Result<(), &'static str> {
    let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 1 /*CREATE*/, 0);
    if fd == u64::MAX {
        return Err("open failed");
    }
    // Truncate then write from offset 0.
    dispatch(SYS_TRUNCATE, path.as_ptr() as u64, path.len() as u64, 0, 0);
    let w = dispatch(SYS_FWRITE, fd, data.as_ptr() as u64, data.len() as u64, 0);
    dispatch(SYS_CLOSE, fd, 0, 0, 0);
    if w == u64::MAX {
        Err("write failed")
    } else {
        Ok(())
    }
}
