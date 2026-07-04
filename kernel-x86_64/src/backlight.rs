//! Intel Haswell (HD 4600) PCH PWM backlight control — M14 step 2.
//!
//! This is a tiny, safe, targeted capability rather than a full display driver.
//! It drives the T540p internal eDP panel through the Lynx-Point PCH PWM path,
//! matching what Linux `i915` does on this machine.
//!
//! Safety rules from the M14 plan:
//! - Device whitelist: only Intel HD 4600 (`8086:0416`) is accepted for writes.
//! - Minimum brightness clamp: 10% floor so the panel never blanks.
//! - Save/restore: original `BLC_PWM_PCH_CTL2` is captured at init and can be
//!   restored explicitly.
//! - Only PCI memory space is enabled; bus mastering is left untouched.
//! - If CPU PWM is enabled, it is disabled before PCH override mode is used.

use crate::{igpu, pci, println};
use crate::display::mmio::MmioReg;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

pub const HASWELL_GT2_MOBILE_HD4600: u16 = igpu::HASWELL_GT2_MOBILE_HD4600;

// Register offsets relative to BAR0 (display MMIO).
const BLC_PWM_PCH_CTL1: u64 = 0xC8250;
const BLC_PWM_PCH_CTL2: u64 = 0xC8254;
const BLC_PWM_CPU_CTL2: u64 = 0x48250;

// Bitfields.
const BLM_PCH_PWM_ENABLE: u32 = 1 << 31;
const BLM_PCH_OVERRIDE_ENABLE: u32 = 1 << 30;
const BLM_PCH_POLARITY: u32 = 1 << 29;
const BLM_PWM_ENABLE: u32 = 1 << 31;

const MIN_BRIGHTNESS_PERCENT: u8 = 10;

/// Snapshot of the backlight state.
#[derive(Clone, Copy, Debug)]
pub struct BacklightState {
    pub percent: u8,
    pub raw_duty: u16,
    pub max_duty: u16,
}

// Global, lazily-initialized backlight state. Access is effectively single-
// threaded through the shell, so contention is low for M14.
static MMIO: Mutex<Option<MmioReg>> = Mutex::new(None);
static mut MAX_DUTY: u16 = 0;
static mut ORIGINAL_CTL2: u32 = 0;
static PROBED: AtomicBool = AtomicBool::new(false);

/// Initialize the backlight controller. Safe to call repeatedly; returns true
/// only on the T540p-class HD 4600 target. On other hardware or in QEMU this
/// is a harmless no-op that leaves `brightness_get/set` returning "unavailable".
pub fn init() -> bool {
    if PROBED.load(Ordering::Acquire) {
        return MMIO.lock().is_some();
    }

    let info = match igpu::find() {
        Some(i) if i.device_id == HASWELL_GT2_MOBILE_HD4600 => i,
        _ => {
            println!("[backlight] no supported Intel GPU — disabled");
            PROBED.store(true, Ordering::Release);
            return false;
        }
    };

    // Enable PCI memory space if firmware did not. Do not enable bus mastering.
    let command = pci::read_u16(info.loc.bus, info.loc.slot, info.loc.func, pci::regs::COMMAND);
    if command & pci::cmd::MEMORY_SPACE == 0 {
        let new = command | pci::cmd::MEMORY_SPACE;
        let status_cmd = pci::read_u32(info.loc.bus, info.loc.slot, info.loc.func, pci::regs::COMMAND);
        pci::write_u32(
            info.loc.bus,
            info.loc.slot,
            info.loc.func,
            pci::regs::COMMAND,
            (status_cmd & 0xFFFF_0000) | (new as u32),
        );
    }

    let mmio = match MmioReg::new() {
        Some(m) => m,
        None => {
            println!("[backlight] BAR0 is not MMIO — disabled");
            PROBED.store(true, Ordering::Release);
            return false;
        }
    };

    unsafe {
        ORIGINAL_CTL2 = mmio.read32(BLC_PWM_PCH_CTL2);
        MAX_DUTY = (ORIGINAL_CTL2 >> 16) as u16;
        *MMIO.lock() = Some(mmio);
    }

    println!(
        "[backlight] Intel HD 4600 PWM init: max_duty={} original_ctl2=0x{:08X}",
        unsafe { MAX_DUTY },
        unsafe { ORIGINAL_CTL2 }
    );
    PROBED.store(true, Ordering::Release);
    true
}

/// Return the current backlight state, or None if no supported controller.
pub fn get() -> Option<BacklightState> {
    if !init() {
        return None;
    }
    unsafe {
        let ctl2 = read_reg32(BLC_PWM_PCH_CTL2);
        let max = (ctl2 >> 16) as u16;
        let duty = (ctl2 & 0xFFFF) as u16;
        let max = if max == 0 { MAX_DUTY } else { max };
        let percent = if max == 0 {
            0
        } else {
            ((duty as u32 * 100) / max as u32) as u8
        };
        Some(BacklightState {
            percent,
            raw_duty: duty,
            max_duty: max,
        })
    }
}

/// Return just the current brightness percent, or None if unavailable.
pub fn get_percent() -> Option<u8> {
    get().map(|s| s.percent)
}

/// Set the backlight to `percent` (0-100), clamped to a visible floor.
/// Returns Ok(()) on success, Err on any error.
pub fn set_percent(percent: u8) -> Result<(), &'static str> {
    if !init() {
        return Err("backlight not available");
    }

    let clamped = percent.max(MIN_BRIGHTNESS_PERCENT).min(100);
    unsafe {
        let max = if MAX_DUTY == 0 {
            (read_reg32(BLC_PWM_PCH_CTL2) >> 16) as u16
        } else {
            MAX_DUTY
        };
        if max == 0 {
            return Err("invalid max duty");
        }

        let duty = ((clamped as u32 * max as u32) / 100) as u16;
        let new_ctl2 = ((max as u32) << 16) | (duty as u32);

        // Safe transition to PCH PWM override, matching Linux i915 lpt_pwm_funcs:
        // 1. Disable CPU PWM if it is enabled.
        let cpu_ctl2 = read_reg32(BLC_PWM_CPU_CTL2);
        if cpu_ctl2 & BLM_PWM_ENABLE != 0 {
            write_reg32(BLC_PWM_CPU_CTL2, cpu_ctl2 & !BLM_PWM_ENABLE);
        }

        // 2. Enable PCH override mode (preserve polarity).
        let pch_ctl1 = read_reg32(BLC_PWM_PCH_CTL1);
        write_reg32(BLC_PWM_PCH_CTL1, pch_ctl1 | BLM_PCH_OVERRIDE_ENABLE);

        // 3. Write the new duty cycle while preserving the frequency (upper 16 bits).
        write_reg32(BLC_PWM_PCH_CTL2, new_ctl2);

        // 4. Enable PCH PWM output.
        write_reg32(
            BLC_PWM_PCH_CTL1,
            (pch_ctl1 | BLM_PCH_OVERRIDE_ENABLE | BLM_PCH_PWM_ENABLE) & !BLM_PCH_POLARITY
                | (pch_ctl1 & BLM_PCH_POLARITY),
        );
    }

    println!("[backlight] set {}% (clamped from {}%)", clamped, percent);
    Ok(())
}

/// Restore the original backlight value captured at init.
pub fn restore() -> Result<(), &'static str> {
    if !init() {
        return Err("backlight not available");
    }
    unsafe {
        write_reg32(BLC_PWM_PCH_CTL2, ORIGINAL_CTL2);
    }
    println!("[backlight] restored original duty");
    Ok(())
}

#[inline]
unsafe fn read_reg32(off: u64) -> u32 {
    MMIO.lock().as_ref().map_or(0, |m| m.read32(off))
}

#[inline]
unsafe fn write_reg32(off: u64, v: u32) {
    if let Some(m) = MMIO.lock().as_ref() {
        m.write32(off, v);
    }
}
