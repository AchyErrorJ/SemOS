//! iwlwifi PCI probe — M11 stage 1.
//!
//! Scans PCI bus 0 for Intel VID 0x8086 + known iwlwifi device IDs.
//! On a match, enables bus-mastering + memory-space access and maps
//! BAR0 (the CSR/MMIO region).  QEMU-safe: if no device is found the
//! probe simply returns `None`.

use crate::pci;
use crate::println;

/// Intel PCI vendor ID — constant across all AX/AC/7260 families.
pub const INTEL_VENDOR_ID: u16 = 0x8086;

/// Known iwlwifi devices: (device_id, friendly_name).
pub const IWLWIFI_DEVICES: &[(u16, &str)] = &[
    // ThinkPad T540 / T440p stage 1 — 7260 / 3160 (mini-PCIe).
    (0x08B1, "Wireless 7260"),
    (0x08B2, "Wireless 7260"),
    (0x08B3, "Wireless 3160"),
    (0x08B4, "Wireless 3160"),
    // ThinkPad P1 Gen 6 stage 2 — AX211 (Wi-Fi 6E, Raptor Lake).
    (0x51F0, "Wi-Fi 6E AX211"),
    (0x51F1, "Wi-Fi 6E AX211"),
    (0x54F0, "Wi-Fi 6E AX211"),
];

/// Result of a successful PCI probe.
#[derive(Copy, Clone, Debug)]
pub struct IwlPciInfo {
    pub loc: pci::Location,
    pub device_id: u16,
    pub name: &'static str,
    /// Physical address of BAR0 (the CSR region).  This is the address
    /// we pass to `phys_to_virt` before doing MMIO read/write.
    pub bar0_phys: u64,
    /// Size of the BAR0 region (from the PCI BAR mask).
    pub bar0_size: u64,
}

/// Walk PCI bus 0 looking for a known iwlwifi NIC.
/// Returns `Some(IwlPciInfo)` on the first match, `None` if none found.
pub fn probe() -> Option<IwlPciInfo> {
    // Scan the first 32 slots on bus 0, function 0.  iwlwifi is usually
    // at a low slot number on Intel PCHs (e.g. 00:03.0).
    for slot in 0..32u8 {
        let vendor = pci::read_u16(0, slot, 0, pci::regs::VENDOR_ID);
        if vendor != INTEL_VENDOR_ID {
            continue;
        }
        let device = pci::read_u16(0, slot, 0, pci::regs::DEVICE_ID);
        if let Some(name) = device_name(device) {
            let loc = pci::Location { bus: 0, slot, func: 0 };

            // Enable bus-master + memory-space in the PCI command register.
            let cmd = pci::read_u32(0, slot, 0, pci::regs::COMMAND);
            let cmd_en = cmd | 0x0006; // bit 1 = Memory Space, bit 2 = Bus Master
            pci::write_u32(0, slot, 0, pci::regs::COMMAND, cmd_en);

            // Read BAR0 (offset 0x10) to get the MMIO base.
            let bar0_raw = pci::read_u32(0, slot, 0, pci::regs::BAR0);
            if bar0_raw & 0x1 != 0 {
                println!("[iwlwifi-pci] {} @ {:02X}:{:02X}.{} has IO BAR (unsupported)",
                    name, 0, slot, 0);
                continue;
            }
            let bar0_phys = (bar0_raw & !0xF) as u64;

            // Determine BAR size by writing all-1s, reading back the mask,
            // then restoring the original value.
            pci::write_u32(0, slot, 0, pci::regs::BAR0, 0xFFFF_FFFF);
            let mask = pci::read_u32(0, slot, 0, pci::regs::BAR0);
            pci::write_u32(0, slot, 0, pci::regs::BAR0, bar0_raw);
            let bar0_size = if mask == 0 {
                0
            } else {
                let size_mask = !(mask & !0xF) + 1;
                size_mask as u64
            };

            println!("[iwlwifi-pci] found {} @ {:02X}:{:02X}.{}  device=0x{:04X}  BAR0=0x{:08X} size=0x{:X}",
                name, 0, slot, 0, device, bar0_phys, bar0_size);

            return Some(IwlPciInfo {
                loc,
                device_id: device,
                name,
                bar0_phys,
                bar0_size,
            });
        }
    }
    None
}

fn device_name(device_id: u16) -> Option<&'static str> {
    for &(d, name) in IWLWIFI_DEVICES {
        if d == device_id {
            return Some(name);
        }
    }
    None
}
