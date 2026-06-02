//! Minimal PCI configuration-space access for x86_64.
//!
//! Uses the legacy I/O-port mechanism (CONFIG_ADDRESS=0xCF8 /
//! CONFIG_DATA=0xCFC). Sufficient to find a VirtIO block device on
//! QEMU. Doesn't yet handle multifunction devices, PCI-to-PCI bridges,
//! or memory-mapped configuration (MMCONFIG).
//!
//! # Discovering a VirtIO device
//!
//! ```ignore
//! if let Some(loc) = pci::find_first(0x1AF4, 0x1001) {
//!     // Found VirtIO Legacy block device at loc.bus / loc.slot
//! }
//! ```

use core::arch::asm;

/// CONFIG_ADDRESS port — write the (bus, slot, fn, offset) here.
const CONFIG_ADDRESS: u16 = 0xCF8;
/// CONFIG_DATA port — read or write a 32-bit value at the configured address.
const CONFIG_DATA: u16 = 0xCFC;

/// Encode the bus/slot/function/offset into the CONFIG_ADDRESS layout.
/// Bit 31 = enable, bits 23..16 = bus, 15..11 = slot, 10..8 = function,
/// 7..2 = register offset (must be DWORD-aligned).
#[inline]
fn config_address(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    (1 << 31)
        | ((bus as u32) << 16)
        | (((slot as u32) & 0x1F) << 11)
        | (((func as u32) & 0x07) << 8)
        | ((offset as u32) & 0xFC)
}

#[inline]
unsafe fn outl(port: u16, value: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
}

#[inline]
unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

/// Read a 32-bit register from the given (bus, slot, function, offset).
/// `offset` is in bytes, but only DWORD-aligned (offset & 0xFC) accesses are addressable.
pub fn read_u32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, config_address(bus, slot, func, offset));
        inl(CONFIG_DATA)
    }
}

/// Read a 16-bit register at the given offset (offset can be byte-granular within the DWORD).
pub fn read_u16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    let dword = read_u32(bus, slot, func, offset & 0xFC);
    let shift = (offset & 0x02) * 8;
    ((dword >> shift) & 0xFFFF) as u16
}

/// Write a 32-bit register at a DWORD-aligned offset.
pub fn write_u32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, config_address(bus, slot, func, offset));
        outl(CONFIG_DATA, value);
    }
}

/// PCI config-space register offsets (subset).
pub mod regs {
    pub const VENDOR_ID:    u8 = 0x00; // u16
    pub const DEVICE_ID:    u8 = 0x02; // u16
    pub const COMMAND:      u8 = 0x04; // u16
    pub const STATUS:       u8 = 0x06; // u16
    pub const CLASS_CODE:   u8 = 0x0B; // u8
    pub const HEADER_TYPE:  u8 = 0x0E; // u8
    pub const BAR0:         u8 = 0x10; // u32
    pub const BAR1:         u8 = 0x14; // u32
    pub const INTERRUPT_LINE: u8 = 0x3C; // u8
}

/// COMMAND register bits we need.
pub mod cmd {
    pub const IO_SPACE:     u16 = 1 << 0;
    pub const MEMORY_SPACE: u16 = 1 << 1;
    pub const BUS_MASTER:   u16 = 1 << 2;
}

/// Where on the PCI bus a device lives.
#[derive(Clone, Copy, Debug)]
pub struct Location {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
}

impl Location {
    #[inline]
    pub fn vendor_id(self) -> u16 {
        read_u16(self.bus, self.slot, self.func, regs::VENDOR_ID)
    }
    #[inline]
    pub fn device_id(self) -> u16 {
        read_u16(self.bus, self.slot, self.func, regs::DEVICE_ID)
    }
    #[inline]
    pub fn bar0(self) -> u32 {
        read_u32(self.bus, self.slot, self.func, regs::BAR0)
    }
    /// Enable I/O space + bus mastering. Required before the device
    /// will respond to MMIO/IO and DMA reads/writes.
    pub fn enable_io_and_bus_master(self) {
        let cmd = read_u16(self.bus, self.slot, self.func, regs::COMMAND);
        let new = cmd | cmd::IO_SPACE | cmd::MEMORY_SPACE | cmd::BUS_MASTER;
        // Write as part of the same DWORD (status register is the upper 16 bits).
        let status_cmd = read_u32(self.bus, self.slot, self.func, regs::COMMAND);
        let updated = (status_cmd & 0xFFFF_0000) | (new as u32);
        write_u32(self.bus, self.slot, self.func, regs::COMMAND, updated);
    }
}

/// Iterate all PCI bus 0 slots looking for a device that matches
/// `(vendor, device)`. Returns the first match, or None if no match.
/// Doesn't recurse into PCI-PCI bridges or scan multifunction devices.
pub fn find_first(vendor: u16, device: u16) -> Option<Location> {
    for slot in 0..32u8 {
        let loc = Location { bus: 0, slot, func: 0 };
        let v = loc.vendor_id();
        if v == 0xFFFF {
            // No device present.
            continue;
        }
        let d = loc.device_id();
        if v == vendor && d == device {
            return Some(loc);
        }
    }
    None
}

/// Read the (class, subclass, prog_if) triple for a device. These live in the
/// upper 3 bytes of the DWORD at offset 0x08 (revision is the low byte).
pub fn class_triple(loc: Location) -> (u8, u8, u8) {
    let dw = read_u32(loc.bus, loc.slot, loc.func, 0x08);
    let class = ((dw >> 24) & 0xFF) as u8;
    let subclass = ((dw >> 16) & 0xFF) as u8;
    let prog_if = ((dw >> 8) & 0xFF) as u8;
    (class, subclass, prog_if)
}

/// Find the first device on bus 0 matching a (class, subclass, prog_if) triple.
/// Used for class-coded devices whose vendor/device IDs vary (e.g. NVMe =
/// 0x01/0x08/0x02). Now walks all 8 functions of multifunction slots —
/// required for real-hardware Intel PCH chipsets where the SATA AHCI
/// controller is at `0:1F.2` (function 2), not function 0. QEMU's
/// ich9-ahci sits at function 0 so the old function-0-only scan
/// happened to work in emulation but failed on real boards.
pub fn find_by_class(class: u8, subclass: u8, prog_if: u8) -> Option<Location> {
    for slot in 0..32u8 {
        let loc0 = Location { bus: 0, slot, func: 0 };
        if loc0.vendor_id() == 0xFFFF {
            continue;
        }
        if class_triple(loc0) == (class, subclass, prog_if) {
            return Some(loc0);
        }
        // PCI header-type byte (offset 0x0E): bit 7 set = multifunction.
        let header_type = (read_u32(0, slot, 0, 0x0C) >> 16) as u8;
        if header_type & 0x80 == 0 {
            continue;
        }
        for func in 1..8u8 {
            let loc = Location { bus: 0, slot, func };
            if loc.vendor_id() == 0xFFFF {
                continue;
            }
            if class_triple(loc) == (class, subclass, prog_if) {
                return Some(loc);
            }
        }
    }
    None
}

/// Read a 64-bit MMIO BAR pair (BAR0 low + BAR1 high), masking the type bits.
/// Returns the physical base address, or None if BAR0 is an I/O (not memory) BAR.
pub fn mmio_bar64(loc: Location) -> Option<u64> {
    let bar0 = loc.bar0();
    if bar0 & 1 != 0 {
        return None; // I/O space BAR, not memory
    }
    let bar_type = (bar0 >> 1) & 0x3;
    let base = if bar_type == 0x2 {
        let bar1 = read_u32(loc.bus, loc.slot, loc.func, regs::BAR1);
        ((bar1 as u64) << 32) | ((bar0 as u64) & 0xFFFF_FFF0)
    } else {
        (bar0 as u64) & 0xFFFF_FFF0
    };
    Some(base)
}

/// Print a one-line summary of every device on bus 0, including
/// multifunction sub-functions. Useful for boot-time diagnostics.
pub fn print_bus_0() {
    let mut count = 0;
    for slot in 0..32u8 {
        let loc0 = Location { bus: 0, slot, func: 0 };
        let v = loc0.vendor_id();
        if v == 0xFFFF {
            continue;
        }
        let d = loc0.device_id();
        let (c, sc, pi) = class_triple(loc0);
        crate::println!("    PCI 00:{:02X}.0  ven=0x{:04X} dev=0x{:04X} class={:02X}/{:02X}/{:02X}",
            slot, v, d, c, sc, pi);
        count += 1;
        // Walk other functions of multifunction devices (e.g. PCH at 0:1F).
        let header_type = (read_u32(0, slot, 0, 0x0C) >> 16) as u8;
        if header_type & 0x80 == 0 {
            continue;
        }
        for func in 1..8u8 {
            let loc = Location { bus: 0, slot, func };
            let v = loc.vendor_id();
            if v == 0xFFFF {
                continue;
            }
            let d = loc.device_id();
            let (c, sc, pi) = class_triple(loc);
            crate::println!("    PCI 00:{:02X}.{}  ven=0x{:04X} dev=0x{:04X} class={:02X}/{:02X}/{:02X}",
                slot, func, v, d, c, sc, pi);
            count += 1;
        }
    }
    crate::println!("    {} PCI devices on bus 0", count);
}
