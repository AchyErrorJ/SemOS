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

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use kernel_core::syscall::{dispatch, numbers::*, StatX};

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

/// First non-empty of the two compile-time env values, else `default`.
///
/// Critically treats a **set-but-empty** env var as absent. The dev shell
/// exports `ANTHROPIC_BASE_URL=` / `ANTHROPIC_MODEL=` (empty) for Claude Code,
/// and a plain `.or().unwrap_or()` chain would take that `Some("")` and
/// shadow the default — which silently produced an empty host/model in the
/// first bake (caught by the boot endpoint-probe on 2026-07-22).
fn first_nonempty(
    primary: Option<&'static str>,
    secondary: Option<&'static str>,
    default: &'static str,
) -> &'static str {
    if let Some(s) = primary {
        if !s.is_empty() {
            return s;
        }
    }
    if let Some(s) = secondary {
        if !s.is_empty() {
            return s;
        }
    }
    default
}

/// The API key for outbound requests. Compile-time only (so it lands in the
/// gitignored binary, never in source/git); empty if not set. Reads the
/// exo-agent-style `KIMI_API_KEY` first, falling back to the older
/// `ANTHROPIC_KEY` so pre-existing build scripts keep working. The persistent
/// (runtime) mechanism is a future `/etc/agent.conf` read.
pub fn api_key() -> &'static str {
    first_nonempty(option_env!("KIMI_API_KEY"), option_env!("ANTHROPIC_KEY"), "")
}

/// Base URL of the (Anthropic-Messages-compatible) endpoint. Compile-time only
/// (mirrors `api_key()` — lands only in the gitignored binary), so a build can
/// target any provider that speaks the Anthropic Messages wire format. Reads
/// `KIMI_BASE_URL` first, then `ANTHROPIC_BASE_URL`; defaults to the Kimi
/// Coding endpoint (matches Orchestre's `kimi_worker.py`).
///
/// The TLS layer needs no changes to support an alternate provider as long as
/// it's fronted by the same pinned intermediate (`tls::spki_pin`, GTS WE1) —
/// confirmed for api.kimi.com 2026-07-22 (identical WE1 SPKI hash to
/// api.anthropic.com). A provider behind a different CA would need a second
/// pin added there.
fn base_url() -> &'static str {
    first_nonempty(
        option_env!("KIMI_BASE_URL"),
        option_env!("ANTHROPIC_BASE_URL"),
        "https://api.kimi.com/coding",
    )
}

/// Host portion of [`base_url`] — used as both the DNS/TCP target and the TLS
/// SNI.
pub fn endpoint_host() -> &'static str {
    let u = base_url();
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    match u.find('/') {
        Some(i) => &u[..i],
        None => u,
    }
}

/// Path prefix from [`base_url`] (empty for the plain Anthropic API), with
/// any trailing slash trimmed so it can be concatenated with `/v1/messages`.
fn endpoint_path_prefix() -> &'static str {
    let u = base_url();
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    match u.find('/') {
        Some(i) => u[i..].trim_end_matches('/'),
        None => "",
    }
}

/// Full Messages-API path for outbound requests, e.g. `/v1/messages` or
/// `/coding/v1/messages`.
pub fn endpoint_path() -> String {
    format!("{}/v1/messages", endpoint_path_prefix())
}

/// Model identifier for outbound requests. Compile-time only (mirrors
/// `api_key()`). Reads `KIMI_MODEL` first, then `ANTHROPIC_MODEL`; defaults to
/// `kimi-k2.7` (matches Orchestre's `kimi_worker.py`).
pub fn model_name() -> &'static str {
    first_nonempty(
        option_env!("KIMI_MODEL"),
        option_env!("ANTHROPIC_MODEL"),
        "kimi-k2.7",
    )
}

/// Resolve the configured endpoint host to an IP. DNS first; if that fails we
/// only have a hardcoded fallback for the real Anthropic API (its IP was
/// captured for the SLIRP-era demos) — a custom provider must resolve via DNS,
/// so this returns `None` there rather than connecting to the wrong host.
fn resolve_endpoint() -> Option<kernel_core::net::Ipv4Address> {
    use kernel_core::net::Ipv4Address;
    let host = endpoint_host();
    if let Some(ip) = kernel_core::net::resolve(host) {
        return Some(ip);
    }
    if host == "api.anthropic.com" {
        return Some(Ipv4Address::new(160, 79, 104, 10));
    }
    None
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
        "{\"name\":\"write_file\",\"description\":\"Write contents to a file (creates/overwrites by default). Keep each call under ~2000 chars of content; for bigger files pass \\\"append\\\":true to append a chunk to the existing end instead of truncating.\",",
        "\"input_schema\":{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"},\"content\":{\"type\":\"string\"},\"append\":{\"type\":\"boolean\"}},\"required\":[\"path\",\"content\"]}},",
        "{\"name\":\"bash\",\"description\":\"Run a command in the sem-sh shell and return its stdout. Supports ; sequencing, | pipes, < > >> redirection, $VAR, and builtins: echo, pwd, cd, ls, cat, grep PATTERN [file], which, env, true, false, ps (tasks+tiers), free (heap), uptime, netinfo (network/NIC diagnostics), fetch URL (HTTP GET), ask QUESTION. External programs run from /bin (PATH also includes /apps), so an ELF the agent compiles into /apps runs by name.\",",
        "\"input_schema\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}},\"required\":[\"command\"]}},",
        "{\"name\":\"compile\",\"description\":\"Compile a Rust source file to a runnable ELF with the on-device compiler (semos-rustc, Cranelift backend) and return the compiler's output. Provide source and out; out defaults to source with .rs replaced by .elf. The result is a no_std/no_main SemOS program: write it to /apps/<name>.elf and it runs from the shell as <name>.\",",
        "\"input_schema\":{\"type\":\"object\",\"properties\":{\"source\":{\"type\":\"string\"},\"out\":{\"type\":\"string\"}},\"required\":[\"source\"]}}",
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
    // Disable extended thinking: reasoning models (e.g. kimi-k2.7) otherwise
    // return a huge `"type":"thinking"` block (with a multi-KB signature) that
    // overflows the fixed response buffer and can burn the whole token budget
    // before any `"type":"text"` block is emitted — surfacing as "no answer".
    // Valid for the Anthropic fallback endpoint too.
    body.push_str(",\"thinking\":{\"type\":\"disabled\"}");
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
    // See build_request: disable extended thinking so reasoning models return a
    // compact `"type":"text"` answer that fits the response buffer.
    body.push_str(",\"thinking\":{\"type\":\"disabled\"}");
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
    req.push_str("POST ");
    req.push_str(&endpoint_path());
    req.push_str(" HTTP/1.1\r\n");
    req.push_str("Host: ");
    req.push_str(endpoint_host());
    req.push_str("\r\n");
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
///   - chunked: scan chunk frames until the terminating zero-chunk — exact,
///     not a substring search for `0\r\n\r\n`, and no large scratch buffer.
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
        kernel_core::net::http::chunked_complete(body)
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
    use kernel_core::tls::transport_tls::{configure_global, global_tls_transport};

    let sni = endpoint_host();
    const PORT: u16 = 443;

    let ip = match resolve_endpoint() {
        Some(ip) => ip,
        None => return Err("dns resolve failed (no fallback for custom host)"),
    };
    crate::println!("    [tls] attempt {}: resolved, connecting...", attempt);
    configure_global(ip, PORT);

    unsafe {
        let mut t = global_tls_transport();
        if t.connect(sni, PORT).is_err() {
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
        use kernel_core::tls::transport_tls::{configure_global, global_tls_transport};
        let sni = endpoint_host();
        let ip = match resolve_endpoint() {
            Some(ip) => ip,
            None => {
                self.connected = false;
                return false;
            }
        };
        configure_global(ip, Self::PORT);
        unsafe {
            let mut t = global_tls_transport();
            if t.connect(sni, Self::PORT).is_err() {
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
            let mut t = global_tls_transport();
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

    let model = model_name();
    let sys = "You are a terse assistant embedded in the Semantic OS shell. Answer in one or two plain sentences, no preamble.";
    let msgs = [Message::text("user", prompt)];
    let req = build_query(model, 512, sys, &msgs);
    let http = build_http_request(&req, key, true);

    let mut session = match Session::open() {
        Ok(s) => s,
        Err(e) => return write_out(out, &format!("ask: connection failed ({})", e)),
    };
    // Heap buffers, not stack buffers: `ask()` is nested under the fullscreen
    // TUI path, so keeping the HTTP response and body scratch arrays on
    // the stack reintroduced the layout-sensitive overflow class called out in
    // the 2026-07-17 review. The request/parse strings are already heap-backed;
    // put the fixed-size transport buffers there too.
    let mut resp = Box::new([0u8; RESP_CAP]);
    let n = match session.request(http.as_bytes(), &mut resp[..]) {
        Ok(n) => n,
        Err(e) => {
            session.close();
            return write_out(out, &format!("ask: request failed ({})", e));
        }
    };
    session.close();

    let mut body = Box::new([0u8; RESP_CAP]);
    let bn = decode_body(&resp[..n], &mut body[..]);
    let parsed = parse_response(&String::from_utf8_lossy(&body[..bn]));
    match parsed.text {
        Some(t) if !t.trim().is_empty() => write_out(out, t.trim()),
        _ => {
            let status = http_status(&resp[..n]).unwrap_or(0);
            write_out(out, &format!("ask: no answer (HTTP {})", status))
        }
    }
}

/// HTTP response + decoded-body buffer size for `ask` and `run_agent`.
/// 8 KiB was the tetris killer: a whole-game write_file call is a >8 KiB
/// response, the buffer truncated the JSON mid-tool_use, and the loop bailed
/// with "no answer". 64 KiB plus the system prompt's chunking rule (append
/// mode for big files) keeps large generations alive. Heap-allocated at both
/// call sites.
const RESP_CAP: usize = 65536;

/// The system prompt for the agentic loop. Tells the model it drives a real
/// bare-metal shell through tools, and to finish with a short plain-text answer
/// once the work is done (a text turn with no tool_use ends the loop).
///
/// The semos-rustc paragraph is load-bearing: the on-device compiler is a
/// minimal Cranelift pipeline, and code outside that shape compiles to
/// panics/ICEs (the "agent writes tetris from scratch" failure). The chunking
/// paragraph exists because the whole file content rides in ONE tool-call
/// response — a >RESP_CAP response truncates and kills the turn.
const AGENT_SYSTEM: &str = "You are the resident agent of Semantic OS, a bare-metal \
Rust operating system. You act by calling tools: read_file, write_file, bash (the \
sem-sh shell), and compile. Work in small concrete steps — inspect with \
bash/read_file before you change anything, and verify your work after. Paths are \
absolute (e.g. /apps/foo). You can extend the OS: to add a program, write a \
no_std/no_main Rust source (e.g. write_file /apps/foo.rs), compile it (compile \
source=/apps/foo.rs), then run it from the shell by name (bash foo) — semos-rustc \
emits a runnable ELF. \
semos-rustc is a MINIMAL compiler: integer math only, and no '/' or '%' operators \
(use power-of-two shifts and masks instead); no indexing that could panic (fixed \
arrays with mask-wrapped indices, no slice[i] on runtime i); no inline asm; no \
closures; no String/Vec/heap — 'static mut' state only. A complete known-good \
game written in exactly this shape is at /templates/snake.rs — ALWAYS read it \
first when writing or modifying a program, and stay in its shape. \
write_file sends the whole 'content' in one response, so keep each write under \
~2000 characters: for a bigger file, write the first chunk normally, then send \
the rest in chunks with \"append\":true. When the task is complete, reply with \
a short plain-text summary and no further tool call; that ends your turn.";

/// The loop runs until the model finishes (a text turn with no tool_use). There
/// is no artificial turn cap: tools are tier-clamped so an agent that loops only
/// spends model tokens, it can't escalate. (A manual keypress-interrupt is a
/// planned follow-up alongside the TUI scrollback work.)

/// A sink for live progress from `run_agent`, so the same loop core can render
/// into the framebuffer TUI (interactive terminal) or, later, a byte buffer /
/// serial log (a headless `agent "goal"` builtin). Every hook is optional.
pub trait AgentReporter {
    /// The model's free-text thinking/narration for a turn (may be empty).
    fn on_text(&mut self, _text: &str) {}
    /// The model requested a tool call, about to run.
    fn on_tool_call(&mut self, _name: &str, _input_json: &str) {}
    /// The tool returned this result (fed back to the model next turn).
    fn on_tool_result(&mut self, _result: &str) {}
    /// A transient status word for a spinner/status line ("thinking", "read_file").
    fn on_status(&mut self, _status: &str) {}
    /// A loop-level error (connection/parse/turn-cap) — terminal for this run.
    fn on_error(&mut self, _msg: &str) {}
}

/// A no-op reporter for callers that only want the final return value.
pub struct NullReporter;
impl AgentReporter for NullReporter {}

/// Renders `run_agent` progress into the split-pane framebuffer TUI: user and
/// assistant text land in the left conversation pane, tool calls/results and
/// status words stream down the right activity pane. Borrows the `Tui` for
/// the duration of one `run_agent` call.
struct TuiReporter<'a> {
    tui: &'a mut crate::tui::Tui,
}
impl<'a> AgentReporter for TuiReporter<'a> {
    fn on_text(&mut self, text: &str) {
        self.tui.push_assistant(text);
    }
    fn on_tool_call(&mut self, name: &str, input_json: &str) {
        self.tui.push_tool_call(name, input_json);
    }
    fn on_tool_result(&mut self, result: &str) {
        self.tui.push_tool_result(result);
    }
    fn on_status(&mut self, status: &str) {
        self.tui.set_status(status);
        self.tui.push_activity_status(status);
    }
    fn on_error(&mut self, msg: &str) {
        self.tui.push_error(msg);
    }
}

/// The agentic tool loop — the self-extension keystone. Given a natural-language
/// `goal`, drive a multi-turn Messages conversation with the tool set:
///
///   send(goal + tools) → parse → if tool_use { run it, append the assistant
///   tool_use turn + the tool_result turn, loop } else { done, return the text }
///
/// Runs until the model returns a text turn with no tool_use (no artificial cap:
/// tools are tier-clamped, so a looping agent only burns tokens, not access).
/// Reuses the one keep-alive `Session` for every turn (the connection survives
/// between requests), so a whole task is one TLS handshake. Live progress flows
/// through `rep`; the final assistant text (or an error line) is returned.
///
/// SECURITY: tools are tier-clamped. `bash` runs in a shell spawned at tier 0
/// (Public), and `read_file`/`write_file` are clamped to `AGENT_TIER` (Public) by
/// `agent_may_access`, so the agent can only touch Public objects even though
/// those tools execute in (higher-clearance) kernel context.
pub fn run_agent(goal: &str, rep: &mut dyn AgentReporter) -> String {
    let key = api_key();
    if key.is_empty() {
        let m = "agent: no ANTHROPIC_KEY configured in this build";
        rep.on_error(m);
        return String::from(m);
    }
    if goal.trim().is_empty() {
        let m = "agent: empty goal";
        rep.on_error(m);
        return String::from(m);
    }

    let model = model_name();
    // The running transcript. Grows by two turns per tool call (the assistant's
    // tool_use, then our tool_result) so the model always sees the full history.
    let mut msgs: Vec<Message> = Vec::new();
    msgs.push(Message::text("user", goal));

    let mut session = match Session::open() {
        Ok(s) => s,
        Err(e) => {
            let m = format!("agent: connection failed ({})", e);
            rep.on_error(&m);
            return m;
        }
    };

    // Fixed transport buffers on the heap (see `ask`): this runs under the
    // fullscreen TUI, so large stack arrays risk the layout-sensitive overflow.
    let mut resp = Box::new([0u8; RESP_CAP]);
    let mut body = Box::new([0u8; RESP_CAP]);
    let mut final_text = String::new();

    // Arm Ctrl+C abort: the PS/2 IRQ handler sets ABORT_REQUESTED even while
    // we're blocked in a network wait or tool call, so polling it here gives a
    // responsive interrupt for a runaway or unwanted loop.
    crate::keyboard::clear_abort();

    let mut turn: u32 = 0;
    loop {
        // Ctrl+C between turns (and on entry) → stop cleanly, closing the session.
        if crate::keyboard::abort_requested() {
            session.close();
            let m = format!("agent: aborted by user (Ctrl+C) after {} turn(s)", turn);
            rep.on_error(&m);
            return m;
        }
        turn += 1;
        rep.on_status("thinking");
        let req = build_request(model, 1024, AGENT_SYSTEM, &msgs);
        let http = build_http_request(&req, key, true);

        let n = match session.request(http.as_bytes(), &mut resp[..]) {
            Ok(n) => n,
            Err(e) => {
                let m = format!("agent: request failed on turn {} ({})", turn, e);
                rep.on_error(&m);
                session.close();
                return m;
            }
        };

        let bn = decode_body(&resp[..n], &mut body[..]);
        // A buffer that fills EXACTLY means the response wanted more bytes
        // than we can hold — the JSON is cut mid-stream and any parse would
        // hallucinate a partial tool call. Fail loud with the real reason.
        if n == resp.len() || bn == body.len() {
            session.close();
            let m = format!(
                "agent: response exceeded {} bytes on turn {} (truncated) — the model must write files in smaller chunks",
                RESP_CAP, turn
            );
            rep.on_error(&m);
            return m;
        }
        let parsed = parse_response(&String::from_utf8_lossy(&body[..bn]));

        // Narrate any free text the model emitted alongside (or instead of) a call.
        if let Some(t) = parsed.text.as_ref() {
            let t = t.trim();
            if !t.is_empty() {
                rep.on_text(t);
                final_text = String::from(t);
            }
        }

        match parsed.tool_use {
            Some(tu) => {
                // Replay the assistant's tool_use turn verbatim (the API requires
                // the tool_use to precede its tool_result), run the tool, then
                // feed the result back as a user turn for the next iteration.
                rep.on_status(&tu.name);
                rep.on_tool_call(&tu.name, &tu.input_json);
                let result = run_tool(&tu.name, &tu.input_json);
                rep.on_tool_result(&result);
                msgs.push(Message::assistant_tool_use(&tu));
                msgs.push(Message::tool_result(&tu.id, &result));
                // fall through to the next turn
            }
            None => {
                // No tool call: the model is done (or produced only text).
                session.close();
                if final_text.is_empty() {
                    let status = http_status(&resp[..n]).unwrap_or(0);
                    let m = format!("agent: no answer (HTTP {})", status);
                    rep.on_error(&m);
                    return m;
                }
                return final_text;
            }
        }
    }
}

/// SYS_AGENT — the interactive split-pane agent terminal launched by the
/// shell's `agent` builtin. Each task the user types drives the full agentic
/// tool loop (`run_agent`): the model reads/writes files and runs shell commands
/// via tools until the work is done, with every turn — assistant text, tool
/// calls, tool results — rendered live in the conversation pane, until they type
/// `exit`/`quit`. Without a baked-in key it still runs — you can see the UI and
/// type — and reports that acting needs a key. Headless (no framebuffer) →
/// nothing to show, returns 1.
///
/// While this runs, the shell is blocked in the syscall and the interactive
/// wait loop must not pump the HID ring (it would race our `read_line` pump),
/// so we hold `FULLSCREEN_APP_ACTIVE` for the duration and clear the screen on exit.
pub fn run_interactive(_flags: u64) -> u64 {
    use crate::tui::Tui;
    use core::sync::atomic::Ordering;

    // Clear the boot console first so the TUI sits on a clean screen instead of
    // overlaying leftover demo/shell scrollback.
    crate::framebuffer::clear();
    // Box the Tui: four TtyConsole scrollback rings are ~27 KiB, and held on
    // the stack for the whole session they made run_agent_tui's frame ~55 KiB
    // (measured via -Zemit-stack-sizes) — one nested ask() away from smashing
    // the 64 KiB task stack. The pane state lives on the heap instead; the
    // ask output buffer goes with it. See 2026-07-17 review, medium #4.2.
    let mut tui: Box<Tui> = match Tui::new(model_name()) {
        Some(t) => Box::new(t),
        None => return 1, // headless — no framebuffer to draw the TUI
    };
    tui.push_assistant("Agent terminal — describe a task, Enter to send. I can read/write files and run shell commands to do it. 'exit' returns to the shell.");

    let have_key = !api_key().is_empty();
    if !have_key {
        tui.push_error("(no ANTHROPIC_KEY in this build — you can type, but acting needs a key)");
    }

    crate::FULLSCREEN_APP_ACTIVE.store(true, Ordering::Relaxed);
    // The TUI renders the typed line itself in its prompt pane — stop the
    // line discipline from also echoing it into the legacy fb console behind
    // the panes (that's the "typed text appears twice" artifact).
    crate::tty::SUPPRESS_TTY_FB_ECHO.store(true, Ordering::Relaxed);

    loop {
        tui.set_status("ready");
        let mut qbuf = [0u8; 512];
        let n = tui.read_line(&mut qbuf);
        let q = core::str::from_utf8(&qbuf[..n]).unwrap_or("").trim();
        if q.is_empty() {
            // Empty line (user just hit Enter): show the prompt again but don't
            // busy-spin a request. read_line already blocks on input, so looping
            // here is cheap only because we immediately block again — keep it.
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
        // Drive the full agentic tool loop. `run_agent` renders every turn —
        // assistant text, tool calls, tool results — through the reporter as it
        // goes, so the return value is already on screen; we discard it here.
        let mut rep = TuiReporter { tui: &mut *tui };
        let _ = run_agent(q, &mut rep);
    }

    crate::FULLSCREEN_APP_ACTIVE.store(false, Ordering::Relaxed);
    crate::tty::SUPPRESS_TTY_FB_ECHO.store(false, Ordering::Relaxed);
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
            let append = field_bool(input_json, "append");
            match (path, content) {
                (Some(p), Some(c)) => match write_file(&p, c.as_bytes(), append) {
                    Ok(()) if append => format!("appended {} bytes to {}", c.len(), p),
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
        "compile" => {
            let source = field_str(input_json, "source");
            let out = field_str(input_json, "out");
            match source {
                Some(src) => compile_source(&src, out.as_deref()),
                None => String::from("error: missing 'source'"),
            }
        }
        other => format!("error: unknown tool '{}'", other),
    }
}

/// The agent's `compile` tool: run the on-device Rust compiler on `source` and
/// return its output. Implemented by spawning `/bin/semos-rustc <src> -o <out>`
/// through the same tier-0 `bash` sandbox as the `bash` tool, so the compiler
/// inherits the agent's Public clearance — it can't read a higher-tier source or
/// write a higher-tier output. `out` defaults to `source` with a trailing `.rs`
/// swapped for `.elf` (or `.elf` appended).
fn compile_source(source: &str, out: Option<&str>) -> String {
    if !agent_may_access(source) {
        return String::from("error: denied: source exceeds agent tier (Public)");
    }
    let default_out;
    let out = match out {
        Some(o) => o,
        None => {
            default_out = if let Some(stem) = source.strip_suffix(".rs") {
                format!("{}.elf", stem)
            } else {
                format!("{}.elf", source)
            };
            &default_out
        }
    };
    if !agent_may_access(out) {
        return String::from("error: denied: output exceeds agent tier (Public)");
    }
    run_bash(&format!("/bin/semos-rustc {} -o {}", source, out))
}

/// Pull a string field out of a small JSON object (the tool input).
fn field_str(obj_json: &str, key: &str) -> Option<String> {
    scan_string_field(obj_json.as_bytes(), key, 0).map(|(v, _)| v)
}

/// Pull a boolean field out of a small JSON object (`"key":true/false`).
/// Absent or malformed → false (callers use it for opt-in flags only).
fn field_bool(obj_json: &str, key: &str) -> bool {
    let b = obj_json.as_bytes();
    let mut i = 0usize;
    // Reuse the string-field scanner's key walk by scanning for `"key"`.
    while i + key.len() + 2 <= b.len() {
        if b[i] == b'"' && &b[i + 1..i + 1 + key.len().min(b.len() - i - 1)] == key.as_bytes()
            && i + 1 + key.len() < b.len() && b[i + 1 + key.len()] == b'"'
        {
            let mut j = i + key.len() + 2;
            while j < b.len() && (b[j] == b' ' || b[j] == b':' || b[j] == b'\t') { j += 1; }
            return b[j..].starts_with(b"true");
        }
        i += 1;
    }
    false
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

/// The agent's security clearance. The LLM is the least-trusted component in the
/// 4-tier model, so both its shell (spawned at tier 0) and its direct file tools
/// (read_file/write_file, which run in kernel context) are clamped to Public —
/// the agent can only ever touch Public-tier objects, never Internal/Sensitive/
/// Secret, regardless of the kernel task's own (higher) clearance.
///
/// This constant is the single mutation point for the agent's tier: when tasks
/// gain a real per-task agent tier, thread `current_task_max_tier()` through here
/// instead of the hard-coded Public.
const AGENT_TIER: kernel_core::memory::pools::SecurityTier =
    kernel_core::memory::pools::SecurityTier::Public;

/// Look up the tier of the object at `path`. Returns None if the path doesn't
/// resolve (a write would then *create* it — treated as the agent's own tier).
fn path_tier(path: &str) -> Option<kernel_core::memory::pools::SecurityTier> {
    use kernel_core::fs::Namespace;
    let suid = Namespace::resolve(path).ok()?;
    kernel_core::semantic::registry::global_registry()
        .get(&suid)
        .map(|o| o.tier)
}

/// Gate one file tool against the agent's tier: deny if the target object's tier
/// exceeds the agent's clearance. A not-yet-existing path (write→create) passes
/// and is created Public, which the agent by definition can access.
fn agent_may_access(path: &str) -> bool {
    match path_tier(path) {
        Some(t) => (t as u8) <= (AGENT_TIER as u8),
        None => true,
    }
}

/// Read a path-namespace file via SYS_OPEN + SYS_FREAD. Clamped to the agent's
/// tier (Public) — reading a higher-tier object is denied here, before the
/// kernel's own (kernel-task) clearance would allow it.
fn read_file(path: &str) -> Result<String, &'static str> {
    if !agent_may_access(path) {
        return Err("denied: path exceeds agent tier (Public)");
    }
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

/// Create every ancestor directory of `path` (mkdir -p on its parent), so a
/// write to e.g. `/apps/hello.txt` succeeds even when `/apps` doesn't exist yet.
/// Errors are ignored on purpose: SYS_MKDIR on an existing dir is a harmless
/// no-op for us, and any real failure surfaces at the subsequent SYS_OPEN.
fn ensure_parent_dirs(path: &str) {
    let bytes = path.as_bytes();
    // Walk interior '/' separators; each prefix up to (not including) a slash is
    // an ancestor directory. The leading '/' (root) and the final path component
    // (the file itself, no trailing slash) are both naturally skipped.
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'/' {
            let dir = &path[..i];
            dispatch(SYS_MKDIR, dir.as_ptr() as u64, dir.len() as u64, 0, 0);
        }
        i += 1;
    }
}

/// Write a file via SYS_OPEN(CREATE) + SYS_FWRITE, creating any missing parent
/// directories first so the agent can write to a fresh path. Default mode
/// truncates and writes from offset 0; `append` seeks to the current end
/// instead — the chunking escape hatch for files too big for one tool-call
/// response (see AGENT_SYSTEM). Clamped to the agent's tier (Public):
/// overwriting a higher-tier object is denied before we touch it (the kernel
/// task's own clearance would otherwise allow it). A not-yet-existing path is
/// created Public — allowed by definition.
fn write_file(path: &str, data: &[u8], append: bool) -> Result<(), &'static str> {
    if !agent_may_access(path) {
        return Err("denied: path exceeds agent tier (Public)");
    }
    ensure_parent_dirs(path);
    let fd = dispatch(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 1 /*CREATE*/, 0);
    if fd == u64::MAX {
        return Err("open failed");
    }
    if append {
        // Current length via STATX, then absolute seek to it.
        let mut st = StatX {
            size: 0, suid_high: 0, suid_low: 0, created_at: 0, modified_at: 0,
            file_type: 0, tier: 0, _reserved: [0; 3],
        };
        dispatch(SYS_STATX, path.as_ptr() as u64, path.len() as u64,
                 &mut st as *mut _ as u64, 0);
        dispatch(SYS_SEEK, fd, st.size, 0, 0);
    } else {
        // Truncate then write from offset 0.
        dispatch(SYS_TRUNCATE, path.as_ptr() as u64, path.len() as u64, 0, 0);
    }
    let w = dispatch(SYS_FWRITE, fd, data.as_ptr() as u64, data.len() as u64, 0);
    dispatch(SYS_CLOSE, fd, 0, 0, 0);
    if w == u64::MAX {
        Err("write failed")
    } else {
        Ok(())
    }
}
