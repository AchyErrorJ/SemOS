//! `TlsCipherSuite` impl that ties [`super::crypto_shim`]'s pieces
//! together into the cipher suite embedded-tls 0.17 doesn't ship.
//!
//! Code point `0x1303` — `TLS_CHACHA20_POLY1305_SHA256`. After this
//! lands, embedded-tls's `TlsConfig` parameterised on
//! [`Chacha20Poly1305Sha256`] will offer (and accept) only this suite.

use embedded_tls::TlsCipherSuite;
use generic_array::typenum::{Sum, U10, U32, U12};
use generic_array::ArrayLength;

/// `TLS_CHACHA20_POLY1305_SHA256` code point per RFC 8446 §B.4.
/// embedded-tls's own `CipherSuite` enum (with the same name `TlsChacha20Poly1305Sha256`
/// = `0x1303`) lives in a private module, so we restate the literal.
const TLS_CHACHA20_POLY1305_SHA256: u16 = 0x1303;

use super::crypto_shim::{KernelChacha20Poly1305, KernelSha256};

// Match the LabelBuffer / LongestLabel pattern from embedded-tls's
// own `Aes128GcmSha256` impl. The `LongestLabel` constant in upstream
// isn't `pub`, so we restate the size: the longest TLS 1.3 label string
// embedded-tls ever passes to `HKDF-Expand-Label` is `"derived"` /
// `"finished"` / `"resumption"` etc., all ≤ ~22 chars including the
// `"tls13 "` prefix. The +10 overhead covers the 2-byte length field
// plus the 1-byte label-length + 1-byte context-length bytes plus
// slack. 32 (hash output) + 22 + 10 = 64 — round up to U64 worst case.
//
// The actual `LabelBuffer<CipherSuite>` type in upstream config.rs is
// `Sum<Hash::OutputSize, Sum<LongestLabel, U10>>`, which for SHA-256
// (U32) and LongestLabel=U22 evaluates to U64. Mirror that.
type LongestLabel = generic_array::typenum::U22;
type LabelBuffer = Sum<U32, Sum<LongestLabel, U10>>;

/// `TLS_CHACHA20_POLY1305_SHA256` (RFC 8446 §B.4). Code point `0x1303`.
///
/// Bundles our SHA-256 (`KernelSha256`) and ChaCha20-Poly1305
/// (`KernelChacha20Poly1305`) shims for embedded-tls's generic
/// `TlsConfig` / `TlsConnection` to consume.
pub struct Chacha20Poly1305Sha256;

impl TlsCipherSuite for Chacha20Poly1305Sha256 {
    const CODE_POINT: u16 = TLS_CHACHA20_POLY1305_SHA256;

    type Cipher = KernelChacha20Poly1305;
    type KeyLen = U32;
    type IvLen  = U12;

    type Hash = KernelSha256;
    type LabelBufferSize = LabelBuffer;
}

// We don't expose any constructors — `TlsCipherSuite` is a type-level
// tag selected at TlsConfig construction time; it's never instantiated.
// `'static` is satisfied by the unit struct above; ArrayLength<u8> is
// already supplied for each typenum in scope.

// Static check: ArrayLength<u8> is what TlsCipherSuite requires. Force
// the trait bounds to be checked at compile time so a future typenum
// change blows up here, not deep inside embedded-tls.
const _: fn() = || {
    fn assert_array_length<T: ArrayLength<u8>>() {}
    assert_array_length::<<Chacha20Poly1305Sha256 as TlsCipherSuite>::KeyLen>();
    assert_array_length::<<Chacha20Poly1305Sha256 as TlsCipherSuite>::IvLen>();
    assert_array_length::<<Chacha20Poly1305Sha256 as TlsCipherSuite>::LabelBufferSize>();
};
