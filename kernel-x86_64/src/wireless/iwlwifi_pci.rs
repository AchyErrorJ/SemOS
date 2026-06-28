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

/// Walk ALL PCI buses looking for a known iwlwifi NIC. On a ThinkPad the
/// WiFi card sits behind a PCIe root port, so it lives on a higher bus
/// (e.g. 01:00.0 / 02:00.0), not bus 0 — a bus-0-only scan misses it.
/// Also dumps every network-class (0x02) device so an unrecognized card
/// (wrong device ID, or a non-Intel module) is still visible in the log.
/// Returns `Some(IwlPciInfo)` on the first iwlwifi match.
pub fn probe() -> Option<IwlPciInfo> {
    let mut found: Option<(u8, u8, u8, u16)> = None;
    for bus in 0..=255u8 {
        for slot in 0..32u8 {
            for func in 0..8u8 {
                let vendor = pci::read_u16(bus, slot, func, pci::regs::VENDOR_ID);
                if vendor == 0xFFFF {
                    continue;
                }
                let device = pci::read_u16(bus, slot, func, pci::regs::DEVICE_ID);
                let loc = pci::Location { bus, slot, func };
                let (class, sub, _pi) = pci::class_triple(loc);
                // Diagnostic: surface every network-controller (class 0x02)
                // so a card we don't recognize is still in the log.
                if class == 0x02 {
                    println!("[iwlwifi-pci] network device {:02X}:{:02X}.{}  vendor=0x{:04X} device=0x{:04X} class=0x{:02X} subclass=0x{:02X}",
                        bus, slot, func, vendor, device, class, sub);
                }
                if vendor == INTEL_VENDOR_ID {
                    if let Some(_name) = device_name(device) {
                        if found.is_none() {
                            found = Some((bus, slot, func, device));
                        }
                    }
                }
                // Skip non-multifunction extra funcs to save time.
                if func == 0 {
                    let header_type = (pci::read_u32(bus, slot, 0, 0x0C) >> 16) as u8;
                    if header_type & 0x80 == 0 {
                        break;
                    }
                }
            }
        }
    }

    let (bus, slot, func, device) = found?;
    let name = device_name(device).unwrap_or("iwlwifi");
    let loc = pci::Location { bus, slot, func };

    // Enable bus-master + memory-space (bits 1,2) AND set Interrupt Disable
    // (bit 10): this driver POLLS rb_stts, so we never want the card asserting
    // its legacy INTx line. Once the radio gets active (scan running) it would
    // raise IRQ7 → IDT[0x27], which has no handler → #NP. Masking INTx at the
    // PCI level prevents that regardless of the firmware's internal int state.
    let cmd = pci::read_u32(bus, slot, func, pci::regs::COMMAND);
    pci::write_u32(bus, slot, func, pci::regs::COMMAND, (cmd | 0x0006) | 0x0400);

    let bar0_raw = pci::read_u32(bus, slot, func, pci::regs::BAR0);
    if bar0_raw & 0x1 != 0 {
        println!("[iwlwifi-pci] {} @ {:02X}:{:02X}.{} has IO BAR (unsupported)",
            name, bus, slot, func);
        return None;
    }
    let bar0_phys = (bar0_raw & !0xF) as u64;
    pci::write_u32(bus, slot, func, pci::regs::BAR0, 0xFFFF_FFFF);
    let mask = pci::read_u32(bus, slot, func, pci::regs::BAR0);
    pci::write_u32(bus, slot, func, pci::regs::BAR0, bar0_raw);
    let bar0_size = if mask == 0 { 0 } else { (!(mask & !0xF) + 1) as u64 };

    println!("[iwlwifi-pci] found {} @ {:02X}:{:02X}.{}  device=0x{:04X}  BAR0=0x{:08X} size=0x{:X}",
        name, bus, slot, func, device, bar0_phys, bar0_size);

    Some(IwlPciInfo { loc, device_id: device, name, bar0_phys, bar0_size })
}

fn device_name(device_id: u16) -> Option<&'static str> {
    for &(d, name) in IWLWIFI_DEVICES {
        if d == device_id {
            return Some(name);
        }
    }
    None
}
