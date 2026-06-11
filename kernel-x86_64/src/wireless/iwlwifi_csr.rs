//! iwlwifi CSR (Configuration and Status Registers) — register map + access.
//!
//! BAR0 is a memory-mapped region exposed by the PCI device. All register
//! offsets below are relative to BAR0 and follow Linux's
//! `drivers/net/wireless/intel/iwlwifi/iwl-csr.h` so cross-referencing the
//! reference drivers (Linux iwlwifi, OpenBSD iwm) stays grep-friendly.
//!
//! Offsets were corrected during the 7260 bring-up (the M11 scaffolding had
//! GP_CNTRL aliased onto RESET at 0x020, and the HBUS window off by 0x400).

use core::ptr::{read_volatile, write_volatile};

// ============================================================================
// CSR register offsets (relative to BAR0) — iwl-csr.h
// ============================================================================

pub const CSR_HW_IF_CONFIG_REG:   u64 = 0x000;
pub const CSR_INT_COALESCING:     u64 = 0x004;
pub const CSR_INT:                u64 = 0x008;
pub const CSR_INT_MASK:           u64 = 0x00C;
pub const CSR_FH_INT_STATUS:      u64 = 0x010;
pub const CSR_GPIO_IN:            u64 = 0x018;
pub const CSR_RESET:              u64 = 0x020;
pub const CSR_GP_CNTRL:           u64 = 0x024;
pub const CSR_HW_REV:             u64 = 0x028;
pub const CSR_EEPROM_REG:         u64 = 0x02C;
pub const CSR_EEPROM_GP:          u64 = 0x030;
pub const CSR_OTP_GP_REG:         u64 = 0x034;
pub const CSR_GIO_REG:            u64 = 0x03C;
pub const CSR_GP_UCODE_REG:       u64 = 0x048;
pub const CSR_GP_DRIVER_REG:      u64 = 0x050;
pub const CSR_UCODE_DRV_GP1:      u64 = 0x054;
pub const CSR_UCODE_DRV_GP1_SET:  u64 = 0x058;
pub const CSR_UCODE_DRV_GP1_CLR:  u64 = 0x05C;
pub const CSR_UCODE_DRV_GP2:      u64 = 0x060;
pub const CSR_HW_RF_ID:           u64 = 0x09C;
pub const CSR_LED_REG:            u64 = 0x094;
pub const CSR_DRAM_INT_TBL_REG:   u64 = 0x0A0;
pub const CSR_MAC_SHADOW_REG_CTRL:u64 = 0x0A8;
pub const CSR_GIO_CHICKEN_BITS:   u64 = 0x100;
pub const CSR_ANA_PLL_CFG:        u64 = 0x20C;
pub const CSR_MONITOR_STATUS_REG: u64 = 0x228;
pub const CSR_HW_REV_WA_REG:      u64 = 0x22C;
pub const CSR_DBG_HPET_MEM_REG:   u64 = 0x240;
pub const CSR_HW_RF_ID_TYPE:      u64 = 0x190;

/// HBUS (Host Bus) target windows — indirect access to NIC SRAM (MEM) and
/// the peripheral/APMG register space (PRPH). HBUS base is 0x400, NOT 0x040.
pub const CSR_HBUS_TARG_MEM_RADDR:  u64 = 0x40C;
pub const CSR_HBUS_TARG_MEM_WADDR:  u64 = 0x410;
pub const CSR_HBUS_TARG_MEM_WDAT:   u64 = 0x418;
pub const CSR_HBUS_TARG_MEM_RDAT:   u64 = 0x41C;
pub const CSR_HBUS_TARG_PRPH_WADDR: u64 = 0x444;
pub const CSR_HBUS_TARG_PRPH_RADDR: u64 = 0x448;
pub const CSR_HBUS_TARG_PRPH_WDAT:  u64 = 0x44C;
pub const CSR_HBUS_TARG_PRPH_RDAT:  u64 = 0x450;
pub const CSR_HBUS_TARG_WRPTR:      u64 = 0x460;

// ============================================================================
// Bit definitions
// ============================================================================

pub mod hw_if_config {
    /// Set by SW to tell the device we're ready; device echoes it back.
    pub const NIC_READY:       u32 = 0x0040_0000;
    /// Device has completed the PREPARE handshake (ownership granted).
    pub const NIC_PREPARE_DONE:u32 = 0x0200_0000;
    /// SW requests ownership of the device (away from BIOS/ME).
    pub const PREPARE:         u32 = 0x0800_0000;
    /// Enable HAP INTA (mgmt-bus interrupt) to wake the device.
    pub const HAP_WAKE_L1A:    u32 = 0x0008_0000;
}

pub mod gp_cntrl {
    /// MAC clock is running (bit 0).
    pub const MAC_CLOCK_READY: u32 = 0x0000_0001;
    /// "Initialization complete" — SW sets this to start the MAC clock.
    pub const INIT_DONE:       u32 = 0x0000_0004;
    /// SW requests access to the MAC (bit 3).
    pub const MAC_ACCESS_REQ:  u32 = 0x0000_0008;
    /// Device is going to sleep.
    pub const GOING_TO_SLEEP:  u32 = 0x0000_0010;
}

pub mod reset {
    /// Full software reset of the device.
    pub const SW_RESET:        u32 = 0x0000_0080;
    pub const STOP_MASTER:     u32 = 0x0000_0020;
    pub const MASTER_DISABLED: u32 = 0x0000_0100;
}

pub mod gio_chicken {
    pub const L1A_NO_L0S_RX:        u32 = 0x0080_0000;
    pub const DIS_L0S_EXIT_TIMER:   u32 = 0x2000_0000;
}

/// FH (Flow Handler) registers — direct BAR0 MMIO, used for firmware DMA.
/// Offsets per iwl-fh.h. The firmware service channel is channel 9.
pub mod fh {
    pub const SRVC_CHNL: u64 = 9;
    const MEM_LOWER: u64 = 0x1000;
    // TFDIB (Transfer Frame Descriptor Image Buffer) control.
    pub const fn tfdib_ctrl0(chnl: u64) -> u64 { MEM_LOWER + 0x900 + 0x8 * chnl }
    pub const fn tfdib_ctrl1(chnl: u64) -> u64 { MEM_LOWER + 0x900 + 0x8 * chnl + 0x4 }
    pub const ADDR_BITSHIFT: u32 = 28;
    // Service-channel SRAM destination address.
    pub const fn srvc_sram_addr(chnl: u64) -> u64 { MEM_LOWER + 0x9C8 + (chnl - 9) * 0x4 }
    // TX channel config + buffer status.
    pub const fn tcsr_tx_config(chnl: u64) -> u64 { MEM_LOWER + 0xD00 + 0x20 * chnl }
    pub const fn tcsr_tx_buf_sts(chnl: u64) -> u64 { MEM_LOWER + 0xD00 + 0x20 * chnl + 0x8 }
    // TX shared status (channel-idle bits).
    pub const TSSR_TX_STATUS: u64 = MEM_LOWER + 0xEA0 + 0x010;
    // Keep-warm DMA page address register (value = kw_phys >> 4). The FH
    // DMA engine needs this set before the service channel will run.
    pub const KW_MEM_ADDR: u64 = MEM_LOWER + 0x97C;

    pub const TX_CONFIG_DMA_PAUSE: u32 = 0x0000_0000;
    pub const TX_CONFIG_DMA_ENABLE: u32 = 0x8000_0000;
    pub const TX_CONFIG_CIRQ_HOST_ENDTFD: u32 = 0x0010_0000;
    pub const BUF_STS_TB_NUM_POS: u32 = 20;
    pub const BUF_STS_TB_IDX_POS: u32 = 12;
    pub const BUF_STS_TFBD_VALID: u32 = 0x0000_4000;

    /// The two idle-status bits for a given channel in TSSR_TX_STATUS.
    pub const fn tssr_idle_mask(chnl: u64) -> u32 {
        (1u32 << (chnl + 16)) | (1u32 << chnl)
    }
}

/// APMG (power management gateway) registers — accessed via PRPH, 7000-series.
pub mod apmg {
    pub const CLK_EN_REG:        u32 = 0x0000_3004;
    pub const CLK_DIS_REG:       u32 = 0x0000_3008;
    pub const PS_CTRL_REG:       u32 = 0x0000_300C;
    pub const PCIDEV_STT_REG:    u32 = 0x0000_3010;
    pub const RTC_INT_STT_REG:   u32 = 0x0000_3014;

    pub const CLK_VAL_DMA_CLK_RQT: u32 = 0x0000_0200;
    pub const CLK_VAL_BSM_CLK_RQT: u32 = 0x0000_0800;
    pub const PS_CTRL_VAL_RESET_REQ: u32 = 0x0400_0000;
    pub const PS_CTRL_MSK_PWR_SRC:   u32 = 0x0300_0000;
    pub const PS_CTRL_VAL_PWR_SRC_VMAIN: u32 = 0x0000_0000;
    pub const PCIDEV_STT_VAL_L1_ACT_DIS: u32 = 0x0000_0800;
    pub const RTC_INT_STT_RFKILL:    u32 = 0x1000_0000;
}

// ============================================================================
// CSR wrapper
// ============================================================================

/// Typed accessor for the iwlwifi CSR region.
pub struct Csr {
    base: u64,
}

impl Csr {
    /// Create a CSR accessor from the **virtual** address of BAR0 (already
    /// translated via `crate::paging::phys_to_virt`).
    pub fn new(base_virt: u64) -> Self {
        Self { base: base_virt }
    }

    #[inline]
    pub fn read32(&self, offset: u64) -> u32 {
        unsafe { read_volatile((self.base + offset) as *const u32) }
    }

    #[inline]
    pub fn write32(&self, offset: u64, value: u32) {
        unsafe { write_volatile((self.base + offset) as *mut u32, value) }
    }

    /// Read-modify-write: set `bits` in the register at `offset`.
    pub fn set_bit(&self, offset: u64, bits: u32) {
        let v = self.read32(offset);
        self.write32(offset, v | bits);
    }

    /// Read-modify-write: clear `bits` in the register at `offset`.
    pub fn clear_bit(&self, offset: u64, bits: u32) {
        let v = self.read32(offset);
        self.write32(offset, v & !bits);
    }

    /// Poll a register until `mask` bits match `wait_set`. Spins up to
    /// `timeout_us` microseconds (~1 µs/iteration). True on success.
    pub fn poll32(&self, offset: u64, mask: u32, wait_set: bool, timeout_us: u64) -> bool {
        for _ in 0..timeout_us {
            let val = self.read32(offset);
            let ok = if wait_set { (val & mask) == mask } else { (val & mask) == 0 };
            if ok {
                return true;
            }
            for _ in 0..100 { core::hint::spin_loop(); }
        }
        false
    }

    // ---- PRPH (peripheral / APMG) indirect access -----------------------
    // A PRPH access goes through the HBUS window: write the target address
    // (OR'd with the 4-dword byte-enable mask 0x3<<24) then read/write data.

    fn prph_mask(addr: u32) -> u32 {
        (addr & 0x000F_FFFF) | (3 << 24)
    }

    pub fn read_prph(&self, addr: u32) -> u32 {
        self.write32(CSR_HBUS_TARG_PRPH_RADDR, Self::prph_mask(addr));
        self.read32(CSR_HBUS_TARG_PRPH_RDAT)
    }

    pub fn write_prph(&self, addr: u32, value: u32) {
        self.write32(CSR_HBUS_TARG_PRPH_WADDR, Self::prph_mask(addr));
        self.write32(CSR_HBUS_TARG_PRPH_WDAT, value);
    }

    pub fn set_bits_prph(&self, addr: u32, bits: u32) {
        let v = self.read_prph(addr);
        self.write_prph(addr, v | bits);
    }

    pub fn clear_bits_prph(&self, addr: u32, bits: u32) {
        let v = self.read_prph(addr);
        self.write_prph(addr, v & !bits);
    }

    // ---- HBUS target memory (NIC SRAM) window ---------------------------

    pub fn mem_read32(&self, addr: u32) -> u32 {
        self.write32(CSR_HBUS_TARG_MEM_RADDR, addr);
        self.read32(CSR_HBUS_TARG_MEM_RDAT)
    }

    pub fn mem_write32(&self, addr: u32, value: u32) {
        self.write32(CSR_HBUS_TARG_MEM_WADDR, addr);
        self.write32(CSR_HBUS_TARG_MEM_WDAT, value);
    }
}
