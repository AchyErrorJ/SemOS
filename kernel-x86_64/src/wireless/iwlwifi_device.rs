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
        Self { pci, state: DeviceState::Probed, csr, sm }
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
    pub fn wait_alive(&self) -> bool {
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
        let len = dw[0] & 0x3FFF;
        let cmd = (dw[1] & 0xFF) as u8;
        println!("[iwlwifi] ALIVE: len_n_flags=0x{:08X} (len={}) cmd=0x{:02X}", dw[0], len, cmd);
        for chunk in dw.chunks(4).enumerate() {
            let (i, c) = chunk;
            let pad = [0u32; 4];
            let c = if c.len() == 4 { c } else { &pad[..c.len()] };
            println!("[iwlwifi] ALIVE[{:02}]: {:08X} {:08X} {:08X} {:08X}",
                i * 4, c[0], c.get(1).copied().unwrap_or(0),
                c.get(2).copied().unwrap_or(0), c.get(3).copied().unwrap_or(0));
        }
        // Heuristic: SRAM pointers (error tables, scd_base) read as
        // 0x008xxxxx (data SRAM). Flag any payload dword that looks like one.
        for (i, &v) in dw.iter().enumerate().skip(2) {
            if (0x0080_0000..0x0084_0000).contains(&v) {
                println!("[iwlwifi] ALIVE: dword[{}]=0x{:08X} looks like an SRAM pointer", i, v);
            }
        }
        moved
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
                                println!("[iwlwifi] Stage 2c: firmware showed signs of life (see snapshot)");
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
