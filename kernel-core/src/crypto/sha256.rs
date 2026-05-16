//! SHA-256 — streaming implementation (FIPS 180-4).
//!
//! Replaces the earlier inline impl in `mod.rs` which truncated inputs
//! to 55 bytes ("// Fit in one block with padding") and silently produced
//! wrong hashes for anything longer. This implementation accepts arbitrary
//! input length via the streaming [`Sha256`] API, with a one-shot [`hash`]
//! convenience that matches the old call sites byte-for-byte on short input
//! and now also works correctly on long input.
//!
//! Test vectors: RFC 6234 §B.1 (and FIPS 180-2 §B). Includes regression for
//! inputs > 55 bytes (where the old impl was wrong) and multi-block inputs.
//!
//! No `alloc`, no `std`. Suitable for kernel use. Not constant-time —
//! SHA-256 over public data doesn't need to be; for HMAC over secrets we
//! rely on the AEAD layer above to keep the secret-handling separate.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// SHA-256 block size in bytes.
pub const BLOCK_SIZE: usize = 64;

/// SHA-256 output size in bytes.
pub const OUTPUT_SIZE: usize = 32;

/// Streaming SHA-256 hasher. Update with arbitrary bytes; call `finalize`
/// (or `finalize_into`) once to consume the hasher and produce the digest.
///
/// Reusing the same hasher across multiple messages requires explicit
/// re-creation (`Sha256::new()`) — `Drop` does not reset, and `finalize`
/// takes the hasher by value.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    /// Bytes processed so far, used to compute the final bit length.
    bytes_processed: u64,
    /// Partial block buffer. `buf_len` is the number of valid bytes in it.
    buf: [u8; BLOCK_SIZE],
    buf_len: usize,
}

impl Sha256 {
    /// Create a new SHA-256 hasher in its initial state.
    pub const fn new() -> Self {
        Self {
            state: H0,
            bytes_processed: 0,
            buf: [0u8; BLOCK_SIZE],
            buf_len: 0,
        }
    }

    /// Feed bytes into the hasher. Any number of bytes, any number of calls.
    pub fn update(&mut self, mut data: &[u8]) {
        // First, top up any partial block held over from a previous call.
        if self.buf_len > 0 {
            let needed = BLOCK_SIZE - self.buf_len;
            let take = needed.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            self.bytes_processed = self.bytes_processed.wrapping_add(take as u64);
            if self.buf_len == BLOCK_SIZE {
                let block = self.buf; // copy out so we don't borrow `self.buf` while calling
                compress(&mut self.state, &block);
                self.buf_len = 0;
            }
        }

        // Process whole 64-byte blocks directly from `data` to avoid copying.
        while data.len() >= BLOCK_SIZE {
            let mut block = [0u8; BLOCK_SIZE];
            block.copy_from_slice(&data[..BLOCK_SIZE]);
            compress(&mut self.state, &block);
            self.bytes_processed = self.bytes_processed.wrapping_add(BLOCK_SIZE as u64);
            data = &data[BLOCK_SIZE..];
        }

        // Stash any leftover bytes for the next update or finalize.
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
            self.bytes_processed = self.bytes_processed.wrapping_add(data.len() as u64);
        }
    }

    /// Finalize and produce the 32-byte digest. Consumes the hasher.
    pub fn finalize(mut self) -> [u8; OUTPUT_SIZE] {
        // FIPS 180-4 padding: append 0x80, pad with zeros to 56 mod 64,
        // then append the 64-bit big-endian bit length.
        let bit_len = self.bytes_processed.wrapping_mul(8);

        // First, the 0x80 terminator.
        let pos = self.buf_len;
        self.buf[pos] = 0x80;
        self.buf_len += 1;

        // If we can't fit the 8-byte length in the current block, finish
        // this block and start a fresh one for the length.
        if self.buf_len > BLOCK_SIZE - 8 {
            // Zero-fill to end of block and compress.
            for b in &mut self.buf[self.buf_len..BLOCK_SIZE] { *b = 0; }
            let block = self.buf;
            compress(&mut self.state, &block);
            self.buf_len = 0;
        }
        // Zero-fill up to the length slot.
        for b in &mut self.buf[self.buf_len..BLOCK_SIZE - 8] { *b = 0; }
        // Big-endian 64-bit bit length.
        self.buf[BLOCK_SIZE - 8..BLOCK_SIZE].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        compress(&mut self.state, &block);

        let mut out = [0u8; OUTPUT_SIZE];
        for (i, &word) in self.state.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

impl Default for Sha256 {
    fn default() -> Self { Self::new() }
}

/// One-shot SHA-256 of `data`. Drop-in replacement for the old
/// `crypto::sha256::hash` — same signature, same output for the inputs
/// where the old impl wasn't truncating, **correct** output for inputs
/// longer than 55 bytes (where the old impl was broken).
pub fn hash(data: &[u8]) -> [u8; OUTPUT_SIZE] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}

// ============================================================================
// HMAC-SHA256 (RFC 2104). Co-located here because the existing call sites
// access it as `crypto::sha256::hmac`; keeping the API path stable lets the
// callers (key_hierarchy, master_key PBKDF2, syscall) continue to work
// without source changes. Move to a dedicated `hmac.rs` later if it grows.
// ============================================================================

/// HMAC-SHA256 of `data` under `key`. One-shot convenience.
///
/// Replaces the earlier inline HMAC that capped `data` at 64 bytes. For
/// callers that need to MAC data arriving in pieces (e.g. HKDF-Expand
/// concatenates T(i-1) || info || counter), see [`HmacSha256`] below.
pub fn hmac(key: &[u8], data: &[u8]) -> [u8; OUTPUT_SIZE] {
    let mut h = HmacSha256::new(key);
    h.update(data);
    h.finalize()
}

/// Streaming HMAC-SHA256 (RFC 2104).
///
/// Lets the caller feed the message in pieces — needed by HKDF-Expand,
/// which would otherwise have to concatenate into a stack buffer with a
/// hard-coded upper bound on `info` length.
#[derive(Clone)]
pub struct HmacSha256 {
    /// Inner SHA-256, pre-fed with `key XOR ipad`. Subsequent `update`
    /// calls just feed straight through.
    inner: Sha256,
    /// Pre-computed `key XOR opad` for the outer hash at finalize time.
    outer_key: [u8; BLOCK_SIZE],
}

impl HmacSha256 {
    /// Initialise an HMAC-SHA256 context with `key`. Per RFC 2104, a
    /// `key` longer than [`BLOCK_SIZE`] is hashed down first; a shorter
    /// `key` is zero-padded out.
    pub fn new(key: &[u8]) -> Self {
        let mut k = [0u8; BLOCK_SIZE];
        if key.len() > BLOCK_SIZE {
            let h = hash(key);
            k[..OUTPUT_SIZE].copy_from_slice(&h);
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut inner_key = [0x36u8; BLOCK_SIZE];
        let mut outer_key = [0x5cu8; BLOCK_SIZE];
        for i in 0..BLOCK_SIZE {
            inner_key[i] ^= k[i];
            outer_key[i] ^= k[i];
        }
        let mut inner = Sha256::new();
        inner.update(&inner_key);
        Self { inner, outer_key }
    }

    /// Append `data` to the MAC's input. Any number of bytes, any number
    /// of calls — same correctness as one big `update` of the concat.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finalise and produce the 32-byte tag. Consumes the context.
    pub fn finalize(self) -> [u8; OUTPUT_SIZE] {
        let inner_digest = self.inner.finalize();
        let mut outer = Sha256::new();
        outer.update(&self.outer_key);
        outer.update(&inner_digest);
        outer.finalize()
    }
}

#[cfg(test)]
mod hmac_tests {
    use super::*;

    fn hex(d: &[u8], out: &mut [u8; 64]) -> usize {
        let mut p = 0;
        for &b in d {
            let high = b >> 4;
            let low = b & 0xF;
            out[p]     = if high < 10 { b'0' + high } else { b'a' + high - 10 };
            out[p + 1] = if low  < 10 { b'0' + low  } else { b'a' + low  - 10 };
            p += 2;
        }
        p
    }

    fn check(key: &[u8], data: &[u8], expected: &[u8]) {
        let mac = hmac(key, data);
        let mut buf = [0u8; 64];
        let n = hex(&mac, &mut buf);
        assert_eq!(&buf[..n], expected,
            "HMAC mismatch (key len={}, data len={})", key.len(), data.len());
    }

    // RFC 4231 §4.2 — Test Case 1
    #[test]
    fn rfc4231_tc1() {
        let key = [0x0b; 20];
        let data = b"Hi There";
        check(&key, data,
            b"b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }

    // RFC 4231 §4.3 — Test Case 2 (short key, longer message)
    #[test]
    fn rfc4231_tc2() {
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        check(key, data,
            b"5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    // RFC 4231 §4.4 — Test Case 3 (50-byte data, ~exceeds one block in inner SHA)
    #[test]
    fn rfc4231_tc3() {
        let key = [0xaa; 20];
        let data = [0xdd; 50];
        check(&key, &data,
            b"773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe");
    }

    // RFC 4231 §4.6 — Test Case 5 (truncated output not implemented here;
    // we always return the full 32 bytes)
    #[test]
    fn rfc4231_tc5() {
        let key = [0x0c; 20];
        let data = b"Test With Truncation";
        check(&key, data,
            b"a3b6167473100ee06e0c796c2955552bfa6f7c0a6a8aef8b93f860aab0cd20c5");
    }

    // RFC 4231 §4.7 — Test Case 6 (oversized key, gets hashed down)
    #[test]
    fn rfc4231_tc6() {
        let key = [0xaa; 131];
        let data = b"Test Using Larger Than Block-Size Key - Hash Key First";
        check(&key, data,
            b"60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    // RFC 4231 §4.8 — Test Case 7 (oversized key AND oversized data)
    // This is the big one: the old impl truncated `data` to 64 bytes,
    // which would have produced a wrong MAC here. New impl must be correct.
    #[test]
    fn rfc4231_tc7_oversized_data_regression() {
        let key = [0xaa; 131];
        let data = b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.";
        check(&key, data,
            b"9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2");
    }

    // Streaming HMAC must equal one-shot HMAC for every chunking pattern.
    // Catches buggy streaming impls that would only show up under HKDF.
    #[test]
    fn streaming_hmac_matches_oneshot() {
        let key = b"secret-key-value-with-medium-length-content";
        let data: [u8; 200] = core::array::from_fn(|i| (i * 7) as u8);
        let oneshot = hmac(key, &data);
        // Single-call streaming
        let mut h = HmacSha256::new(key);
        h.update(&data);
        assert_eq!(oneshot, h.finalize(), "single-update streaming mismatch");
        // Byte-by-byte streaming
        let mut h = HmacSha256::new(key);
        for b in &data { h.update(&[*b]); }
        assert_eq!(oneshot, h.finalize(), "byte-by-byte streaming mismatch");
        // Irregular chunks (catches buffer-boundary bugs)
        let mut h = HmacSha256::new(key);
        for c in data.chunks(13) { h.update(c); }
        assert_eq!(oneshot, h.finalize(), "irregular-chunk streaming mismatch");
    }
}

// ============================================================================
// Compression function — FIPS 180-4 §6.2.2.
// ============================================================================

#[inline(always)]
fn rotr(x: u32, n: u32) -> u32 {
    x.rotate_right(n)
}
#[inline(always)]
fn ch(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (!x & z) }
#[inline(always)]
fn maj(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (x & z) ^ (y & z) }
#[inline(always)]
fn sigma0(x: u32) -> u32 { rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22) }
#[inline(always)]
fn sigma1(x: u32) -> u32 { rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25) }
#[inline(always)]
fn gamma0(x: u32) -> u32 { rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3) }
#[inline(always)]
fn gamma1(x: u32) -> u32 { rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10) }

fn compress(state: &mut [u32; 8], block: &[u8; BLOCK_SIZE]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        let j = i * 4;
        w[i] = u32::from_be_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
    }
    for i in 16..64 {
        w[i] = gamma1(w[i - 2])
            .wrapping_add(w[i - 7])
            .wrapping_add(gamma0(w[i - 15]))
            .wrapping_add(w[i - 16]);
    }

    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) =
        (state[0], state[1], state[2], state[3],
         state[4], state[5], state[6], state[7]);

    for i in 0..64 {
        let t1 = h
            .wrapping_add(sigma1(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let t2 = sigma0(a).wrapping_add(maj(a, b, c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

// ============================================================================
// Tests — RFC 6234 / FIPS 180-2 vectors + regressions for the old bug.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Render a digest as lowercase ASCII hex into a fixed buffer.
    /// Returns the prefix as a `&[u8]` for direct comparison against
    /// the test-vector byte strings below — avoids pulling in any
    /// formatting crate.
    fn hex(d: &[u8], out: &mut [u8; 64]) -> usize {
        let mut p = 0;
        for &b in d {
            let high = b >> 4;
            let low = b & 0xF;
            out[p]     = if high < 10 { b'0' + high } else { b'a' + high - 10 };
            out[p + 1] = if low  < 10 { b'0' + low  } else { b'a' + low  - 10 };
            p += 2;
        }
        p
    }

    fn assert_hash(input: &[u8], expected: &[u8]) {
        let mut buf = [0u8; 64];
        let n = hex(&hash(input), &mut buf);
        assert_eq!(&buf[..n], expected,
            "hash mismatch for input of length {}", input.len());
    }

    // Vectors from FIPS 180-2 §B / RFC 6234 §B.

    #[test]
    fn empty_string() {
        assert_hash(b"",
            b"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn abc() {
        assert_hash(b"abc",
            b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn fips_56_byte_block_boundary() {
        // 56-byte input — pushes padding into a second block.
        let input = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(input.len(), 56);
        assert_hash(input,
            b"248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    }

    #[test]
    fn regression_old_truncation_bug() {
        // 100 bytes — the old impl silently truncated to 55 bytes,
        // producing SHA-256 of only the first 55 bytes. New impl must
        // produce the correct full-input hash.
        let input = [b'a'; 100];
        let full = hash(&input);
        let trunc = hash(&input[..55]);
        assert_ne!(full, trunc,
            "post-fix hash must differ from pre-fix truncation");
        // Known SHA-256("a" * 100) from external reference:
        let mut buf = [0u8; 64];
        let n = hex(&full, &mut buf);
        assert_eq!(&buf[..n],
            b"2816597888e4a0d3a36b82b83316ab32680eb8f00f8cd3b904d681246d285a0e");
    }

    #[test]
    fn one_million_a_streaming() {
        // SHA-256("a" * 1_000_000) — standard NIST vector. Exercises
        // multi-block streaming and the buffer-refill path. Note the
        // chunk size is 1000 (not 1024) so the total is exactly 1M bytes.
        let mut h = Sha256::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 { h.update(&chunk); }
        let digest = h.finalize();
        let mut buf = [0u8; 64];
        let n = hex(&digest, &mut buf);
        assert_eq!(&buf[..n],
            b"cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn streaming_matches_oneshot_byte_by_byte() {
        let data = b"the quick brown fox jumps over the lazy dog";
        let oneshot = hash(data);
        let mut h = Sha256::new();
        for b in data { h.update(&[*b]); }
        let streamed = h.finalize();
        assert_eq!(oneshot, streamed);
    }

    #[test]
    fn streaming_matches_oneshot_irregular_chunks() {
        // Various chunk boundaries to catch partial-block edge cases.
        let data: [u8; 200] = core::array::from_fn(|i| i as u8);
        let oneshot = hash(&data);
        let mut h = Sha256::new();
        for chunk in data.chunks(13) { h.update(chunk); }
        assert_eq!(oneshot, h.finalize());
    }
}
