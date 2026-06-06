//! iwlwifi firmware mapping — M11 stage 5.
//!
//! Maps each supported PCI device ID to the firmware file name(s) that
//! Linux's `iwlwifi` driver loads for that chip.  When metal bring-up
//! begins, the relevant blob is embedded with `include_bytes!` and
//! referenced here.
//!
//! QEMU-safe: pure lookup tables, no hardware access.

use super::iwlwifi_pci::IwlPciInfo;
use crate::println;

/// A single firmware entry.
#[derive(Copy, Clone, Debug)]
pub struct FirmwareEntry {
    /// Human-readable chip family.
    pub family: &'static str,
    /// Primary ucode file (e.g. `iwlwifi-7260-17.ucode`).
    pub ucode: &'static str,
    /// Optional PNVM (PHY NVM) file for newer chips (AX200/AX211).
    pub pnvm: Option<&'static str>,
    /// Optional regulatory NVM file.
    pub nvm: Option<&'static str>,
}

/// Lookup table: device_id → firmware entry.
/// Keep in sync with `IWLWIFI_DEVICES` in `iwlwifi_pci.rs`.
pub const FW_TABLE: &[(u16, FirmwareEntry)] = &[
    // 7260 / 3160 (ThinkPad T540 / T440p era).
    (0x08B1, FirmwareEntry {
        family: "7260",
        ucode: "iwlwifi-7260-17.ucode",
        pnvm: None,
        nvm: Some("iwlwifi-7260-10.nvm"),
    }),
    (0x08B2, FirmwareEntry {
        family: "7260",
        ucode: "iwlwifi-7260-17.ucode",
        pnvm: None,
        nvm: Some("iwlwifi-7260-10.nvm"),
    }),
    (0x08B3, FirmwareEntry {
        family: "3160",
        ucode: "iwlwifi-3160-17.ucode",
        pnvm: None,
        nvm: Some("iwlwifi-3160-10.nvm"),
    }),
    (0x08B4, FirmwareEntry {
        family: "3160",
        ucode: "iwlwifi-3160-17.ucode",
        pnvm: None,
        nvm: Some("iwlwifi-3160-10.nvm"),
    }),
    // AX211 (ThinkPad P1 Gen 6 / Raptor Lake).
    (0x51F0, FirmwareEntry {
        family: "AX211",
        ucode: "iwlwifi-ty-a0-gf-a0-83.ucode",
        pnvm: Some("iwlwifi-ty-a0-gf-a0.pnvm"),
        nvm: Some("iwlwifi-ty-a0-gf-a0-83.nvm"),
    }),
    (0x51F1, FirmwareEntry {
        family: "AX211",
        ucode: "iwlwifi-ty-a0-gf-a0-83.ucode",
        pnvm: Some("iwlwifi-ty-a0-gf-a0.pnvm"),
        nvm: Some("iwlwifi-ty-a0-gf-a0-83.nvm"),
    }),
    (0x54F0, FirmwareEntry {
        family: "AX211",
        ucode: "iwlwifi-ty-a0-gf-a0-83.ucode",
        pnvm: Some("iwlwifi-ty-a0-gf-a0.pnvm"),
        nvm: Some("iwlwifi-ty-a0-gf-a0-83.nvm"),
    }),
];

/// Look up the firmware entry for a probed PCI device.
/// Returns `None` if the device ID is not in the table.
pub fn lookup(pci: &IwlPciInfo) -> Option<&'static FirmwareEntry> {
    for &(id, ref entry) in FW_TABLE {
        if id == pci.device_id {
            return Some(entry);
        }
    }
    None
}

/// Print a diagnostic summary of the firmware mapping for a probed device.
pub fn print_mapping(pci: &IwlPciInfo) {
    match lookup(pci) {
        Some(fw) => {
            println!("[iwlwifi-fw] {}  family={}  ucode={}",
                pci.name, fw.family, fw.ucode);
            if let Some(pnvm) = fw.pnvm {
                println!("[iwlwifi-fw]   PNVM = {}", pnvm);
            }
            if let Some(nvm) = fw.nvm {
                println!("[iwlwifi-fw]   NVM  = {}", nvm);
            }
        }
        None => {
            println!("[iwlwifi-fw] WARNING: no firmware mapping for device_id=0x{:04X}",
                pci.device_id);
        }
    }
}
