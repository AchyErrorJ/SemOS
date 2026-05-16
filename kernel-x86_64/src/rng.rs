//! Hardware random number generator using RDRAND.
//!
//! RDRAND is an x86_64 CPU instruction available on Intel since
//! Ivy Bridge (2012) and AMD since Excavator (2015). Returns
//! cryptographically-strong random bytes from the on-die HRNG.
//!
//! # Why this matters
//!
//! TLS 1.3 needs ~64 bytes of high-quality randomness per handshake:
//! 32 bytes for `ClientHello.random` and 32 bytes for the X25519
//! ephemeral private scalar. Any predictability here is catastrophic
//! — a guessable ephemeral scalar lets an attacker recover the
//! session key after the fact from a packet capture.
//!
//! Our fallback story is "panic, don't degrade." If `RDRAND` is
//! unavailable (e.g., very old CPU or QEMU `-cpu` without `+rdrand`),
//! `fill_bytes` returns `Err(())` and the caller refuses the operation.
//! Better than silently using `0` or a TSC-jitter approximation that
//! could be backed out of by an attacker.
//!
//! # Reliability
//!
//! RDRAND can fail (CF=0) under pathological entropy-pool conditions.
//! We retry up to [`RDRAND_RETRIES`] times per 64-bit word before giving
//! up — Intel's published guidance is "retry up to 10 times" which is
//! what we use.

use core::arch::x86_64::_rdrand64_step;

/// Number of times to retry RDRAND if it reports failure for a single
/// word. Intel's "Digital Random Number Generator Software
/// Implementation Guide" recommends 10.
const RDRAND_RETRIES: u32 = 10;

/// Fill `buf` with random bytes from RDRAND.
///
/// Returns `Ok(())` on success, `Err(())` if RDRAND repeatedly failed
/// to produce a value (which on modern hardware essentially never
/// happens — the entropy pool refills continuously).
///
/// Caller must have already confirmed RDRAND is supported (or be OK
/// with the `Err` path). We don't CPUID-check on every call.
pub fn fill_bytes(buf: &mut [u8]) -> Result<(), ()> {
    let mut i = 0;
    while i < buf.len() {
        let mut value: u64 = 0;
        let mut retries = RDRAND_RETRIES;
        let ok = loop {
            // SAFETY: `_rdrand64_step` is the standard wrapper around
            // the RDRAND instruction. It writes a u64 into `*out` and
            // returns 1 on success / 0 on transient failure.
            let r = unsafe { _rdrand64_step(&mut value) };
            if r == 1 { break true; }
            retries -= 1;
            if retries == 0 { break false; }
        };
        if !ok { return Err(()); }
        let take = (buf.len() - i).min(8);
        let bytes = value.to_le_bytes();
        buf[i..i + take].copy_from_slice(&bytes[..take]);
        i += take;
    }
    Ok(())
}

/// Probe whether RDRAND is actually available via CPUID leaf 1 ECX bit 30.
/// Called once at boot so we can log presence and fail fast on machines
/// without it (rather than silently always returning `Err(())` later).
pub fn supported() -> bool {
    // CPUID leaf 1: ECX bit 30 is the RDRAND feature flag. The `__cpuid`
    // intrinsic wraps the CPUID instruction and dodges LLVM's rbx
    // reservation that breaks naïve inline asm.
    let res = unsafe { core::arch::x86_64::__cpuid(1) };
    (res.ecx & (1 << 30)) != 0
}
