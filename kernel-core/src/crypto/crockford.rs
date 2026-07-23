//! Crockford base32 encoding/decoding.
//!
//! Used by the SemOS pairing protocol to render the 59-byte QR payload as a
//! human-typable string.  See `docs/pairing-v1.md` §3.
//!
//! Properties:
//! - Uppercase output, no padding.
//! - Decoder accepts lowercase input and ignores hyphens (`-`).
//! - Crockford aliases are accepted on decode: `I`/`i`/`L`/`l` → `1`,
//!   `O`/`o` → `0`.
//! - No heap allocation; callers provide output buffers.

#[cfg(test)]
extern crate alloc;

/// Crockford base32 alphabet (uppercase).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Return the encoded length for `data_len` bytes (no padding, no hyphens).
pub const fn encoded_len(data_len: usize) -> usize {
    (data_len * 8 + 4) / 5
}

/// Maximum number of input bytes that can be decoded from an encoded string
/// of length `enc_len`.  Because Crockford packs 5 bits per char, every 8
/// chars decode to 5 bytes; any remainder may contribute fewer bytes.
pub const fn decoded_max_len(enc_len: usize) -> usize {
    (enc_len * 5) / 8
}

/// Encode `data` into `out` using Crockford base32.
///
/// Returns the number of bytes written on success, or `None` if `out` is too
/// small.  Use [`encoded_len`] to size the buffer.
pub fn encode_into(data: &[u8], out: &mut [u8]) -> Option<usize> {
    let len = encoded_len(data.len());
    if out.len() < len {
        return None;
    }

    let mut bits = 0u32;
    let mut bits_left = 0u8;
    let mut idx = 0usize;

    for &b in data {
        bits = (bits << 8) | u32::from(b);
        bits_left += 8;
        while bits_left >= 5 {
            bits_left -= 5;
            let val = ((bits >> bits_left) & 0x1F) as usize;
            out[idx] = ALPHABET[val];
            idx += 1;
        }
    }

    if bits_left > 0 {
        let val = ((bits << (5 - bits_left)) & 0x1F) as usize;
        out[idx] = ALPHABET[val];
        idx += 1;
    }

    Some(idx)
}

/// Decode Crockford base32 bytes from `input` into `out`.
///
/// Accepts uppercase, lowercase, and hyphens (which are ignored).  Returns the
/// number of bytes written on success, or `None` on invalid input or if `out`
/// is too small.
pub fn decode_into(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut bits = 0u32;
    let mut bits_left = 0u8;
    let mut idx = 0usize;

    for &c in input {
        if c == b'-' {
            continue;
        }
        let val = decode_char(c)?;
        bits = (bits << 5) | u32::from(val);
        bits_left += 5;
        if bits_left >= 8 {
            bits_left -= 8;
            if idx >= out.len() {
                return None;
            }
            out[idx] = ((bits >> bits_left) & 0xFF) as u8;
            idx += 1;
        }
    }

    // Leftover bits must be all zeros (no partial final byte in our protocol).
    if bits_left > 0 && (bits & ((1u32 << bits_left) - 1)) != 0 {
        return None;
    }

    Some(idx)
}

/// Map an ASCII Crockford character to its 5-bit value.
fn decode_char(c: u8) -> Option<u8> {
    match c {
        b'0' | b'O' | b'o' => Some(0),
        b'1' | b'I' | b'i' | b'L' | b'l' => Some(1),
        b'2' => Some(2),
        b'3' => Some(3),
        b'4' => Some(4),
        b'5' => Some(5),
        b'6' => Some(6),
        b'7' => Some(7),
        b'8' => Some(8),
        b'9' => Some(9),
        b'A' | b'a' => Some(10),
        b'B' | b'b' => Some(11),
        b'C' | b'c' => Some(12),
        b'D' | b'd' => Some(13),
        b'E' | b'e' => Some(14),
        b'F' | b'f' => Some(15),
        b'G' | b'g' => Some(16),
        b'H' | b'h' => Some(17),
        b'J' | b'j' => Some(18),
        b'K' | b'k' => Some(19),
        b'M' | b'm' => Some(20),
        b'N' | b'n' => Some(21),
        b'P' | b'p' => Some(22),
        b'Q' | b'q' => Some(23),
        b'R' | b'r' => Some(24),
        b'S' | b's' => Some(25),
        b'T' | b't' => Some(26),
        b'V' | b'v' => Some(27),
        b'W' | b'w' => Some(28),
        b'X' | b'x' => Some(29),
        b'Y' | b'y' => Some(30),
        b'Z' | b'z' => Some(31),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_to_str(data: &[u8]) -> alloc::string::String {
        let mut buf = [0u8; 256];
        let len = encode_into(data, &mut buf).expect("output buffer too small");
        alloc::string::String::from_utf8(buf[..len].to_vec()).expect("base32 output is ASCII")
    }

    fn decode_to_vec(s: &str) -> Option<alloc::vec::Vec<u8>> {
        let mut out = alloc::vec![0u8; decoded_max_len(s.len())];
        let len = decode_into(s.as_bytes(), &mut out)?;
        out.truncate(len);
        Some(out)
    }

    #[test]
    fn rfc_crockford_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", ""),
            (b"f", "CR"),
            (b"fo", "CSQG"),
            (b"foo", "CSQPY"),
            (b"foob", "CSQPYRG"),
            (b"fooba", "CSQPYRK1"),
            (b"foobar", "CSQPYRK1E8"),
        ];
        for (data, expected) in cases {
            assert_eq!(encode_to_str(data), *expected, "encode {:?}", data);
            assert_eq!(decode_to_vec(expected).as_deref(), Some(*data), "decode {}", expected);
        }
    }

    #[test]
    fn decode_ignores_hyphens_and_case() {
        assert_eq!(decode_to_vec("csq-py-rk1e8").as_deref(), Some(b"foobar" as &[u8]));
        assert_eq!(decode_to_vec("CSQ-PY-RK1E8").as_deref(), Some(b"foobar" as &[u8]));
    }

    #[test]
    fn decode_aliases() {
        // I/L → 1, O → 0; these must decode identically to "1100".
        let expected: &[u8] = &[0x08u8, 0x40];
        assert_eq!(decode_to_vec("1100").as_deref(), Some(expected));
        assert_eq!(decode_to_vec("IIOO").as_deref(), Some(expected));
        assert_eq!(decode_to_vec("LLOO").as_deref(), Some(expected));
        assert_eq!(decode_to_vec("ILOO").as_deref(), Some(expected));
    }

    #[test]
    fn decode_rejects_invalid() {
        assert!(decode_to_vec("CSQPYRK1E!").is_none());
        assert!(decode_to_vec("CSQPYRK1E9").is_none()); // 9 extra bits not zero
    }

    #[test]
    fn encode_into_buffer_too_small() {
        let mut out = [0u8; 1];
        assert!(encode_into(b"foobar", &mut out).is_none());
    }
}
