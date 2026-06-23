//! WPA2-Personal key derivation — the crypto "brain" for joining a WPA2 AP.
//!
//! WPA2-PSK needs SHA-1 (the OS TLS stack only has SHA-256), so this module
//! implements SHA-1 → HMAC-SHA1 → PBKDF2-SHA1 → PMK, plus the IEEE 802.11
//! PRF used to expand the PMK into the PTK during the 4-way handshake.
//!
//! Everything here is pure, offline-testable crypto. `self_test()` runs known
//! answer tests (SHA-1 "abc" + the IEEE 802.11i PMK vector) — KAT discipline:
//! a round trip isn't enough, validate against the spec vectors.

/// SHA-1 initial state (FIPS 180-4).
const H0: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];

/// Compress one 64-byte block into the running state.
fn sha1_block(h: &mut [u32; 5], blk: &[u8; 64]) {
    let mut w = [0u32; 80];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([blk[i * 4], blk[i * 4 + 1], blk[i * 4 + 2], blk[i * 4 + 3]]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
    for (i, &wi) in w.iter().enumerate() {
        let (f, k) = if i < 20 {
            ((b & c) | ((!b) & d), 0x5A82_7999u32)
        } else if i < 40 {
            (b ^ c ^ d, 0x6ED9_EBA1)
        } else if i < 60 {
            ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC)
        } else {
            (b ^ c ^ d, 0xCA62_C1D6)
        };
        let t = a.rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
}

/// Finish: run `rest` (after `total_prefix` already-hashed bytes) through the
/// state with SHA-1 padding and return the digest.
fn sha1_finish(mut h: [u32; 5], rest: &[u8], total_prefix: usize) -> [u8; 20] {
    let bitlen = ((total_prefix + rest.len()) as u64).wrapping_mul(8);
    let mut chunks = rest.chunks_exact(64);
    for c in chunks.by_ref() {
        let mut b = [0u8; 64];
        b.copy_from_slice(c);
        sha1_block(&mut h, &b);
    }
    let rem = chunks.remainder();
    let mut blk = [0u8; 64];
    blk[..rem.len()].copy_from_slice(rem);
    blk[rem.len()] = 0x80;
    if rem.len() >= 56 {
        sha1_block(&mut h, &blk);
        blk = [0u8; 64];
    }
    blk[56..64].copy_from_slice(&bitlen.to_be_bytes());
    sha1_block(&mut h, &blk);
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

/// SHA-1 of a single buffer.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    sha1_finish(H0, data, 0)
}

/// SHA-1 of `first` (exactly 64 bytes) concatenated with `rest`. Lets HMAC hash
/// `ipad||msg` without allocating a combined buffer (no allocator here).
fn sha1_block_then(first: &[u8; 64], rest: &[u8]) -> [u8; 20] {
    let mut h = H0;
    sha1_block(&mut h, first);
    sha1_finish(h, rest, 64)
}

/// HMAC-SHA1(key, msg).
pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = sha1_block_then(&ipad, msg);
    sha1_block_then(&opad, &inner)
}

/// One PBKDF2 output block: F = U1 ^ U2 ^ ... ^ Uc, Ui = HMAC(pass, ...).
fn pbkdf2_block(pass: &[u8], salt: &[u8], iters: u32, block_num: u32) -> [u8; 20] {
    // U1 = HMAC(pass, salt || INT_BE(block_num))
    let mut salt_i = [0u8; 64];
    let sl = salt.len().min(60);
    salt_i[..sl].copy_from_slice(&salt[..sl]);
    salt_i[sl..sl + 4].copy_from_slice(&block_num.to_be_bytes());
    let mut u = hmac_sha1(pass, &salt_i[..sl + 4]);
    let mut t = u;
    for _ in 1..iters {
        u = hmac_sha1(pass, &u);
        for i in 0..20 {
            t[i] ^= u[i];
        }
    }
    t
}

/// WPA2-PSK PMK = PBKDF2-HMAC-SHA1(passphrase, ssid, 4096, 256 bits).
pub fn pmk(passphrase: &[u8], ssid: &[u8]) -> [u8; 32] {
    let b1 = pbkdf2_block(passphrase, ssid, 4096, 1);
    let b2 = pbkdf2_block(passphrase, ssid, 4096, 2);
    let mut out = [0u8; 32];
    out[..20].copy_from_slice(&b1);
    out[20..32].copy_from_slice(&b2[..12]);
    out
}

/// IEEE 802.11 PRF (HMAC-SHA1 based) used to derive the PTK from the PMK.
/// `out` is filled with `out.len()` bytes: PRF-n where n = out.len()*8.
/// Per 802.11: for i in 0.. : HMAC(key, label || 0x00 || data || i_byte).
pub fn prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    let mut pos = 0usize;
    let mut i = 0u8;
    // Concat label || 0x00 || data || i into a small stack buffer.
    let mut buf = [0u8; 128];
    let base = label.len() + 1 + data.len();
    buf[..label.len()].copy_from_slice(label);
    buf[label.len()] = 0x00;
    buf[label.len() + 1..base].copy_from_slice(data);
    while pos < out.len() {
        buf[base] = i;
        let blk = hmac_sha1(key, &buf[..base + 1]);
        let n = (out.len() - pos).min(20);
        out[pos..pos + n].copy_from_slice(&blk[..n]);
        pos += n;
        i += 1;
    }
}

/// Derive the Pairwise Transient Key from the PMK and the 4-way handshake
/// addresses + nonces (IEEE 802.11i §8.5.1.2). PTK = PRF-384("Pairwise key
/// expansion", Min(AA,SPA) || Max(AA,SPA) || Min(ANonce,SNonce) ||
/// Max(ANonce,SNonce)). For CCMP that's 48 bytes:
///   KCK = ptk[0..16]   (EAPOL-Key MIC key)
///   KEK = ptk[16..32]  (EAPOL-Key encryption key, for the GTK in Msg3)
///   TK  = ptk[32..48]  (the CCMP data key handed to the hardware)
/// `aa` = Authenticator (AP) MAC, `spa` = Supplicant (our) MAC.
pub fn ptk(pmk: &[u8; 32], aa: &[u8; 6], spa: &[u8; 6], anonce: &[u8; 32], snonce: &[u8; 32]) -> [u8; 48] {
    let mut data = [0u8; 76]; // 6 + 6 + 32 + 32
    // Min/Max by lexicographic byte order (Rust array Ord does this).
    let (lo_mac, hi_mac) = if aa <= spa { (aa, spa) } else { (spa, aa) };
    data[0..6].copy_from_slice(lo_mac);
    data[6..12].copy_from_slice(hi_mac);
    let (lo_n, hi_n) = if anonce <= snonce { (anonce, snonce) } else { (snonce, anonce) };
    data[12..44].copy_from_slice(lo_n);
    data[44..76].copy_from_slice(hi_n);
    let mut out = [0u8; 48];
    prf(pmk, b"Pairwise key expansion", &data, &mut out);
    out
}

/// EAPOL-Key MIC for WPA2-CCMP (key-descriptor version 2 = HMAC-SHA1-128):
/// HMAC-SHA1(KCK, eapol) truncated to 16 bytes. The caller must zero the 16-byte
/// MIC field inside `eapol` before computing (the MIC covers itself as zeros).
pub fn eapol_mic(kck: &[u8], eapol: &[u8]) -> [u8; 16] {
    let full = hmac_sha1(kck, eapol);
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&full[..16]);
    mic
}

/// Known-answer self-test (call once at init; logs PASS/FAIL).
pub fn self_test() -> bool {
    // SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
    let s = sha1(b"abc");
    let sha_ok = s == [
        0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
        0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
    ];
    // IEEE 802.11i: PMK(pass="password", ssid="IEEE") =
    // f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e
    let p = pmk(b"password", b"IEEE");
    let pmk_ok = p == [
        0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a,
        0x5f, 0x90, 0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e,
        0x97, 0x10, 0xa1, 0x2e,
    ];
    // PTK + EAPOL-MIC KAT. Inputs are a fixed test vector; expected outputs were
    // computed with Python's reference HMAC-SHA1 (cross-implementation check, so
    // this catches byte-order / min-max / label / off-by-one bugs in ptk()/prf()
    // — a round trip wouldn't). PMK reused from the IEEE vector above.
    let aa = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let spa = [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb];
    let mut anonce = [0u8; 32];
    let mut snonce = [0u8; 32];
    for i in 0..32 {
        anonce[i] = (i + 1) as u8;   // 0x01..0x20
        snonce[i] = (0xa0 + i) as u8; // 0xa0..0xbf
    }
    let t = ptk(&p, &aa, &spa, &anonce, &snonce);
    let ptk_ok = t == [
        0x4a, 0x7d, 0x0f, 0x9a, 0xd3, 0x0e, 0xf8, 0x50, 0x33, 0x15, 0x80, 0x26, 0x63, 0x82,
        0x77, 0xe3, 0xd0, 0x04, 0xf2, 0x77, 0x50, 0x31, 0x81, 0xfb, 0x64, 0x99, 0x62, 0x58,
        0xe8, 0xfc, 0x54, 0x39, 0xb8, 0x9d, 0xdf, 0x67, 0xeb, 0xbd, 0xb6, 0xe7, 0x47, 0x13,
        0x12, 0x0c, 0x72, 0x8b, 0x2d, 0xb7,
    ];
    let mic = eapol_mic(&t[..16], b"EAPOL-Key 4-way handshake KAT frame (mic zeroed)");
    let mic_ok = mic == [
        0xb7, 0x83, 0x25, 0x1c, 0x70, 0xf5, 0xc8, 0xaa, 0xaa, 0x57, 0x91, 0x08, 0x69, 0xc1,
        0x89, 0xcb,
    ];
    crate::println!("[wpa2] self-test: SHA1 {}, PMK {}, PTK {}, EAPOL-MIC {}",
        if sha_ok { "PASS" } else { "FAIL" },
        if pmk_ok { "PASS" } else { "FAIL" },
        if ptk_ok { "PASS" } else { "FAIL" },
        if mic_ok { "PASS" } else { "FAIL" });
    sha_ok && pmk_ok && ptk_ok && mic_ok
}
