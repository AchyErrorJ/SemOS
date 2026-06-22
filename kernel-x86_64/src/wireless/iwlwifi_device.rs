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
/// TFD ring for the data/mgmt TX queue (queue 1) — separate from the command
/// queue's ring (queue 0) so auth/assoc frames don't collide with host commands.
static mut TX1_TFD_RING: TxTfdRing = TxTfdRing([[0; 128]; TX_RING_SIZE]);
static mut TX1_WRITE_IDX: u16 = 0;
/// Scheduler byte-count table. The firmware indexes it by queue id
/// (`scd_bc_tbl[qid].tfd_offset[idx]`), 320 × u16 per queue. We size it for the
/// command queue (0) AND the data queue (1): 2 × 320 = 640 × u16, 1 KiB-aligned.
#[repr(C, align(1024))]
struct TxBcTbl([u16; 640]);
static mut TX_BC_TBL: TxBcTbl = TxBcTbl([0; 640]);
/// Staging buffer for one outbound frame: device-cmd header (4) + iwm_tx_cmd
/// (60) + 802.11 header + body, all contiguous. Page-aligned so the whole frame
/// sits in one physical page (the TFD splits it into TBs by offset).
#[repr(C, align(4096))]
struct TxFrameBuf([u8; 512]);
static mut TX_FRAME_BUF: TxFrameBuf = TxFrameBuf([0; 512]);
/// Command staging buffer (header + payload) the TFD points at.
// Command buffer for a single host command (header + payload). Sized + page
// aligned to hold the largest command we send — the LMAC SCAN request is
// ~1772 bytes and a PHY_DB section (CALIB_NCH) can be a couple KB. align(4096)
// keeps the whole buffer inside one physical page so the single-TB DMA stays
// contiguous. (TFD length field caps a single TB at 4095 bytes.)
#[repr(C, align(4096))]
struct CmdBuf([u8; 4096]);
static mut CMD_BUF: CmdBuf = CmdBuf([0; 4096]);

/// First 24 dwords of the most recent command response, captured by `send_cmd`.
/// Lets a caller read a command-specific status word (e.g. ADD_STA) without
/// re-walking the RX ring. `r[2]` is the first payload dword for the
/// `*_pdu_status` commands (binding/add-sta return their status there).
static mut LAST_RESP: [u32; 24] = [0; 24];

// ---- PHY calibration database (forwarded INIT-ucode → RUNTIME ucode) -------
// The INIT ucode runs RF calibration and reports results as CALIB_RES_NOTIF
// (0x6B) notifications. We capture each section, then replay them to the
// RUNTIME ucode via PHY_DB_CMD (0x6C) before configuring the radio — without
// this the PHY_CONTEXT_CMD asserts (err 0x14FE: radio not calibrated).
// Largest single PHY_DB section (CALIB_NCH is the big one). Capped so the
// whole PHY_DB_CMD (4-byte cmd hdr + 4-byte type/length + data) fits one TFD
// transfer block: 8 + 4087 = 4095 = the gen1 TB length limit. Truncating this
// would forward incomplete calibration → mis-tuned radio → zero RX.
const PHY_DB_BLOB_MAX: usize = 4087;
const NUM_PAPD_GROUPS: usize = 9;
const NUM_TXP_GROUPS: usize = 9;

#[derive(Copy, Clone)]
struct PhyDbEntry {
    data: [u8; PHY_DB_BLOB_MAX],
    len: usize,
}
impl PhyDbEntry {
    const fn new() -> Self { Self { data: [0; PHY_DB_BLOB_MAX], len: 0 } }
}

// Section storage: CFG(1), CALIB_NCH(2), CHG_PAPD(4)[9], CHG_TXP(5)[9].
static mut PHY_DB_CFG: PhyDbEntry = PhyDbEntry::new();
static mut PHY_DB_CALIB_NCH: PhyDbEntry = PhyDbEntry::new();
static mut PHY_DB_PAPD: [PhyDbEntry; NUM_PAPD_GROUPS] = [PhyDbEntry::new(); NUM_PAPD_GROUPS];
static mut PHY_DB_TXP: [PhyDbEntry; NUM_TXP_GROUPS] = [PhyDbEntry::new(); NUM_TXP_GROUPS];

// PHY_DB section type IDs (iwm).
const PHY_DB_CFG_T: u16 = 1;
const PHY_DB_CALIB_NCH_T: u16 = 2;
const PHY_DB_CHG_PAPD_T: u16 = 4;
const PHY_DB_CHG_TXP_T: u16 = 5;

/// Resolve the storage slot for a PHY_DB section. PAPD/TXP are per channel
/// group (`chg_id`). Single-threaded boot init, so the &'static mut is sound.
fn phy_db_entry(sec_type: u16, chg_id: u16) -> Option<&'static mut PhyDbEntry> {
    unsafe {
        match sec_type {
            PHY_DB_CFG_T => Some(&mut *core::ptr::addr_of_mut!(PHY_DB_CFG)),
            PHY_DB_CALIB_NCH_T => Some(&mut *core::ptr::addr_of_mut!(PHY_DB_CALIB_NCH)),
            PHY_DB_CHG_PAPD_T => (&mut *core::ptr::addr_of_mut!(PHY_DB_PAPD)).get_mut(chg_id as usize),
            PHY_DB_CHG_TXP_T => (&mut *core::ptr::addr_of_mut!(PHY_DB_TXP)).get_mut(chg_id as usize),
            _ => None,
        }
    }
}

// ---- Scan-result network list (deduplicated by BSSID) ----------------------
// A scan reports the same AP once per pass/iteration; the `wifi` command wants
// a clean numbered list, so beacons are recorded here de-duped by BSSID.
const MAX_NETS: usize = 32;

#[derive(Copy, Clone)]
pub struct NetEntry {
    pub bssid: [u8; 6],
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub channel: u8,
}
static mut NET_LIST: [NetEntry; MAX_NETS] =
    [NetEntry { bssid: [0; 6], ssid: [0; 32], ssid_len: 0, channel: 0 }; MAX_NETS];
static mut NET_COUNT: usize = 0;

fn net_reset() {
    unsafe { NET_COUNT = 0; }
}

/// Record a beacon, de-duplicating by BSSID. A non-hidden SSID seen later for a
/// BSSID first recorded hidden upgrades the stored name.
fn net_record(bssid: &[u8; 6], ssid: &[u8], slen: usize, channel: u8) {
    unsafe {
        let list = &mut *core::ptr::addr_of_mut!(NET_LIST);
        let count = NET_COUNT;
        for e in list.iter_mut().take(count) {
            if &e.bssid == bssid {
                if e.ssid_len == 0 && slen > 0 {
                    let n = slen.min(32);
                    e.ssid[..n].copy_from_slice(&ssid[..n]);
                    e.ssid_len = n as u8;
                }
                if channel != 0 { e.channel = channel; }
                return;
            }
        }
        if count < MAX_NETS {
            let e = &mut list[count];
            e.bssid = *bssid;
            let n = slen.min(32);
            e.ssid[..n].copy_from_slice(&ssid[..n]);
            e.ssid_len = n as u8;
            e.channel = channel;
            NET_COUNT = count + 1;
        }
    }
}

/// Sort the network list by BSSID so indices are stable across scans (beacons
/// arrive in a different order each pass, which would otherwise reshuffle the
/// numbering between `wifi` runs).
fn net_sort() {
    unsafe {
        let list = &mut *core::ptr::addr_of_mut!(NET_LIST);
        let count = NET_COUNT;
        for i in 1..count {
            let mut j = i;
            while j > 0 && list[j].bssid < list[j - 1].bssid {
                list.swap(j, j - 1);
                j -= 1;
            }
        }
    }
}

/// Print the de-duplicated network list with indices (for the `wifi` command).
fn net_print() {
    unsafe {
        let list = &*core::ptr::addr_of!(NET_LIST);
        let count = NET_COUNT;
        println!("[wifi] {} network(s) found:", count);
        for (i, e) in list.iter().take(count).enumerate() {
            let name = if e.ssid_len == 0 {
                "<hidden>"
            } else {
                core::str::from_utf8(&e.ssid[..e.ssid_len as usize]).unwrap_or("<non-utf8>")
            };
            println!("  [{}] {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}  ch{:<3} {}",
                i, e.bssid[0], e.bssid[1], e.bssid[2], e.bssid[3], e.bssid[4], e.bssid[5],
                e.channel, name);
        }
    }
}

/// Copy of network `idx` from the last scan, if present (for `wifi connect`).
pub fn net_get(idx: usize) -> Option<NetEntry> {
    unsafe {
        if idx < NET_COUNT {
            Some((&*core::ptr::addr_of!(NET_LIST))[idx])
        } else {
            None
        }
    }
}

// ---- Connection state (target + derived keys for the association engine) ----
struct ConnState {
    pmk: [u8; 32],
    ssid: [u8; 32],
    ssid_len: u8,
    bssid: [u8; 6],
    channel: u8,
    // WPA2 4-way handshake state (filled during the handshake):
    snonce: [u8; 32], // our nonce, generated on Msg1
    kck: [u8; 16],    // EAPOL-Key MIC key  (PTK[0..16])
    kek: [u8; 16],    // EAPOL-Key enc key  (PTK[16..32], unwraps the GTK in Msg3)
    tk: [u8; 16],     // CCMP data key      (PTK[32..48], installed to the firmware)
    ptk_valid: bool,
}
static mut CONN: ConnState = ConnState {
    pmk: [0; 32],
    ssid: [0; 32],
    ssid_len: 0,
    bssid: [0; 6],
    channel: 0,
    snonce: [0; 32],
    kck: [0; 16],
    kek: [0; 16],
    tk: [0; 16],
    ptk_valid: false,
};

// ---- Connect-event mailbox: drain_rx deposits results, connect() consumes them ----
const EAPOL_QUEUE_SLOTS: usize = 4;
static mut EAPOL_Q: [[u8; 512]; EAPOL_QUEUE_SLOTS] = [[0; 512]; EAPOL_QUEUE_SLOTS];
static mut EAPOL_QLENS: [usize; EAPOL_QUEUE_SLOTS] = [0; EAPOL_QUEUE_SLOTS];
static mut EAPOL_QHEAD: usize = 0;
static mut EAPOL_QTAIL: usize = 0;
static mut AUTH_SUCCESS: bool = false;
static mut ASSOC_SUCCESS: bool = false;
static mut LAST_DEAUTH_REASON: u16 = 0;

fn clear_connect_events() {
    unsafe {
        EAPOL_QHEAD = 0;
        EAPOL_QTAIL = 0;
        for len in EAPOL_QLENS.iter_mut() { *len = 0; }
        AUTH_SUCCESS = false;
        ASSOC_SUCCESS = false;
        LAST_DEAUTH_REASON = 0;
    }
}

fn push_eapol(frame: &[u8]) {
    unsafe {
        let tail = EAPOL_QTAIL;
        if (tail + 1) % EAPOL_QUEUE_SLOTS == EAPOL_QHEAD { return; } // drop if full
        let n = frame.len().min(512);
        EAPOL_Q[tail][..n].copy_from_slice(&frame[..n]);
        EAPOL_QLENS[tail] = n;
        EAPOL_QTAIL = (tail + 1) % EAPOL_QUEUE_SLOTS;
    }
}

fn take_eapol(out: &mut [u8]) -> Option<usize> {
    unsafe {
        if EAPOL_QHEAD == EAPOL_QTAIL { return None; }
        let head = EAPOL_QHEAD;
        let n = EAPOL_QLENS[head].min(out.len());
        out[..n].copy_from_slice(&EAPOL_Q[head][..n]);
        EAPOL_QLENS[head] = 0;
        EAPOL_QHEAD = (head + 1) % EAPOL_QUEUE_SLOTS;
        Some(n)
    }
}

fn was_auth_success() -> bool {
    unsafe { core::mem::replace(&mut AUTH_SUCCESS, false) }
}

fn was_assoc_success() -> bool {
    unsafe { core::mem::replace(&mut ASSOC_SUCCESS, false) }
}

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
        super::wifidbg!("[iwlwifi] HW_REV=0x{:08X} RF_ID=0x{:08X}", rev, rf_id);
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
                super::wifidbg!("[iwlwifi] prepare_card_hw: NIC_READY after {} attempt(s)", attempt + 1);
                return true;
            }
        }
        super::wifidbg!("[iwlwifi] prepare_card_hw: NIC never became ready (BIOS/ME may hold the card)");
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
            super::wifidbg!("[iwlwifi] apm_init: MAC clock never became ready (GP_CNTRL=0x{:08X})",
                self.csr.read32(CSR_GP_CNTRL));
            return false;
        }
        // 7000-series: enable the DMA clock via APMG, disable L1-Active,
        // clear the RF-kill monitor disable. These are PRPH (indirect).
        self.csr.write_prph(apmg::CLK_EN_REG, apmg::CLK_VAL_DMA_CLK_RQT);
        for _ in 0..20 { for _ in 0..100 { core::hint::spin_loop(); } } // ~20 µs
        self.csr.set_bits_prph(apmg::PCIDEV_STT_REG, apmg::PCIDEV_STT_VAL_L1_ACT_DIS);
        self.csr.clear_bits_prph(apmg::RTC_INT_STT_REG, apmg::RTC_INT_STT_RFKILL);
        super::wifidbg!("[iwlwifi] apm_init: MAC clock ready, APMG configured (GP_CNTRL=0x{:08X})",
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
        super::wifidbg!("[iwlwifi] power_up: HW_REV=0x{:08X} RF_ID=0x{:08X}", rev, rf);
        if rev == 0xFFFF_FFFF {
            super::wifidbg!("[iwlwifi] power_up: CSRs read all-ones — BAR/PCI mapping wrong; aborting");
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
        super::wifidbg!("[iwlwifi] power_up OK: HW_IF_CONFIG=0x{:08X} GP_CNTRL=0x{:08X}", cfg, gpc);
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
        super::wifidbg!("[iwlwifi] grab_nic_access timed out (GP_CNTRL=0x{:08X})",
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
        super::wifidbg!("[iwlwifi] init_phy: STUB — needs NVM parse + channel table");
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
        super::wifidbg!("[iwlwifi] rx_init: ring base=0x{:08X} stts=0x{:08X} cfg=0x{:08X}",
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
            super::wifidbg!("[iwlwifi] ALIVE: unexpected cmd=0x{:02X} status=0x{:04X} — dumping payload",
                cmd, status);
            for (i, &v) in dw.iter().enumerate() {
                println!("[iwlwifi]   ALIVE dw[{:02}]=0x{:08X}", i, v);
            }
            return moved;
        }
        super::wifidbg!("[iwlwifi] ALIVE OK (status=0xCAFE): scd_base=0x{:08X} err_table=0x{:08X} log_table=0x{:08X}",
            self.scd_base_ptr, self.error_table_ptr, self.log_table_ptr);

        // Peek the firmware error table: first dword is a "valid" flag —
        // non-zero means the ucode logged a fatal error during boot.
        if self.grab_nic_access() {
            let valid = self.csr.mem_read32(self.error_table_ptr);
            let err_id = self.csr.mem_read32(self.error_table_ptr + 4);
            self.release_nic_access();
            if valid != 0 {
                super::wifidbg!("[iwlwifi] ALIVE: firmware error table VALID=0x{:08X} error_id=0x{:08X} (ucode logged a fault!)",
                    valid, err_id);
            } else {
                super::wifidbg!("[iwlwifi] ALIVE: firmware error table clean (no fault)");
            }
        }
        self.state = DeviceState::Alive;
        true
    }

    /// Reload firmware and bring it to ALIVE. Halts the currently-running
    /// ucode (SW reset), re-runs the ownership + MAC-clock bring-up, loads
    /// `img`'s sections into SRAM, re-arms the RX ring, releases the CPU and
    /// waits for the ALIVE notification. Used to switch from the INIT ucode
    /// (after calibration) to the operational RUNTIME ucode — the 2nd ALIVE.
    /// `label` is only for the log. Mirrors `power_up`'s reset path so the
    /// CPU is cleanly back in reset before we overwrite its instruction SRAM.
    pub fn load_and_alive(&mut self, img: &super::iwlwifi_fw_image::FwImage, label: &str) -> bool {
        println!("[iwlwifi] reloading {} firmware ({} section(s), {} bytes)...",
            label, img.count, img.total_bytes());
        self.sw_reset();
        if !self.prepare_card_hw() {
            println!("[iwlwifi] {}: prepare_card_hw after reset FAILED", label);
            return false;
        }
        if !self.apm_init() {
            println!("[iwlwifi] {}: apm_init after reset FAILED", label);
            return false;
        }
        if !self.load_image(img) {
            println!("[iwlwifi] {}: load_image FAILED", label);
            return false;
        }
        if !self.rx_init() {
            println!("[iwlwifi] {}: rx_init FAILED", label);
            return false;
        }
        self.release_cpu();
        self.wait_alive()
    }

    /// Stage 3a: set up the TX command queue (queue 0) + configure the
    /// scheduler, then read the scheduler registers back to confirm it's
    /// responding before we trust it. Requires ALIVE (scd_base_ptr set).
    pub fn tx_init(&mut self) -> bool {
        use super::iwlwifi_csr::{scd, fh_cbbc_queue};
        if self.scd_base_ptr == 0 {
            super::wifidbg!("[iwlwifi] tx_init: no scd_base (not ALIVE?)");
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
        // Reset queue-0 read pointer + the hardware write pointer. Keep the
        // software mirror in sync — after a runtime-fw reload this is called
        // a second time and tx_write_idx must restart from 0, or the next
        // command lands at a stale index ahead of the scheduler.
        self.csr.write_prph(scd::queue_rdptr(0), 0);
        self.csr.write32(CSR_HBUS_TARG_WRPTR, 0);
        self.tx_write_idx = 0;
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

        super::wifidbg!("[iwlwifi] tx_init: tfd=0x{:08X} bc=0x{:08X} scd_base=0x{:08X}",
            (tfd_phys >> 8) as u32, (bc_phys >> 10) as u32, self.scd_base_ptr);
        super::wifidbg!("[iwlwifi] tx_init readback: TXFACT=0x{:08X} CHAIN=0x{:08X} DRAM_BASE=0x{:08X} Q0_STTS=0x{:08X} CBBC0=0x{:08X}",
            txfact, chain, dram, q0, cbbc);
        // TXFACT is read/write and is the reliable confirmation: it echoing
        // our queue-0 enable proves the SCD PRPH base is correct and the
        // scheduler took our config. DRAM_BASE + queue-status are write-only
        // on this generation (read back 0 / a hardware status), so they're
        // informational, not pass/fail.
        let _ = (dram, q0, cbbc, chain);
        let ok = txfact != 0 && txfact != 0xFFFF_FFFF;
        if ok {
            super::wifidbg!("[iwlwifi] tx_init: scheduler responding (TXFACT=0x{:08X}) — command queue armed", txfact);
        } else {
            super::wifidbg!("[iwlwifi] tx_init: TXFACT readback wrong (0x{:08X}) — SCD base may be off", txfact);
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

        super::wifidbg!("[iwlwifi] send_cmd 0x{:02X}: wr_idx={} SCD_rdptr {}->{} rb_stts 0x{:08X}->0x{:08X} responded={}",
            cmd_id, self.tx_write_idx, rd_before, rd_after, stts_before, stts_after, responded as u8);
        super::wifidbg!("[iwlwifi]   diag: scd_ctx[0]=0x{:08X}/0x{:08X} INT=0x{:08X} FH_INT=0x{:08X} TFD=[0x{:08X} 0x{:08X}] bc=0x{:04X}",
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
            super::wifidbg!("[iwlwifi] send_cmd: error table clean (no fault)");
        }
        // If we got a response, dump the response packet (16 dwords) from
        // the buffer the NIC just closed.
        if responded {
            let closed = (stts_after & 0xFFFF) as usize;
            let bufi = closed.wrapping_sub(1) % RX_RING_SIZE;
            let mut r = [0u32; 24];
            unsafe {
                let p = &raw const RX_BUFS.0[bufi] as *const u32;
                for (i, slot) in r.iter_mut().enumerate() {
                    *slot = core::ptr::read_volatile(p.add(i));
                }
            }
            // Stash the response so callers can read a command-specific status
            // (e.g. ADD_STA) without re-parsing the RX ring or eyeballing dumps.
            unsafe { LAST_RESP = r; }
            super::wifidbg!("[iwlwifi] send_cmd: response cmd=0x{:02X} (buf {}): {:08X} {:08X} {:08X} {:08X}",
                (r[1] & 0xFF) as u8, bufi, r[0], r[1], r[2], r[3]);
            super::wifidbg!("[iwlwifi]   resp[4..12]: {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}",
                r[4], r[5], r[6], r[7], r[8], r[9], r[10], r[11]);
            super::wifidbg!("[iwlwifi]   resp[12..20]: {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}",
                r[12], r[13], r[14], r[15], r[16], r[17], r[18], r[19]);
            // Extract the station MAC from the NVM HW-section read. The
            // section data starts at r[4]; the HW section is big-endian 16-bit
            // words (device 0x08B2 + vendor 0x8086 + subsys 0xC270 verified at
            // words 8/9/0x0A). The MAC is at word HW_ADDR=0x15 (byte 42),
            // stored as iwlwifi pair-swapped bytes. Confirmed on the T540p:
            // byte-42 swapped = E8:2A:EA:60:AD:BF = the card's real WiFi MAC.
            if cmd_id == 0x88 {
                let byte = |k: usize| -> u8 {
                    ((r[4 + k / 4] >> ((k % 4) * 8)) & 0xFF) as u8
                };
                let mac: MacAddr = [
                    byte(43), byte(42), byte(45), byte(44), byte(47), byte(46),
                ];
                super::wifidbg!("[iwlwifi] station MAC = {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} (NVM HW word 0x15)",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
                self.sm.sta_mac = mac;
            }
        }
        responded
    }

    /// Poll the RX ring for ~`budget_ms` and report each notification the
    /// firmware pushes. Used right after a SCAN request to observe the
    /// received beacons/probe-responses (RX_MPDU 0xC1/0xC0) and the
    /// `SCAN_OFFLOAD_COMPLETE` (0x6D). Read-only: it does not recycle RX
    /// buffer descriptors, so it relies on the 256-deep ring being enough for
    /// one short scan.
    pub fn drain_rx(&mut self, from: usize, budget_ms: u32) {
        let read_stts = || (unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) } & 0xFFFF) as usize;
        // Start from `from` (RX count before the SCAN was sent) so we don't
        // miss the SCAN_COMPLETE / beacons that landed while send_cmd was still
        // consuming the scan echo — same fix as capture_calib.
        let mut last = from;
        let mut frames = 0u32;
        let mut phy = 0u32;
        let mut other = 0u32;
        let iters = (budget_ms / 5).max(1);
        for _ in 0..iters {
            let cur = read_stts();
            while last != cur {
                let bufi = last % RX_RING_SIZE;
                let base = unsafe { &raw const RX_BUFS.0[bufi] as *const u8 };
                let rb = |off: usize| unsafe { core::ptr::read_volatile(base.add(off)) };
                let rd32 = |off: usize| (rb(off) as u32) | ((rb(off + 1) as u32) << 8)
                    | ((rb(off + 2) as u32) << 16) | ((rb(off + 3) as u32) << 24);
                // A single 4 KB RX buffer can hold MULTIPLE packed packets
                // (iwm_rx_pkt): the firmware packs e.g. RX_PHY (0xC0) then
                // RX_MPDU (0xC1) back-to-back. Walk them — each packet is
                // len_n_flags(4) + payload, next at +roundup(total, 0x40).
                let mut off = 0usize;
                loop {
                    if off + 8 > 4096 { break; }
                    let lnf = rd32(off);
                    let plen = (lnf & 0x3FFF) as usize; // payload (hdr + data)
                    if plen == 0 || lnf == 0xFFFF_FFFF { break; } // invalid/empty → end
                    let total = 4 + plen;
                    if total < 8 || off + total > 4096 { break; }
                    let cmd = rb(off + 4); // pkt->hdr.code
                    // pkt->data starts at off+8 (after len_n_flags + 4-byte hdr).
                    match cmd {
                        0x6D => println!("[iwlwifi] *** SCAN COMPLETE (0x6D) status=0x{:08X} ***", rd32(off + 8)),
                        0xE7 => println!("[iwlwifi] scan iteration complete (0xE7)"),
                        0xC1 => {
                            // RX MPDU: data = rx_mpdu_res_start(4) then 802.11 frame.
                            // frame at off+12; beacon BSSID=addr3 (frame+16),
                            // fixed params 12B, IEs from frame+36.
                            let f = off + 12;
                            let fc0 = rb(f);
                            if fc0 == 0x80 || fc0 == 0x50 {
                                let frame_len = (((rb(off + 8) as usize) | ((rb(off + 9) as usize) << 8))).min(1024);
                                let mut ssid = [0u8; 33];
                                let mut slen = 0usize;
                                let mut channel = 0u8;
                                let mut ie = 36usize; // IE start (rel. to frame)
                                while ie + 2 <= frame_len {
                                    let id = rb(f + ie);
                                    let l = rb(f + ie + 1) as usize;
                                    if id == 0 { slen = l.min(32); for i in 0..slen { ssid[i] = rb(f + ie + 2 + i); } }
                                    else if id == 3 && l >= 1 { channel = rb(f + ie + 2); } // DS Param Set
                                    ie += 2 + l;
                                }
                                frames += 1;
                                let bssid = [rb(f + 16), rb(f + 17), rb(f + 18), rb(f + 19), rb(f + 20), rb(f + 21)];
                                net_record(&bssid, &ssid[..slen], slen, channel);
                                let name = core::str::from_utf8(&ssid[..slen]).unwrap_or("<non-utf8>");
                                println!("[iwlwifi] beacon #{}: BSSID {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}  ch{} SSID \"{}\"",
                                    frames, bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5],
                                    channel, if slen == 0 { "<hidden>" } else { name });
                            } else if fc0 == 0xB0 {
                                // Authentication response from the AP. Body @
                                // frame+24: algo(2), seq(2), status(2).
                                frames += 1;
                                let seq = (rb(f + 26) as u16) | ((rb(f + 27) as u16) << 8);
                                let st = (rb(f + 28) as u16) | ((rb(f + 29) as u16) << 8);
                                if st == 0 { unsafe { AUTH_SUCCESS = true; } }
                                println!("[iwlwifi] *** AUTH RESPONSE from AP: seq={} status={} ({}) ***",
                                    seq, st, if st == 0 { "SUCCESS" } else { "rejected" });
                            } else if fc0 == 0x10 {
                                // Association response: cap(2), status(2)@frame+26, aid(2)@frame+28.
                                frames += 1;
                                let st = (rb(f + 26) as u16) | ((rb(f + 27) as u16) << 8);
                                let aid = (rb(f + 28) as u16) | ((rb(f + 29) as u16) << 8);
                                if st == 0 { unsafe { ASSOC_SUCCESS = true; } }
                                println!("[iwlwifi] *** ASSOC RESPONSE from AP: status={} aid={} ({}) ***",
                                    st, aid & 0x3FFF, if st == 0 { "SUCCESS" } else { "rejected" });
                            } else if fc0 == 0xC0 {
                                frames += 1;
                                let reason = (rb(f + 24) as u16) | ((rb(f + 25) as u16) << 8);
                                unsafe { LAST_DEAUTH_REASON = reason; }
                                println!("[iwlwifi] *** DEAUTH from AP: reason={} ***", reason);
                            } else if fc0 & 0x0C == 0x08 {
                                // Data frame (type=2): look for EAPOL-Key inside LLC/SNAP.
                                frames += 1;
                                let frame_len = (((rb(off + 8) as usize) | ((rb(off + 9) as usize) << 8))).min(1024);
                                let mut hdr_len = 24usize;
                                if fc0 & 0x80 != 0 { hdr_len += 2; } // QoS Control present
                                // Pre-key EAPOL is never encrypted; ignore protected frames.
                                if fc0 & 0x40 == 0 && frame_len >= hdr_len + 8 + 4 {
                                    let llc = f + hdr_len;
                                    if rb(llc) == 0xAA && rb(llc + 1) == 0xAA && rb(llc + 2) == 0x03
                                        && rb(llc + 3) == 0x00 && rb(llc + 4) == 0x00 && rb(llc + 5) == 0x00
                                        && rb(llc + 6) == 0x88 && rb(llc + 7) == 0x8E
                                    {
                                        let eapol_start = llc + 8;
                                        let body_len = ((rb(eapol_start + 2) as usize) << 8)
                                            | (rb(eapol_start + 3) as usize);
                                        let eapol_total = 4 + body_len;
                                        if eapol_total > 0 && eapol_total <= 512
                                            && eapol_start + eapol_total <= frame_len
                                        {
                                            let mut eapol = [0u8; 512];
                                            for i in 0..eapol_total {
                                                eapol[i] = rb(eapol_start + i);
                                            }
                                            push_eapol(&eapol[..eapol_total]);
                                            println!("[wifi] RX EAPOL-Key {} bytes (data frame fc0=0x{:02X})",
                                                eapol_total, fc0);
                                        }
                                    }
                                }
                            } else {
                                frames += 1;
                                println!("[iwlwifi] RX frame #{} (fc0=0x{:02X})", frames, fc0);
                            }
                        }
                        0xC0 => phy += 1, // RX PHY info — radio received a frame
                        _ => {
                            other += 1;
                            if other <= 12 {
                                println!("[iwlwifi] notif cmd=0x{:02X}: {:08X} {:08X}", cmd, rd32(off + 8), rd32(off + 12));
                            }
                        }
                    }
                    off += (total + 0x3F) & !0x3F; // next packet (64-byte aligned)
                }
                last = (last + 1) & 0xFFFF;
            }
            for _ in 0..5000 { for _ in 0..100 { core::hint::spin_loop(); } } // ~5 ms
        }
        println!("[iwlwifi] drain_rx done: {} beacon(s)/MPDU(0xC1), {} phy-info(0xC0), {} other notif(s)",
            frames, phy, other);
    }

    /// Add the auxiliary station (sta_id = 1) the firmware needs in its table
    /// before it will scan, and enable that station's TX queue (15) in the
    /// scheduler. `iwm_init_hw` does exactly this ("Add auxiliary station for
    /// scanning") right before scanning becomes possible. The 7260 is pre-DQA
    /// and pre-STA_TYPE, so the queue is a plain AC queue on TX FIFO MCAST(5)
    /// and ADD_STA takes the 44-byte v7 payload. Returns the send result; the
    /// firmware's ADD_STA status (0x1 = success) appears in the response dump.
    pub fn add_aux_station(&mut self) -> bool {
        use super::iwlwifi_csr::{scd, fh_cbbc_queue, CSR_HBUS_TARG_WRPTR};
        const AUX_QUEUE: u32 = 15;
        const TX_FIFO_MCAST: u32 = 5;
        const MAC_INDEX_AUX: u32 = 4;

        // --- enable aux TX queue 15 in the scheduler (iwm_enable_ac_txq) ---
        let tfd_phys = match phys_of(unsafe { &raw const TX_TFD_RING } as u64) { Some(p)=>p, None=>return false };
        if !self.grab_nic_access() { return false; }
        let scd_base = self.scd_base_ptr;
        self.csr.write32(CSR_HBUS_TARG_WRPTR, AUX_QUEUE << 8);
        self.csr.clear_bits_prph(scd::AGGR_SEL, 1 << AUX_QUEUE);
        self.csr.write_prph(scd::queue_rdptr(AUX_QUEUE), 0);
        // Point the queue's TFD-ring base at the (idle) command ring — a
        // passive scan never transmits, so this queue is never walked.
        self.csr.write32(fh_cbbc_queue(AUX_QUEUE as u64), (tfd_phys >> 8) as u32);
        self.csr.mem_write32(scd_base + scd::context_queue_offset(AUX_QUEUE), 0);
        let frame_limit: u32 = 64;
        self.csr.mem_write32(scd_base + scd::context_queue_offset(AUX_QUEUE) + 4,
            ((frame_limit << scd::CTX_WIN_SIZE_POS) & 0x7F)
            | ((frame_limit << scd::CTX_FRAME_LIMIT_POS) & 0x7F_0000));
        self.csr.write_prph(scd::queue_status(AUX_QUEUE), scd::queue_enable_val(TX_FIFO_MCAST));
        self.release_nic_access();

        // --- ADD_STA (0x18), v7 (44-byte) payload for the aux station ---
        let mut cmd = [0u8; 44];
        // [0] add_modify = 0 (ADD); [1] awake_acs = 0
        cmd[2..4].copy_from_slice(&0xFFFFu16.to_le_bytes()); // tid_disable_tx
        cmd[4..8].copy_from_slice(&MAC_INDEX_AUX.to_le_bytes()); // mac_id_n_color (color 0)
        // [8..14] addr = 0 (aux station has no MAC); [16] sta_id
        cmd[16] = 1; // sta_id = IWM_AUX_STA_ID
        cmd[40..44].copy_from_slice(&(1u32 << AUX_QUEUE).to_le_bytes()); // tfd_queue_msk
        println!("[iwlwifi] add_aux_sta: queue {} enabled, sending ADD_STA sta_id=1", AUX_QUEUE);
        self.send_cmd(0x18, &cmd)
    }

    /// Current RX fill count (rb_stts low 16) — the number of buffers the NIC
    /// has filled so far. Used to mark a start point before a command so a
    /// later drain can scan every notification that landed since.
    fn rx_count(&self) -> usize {
        (unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) } & 0xFFFF) as usize
    }

    /// Drain the RX ring during INIT-ucode calibration and capture each
    /// CALIB_RES_NOTIF_PHY_DB (0x6B) section into PHY_DB storage. Stops early
    /// on INIT_COMPLETE_NOTIF (0x04) or after `budget_ms`. Called in place of
    /// the old blind settle-spin after the INIT PHY_CFG.
    pub fn capture_calib(&mut self, from: usize, budget_ms: u32) {
        let read_stts = || (unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) } & 0xFFFF) as usize;
        // Start scanning from `from` (the RX count BEFORE PHY_CFG was sent) so
        // we don't miss calibration notifications that landed while send_cmd
        // was still consuming the PHY_CFG echo.
        let mut last = from;
        let mut sections = 0u32;
        let mut init_complete = false;
        let iters = (budget_ms / 5).max(1);
        for _ in 0..iters {
            let cur = read_stts();
            while last != cur {
                let bufi = last % RX_RING_SIZE;
                let base = unsafe { &raw const RX_BUFS.0[bufi] as *const u8 };
                let rd8 = |off: usize| unsafe { core::ptr::read_volatile(base.add(off)) };
                let rd16 = |off: usize| (rd8(off) as u16) | ((rd8(off + 1) as u16) << 8);
                // iwl_rx_packet: [0..4] len_n_flags, [4] cmd, [5] group, [6..8] seq.
                let cmd = rd8(4);
                match cmd {
                    0x6B => {
                        // iwm_calib_res_notif_phy_db @ byte 8: type, length, data.
                        let sec_type = rd16(8);
                        let length = rd16(10) as usize;
                        let chg_id = rd16(12); // first u16 of data = channel group
                        if let Some(entry) = phy_db_entry(sec_type, chg_id) {
                            let n = length.min(PHY_DB_BLOB_MAX);
                            for i in 0..n {
                                entry.data[i] = rd8(12 + i);
                            }
                            entry.len = n;
                            sections += 1;
                            if length > PHY_DB_BLOB_MAX {
                                println!("[iwlwifi] calib: section type={} chg={} len={} TRUNCATED to {}!",
                                    sec_type, chg_id, length, PHY_DB_BLOB_MAX);
                            } else {
                                println!("[iwlwifi] calib: section type={} chg={} len={}", sec_type, chg_id, length);
                            }
                        }
                    }
                    0x04 => { init_complete = true; }
                    _ => {}
                }
                last = (last + 1) & 0xFFFF;
            }
            if init_complete { break; }
            for _ in 0..5000 { for _ in 0..100 { core::hint::spin_loop(); } } // ~5 ms
        }
        super::wifidbg!("[iwlwifi] capture_calib: {} section(s), init_complete={}", sections, init_complete as u8);
    }

    /// Send one captured PHY_DB section to the runtime ucode via PHY_DB_CMD
    /// (0x6C). Payload = iwm_phy_db_cmd { type:u16, length:u16, data[length] }.
    /// Skips empty sections (the 7260 doesn't populate all 9 PAPD/TXP groups).
    fn send_one_phy_db(&mut self, sec_type: u16, e: &PhyDbEntry) {
        if e.len == 0 {
            return;
        }
        let n = e.len.min(PHY_DB_BLOB_MAX);
        let mut buf = [0u8; 4 + PHY_DB_BLOB_MAX];
        buf[0..2].copy_from_slice(&sec_type.to_le_bytes());
        buf[2..4].copy_from_slice(&(n as u16).to_le_bytes());
        buf[4..4 + n].copy_from_slice(&e.data[..n]);
        self.send_cmd(0x6c, &buf[..4 + n]);
    }

    /// Forward all captured PHY calibration sections to the runtime ucode, in
    /// iwm order: CFG, CALIB_NCH, every PAPD group, every TXP group. Must run
    /// after the runtime ALIVE and before PHY_CONTEXT_CMD (the radio config
    /// asserts without calibration).
    pub fn send_phy_db(&mut self) {
        super::wifidbg!("[iwlwifi] PHY_DB: forwarding calibration to runtime ucode...");
        self.send_one_phy_db(PHY_DB_CFG_T, unsafe { &*core::ptr::addr_of!(PHY_DB_CFG) });
        self.send_one_phy_db(PHY_DB_CALIB_NCH_T, unsafe { &*core::ptr::addr_of!(PHY_DB_CALIB_NCH) });
        let papd = unsafe { &*core::ptr::addr_of!(PHY_DB_PAPD) };
        for g in 0..NUM_PAPD_GROUPS {
            self.send_one_phy_db(PHY_DB_CHG_PAPD_T, &papd[g]);
        }
        let txp = unsafe { &*core::ptr::addr_of!(PHY_DB_TXP) };
        for g in 0..NUM_TXP_GROUPS {
            self.send_one_phy_db(PHY_DB_CHG_TXP_T, &txp[g]);
        }
    }

    /// Send the device power command (`IWM_POWER_TABLE_CMD` 0x77) with power
    /// save DISABLED (flags=0) so the radio stays fully awake — a power-saving
    /// radio can accept a scan but never leave its idle state to run it.
    pub fn send_power_awake(&mut self) {
        let cmd = [0u8; 4]; // flags=0 (no power save), reserved=0
        println!("[iwlwifi] POWER_TABLE: power-save disabled (radio stays awake)");
        self.send_cmd(0x77, &cmd);
    }

    /// Send the BT coexistence config (`IWM_BT_CONFIG` 0x9B). The 7260 is a
    /// combo Wi-Fi+Bluetooth card; the firmware gates radio operations on a BT
    /// coex configuration. Without it a scan is *accepted* but never executes
    /// (no SCAN_COMPLETE, no beacons). mode = BT_COEX_WIFI(3), enabled_modules
    /// = BT_COEX_HIGH_BAND_RET(0x10). iwm sends this in init_hw before the scan.
    pub fn send_bt_init(&mut self) {
        let cmd = [0x03u8, 0, 0, 0, 0x10, 0, 0, 0];
        println!("[iwlwifi] BT_CONFIG: coex mode=WIFI");
        self.send_cmd(0x9b, &cmd);
    }

    /// Add PHY context 0 (`IWM_PHY_CONTEXT_CMD` 0x08) — defines the band /
    /// channel / antenna config the radio tunes to. `iwm_init_hw` adds the PHY
    /// contexts right after the aux station; without one the firmware accepts
    /// a scan but never goes on-channel (so: no beacons). Non-UHB 36-byte
    /// variant (the 7260 has no ULTRA_HB_CHANNELS). 2.4 GHz / 20 MHz / channel
    /// 1 (the scan overrides the channel per dwell), both chains valid.
    pub fn add_phy_context(&mut self) -> bool {
        self.phy_context(1, 1) // ADD, channel 1 (scan default)
    }

    /// PHY_CONTEXT_CMD (0x08) for `action` (1=ADD, 2=MODIFY) on `channel`.
    /// Connecting MODIFY-retunes the existing ctxt 0 to the AP's channel.
    pub fn phy_context(&mut self, action: u32, channel: u8) -> bool {
        let mut cmd = [0u8; 36];
        let put32 = |c: &mut [u8], o: usize, v: u32| c[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // [0] id_and_color = ID_AND_COLOR(id 0, color 0) = 0
        put32(&mut cmd, 4, action);
        // [8] apply_time = 0 (immediate); [12] tx_param_color = 0
        cmd[16] = 1; // ci.band = IWM_PHY_BAND_24
        cmd[17] = channel; // ci.channel
        cmd[18] = 0; // ci.width = IWM_PHY_VHT_CHANNEL_MODE20
        cmd[19] = 0; // ci.ctrl_pos = IWM_PHY_VHT_CTRL_POS_1_BELOW
        put32(&mut cmd, 20, 0x3); // txchain_info = valid_tx_ant (both)
        put32(&mut cmd, 24, (0x3 << 1) | (1 << 10) | (1 << 12)); // rxchain_info = 0x1406
        println!("[iwlwifi] PHY_CONTEXT_CMD action={} id=0 ch={} 2.4GHz/20MHz", action, channel);
        self.send_cmd(0x08, &cmd)
    }

    /// MAC_CONTEXT_CMD (0x28) — create/modify the station MAC context for the
    /// connection (iwm_mac_ctxt_cmd). 152-byte struct: common header + AC-QoS
    /// array (zeroed) + the STA union (zeroed pre-assoc). The fields that
    /// matter for ADD: type=BSS_STA, our node addr, the AP's BSSID, and the
    /// ACCEPT_GRP filter. MAC id 0, color 0. `action` 1=ADD, 2=MODIFY.
    pub fn mac_context(&mut self, action: u32, bssid: &[u8; 6]) -> bool {
        // sizeof(struct iwm_mac_ctx_cmd) = 148: 100-byte common+QoS header +
        // 48-byte union (largest member is p2p_sta). The firmware validates the
        // command payload length against this exact API size — sending 152 (or
        // 144) trips a structural assert (err_id 0x66), which is why the fault
        // was invariant to every field value I tried.
        let mut cmd = [0u8; 148];
        let put32 = |c: &mut [u8], o: usize, v: u32| c[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // [0] id_and_color = ID_AND_COLOR(mac id 0, color 0) = 0
        put32(&mut cmd, 4, action); // action
        put32(&mut cmd, 8, 5); // mac_type = IWM_FW_MAC_TYPE_BSS_STA
        put32(&mut cmd, 12, 0); // tsf_id = IWM_TSF_ID_A
        cmd[16..22].copy_from_slice(&self.sm.sta_mac); // node_addr = our MAC
        cmd[24..30].copy_from_slice(bssid); // bssid_addr = the AP
        put32(&mut cmd, 32, 0x0F); // cck_rates: 1/2/5.5/11 (CCK basic bitmap)
        put32(&mut cmd, 36, 0x15); // ofdm_rates: 6/12/24 (OFDM basic bitmap)
        // [40] protection_flags, [44] cck_short_preamble, [48] short_slot = 0
        // filter_flags: ACCEPT_GRP (1<<2) | IN_BEACON (1<<6). The reference
        // sets IN_BEACON for the un-associated ADD so the firmware keeps
        // delivering the AP's beacons (we need them for the auth/assoc timing).
        put32(&mut cmd, 52, (1 << 2) | (1 << 6));
        // QoS EDCA: ac[5] @60 (u16 cw_min, cw_max; u8 aifsn, fifos_mask; u16 txop).
        // Indexed by TX fifo (iwm_ac_to_tx_fifo): ac[0]=BK ac[1]=BE ac[2]=VI
        // ac[3]=VO; fifos_mask = 1<<txf. Standard 802.11 station EDCA defaults.
        let put16 = |c: &mut [u8], o: usize, v: u16| c[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let acs: [(u16, u16, u8, u16); 4] = [
            (15, 1023, 7, 0),  // ac[0] = AC_BK (txf 0)
            (15, 1023, 3, 0),  // ac[1] = AC_BE (txf 1)
            (7, 15, 2, 94),    // ac[2] = AC_VI (txf 2)
            (3, 7, 2, 47),     // ac[3] = AC_VO (txf 3)
        ];
        for (i, &(cwmin, cwmax, aifsn, txop)) in acs.iter().enumerate() {
            let o = 60 + i * 8;
            put16(&mut cmd, o, cwmin);
            put16(&mut cmd, o + 2, cwmax);
            cmd[o + 4] = aifsn;
            cmd[o + 5] = 1 << i; // fifos_mask
            put16(&mut cmd, o + 6, txop);
        }
        // qos_flags @56 = 0: the node has NOT negotiated QoS yet (UPDATE_EDCA is
        // only set post-assoc). STA union @100 stays ENTIRELY ZEROED for the ADD
        // — the reference iwm_mac_ctxt_cmd does NOT fill bi/dtim until the node
        // is associated (is_assoc=0 means the firmware never touches the timing
        // math, so there is no divide-by-zero to guard against).
        println!("[iwlwifi] MAC_CONTEXT_CMD action={} BSS_STA node={:02X}:..:{:02X} bssid={:02X}:..:{:02X} (148B)",
            action, self.sm.sta_mac[0], self.sm.sta_mac[5], bssid[0], bssid[5]);
        self.send_cmd(0x28, &cmd)
    }

    /// BINDING_CONTEXT_CMD (0x2b) — bind our MAC (id 0) to PHY context 0
    /// (iwm_binding_cmd). The 7260 has no CDB support, so it uses the v1 struct
    /// (24 bytes, no lmac_id). The binding is what actually associates the MAC
    /// with a tuned radio; without it the firmware has a MAC and a PHY but no
    /// link between them. `action` 1=ADD, 3=REMOVE.
    pub fn binding_context(&mut self, action: u32) -> bool {
        // sizeof(struct iwm_binding_cmd_v1) = 24: id_and_color(4) + action(4) +
        // macs[3](12) + phy(4). (The v2 struct adds a 4-byte lmac_id, only sent
        // when the ucode advertises BINDING_CDB_SUPPORT — the 7260 does not.)
        let mut cmd = [0u8; 24];
        let put32 = |c: &mut [u8], o: usize, v: u32| c[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // id_and_color @0 = ID_AND_COLOR(phyctxt id 0, color 0) = 0.
        put32(&mut cmd, 4, action); // action
        // macs[0] @8 = our MAC's ID_AND_COLOR (mac id 0, color 0) = 0.
        put32(&mut cmd, 8, 0);
        // macs[1], macs[2] @12,@16 = IWM_FW_CTXT_INVALID (unused binding slots).
        put32(&mut cmd, 12, 0xFFFF_FFFF);
        put32(&mut cmd, 16, 0xFFFF_FFFF);
        // phy @20 = ID_AND_COLOR(phyctxt id 0, color 0) = 0.
        put32(&mut cmd, 20, 0);
        println!("[iwlwifi] BINDING_CONTEXT_CMD action={} mac0->phy0 (24B)", action);
        self.send_cmd(0x2b, &cmd)
    }

    /// ADD_STA (0x18) — register the AP itself as a station in the firmware's
    /// station table (iwm_add_sta_cmd, update=0). This is the *real* station
    /// (sta_id 0 = IWM_STATION_ID), distinct from the aux scan station (id 1).
    /// The 7260 lacks the STA_TYPE ucode API, so it uses the v7 struct (44
    /// bytes, no station_type / DQA fields). Pre-association we register it
    /// non-HT with the legacy AC data queues 0-3.
    pub fn add_station(&mut self, bssid: &[u8; 6]) -> bool {
        // sizeof(struct iwm_add_sta_cmd_v7) = 44.
        let mut cmd = [0u8; 44];
        let put16 = |c: &mut [u8], o: usize, v: u16| c[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let put32 = |c: &mut [u8], o: usize, v: u32| c[o..o + 4].copy_from_slice(&v.to_le_bytes());
        cmd[0] = 0; // add_modify = 0 (ADD)
        put16(&mut cmd, 2, 0xFFFF); // tid_disable_tx = in->tid_disable_ampdu (0xffff)
        put32(&mut cmd, 4, 0); // mac_id_n_color = ID_AND_COLOR(mac id 0, color 0)
        cmd[8..14].copy_from_slice(bssid); // addr = the AP (in->in_macaddr)
        cmd[16] = 0; // sta_id = IWM_STATION_ID
        // [20] station_flags = 0 (non-HT pre-assoc).
        // [24] station_flags_msk = FAT_EN_MSK (3<<26) | MIMO_EN_MSK (3<<28).
        put32(&mut cmd, 24, (3 << 26) | (3 << 28));
        put32(&mut cmd, 40, 0x0F); // tfd_queue_msk = AC queues 0-3 (legacy, non-DQA)
        println!("[iwlwifi] ADD_STA (real) sta_id=0 addr={:02X}:..:{:02X} (44B)", bssid[0], bssid[5]);
        let responded = self.send_cmd(0x18, &cmd);
        // ADD_STA returns a status (iwm_send_cmd_pdu_status): the first response
        // payload dword (r[2]); low byte == IWM_ADD_STA_SUCCESS(0x1) on success.
        let status = unsafe { LAST_RESP[2] };
        let ok = responded && (status & 0xFF) == 0x1;
        println!("[iwlwifi] ADD_STA status=0x{:08X} -> {}", status,
            if ok { "SUCCESS" } else if responded { "NOT success" } else { "NO RESPONSE" });
        ok
    }

    /// TIME_EVENT_CMD (0x29) — reserve a protected on-channel air-time window so
    /// the firmware won't wander off-channel during auth/assoc (iwm_protect_
    /// session). 36-byte iwm_time_event_cmd. Durations are in TU; we assume the
    /// standard 100 TU beacon interval (duration = 2×bi, max_delay = bi/2).
    /// Returns the firmware-assigned unique_id on success (response status 0).
    pub fn time_event(&mut self) -> bool {
        let bi = 100u32; // assumed AP beacon interval (TU); we don't parse it yet
        let mut cmd = [0u8; 36];
        let put16 = |c: &mut [u8], o: usize, v: u16| c[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let put32 = |c: &mut [u8], o: usize, v: u32| c[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // id_and_color @0 = 0 (mac id 0, color 0)
        put32(&mut cmd, 4, 1); // action = ADD
        put32(&mut cmd, 8, 0); // id = IWM_TE_BSS_STA_AGGRESSIVE_ASSOC
        // apply_time @12 = 0 (immediate)
        put32(&mut cmd, 16, bi / 2); // max_delay = bi/2
        // depends_on @20 = 0
        put32(&mut cmd, 24, 1); // interval = 1 (iwm sets this even though one-shot)
        // duration: iwm uses 2*bi (~205ms). We widen to ~1 s so the auth/assoc
        // frames + their responses all land inside the on-channel window even
        // with our slower host-command timing.
        let duration = 1024u32;
        put32(&mut cmd, 28, duration);
        cmd[32] = 1; // repeat
        cmd[33] = 0; // max_frags = IWM_TE_V2_FRAG_NONE
        // policy @34 = HOST_EVENT_START | HOST_EVENT_END | START_IMMEDIATELY
        put16(&mut cmd, 34, (1 << 0) | (1 << 1) | (1 << 11));
        println!("[iwlwifi] TIME_EVENT_CMD action=ADD id=AGGRESSIVE_ASSOC dur={} TU (36B)", duration);
        let responded = self.send_cmd(0x29, &cmd);
        // TIME_EVENT (0x29) returns TWO 0x29 packets: the immediate
        // iwm_time_event_resp {status, id, unique_id, id_and_color} AND — because
        // we set START_IMMEDIATELY|NOTIF_HOST_EVENT_START — an async
        // iwm_time_event_notif {timestamp, session_id, unique_id, id_and_color,
        // action, status}. send_cmd captures whichever closed last, so LAST_RESP
        // may be EITHER. Read both interpretations: resp.status@r[2], and if that
        // looks like a timestamp (notif), notif.status@r[7]. The notification
        // firing AT ALL means the event was scheduled and started, so we treat a
        // response as success — the air-time reservation is best-effort anyway.
        let (resp_status, resp_uid) = unsafe { (LAST_RESP[2], LAST_RESP[4]) };
        let notif_status = unsafe { LAST_RESP[7] };
        let ok = responded; // got a 0x29 packet back ⇒ event accepted
        println!("[iwlwifi] TIME_EVENT resp.status=0x{:08X} uid=0x{:08X} notif.status=0x{:08X} -> {}",
            resp_status, resp_uid, notif_status,
            if ok { "scheduled (best-effort)" } else { "NO RESPONSE" });
        ok
    }

    /// Enable the data/management TX queue (queue 1, TX FIFO BE) for the station.
    /// Configure the SCD HARDWARE registers directly — the exact sequence
    /// `tx_init` uses for the (working) command queue 0, just with queue 1 and TX
    /// FIFO BE. SCD_QUEUE_CFG (0x1d) does NOT configure the SCD context on this
    /// firmware, so the scheduler never serviced the queue (rdptr stuck at 0).
    /// The station↔queue binding already comes from ADD_STA's tfd_queue_msk
    /// (0x0F includes queue 1), so no host command is needed here.
    pub fn enable_data_queue(&mut self) -> bool {
        use super::iwlwifi_csr::{scd, fh_cbbc_queue, CSR_HBUS_TARG_WRPTR};
        const DATA_QUEUE: u32 = 1;
        const TX_FIFO_BE: u32 = 1;
        let tfd_phys = match phys_of(unsafe { &raw const TX1_TFD_RING } as u64) { Some(p)=>p, None=>return false };
        if !self.grab_nic_access() { return false; }
        let scd_base = self.scd_base_ptr;
        // Disable the scheduler while we configure the new queue (mirrors tx_init).
        self.csr.write_prph(scd::TXFACT, 0);
        // Same per-queue setup tx_init does for queue 0 (proven to schedule).
        self.csr.write32(fh_cbbc_queue(DATA_QUEUE as u64), (tfd_phys >> 8) as u32);
        self.csr.clear_bits_prph(scd::AGGR_SEL, 1 << DATA_QUEUE);
        self.csr.write_prph(scd::CHAINEXT_EN, 0);
        self.csr.write_prph(scd::queue_rdptr(DATA_QUEUE), 0);
        self.csr.write32(CSR_HBUS_TARG_WRPTR, DATA_QUEUE << 8); // HW write ptr -> idx 0
        self.csr.mem_write32(scd_base + scd::context_queue_offset(DATA_QUEUE), 0);
        let frame_limit: u32 = 64;
        self.csr.mem_write32(scd_base + scd::context_queue_offset(DATA_QUEUE) + 4,
            ((frame_limit << scd::CTX_WIN_SIZE_POS) & 0x7F)
            | ((frame_limit << scd::CTX_FRAME_LIMIT_POS) & 0x7F_0000));
        self.csr.write_prph(scd::queue_status(DATA_QUEUE), scd::queue_enable_val(TX_FIFO_BE));
        // Re-enable all FIFOs now that queue 1 is configured.
        self.csr.write_prph(scd::TXFACT, 0xFF);
        let q_stts = self.csr.read_prph(scd::queue_status(DATA_QUEUE));
        let txfact = self.csr.read_prph(scd::TXFACT);
        self.release_nic_access();
        unsafe { TX1_WRITE_IDX = 0; }
        println!("[iwlwifi] data TX queue {} enabled (FIFO BE, SCD regs) q_stts=0x{:08X} TXFACT=0x{:08X}",
            DATA_QUEUE, q_stts, txfact);
        true
    }

    /// Transmit one 802.11 frame to the AP on the data queue. Wraps the frame in
    /// an iwm_tx_cmd (0x1c): builds [cmd_header(4) | tx_cmd(60) | 802.11 hdr |
    /// pad | body] in TX_FRAME_BUF and a 3-TB TFD (TB0=16B, TB1=rest of cmd+hdr,
    /// TB2=body), updates the SCD byte-count, and rings the queue-1 doorbell.
    /// `frame` = full 802.11 frame (header + body); `hdrlen` = 802.11 header len.
    pub fn tx_mgmt(&mut self, frame: &[u8], hdrlen: usize) -> bool {
        use super::iwlwifi_csr::{CSR_HBUS_TARG_WRPTR, scd};
        const DATA_QUEUE: u16 = 1;
        const TX_CMD_HDR: usize = 4;   // sizeof(iwm_cmd_header)
        const TX_CMD_SIZE: usize = 56; // sizeof(iwm_tx_cmd) v6 (packed)
        const TB0_SIZE: usize = TX_CMD_HDR + TX_CMD_SIZE; // cmd header + full tx_cmd
        let totlen = frame.len();
        let bodylen = totlen - hdrlen;
        let pad = (4 - (hdrlen & 3)) & 3; // 802.11 header must be 4-byte aligned
        let idx = unsafe { TX1_WRITE_IDX } as usize;
        let frame_phys = match phys_of(unsafe { &raw const TX_FRAME_BUF } as u64) { Some(p)=>p, None=>return false };

        // ---- Build the staging buffer ----
        unsafe {
            let b = &mut TX_FRAME_BUF.0;
            for x in b.iter_mut() { *x = 0; }
            // iwm_cmd_header: code=TX_CMD(0x1c), flags=0, idx, qid=1
            b[0] = 0x1c; b[1] = 0; b[2] = idx as u8; b[3] = DATA_QUEUE as u8;
            let tx = TX_CMD_HDR; // tx_cmd starts here
            let put16 = |b: &mut [u8], o: usize, v: u16| b[o..o+2].copy_from_slice(&v.to_le_bytes());
            let put32 = |b: &mut [u8], o: usize, v: u32| b[o..o+4].copy_from_slice(&v.to_le_bytes());
            put16(b, tx + 0, totlen as u16); // len = full frame length
            // tx_flags @tx+4: ACK | BT_DIS | SEQ_CTL (fw owns the seqno)
            put32(b, tx + 4, (1 << 3) | (1 << 12) | (1 << 13));
            // rate_n_flags @tx+12: 1 Mbps CCK on antenna A (mgmt frames go slow+robust)
            put32(b, tx + 12, 10 | (1 << 9) | (1 << 14));
            b[tx + 16] = 0; // sta_id = IWM_STATION_ID (the AP)
            // sec_ctl @tx+17 = 0 (no encryption on auth/assoc)
            put32(b, tx + 40, 0xFFFF_FFFF); // life_time = INFINITE @ tx+40
            // scratch dram ptr @tx+44/48: point at the tx_cmd.scratch area (tx+8)
            let scratch_phys = frame_phys + (TX_CMD_HDR + 8) as u64;
            put32(b, tx + 44, (scratch_phys & 0xFFFF_FFFF) as u32);
            b[tx + 48] = ((scratch_phys >> 32) & 0xFF) as u8;
            b[tx + 49] = 3; // rts_retry_limit
            b[tx + 50] = 3; // data_retry_limit (IWM_MGMT_DFAULT_RETRY_LIMIT)
            // Copy the 802.11 header right after tx_cmd, then the body after pad.
            let hdr_off = TX_CMD_HDR + TX_CMD_SIZE;
            b[hdr_off..hdr_off + hdrlen].copy_from_slice(&frame[..hdrlen]);
            let body_off = hdr_off + hdrlen + pad;
            b[body_off..body_off + bodylen].copy_from_slice(&frame[hdrlen..]);
        }

        // ---- Build the 3-TB gen1 TFD ----
        let tb1_len = (hdrlen + pad) as u16;
        let body_off = (TB0_SIZE + hdrlen + pad) as u64;
        unsafe {
            let tfd = &mut TX1_TFD_RING.0[idx];
            for x in tfd.iter_mut() { *x = 0; }
            tfd[3] = 3; // num_tbs
            let hi = ((frame_phys >> 32) & 0xF) as u16;
            let set_tb = |tfd: &mut [u8; 128], i: usize, addr: u64, len: u16| {
                let o = 4 + i * 6;
                tfd[o..o+4].copy_from_slice(&((addr & 0xFFFF_FFFF) as u32).to_le_bytes());
                tfd[o+4..o+6].copy_from_slice(&((((addr >> 32) & 0xF) as u16) | (len << 4)).to_le_bytes());
            };
            set_tb(tfd, 0, frame_phys, TB0_SIZE as u16);
            set_tb(tfd, 1, frame_phys + TB0_SIZE as u64, tb1_len);
            set_tb(tfd, 2, frame_phys + body_off, bodylen as u16);
            // SCD byte-count for queue 1: val = sta_id<<12 | (totlen + CRC + delim).
            let _ = hi;
            let bc = ((totlen + 8) & 0xFFF) as u16;
            TX_BC_TBL.0[320 + idx] = bc;
            if idx < 64 { TX_BC_TBL.0[320 + 256 + idx] = bc; }
        }

        // ---- Advance + ring the queue-1 doorbell ----
        let new_idx = ((idx + 1) % TX_RING_SIZE) as u16;
        unsafe { TX1_WRITE_IDX = new_idx; }
        let stts_before = unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) };
        if !self.grab_nic_access() { return false; }
        let rd_before = self.csr.read_prph(scd::queue_rdptr(DATA_QUEUE as u32));
        self.csr.write32(CSR_HBUS_TARG_WRPTR, ((DATA_QUEUE as u32) << 8) | new_idx as u32);
        self.release_nic_access();
        // Wait briefly for the SCD to consume the TFD (read-ptr advance) and/or
        // the firmware to push a TX-status notification (rb_stts advance).
        let mut responded = false;
        for _ in 0..150 {
            if unsafe { core::ptr::read_volatile(&raw const RB_STTS.0[0]) } != stts_before { responded = true; break; }
            for _ in 0..2000 { for _ in 0..100 { core::hint::spin_loop(); } } // ~2 ms
        }
        if !self.grab_nic_access() { return false; }
        let rd_after = self.csr.read_prph(scd::queue_rdptr(DATA_QUEUE as u32));
        let mut e = [0u32; 32];
        self.csr.mem_read_block(self.error_table_ptr, &mut e);
        let q_stts = self.csr.read_prph(scd::queue_status(DATA_QUEUE as u32));
        self.release_nic_access();
        super::wifidbg!("[iwlwifi] TX frame on q{}: {} bytes (hdr {} body {}), TBs {}/{}/{}",
            DATA_QUEUE, totlen, hdrlen, bodylen, TB0_SIZE, tb1_len, bodylen);
        println!("[iwlwifi]   TX diag: SCD_rdptr {}->{} (consumed={}) q_stts=0x{:08X} responded={} TBs {}/{}/{}",
            rd_before, rd_after, (rd_after != rd_before) as u8, q_stts, responded as u8,
            TB0_SIZE, tb1_len, bodylen);
        if e[0] != 0 {
            // iwm_error_event_table: [1]error_id [7]data1 [20]log_pc [23]hcmd
            // [29]last_cmd_id [16]major [17]minor. hcmd/last_cmd_id tell us WHICH
            // command the firmware was running when it asserted.
            println!("[iwlwifi]   TX FAULT: err_id=0x{:08X} log_pc=0x{:08X} data1=0x{:08X} data2=0x{:08X}",
                e[1], e[20], e[7], e[8]);
            println!("[iwlwifi]   TX FAULT: hcmd=0x{:08X} last_cmd_id=0x{:08X} fw {}.{} isr0=0x{:08X}",
                e[23], e[29], e[16], e[17], e[24]);
        } else {
            super::wifidbg!("[iwlwifi]   TX diag: error table clean (no fault)");
        }
        true
    }

    /// Build + transmit an open-system authentication request to the AP. This is
    /// the FIRST on-air frame — everything before was host→firmware commands.
    /// 802.11 mgmt auth: 24-byte header + 6-byte body (algo=0 open, seq=1,
    /// status=0). Then drains the RX ring for the AP's auth response (seq 2).
    pub fn send_auth(&mut self, bssid: &[u8; 6]) -> bool {
        let mac = self.sm.sta_mac;
        let mut frame = [0u8; 30];
        // ---- 802.11 management header (24 bytes) ----
        frame[0] = 0xB0; // fc0: type=mgmt(0), subtype=auth(0xB)
        frame[1] = 0x00; // fc1
        // duration @2..4 = 0
        frame[4..10].copy_from_slice(bssid);   // addr1 = RA = BSSID
        frame[10..16].copy_from_slice(&mac);   // addr2 = TA = our MAC
        frame[16..22].copy_from_slice(bssid);  // addr3 = BSSID
        // seq_ctl @22..24 = 0 (firmware fills it, FLG_SEQ_CTL set)
        // ---- Authentication body (6 bytes) ----
        frame[24] = 0; frame[25] = 0; // auth algorithm = 0 (Open System)
        frame[26] = 1; frame[27] = 0; // auth transaction seq = 1
        frame[28] = 0; frame[29] = 0; // status code = 0
        println!("[wifi] TX auth (open) -> {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5]);
        // Fault check BEFORE the frame TX, to localize the assert: if a fault is
        // already present here, the SCD_QUEUE_CFG (0x1d) caused it; if the table
        // is clean here but faults after tx_mgmt, the TX frame caused it.
        if self.grab_nic_access() {
            let pre_valid = self.csr.mem_read32(self.error_table_ptr);
            let pre_id = self.csr.mem_read32(self.error_table_ptr + 4);
            self.release_nic_access();
            println!("[iwlwifi]   pre-TX fault check: err_table_ptr=0x{:08X} valid=0x{:08X} id=0x{:08X} ({})",
                self.error_table_ptr, pre_valid, pre_id,
                if pre_valid != 0 { "0x1d FAULTED" } else { "clean — TX is next" });
        }
        let from = self.rx_count();
        if !self.tx_mgmt(&frame, 24) { return false; }
        // The AP should answer with an auth response (seq 2). Watch the RX ring.
        self.drain_rx(from, 2000);
        true
    }

    /// Build + transmit a WPA2 Association Request (after open-auth succeeds).
    /// Carries the RSN IE so the AP starts the 4-way handshake; drains for the
    /// Association Response (subtype 0x10 → status 0 + AID). Mirrors send_auth.
    pub fn send_assoc(&mut self, bssid: &[u8; 6], ssid: &[u8]) -> bool {
        let mac = self.sm.sta_mac;
        let mut frame = [0u8; 128];
        let n = match super::build_association_request_wpa2(&mut frame, &mac, bssid, ssid) {
            Some(n) => n,
            None => { println!("[wifi] assoc: frame build failed"); return false; }
        };
        println!("[wifi] TX assoc-req (+RSN IE) -> {:02X}:..:{:02X} ({} bytes)", bssid[0], bssid[5], n);
        let from = self.rx_count();
        if !self.tx_mgmt(&frame[..n], 24) { return false; }
        self.drain_rx(from, 2000);
        true
    }

    /// Transmit an EAPOL-Key frame (Msg2 / Msg4) to the AP as a data frame
    /// wrapped in LLC/SNAP (ethertype 0x888E). Uses the data TX queue.
    pub fn send_eapol(&mut self, bssid: &[u8; 6], eapol: &[u8]) -> bool {
        const LLC_SNAP_EAPOL: [u8; 8] = [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x88, 0x8E];
        let mac = self.sm.sta_mac;
        let mut frame = [0u8; 512];
        // 802.11 data header: STA -> AP (ToDS=1, FromDS=0).
        let fc: u16 = (1u16 << 8) | (2u16 << 2); // type=Data, ToDS=1
        frame[0..2].copy_from_slice(&fc.to_le_bytes());
        // duration @2..4 = 0
        frame[4..10].copy_from_slice(bssid);   // addr1 = RA = BSSID
        frame[10..16].copy_from_slice(&mac);   // addr2 = TA = our MAC
        frame[16..22].copy_from_slice(bssid);  // addr3 = BSSID
        // seq_ctl @22..24 = 0 (firmware fills it)
        frame[24..32].copy_from_slice(&LLC_SNAP_EAPOL);
        let body_start = 32;
        if body_start + eapol.len() > frame.len() {
            println!("[wifi] EAPOL TX: frame too big ({} bytes)", eapol.len());
            return false;
        }
        frame[body_start..body_start + eapol.len()].copy_from_slice(eapol);
        let total = body_start + eapol.len();
        println!("[wifi] TX EAPOL-Key {} bytes -> AP", eapol.len());
        self.tx_mgmt(&frame[..total], 24)
    }

    /// Drive one step of the WPA2 4-way handshake from an inbound EAPOL-Key frame
    /// (the `key_data` of a received data frame, ethertype 0x888E). Writes the
    /// response (Msg2 or Msg4) into `out` and returns its length.
    ///   Msg1 (no MIC): AP's ANonce → generate SNonce, derive PTK (store KCK/KEK/
    ///     TK in CONN), build Msg2 (SNonce + RSN IE + MIC).
    ///   Msg3 (has MIC): verify the AP's MIC with our KCK, build Msg4 (MIC). The
    ///     TK is now ready to install to the firmware; the GTK (AES-key-wrapped
    ///     in Msg3's key_data with the KEK) is a follow-on (needs AES key-unwrap).
    pub fn handshake_step(&mut self, eapol: &[u8], out: &mut [u8]) -> Option<usize> {
        let key = super::parse_eapol_key(eapol)?;
        let mac = self.sm.sta_mac;
        let conn = unsafe { &mut *core::ptr::addr_of_mut!(CONN) };
        if !key.has_mic() {
            // ---- Msg1: derive the PTK from PMK + ANonce + our fresh SNonce ----
            let mut snonce = [0u8; 32];
            if crate::rng::fill_bytes(&mut snonce).is_err() {
                println!("[wifi] 4-way: RNG failed for SNonce");
                return None;
            }
            let ptk = super::wpa2::ptk(&conn.pmk, &conn.bssid, &mac, &key.nonce, &snonce);
            conn.snonce = snonce;
            conn.kck.copy_from_slice(&ptk[0..16]);
            conn.kek.copy_from_slice(&ptk[16..32]);
            conn.tk.copy_from_slice(&ptk[32..48]);
            conn.ptk_valid = true;
            let n = super::build_eapol_msg2(out, &snonce, key.replay, &super::RSN_IE_WPA2_PSK_CCMP)?;
            super::finalize_eapol_mic(&mut out[..n], &conn.kck);
            println!("[wifi] 4-way: Msg1 rx (ANonce) -> derived PTK, TX Msg2 ({} B)", n);
            Some(n)
        } else {
            // ---- Msg3: verify the AP's MIC, then ACK with Msg4 ----
            if !conn.ptk_valid {
                println!("[wifi] 4-way: Msg3 before Msg2 — ignoring");
                return None;
            }
            // Verify MIC: recompute over the received frame with its MIC field
            // zeroed, compare to what the AP sent (key.mic).
            let mut tmp = [0u8; 256];
            let fl = eapol.len().min(tmp.len());
            tmp[..fl].copy_from_slice(&eapol[..fl]);
            for b in &mut tmp[81..97.min(fl)] { *b = 0; }
            let want = super::wpa2::eapol_mic(&conn.kck, &tmp[..fl]);
            if want != key.mic {
                println!("[wifi] 4-way: Msg3 MIC MISMATCH — wrong passphrase or attack; abort");
                return None;
            }
            let n = super::build_eapol_msg4(out, key.replay)?;
            super::finalize_eapol_mic(&mut out[..n], &conn.kck);
            println!("[wifi] 4-way: Msg3 MIC ok -> TX Msg4; TK ready to install");
            Some(n)
        }
    }

    /// Send a PASSIVE LMAC scan of the 2.4 GHz band, then drain the RX ring to
    /// log the beacons + the scan-complete notification. Passive needs no
    /// aux-station / PHY-context setup. Requires the RUNTIME ucode ALIVE + the
    /// command queue armed.
    pub fn scan_passive(&mut self) -> bool {
        if self.state != DeviceState::Alive && !self.is_ready() {
            println!("[iwlwifi] scan: device not ALIVE");
            return false;
        }
        let mut buf = [0u8; super::iwlwifi_scan::SCAN_CMD_LEN];
        let sta = self.sm.sta_mac;
        let n = super::iwlwifi_scan::build_passive_scan(&mut buf, &sta, 11);
        println!("[iwlwifi] SCAN: passive LMAC scan, 11 channels, {}-byte payload", n);
        // Mark RX count before the scan so the drain can't miss fast results.
        let scan_from = self.rx_count();
        let ok = self.send_cmd(super::iwlwifi_scan::SCAN_OFFLOAD_REQUEST_CMD, &buf[..n]);
        // Beacons + SCAN_OFFLOAD_COMPLETE arrive async. 3 iterations × 11 ch ×
        // ~150 ms ≈ 5-6 s; drain well past that.
        self.drain_rx(scan_from, 9000);
        ok
    }

    /// Run a scan and print the de-duplicated numbered network list. Backs the
    /// `wifi` shell command. Returns the number of unique networks found.
    pub fn scan_and_list(&mut self) -> usize {
        net_reset();
        self.scan_passive();
        net_sort();
        net_print();
        unsafe { NET_COUNT }
    }

    /// Connect to scan-list network `idx` with `password` (the `wifi connect`
    /// command). FOUNDATION (this build): pick the network, derive the WPA2 PMK
    /// from the password, and retune the radio to the AP's channel. The full
    /// association — MAC context (0x28), binding (0x2b), real ADD_STA, time
    /// event (0x29), auth/assoc frame TX, and the WPA2 4-way handshake using
    /// the stored PMK — is built on top of this in the following steps.
    pub fn connect(&mut self, idx: usize, password: &[u8]) -> bool {
        let net = match net_get(idx) {
            Some(n) => n,
            None => {
                println!("[wifi] connect: no network #{} (run `wifi` to scan first)", idx);
                return false;
            }
        };
        if net.ssid_len == 0 {
            println!("[wifi] connect: hidden SSID (#{}) not supported yet", idx);
            return false;
        }
        let ssid = &net.ssid[..net.ssid_len as usize];
        let ssid_str = core::str::from_utf8(ssid).unwrap_or("<non-utf8>");
        println!("[wifi] connect: \"{}\"  BSSID {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}  ch{}",
            ssid_str, net.bssid[0], net.bssid[1], net.bssid[2], net.bssid[3], net.bssid[4],
            net.bssid[5], net.channel);

        // Derive the WPA2-PSK PMK from the typed password (PBKDF2-HMAC-SHA1,
        // 4096 iterations). This is the root key for the 4-way handshake.
        println!("[wifi] deriving PMK (PBKDF2-SHA1, 4096 iters)...");
        let pmk = super::wpa2::pmk(password, ssid);
        println!("[wifi] PMK = {:02X}{:02X}{:02X}{:02X}...{:02X}{:02X} (derived)",
            pmk[0], pmk[1], pmk[2], pmk[3], pmk[30], pmk[31]);
        unsafe {
            let c = &mut *core::ptr::addr_of_mut!(CONN);
            c.pmk = pmk;
            c.bssid = net.bssid;
            c.channel = net.channel;
            c.ssid_len = net.ssid_len;
            c.ssid = net.ssid;
        }

        // Step 1: retune PHY context 0 from the scan default to the AP channel.
        println!("[wifi] retuning radio to ch{}...", net.channel);
        if !self.phy_context(2 /* MODIFY */, net.channel) {
            println!("[wifi] connect: PHY-context retune FAILED");
            return false;
        }
        // Phase A step 1: add the station MAC context for the AP.
        println!("[wifi] adding MAC context...");
        if !self.mac_context(1 /* ADD */, &net.bssid) {
            println!("[wifi] connect: MAC_CONTEXT FAILED");
            return false;
        }
        // Phase A step 2: bind the MAC to PHY context 0.
        println!("[wifi] binding MAC to radio...");
        if !self.binding_context(1 /* ADD */) {
            println!("[wifi] connect: BINDING FAILED");
            return false;
        }
        // Phase A step 3: register the AP as the real station (sta_id 0).
        println!("[wifi] adding AP as station...");
        if !self.add_station(&net.bssid) {
            println!("[wifi] connect: ADD_STA FAILED");
            return false;
        }
        // Enable the data/mgmt TX queue NOW (before the time event), because
        // SCD_QUEUE_CFG waits ~hundreds of ms for its response. If we scheduled
        // the protected window first, it would expire during that wait and the
        // auth frame would TX on an OFF-CHANNEL MAC — which trips the firmware
        // assert (err_id 0x90A, hcmd=0x..001C = our TX_CMD). Enable first, then
        // open the window, then TX immediately inside it.
        println!("[wifi] --- Phase B: enabling TX queue ---");
        if !self.enable_data_queue() {
            println!("[wifi] connect: data TX queue enable FAILED");
            return false;
        }
        // Phase A step 4: reserve a protected on-channel window, then TX the auth
        // frame RIGHT AWAY so it goes out while the radio is still on ch{}.
        println!("[wifi] reserving air-time (time event)...");
        if !self.time_event() {
            println!("[wifi] connect: TIME_EVENT got no response — continuing without protection");
        }
        println!("[wifi] *** Phase A complete: MAC+binding+station+queue+air-time up on ch{}. ***", net.channel);

        clear_connect_events();

        // Phase B: open-system auth, then association, then WPA2 4-way handshake.
        if !self.send_auth(&net.bssid) {
            println!("[wifi] connect: auth frame TX failed");
            return false;
        }
        if !was_auth_success() {
            println!("[wifi] connect: authentication failed or no response");
            return false;
        }
        println!("[wifi] auth accepted — sending association request");
        if !self.send_assoc(&net.bssid, ssid) {
            println!("[wifi] connect: assoc frame TX failed");
            return false;
        }
        if !was_assoc_success() {
            println!("[wifi] connect: association failed or no response");
            return false;
        }
        println!("[wifi] assoc accepted — starting WPA2 4-way handshake");
        self.sm.state = super::iwlwifi_sm::State::FourWayHandshake;

        let mut eapol_buf = [0u8; 512];
        let mut resp_buf = [0u8; 256];
        let mut attempts = 0usize;
        const MAX_ATTEMPTS: usize = 40; // 40 * 250 ms = 10 s
        while attempts < MAX_ATTEMPTS {
            if let Some(eapol_len) = take_eapol(&mut eapol_buf) {
                if let Some(resp_len) = self.handshake_step(&eapol_buf[..eapol_len], &mut resp_buf) {
                    if !self.send_eapol(&net.bssid, &resp_buf[..resp_len]) {
                        println!("[wifi] connect: EAPOL response TX failed");
                        return false;
                    }
                    // If the EAPOL we just answered had the MIC bit set, it was
                    // Msg3; our response is Msg4 and the handshake is complete.
                    let key_info = u16::from_be_bytes([eapol_buf[5], eapol_buf[6]]);
                    if key_info & super::KeyInfo::MIC.bits() != 0 {
                        println!("[wifi] 4-way handshake complete — associated to \"{}\"", ssid_str);
                        self.state = DeviceState::Associated;
                        self.sm.state = super::iwlwifi_sm::State::Associated;
                        // TODO(M11): install TK/GTK in firmware and start data path.
                        return true;
                    }
                    // Otherwise we just sent Msg2; keep waiting for Msg3.
                }
                // handshake_step already printed on MIC failure; continuing here
                // would loop forever on a wrong passphrase. Abort instead.
                else {
                    println!("[wifi] connect: 4-way handshake aborted");
                    return false;
                }
            }
            let from = self.rx_count();
            self.drain_rx(from, 250);
            attempts += 1;
        }
        println!("[wifi] connect: 4-way handshake timed out");
        false
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
    // WPA2 crypto KATs — run regardless of hardware (offline-verifiable):
    // SHA1/PMK/PTK/EAPOL-MIC primitives + the full Msg2 frame + MIC.
    super::wpa2::self_test();
    super::eapol_self_test();
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
                super::wifidbg!("[iwlwifi] Stage 1 (power-up) complete — ready for firmware load");
                // Stage 2a: parse the embedded ucode.
                if let Some(fw) = super::iwlwifi_fw_image::parse() {
                    // Stage 2b: DMA the INIT image sections into device SRAM
                    // via the FH service channel. (CPU stays in reset — the
                    // ucode does not run yet; ALIVE handshake is the next
                    // step and needs the RX ring.)
                    super::wifidbg!("[iwlwifi] Stage 2b: loading INIT image into NIC SRAM...");
                    if dev.load_image(&fw.init) {
                        super::wifidbg!("[iwlwifi] Stage 2b: INIT image DMA complete — all sections in SRAM");
                        // Stage 2c: RX ring → release CPU → watch for ALIVE.
                        super::wifidbg!("[iwlwifi] Stage 2c: RX ring + release CPU, waiting for ALIVE...");
                        if dev.rx_init() {
                            dev.release_cpu();
                            if dev.wait_alive() {
                                super::wifidbg!("[iwlwifi] Stage 2c: firmware ALIVE");
                                // Stage 3a: stand up the TX command queue +
                                // scheduler, verify by register readback.
                                super::wifidbg!("[iwlwifi] Stage 3a: configuring TX command queue...");
                                if dev.tx_init() {
                                    // Stage 3b: send the first host command as a
                                    // queue-plumbing probe (PHY_CONFIGURATION_CMD
                                    // 0x6a, zeroed payload). We watch consumption
                                    // + response + the fault table.
                                    super::wifidbg!("[iwlwifi] Stage 3b: sending first command...");
                                    // TX_ANT_CONFIGURATION_CMD (0x98): the real
                                    // first INIT-ucode command. Payload = a u32
                                    // antenna mask (0x3 = both antennas on 7260).
                                    let payload = [0x03u8, 0, 0, 0];
                                    if dev.send_cmd(0x98, &payload) {
                                        // NVM_ACCESS_CMD (0x88): read the NVM SW
                                        // section so we can pull the WiFi MAC.
                                        // [op=read, target=cache, type=1(SW),
                                        //  offset=0, length=0x100].
                                        super::wifidbg!("[iwlwifi] Stage 3c: reading NVM HW section (MAC)...");
                                        // [op=read, target=cache, type=0(HW),
                                        //  offset=0, length=0x40]. MAC is ~byte 42.
                                        let nvm = [0u8, 0, 0, 0, 0, 0, 0x40, 0];
                                        // send_cmd extracts the station MAC from
                                        // the response (byte 42, pair-swapped)
                                        // and stores it into sm.sta_mac.
                                        dev.send_cmd(0x88, &nvm);
                                        // Stage 3e: PHY_CONFIGURATION_CMD (0x6a)
                                        // — kick off the INIT-ucode calibrations
                                        // with the REAL payload (phy_config +
                                        // calib triggers from the fw TLVs). The
                                        // zeroed-payload version faulted earlier
                                        // (err_id=0x34), proving the fw validates
                                        // this. iwl_mvm_get_phy_config re-masks
                                        // the TX/RX chain bits with the valid
                                        // antennas (here == fw.phy_config's own
                                        // chains, so an identity on the 7260).
                                        super::wifidbg!("[iwlwifi] Stage 3e: PHY_CFG (calibration trigger)...");
                                        let mut phy = [0u8; 12];
                                        phy[0..4].copy_from_slice(&fw.phy_config.to_le_bytes());
                                        phy[4..8].copy_from_slice(&fw.calib_flow.to_le_bytes());
                                        phy[8..12].copy_from_slice(&fw.calib_event.to_le_bytes());
                                        // Mark the RX count BEFORE PHY_CFG so the
                                        // calib capture scans from here (the echo
                                        // + calib notifications may arrive faster
                                        // than send_cmd returns).
                                        let calib_from = dev.rx_count();
                                        dev.send_cmd(0x6a, &phy);
                                        // The INIT ucode now runs RF calibration
                                        // and streams the results as CALIB_RES
                                        // (0x6B) notifications. Capture them so we
                                        // can replay them to the runtime ucode —
                                        // the radio config (PHY_CONTEXT) asserts
                                        // without calibration. Stops on
                                        // INIT_COMPLETE or ~1.5 s.
                                        super::wifidbg!("[iwlwifi] Stage 3f: capturing calibration results...");
                                        dev.capture_calib(calib_from, 1500);
                                        // Stage 4: switch to the RUNTIME ucode —
                                        // the operational firmware (2nd ALIVE).
                                        super::wifidbg!("[iwlwifi] Stage 4: loading RUNTIME firmware → 2nd ALIVE...");
                                        if dev.load_and_alive(&fw.runtime, "RUNTIME") {
                                            super::wifidbg!("[iwlwifi] Stage 4: RUNTIME ALIVE — operational ucode running");
                                            // Re-arm the TX command queue for the
                                            // runtime ucode (new scd_base), so we
                                            // can send SCAN next.
                                            if dev.tx_init() {
                                                // The runtime ucode came up
                                                // "naked" — give it the same
                                                // antenna + PHY config the init
                                                // ucode got, or the radio won't
                                                // tune and the scan finds nothing.
                                                super::wifidbg!("[iwlwifi] Stage 4b: configuring runtime ucode (ant + calib + PHY)...");
                                                let ant = [0x03u8, 0, 0, 0];
                                                dev.send_cmd(0x98, &ant);
                                                // Forward the captured INIT-ucode
                                                // calibration (PHY_DB) BEFORE the
                                                // PHY config — iwm_init_hw order.
                                                dev.send_phy_db();
                                                let mut phy = [0u8; 12];
                                                phy[0..4].copy_from_slice(&fw.phy_config.to_le_bytes());
                                                phy[4..8].copy_from_slice(&fw.calib_flow.to_le_bytes());
                                                phy[8..12].copy_from_slice(&fw.calib_event.to_le_bytes());
                                                dev.send_cmd(0x6a, &phy);
                                                // BT coex config — the 7260 is a
                                                // combo card; the radio won't
                                                // scan without it (accepted but
                                                // no-op otherwise).
                                                dev.send_bt_init();
                                                // Keep the radio awake (no power
                                                // save) so the scan can actually
                                                // run.
                                                dev.send_power_awake();
                                                // Stage 4c: the firmware won't
                                                // scan until the aux station
                                                // (sta_id=1) is in its table.
                                                super::wifidbg!("[iwlwifi] Stage 4c: adding aux station for scan...");
                                                dev.add_aux_station();
                                                // Stage 4d: add a PHY context so
                                                // the radio actually tunes a
                                                // channel during the scan.
                                                super::wifidbg!("[iwlwifi] Stage 4d: adding PHY context...");
                                                dev.add_phy_context();
                                                // Device is fully set up (aux
                                                // station + PHY context). The
                                                // boot-time scan is gone — it
                                                // added ~9 s + a wall of beacon
                                                // logs every boot for no reason.
                                                // Type `wifi` at the shell to
                                                // scan on demand instead.
                                                println!("[iwlwifi] ready — type `wifi` to scan, `wifi connect <n> <pass>` to join");
                                            }
                                        } else {
                                            super::wifidbg!("[iwlwifi] Stage 4: RUNTIME firmware did NOT ALIVE");
                                        }
                                    }
                                }
                            } else {
                                super::wifidbg!("[iwlwifi] Stage 2c: no ALIVE signal — firmware silent after release");
                            }
                        }
                    } else {
                        super::wifidbg!("[iwlwifi] Stage 2b: INIT image load FAILED — see FH error above");
                    }
                }
            } else {
                super::wifidbg!("[iwlwifi] Stage 1 (power-up) FAILED — see CSR dump above");
            }
        }
    }
    true
}

/// Access the global device.  Returns `Some` after `init()` succeeds.
pub fn device() -> Option<&'static mut IwlDevice> {
    unsafe { DEVICE.as_mut() }
}
