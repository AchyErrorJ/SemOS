//! HTTP/1.1 chunked-transfer-encoding decoder (M13).
//!
//! RFC 7230 §4.1 chunked bodies look like:
//!
//! ```text
//!   <hex-size>[;chunk-ext]\r\n
//!   <data ... size bytes>\r\n
//!   <hex-size>\r\n
//!   <data>\r\n
//!   0\r\n
//!   [trailer-field\r\n]*
//!   \r\n
//! ```
//!
//! `decode_chunked` consumes the raw chunked bytes and writes the
//! reassembled body into a caller-provided output buffer, returning the
//! number of bytes written. kernel-core has no global allocator, so the
//! output buffer is caller-owned (matching the rest of the net/llm code,
//! which threads fixed scratch buffers rather than `Vec`s). The decoder is
//! defensive: any truncation or malformed length line yields a clean
//! [`ChunkedError`] instead of panicking, indexing out of bounds, or
//! over-/under-flowing.

/// Reasons a chunked body failed to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkedError {
    /// A chunk-size line was not terminated by CRLF before input ran out.
    UnterminatedSizeLine,
    /// A chunk-size line contained no valid hex digits.
    InvalidChunkSize,
    /// The declared chunk data (plus its trailing CRLF) ran past EOF.
    TruncatedChunkData,
    /// A chunk's data was not followed by the required CRLF.
    MissingChunkCrlf,
    /// Input ended before the terminating zero-length chunk was seen.
    MissingFinalChunk,
    /// The decoded body did not fit in the caller's output buffer.
    OutputTooSmall,
}

/// Decode an HTTP chunked-transfer-encoded body.
///
/// `input` is the raw bytes *after* the HTTP header separator (the chunk
/// stream itself). Decoded body bytes are written into `out`; the number of
/// bytes written is returned on success.
///
/// Handles multiple chunks, hex chunk sizes (any case), optional chunk
/// extensions (`;name=value` after the size), and optional trailer headers
/// after the final `0`-sized chunk. The final chunk terminates decoding even
/// if a complete trailer section never arrives — a length-zero chunk is the
/// authoritative end-of-body marker.
pub fn decode_chunked(input: &[u8], out: &mut [u8]) -> Result<usize, ChunkedError> {
    let mut i = 0usize;
    let mut written = 0usize;

    loop {
        // ---- parse chunk-size line -------------------------------------
        // Find the CRLF ending the size line, scanning from `i`.
        let line_end = find_crlf(input, i).ok_or(ChunkedError::UnterminatedSizeLine)?;
        let size_line = &input[i..line_end];

        // Chunk extensions (";...") follow the size; stop at the first ';'.
        let size_field = match size_line.iter().position(|&b| b == b';') {
            Some(semi) => &size_line[..semi],
            None => size_line,
        };

        let chunk_size = parse_hex(size_field).ok_or(ChunkedError::InvalidChunkSize)?;

        // Advance past "<size>...\r\n".
        i = line_end + 2;

        if chunk_size == 0 {
            // Final chunk. Anything remaining is the (optional) trailer
            // section terminated by a blank line; we don't need its content,
            // so a zero-length chunk is a clean, authoritative end-of-body.
            return Ok(written);
        }

        // ---- copy chunk data -------------------------------------------
        // `i + chunk_size` must stay within `input`; guard against overflow
        // and truncation.
        let data_end = i.checked_add(chunk_size).ok_or(ChunkedError::TruncatedChunkData)?;
        if data_end > input.len() {
            return Err(ChunkedError::TruncatedChunkData);
        }

        if written.checked_add(chunk_size).map_or(true, |w| w > out.len()) {
            return Err(ChunkedError::OutputTooSmall);
        }
        out[written..written + chunk_size].copy_from_slice(&input[i..data_end]);
        written += chunk_size;
        i = data_end;

        // ---- consume the CRLF after the chunk data ---------------------
        if i + 2 > input.len() || input[i] != b'\r' || input[i + 1] != b'\n' {
            return Err(ChunkedError::MissingChunkCrlf);
        }
        i += 2;
    }
}

/// Whether the HTTP header block declares `Transfer-Encoding: chunked`.
///
/// `headers` is the bytes from the start of the response up to (and
/// optionally including) the `\r\n\r\n` separator. The match is
/// case-insensitive on both the field name and the `chunked` token, since
/// RFC 7230 makes both case-insensitive.
pub fn is_chunked(headers: &[u8]) -> bool {
    const NAME: &[u8] = b"transfer-encoding:";
    let mut i = 0;
    while i + NAME.len() <= headers.len() {
        if eq_ignore_case(&headers[i..i + NAME.len()], NAME) {
            // Scan the field value to end-of-line for the `chunked` token.
            let val_start = i + NAME.len();
            let val_end = find_crlf(headers, val_start).unwrap_or(headers.len());
            if contains_token_ignore_case(&headers[val_start..val_end], b"chunked") {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Parse the `Content-Length` header value out of an HTTP header block, if
/// present. `headers` is the bytes up to (and optionally including) the blank
/// line. Returns `None` if the header is absent or its value isn't a decimal.
/// Case-insensitive on the field name (RFC 7230). Used by keep-alive response
/// framing to know how many body bytes to read before the connection's next
/// request.
pub fn content_length(headers: &[u8]) -> Option<usize> {
    const NAME: &[u8] = b"content-length:";
    let mut i = 0;
    while i + NAME.len() <= headers.len() {
        // Must be at a line start (index 0 or just after a CRLF) so we don't
        // match a header whose name merely ends in "content-length".
        let at_line_start = i == 0 || (i >= 2 && headers[i - 2] == b'\r' && headers[i - 1] == b'\n');
        if at_line_start && eq_ignore_case(&headers[i..i + NAME.len()], NAME) {
            let val_start = i + NAME.len();
            let val_end = find_crlf(headers, val_start).unwrap_or(headers.len());
            let mut value: usize = 0;
            let mut any = false;
            for &b in &headers[val_start..val_end] {
                match b {
                    b'0'..=b'9' => {
                        value = value.checked_mul(10)?.checked_add((b - b'0') as usize)?;
                        any = true;
                    }
                    b' ' | b'\t' => {}
                    _ => return None,
                }
            }
            return if any { Some(value) } else { None };
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Return the index of the `\r` of the next CRLF at or after `from`.
fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    if from >= buf.len() {
        return None;
    }
    let mut i = from;
    while i + 1 < buf.len() {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse an ASCII hex byte string into a `usize`. Returns `None` for an empty
/// field, a non-hex byte, or a value that overflows `usize`. Surrounding
/// horizontal whitespace is tolerated.
fn parse_hex(field: &[u8]) -> Option<usize> {
    // Trim leading/trailing spaces and tabs.
    let mut start = 0;
    let mut end = field.len();
    while start < end && (field[start] == b' ' || field[start] == b'\t') {
        start += 1;
    }
    while end > start && (field[end - 1] == b' ' || field[end - 1] == b'\t') {
        end -= 1;
    }
    let field = &field[start..end];
    if field.is_empty() {
        return None;
    }
    let mut value: usize = 0;
    for &b in field {
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as usize,
            b'a'..=b'f' => (b - b'a' + 10) as usize,
            b'A'..=b'F' => (b - b'A' + 10) as usize,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(digit)?;
    }
    Some(value)
}

fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(&x, &y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

/// Whether `haystack` contains `token` (case-insensitive). `token` must be
/// given in lowercase.
fn contains_token_ignore_case(haystack: &[u8], token: &[u8]) -> bool {
    if token.is_empty() || token.len() > haystack.len() {
        return false;
    }
    let last = haystack.len() - token.len();
    let mut i = 0;
    while i <= last {
        if haystack[i..i + token.len()]
            .iter()
            .zip(token.iter())
            .all(|(&x, &y)| x.to_ascii_lowercase() == y)
        {
            return true;
        }
        i += 1;
    }
    false
}
