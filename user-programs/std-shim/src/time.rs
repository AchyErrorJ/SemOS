//! Minimal `std::time` — monotonic Instant + Duration over SYS_TIME.
//!
//! The kernel timer is 100 Hz (scheduler tick), so the underlying resolution
//! is 10 ms. This API mirrors the std shape closely enough that ports from
//! standard Rust code rarely need to change: `Instant::now()`,
//! `instant.elapsed()`, `Duration::from_secs / from_millis / as_secs /
//! as_millis`, and `Instant - Instant -> Duration` all behave the way a
//! caller expects.

use crate::arch::{syscall0, SYS_TIME};

/// Kernel scheduler tick rate. Matches `kernel_core::scheduler::TICK_HZ`.
const TICKS_PER_SEC: u64 = 100;
const MILLIS_PER_TICK: u128 = 10;

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

    pub const fn from_secs(s: u64) -> Self {
        Self { ticks: s.saturating_mul(TICKS_PER_SEC) }
    }

    pub const fn from_millis(m: u64) -> Self {
        // 10 ms per tick — round to nearest tick.
        Self { ticks: (m + 5) / 10 }
    }

    pub const fn as_secs(&self) -> u64 {
        self.ticks / TICKS_PER_SEC
    }

    pub const fn as_millis(&self) -> u128 {
        (self.ticks as u128) * MILLIS_PER_TICK
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
