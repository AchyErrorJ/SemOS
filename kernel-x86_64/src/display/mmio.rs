//! iGPU display MMIO register access.
//!
//! Wraps the BAR0 region with uncached page-table attributes and provides
//! the volatile 32-bit read/write primitives used by backlight, native
//! modeset, and future display code.

use crate::{igpu, paging};

/// Whitelisted device for display MMIO writes.
const TARGET_DEVICE_ID: u16 = igpu::HASWELL_GT2_MOBILE_HD4600;

/// A thin, audited wrapper around the iGPU BAR0 virtual base address.
///
/// # Safety
/// The caller must ensure this is constructed only after the iGPU has been
/// probed and whitelisted. All register offsets are checked against BAR0 size.
pub struct MmioReg {
    base: u64,
    size: u64,
}

impl MmioReg {
    /// Create a new `MmioReg` for the first supported Intel display controller.
    /// Returns `None` if no supported GPU is found or BAR0 is not MMIO.
    pub fn new() -> Option<Self> {
        let info = igpu::find()?;
        if info.device_id != TARGET_DEVICE_ID {
            return None;
        }
        let (base_phys, size) = match info.bar0.kind {
            igpu::BarKind::Mmio32 { base, .. } => (base as u64, info.bar0.size),
            igpu::BarKind::Mmio64 { base, .. } => (base, info.bar0.size),
            _ => return None,
        };
        if size == 0 {
            return None;
        }

        // Ensure the BAR0 region is mapped uncached in the kernel physical map.
        // This is best-effort: if it fails we still return the wrapper so the
        // caller can decide whether to proceed (the legacy backlight path worked
        // without explicit cache attributes).
        let _ = paging::set_region_uncached(base_phys, size);

        Some(Self {
            base: paging::phys_to_virt(base_phys),
            size,
        })
    }

    /// Read a 32-bit register at `offset` bytes from BAR0 base.
    /// Out-of-range offsets return 0xFFFFFFFF as a safe sentinel.
    #[inline]
    pub fn read32(&self, offset: u64) -> u32 {
        if offset + 4 > self.size {
            return 0xFFFFFFFF;
        }
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    /// Write a 32-bit register at `offset` bytes from BAR0 base.
    /// Out-of-range offsets are silently dropped.
    #[inline]
    pub fn write32(&self, offset: u64, value: u32) {
        if offset + 4 > self.size {
            return;
        }
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }

    /// BAR0 size in bytes.
    pub fn size(&self) -> u64 { self.size }
}

/// Convenience: read a 32-bit register from the first supported GPU, or
/// return `None` if the GPU is absent.
pub fn read32(offset: u64) -> Option<u32> {
    Some(MmioReg::new()?.read32(offset))
}

/// Convenience: write a 32-bit register to the first supported GPU.
/// Returns `false` if no supported GPU is present.
pub fn write32(offset: u64, value: u32) -> bool {
    if let Some(mmio) = MmioReg::new() {
        mmio.write32(offset, value);
        true
    } else {
        false
    }
}
