//! BLAKE2s (RFC 7693) — 32-byte digest, unkeyed and keyed.
//!
//! Added for the WireGuard data plane (SemNet S1): the Noise_IK handshake
//! chains BLAKE2s hashes and uses *keyed* BLAKE2s as its KDF MAC. This is a
//! straightforward reference implementation (no SIMD), matching the style of
//! the other vendored primitives in this module.

const IV: [u32; 8] = [
    0x6A09_E667, 0xBB67_AE85, 0x3C6E_F372, 0xA54F_F53A,
    0x510E_527F, 0x9B05_688C, 0x1F83_D9AB, 0x5BE0_CD19,
];

const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

pub const OUT_LEN: usize = 32;
pub const KEY_LEN: usize = 32;
const BLOCK_LEN: usize = 64;

/// Streaming BLAKE2s state. Supports an optional 32-byte key (keyed mode is
/// WireGuard's MAC), always produces a 32-byte digest.
pub struct Blake2s {
    h: [u32; 8],
    t: u64,
    buf: [u8; BLOCK_LEN],
    buflen: usize,
}

#[inline]
fn g(v: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(12);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(8);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(7);
}

fn compress(h: &mut [u32; 8], block: &[u8], t: u64, last: bool) {
    debug_assert_eq!(block.len(), BLOCK_LEN);
    let mut m = [0u32; 16];
    for i in 0..16 {
        m[i] = u32::from_le_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
    }
    let mut v = [0u32; 16];
    v[..8].copy_from_slice(h);
    v[8..].copy_from_slice(&IV);
    v[12] ^= t as u32;
    v[13] ^= (t >> 32) as u32;
    if last {
        v[14] = !v[14];
    }
    for round in 0..10 {
        let s = &SIGMA[round];
        g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
        g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
        g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
        g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
        g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
        g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
        g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
        g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
}

impl Blake2s {
    /// Unkeyed BLAKE2s-256.
    pub fn new() -> Self {
        Self::init(0)
    }

    /// Keyed BLAKE2s-256 (RFC 7693 §2.5): the key occupies the first block.
    pub fn new_keyed(key: &[u8; KEY_LEN]) -> Self {
        let mut st = Self::init(KEY_LEN as u8);
        let mut block = [0u8; BLOCK_LEN];
        block[..KEY_LEN].copy_from_slice(key);
        st.update(&block);
        st
    }

    fn init(keylen: u8) -> Self {
        let mut h = IV;
        // Parameter block: digest 32, key 0|32, fanout 1, depth 1.
        h[0] ^= 0x0101_0000 ^ ((keylen as u32) << 8) ^ (OUT_LEN as u32);
        Self { h, t: 0, buf: [0u8; BLOCK_LEN], buflen: 0 }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        // If the buffer is full and more data arrives, flush it as a
        // non-final block. A full buffer is held back so finalize() always
        // has the last block in hand.
        if self.buflen == BLOCK_LEN && !data.is_empty() {
            self.t += BLOCK_LEN as u64;
            let block = self.buf;
            compress(&mut self.h, &block, self.t, false);
            self.buflen = 0;
        }
        while !data.is_empty() {
            let take = core::cmp::min(BLOCK_LEN - self.buflen, data.len());
            self.buf[self.buflen..self.buflen + take].copy_from_slice(&data[..take]);
            self.buflen += take;
            data = &data[take..];
            if self.buflen == BLOCK_LEN && !data.is_empty() {
                self.t += BLOCK_LEN as u64;
                let block = self.buf;
                compress(&mut self.h, &block, self.t, false);
                self.buflen = 0;
            }
        }
    }

    pub fn finalize(mut self) -> [u8; OUT_LEN] {
        self.t += self.buflen as u64;
        for b in &mut self.buf[self.buflen..] {
            *b = 0;
        }
        let block = self.buf;
        compress(&mut self.h, &block, self.t, true);
        let mut out = [0u8; OUT_LEN];
        for i in 0..8 {
            out[4 * i..4 * i + 4].copy_from_slice(&self.h[i].to_le_bytes());
        }
        out
    }
}

/// One-shot unkeyed BLAKE2s-256.
pub fn blake2s_256(data: &[u8]) -> [u8; OUT_LEN] {
    let mut st = Blake2s::new();
    st.update(data);
    st.finalize()
}

/// One-shot keyed BLAKE2s-256 — WireGuard's `MAC(key, input)`.
pub fn keyed_blake2s_256(key: &[u8; KEY_LEN], data: &[u8]) -> [u8; OUT_LEN] {
    let mut st = Blake2s::new_keyed(key);
    st.update(data);
    st.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_hex(s: &str) -> [u8; OUT_LEN] {
        let b = s.as_bytes();
        let mut out = [0u8; OUT_LEN];
        let val = |c: u8| -> u8 {
            match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                _ => 0,
            }
        };
        for i in 0..OUT_LEN {
            out[i] = (val(b[2 * i]) << 4) | val(b[2 * i + 1]);
        }
        out
    }

    // RFC 7693 Appendix B.
    #[test]
    fn empty_unkeyed() {
        assert_eq!(
            blake2s_256(b""),
            from_hex("69217a3079908094e11121d042354a7c1f55b6482ca1a51e1b250dfd1ed0eef9")
        );
    }

    #[test]
    fn abc_unkeyed() {
        assert_eq!(
            blake2s_256(b"abc"),
            from_hex("508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982")
        );
    }

    // Keyed vector (hashlib-verified): key = 00..1f, input = 00..3f.
    #[test]
    fn keyed_vector() {
        let mut key = [0u8; KEY_LEN];
        let mut input = [0u8; 64];
        for i in 0..32 {
            key[i] = i as u8;
        }
        for i in 0..64 {
            input[i] = i as u8;
        }
        assert_eq!(
            keyed_blake2s_256(&key, &input),
            from_hex("8975b0577fd35566d750b362b0897a26c399136df07bababbde6203ff2954ed4")
        );
    }

    // Streaming in odd-sized chunks must match the one-shot result.
    #[test]
    fn streaming_matches_one_shot() {
        let mut data = [0u8; 200];
        for i in 0..200 {
            data[i] = (i * 7 + 3) as u8;
        }
        let want = blake2s_256(&data);
        for chunk in [1usize, 3, 63, 64, 65, 127] {
            let mut st = Blake2s::new();
            for piece in data.chunks(chunk) {
                st.update(piece);
            }
            assert_eq!(st.finalize(), want, "chunk size {}", chunk);
        }
    }
}
