//! TLS 1.3 client glue between embedded-tls and kernel-core::crypto.
//!
//! This module's job is to satisfy embedded-tls's generic
//! [`TlsCipherSuite`](embedded_tls::TlsCipherSuite) bound with crypto
//! from our own [`crate::crypto`] modules, so the in-binary TLS path
//! uses *one* audited implementation per primitive rather than the
//! bundled `aes-gcm` / `sha2` / `p256` duplicates.
//!
//! See `docs/EMBEDDED_TLS_VENDORING_BRIEF.md` §3 for the swap plan and
//! §4 for the SPKI-pinning verifier design that will sit alongside.
//!
//! # Layout
//!
//! - [`crypto_shim`] — RustCrypto-trait wrappers (Digest, AeadInPlace, …)
//!   around our SHA-256 and ChaCha20-Poly1305.
//! - [`cipher_suite`] — `TlsCipherSuite` impl gluing the wrappers
//!   together into the `TLS_CHACHA20_POLY1305_SHA256` (code point
//!   `0x1303`) suite embedded-tls 0.17 doesn't ship.
//!
//! Coming next: `spki_pin.rs` (the SPKI-pinning `TlsVerifier`),
//! `transport_tls.rs` (`NetworkTransport` impl wrapping `TlsConnection`).

pub mod crypto_shim;
pub mod cipher_suite;
pub mod spki_pin;
