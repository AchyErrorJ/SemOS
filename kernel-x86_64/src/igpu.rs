//! Intel integrated graphics read-only probe — M14 step 1.
//!
//! This is deliberately **not** a native modesetting driver yet. The T540p
//! already boots through UEFI GOP, and `framebuffer.rs` owns that linear
//! framebuffer. The first iGPU milestone is safe inventory:
//!
//! - identify the Intel HD 4600 / Haswell GT2 (`8086:0416`) by PCI config space;
//! - report BARs and command-register state without resizing BARs;
//! - report the current GOP framebuffer geometry/format;
//! - leave every display/MMIO register untouched.
//!
//! Reading PCI config space is safe in QEMU and on metal. This module performs
//! no MMIO reads and no PCI config writes.

use crate::{framebuffer, pci, println};

pub const INTEL_VENDOR_ID: u16 = 0x8086;
pub const HASWELL_GT2_MOBILE_HD4600: u16 = 0x0416;

#[derive(Clone, Copy)]
pub struct IgpuInfo {
    pub loc: pci::Location,
    pub device_id: u16,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
    pub bar0: BarInfo,
    pub bar2: BarInfo,
    pub bar4: BarInfo,
}

#[derive(Clone, Copy)]
pub struct BarInfo {
    pub index: u8,
    pub raw_low: u32,
    pub raw_high: u32,
    pub kind: BarKind,
    /// Decoded BAR size in bytes, or 0 if absent/un-sized.
    pub size: u64,
}

#[derive(Clone, Copy)]
pub enum BarKind {
    Absent,
    Io {
        base: u32,
    },
    Mmio32 {
        base: u32,
        prefetchable: bool,
    },
    Mmio64 {
        base: u64,
        prefetchable: bool,
    },
    Reserved,
}

/// Find the primary Intel display controller by PCI config space. Read-only;
/// safe if absent. This is the non-printing version used by drivers that need
/// the location/BARs without producing boot noise.
pub fn find() -> Option<IgpuInfo> {
    let loc = find_intel_display();
    let loc = loc?;

    let device_id = loc.device_id();
    let subsystem_vendor = pci::read_u16(loc.bus, loc.slot, loc.func, 0x2C);
    let subsystem_device = pci::read_u16(loc.bus, loc.slot, loc.func, 0x2E);

    let bar0 = read_bar(loc, 0);
    let bar2 = read_bar(loc, 2);
    let bar4 = read_bar(loc, 4);

    Some(IgpuInfo {
        loc,
        device_id,
        subsystem_vendor,
        subsystem_device,
        bar0,
        bar2,
        bar4,
    })
}

/// Probe for the primary Intel display controller and print diagnostics.
/// Read-only; safe if absent.
pub fn probe() -> Option<IgpuInfo> {
    let info = match find() {
        Some(i) => i,
        None => {
            println!("[igpu] no Intel integrated display controller found");
            print_gop_framebuffer();
            return None;
        }
    };

    let (class, subclass, prog_if) = pci::class_triple(info.loc);
    let name = intel_display_name(info.device_id);
    println!(
        "[igpu] {} @ {:02X}:{:02X}.{} device=0x{:04X} class={:02X}/{:02X}/{:02X}",
        name, info.loc.bus, info.loc.slot, info.loc.func,
        info.device_id, class, subclass, prog_if
    );
    println!(
        "[igpu] subsystem vendor/device=0x{:04X}:0x{:04X}",
        info.subsystem_vendor, info.subsystem_device
    );

    let command = pci::read_u16(info.loc.bus, info.loc.slot, info.loc.func, pci::regs::COMMAND);
    println!(
        "[igpu] PCI command: IO={} MEM={} BUSMASTER={} (read-only probe; no writes)",
        yes(command & pci::cmd::IO_SPACE != 0),
        yes(command & pci::cmd::MEMORY_SPACE != 0),
        yes(command & pci::cmd::BUS_MASTER != 0),
    );

    // Intel Gen7/Haswell display device BAR layout observed on the T540p:
    // BAR0/1 = MMIO registers, BAR2/3 = graphics aperture, BAR4 = VGA I/O.
    // We only decode current BAR values. We do NOT perform BAR sizing because
    // that requires temporary PCI config writes.
    print_bar("BAR0 MMIO", info.bar0);
    print_bar("BAR2 aperture", info.bar2);
    print_bar("BAR4 I/O", info.bar4);

    if info.device_id == HASWELL_GT2_MOBILE_HD4600 {
        println!("[igpu] target match: Haswell GT2 / Intel HD 4600 (8086:0416)");
    } else {
        println!(
            "[igpu] Intel display device is not the M14 write target; keeping probe read-only"
        );
    }

    print_gop_framebuffer();
    println!("[igpu] native-control status: PCI inventory only; GOP framebuffer remains active");

    Some(info)
}

fn find_intel_display() -> Option<pci::Location> {
    let mut first_intel_display = None;
    for bus in 0..=255u8 {
        for slot in 0..32u8 {
            for func in 0..8u8 {
                let vendor = pci::read_u16(bus, slot, func, pci::regs::VENDOR_ID);
                if vendor == 0xFFFF {
                    continue;
                }
                let loc = pci::Location { bus, slot, func };
                let device = pci::read_u16(bus, slot, func, pci::regs::DEVICE_ID);
                let (class, _subclass, _prog_if) = pci::class_triple(loc);
                if vendor == INTEL_VENDOR_ID && class == 0x03 {
                    if device == HASWELL_GT2_MOBILE_HD4600 {
                        return Some(loc);
                    }
                    if first_intel_display.is_none() {
                        first_intel_display = Some(loc);
                    }
                }
                if func == 0 {
                    let header_type = (pci::read_u32(bus, slot, 0, 0x0C) >> 16) as u8;
                    if header_type & 0x80 == 0 {
                        break;
                    }
                }
            }
        }
    }
    first_intel_display
}

fn read_bar(loc: pci::Location, index: u8) -> BarInfo {
    let offset = 0x10 + index * 4;
    let raw_low = pci::read_u32(loc.bus, loc.slot, loc.func, offset);
    if raw_low == 0 {
        return BarInfo { index, raw_low, raw_high: 0, kind: BarKind::Absent, size: 0 };
    }

    if raw_low & 1 != 0 {
        let size = bar_size(loc, index, false);
        return BarInfo {
            index,
            raw_low,
            raw_high: 0,
            kind: BarKind::Io { base: raw_low & !0x3 },
            size,
        };
    }

    let prefetchable = raw_low & (1 << 3) != 0;
    match (raw_low >> 1) & 0x3 {
        0x0 => {
            let size = bar_size(loc, index, false);
            BarInfo {
                index,
                raw_low,
                raw_high: 0,
                kind: BarKind::Mmio32 { base: raw_low & !0xF, prefetchable },
                size,
            }
        }
        0x2 => {
            let raw_high = pci::read_u32(loc.bus, loc.slot, loc.func, offset + 4);
            let size = bar_size(loc, index, true);
            let base = ((raw_high as u64) << 32) | ((raw_low as u64) & 0xFFFF_FFF0);
            BarInfo {
                index,
                raw_low,
                raw_high,
                kind: BarKind::Mmio64 { base, prefetchable },
                size,
            }
        }
        _ => BarInfo { index, raw_low, raw_high: 0, kind: BarKind::Reserved, size: 0 },
    }
}

/// Compute a BAR's size by the standard PCI sizing dance: save the current
/// value, write all-ones, read back the mask, restore the original value.
fn bar_size(loc: pci::Location, index: u8, is_64bit: bool) -> u64 {
    let offset = 0x10 + index * 4;
    let low = pci::read_u32(loc.bus, loc.slot, loc.func, offset);

    pci::write_u32(loc.bus, loc.slot, loc.func, offset, 0xFFFF_FFFF);
    let masked_low = pci::read_u32(loc.bus, loc.slot, loc.func, offset);
    pci::write_u32(loc.bus, loc.slot, loc.func, offset, low);

    let high = if is_64bit {
        let offset_high = offset + 4;
        let saved_high = pci::read_u32(loc.bus, loc.slot, loc.func, offset_high);
        pci::write_u32(loc.bus, loc.slot, loc.func, offset_high, 0xFFFF_FFFF);
        let masked_high = pci::read_u32(loc.bus, loc.slot, loc.func, offset_high);
        pci::write_u32(loc.bus, loc.slot, loc.func, offset_high, saved_high);
        masked_high as u64
    } else {
        0xFFFF_FFFF
    };

    let mask = ((high << 32) | (masked_low as u64)) & !0xF;
    if mask == 0 {
        return 0;
    }
    let size = (!mask).wrapping_add(1) & ((1u64 << 63) - 1);
    if size == 0 { 0 } else { size }
}

fn print_bar(label: &str, bar: BarInfo) {
    match bar.kind {
        BarKind::Absent => println!(
            "[igpu] {} BAR{}: absent/raw=0x{:08X}",
            label, bar.index, bar.raw_low
        ),
        BarKind::Io { base } => println!(
            "[igpu] {} BAR{}: I/O base=0x{:04X} size=0x{:08X} raw=0x{:08X}",
            label, bar.index, base, bar.size, bar.raw_low
        ),
        BarKind::Mmio32 { base, prefetchable } => println!(
            "[igpu] {} BAR{}: MMIO32 base=0x{:08X} size=0x{:08X} prefetch={} raw=0x{:08X}",
            label, bar.index, base, bar.size, yes(prefetchable), bar.raw_low
        ),
        BarKind::Mmio64 { base, prefetchable } => println!(
            "[igpu] {} BAR{}: MMIO64 base=0x{:016X} size=0x{:08X} prefetch={} raw=0x{:08X}:0x{:08X}",
            label, bar.index, base, bar.size, yes(prefetchable), bar.raw_high, bar.raw_low
        ),
        BarKind::Reserved => println!(
            "[igpu] {} BAR{}: reserved memory BAR type raw=0x{:08X}",
            label, bar.index, bar.raw_low
        ),
    }
}

fn print_gop_framebuffer() {
    match framebuffer::fb_info() {
        Some(info) => println!(
            "[igpu] GOP framebuffer: {}x{} stride={} bpp={} bytes={} fmt={} virt=0x{:016X}",
            info.width,
            info.height,
            info.stride,
            info.bytes_per_pixel,
            info.byte_len,
            framebuffer::pixel_format_name(info.format),
            info.addr,
        ),
        None => println!("[igpu] GOP framebuffer: unavailable"),
    }
}

fn intel_display_name(device_id: u16) -> &'static str {
    match device_id {
        HASWELL_GT2_MOBILE_HD4600 => "Intel HD 4600 / Haswell GT2",
        0x0412 => "Intel HD 4600 / Haswell GT2 desktop",
        0x041A => "Intel Haswell server/workstation graphics",
        0x0A16 => "Intel Haswell ULT integrated graphics",
        0x0D26 => "Intel Crystal Well integrated graphics",
        _ => "Intel integrated/display controller",
    }
}

#[inline]
fn yes(v: bool) -> &'static str {
    if v { "yes" } else { "no" }
}
