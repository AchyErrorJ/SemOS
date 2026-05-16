//! ECDSA verify over NIST P-256 (secp256r1) per FIPS 186-4 §6.4.
//!
//! Verify-only. There's no signing path; the kernel never holds the
//! private key, only verifies signatures on certificates (the TLS 1.3
//! server CertificateVerify and the cert chain itself).
//!
//! # Why this matters
//! P-256 is the most common signature algorithm in modern HTTPS certs
//! (the agent brief found Anthropic's edge is likely ECDSA-P256-fronted;
//! confirm with `openssl s_client -connect ... -showcerts` before
//! committing to ECDSA-only in the TLS layer). With this primitive in
//! place, the TLS 1.3 client crypto surface is complete: ChaCha20-
//! Poly1305 + SHA-256 + HMAC + HKDF + X25519 + ECDSA-P256-verify.
//!
//! # Approach
//! - **Field arithmetic** over GF(p256) via CIOS Montgomery multiplication
//!   with 4 × u64 limbs. p_inv = 1 (because p256's low limb is `-1` mod
//!   `2^64`), which makes the algorithm slightly cleaner than the general
//!   case.
//! - **Scalar arithmetic** over GF(n256) with the same Montgomery
//!   structure (different modulus, different `n_inv`).
//! - **Point arithmetic** in Jacobian projective coordinates (X, Y, Z)
//!   representing affine (X/Z², Y/Z³). Cheaper than affine for chains.
//! - **Scalar multiplication** via plain double-and-add. Not constant-
//!   time; verify operates on public inputs so timing leaks don't matter.
//!
//! # Surface
//! [`verify_p256`] is the only public function. It takes:
//! - A 65-byte uncompressed public key (`0x04 || x || y`)
//! - A 32-byte message hash (typically SHA-256(message))
//! - A 32-byte `r` and 32-byte `s` from the signature
//!
//! Returns `true` iff the signature is valid for the key+hash, per
//! the FIPS 186-4 §6.4.2 verify algorithm.
//!
//! # Caveats
//! - **No DER parsing**: signatures arrive in DER inside X.509 certs
//!   (`ECDSA-Sig-Value ::= SEQUENCE { r INTEGER, s INTEGER }`). The
//!   caller (TLS / X.509 layer) parses DER and hands us raw `(r, s)`
//!   as 32-byte big-endian values. Keeps this module out of the
//!   ASN.1 minefield.
//! - **Hash truncation**: for P-256 with SHA-256, `qlen == outlen == 256`
//!   so the hash is used directly (no truncation needed). If a different
//!   hash were used, the caller would truncate to leftmost 256 bits.
//! - **Public-key validation**: we check the point is on the curve and
//!   isn't the identity. We don't check it's in the prime-order subgroup
//!   — P-256's cofactor is 1, so every on-curve point is automatically
//!   in the right subgroup.

// ============================================================================
// Constants
// ============================================================================

/// P-256 prime modulus p = 2^256 - 2^224 + 2^192 + 2^96 - 1.
/// Limbs are little-endian u64 (limb 0 = bits 0..63).
const P: [u64; 4] = [
    0xFFFF_FFFF_FFFF_FFFF,
    0x0000_0000_FFFF_FFFF,
    0x0000_0000_0000_0000,
    0xFFFF_FFFF_0000_0001,
];

/// P-256 group order n.
const N: [u64; 4] = [
    0xF3B9_CAC2_FC63_2551,
    0xBCE6_FAAD_A717_9E84,
    0xFFFF_FFFF_FFFF_FFFF,
    0xFFFF_FFFF_0000_0000,
];

/// `-p^(-1) mod 2^64` for the Montgomery reduction over Fp.
/// p's low limb is `-1 mod 2^64`, so the inverse is just `1`.
const P_INV: u64 = 1;

/// `-n^(-1) mod 2^64` for Montgomery reduction over Fn.
/// Verified once in tests against `mont_mul(R_N, 1) == 1` round-trip.
const N_INV: u64 = 0xCCD1_C8AA_EE00_BC4F;

/// `R mod p` where `R = 2^256`. Used by `Fp::from_bytes` after conversion.
const R_MOD_P: [u64; 4] = [
    0x0000_0000_0000_0001,
    0xFFFF_FFFF_0000_0000,
    0xFFFF_FFFF_FFFF_FFFF,
    0x0000_0000_FFFF_FFFE,
];

/// `R^2 mod p` — used to convert into Montgomery form via `mont_mul(a, R²) == a*R`.
/// Verified at test time against a brute-force computation (256 modular
/// doublings of `R_MOD_P`).
const R2_MOD_P: [u64; 4] = [
    0x0000_0000_0000_0003,
    0xFFFF_FFFB_FFFF_FFFF,
    0xFFFF_FFFF_FFFF_FFFE,
    0x0000_0004_FFFF_FFFD,
];

/// `R^2 mod n` — used to convert into scalar Montgomery form.
const R2_MOD_N: [u64; 4] = [
    0x83244c95be79eea2,
    0x4699799c49bd6fa6,
    0x2845b2392b6bec59,
    0x66e12d94f3d95620,
];

/// Generator G of P-256, x-coordinate (raw integer, NOT Montgomery form).
/// Big-endian: `6B17D1F2_E12C4247_F8BCE6E5_63A440F2_77037D81_2DEB33A0_F4A13945_D898C296`.
const G_X: [u8; 32] = [
    0x6B, 0x17, 0xD1, 0xF2, 0xE1, 0x2C, 0x42, 0x47,
    0xF8, 0xBC, 0xE6, 0xE5, 0x63, 0xA4, 0x40, 0xF2,
    0x77, 0x03, 0x7D, 0x81, 0x2D, 0xEB, 0x33, 0xA0,
    0xF4, 0xA1, 0x39, 0x45, 0xD8, 0x98, 0xC2, 0x96,
];

/// Generator G of P-256, y-coordinate (big-endian).
const G_Y: [u8; 32] = [
    0x4F, 0xE3, 0x42, 0xE2, 0xFE, 0x1A, 0x7F, 0x9B,
    0x8E, 0xE7, 0xEB, 0x4A, 0x7C, 0x0F, 0x9E, 0x16,
    0x2B, 0xCE, 0x33, 0x57, 0x6B, 0x31, 0x5E, 0xCE,
    0xCB, 0xB6, 0x40, 0x68, 0x37, 0xBF, 0x51, 0xF5,
];

/// Curve coefficient `b` (big-endian).
const B_BYTES: [u8; 32] = [
    0x5A, 0xC6, 0x35, 0xD8, 0xAA, 0x3A, 0x93, 0xE7,
    0xB3, 0xEB, 0xBD, 0x55, 0x76, 0x98, 0x86, 0xBC,
    0x65, 0x1D, 0x06, 0xB0, 0xCC, 0x53, 0xB0, 0xF6,
    0x3B, 0xCE, 0x3C, 0x3E, 0x27, 0xD2, 0x60, 0x4B,
];

// ============================================================================
// Big-integer helpers (4-limb little-endian)
// ============================================================================

/// `a >= b` for 4-limb little-endian integers.
fn ge(a: &[u64; 4], b: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if a[i] != b[i] { return a[i] > b[i]; }
    }
    true
}

/// `a == 0`.
fn is_zero(a: &[u64; 4]) -> bool {
    a[0] | a[1] | a[2] | a[3] == 0
}

/// Conditional subtract: `a -= modulus` if `a >= modulus`. Used to bring
/// Montgomery results into canonical form when they sit in [p, 2p).
fn cond_sub(a: &mut [u64; 4], modulus: &[u64; 4]) {
    if ge(a, modulus) {
        let mut borrow: u64 = 0;
        for i in 0..4 {
            let (s1, b1) = a[i].overflowing_sub(modulus[i]);
            let (s2, b2) = s1.overflowing_sub(borrow);
            a[i] = s2;
            borrow = (b1 as u64) | (b2 as u64);
        }
    }
}

/// Mod-modulus add: `(a + b) mod modulus`. Inputs assumed to be < modulus.
fn add_mod(a: &[u64; 4], b: &[u64; 4], modulus: &[u64; 4]) -> [u64; 4] {
    let mut r = [0u64; 4];
    let mut carry: u64 = 0;
    for i in 0..4 {
        let s = (a[i] as u128) + (b[i] as u128) + (carry as u128);
        r[i] = s as u64;
        carry = (s >> 64) as u64;
    }
    // If carry or r >= modulus, subtract once.
    if carry != 0 || ge(&r, modulus) {
        let mut borrow: u64 = 0;
        for i in 0..4 {
            let (s1, b1) = r[i].overflowing_sub(modulus[i]);
            let (s2, b2) = s1.overflowing_sub(borrow);
            r[i] = s2;
            borrow = (b1 as u64) | (b2 as u64);
        }
    }
    r
}

/// Mod-modulus subtract: `(a - b) mod modulus`. Inputs < modulus.
fn sub_mod(a: &[u64; 4], b: &[u64; 4], modulus: &[u64; 4]) -> [u64; 4] {
    let mut r = [0u64; 4];
    let mut borrow: u64 = 0;
    for i in 0..4 {
        let (s1, b1) = a[i].overflowing_sub(b[i]);
        let (s2, b2) = s1.overflowing_sub(borrow);
        r[i] = s2;
        borrow = (b1 as u64) | (b2 as u64);
    }
    // If borrow, we underflowed — add modulus back.
    if borrow != 0 {
        let mut carry: u64 = 0;
        for i in 0..4 {
            let s = (r[i] as u128) + (modulus[i] as u128) + (carry as u128);
            r[i] = s as u64;
            carry = (s >> 64) as u64;
        }
    }
    r
}

/// CIOS Montgomery multiplication. Computes `a * b * R^(-1) mod modulus`
/// where `R = 2^256`. Both `a` and `b` are in Montgomery form (so the
/// result is `(a/R * b/R) * R = (a*b)/R`, which is also in Montgomery form).
///
/// `mod_inv` is `-modulus^(-1) mod 2^64`.
fn mont_mul(a: &[u64; 4], b: &[u64; 4], modulus: &[u64; 4], mod_inv: u64) -> [u64; 4] {
    let mut t = [0u64; 5];
    for i in 0..4 {
        // First inner loop: t = t + a[i] * b
        let mut carry: u64 = 0;
        for j in 0..4 {
            let prod = (a[i] as u128) * (b[j] as u128)
                     + (t[j] as u128) + (carry as u128);
            t[j] = prod as u64;
            carry = (prod >> 64) as u64;
        }
        let (sum, ov1) = t[4].overflowing_add(carry);
        t[4] = sum;
        let top_after_mul = ov1;

        // m = t[0] * mod_inv mod 2^64
        let m = t[0].wrapping_mul(mod_inv);

        // Second inner loop: t = (t + m * modulus); t[0] becomes 0; then shift right by 64.
        let mut carry: u64 = 0;
        for j in 0..4 {
            let prod = (m as u128) * (modulus[j] as u128)
                     + (t[j] as u128) + (carry as u128);
            t[j] = prod as u64;
            carry = (prod >> 64) as u64;
        }
        let (sum, ov2) = t[4].overflowing_add(carry);
        t[4] = sum;
        // Shift right by one word (t[0] is provably 0 by choice of m).
        for j in 0..4 { t[j] = t[j+1]; }
        t[4] = (top_after_mul as u64) + (ov2 as u64);
    }

    let mut result = [t[0], t[1], t[2], t[3]];
    // Final reduction: result might be in [modulus, 2*modulus). One conditional
    // subtract suffices given the CIOS bound. If t[4] != 0 we also need to subtract.
    if t[4] != 0 || ge(&result, modulus) {
        let mut borrow: u64 = 0;
        for i in 0..4 {
            let (s1, b1) = result[i].overflowing_sub(modulus[i]);
            let (s2, b2) = s1.overflowing_sub(borrow);
            result[i] = s2;
            borrow = (b1 as u64) | (b2 as u64);
        }
    }
    result
}

/// Convert big-endian 32-byte to 4 LE u64 limbs.
fn bytes_be_to_limbs(b: &[u8; 32]) -> [u64; 4] {
    let mut limbs = [0u64; 4];
    for i in 0..4 {
        let base = 24 - i * 8; // limb 0 is the LOW limb, so it comes from the LAST 8 bytes
        limbs[i] = u64::from_be_bytes([
            b[base], b[base+1], b[base+2], b[base+3],
            b[base+4], b[base+5], b[base+6], b[base+7],
        ]);
    }
    limbs
}

/// Convert 4 LE u64 limbs to big-endian 32-byte array.
fn limbs_to_bytes_be(l: &[u64; 4]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..4 {
        let base = 24 - i * 8;
        out[base..base+8].copy_from_slice(&l[i].to_be_bytes());
    }
    out
}

// ============================================================================
// Fp — field GF(p256) in Montgomery form
// ============================================================================

/// Field element mod p256, internally in Montgomery form (`actual_value * R mod p`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Fp([u64; 4]);

impl Fp {
    const ZERO: Fp = Fp([0; 4]);
    const ONE: Fp  = Fp(R_MOD_P); // 1 in Montgomery form is R mod p

    /// Parse a big-endian 32-byte value as a field element, converting
    /// to Montgomery form. Returns `None` if the value is >= p (not a
    /// valid field element — common rejection on bad cert input).
    fn from_bytes_be(b: &[u8; 32]) -> Option<Fp> {
        let v = bytes_be_to_limbs(b);
        if ge(&v, &P) { return None; }
        // mont(v) = v * R mod p = mont_mul(v, R²)
        Some(Fp(mont_mul(&v, &R2_MOD_P, &P, P_INV)))
    }

    /// Convert this field element from Montgomery form to a big-endian
    /// 32-byte value.
    fn to_bytes_be(&self) -> [u8; 32] {
        // mont_mul(self, 1) = self * 1 * R^-1 = self/R = actual value
        let one_raw = [1u64, 0, 0, 0];
        let v = mont_mul(&self.0, &one_raw, &P, P_INV);
        limbs_to_bytes_be(&v)
    }

    fn add(self, other: Fp) -> Fp { Fp(add_mod(&self.0, &other.0, &P)) }
    fn sub(self, other: Fp) -> Fp { Fp(sub_mod(&self.0, &other.0, &P)) }
    fn mul(self, other: Fp) -> Fp { Fp(mont_mul(&self.0, &other.0, &P, P_INV)) }
    fn sq(self) -> Fp { self.mul(self) }
    fn neg(self) -> Fp { Fp(sub_mod(&[0;4], &self.0, &P)) }
    fn is_zero(&self) -> bool { is_zero(&self.0) }

    /// Modular inverse via Fermat: `a^(p-2) mod p`. Square-and-multiply
    /// over the 256-bit exponent. ~256 squarings + ~256 multiplications.
    fn inv(self) -> Fp {
        // p - 2 in big-endian: FFFFFFFF 00000001 00000000 00000000 00000000 FFFFFFFF FFFFFFFF FFFFFFFD
        let exp: [u8; 32] = [
            0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFD,
        ];
        let mut result = Fp::ONE;
        // Square-and-multiply, MSB first.
        for &byte in exp.iter() {
            for bit in (0..8).rev() {
                result = result.sq();
                if (byte >> bit) & 1 == 1 {
                    result = result.mul(self);
                }
            }
        }
        result
    }
}

// ============================================================================
// Scalar — field GF(n256) in Montgomery form
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Scalar([u64; 4]);

impl Scalar {
    /// Parse a big-endian 32-byte signature component. Returns `None` if
    /// the value is 0 or >= n — matches the FIPS 186-4 §6.4.2 requirement
    /// that signature `(r, s)` both be in `[1, n-1]`.
    fn from_bytes_be(b: &[u8; 32]) -> Option<Scalar> {
        let v = bytes_be_to_limbs(b);
        if is_zero(&v) || ge(&v, &N) { return None; }
        Some(Scalar(mont_mul(&v, &R2_MOD_N, &N, N_INV)))
    }

    /// Parse a hash as a scalar. Per FIPS 186-4 §6.4.2 step 5: take the
    /// integer value of the hash and reduce mod n. Doesn't reject zero
    /// or out-of-range values — those are mathematically valid even if
    /// silly (a hash collision to zero is astronomically unlikely).
    fn from_hash_be(b: &[u8; 32]) -> Scalar {
        let mut v = bytes_be_to_limbs(b);
        // Reduce mod n if needed.
        cond_sub(&mut v, &N);
        Scalar(mont_mul(&v, &R2_MOD_N, &N, N_INV))
    }

    fn mul(self, other: Scalar) -> Scalar {
        Scalar(mont_mul(&self.0, &other.0, &N, N_INV))
    }

    /// Inverse via Fermat: `a^(n-2) mod n`. Square-and-multiply.
    fn inv(self) -> Scalar {
        // n - 2 in big-endian
        let exp: [u8; 32] = [
            0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xBC, 0xE6, 0xFA, 0xAD, 0xA7, 0x17, 0x9E, 0x84,
            0xF3, 0xB9, 0xCA, 0xC2, 0xFC, 0x63, 0x25, 0x4F,
        ];
        let mut result = Scalar(mont_mul(&[1, 0, 0, 0], &R2_MOD_N, &N, N_INV));
        for &byte in exp.iter() {
            for bit in (0..8).rev() {
                result = result.mul(result);
                if (byte >> bit) & 1 == 1 {
                    result = result.mul(self);
                }
            }
        }
        result
    }

    /// Convert from Montgomery form back to a big-endian 32-byte integer.
    fn to_bytes_be(&self) -> [u8; 32] {
        let v = mont_mul(&self.0, &[1, 0, 0, 0], &N, N_INV);
        limbs_to_bytes_be(&v)
    }
}

// ============================================================================
// Jacobian point arithmetic
// ============================================================================

/// Point in Jacobian projective coordinates representing affine
/// `(X/Z², Y/Z³)`. The "identity" / point-at-infinity is `Z == 0`.
#[derive(Clone, Copy, Debug)]
struct Point {
    x: Fp,
    y: Fp,
    z: Fp,
}

impl Point {
    /// Point at infinity (identity for the group law).
    /// Use (0, 1, 0) which is the standard convention.
    const IDENTITY: Point = Point { x: Fp::ZERO, y: Fp::ONE, z: Fp::ZERO };

    fn is_identity(&self) -> bool { self.z.is_zero() }

    /// Build a Jacobian point from affine `(x, y)`. Z = 1.
    fn from_affine(x: Fp, y: Fp) -> Point { Point { x, y, z: Fp::ONE } }

    /// Convert to affine. Returns `None` if this is the identity.
    fn to_affine(&self) -> Option<(Fp, Fp)> {
        if self.is_identity() { return None; }
        let z_inv = self.z.inv();
        let z_inv2 = z_inv.sq();
        let z_inv3 = z_inv2.mul(z_inv);
        Some((self.x.mul(z_inv2), self.y.mul(z_inv3)))
    }

    /// Check the affine projection (x, y) satisfies `y² = x³ - 3x + b`
    /// over GF(p256). Cheap rejection of garbage public keys.
    fn is_on_curve(&self) -> bool {
        match self.to_affine() {
            None => false, // identity isn't usefully "on" the curve here
            Some((x, y)) => {
                let b = match Fp::from_bytes_be(&B_BYTES) {
                    Some(b) => b,
                    None => return false,
                };
                let three = Fp::ONE.add(Fp::ONE).add(Fp::ONE);
                let lhs = y.sq();
                let rhs = x.sq().mul(x).sub(three.mul(x)).add(b);
                lhs == rhs
            }
        }
    }

    /// Point doubling using the `a = -3` shortcut formulas (RFC 6090
    /// Appendix F.5 / Hankerson Algorithm 3.21).
    fn double(&self) -> Point {
        if self.is_identity() { return Point::IDENTITY; }
        // delta = Z²
        let delta = self.z.sq();
        // gamma = Y²
        let gamma = self.y.sq();
        // beta = X*gamma
        let beta = self.x.mul(gamma);
        // alpha = 3*(X - delta)*(X + delta)
        let three = Fp::ONE.add(Fp::ONE).add(Fp::ONE);
        let alpha = three.mul(self.x.sub(delta)).mul(self.x.add(delta));
        // X' = alpha² - 8*beta
        let two = Fp::ONE.add(Fp::ONE);
        let four = two.add(two);
        let eight = four.add(four);
        let x3 = alpha.sq().sub(eight.mul(beta));
        // Z' = (Y + Z)² - gamma - delta
        let z3 = self.y.add(self.z).sq().sub(gamma).sub(delta);
        // Y' = alpha*(4*beta - X') - 8*gamma²
        let y3 = alpha.mul(four.mul(beta).sub(x3)).sub(eight.mul(gamma.sq()));
        Point { x: x3, y: y3, z: z3 }
    }

    /// Point addition in Jacobian coordinates. Falls through to `double`
    /// when both inputs are the same affine point.
    fn add(&self, other: &Point) -> Point {
        if self.is_identity() { return *other; }
        if other.is_identity() { return *self; }

        // U1 = X1 * Z2², U2 = X2 * Z1²
        let z1_sq = self.z.sq();
        let z2_sq = other.z.sq();
        let u1 = self.x.mul(z2_sq);
        let u2 = other.x.mul(z1_sq);
        // S1 = Y1 * Z2³, S2 = Y2 * Z1³
        let s1 = self.y.mul(z2_sq.mul(other.z));
        let s2 = other.y.mul(z1_sq.mul(self.z));

        if u1 == u2 {
            // Same x. Either same point (double) or P + (-P) = identity.
            if s1 == s2 { return self.double(); }
            return Point::IDENTITY;
        }

        let h = u2.sub(u1);
        let r = s2.sub(s1);
        let h_sq = h.sq();
        let h_cu = h_sq.mul(h);
        let u1_h_sq = u1.mul(h_sq);
        let two = Fp::ONE.add(Fp::ONE);

        // X3 = r² - h³ - 2*U1*h²
        let x3 = r.sq().sub(h_cu).sub(two.mul(u1_h_sq));
        // Y3 = r*(U1*h² - X3) - S1*h³
        let y3 = r.mul(u1_h_sq.sub(x3)).sub(s1.mul(h_cu));
        // Z3 = Z1 * Z2 * h
        let z3 = self.z.mul(other.z).mul(h);

        Point { x: x3, y: y3, z: z3 }
    }

    /// Scalar multiplication via plain double-and-add. Not constant-time;
    /// adequate for verify (public inputs). Reads the scalar in
    /// Montgomery form, converts back, then iterates MSB-first over its
    /// integer bytes.
    fn scalar_mul(&self, k: &Scalar) -> Point {
        let k_bytes = k.to_bytes_be(); // big-endian integer
        let mut result = Point::IDENTITY;
        for &byte in k_bytes.iter() {
            for bit in (0..8).rev() {
                result = result.double();
                if (byte >> bit) & 1 == 1 {
                    result = result.add(self);
                }
            }
        }
        result
    }
}

// ============================================================================
// Public API — ECDSA verify per FIPS 186-4 §6.4.2
// ============================================================================

/// Verify an ECDSA-P256 signature.
///
/// # Arguments
/// - `pubkey_uncompressed`: 65 bytes, `0x04 || x (32B) || y (32B)`.
///   This is the standard uncompressed point encoding (SEC1 §2.3.3).
/// - `msg_hash`: 32-byte message hash (typically SHA-256(message)).
/// - `r`, `s`: signature components, 32 bytes each, big-endian.
///   Caller is responsible for parsing them out of the DER-encoded
///   `ECDSA-Sig-Value` structure used in X.509 / TLS.
///
/// Returns `true` iff the signature verifies. Rejects on:
/// - Wrong-length / non-uncompressed public key
/// - Public key not on the curve
/// - `r` or `s` outside `[1, n-1]`
/// - Computed `R` point is the identity
/// - `x_R mod n != r`
pub fn verify_p256(
    pubkey_uncompressed: &[u8; 65],
    msg_hash: &[u8; 32],
    r: &[u8; 32],
    s: &[u8; 32],
) -> bool {
    // SEC1 uncompressed format check.
    if pubkey_uncompressed[0] != 0x04 { return false; }
    let mut qx_bytes = [0u8; 32];
    let mut qy_bytes = [0u8; 32];
    qx_bytes.copy_from_slice(&pubkey_uncompressed[1..33]);
    qy_bytes.copy_from_slice(&pubkey_uncompressed[33..65]);

    let qx = match Fp::from_bytes_be(&qx_bytes) { Some(v) => v, None => return false };
    let qy = match Fp::from_bytes_be(&qy_bytes) { Some(v) => v, None => return false };
    let q = Point::from_affine(qx, qy);

    // Public-key validation: must be on the curve and not the identity.
    if !q.is_on_curve() { return false; }

    // Scalar parse — also rejects 0 and >= n.
    let r_scalar = match Scalar::from_bytes_be(r) { Some(v) => v, None => return false };
    let s_scalar = match Scalar::from_bytes_be(s) { Some(v) => v, None => return false };

    // z = leftmost min(qlen, hashlen) bits of the hash. For P-256 with
    // SHA-256, both are 256 bits — no truncation, just convert to scalar.
    let z = Scalar::from_hash_be(msg_hash);

    // w = s^-1 mod n
    let w = s_scalar.inv();
    // u1 = z * w mod n; u2 = r * w mod n
    let u1 = z.mul(w);
    let u2 = r_scalar.mul(w);

    // Compute R = u1*G + u2*Q
    let gx = match Fp::from_bytes_be(&G_X) { Some(v) => v, None => return false };
    let gy = match Fp::from_bytes_be(&G_Y) { Some(v) => v, None => return false };
    let g = Point::from_affine(gx, gy);
    let p_sum = g.scalar_mul(&u1).add(&q.scalar_mul(&u2));

    if p_sum.is_identity() { return false; }
    let (px, _) = match p_sum.to_affine() { Some(v) => v, None => return false };

    // Check x(R) mod n == r.
    let px_bytes = px.to_bytes_be();
    let mut px_int = bytes_be_to_limbs(&px_bytes);
    cond_sub(&mut px_int, &N);
    let r_int = bytes_be_to_limbs(r);
    px_int == r_int
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::sha256;

    fn hex_to_32(s: &str) -> [u8; 32] {
        let bytes = s.as_bytes();
        assert_eq!(bytes.len(), 64);
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = (nibble(bytes[i*2]) << 4) | nibble(bytes[i*2 + 1]);
        }
        out
    }

    fn hex_to_65(s: &str) -> [u8; 65] {
        let bytes = s.as_bytes();
        assert_eq!(bytes.len(), 130);
        let mut out = [0u8; 65];
        for i in 0..65 {
            out[i] = (nibble(bytes[i*2]) << 4) | nibble(bytes[i*2 + 1]);
        }
        out
    }

    fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex"),
        }
    }

    // ----- Constants sanity -----

    #[test]
    fn p_inv_satisfies_invariant() {
        // -p^-1 mod 2^64: low limb of p is 0xFFFFFFFFFFFFFFFF = -1 mod 2^64.
        // So -p^-1 should be 1. Verify: p[0] * P_INV ≡ -1 (mod 2^64).
        let prod = P[0].wrapping_mul(P_INV);
        assert_eq!(prod, 0xFFFF_FFFF_FFFF_FFFF, "P_INV is wrong");
    }

    /// Slow-but-obviously-correct R² mod p computed by repeated modular
    /// doubling of R_MOD_P (256 iterations). After 256 doublings,
    /// `R_MOD_P * 2^256 mod p = R * R mod p = R² mod p`. Each doubling
    /// is mod-p arithmetic: when `2x` overflows bit 256, the true value
    /// is `2^256 + low`, whose reduction mod p is `R_MOD_P + low` (since
    /// `2^256 ≡ R_MOD_P (mod p)`). Then one more conditional subtract
    /// handles the case where the sum exceeds p.
    fn r_squared_brute() -> [u64; 4] {
        let mut x = R_MOD_P;
        for _ in 0..256 {
            let new_top = x[3] >> 63;
            let mut new_x = [
                x[0] << 1,
                (x[1] << 1) | (x[0] >> 63),
                (x[2] << 1) | (x[1] >> 63),
                (x[3] << 1) | (x[2] >> 63),
            ];

            if new_top == 1 {
                // Add R_MOD_P (= 2^256 mod p) into new_x, then possibly subtract p.
                let mut carry: u64 = 0;
                for i in 0..4 {
                    let sum = (new_x[i] as u128) + (R_MOD_P[i] as u128) + (carry as u128);
                    new_x[i] = sum as u64;
                    carry = (sum >> 64) as u64;
                }
                if carry != 0 || ge(&new_x, &P) {
                    let mut borrow: u64 = 0;
                    for i in 0..4 {
                        let (s1, b1) = new_x[i].overflowing_sub(P[i]);
                        let (s2, b2) = s1.overflowing_sub(borrow);
                        new_x[i] = s2;
                        borrow = (b1 as u64) | (b2 as u64);
                    }
                }
            } else if ge(&new_x, &P) {
                let mut borrow: u64 = 0;
                for i in 0..4 {
                    let (s1, b1) = new_x[i].overflowing_sub(P[i]);
                    let (s2, b2) = s1.overflowing_sub(borrow);
                    new_x[i] = s2;
                    borrow = (b1 as u64) | (b2 as u64);
                }
            }
            x = new_x;
        }
        x
    }

    #[test]
    fn r2_mod_p_constant_matches_brute_force() {
        let brute = r_squared_brute();
        assert_eq!(brute, R2_MOD_P,
            "R2_MOD_P hardcoded value doesn't match brute-force computation.\n  hardcoded = {:#x?}\n  brute     = {:#x?}",
            R2_MOD_P, brute);
    }

    #[test]
    fn mont_mul_one_by_r_squared_gives_r() {
        // mont_mul(1, R²) = 1 * R² / R = R = R_MOD_P. If this fails,
        // either R2_MOD_P is wrong (caught by previous test) or mont_mul
        // is wrong.
        let one_raw = [1u64, 0, 0, 0];
        let got = mont_mul(&one_raw, &R2_MOD_P, &P, P_INV);
        assert_eq!(got, R_MOD_P,
            "mont_mul(1, R²) should give R_MOD_P\n  expected = {:#x?}\n  got      = {:#x?}",
            R_MOD_P, got);
    }

    #[test]
    fn n_inv_satisfies_invariant() {
        // n[0] * N_INV ≡ -1 (mod 2^64)
        let prod = N[0].wrapping_mul(N_INV);
        assert_eq!(prod, 0xFFFF_FFFF_FFFF_FFFF, "N_INV is wrong");
    }

    // ----- Fp basic sanity -----

    #[test]
    fn fp_one_round_trip() {
        let one_bytes = {
            let mut b = [0u8; 32]; b[31] = 1; b
        };
        let one = Fp::from_bytes_be(&one_bytes).unwrap();
        assert_eq!(one.to_bytes_be(), one_bytes, "1 must round-trip");
        // 1 in Montgomery form is R mod p:
        assert_eq!(one, Fp::ONE);
    }

    #[test]
    fn fp_add_zero_and_sub_self() {
        let mut b = [0u8; 32]; b[31] = 42;
        let v = Fp::from_bytes_be(&b).unwrap();
        assert_eq!(v.add(Fp::ZERO), v);
        let mut zero_bytes = [0u8; 32];
        assert_eq!(v.sub(v).to_bytes_be(), zero_bytes);
        let _ = &mut zero_bytes;
    }

    #[test]
    fn fp_mul_inv() {
        // a * a^-1 == 1 for a non-zero a.
        let a_bytes = hex_to_32("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde0");
        let a = Fp::from_bytes_be(&a_bytes).unwrap();
        let prod = a.mul(a.inv());
        assert_eq!(prod, Fp::ONE);
    }

    #[test]
    fn scalar_mul_inv() {
        let a_bytes = hex_to_32("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        let a = Scalar::from_bytes_be(&a_bytes).unwrap();
        // a * a^-1 = 1
        let prod = a.mul(a.inv());
        let one_bytes = {
            let mut b = [0u8; 32]; b[31] = 1; b
        };
        assert_eq!(prod.to_bytes_be(), one_bytes);
    }

    // ----- Generator and curve sanity -----

    #[test]
    fn generator_is_on_curve() {
        let gx = Fp::from_bytes_be(&G_X).unwrap();
        let gy = Fp::from_bytes_be(&G_Y).unwrap();
        let g = Point::from_affine(gx, gy);
        assert!(g.is_on_curve(), "generator must satisfy curve equation");
    }

    #[test]
    fn generator_double_then_to_affine_is_2g() {
        // Known: 2*G x-coordinate (from any P-256 reference):
        //   7CF27B188D034F7E8A52380304B51AC3C08969E277F21B35A60B48FC47669978
        let gx = Fp::from_bytes_be(&G_X).unwrap();
        let gy = Fp::from_bytes_be(&G_Y).unwrap();
        let two_g = Point::from_affine(gx, gy).double();
        let (x2, _) = two_g.to_affine().unwrap();
        let want = hex_to_32("7cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc47669978");
        assert_eq!(x2.to_bytes_be(), want, "2*G x-coordinate mismatch");
    }

    #[test]
    fn generator_add_then_double_is_3g() {
        // 3*G x-coordinate:
        //   5ECBE4D1A6330A44C8F7EF951D4BF165E6C6B721EFADA985FB41661BC6E7FD6C
        let gx = Fp::from_bytes_be(&G_X).unwrap();
        let gy = Fp::from_bytes_be(&G_Y).unwrap();
        let g  = Point::from_affine(gx, gy);
        let two_g = g.double();
        let three_g = two_g.add(&g);
        let (x3, _) = three_g.to_affine().unwrap();
        let want = hex_to_32("5ecbe4d1a6330a44c8f7ef951d4bf165e6c6b721efada985fb41661bc6e7fd6c");
        assert_eq!(x3.to_bytes_be(), want, "3*G x-coordinate mismatch");
    }

    // ----- ECDSA verify against a Wycheproof valid signature -----

    #[test]
    fn ecdsa_p256_verify_wycheproof_valid() {
        // ecdsa_secp256r1_sha256_test.json, tcId=1 (valid).
        // msg = "313233343030" = ASCII "123400"
        // public key:
        //   x = 2927b10512bae3eddcfe467828128bad2903269919f7086069c8c4df6c732838
        //   y = c7787964eaac00e5921fb1498a60f4606766b3d9685001558d1a974e7341513e
        // signature:
        //   r = 2ba3a8be6b94d5ec80a6d9d1190a436effe50d85a1eee859b8cc6af9bd5c2e18
        //   s = 4cd60b855d442f5b3c7b11eb6c4e0ae7525fe710fab9aa7c77a67f79e6fadd76
        let pk = hex_to_65("042927b10512bae3eddcfe467828128bad2903269919f7086069c8c4df6c732838c7787964eaac00e5921fb1498a60f4606766b3d9685001558d1a974e7341513e");
        let hash = sha256::hash(b"123400");
        let r = hex_to_32("2ba3a8be6b94d5ec80a6d9d1190a436effe50d85a1eee859b8cc6af9bd5c2e18");
        let s = hex_to_32("4cd60b855d442f5b3c7b11eb6c4e0ae7525fe710fab9aa7c77a67f79e6fadd76");
        assert!(verify_p256(&pk, &hash, &r, &s),
            "Wycheproof valid signature must verify");
    }

    #[test]
    fn ecdsa_p256_rejects_tampered_signature() {
        let pk = hex_to_65("042927b10512bae3eddcfe467828128bad2903269919f7086069c8c4df6c732838c7787964eaac00e5921fb1498a60f4606766b3d9685001558d1a974e7341513e");
        let hash = sha256::hash(b"123400");
        let mut r = hex_to_32("2ba3a8be6b94d5ec80a6d9d1190a436effe50d85a1eee859b8cc6af9bd5c2e18");
        let s = hex_to_32("4cd60b855d442f5b3c7b11eb6c4e0ae7525fe710fab9aa7c77a67f79e6fadd76");
        // Flip a bit in r — must reject.
        r[31] ^= 1;
        assert!(!verify_p256(&pk, &hash, &r, &s), "tampered r must reject");
    }

    #[test]
    fn ecdsa_p256_rejects_wrong_message() {
        let pk = hex_to_65("042927b10512bae3eddcfe467828128bad2903269919f7086069c8c4df6c732838c7787964eaac00e5921fb1498a60f4606766b3d9685001558d1a974e7341513e");
        let wrong_hash = sha256::hash(b"different message");
        let r = hex_to_32("2ba3a8be6b94d5ec80a6d9d1190a436effe50d85a1eee859b8cc6af9bd5c2e18");
        let s = hex_to_32("4cd60b855d442f5b3c7b11eb6c4e0ae7525fe710fab9aa7c77a67f79e6fadd76");
        assert!(!verify_p256(&pk, &wrong_hash, &r, &s), "wrong message must reject");
    }

    #[test]
    fn ecdsa_p256_rejects_off_curve_pubkey() {
        // Take the valid pubkey, twiddle x; result is almost certainly off-curve.
        let mut pk = hex_to_65("042927b10512bae3eddcfe467828128bad2903269919f7086069c8c4df6c732838c7787964eaac00e5921fb1498a60f4606766b3d9685001558d1a974e7341513e");
        pk[32] ^= 1; // perturb the last byte of x
        let hash = sha256::hash(b"123400");
        let r = hex_to_32("2ba3a8be6b94d5ec80a6d9d1190a436effe50d85a1eee859b8cc6af9bd5c2e18");
        let s = hex_to_32("4cd60b855d442f5b3c7b11eb6c4e0ae7525fe710fab9aa7c77a67f79e6fadd76");
        assert!(!verify_p256(&pk, &hash, &r, &s), "off-curve pubkey must reject");
    }

    #[test]
    fn ecdsa_p256_rejects_zero_r_and_s() {
        let pk = hex_to_65("042927b10512bae3eddcfe467828128bad2903269919f7086069c8c4df6c732838c7787964eaac00e5921fb1498a60f4606766b3d9685001558d1a974e7341513e");
        let hash = sha256::hash(b"123400");
        let zero = [0u8; 32];
        let valid_s = hex_to_32("4cd60b855d442f5b3c7b11eb6c4e0ae7525fe710fab9aa7c77a67f79e6fadd76");
        assert!(!verify_p256(&pk, &hash, &zero, &valid_s), "r=0 must reject");
        let valid_r = hex_to_32("2ba3a8be6b94d5ec80a6d9d1190a436effe50d85a1eee859b8cc6af9bd5c2e18");
        assert!(!verify_p256(&pk, &hash, &valid_r, &zero), "s=0 must reject");
    }

    #[test]
    fn ecdsa_p256_rejects_compressed_pubkey() {
        // First byte 0x02 = compressed (we don't decompress).
        let mut pk = [0u8; 65]; pk[0] = 0x02;
        let hash = [0u8; 32];
        let r = [1u8; 32];
        let s = [1u8; 32];
        assert!(!verify_p256(&pk, &hash, &r, &s));
    }
}
