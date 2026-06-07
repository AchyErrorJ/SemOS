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

    /// Stub: assert SW reset via CSR_RESET, then wait for MAC clocks.
    /// Real implementation will pulse the reset bit, poll GP_CNTRL for
    /// MAC_CLOCK_READY, then clear the reset bit.
    pub fn reset_device(&mut self) -> bool {
        println!("[iwlwifi] reset_device: STUB");
        // TODO(M11):
        //   1. write32(CSR_RESET, SW_RESET_BIT)
        //   2. poll32(CSR_GP_CNTRL, MAC_CLOCK_READY, true, 100_000)
        //   3. write32(CSR_RESET, 0)
        false
    }

    /// Stub: load firmware blob into NIC SRAM and kick the ucode.
    /// On real hardware this reads `iwlwifi-7260-17.ucode` (or AX211 equiv)
    /// from an embedded binary blob, writes it via the DMA engine, and
    /// sets the INIT bit in CSR_GP_CNTRL.
    pub fn load_firmware(&mut self) -> bool {
        println!("[iwlwifi] load_firmware: STUB — needs firmware blob + real CSR sequence");
        // TODO(M11): embed firmware blob, write to SRAM, kick ucode.
        false
    }

    /// Stub: wait for the ALIVE notification from the ucode.
    /// The ALIVE event is a notification sent by the running firmware
    /// to confirm it booted successfully.  Without it, no further commands
    /// can be issued.
    pub fn wait_alive(&mut self) -> bool {
        println!("[iwlwifi] wait_alive: STUB — needs event-ring polling on real hardware");
        // TODO(M11): poll RX queue / event ring for ALIVE notification.
        false
    }

    /// Stub: initialise PHY from EEPROM/NVM + PNVM + regulatory caps.
    pub fn init_phy(&mut self) -> bool {
        println!("[iwlwifi] init_phy: STUB — needs NVM parse + channel table");
        // TODO(M11): read EEPROM, apply regulatory, run TX/RX calibration.
        false
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
        if let Some(dev) = DEVICE.as_ref() {
            dev.read_hw_rev();
            let cfg = dev.csr.read32(super::iwlwifi_csr::CSR_HW_IF_CONFIG_REG);
            let gpc = dev.csr.read32(super::iwlwifi_csr::CSR_GP_CNTRL);
            println!("[iwlwifi] HW_IF_CONFIG=0x{:08X} GP_CNTRL=0x{:08X}", cfg, gpc);
            if cfg == 0xFFFFFFFF && gpc == 0xFFFFFFFF {
                println!("[iwlwifi] WARN: CSRs read all-ones — BAR mapping or PCI command may be wrong");
            }
        }
    }
    true
}

/// Access the global device.  Returns `Some` after `init()` succeeds.
pub fn device() -> Option<&'static mut IwlDevice> {
    unsafe { DEVICE.as_mut() }
}
