//! iwlwifi CSR (Configuration and Status Registers) — M11 stage 2.
//!
//! BAR0 is a memory-mapped region exposed by the PCI device.  All
//! register offsets below are relative to BAR0.  The `Csr` struct
//! wraps the virtual address (obtained via `phys_to_virt`) and
//! provides typed read/write accessors.
//!
//! Register names follow Linux's `drivers/net/wireless/intel/iwlwifi/`
//! conventions so grep-friendly cross-referencing works.

use core::ptr::{read_volatile, write_volatile};

// ============================================================================
// Register offsets (relative to BAR0)
// ============================================================================

/// General-purpose control / HW interface config.
pub const CSR_HW_IF_CONFIG_REG:    u64 = 0x000;
pub const CSR_INT_COALESCING:      u64 = 0x004;
pub const CSR_HW_RF_ID:            u64 = 0x014;
pub const CSR_HW_REV:              u64 = 0x028;
pub const CSR_HW_RF_ID_TYPE:       u64 = 0x190;

/// GP (General Purpose) control — bit 1 = MAC clocks ready, bit 2 = init done.
pub const CSR_GP_CNTRL:            u64 = 0x020;

/// Reset register.
pub const CSR_RESET:               u64 = 0x020;

/// MAC shadow registers (for ucode download).
pub const CSR_MAC_SHADOW_REG_CTRL: u64 = 0x02C;
pub const CSR_MAC_SHADOW_REG_MEM:  u64 = 0x030;

/// Scratch/debug.
pub const CSR_MONITOR_CFG_REG:     u64 = 0x038;
pub const CSR_MONITOR_STATUS_REG:  u64 = 0x03C;

/// HBUS (Host Bus) target memory window — used for firmware upload.
pub const HBUS_TARG_MEM_RADDR:     u64 = 0x040;
pub const HBUS_TARG_MEM_WADDR:     u64 = 0x044;
pub const HBUS_TARG_MEM_WDAT:      u64 = 0x048;
pub const HBUS_TARG_MEM_RDAT:      u64 = 0x04C;
pub const HBUS_TARG_MEM_RDAT2:     u64 = 0x050;

/// NIC (device → host) scratch registers.
pub const CSR_DRAM_INT_TBL:        u64 = 0x0A0;
pub const CSR_MAC_SHADOW_REG_CTL2: u64 = 0x0B4;
pub const CSR_MAC_SHADOW_REG2:     u64 = 0x0B8;

/// FH (Firmware/Frame Handler) registers — TX/RX queue management.
pub const FH_MEM_CBBC_QUEUE:       u64 = 0x900; // base, +0x16 per queue
pub const FH_MEM_RCSRQueue:        u64 = 0x940; // base, +0x04 per queue
pub const FH_MEM_RWSR:             u64 = 0x948;
pub const FH_MEM_RSSR:             u64 = 0x950;
pub const FH_MEM_TXQ:              u64 = 0x980; // base, +0x80 per queue

/// MSIX registers (interrupt distribution).
pub const CSR_MSIX_BASE:           u64 = 0x2000;

// ============================================================================
// Bit masks for GP_CNTRL
// ============================================================================

pub mod gp_cntrl {
    /// MAC clock is running (bit 1).
    pub const MAC_CLOCK_READY: u32 = 1 << 1;
    /// Device init done / alive (bit 2).
    pub const INIT_DONE:       u32 = 1 << 2;
    /// MAC access is enabled (bit 3).
    pub const MAC_ACCESS_ENA:  u32 = 1 << 3;
    /// SW reset bit (bit 7).
    pub const SW_RESET:        u32 = 1 << 7;
}

// ============================================================================
// CSR wrapper
// ============================================================================

/// Typed accessor for the iwlwifi CSR region.
pub struct Csr {
    base: u64,
}

impl Csr {
    /// Create a CSR accessor from the **virtual** address of BAR0.
    /// The caller must have already translated `bar0_phys` via
    /// `crate::paging::phys_to_virt`.
    pub fn new(base_virt: u64) -> Self {
        Self { base: base_virt }
    }

    /// Read a 32-bit register.
    #[inline]
    pub fn read32(&self, offset: u64) -> u32 {
        unsafe { read_volatile((self.base + offset) as *const u32) }
    }

    /// Write a 32-bit register.
    #[inline]
    pub fn write32(&self, offset: u64, value: u32) {
        unsafe { write_volatile((self.base + offset) as *mut u32, value) }
    }

    /// Poll a register until `mask` bits are set (or `mask` bits are clear
    /// if `wait_set == false`).  Spins up to `timeout_us` microseconds.
    /// Returns `true` on success, `false` on timeout.
    pub fn poll32(&self, offset: u64, mask: u32, wait_set: bool, timeout_us: u64) -> bool {
        for _ in 0..timeout_us {
            let val = self.read32(offset);
            let condition = if wait_set {
                (val & mask) == mask
            } else {
                (val & mask) == 0
            };
            if condition {
                return true;
            }
            // ~1 µs per iteration at typical CPU frequencies.
            for _ in 0..100 { core::hint::spin_loop(); }
        }
        false
    }

    /// Read a 32-bit value from the HBUS target memory window.
    /// This is the canonical path for firmware-upload reads.
    pub fn hbus_read32(&self, addr: u32) -> u32 {
        self.write32(HBUS_TARG_MEM_RADDR, addr);
        self.read32(HBUS_TARG_MEM_RDAT)
    }

    /// Write a 32-bit value through the HBUS target memory window.
    /// This is the canonical path for firmware-upload writes.
    pub fn hbus_write32(&self, addr: u32, value: u32) {
        self.write32(HBUS_TARG_MEM_WADDR, addr);
        self.write32(HBUS_TARG_MEM_WDAT, value);
    }

    /// Write a byte through the MAC shadow register interface.
    /// Used during early ucode loading when the DMA engine isn't ready.
    pub fn shadow_write8(&self, addr: u16, value: u8) {
        self.write32(CSR_MAC_SHADOW_REG_CTRL, (1u32 << 31) | (addr as u32));
        self.write32(CSR_MAC_SHADOW_REG_MEM, value as u32);
    }

    /// Read a byte through the MAC shadow register interface.
    pub fn shadow_read8(&self, addr: u16) -> u8 {
        self.write32(CSR_MAC_SHADOW_REG_CTRL, addr as u32);
        self.read32(CSR_MAC_SHADOW_REG_MEM) as u8
    }
}
