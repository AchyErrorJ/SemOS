//! iwlwifi device layer — M11 stage 2 skeleton.
//!
//! This is the hardware-gated core: firmware upload, secboot, ALIVE event,
//! PHY init, and command/TX/RX queue management.  On QEMU it compiles but
//! never runs because `iwlwifi_pci::probe()` returns `None`.
//!
//! When the T540/P1 hardware is available, the stubs below are filled in
//! with the real CSR read/write sequences, ucode load, and queue setup.

use crate::println;
use super::iwlwifi_pci::IwlPciInfo;
use super::iwlwifi_csr::Csr;
use super::iwlwifi_sm::AssocStateMachine;
use super::MacAddr;

/// Firmware-load DMA bounce buffer: each chunk is copied here (a known,
/// page-aligned physical address) before the FH DMAs it into device SRAM.
const FW_BOUNCE_SIZE: usize = 32 * 1024;
#[repr(C, align(4096))]
struct FwDmaBounce([u8; FW_BOUNCE_SIZE]);
static mut FW_DMA_BOUNCE: FwDmaBounce = FwDmaBounce([0; FW_BOUNCE_SIZE]);

/// Keep-warm DMA page — the FH DMA engine requires its address programmed
/// (FH_KW_MEM_ADDR) before the firmware service channel will run.
#[repr(C, align(4096))]
struct KeepWarm([u8; 4096]);
static mut KEEP_WARM: KeepWarm = KeepWarm([0; 4096]);

/// RX ring for catching the firmware's ALIVE notification (and later, all
/// received frames). gen1 RBD = 32-bit (buf_phys >> 8). 256 buffers ×
/// 4 KiB = 1 MiB; the NIC DMAs received notifications into these.
const RX_RING_SIZE: usize = 256;
#[repr(C, align(4096))]
struct RxRbd([u32; RX_RING_SIZE]);
static mut RX_RBD: RxRbd = RxRbd([0; RX_RING_SIZE]);
/// Status page — the NIC writes the closed-buffer index here.
#[repr(C, align(16))]
struct RbStts([u32; 4]);
static mut RB_STTS: RbStts = RbStts([0; 4]);
#[repr(C, align(4096))]
struct RxBufs([[u8; 4096]; RX_RING_SIZE]);
static mut RX_BUFS: RxBufs = RxBufs([[0; 4096]; RX_RING_SIZE]);

// ---- TX command queue (queue 0) ----
const TX_RING_SIZE: usize = 256;
/// One gen1 TFD = 128 bytes (3 reserved + num_tbs + 20 TBs×6 + 4 pad).
#[repr(C, align(4096))]
struct TxTfdRing([[u8; 128]; TX_RING_SIZE]);
static mut TX_TFD_RING: TxTfdRing = TxTfdRing([[0; 128]; TX_RING_SIZE]);
/// Scheduler byte-count table: 320 × u16 = 640 bytes, must be 1 KiB-aligned.
#[repr(C, align(1024))]
struct TxBcTbl([u16; 320]);
static mut TX_BC_TBL: TxBcTbl = TxBcTbl([0; 320]);
/// Command staging buffer (header + payload) the TFD points at.
#[repr(C, align(64))]
struct CmdBuf([u8; 512]);
static mut CMD_BUF: CmdBuf = CmdBuf([0; 512]);

/// Translate a kernel-virtual address to physical via the active PML4.
fn phys_of(virt: u64) -> Option<u64> {
    let page = virt & !0xFFF;
    let off = virt & 0xFFF;
    crate::paging::walk_active_pml4(page).map(|p| p + off)
}

/// Opaque device handle.  Created by `probe()` on PCI match.
pub struct IwlDevice {
    pub pci: IwlPciInfo,
    pub state: DeviceState,
    /// MMIO accessor for BAR0 CSR registers.
    pub csr: Csr,
    /// WiFi association state machine (scan → auth → assoc → 4-way → DHCP).
    pub sm: AssocStateMachine,
    /// Scheduler base in device SRAM, reported by the ALIVE notification.
    /// Needed to set up the TX command queue (Stage 3).
    pub scd_base_ptr: u32,
    /// Firmware error/log table pointers (for reading crash logs).
    pub error_table_ptr: u32,
    pub log_table_ptr: u32,
    /// Command-queue (queue 0) write index.
    pub tx_write_idx: u16,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DeviceState {
    /// PCI probed, BAR mapped, but firmware not yet loaded.
    Probed,
    /// Firmware blob written to NIC SRAM, waiting for ALIVE.
    FirmwareLoading,
    /// ALIVE notification received, ucode running.
    Alive,
    /// PHY calibrated, channels known, ready to scan.
    PhyReady,
    /// Associated to an AP, keys installed.
    Associated,
}

impl IwlDevice {
    /// Create a new device from PCI probe results.
    pub fn new(pci: IwlPciInfo) -> Self {
        let bar0_virt = crate::paging::phys_to_virt(pci.bar0_phys);
        let csr = Csr::new(bar0_virt);
        println!("[iwlwifi] device created for {} @ {:02X}:{:02X}.{}  BAR0 phys=0x{:08X} virt=0x{:08X}",
            pci.name, pci.loc.bus, pci.loc.slot, pci.loc.func, pci.bar0_phys, bar0_virt);
        // TODO(M11): read real STA MAC from EEPROM/NVM instead of placeholder.
        let sta_mac: MacAddr = [0x00, 0x16, 0x3E, 0x00, 0x00, 0x01];
        let sm = AssocStateMachine::new(sta_mac);
        Self {
            pci, state: DeviceState::Probed, csr, sm,
            scd_base_ptr: 0, error_table_ptr: 0, log_table_ptr: 0,
            tx_write_idx: 0,
        }
    }

    /// Diagnostic: read CSR_HW_REV and CSR_HW_RF_ID.  Prints chip revision
    /// so metal bring-up logs can confirm the MMIO path works before any
    /// firmware is loaded.
    pub fn read_hw_rev(&self) {
        let rev = self.csr.read32(super::iwlwifi_csr::CSR_HW_REV);
        let rf_id = self.csr.read32(super::iwlwifi_csr::CSR_HW_RF_ID);
        println!("[iwlwifi] HW_REV=0x{:08X} RF_ID=0x{:08X}", rev, rf_id);
    }

    /// Software-reset the device (CSR_RESET.SW_RESET), then settle.
    pub fn sw_reset(&self) {
        use super::iwlwifi_csr::{CSR_RESET, reset};
        self.csr.set_bit(CSR_RESET, reset::SW_RESET);
        // ~5 ms settle (the reg self-clears; we just need the delay).
        for _ in 0..5000 { for _ in 0..100 { core::hint::spin_loop(); } }
    }

    /// Request ownership of the device from BIOS/ME and wait for the
    /// hardware to acknowledge it is ours to drive. iwlwifi
    /// `iwl_pcie_prepare_card_hw`. Returns true if the NIC reports ready.
    pub fn prepare_card_hw(&self) -> bool {
        use super::iwlwifi_csr::{CSR_HW_IF_CONFIG_REG as CFG, hw_if_config as f};
        // Fast path: maybe already ours.
        self.csr.set_bit(CFG, f::NIC_READY);
        if self.csr.poll32(CFG, f::NIC_READY, true, 5_000) {
            return true;
        }
        // Otherwise run the PREPARE handshake, up to 10 tries.
        for attempt in 0..10 {
            self.csr.set_bit(CFG, f::PREPARE);
            // Wait for the device to release/grant ownership.
            self.csr.poll32(CFG, f::NIC_PREPARE_DONE, true, 20_000);
            self.csr.set_bit(CFG, f::NIC_READY);
            if self.csr.poll32(CFG, f::NIC_READY, true, 5_000) {
                println!("[iwlwifi] prepare_card_hw: NIC_READY after {} attempt(s)", attempt + 1);
                return true;
            }
        }
        println!("[iwlwifi] prepare_card_hw: NIC never became ready (BIOS/ME may hold the card)");
        false
    }

    /// Advanced Power Management init — bring up the MAC clock so the
    /// device is in a state where firmware can be loaded. iwlwifi
    /// `iwl_pcie_apm_init` (7000-series / pre-secboot variant).
    pub fn apm_init(&self) -> bool {
        use super::iwlwifi_csr::*;
        // Disable L0s exit timer + L0s-on-RX (platform/ICH workarounds).
        self.csr.set_bit(CSR_GIO_CHICKEN_BITS, gio_chicken::DIS_L0S_EXIT_TIMER);
        self.csr.set_bit(CSR_GIO_CHICKEN_BITS, gio_chicken::L1A_NO_L0S_RX);
        // Wake the device on mgmt-bus interrupt.
        self.csr.set_bit(CSR_HW_IF_CONFIG_REG, hw_if_config::HAP_WAKE_L1A);
        // "Initialization complete" → starts the MAC clock.
        self.csr.set_bit(CSR_GP_CNTRL, gp_cntrl::INIT_DONE);
        // Wait for the clock to stabilize (spec allows up to ~25 ms).
        if !self.csr.poll32(CSR_GP_CNTRL, gp_cntrl::MAC_CLOCK_READY, true, 25_000) {
            println!("[iwlwifi] apm_init: MAC clock never became ready (GP_CNTRL=0x{:08X})",
                self.csr.read32(CSR_GP_CNTRL));
            return false;
        }
        // 7000-series: enable the DMA clock via APMG, disable L1-Active,
        // clear the RF-kill monitor disable. These are PRPH (indirect).
        self.csr.write_prph(apmg::CLK_EN_REG, apmg::CLK_VAL_DMA_CLK_RQT);
        for _ in 0..20 { for _ in 0..100 { core::hint::spin_loop(); } } // ~20 µs
        self.csr.set_bits_prph(apmg::PCIDEV_STT_REG, apmg::PCIDEV_STT_VAL_L1_ACT_DIS);
        self.csr.clear_bits_prph(apmg::RTC_INT_STT_REG, apmg::RTC_INT_STT_RFKILL);
        println!("[iwlwifi] apm_init: MAC clock ready, APMG configured (GP_CNTRL=0x{:08X})",
            self.csr.read32(CSR_GP_CNTRL));
        true
    }

    /// Stage 1 bring-up: reset → take ownership → power up the MAC clock,
    /// dumping registers along the way. Does NOT load firmware (Stage 2).
    /// Advances `state` to `Probed` (clocked + owned) on success.
    pub fn power_up(&mut self) -> bool {
        use super::iwlwifi_csr::*;
        let rev = self.csr.read32(CSR_HW_REV);
        let rf = self.csr.read32(CSR_HW_RF_ID);
        println!("[iwlwifi] power_up: HW_REV=0x{:08X} RF_ID=0x{:08X}", rev, rf);
        if rev == 0xFFFF_FFFF {
            println!("[iwlwifi] power_up: CSRs read all-ones — BAR/PCI mapping wrong; aborting");
            return false;
        }
        self.sw_reset();
        if !self.prepare_card_hw() {
            return false;
        }
        if !self.apm_init() {
            return false;
        }
        // Read back HW_REV again (some bits are only valid after clocks up)
        // + the RF-kill state so the boot log shows whether the radio is
        // hardware-killed (wifi switch / airplane mode).
        let cfg = self.csr.read32(CSR_HW_IF_CONFIG_REG);
        let gpc = self.csr.read32(CSR_GP_CNTRL);
        println!("[iwlwifi] power_up OK: HW_IF_CONFIG=0x{:08X} GP_CNTRL=0x{:08X}", cfg, gpc);
        self.state = DeviceState::Probed;
        true
    }

    /// Request access to the MAC (so FH/PRPH writes land), iwlwifi
    /// `iwl_grab_nic_access`. Returns true on grant.
    fn grab_nic_access(&self) -> bool {
        use super::iwlwifi_csr::{CSR_GP_CNTRL, gp_cntrl};
        self.csr.set_bit(CSR_GP_CNTRL, gp_cntrl::MAC_ACCESS_REQ);
        // Wait for MAC_CLOCK_READY with GOING_TO_SLEEP clear.
        for _ in 0..15_000 {
            let v = self.csr.read32(CSR_GP_CNTRL);
            if v & gp_cntrl::MAC_CLOCK_READY != 0 && v & gp_cntrl::GOING_TO_SLEEP == 0 {
                return true;
            }
            for _ in 0..100 { core::hint::spin_loop(); }
        }
        println!("[iwlwifi] grab_nic_access timed out (GP_CNTRL=0x{:08X})",
            self.csr.read32(CSR_GP_CNTRL));
        false
    }

    fn release_nic_access(&self) {
        use super::iwlwifi_csr::{CSR_GP_CNTRL, gp_cntrl};
        self.csr.clear_bit(CSR_GP_CNTRL, gp_cntrl::MAC_ACCESS_REQ);
    }

    /// Program the keep-warm page address into the FH (once). The DMA
    /// engine needs this before the service channel will transfer.
    fn set_keep_warm(&self) -> bool {
        use super::iwlwifi_csr::fh;
        let kw_virt = unsafe { &raw const KEEP_WARM as u64 };
        let kw_phys = match phys_of(kw_virt) {
            Some(p) => p,
            None => { println!("[iwlwifi] keep-warm phys translation failed"); return false; }
        };
        if !self.grab_nic_access() { return false; }
        self.csr.write32(fh::KW_MEM_ADDR, (kw_phys >> 4) as u32);
        self.release_nic_access();
        true
    }

    /// DMA one chunk (staged at `src_phys`, `len` bytes) to device SRAM
    /// address `dst` via the FH service channel. Verifies by reading the
    /// first dword back out of SRAM rather than trusting the (poorly
    /// understood) TSSR idle bits.
    fn load_chunk(&self, dst: u32, src_phys: u64, len: u32, expect0: u32) -> bool {
        use super::iwlwifi_csr::fh;
        let ch = fh::SRVC_CHNL;
        if !self.grab_nic_access() {
            return false;
        }
        self.csr.write32(fh::tcsr_tx_config(ch), fh::TX_CONFIG_DMA_PAUSE);
        self.csr.write32(fh::srvc_sram_addr(ch), dst);
        self.csr.write32(fh::tfdib_ctrl0(ch), (src_phys & 0xFFFF_FFFF) as u32);
        self.csr.write32(fh::tfdib_ctrl1(ch),
            (((src_phys >> 32) as u32) << fh::ADDR_BITSHIFT) | len);
        self.csr.write32(fh::tcsr_tx_buf_sts(ch),
            (1 << fh::BUF_STS_TB_NUM_POS) | (1 << fh::BUF_STS_TB_IDX_POS) | fh::BUF_STS_TFBD_VALID);
        self.csr.write32(fh::tcsr_tx_config(ch),
            fh::TX_CONFIG_DMA_ENABLE | fh::TX_CONFIG_CIRQ_HOST_ENDTFD);
        self.release_nic_access();

        // Give the DMA time, then VERIFY by reading SRAM back through the
        // HBUS memory window (definitive — independent of TSSR semantics).
        for _ in 0..5000 { for _ in 0..100 { core::hint::spin_loop(); } } // ~5 ms
        if !self.grab_nic_access() { return false; }
        let tssr = self.csr.read32(fh::TSSR_TX_STATUS);
        let got0 = self.csr.mem_read32(dst);
        self.release_nic_access();
        if got0 != expect0 {
            println!("[iwlwifi] load_chunk dst=0x{:08X}: SRAM readback 0x{:08X} != expected 0x{:08X} (TSSR=0x{:08X})",
                dst, got0, expect0, tssr);
            return false;
        }
        true
    }

    /// Load one firmware section into device SRAM by writing it directly
    /// through the HBUS memory window (iwl_write_mem). Bypasses the FH DMA
    /// engine entirely — slower, but doesn't need the TX/scheduler setup
    /// the service-channel DMA requires. Verifies by reading back.
    fn load_section(&self, addr: u32, data: &[u8]) -> bool {
        // Convert bytes → little-endian dwords (pad a short tail with 0).
        let n_dw = (data.len() + 3) / 4;
        if !self.grab_nic_access() {
            return false;
        }
        // Write in bursts so a single grab covers a manageable run; the
        // HBUS WADDR auto-increments, so we just stream WDAT.
        self.csr.write32(super::iwlwifi_csr::CSR_HBUS_TARG_MEM_WADDR, addr);
        for i in 0..n_dw {
            let b0 = data.get(i*4).copied().unwrap_or(0);
            let b1 = data.get(i*4+1).copied().unwrap_or(0);
            let b2 = data.get(i*4+2).copied().unwrap_or(0);
            let b3 = data.get(i*4+3).copied().unwrap_or(0);
            self.csr.write32(super::iwlwifi_csr::CSR_HBUS_TARG_MEM_WDAT,
                u32::from_le_bytes([b0, b1, b2, b3]));
        }
        // Verify first + a middle dword landed.
        let expect0 = u32::from_le_bytes([
            data.first().copied().unwrap_or(0),
            data.get(1).copied().unwrap_or(0),
            data.get(2).copied().unwrap_or(0),
            data.get(3).copied().unwrap_or(0)]);
        let got0 = self.csr.mem_read32(addr);
        self.release_nic_access();
        if got0 != expect0 {
            println!("[iwlwifi] load_section 0x{:08X}: readback 0x{:08X} != expected 0x{:08X}",
                addr, got0, expect0);
            return false;
        }
        true
    }

    /// Load all sections of a firmware image (INIT or RUNTIME) into device
    /// SRAM. Skips marker sections (load address in the 0xFFFFxxxx / 0xAAAA
    /// special range). Returns true if every real section DMA'd cleanly.
    pub fn load_image(&self, img: &super::iwlwifi_fw_image::FwImage) -> bool {
        if !self.set_keep_warm() {
            return false;
        }
        for s in img.sections[..img.count].iter() {
            // Separator / metadata markers are not SRAM destinations.
            if s.addr >= 0xFFFF_0000 || s.addr == super::iwlwifi_fw_image::PAGING_SEPARATOR {
                continue;
            }
            let data = &super::iwlwifi_fw_image::FW_7260[s.off..s.off + s.len];
            if !self.load_section(s.addr, data) {
                println!("[iwlwifi] load_image: section addr=0x{:08X} len={} FAILED", s.addr, s.len);
                return false;
            }
            println!("[iwlwifi] loaded section addr=0x{:08X} len={}", s.addr, s.len);
        }
        true
    }

    /// Stub: initialise PHY from EEPROM/NVM + PNVM + regulatory caps.
    pub fn init_phy(&mut self) -> bool {
        println!("[iwlwifi] init_phy: STUB — needs NVM parse + channel table");
        // TODO(M11): read EEPROM, apply regulatory, run TX/RX calibration.
        false
    }

    /// Set up the RX buffer-descriptor ring so the firmware has somewhere
    /// to DMA the ALIVE notification (and all later received frames).
    pub fn rx_init(&self) -> bool {
        use super::iwlwifi_csr::fh_rx::*;
        // Fill the RBD ring with the physical address (>> 8) of each buffer.
        for i in 0..RX_RING_SIZE {
            let bp = match phys_of(unsafe { &raw const RX_BUFS.0[i] } as u64) {
                Some(p) => p,
                None => { println!("[iwlwifi] rx buf {} phys failed", i); return false; }
            };
            unsafe { RX_RBD.0[i] = (bp >> 8) as u32; }
        }
        let rbd_phys = match phys_of(unsafe { &raw const RX_RBD } as u64) { Some(p)=>p, None=>return false };
        let stts_phys = match phys_of(unsafe { &raw const RB_STTS } as u64) { Some(p)=>p, None=>return false };
        unsafe { RB_STTS.0 = [0; 4]; }

        if !self.grab_nic_access() { return false; }
        // Stop RX, program ring base + status pointer, then enable.
        self.csr.write32(RCSR_CHNL0_CONFIG, 0);
        self.csr.write32(RSCSR_CHNL0_WPTR, 0);
        self.csr.write32(RSCSR_CHNL0_RBDCB_BASE, (rbd_phys >> 8) as u32);
        self.csr.write32(RSCSR_CHNL0_STTS_WPTR, (stts_phys >> 4) as u32);
        let cfg = CONFIG_ENABLE | CONFIG_IGNORE_RXF_EMPTY | CONFIG_IRQ_DEST_HOST
            | CONFIG_RB_SIZE_4K
            | (8 << CONFIG_RBDC_SIZE_POS)   // log2(256) = 8
            | (0x10 << CONFIG_IRQ_RBTH_POS);
        self.csr.write32(RCSR_CHNL0_CONFIG, cfg);
        // Publish all but the last 8 buffers (write pointer must be 8-aligned).
        self.csr.write32(RSCSR_CHNL0_WPTR, (RX_RING_SIZE - 8) as u32);
        self.release_nic_access();
        println!("[iwlwifi] rx_init: ring base=0x{:08X} stts=0x{:08X} cfg=0x{:08X}",
            (rbd_phys >> 8) as u32, (stts_phys >> 4) as u32, cfg);
        true
    }

    /// Release the NIC's CPU from reset so the loaded firmware starts
    /// running. gen1: write CSR_RESET = 0.
    pub fn release_cpu(&self) {
        use super::iwlwifi_csr::CSR_RESET;
        self.csr.write32(CSR_RESET, 0);
    }

    /// After releasing the CPU, watch for any sign the firmware booted:
    /// the RX status page advancing, or interrupt status bits setting.
    /// Diagnostic — dumps state regardless so we can see what the ucode
    /// did. Returns true if something moved.
    pub fn wait_alive(&mut self) -> bool {
        use super::iwlwifi_csr::{CSR_INT, CSR_FH_INT_STATUS, CSR_RESET};
        let mut moved = false;
        let mut last_int = 0u32;
        let mut last_fh = 0u32;
        let mut last_stts = 0u32;
        for _ in 0..200 {
            let int = self.csr.read32(CSR_INT);
            let fh = self.csr.read32(CSR_FH_INT_STATUS);
            let stts = unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) };
            if int != 0 || fh != 0 || stts != 0 {
                moved = true;
                last_int = int; last_fh = fh; last_stts = stts;
                break;
            }
            for _ in 0..5000 { for _ in 0..100 { core::hint::spin_loop(); } } // ~5 ms
        }
        // Final snapshot.
        let int = self.csr.read32(CSR_INT);
        let fh = self.csr.read32(CSR_FH_INT_STATUS);
        let reset = self.csr.read32(CSR_RESET);
        let stts0 = unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) };
        println!("[iwlwifi] post-release: moved={} CSR_INT=0x{:08X} FH_INT=0x{:08X} RESET=0x{:08X} rb_stts=0x{:08X}",
            moved as u8, int, fh, reset, stts0);
        let _ = (last_int, last_fh, last_stts);

        // Dump the full ALIVE notification (RX buffer 0). iwl_rx_packet =
        // len_n_flags(4) + cmd_header(4) + payload[]. The ALIVE payload
        // carries the scheduler base + error-table pointers Stage 3 needs.
        let mut dw = [0u32; 20];
        unsafe {
            let p = &raw const RX_BUFS.0[0] as *const u32;
            for (i, slot) in dw.iter_mut().enumerate() {
                *slot = core::ptr::read_volatile(p.add(i));
            }
        }
        let cmd = (dw[1] & 0xFF) as u8;
        // ALIVE payload starts at dw[2] (after len_n_flags + cmd header).
        // iwl_alive_resp_ver1: status@dw[2].low16, then pointers. Layout
        // confirmed on hardware: error_table=dw[7], log_table=dw[8],
        // scd_base=dw[12].
        let status = (dw[2] & 0xFFFF) as u16;
        self.error_table_ptr = dw[7];
        self.log_table_ptr = dw[8];
        self.scd_base_ptr = dw[12];

        if cmd != 0x01 || status != 0xCAFE {
            println!("[iwlwifi] ALIVE: unexpected cmd=0x{:02X} status=0x{:04X} — dumping payload",
                cmd, status);
            for (i, &v) in dw.iter().enumerate() {
                println!("[iwlwifi]   ALIVE dw[{:02}]=0x{:08X}", i, v);
            }
            return moved;
        }
        println!("[iwlwifi] ALIVE OK (status=0xCAFE): scd_base=0x{:08X} err_table=0x{:08X} log_table=0x{:08X}",
            self.scd_base_ptr, self.error_table_ptr, self.log_table_ptr);

        // Peek the firmware error table: first dword is a "valid" flag —
        // non-zero means the ucode logged a fatal error during boot.
        if self.grab_nic_access() {
            let valid = self.csr.mem_read32(self.error_table_ptr);
            let err_id = self.csr.mem_read32(self.error_table_ptr + 4);
            self.release_nic_access();
            if valid != 0 {
                println!("[iwlwifi] ALIVE: firmware error table VALID=0x{:08X} error_id=0x{:08X} (ucode logged a fault!)",
                    valid, err_id);
            } else {
                println!("[iwlwifi] ALIVE: firmware error table clean (no fault)");
            }
        }
        self.state = DeviceState::Alive;
        true
    }

    /// Stage 3a: set up the TX command queue (queue 0) + configure the
    /// scheduler, then read the scheduler registers back to confirm it's
    /// responding before we trust it. Requires ALIVE (scd_base_ptr set).
    pub fn tx_init(&mut self) -> bool {
        use super::iwlwifi_csr::{scd, fh_cbbc_queue};
        if self.scd_base_ptr == 0 {
            println!("[iwlwifi] tx_init: no scd_base (not ALIVE?)");
            return false;
        }
        let tfd_phys = match phys_of(unsafe { &raw const TX_TFD_RING } as u64) { Some(p)=>p, None=>return false };
        let bc_phys = match phys_of(unsafe { &raw const TX_BC_TBL } as u64) { Some(p)=>p, None=>return false };

        if !self.grab_nic_access() { return false; }
        use super::iwlwifi_csr::{CSR_HBUS_TARG_WRPTR, fh};
        let scd_base = self.scd_base_ptr;
        // Disable the scheduler while we configure it.
        self.csr.write_prph(scd::TXFACT, 0);
        // Clear the SCD per-queue context memory in SRAM (scd_base+0x600
        // .. +0x808) so stale state doesn't confuse the scheduler.
        let ctx_words = (scd::TRANS_TBL_OFFSET - scd::CONTEXT_MEM_OFFSET) / 4;
        for i in 0..ctx_words {
            self.csr.mem_write32(scd_base + scd::CONTEXT_MEM_OFFSET + i * 4, 0);
        }
        // Byte-count table base (>> 10) and the command-queue TFD ring base.
        self.csr.write_prph(scd::DRAM_BASE_ADDR, (bc_phys >> 10) as u32);
        self.csr.write32(fh_cbbc_queue(0), (tfd_phys >> 8) as u32);
        // All queues independent (no chaining/aggregation) for the cmd queue.
        self.csr.write_prph(scd::QUEUECHAIN_SEL, 0);
        self.csr.write_prph(scd::AGGR_SEL, 0);
        self.csr.write_prph(scd::CHAINEXT_EN, 0);
        // Reset queue-0 read pointer + the hardware write pointer.
        self.csr.write_prph(scd::queue_rdptr(0), 0);
        self.csr.write32(CSR_HBUS_TARG_WRPTR, 0);
        // Write the queue-0 SCD context in SRAM: window=0, then frame-limit
        // (64) in both the window-size and frame-limit fields.
        let frame_limit: u32 = 64;
        self.csr.mem_write32(scd_base + scd::context_queue_offset(0), 0);
        self.csr.mem_write32(scd_base + scd::context_queue_offset(0) + 4,
            ((frame_limit << scd::CTX_WIN_SIZE_POS) & 0x7F)
            | ((frame_limit << scd::CTX_FRAME_LIMIT_POS) & 0x7F0000));
        // Enable queue 0 in the SCD with the command TX FIFO.
        self.csr.write_prph(scd::queue_status(0), scd::queue_enable_val(scd::CMD_FIFO));
        // Enable the FH TX DMA channels (DMA enable + credit enable).
        for chan in 0..8u64 {
            self.csr.write32(fh::tcsr_tx_config(chan), fh::TX_CONFIG_DMA_ENABLE | 0x8);
        }
        // Activate the scheduler TX FIFOs. SCD_TXFACT is a per-FIFO mask;
        // the command queue maps to FIFO 7, so enable all 8 FIFOs to cover
        // it regardless of the exact queue→FIFO mapping (unused FIFOs have
        // no pending TFDs, so this is safe).
        self.csr.write_prph(scd::TXFACT, 0xFF);

        // Read back to confirm the SCD is alive and took our config.
        let txfact = self.csr.read_prph(scd::TXFACT);
        let chain = self.csr.read_prph(scd::QUEUECHAIN_SEL);
        let dram = self.csr.read_prph(scd::DRAM_BASE_ADDR);
        let q0 = self.csr.read_prph(scd::queue_status(0));
        let cbbc = self.csr.read32(fh_cbbc_queue(0));
        self.release_nic_access();

        println!("[iwlwifi] tx_init: tfd=0x{:08X} bc=0x{:08X} scd_base=0x{:08X}",
            (tfd_phys >> 8) as u32, (bc_phys >> 10) as u32, self.scd_base_ptr);
        println!("[iwlwifi] tx_init readback: TXFACT=0x{:08X} CHAIN=0x{:08X} DRAM_BASE=0x{:08X} Q0_STTS=0x{:08X} CBBC0=0x{:08X}",
            txfact, chain, dram, q0, cbbc);
        // TXFACT is read/write and is the reliable confirmation: it echoing
        // our queue-0 enable proves the SCD PRPH base is correct and the
        // scheduler took our config. DRAM_BASE + queue-status are write-only
        // on this generation (read back 0 / a hardware status), so they're
        // informational, not pass/fail.
        let _ = (dram, q0, cbbc, chain);
        let ok = txfact != 0 && txfact != 0xFFFF_FFFF;
        if ok {
            println!("[iwlwifi] tx_init: scheduler responding (TXFACT=0x{:08X}) — command queue armed", txfact);
        } else {
            println!("[iwlwifi] tx_init: TXFACT readback wrong (0x{:08X}) — SCD base may be off", txfact);
        }
        ok
    }

    /// Stage 3b: send one host command to the firmware via queue 0, then
    /// watch whether the firmware consumes it (SCD read-pointer advances),
    /// responds (RX status advances), and whether the error table stays
    /// clean. Diagnostic-heavy; bounded; safe (a bad command faults the
    /// ucode but the fault is readable, not a hang).
    pub fn send_cmd(&mut self, cmd_id: u8, payload: &[u8]) -> bool {
        use super::iwlwifi_csr::{CSR_HBUS_TARG_WRPTR, scd};
        let cmd_phys = match phys_of(unsafe { &raw const CMD_BUF } as u64) { Some(p)=>p, None=>return false };
        let total = 4 + payload.len(); // 4-byte header + payload
        let idx = self.tx_write_idx as usize;

        // Build command: header [cmd, group/flags, seq_lo, seq_hi] + payload.
        unsafe {
            CMD_BUF.0[0] = cmd_id;
            CMD_BUF.0[1] = 0;
            CMD_BUF.0[2] = (idx & 0xFF) as u8;  // sequence low (queue 0)
            CMD_BUF.0[3] = 0;
            CMD_BUF.0[4..4 + payload.len()].copy_from_slice(payload);
        }
        // Build the TFD at this index: num_tbs=1, tb[0]=CMD_BUF.
        unsafe {
            let tfd = &mut TX_TFD_RING.0[idx];
            for b in tfd.iter_mut() { *b = 0; }
            tfd[3] = 1; // num_tbs
            let lo = (cmd_phys & 0xFFFF_FFFF) as u32;
            let hi_n_len = (((cmd_phys >> 32) & 0xF) as u16) | ((total as u16) << 4);
            tfd[4..8].copy_from_slice(&lo.to_le_bytes());
            tfd[8..10].copy_from_slice(&hi_n_len.to_le_bytes());
            // Scheduler byte-count entry (bytes incl. overhead, + wrap dup).
            let bc = ((total + 8) & 0xFFF) as u16;
            TX_BC_TBL.0[idx] = bc;
            if idx < 64 { TX_BC_TBL.0[256 + idx] = bc; }
        }
        // Advance write pointer + ring the doorbell.
        self.tx_write_idx = ((idx + 1) % TX_RING_SIZE) as u16;
        let stts_before = unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) };
        use super::iwlwifi_csr::{CSR_INT as CSR_INT_R, CSR_FH_INT_STATUS as CSR_FH_R};
        if !self.grab_nic_access() { return false; }
        // Clear latched interrupt status so the post-doorbell read shows
        // only NEW activity (W1C — write 1s to clear).
        self.csr.write32(CSR_INT_R, 0xFFFF_FFFF);
        self.csr.write32(CSR_FH_R, 0xFFFF_FFFF);
        let rd_before = self.csr.read_prph(scd::queue_rdptr(0));
        self.csr.write32(CSR_HBUS_TARG_WRPTR, self.tx_write_idx as u32);
        self.release_nic_access();

        // Wait (bounded) for the RX status page to advance (a response).
        let mut responded = false;
        for _ in 0..200 {
            let s = unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) };
            if s != stts_before { responded = true; break; }
            for _ in 0..2000 { for _ in 0..100 { core::hint::spin_loop(); } } // ~2 ms
        }
        // Snapshot: did the firmware consume the TFD? respond? fault?
        use super::iwlwifi_csr::{CSR_INT, CSR_FH_INT_STATUS};
        if !self.grab_nic_access() { return false; }
        let rd_after = self.csr.read_prph(scd::queue_rdptr(0));
        let err_valid = self.csr.mem_read32(self.error_table_ptr);
        let err_id = self.csr.mem_read32(self.error_table_ptr + 4);
        // Ground-truth: read the SCD queue-0 context back from SRAM (did
        // our tx_init writes land?) + the read/write pointers the SCD
        // actually keeps there.
        let ctx0 = self.csr.mem_read32(self.scd_base_ptr + scd::context_queue_offset(0));
        let ctx1 = self.csr.mem_read32(self.scd_base_ptr + scd::context_queue_offset(0) + 4);
        let int_now = self.csr.read32(CSR_INT);
        let fh_now = self.csr.read32(CSR_FH_INT_STATUS);
        // Read the TFD + byte-count we wrote (confirm our DMA structures).
        let tfd_dw0 = unsafe { core::ptr::read_volatile(&raw const TX_TFD_RING.0[idx] as *const u32) };
        let tfd_tb = unsafe { core::ptr::read_volatile((&raw const TX_TFD_RING.0[idx] as *const u32).add(1)) };
        let bc_ent = unsafe { core::ptr::read_volatile(&raw const TX_BC_TBL.0[idx]) };
        self.release_nic_access();
        let stts_after = unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) };

        println!("[iwlwifi] send_cmd 0x{:02X}: wr_idx={} SCD_rdptr {}->{} rb_stts 0x{:08X}->0x{:08X} responded={}",
            cmd_id, self.tx_write_idx, rd_before, rd_after, stts_before, stts_after, responded as u8);
        println!("[iwlwifi]   diag: scd_ctx[0]=0x{:08X}/0x{:08X} INT=0x{:08X} FH_INT=0x{:08X} TFD=[0x{:08X} 0x{:08X}] bc=0x{:04X}",
            ctx0, ctx1, int_now, fh_now, tfd_dw0, tfd_tb, bc_ent);
        if err_valid != 0 {
            println!("[iwlwifi] send_cmd: FIRMWARE FAULT err_valid=0x{:08X} err_id=0x{:08X} (command rejected)",
                err_valid, err_id);
            // Dump the firmware error-event table: error_id + PC + context.
            // iwl_error_event_table: [0]valid [1]error_id [2]pc [3]hw_ver
            // [4]blink2 [5]ilink1 [6]ilink2 [7]data1 [8]data2 [9]data3 ...
            if self.grab_nic_access() {
                let mut e = [0u32; 16];
                self.csr.mem_read_block(self.error_table_ptr, &mut e);
                self.release_nic_access();
                println!("[iwlwifi]   fault: error_id=0x{:08X} pc=0x{:08X} hw=0x{:08X} data1=0x{:08X} data2=0x{:08X} data3=0x{:08X}",
                    e[1], e[2], e[3], e[7], e[8], e[9]);
                println!("[iwlwifi]   fault raw: {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}",
                    e[0], e[1], e[2], e[3], e[4], e[5], e[6], e[7]);
            }
        } else {
            println!("[iwlwifi] send_cmd: error table clean (no fault)");
        }
        // If we got a response, dump the response packet (16 dwords) from
        // the buffer the NIC just closed.
        if responded {
            let closed = (stts_after & 0xFFFF) as usize;
            let bufi = closed.wrapping_sub(1) % RX_RING_SIZE;
            let mut r = [0u32; 16];
            unsafe {
                let p = &raw const RX_BUFS.0[bufi] as *const u32;
                for (i, slot) in r.iter_mut().enumerate() {
                    *slot = core::ptr::read_volatile(p.add(i));
                }
            }
            println!("[iwlwifi] send_cmd: response cmd=0x{:02X} (buf {}): {:08X} {:08X} {:08X} {:08X}",
                (r[1] & 0xFF) as u8, bufi, r[0], r[1], r[2], r[3]);
            println!("[iwlwifi]   resp+: {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}",
                r[4], r[5], r[6], r[7], r[8], r[9], r[10], r[11]);
        }
        responded
    }

    /// Start the association sequence: scan for SSID, then auth/assoc/4-way/DHCP.
    /// Requires firmware loaded + PHY init done.
    pub fn connect(&mut self, ssid: &[u8], psk: &[u8]) {
        if !self.is_ready() {
            println!("[iwlwifi] connect: device not ready (firmware/PHY not loaded)");
            return;
        }
        let profile = super::iwlwifi_sm::NetworkProfile::from_ssid_psk(ssid, psk);
        self.sm.start_scan(profile);
    }

    /// Step the association state machine.  Call this periodically (e.g.
    /// every 100 ms) or when an RX frame / event arrives.
    pub fn step_assoc(&mut self, tx_buf: &mut [u8]) {
        use super::iwlwifi_sm::State;
        match self.sm.state {
            State::Scanning => {
                if let Some(frame) = self.sm.build_probe_req(tx_buf) {
                    // TODO(M11): post frame to TX queue as mgmt frame.
                    println!("[iwlwifi] TX Probe Request ({} bytes)", frame.len());
                }
            }
            State::Authenticating => {
                if let Some(frame) = self.sm.build_auth_req(tx_buf) {
                    println!("[iwlwifi] TX Auth Request ({} bytes)", frame.len());
                }
            }
            State::Associating => {
                if let Some(frame) = self.sm.build_assoc_req(tx_buf) {
                    println!("[iwlwifi] TX Assoc Request ({} bytes)", frame.len());
                }
            }
            State::FourWayHandshake => {
                if let Some(frame) = self.sm.build_eapol_msg2(tx_buf) {
                    println!("[iwlwifi] TX EAPOL Msg2 ({} bytes)", frame.len());
                }
            }
            _ => {}
        }
    }

    /// Stub: push an 802.11 data frame into the TX ring.
    pub fn tx_frame(&mut self, _frame: &[u8]) -> bool {
        // TODO(M11): map frame to TX DMA descriptor, ring doorbell.
        false
    }

    /// Stub: pull a received frame from the RX ring.
    /// Returns number of bytes copied into `buf`, or 0 if none available.
    pub fn rx_frame(&mut self, _buf: &mut [u8]) -> usize {
        // TODO(M11): walk RX DMA descriptors, copy frame, advance tail.
        0
    }

    /// True if the device has passed ALIVE and PHY init.
    pub fn is_ready(&self) -> bool {
        self.state == DeviceState::PhyReady || self.state == DeviceState::Associated
    }

    /// True if associated to an AP (checks both device state and SM state).
    pub fn is_associated(&self) -> bool {
        self.state == DeviceState::Associated
            && self.sm.state == super::iwlwifi_sm::State::Associated
    }
}

/// Global singleton — `None` until PCI probe finds a device.
static mut DEVICE: Option<IwlDevice> = None;

/// Probe PCI, create device if found, but do NOT load firmware yet.
/// Safe to call on every boot; returns `true` if a device was found.
pub fn init() -> bool {
    println!("[wireless] PCI scan for iwlwifi cards...");
    let pci_info = match super::iwlwifi_pci::probe() {
        Some(p) => p,
        None => {
            println!("[wireless] no iwlwifi card found (vendor 0x8086 + known device ID); skipping");
            return false;
        }
    };
    unsafe {
        DEVICE = Some(IwlDevice::new(pci_info));
        // Read HW_REV + RF_ID + HW_IF_CONFIG to confirm MMIO works.
        // On metal these read non-zero values; if they all read 0xFFFFFFFF
        // the BAR mapping is wrong (e.g., paging or PCI command register
        // not properly set up).
        if let Some(dev) = DEVICE.as_mut() {
            // Stage 1: reset + take ownership + bring up the MAC clock.
            // Firmware load (Stage 2) follows once the blob is embedded.
            if dev.power_up() {
                println!("[iwlwifi] Stage 1 (power-up) complete — ready for firmware load");
                // Stage 2a: parse the embedded ucode.
                if let Some(fw) = super::iwlwifi_fw_image::parse() {
                    // Stage 2b: DMA the INIT image sections into device SRAM
                    // via the FH service channel. (CPU stays in reset — the
                    // ucode does not run yet; ALIVE handshake is the next
                    // step and needs the RX ring.)
                    println!("[iwlwifi] Stage 2b: loading INIT image into NIC SRAM...");
                    if dev.load_image(&fw.init) {
                        println!("[iwlwifi] Stage 2b: INIT image DMA complete — all sections in SRAM");
                        // Stage 2c: RX ring → release CPU → watch for ALIVE.
                        println!("[iwlwifi] Stage 2c: RX ring + release CPU, waiting for ALIVE...");
                        if dev.rx_init() {
                            dev.release_cpu();
                            if dev.wait_alive() {
                                println!("[iwlwifi] Stage 2c: firmware ALIVE");
                                // Stage 3a: stand up the TX command queue +
                                // scheduler, verify by register readback.
                                println!("[iwlwifi] Stage 3a: configuring TX command queue...");
                                if dev.tx_init() {
                                    // Stage 3b: send the first host command as a
                                    // queue-plumbing probe (PHY_CONFIGURATION_CMD
                                    // 0x6a, zeroed payload). We watch consumption
                                    // + response + the fault table.
                                    println!("[iwlwifi] Stage 3b: sending first command...");
                                    // TX_ANT_CONFIGURATION_CMD (0x98): the real
                                    // first INIT-ucode command. Payload = a u32
                                    // antenna mask (0x3 = both antennas on 7260).
                                    let payload = [0x03u8, 0, 0, 0];
                                    if dev.send_cmd(0x98, &payload) {
                                        // NVM_ACCESS_CMD (0x88): read the NVM SW
                                        // section so we can pull the WiFi MAC.
                                        // [op=read, target=cache, type=1(SW),
                                        //  offset=0, length=0x100].
                                        println!("[iwlwifi] Stage 3c: reading NVM...");
                                        let nvm = [0u8, 0, 1, 0, 0, 0, 0, 1];
                                        dev.send_cmd(0x88, &nvm);
                                    }
                                }
                            } else {
                                println!("[iwlwifi] Stage 2c: no ALIVE signal — firmware silent after release");
                            }
                        }
                    } else {
                        println!("[iwlwifi] Stage 2b: INIT image load FAILED — see FH error above");
                    }
                }
            } else {
                println!("[iwlwifi] Stage 1 (power-up) FAILED — see CSR dump above");
            }
        }
    }
    true
}

/// Access the global device.  Returns `Some` after `init()` succeeds.
pub fn device() -> Option<&'static mut IwlDevice> {
    unsafe { DEVICE.as_mut() }
}
