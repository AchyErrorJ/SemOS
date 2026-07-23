//! CRC-16/CCITT-FALSE.
//!
//! Used by the SemOS pairing protocol for typo detection in the hand-typed QR
//! string.  See `docs/pairing-v1.md` §3.
//!
//! Parameters:
//! - Polynomial: `0x1021`
//! - Initial value: `0xFFFF`
//! - No input/output reflection
//! - No final XOR

#[cfg(test)]
extern crate alloc;

/// Compute CRC-16/CCITT-FALSE over `data`.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in data {
        crc ^= (u16::from(byte)) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // Classic CCITT-FALSE check vectors.
        assert_eq!(crc16_ccitt(b"123456789"), 0x29B1);
        assert_eq!(crc16_ccitt(b""), 0xFFFF);
        assert_eq!(crc16_ccitt(b"\x00"), 0xE1F0);
    }

    #[test]
    fn pairing_payload_example() {
        let payload = [
            b'S', b'P', // magic
            0x01,       // version
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // phone_pub (32 zeros)
            192, 168, 1, 42, // ip
            0x1F, 0x90,      // port 8080
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, // nonce
        ];
        let crc = crc16_ccitt(&payload);
        // Big-endian append check.
        let mut with_crc = payload.to_vec();
        with_crc.push((crc >> 8) as u8);
        with_crc.push((crc & 0xFF) as u8);
        assert_eq!(crc16_ccitt(&with_crc), 0x0000);
    }
}
