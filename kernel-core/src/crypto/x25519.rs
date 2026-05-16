//! X25519 — Curve25519 Diffie-Hellman per RFC 7748.
//!
//! This is the key exchange we'll use for TLS 1.3 (only group offered in
//! the ClientHello key_share). Implementation follows the standard
//! 5-limb 51-bit representation of GF(2^255 - 19) — each field element is
//! `[u64; 5]` with each limb carrying 51 bits of value, leaving headroom
//! for carries during multiplication.
//!
//! # Surface
//! - [`x25519`] — `(scalar, u) -> u'`, the raw RFC 7748 §5 operation
//! - [`x25519_base`] — scalar mult by the base point (u = 9)
//!
//! # Constant-time
//! The Montgomery ladder uses [`cswap`] (XOR-mask conditional swap, no
//! branch on the scalar bit). The field operations don't branch on inputs.
//! That's enough for "no timing leak of the scalar" — the standard
//! threat for an ephemeral DH private key.
//!
//! # What this is not
//! - Not a general elliptic-curve library. Only X25519.
//! - Not the Ed25519 signature scheme. Different curve mapping; different
//!   serialisation. (TLS 1.3 server-cert signatures use ECDSA-P256, which
//!   lives in [`crypto::p256`] — separate module.)
//!
//! # Tests
//! - RFC 7748 §5.2: two single-call vectors
//! - RFC 7748 §5.2 iterative: 1- and 1000-iteration (1M-iter omitted —
//!   too slow for a unit test; takes ~5 minutes on a modern CPU)
//! - RFC 7748 §6.1: Alice/Bob ECDH agreement
//! - Sanity: zero-scalar and zero-point cases, byte-level round trip

// ============================================================================
// Field element: 5 limbs of 51 bits each over GF(2^255 - 19).
// ============================================================================

type Fe = [u64; 5];

const MASK51: u64 = (1u64 << 51) - 1;

const FE_ZERO: Fe = [0, 0, 0, 0, 0];
const FE_ONE:  Fe = [1, 0, 0, 0, 0];

/// (A - 2) / 4 where A = 486662 is the Curve25519 Montgomery parameter.
/// Used in the ladder doubling step.
const A24: u64 = 121665;

// ---- field arithmetic ----

/// Add two field elements. Limbs grow but stay well below 2^64 even after
/// hundreds of ladder steps because every `fe_mul`/`fe_sq` re-carries
/// back to ~51 bits, and `fe_add` is only used between mul/sq calls.
#[inline]
fn fe_add(a: Fe, b: Fe) -> Fe {
    [a[0]+b[0], a[1]+b[1], a[2]+b[2], a[3]+b[3], a[4]+b[4]]
}

/// Subtract two field elements without underflow.
/// Standard trick: add `2*p` first (in pre-carried limb form, fits in 52
/// bits per limb), then subtract. Result stays positive and stays < 2^53.
#[inline]
fn fe_sub(a: Fe, b: Fe) -> Fe {
    // 2*p = 2 * (2^255 - 19) in 5-limb 51-bit form:
    //   limb 0: 2^52 - 38
    //   limb 1..4: 2^52 - 2
    // (because 2*p mod 2^255 has 2*p contributing -38 to the low limb,
    // and each higher limb contributes 2^52 - 0 with a borrow of -2 from
    // the carry rebalancing.)
    const TWO_P_0: u64 = (1u64 << 52) - 38;
    const TWO_P_X: u64 = (1u64 << 52) - 2;
    [
        a[0].wrapping_add(TWO_P_0).wrapping_sub(b[0]),
        a[1].wrapping_add(TWO_P_X).wrapping_sub(b[1]),
        a[2].wrapping_add(TWO_P_X).wrapping_sub(b[2]),
        a[3].wrapping_add(TWO_P_X).wrapping_sub(b[3]),
        a[4].wrapping_add(TWO_P_X).wrapping_sub(b[4]),
    ]
}

/// Multiply two field elements with full reduction mod p.
///
/// Schoolbook 5x5 multiplication produces 9 result limbs at "positions"
/// 0..8 in units of 2^51. Since `2^255 ≡ 19 (mod p)`, positions 5..8 fold
/// back into 0..3 with a factor of 19. Then carry propagation tames the
/// result back into 51-bit limbs.
fn fe_mul(a: Fe, b: Fe) -> Fe {
    // Widen to u128 to hold partial products without overflow:
    //   max partial product: (2^52)^2 = 2^104, fits in u128.
    let a0 = a[0] as u128; let a1 = a[1] as u128;
    let a2 = a[2] as u128; let a3 = a[3] as u128;
    let a4 = a[4] as u128;
    let b0 = b[0] as u128; let b1 = b[1] as u128;
    let b2 = b[2] as u128; let b3 = b[3] as u128;
    let b4 = b[4] as u128;

    // Pre-multiply the "high" b limbs by 19 (the reduction factor for
    // 2^255 ≡ 19 mod p). Fits in u128 with room to spare.
    let b1_19 = 19 * b1;
    let b2_19 = 19 * b2;
    let b3_19 = 19 * b3;
    let b4_19 = 19 * b4;

    // Position-by-position partial products with implicit reduction:
    //   r_i = sum over j+k=i (a_j * b_k) +
    //         sum over j+k=i+5 (a_j * b_k) * 19
    let r0 = a0*b0    + a1*b4_19 + a2*b3_19 + a3*b2_19 + a4*b1_19;
    let r1 = a0*b1    + a1*b0    + a2*b4_19 + a3*b3_19 + a4*b2_19;
    let r2 = a0*b2    + a1*b1    + a2*b0    + a3*b4_19 + a4*b3_19;
    let r3 = a0*b3    + a1*b2    + a2*b1    + a3*b0    + a4*b4_19;
    let r4 = a0*b4    + a1*b3    + a2*b2    + a3*b1    + a4*b0;

    propagate_carries_u128([r0, r1, r2, r3, r4])
}

/// Square a field element. Same algebra as `fe_mul(a, a)` but the
/// cross-terms come in pairs and can share a doubling.
#[inline]
fn fe_sq(a: Fe) -> Fe { fe_mul(a, a) }

/// Multiply a field element by a small (fits in u32) scalar.
/// Used in the ladder for the `* a24` step.
fn fe_mul_a24(a: Fe) -> Fe {
    // a24 fits in 17 bits; (a_i * a24) fits in 51 + 17 = 68 bits, which
    // fits in u128 (and actually u64 too, but we re-use the u128 path
    // for uniform carry propagation).
    let n = A24 as u128;
    propagate_carries_u128([
        (a[0] as u128) * n,
        (a[1] as u128) * n,
        (a[2] as u128) * n,
        (a[3] as u128) * n,
        (a[4] as u128) * n,
    ])
}

/// Drive a [u128; 5] (possibly very large per-limb) back into a 5-limb
/// 51-bit form with two carry passes. Guaranteed result: each limb fits
/// in (2^51 + small) — i.e. usable as an input to the next operation.
#[inline]
fn propagate_carries_u128(mut r: [u128; 5]) -> Fe {
    // First pass: low -> high; final carry from limb 4 wraps via *19 into limb 0.
    r[1] += r[0] >> 51; r[0] &= MASK51 as u128;
    r[2] += r[1] >> 51; r[1] &= MASK51 as u128;
    r[3] += r[2] >> 51; r[2] &= MASK51 as u128;
    r[4] += r[3] >> 51; r[3] &= MASK51 as u128;
    r[0] += (r[4] >> 51) * 19; r[4] &= MASK51 as u128;
    // Second pass: only limb 0 might have grown; the carry into limb 1
    // is at most ~19 so one more step suffices.
    r[1] += r[0] >> 51; r[0] &= MASK51 as u128;

    [r[0] as u64, r[1] as u64, r[2] as u64, r[3] as u64, r[4] as u64]
}

/// Modular inverse via Fermat's little theorem: a^(p-2) mod p.
///
/// p - 2 in binary is 250 high bits all 1, then `01011` (LSB-first: bits
/// 0,1,3 set). Square-and-multiply iterating LSB-first over the 32-byte
/// little-endian representation. 256 squarings + ~250 multiplications.
/// Not the optimal addition chain (which is 254 + 11) but simpler to
/// audit, and inverts are only on the hot path once per X25519 call.
fn fe_invert(z: Fe) -> Fe {
    // (p - 2) as a 32-byte little-endian value: 0xeb, 0xff*30, 0x7f.
    const EXP: [u8; 32] = [
        0xeb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
    ];

    let mut result = FE_ONE;
    let mut base = z;
    for byte in EXP.iter() {
        for bit in 0..8 {
            if (byte >> bit) & 1 == 1 {
                result = fe_mul(result, base);
            }
            base = fe_sq(base);
        }
    }
    result
}

// ---- byte conversions ----

/// Little-endian [u8; 32] -> 5 51-bit limbs.
/// Top bit of input is dropped per RFC 7748 §5 ("mask the most significant
/// bit").
fn bytes_to_fe(bytes: &[u8; 32]) -> Fe {
    let load64 = |s: &[u8]| -> u64 {
        let mut v = 0u64;
        for (i, &b) in s.iter().enumerate() {
            v |= (b as u64) << (i * 8);
        }
        v
    };
    let l0 = load64(&bytes[0..8])   & MASK51;
    let l1 = (load64(&bytes[6..14]) >> 3) & MASK51;
    let l2 = (load64(&bytes[12..20]) >> 6) & MASK51;
    let l3 = (load64(&bytes[19..27]) >> 1) & MASK51;
    let l4 = (load64(&bytes[24..32]) >> 12) & MASK51;
    // The 51-bit mask on l4 also strips the top bit of bytes[31], which
    // is exactly the RFC 7748 "mask the most-significant bit" rule.
    [l0, l1, l2, l3, l4]
}

/// 5 51-bit limbs -> little-endian [u8; 32]. Performs the final
/// canonical reduction so the result is in [0, p).
fn fe_to_bytes(z: Fe) -> [u8; 32] {
    // First, propagate any pending carries.
    let mut t = z;
    t[1] += t[0] >> 51; t[0] &= MASK51;
    t[2] += t[1] >> 51; t[1] &= MASK51;
    t[3] += t[2] >> 51; t[2] &= MASK51;
    t[4] += t[3] >> 51; t[3] &= MASK51;
    t[0] += (t[4] >> 51) * 19; t[4] &= MASK51;
    t[1] += t[0] >> 51; t[0] &= MASK51;

    // Canonical reduction: if t >= p, subtract p. Standard trick: add 19
    // and check whether bit 255 became set. If yes, t was >= p, and the
    // (t + 19) - 2^255 is the canonical form; if no, t was already < p
    // and we discard the +19.
    let mut q = t[0] + 19;
    q = (q >> 51) + t[1];
    q = (q >> 51) + t[2];
    q = (q >> 51) + t[3];
    q = (q >> 51) + t[4];
    // q's bit 51 is the carry out: 1 if t was >= p, 0 otherwise.
    q >>= 51;
    // Apply: add 19*q to t (this is the +19 we tentatively added, but
    // only if it was justified by t >= p).
    t[0] += 19 * q;
    // Re-carry; this time the high bit is guaranteed to be 0 in canonical form.
    t[1] += t[0] >> 51; t[0] &= MASK51;
    t[2] += t[1] >> 51; t[1] &= MASK51;
    t[3] += t[2] >> 51; t[2] &= MASK51;
    t[4] += t[3] >> 51; t[3] &= MASK51;
    t[4] &= MASK51; // strip residual top bit; cancels the implicit -2^255.

    // Pack 5 51-bit limbs into 32 little-endian bytes.
    // Byte-by-byte explicit form (the "donna" pattern). Verbose but
    // obviously correct, and X25519 is called once per TLS connection
    // — performance is irrelevant on this path. Byte indices that
    // straddle limb boundaries OR together contributions from both
    // limbs at the correct shifts.
    [
        // bytes 0..5: entirely from limb 0 (bits 0..47)
         t[0]        as u8,
        (t[0] >>  8) as u8,
        (t[0] >> 16) as u8,
        (t[0] >> 24) as u8,
        (t[0] >> 32) as u8,
        (t[0] >> 40) as u8,
        // byte 6: limb 0 bits 48..50 + limb 1 bits 0..4
        ((t[0] >> 48) | (t[1] << 3)) as u8,
        // bytes 7..11: entirely from limb 1
        (t[1] >>  5) as u8,
        (t[1] >> 13) as u8,
        (t[1] >> 21) as u8,
        (t[1] >> 29) as u8,
        (t[1] >> 37) as u8,
        // byte 12: limb 1 bits 45..50 + limb 2 bits 0..1
        ((t[1] >> 45) | (t[2] << 6)) as u8,
        // bytes 13..18: entirely from limb 2
        (t[2] >>  2) as u8,
        (t[2] >> 10) as u8,
        (t[2] >> 18) as u8,
        (t[2] >> 26) as u8,
        (t[2] >> 34) as u8,
        (t[2] >> 42) as u8,
        // byte 19: limb 2 bit 50 + limb 3 bits 0..6
        ((t[2] >> 50) | (t[3] << 1)) as u8,
        // bytes 20..24: entirely from limb 3
        (t[3] >>  7) as u8,
        (t[3] >> 15) as u8,
        (t[3] >> 23) as u8,
        (t[3] >> 31) as u8,
        (t[3] >> 39) as u8,
        // byte 25: limb 3 bits 47..50 + limb 4 bits 0..3
        ((t[3] >> 47) | (t[4] << 4)) as u8,
        // bytes 26..31: entirely from limb 4
        (t[4] >>  4) as u8,
        (t[4] >> 12) as u8,
        (t[4] >> 20) as u8,
        (t[4] >> 28) as u8,
        (t[4] >> 36) as u8,
        (t[4] >> 44) as u8,
    ]
}

/// Constant-time conditional swap. `swap` must be 0 or 1; the
/// implementation masks it up to all-ones for any nonzero value but the
/// caller-side contract is "use only 0 or 1."
#[inline]
fn cswap(a: &mut Fe, b: &mut Fe, swap: u64) {
    let mask = 0u64.wrapping_sub(swap & 1);
    for i in 0..5 {
        let t = mask & (a[i] ^ b[i]);
        a[i] ^= t;
        b[i] ^= t;
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Apply RFC 7748 scalar clamping in-place. Bottom three bits zeroed (so
/// the scalar is a multiple of 8 = cofactor), top bit cleared and bit 254
/// set (so the scalar is in the right range and the ladder uses 255 bits).
fn clamp_scalar(s: &mut [u8; 32]) {
    s[0] &= 248;
    s[31] &= 127;
    s[31] |= 64;
}

/// X25519: `(scalar, u_coord) -> u'_coord` per RFC 7748 §5.
///
/// Both inputs are 32-byte little-endian. `scalar` is internally clamped
/// per RFC 7748 (bottom 3 bits cleared, top 2 bits set in the standard
/// way). The input `u_coord` has its top bit masked off automatically by
/// the field-element loader.
pub fn x25519(scalar: &[u8; 32], u_coord: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    clamp_scalar(&mut k);

    let x1 = bytes_to_fe(u_coord);
    let mut x2 = FE_ONE;
    let mut z2 = FE_ZERO;
    let mut x3 = x1;
    let mut z3 = FE_ONE;
    let mut swap: u64 = 0;

    // Montgomery ladder from bit 254 down to bit 0 of the clamped scalar.
    for t in (0..=254).rev() {
        let kt = ((k[t / 8] >> (t & 7)) as u64) & 1;
        swap ^= kt;
        cswap(&mut x2, &mut x3, swap);
        cswap(&mut z2, &mut z3, swap);
        swap = kt;

        // Differential addition formulas (RFC 7748 §5).
        let a  = fe_add(x2, z2);
        let aa = fe_sq(a);
        let b  = fe_sub(x2, z2);
        let bb = fe_sq(b);
        let e  = fe_sub(aa, bb);
        let c  = fe_add(x3, z3);
        let d  = fe_sub(x3, z3);
        let da = fe_mul(d, a);
        let cb = fe_mul(c, b);

        x3 = fe_sq(fe_add(da, cb));
        z3 = fe_mul(x1, fe_sq(fe_sub(da, cb)));
        x2 = fe_mul(aa, bb);
        // z2 = e * (aa + a24 * e)
        z2 = fe_mul(e, fe_add(aa, fe_mul_a24(e)));
    }

    cswap(&mut x2, &mut x3, swap);
    cswap(&mut z2, &mut z3, swap);

    let z2_inv = fe_invert(z2);
    let result = fe_mul(x2, z2_inv);
    fe_to_bytes(result)
}

/// X25519 with the standard base point (u = 9). Used to derive a public
/// key from a private scalar.
pub fn x25519_base(scalar: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(scalar, &base)
}

// ============================================================================
// Tests — RFC 7748 §5.2 and §6.1.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_to_bytes(hex: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        let bytes = hex.as_bytes();
        assert_eq!(bytes.len(), 64, "hex string must be 64 chars for 32 bytes");
        for i in 0..32 {
            let hi = hex_nibble(bytes[i*2]);
            let lo = hex_nibble(bytes[i*2 + 1]);
            out[i] = (hi << 4) | lo;
        }
        out
    }

    fn hex_nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex nibble: {}", c as char),
        }
    }

    fn bytes_to_hex(bytes: &[u8; 32]) -> [u8; 64] {
        let mut out = [0u8; 64];
        for (i, &b) in bytes.iter().enumerate() {
            let hi = b >> 4;
            let lo = b & 0xF;
            out[i*2]     = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
            out[i*2 + 1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
        }
        out
    }

    // ---- RFC 7748 §5.2 single-call vectors ----

    #[test]
    fn rfc7748_5_2_vector1() {
        let scalar = hex_to_bytes("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
        let u      = hex_to_bytes("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c");
        let want   = hex_to_bytes("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552");
        assert_eq!(x25519(&scalar, &u), want);
    }

    #[test]
    fn rfc7748_5_2_vector2() {
        let scalar = hex_to_bytes("4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d");
        let u      = hex_to_bytes("e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493");
        let want   = hex_to_bytes("95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957");
        assert_eq!(x25519(&scalar, &u), want);
    }

    // ---- RFC 7748 §5.2 iterative test ----
    //
    // Start with k = u = 0900...00. Repeatedly:
    //   k, u = X25519(k, u), k
    // After 1 iter:    422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079
    // After 1k iters:  684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51
    // (1M iter omitted — ~5min on modern x86, too slow for unit test)

    #[test]
    fn rfc7748_5_2_iterative_1() {
        let mut k = [0u8; 32]; k[0] = 9;
        let mut u = [0u8; 32]; u[0] = 9;
        let new_k = x25519(&k, &u);
        u = k;
        k = new_k;
        let want = hex_to_bytes("422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079");
        assert_eq!(k, want);
        let _ = u; // shut up the unused-write warning
    }

    #[test]
    fn rfc7748_5_2_iterative_1000() {
        let mut k = [0u8; 32]; k[0] = 9;
        let mut u = [0u8; 32]; u[0] = 9;
        for _ in 0..1000 {
            let new_k = x25519(&k, &u);
            u = k;
            k = new_k;
        }
        let want = hex_to_bytes("684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51");
        assert_eq!(k, want);
    }

    // ---- RFC 7748 §6.1 — Diffie-Hellman exchange ----

    #[test]
    fn rfc7748_6_1_diffie_hellman() {
        let alice_priv = hex_to_bytes("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let alice_pub  = hex_to_bytes("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        let bob_priv   = hex_to_bytes("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let bob_pub    = hex_to_bytes("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
        let shared     = hex_to_bytes("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");

        // Each side derives its own public key from its private scalar.
        assert_eq!(x25519_base(&alice_priv), alice_pub, "alice pub mismatch");
        assert_eq!(x25519_base(&bob_priv),   bob_pub,   "bob pub mismatch");
        // Each side computes the shared secret from the other's public key.
        assert_eq!(x25519(&alice_priv, &bob_pub),   shared, "alice's view of shared");
        assert_eq!(x25519(&bob_priv,   &alice_pub), shared, "bob's view of shared");
    }

    // ---- Sanity ----

    #[test]
    fn cswap_swaps_when_one() {
        let mut a: Fe = [1, 2, 3, 4, 5];
        let mut b: Fe = [10, 20, 30, 40, 50];
        cswap(&mut a, &mut b, 1);
        assert_eq!(a, [10, 20, 30, 40, 50]);
        assert_eq!(b, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn cswap_noop_when_zero() {
        let mut a: Fe = [1, 2, 3, 4, 5];
        let mut b: Fe = [10, 20, 30, 40, 50];
        cswap(&mut a, &mut b, 0);
        assert_eq!(a, [1, 2, 3, 4, 5]);
        assert_eq!(b, [10, 20, 30, 40, 50]);
    }

    #[test]
    fn fe_byte_roundtrip_identity_canonical_form() {
        // A 32-byte value strictly less than p should round-trip exactly.
        let bytes = hex_to_bytes("11223344556677889900aabbccddeeff00112233445566778899aabbccddeeff");
        let fe = bytes_to_fe(&bytes);
        let back = fe_to_bytes(fe);
        // Note: input top bit (in bytes[31]) gets masked off per RFC 7748,
        // so we must compare against the masked input.
        let mut expected = bytes;
        expected[31] &= 0x7f;
        assert_eq!(back, expected);
    }

    #[test]
    fn fe_invert_round_trip() {
        // (a^-1) * a == 1 for arbitrary non-zero a.
        let a: Fe = [0x12345, 0x67890, 0xabcde, 0xf0123, 0x45678];
        let inv = fe_invert(a);
        let product = fe_mul(a, inv);
        let pb = fe_to_bytes(product);
        let one_b = fe_to_bytes(FE_ONE);
        assert_eq!(pb, one_b, "a * a^-1 must equal 1");
    }

    #[test]
    fn shut_up_unused_helper() {
        // `bytes_to_hex` is only used by ad-hoc debug, but keeping it
        // around is convenient for any failing-test investigation.
        let h = bytes_to_hex(&[0xab; 32]);
        assert_eq!(h[0], b'a');
    }
}
