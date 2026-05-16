//! Monotonic clock adapter for smoltcp.
//!
//! smoltcp wants a [`smoltcp::time::Instant`] for every `poll` call. It
//! uses the deltas to drive TCP retransmission, ARP cache expiry, and
//! DHCP lease timing. The clock must be **monotonic** (never goes
//! backwards) and have at least millisecond granularity. Wall-clock-
//! ness doesn't matter — smoltcp only cares about elapsed time.
//!
//! We map [`crate::platform::ticks`] into milliseconds. The platform's
//! tick rate is set up at boot (APIC timer on x86_64); the conversion
//! constant lives here. If the tick rate changes, only this file does.

use smoltcp::time::Instant;

/// Platform tick rate. The x86_64 APIC timer is configured for
/// ~100 Hz today (one tick = 10 ms). If you re-tune the APIC init
/// count, update this constant — it's the *only* place the rate lives
/// in the network stack.
const TICK_MS: i64 = 10;

/// Current monotonic time as a smoltcp `Instant`. Safe to call from any
/// context; reads the platform tick counter.
pub fn now() -> Instant {
    let ticks = crate::platform::ticks() as i64;
    Instant::from_millis(ticks.saturating_mul(TICK_MS))
}
