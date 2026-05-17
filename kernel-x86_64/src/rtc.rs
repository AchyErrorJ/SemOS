//! MC146818 Real-Time Clock driver — wall-clock time for the kernel.
//!
//! Every x86 PC (real or emulated) since 1984 has a Motorola MC146818
//! battery-backed RTC at I/O ports 0x70 (index) and 0x71 (data). It
//! holds seconds / minutes / hours / day / month / year independently
//! of CPU power.
//!
//! # What this module gives us
//!
//! - [`read()`] — one-shot snapshot of date+time; UIP-race-safe.
//! - [`unix_time()`] — same snapshot, converted to seconds-since-epoch.
//! - `wall_clock()` glue (in `platform_impl.rs`) so kernel-core can ask
//!   "what time is it absolutely" without knowing about ports or BCD.
//!
//! # Why this matters (Phase 9 thread)
//!
//! - TLS `notAfter` validation needs absolute UTC time. Today our
//!   SPKI-pin verifier skips it (pin is the trust anchor); a real PKIX
//!   path would reject expired certs.
//! - File timestamps on Semantic Objects (`created_at`, `modified_at`)
//!   are currently 0; with this driver they get real values.
//! - The Marée / Brise utility apps are time-driven by definition.
//!
//! # Reading the RTC correctly
//!
//! Naive `read each register` races with the chip's internal update
//! cycle that runs once per second. If we catch it mid-update (Update-
//! In-Progress bit set in Status A), we can see internally-inconsistent
//! values like 11:59:60 wrapping mid-read.
//!
//! Two standard defences exist:
//!   1. Poll UIP=0, then read all registers, then re-poll UIP=0. If
//!      UIP went up between, retry.
//!   2. Read all registers TWICE; if both reads agree, take the result.
//!      Otherwise loop.
//!
//! We use #2 (the OSDev wiki "second method"). Simpler logic, same
//! correctness.
//!
//! # BCD vs binary
//!
//! Most BIOSes leave the RTC in BCD mode (bit 2 of Status B = 0). We
//! check Status B once and branch — supporting both costs ~30 LOC and
//! avoids "depends on the BIOS" footguns on real hardware.

use x86_64::instructions::port::Port;

// The kernel exports `println!` via `#[macro_export]` from main.rs; we
// reach it via the crate root for the boot-log line in `init_and_log`.
use crate::println;

/// I/O port for the RTC's index/command register.
const PORT_INDEX: u16 = 0x70;
/// I/O port for the RTC's data register.
const PORT_DATA: u16 = 0x71;

/// RTC register indices we care about.
mod reg {
    pub const SECONDS:      u8 = 0x00;
    pub const MINUTES:      u8 = 0x02;
    pub const HOURS:        u8 = 0x04;
    pub const DAY_OF_MONTH: u8 = 0x07;
    pub const MONTH:        u8 = 0x08;
    pub const YEAR:         u8 = 0x09;
    /// Optional century byte. Set by the BIOS in the ACPI FADT;
    /// most modern firmware uses 0x32. If your firmware doesn't,
    /// the year reading wraps every 100 years and we assume 2000+.
    pub const CENTURY:      u8 = 0x32;

    pub const STATUS_A: u8 = 0x0A;
    pub const STATUS_B: u8 = 0x0B;

    /// Status A bit 7 — Update In Progress.
    pub const STATUS_A_UIP: u8 = 0x80;

    /// Status B bit 1 — 24-hour mode (1) vs 12-hour (0).
    pub const STATUS_B_24H: u8 = 0x02;
    /// Status B bit 2 — data format: binary (1) vs BCD (0).
    pub const STATUS_B_BIN: u8 = 0x04;
}

// ============================================================================
// Low-level port I/O
// ============================================================================

/// Read one RTC register. The high bit of the index byte gates NMI:
/// we leave it CLEAR (NMI enabled) on each access. Some old chipsets
/// require setting bit 7 to disable NMI during access; QEMU and modern
/// hardware tolerate either, and disabling NMI here would mask other
/// kernel-level signals we want.
fn read_reg(index: u8) -> u8 {
    unsafe {
        let mut idx: Port<u8> = Port::new(PORT_INDEX);
        let mut data: Port<u8> = Port::new(PORT_DATA);
        idx.write(index);
        data.read()
    }
}

/// True if the RTC is mid-update — values not stable to read right now.
fn update_in_progress() -> bool {
    (read_reg(reg::STATUS_A) & reg::STATUS_A_UIP) != 0
}

/// BCD-to-binary conversion. `0x42` → `42`.
fn bcd_to_bin(v: u8) -> u8 { (v >> 4) * 10 + (v & 0x0F) }

// ============================================================================
// Snapshot
// ============================================================================

/// Date + time snapshot, all in UTC (the RTC isn't TZ-aware — what
/// it returns IS what's set, conventionally UTC on modern systems).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,    // full year, e.g. 2026
    pub month: u8,    // 1..=12
    pub day: u8,      // 1..=31
    pub hour: u8,     // 0..=23
    pub minute: u8,   // 0..=59
    pub second: u8,   // 0..=59
}

impl DateTime {
    /// Convert to seconds since Unix epoch (1970-01-01T00:00:00 UTC).
    /// No leap seconds — just civil time. Good for ~64-bit Y forever.
    pub fn to_unix_seconds(&self) -> u64 {
        // Days since epoch start of THIS year, then add days in this year.
        let days = days_from_epoch(self.year, self.month, self.day);
        let secs = (days as u64) * 86_400
            + (self.hour as u64) * 3_600
            + (self.minute as u64) * 60
            + (self.second as u64);
        secs
    }
}

/// Read date+time using the read-twice-and-compare technique. Returns
/// `None` if the chip is so unstable that 16 consecutive double-reads
/// can't produce two matching snapshots (basically: hardware is dead).
pub fn read() -> Option<DateTime> {
    // Bail-out budget. 16 attempts is wildly generous — a real chip
    // updates once per second, so the worst case is two reads landing
    // on opposite sides of the update window.
    for _ in 0..16 {
        // Wait for any in-progress update to settle. Bounded so a
        // wedged chip doesn't hang the kernel.
        for _ in 0..1_000_000 {
            if !update_in_progress() { break; }
            core::hint::spin_loop();
        }

        let a = raw_snapshot();

        // Wait again, then take a second read. If they agree, we win.
        for _ in 0..1_000_000 {
            if !update_in_progress() { break; }
            core::hint::spin_loop();
        }
        let b = raw_snapshot();
        if a == b {
            return Some(decode(b));
        }
    }
    None
}

/// Raw byte snapshot — no decoding yet. Used by [`read`] to compare
/// two consecutive reads for stability.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RawSnapshot {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
}

fn raw_snapshot() -> RawSnapshot {
    RawSnapshot {
        second:   read_reg(reg::SECONDS),
        minute:   read_reg(reg::MINUTES),
        hour:     read_reg(reg::HOURS),
        day:      read_reg(reg::DAY_OF_MONTH),
        month:    read_reg(reg::MONTH),
        year:     read_reg(reg::YEAR),
        century:  read_reg(reg::CENTURY),
        status_b: read_reg(reg::STATUS_B),
    }
}

/// Decode a stable RawSnapshot, handling BCD vs binary and 12h vs 24h.
fn decode(r: RawSnapshot) -> DateTime {
    let binary = (r.status_b & reg::STATUS_B_BIN) != 0;
    let mode_24h = (r.status_b & reg::STATUS_B_24H) != 0;

    // Convert each field from its raw form to plain binary.
    let mut hour = if binary { r.hour & 0x7F } else { bcd_to_bin(r.hour & 0x7F) };
    // The high bit of the hours register flags PM in 12-hour mode.
    let pm_flag = (r.hour & 0x80) != 0;
    if !mode_24h {
        // Convert 12-hour to 24-hour. 12 AM = 0, 12 PM = 12, 1-11 PM = +12.
        if hour == 12 { hour = 0; }
        if pm_flag { hour += 12; }
    }

    let second = if binary { r.second } else { bcd_to_bin(r.second) };
    let minute = if binary { r.minute } else { bcd_to_bin(r.minute) };
    let day    = if binary { r.day    } else { bcd_to_bin(r.day) };
    let month  = if binary { r.month  } else { bcd_to_bin(r.month) };
    let year_lo = if binary { r.year  } else { bcd_to_bin(r.year) };

    // Century: if the BIOS populates 0x32 (BCD), use it. If it's 0
    // (firmware doesn't set it — e.g. some QEMU configs), fall back to
    // 2000+. Once year_lo wraps to 99 → 00 we'd misinterpret as 2000
    // again; that's a problem in 2074 with this fallback path.
    let century_bcd_or_zero = r.century;
    let century_full = if century_bcd_or_zero == 0 {
        20u16 // 2000s assumption
    } else {
        bcd_to_bin(century_bcd_or_zero) as u16
    };
    let year = century_full * 100 + year_lo as u16;

    DateTime { year, month, day, hour, minute, second }
}

// ============================================================================
// Date arithmetic
// ============================================================================

/// Civil-time days from epoch (1970-01-01) to the given date. Handles
/// the Gregorian leap-year rule. Negative for pre-epoch dates (shouldn't
/// happen with a real RTC reading the present).
///
/// Algorithm: Howard Hinnant's "date algorithms" — well-known, branch-
/// light, integer-only. Adapted to u32 since we only deal with positive
/// dates in this kernel's lifetime.
fn days_from_epoch(year: u16, month: u8, day: u8) -> i64 {
    let y = year as i64 - if (month as i64) <= 2 { 1 } else { 0 };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    // Day-of-year, 0-indexed, with March=0 (Hinnant's shifted calendar).
    let m = month as i64;
    let m_shifted = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_shifted + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ============================================================================
// Public convenience
// ============================================================================

/// One-shot "what time is it" as Unix seconds. `None` if the RTC
/// read was unstable (see [`read`]).
pub fn unix_time() -> Option<u64> {
    read().map(|dt| dt.to_unix_seconds())
}

/// Probe the RTC at boot and log what we see. Idempotent; calling
/// it twice is fine.
pub fn init_and_log() {
    match read() {
        Some(dt) => println!(
            "[rtc] {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC  (unix={})",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second,
            dt.to_unix_seconds()
        ),
        None => println!("[rtc] read failed — wall_clock() will return None"),
    }
}

// ============================================================================
// Sanity tests — Hinnant's algorithm against a few known dates.
// kernel-x86_64 doesn't run `cargo test` (no_std bin), but we keep
// these here so a future host-test harness can validate them.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::days_from_epoch;

    #[test]
    fn epoch_is_day_zero() {
        assert_eq!(days_from_epoch(1970, 1, 1), 0);
    }
    #[test]
    fn y2k() {
        // 2000-01-01 = 10957 days after epoch
        assert_eq!(days_from_epoch(2000, 1, 1), 10957);
    }
    #[test]
    fn leap_day_2024() {
        // 2024-02-29 is a leap day; 2024-03-01 must be exactly the next day.
        let feb29 = days_from_epoch(2024, 2, 29);
        let mar01 = days_from_epoch(2024, 3, 1);
        assert_eq!(mar01 - feb29, 1);
    }
}
