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

/// Opaque device handle.  Created by `probe()` on PCI match.
pub struct IwlDevice {
    pub pci: IwlPciInfo,
    pub state: DeviceState,
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
        println!("[iwlwifi] device created for {} @ {:02X}:{:02X}.{}  BAR0=0x{:08X}",
            pci.name, pci.loc.bus, pci.loc.slot, pci.loc.func, pci.bar0_phys);
        Self { pci, state: DeviceState::Probed }
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

    /// Stub: build and send a Scan command via the HCMD ring.
    pub fn cmd_scan(&mut self, _ssid: Option<&[u8]>) -> bool {
        println!("[iwlwifi] cmd_scan: STUB");
        // TODO(M11): build SCAN_REQ_CMD, post to command queue.
        false
    }

    /// Stub: build and send an Authentication command.
    pub fn cmd_auth(&mut self, _bssid: &[u8; 6]) -> bool {
        println!("[iwlwifi] cmd_auth: STUB");
        false
    }

    /// Stub: build and send an Association command.
    pub fn cmd_assoc(&mut self, _bssid: &[u8; 6], _ssid: &[u8]) -> bool {
        println!("[iwlwifi] cmd_assoc: STUB");
        false
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

    /// True if associated to an AP.
    pub fn is_associated(&self) -> bool {
        self.state == DeviceState::Associated
    }
}

/// Global singleton — `None` until PCI probe finds a device.
static mut DEVICE: Option<IwlDevice> = None;

/// Probe PCI, create device if found, but do NOT load firmware yet.
/// Safe to call on every boot; returns `true` if a device was found.
pub fn init() -> bool {
    let pci_info = match super::iwlwifi_pci::probe() {
        Some(p) => p,
        None => return false,
    };
    unsafe {
        DEVICE = Some(IwlDevice::new(pci_info));
    }
    true
}

/// Access the global device.  Returns `Some` after `init()` succeeds.
pub fn device() -> Option<&'static mut IwlDevice> {
    unsafe { DEVICE.as_mut() }
}
