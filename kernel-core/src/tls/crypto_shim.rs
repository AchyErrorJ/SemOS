//! RustCrypto-trait wrappers around our SHA-256 and ChaCha20-Poly1305.
//!
//! embedded-tls's `TlsCipherSuite` bound is shaped against the
//! RustCrypto trait ecosystem: `digest::Digest`, `aead::AeadInPlace`,
//! `aead::KeyInit`. Our `kernel_core::crypto` exposes plain functions
//! and small concrete types. This file bridges the two — one wrapper
//! type per primitive, with a small number of trait impls that delegate
//! straight through to our underlying code.
//!
//! Nothing here computes any crypto itself. If a hash or AEAD test
//! fails, the bug is in `crypto::sha256` / `crypto::chacha20` /
//! `crypto::poly1305`, not here.

use core::convert::TryInto;

use aead::{AeadCore, AeadInPlace, Key, KeyInit, KeySizeUser, Nonce, Tag};
use digest::{
    FixedOutput, FixedOutputReset, HashMarker, Output,
    OutputSizeUser, Reset, Update,
};
// BlockSizeUser lives in crypto-common; digest re-exports the crate but
// not the trait at its root in 0.10.
use crypto_common::BlockSizeUser;
use generic_array::GenericArray;
use generic_array::typenum::{U12, U16, U32, U64};

use crate::crypto::sha256::Sha256 as InnerSha256;
use crate::crypto::{chacha20, poly1305};

// ============================================================================
// SHA-256 wrapper — KernelSha256
// ============================================================================
//
// Implements the trait bag embedded-tls needs:
//   Digest + Reset + Clone + OutputSizeUser + BlockSizeUser + FixedOutput
//
// The `Digest` umbrella trait is auto-implemented by the blanket impl in
// the `digest` crate for any type that is `HashMarker + Update +
// FixedOutput + Default`. The other bounds we wire up explicitly.

/// `digest::Digest`-compatible SHA-256 backed by our `crypto::sha256`.
///
/// Wraps `InnerSha256` so we can hand the result to anything that
/// expects RustCrypto's hash interface, without exporting `InnerSha256`
/// publicly in a way that constrains its evolution.
#[derive(Clone, Default)]
pub struct KernelSha256 {
    inner: InnerSha256,
}

impl KernelSha256 {
    pub fn new() -> Self { Self::default() }
}

impl HashMarker for KernelSha256 {}

impl BlockSizeUser for KernelSha256 {
    type BlockSize = U64; // SHA-256 processes 512-bit (64-byte) blocks
}

impl OutputSizeUser for KernelSha256 {
    type OutputSize = U32; // 256-bit digest
}

impl Update for KernelSha256 {
    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }
}

impl FixedOutput for KernelSha256 {
    fn finalize_into(self, out: &mut Output<Self>) {
        let digest = self.inner.finalize();
        out.copy_from_slice(&digest);
    }
}

impl Reset for KernelSha256 {
    fn reset(&mut self) {
        self.inner = InnerSha256::new();
    }
}

impl FixedOutputReset for KernelSha256 {
    fn finalize_into_reset(&mut self, out: &mut Output<Self>) {
        // Clone-then-finalize avoids consuming `self`; our InnerSha256
        // is cheap to clone (just the 5 u32 state + bytes_processed +
        // partial-block buffer).
        let digest = self.inner.clone().finalize();
        out.copy_from_slice(&digest);
        self.reset();
    }
}

// One small ergonomic: BlockSizeUser + Update normally also impls
// `digest::core_api::UpdateCore` via blanket — we don't use that path
// (no `CoreWrapper`) but mentioning it here so a future change doesn't
// re-discover the gap.

// ============================================================================
// ChaCha20-Poly1305 wrapper — KernelChacha20Poly1305
// ============================================================================
//
// `aead::KeyInit::new(&Key)` is the constructor that takes a 32-byte key.
// `AeadInPlace::encrypt_in_place_detached(&self, nonce, aad, buffer)
//   -> Result<Tag, Error>`
// encrypts `buffer` in place and returns the 16-byte authentication tag.
// `decrypt_in_place_detached(&self, nonce, aad, buffer, tag)
//   -> Result<(), Error>`
// verifies and decrypts in place.
//
// Both wrap our existing `chacha20::chacha20_xor` + `poly1305` pieces.
// We compute the Poly1305 one-time key from ChaCha20 block 0 (per RFC
// 8439 §2.6), then XOR-encrypt with ChaCha20 starting at block 1, then
// authenticate AAD + ciphertext + lengths as the spec describes.

/// ChaCha20-Poly1305 AEAD as a RustCrypto-trait type.
///
/// Stores the 32-byte key as a `Key<Self>` (i.e., a `GenericArray<u8,
/// U32>`). Drop zeroises (via Clone's discipline — the underlying
/// `CryptoKey` does this in its own Drop, but here we hold raw bytes
/// to dodge a layer of indirection; if leak hygiene becomes critical,
/// switch back to wrapping `CryptoKey`).
pub struct KernelChacha20Poly1305 {
    key: [u8; 32],
}

impl KeySizeUser for KernelChacha20Poly1305 {
    type KeySize = U32;
}

impl KeyInit for KernelChacha20Poly1305 {
    fn new(key: &Key<Self>) -> Self {
        let mut k = [0u8; 32];
        k.copy_from_slice(key.as_slice());
        Self { key: k }
    }
}

impl AeadCore for KernelChacha20Poly1305 {
    type NonceSize = U12; // 96-bit nonce per RFC 8439
    type TagSize = U16;   // 128-bit Poly1305 tag
    type CiphertextOverhead = aead::generic_array::typenum::U0;
}

impl AeadInPlace for KernelChacha20Poly1305 {
    fn encrypt_in_place_detached(
        &self,
        nonce: &Nonce<Self>,
        associated_data: &[u8],
        buffer: &mut [u8],
    ) -> Result<Tag<Self>, aead::Error> {
        let key  = crate::crypto::CryptoKey::from_bytes(self.key);
        let nonce_bytes: &[u8; 12] = nonce.as_slice().try_into()
            .map_err(|_| aead::Error)?;
        let nonce_obj = crate::crypto::Nonce::from_bytes(*nonce_bytes);

        // Compute Poly1305 one-time key from ChaCha20 block 0.
        let mut poly_key = [0u8; 32];
        {
            let block_cipher = chacha20::ChaCha20::new(&key, &nonce_obj, 0);
            let block = block_cipher.block();
            poly_key.copy_from_slice(&block[..32]);
        }

        // Encrypt the buffer in place with ChaCha20 starting at counter 1.
        chacha20::chacha20_xor(&key, &nonce_obj, 1, buffer);

        // Compute the Poly1305 tag over (AAD || pad16, ciphertext || pad16,
        // aad_len_le_u64 || ct_len_le_u64) per RFC 8439 §2.8.
        let tag = poly1305_aead_tag(&poly_key, associated_data, buffer);

        // Wipe the one-time poly key. The CryptoKey wrapper would
        // handle its own destruction; we only own raw bytes here.
        for b in &mut poly_key { unsafe { core::ptr::write_volatile(b, 0); } }

        Ok(GenericArray::from(tag))
    }

    fn decrypt_in_place_detached(
        &self,
        nonce: &Nonce<Self>,
        associated_data: &[u8],
        buffer: &mut [u8],
        tag: &Tag<Self>,
    ) -> Result<(), aead::Error> {
        let key = crate::crypto::CryptoKey::from_bytes(self.key);
        let nonce_bytes: &[u8; 12] = nonce.as_slice().try_into()
            .map_err(|_| aead::Error)?;
        let nonce_obj = crate::crypto::Nonce::from_bytes(*nonce_bytes);

        // Same Poly1305 one-time key derivation as encrypt.
        let mut poly_key = [0u8; 32];
        {
            let block_cipher = chacha20::ChaCha20::new(&key, &nonce_obj, 0);
            let block = block_cipher.block();
            poly_key.copy_from_slice(&block[..32]);
        }

        // Compute the expected tag over the CIPHERTEXT (buffer is still
        // ciphertext at this point) and compare against the supplied tag.
        let expected = poly1305_aead_tag(&poly_key, associated_data, buffer);
        let provided: &[u8; 16] = tag.as_slice().try_into()
            .map_err(|_| aead::Error)?;
        // Constant-time tag comparison. subtle::ConstantTimeEq is the
        // idiomatic crate; we avoid pulling more dependencies by doing
        // the OR-then-non-zero pattern by hand.
        let mut diff: u8 = 0;
        for i in 0..16 { diff |= expected[i] ^ provided[i]; }

        // Wipe poly key regardless of outcome.
        for b in &mut poly_key { unsafe { core::ptr::write_volatile(b, 0); } }

        if diff != 0 {
            return Err(aead::Error);
        }

        // Tag valid — decrypt in place by re-running the same ChaCha20
        // XOR keystream (ChaCha20 is symmetric).
        chacha20::chacha20_xor(&key, &nonce_obj, 1, buffer);
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Poly1305 tag helper used by both encrypt and decrypt paths.
//
// Same shape as `poly1305::aead_encrypt`'s inner block, lifted out so we
// don't reach into a `pub fn` whose signature wasn't designed for this.
// Layout per RFC 8439 §2.8:
//
//     poly1305(aad || pad16, ciphertext || pad16, aad_len_le_u64 || ct_len_le_u64)
//
// where `pad16` pads to the next 16-byte boundary (zero bytes).
// ----------------------------------------------------------------------------

fn poly1305_aead_tag(poly_key: &[u8; 32], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut p = poly1305::Poly1305::new(poly_key);
    p.update(aad);
    if aad.len() % 16 != 0 {
        let pad = [0u8; 16];
        p.update(&pad[..16 - (aad.len() % 16)]);
    }
    p.update(ciphertext);
    if ciphertext.len() % 16 != 0 {
        let pad = [0u8; 16];
        p.update(&pad[..16 - (ciphertext.len() % 16)]);
    }
    let mut lens = [0u8; 16];
    lens[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lens[8..16].copy_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    p.update(&lens);
    p.finalize()
}

// ============================================================================
// Tests — verify the trait-surface roundtrip matches our existing crypto.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use digest::Digest;

    /// Hashing through the Digest trait surface should match
    /// `crypto::sha256::hash` byte-for-byte.
    #[test]
    fn sha256_trait_matches_inherent() {
        let msg: &[u8] = b"the quick brown fox jumps over the lazy dog";

        let mut h = KernelSha256::new();
        Digest::update(&mut h, msg);
        let trait_out: Output<KernelSha256> = h.finalize();

        let direct = crate::crypto::sha256::hash(msg);
        assert_eq!(trait_out.as_slice(), &direct[..]);
    }

    /// The Digest blanket impl provides `::digest(data)` as a one-shot.
    #[test]
    fn sha256_oneshot_via_digest() {
        let msg: &[u8] = b"abc";
        let out: Output<KernelSha256> = <KernelSha256 as Digest>::digest(msg);
        let direct = crate::crypto::sha256::hash(msg);
        assert_eq!(out.as_slice(), &direct[..]);
    }

    /// Reset + reuse: after finalize_into_reset, the hasher is fresh.
    #[test]
    fn sha256_finalize_reset_resets_state() {
        let mut h = KernelSha256::new();
        Digest::update(&mut h, b"first");
        let mut out = Output::<KernelSha256>::default();
        // Both Digest::finalize_into_reset and FixedOutputReset::finalize_into_reset
        // resolve here — name the one we want explicitly.
        FixedOutputReset::finalize_into_reset(&mut h, &mut out);
        assert_eq!(out.as_slice(), &crate::crypto::sha256::hash(b"first"));
        // h is now fresh; hashing "second" must equal sha256("second").
        Digest::update(&mut h, b"second");
        let out2: Output<KernelSha256> = h.finalize();
        assert_eq!(out2.as_slice(), &crate::crypto::sha256::hash(b"second"));
    }

    /// AEAD round-trip via the trait surface. Tampering with the
    /// ciphertext must cause decrypt_in_place_detached to fail.
    #[test]
    fn chacha20_poly1305_trait_roundtrip() {
        // RFC 8439 test vector key (32 bytes of 0..0x1f).
        let key_bytes: [u8; 32] = core::array::from_fn(|i| i as u8);
        let nonce_bytes: [u8; 12] = [
            0x07, 0x00, 0x00, 0x00,
            0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
        ];
        let aad = b"some authenticated data";
        let plaintext = b"Ladies and Gentlemen of the class of '99: \
                          If I could offer you only one tip for the future, \
                          sunscreen would be it.";

        let key = Key::<KernelChacha20Poly1305>::from(key_bytes);
        let nonce = Nonce::<KernelChacha20Poly1305>::from(nonce_bytes);
        let cipher = KernelChacha20Poly1305::new(&key);

        let mut buffer = [0u8; 256];
        buffer[..plaintext.len()].copy_from_slice(plaintext);
        let tag = cipher
            .encrypt_in_place_detached(&nonce, aad, &mut buffer[..plaintext.len()])
            .expect("encrypt");

        // Buffer must now be ciphertext, not the original plaintext.
        assert_ne!(&buffer[..plaintext.len()], &plaintext[..]);

        // Decrypt should restore the original.
        cipher
            .decrypt_in_place_detached(&nonce, aad, &mut buffer[..plaintext.len()], &tag)
            .expect("decrypt valid tag");
        assert_eq!(&buffer[..plaintext.len()], &plaintext[..]);
    }

    #[test]
    fn chacha20_poly1305_rejects_tampered_ciphertext() {
        let key_bytes = [0x42u8; 32];
        let nonce_bytes = [0x07u8; 12];
        let aad = b"";
        let plaintext = b"hello world, this is a longer message for variety";

        let key = Key::<KernelChacha20Poly1305>::from(key_bytes);
        let nonce = Nonce::<KernelChacha20Poly1305>::from(nonce_bytes);
        let cipher = KernelChacha20Poly1305::new(&key);

        let mut buf = [0u8; 128];
        buf[..plaintext.len()].copy_from_slice(plaintext);
        let tag = cipher
            .encrypt_in_place_detached(&nonce, aad, &mut buf[..plaintext.len()])
            .expect("encrypt");

        // Flip one bit in the ciphertext.
        buf[0] ^= 1;

        // Decrypt must reject.
        let result = cipher.decrypt_in_place_detached(
            &nonce, aad, &mut buf[..plaintext.len()], &tag,
        );
        assert!(result.is_err(), "tampered ciphertext must fail authentication");
    }

    #[test]
    fn chacha20_poly1305_rejects_tampered_aad() {
        let key_bytes = [0x99u8; 32];
        let nonce_bytes = [0x11u8; 12];
        let plaintext = b"some data";

        let key = Key::<KernelChacha20Poly1305>::from(key_bytes);
        let nonce = Nonce::<KernelChacha20Poly1305>::from(nonce_bytes);
        let cipher = KernelChacha20Poly1305::new(&key);

        let mut buf = [0u8; 64];
        buf[..plaintext.len()].copy_from_slice(plaintext);
        let tag = cipher
            .encrypt_in_place_detached(&nonce, b"original aad", &mut buf[..plaintext.len()])
            .expect("encrypt");

        // Try to decrypt with a different AAD — must fail.
        let result = cipher.decrypt_in_place_detached(
            &nonce, b"tampered aad", &mut buf[..plaintext.len()], &tag,
        );
        assert!(result.is_err(), "AAD tampering must fail authentication");
    }
}
