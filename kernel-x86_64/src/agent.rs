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
        "{\"name\":\"bash\",\"description\":\"Run a shell command via sem-sh and return its output.\",",
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
            Some(_cmd) => String::from("error: bash tool not wired yet (stage C)"),
            None => String::from("error: missing 'command'"),
        },
        other => format!("error: unknown tool '{}'", other),
    }
}

/// Pull a string field out of a small JSON object (the tool input).
fn field_str(obj_json: &str, key: &str) -> Option<String> {
    scan_string_field(obj_json.as_bytes(), key, 0).map(|(v, _)| v)
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
