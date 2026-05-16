# Vendored embedded-tls 0.18.0

Imported from crates.io v0.18.0 (`embedded-tls-0.18.0`) on 2026-05-16
for use by Semantic OS. Reason: implement an external `TlsVerifier`
that does SPKI pinning instead of full PKIX validation.

## Patch — single line, in `src/handshake/certificate.rs`

```diff
-    pub(crate) entries: Vec<CertificateEntryRef<'a>, 16>,
+    pub entries: Vec<CertificateEntryRef<'a>, 16>,
```

That's it. No logic changes, no signature changes, no layout changes.
With the field public, `kernel-core/src/tls/verifier.rs` can walk
the cert chain and hash each entry's SPKI against our pin.

## Re-vendoring procedure

When upstream releases a new version we care about:

```bash
# 1. Download fresh source
cd ~/.cargo/registry/src/index.crates.io-*/embedded-tls-<NEW>.0/

# 2. Overwrite vendor tree from there
rsync -a --delete . F:/Software/ArmKernel3/kernel-core/vendor/embedded-tls/

# 3. Re-apply the visibility patch (see diff above)

# 4. Rebuild kernel-core; run DEMO 13 in QEMU; if it passes, commit
```

## Why not contribute the patch upstream?

We could PR `pub` on `entries`, but upstream's design intent is
that the chain is opaque to external verifiers (they want you to
use their `webpki` integration). Our use case — SPKI pinning at
the kernel level — is unusual enough that landing the patch
upstream would take negotiation we don't need right now. Vendoring
ships the fix immediately and the diff stays trivially small if
we ever want to upstream later.

## License

embedded-tls is dual-licensed under MIT OR Apache-2.0. See `LICENSE`
in this directory. Vendoring + the one-line patch are compatible
with both terms.
