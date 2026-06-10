//! Minimal `std::time` — monotonic Instant + Duration over SYS_TIME.
//!
//! The kernel timer is 100 Hz (scheduler tick), so the underlying resolution
//! is 10 ms. This API mirrors the std shape closely enough that ports from
//! standard Rust code rarely need to change: `Instant::now()`,
//! `instant.elapsed()`, `Duration::from_secs / from_millis / as_secs /
//! as_millis`, and `Instant - Instant -> Duration` all behave the way a
//! caller expects.

use crate::arch::{syscall0, SYS_TIME};

/// Kernel scheduler tick rate, matching `kernel_core::scheduler::SCHEDULER_TICK_HZ`.
/// On QEMU's LAPIC this is the configured ~62.5 Hz (1 GHz / 16 / 1e6). On real
/// hardware the LAPIC bus clock varies and the actual rate may drift — same
/// caveat that lives in the kernel-core source-of-truth.
const TICKS_PER_SEC: u64 = 62;
const MILLIS_PER_TICK: u128 = 1000 / 62; // ~16 ms; loses precision but matches reality

/// A span of time, internally stored as ticks (10 ms granularity).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration {
    ticks: u64,
}

impl Duration {
    pub const ZERO: Self = Self { ticks: 0 };

    pub const fn from_ticks(t: u64) -> Self {
        Self { ticks: t }
    }

    /// std-compat: `Duration::new(secs, nanos)`. Nanos rounded to the
    /// nearest tick at our coarse 62 Hz resolution.
    pub const fn new(secs: u64, nanos: u32) -> Self {
        let nano_ticks = (nanos as u64 * TICKS_PER_SEC) / 1_000_000_000;
        Self { ticks: secs.saturating_mul(TICKS_PER_SEC).saturating_add(nano_ticks) }
    }

    pub const fn from_secs(s: u64) -> Self {
        Self { ticks: s.saturating_mul(TICKS_PER_SEC) }
    }

    pub const fn from_millis(m: u64) -> Self {
        // ~16 ms per tick @ 62 Hz — round to nearest tick.
        // ticks = m * 62 / 1000, rounded.
        Self { ticks: (m * TICKS_PER_SEC + 500) / 1000 }
    }

    pub const fn as_secs(&self) -> u64 {
        self.ticks / TICKS_PER_SEC
    }

    pub const fn as_millis(&self) -> u128 {
        (self.ticks as u128) * MILLIS_PER_TICK
    }

    /// std-compat: floating-point seconds. Used by rustc_data_structures::
    /// profiling::duration_to_secs_str for human-readable -Ztime-passes
    /// output.
    pub fn as_secs_f64(&self) -> f64 {
        (self.ticks as f64) / (TICKS_PER_SEC as f64)
    }

    pub const fn as_ticks(&self) -> u64 {
        self.ticks
    }

    pub const fn is_zero(&self) -> bool {
        self.ticks == 0
    }
}

impl core::ops::Add for Duration {
    type Output = Duration;
    fn add(self, rhs: Self) -> Duration {
        Duration { ticks: self.ticks.saturating_add(rhs.ticks) }
    }
}

impl core::ops::AddAssign for Duration {
    fn add_assign(&mut self, rhs: Self) {
        self.ticks = self.ticks.saturating_add(rhs.ticks);
    }
}

impl core::ops::SubAssign for Duration {
    fn sub_assign(&mut self, rhs: Self) {
        self.ticks = self.ticks.saturating_sub(rhs.ticks);
    }
}

impl core::ops::Sub for Duration {
    type Output = Duration;
    fn sub(self, rhs: Self) -> Duration {
        Duration { ticks: self.ticks.saturating_sub(rhs.ticks) }
    }
}

/// A monotonic point in time, backed by the kernel timer (SYS_TIME). Always
/// non-decreasing — safe to subtract earlier from later without wraparound
/// concerns (`saturating_sub` clamps to ZERO if mis-ordered).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    ticks: u64,
}

impl Instant {
    /// Sample the current kernel tick count.
    pub fn now() -> Self {
        Self { ticks: unsafe { syscall0(SYS_TIME) } }
    }

    /// Duration elapsed since `self` was taken.
    pub fn elapsed(&self) -> Duration {
        Self::now().duration_since(*self)
    }

    /// Duration from `earlier` up to `self`. Clamps to ZERO on mis-ordering.
    pub fn duration_since(&self, earlier: Self) -> Duration {
        Duration { ticks: self.ticks.saturating_sub(earlier.ticks) }
    }
}

impl core::ops::Sub for Instant {
    type Output = Duration;
    fn sub(self, rhs: Self) -> Duration {
        self.duration_since(rhs)
    }
}

impl core::ops::Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, rhs: Duration) -> Instant {
        Instant { ticks: self.ticks.saturating_add(rhs.ticks) }
    }
}
