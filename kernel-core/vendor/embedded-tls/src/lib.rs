#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![doc = include_str!("../README.md")]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc // TODO
)]

/*!
# Example

```
use embedded_tls::*;
use embedded_io_adapters::tokio_1::FromTokio;
use rand::rngs::OsRng;
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let stream = TcpStream::connect("google.com:443")
        .await
        .expect("error creating TCP connection");

    println!("TCP connection opened");
    let mut read_record_buffer = [0; 16384];
    let mut write_record_buffer = [0; 16384];
    let config = TlsConfig::new().with_server_name("google.com").enable_rsa_signatures();
    let mut tls = TlsConnection::new(
        FromTokio::new(stream),
        &mut read_record_buffer,
        &mut write_record_buffer,
    );

    // Allows disabling cert verification, in case you are using PSK and don't need it, or are just testing.
    // otherwise, use embedded_tls::webpki::CertVerifier, which only works on std for now.
    tls.open(TlsContext::new(
        &config,
        UnsecureProvider::new::<Aes128GcmSha256>(OsRng),
    ))
    .await
    .expect("error establishing TLS connection");

    println!("TLS session opened");
}
```
*/

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

use parse_buffer::ParseError;
pub mod alert;
mod application_data;
pub mod blocking;
mod buffer;
mod change_cipher_spec;
mod cipher_suites;
mod common;
mod config;
mod connection;
mod content_types;
mod crypto_engine;
mod extensions;
pub mod flush_policy;
mod handshake;
mod key_schedule;
mod parse_buffer;
pub mod read_buffer;
mod record;
mod record_reader;
mod write_buffer;

pub use config::UnsecureProvider;
pub use extensions::extension_data::signature_algorithms::SignatureScheme;
pub use handshake::certificate_verify::CertificateVerify;
pub use rand_core::{CryptoRng, CryptoRngCore};

// VENDORED PATCH (Semantic OS): re-exports added to make types
// reachable from external `TlsVerifier` implementations + external
// CryptoProvider impls (kernel-core/src/tls/transport_tls.rs).
// Upstream leaves these reachable only through the (sealed)
// `mod handshake` and `mod config`. Same rationale as the
// `pub entries:` patch in handshake/certificate.rs — minimum-surface
// change, no logic touched. `TlsConfig`/`TlsContext`/`CryptoProvider`
// are already re-exported via `pub use asynch::*` (asynch.rs does
// `pub use crate::config::*;`), but we restate them explicitly here
// so an upstream re-vendor doesn't quietly drop them.
pub use config::{
    Certificate, CryptoProvider, NoSign, NoVerify, TlsConfig, TlsContext,
    TlsVerifier,
};
pub use handshake::certificate::{CertificateRef, CertificateEntryRef};
pub use handshake::certificate_verify::CertificateVerifyRef;

#[cfg(feature = "webpki")]
pub mod webpki;

#[cfg(feature = "rustpki")]
mod der_certificate;
#[cfg(feature = "rustpki")]
pub mod pki;

mod asynch;
pub use asynch::*;

pub use flush_policy::*;

#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum TlsError {
    ConnectionClosed,
    Unimplemented,
    MissingHandshake,
    HandshakeAborted(alert::AlertLevel, alert::AlertDescription),
    AbortHandshake(alert::AlertLevel, alert::AlertDescription),
    IoError,
    InternalError,
    InvalidRecord,
    UnknownContentType,
    InvalidNonceLength,
    InvalidTicketLength,
    UnknownExtensionType,
    InsufficientSpace,
    InvalidHandshake,
    InvalidCipherSuite,
    InvalidSignatureScheme,
    InvalidSignature,
    InvalidExtensionsLength,
    InvalidSessionIdLength,
    InvalidSupportedVersions,
    InvalidApplicationData,
    InvalidKeyShare,
    InvalidCertificate,
    InvalidCertificateEntry,
    InvalidCertificateRequest,
    InvalidPrivateKey,
    UnableToInitializeCryptoEngine,
    ParseError(ParseError),
    OutOfMemory,
    CryptoError,
    EncodeError,
    DecodeError,
    Io(embedded_io::ErrorKind),
}

impl embedded_io::Error for TlsError {
    fn kind(&self) -> embedded_io::ErrorKind {
        if let Self::Io(k) = self {
            *k
        } else {
            error!("TLS error: {:?}", self);
            embedded_io::ErrorKind::Other
        }
    }
}

impl core::fmt::Display for TlsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl core::error::Error for TlsError {}

#[cfg(feature = "std")]
mod stdlib {
    use crate::config::TlsClock;

    use std::time::SystemTime;
    impl TlsClock for SystemTime {
        fn now() -> Option<u64> {
            Some(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            )
        }
    }
}

/// An internal function to mark an unused value.
///
/// All calls to this should be removed before 1.x.
fn unused<T>(_: T) {}
