//! NVIDIA dGPU probe — Phase 12 / M18 step 1 (local-compute path groundwork).
//!
//! Read-only PCI enumeration of an NVIDIA display controller (vendor 0x10DE,
//! class 0x03) + its BARs. On the T540p this should report a **Kepler GK208**
//! (GeForce GT 740M, CC 3.5) — the pre-signed-firmware generation that makes
//! from-scratch compute tractable (see `docs/KEPLER_DGPU_COMPUTE_DESIGN_2026-06-15.md`).
//!
//! Also settles the long-open "what GPU is actually in this machine" question
//! (project_test_machine_hardware memory).
//!
//! SAFETY: PCI *config-space* reads are always safe. The optional MMIO chip-ID
//! read (PMC_BOOT_0) is gated behind `MMIO_PROBE` because (a) on an Optimus
//! laptop the dGPU may be power-gated until enabled via ACPI, and (b) the BAR
//! must actually be mapped in the bootloader's phys map or the read faults. The
//! PCI device ID already identifies the chip, so the MMIO read is confirmatory.

use crate::pci;
use crate::println;

/// Verbose GPU probe trace. The PCI scan is cheap, but the per-device
/// banner + BAR dump is noisy on machines without an NVIDIA dGPU. Flip to
/// `true` + rebuild when actually debugging M18 bring-up.
pub const GPU_VERBOSE: bool = false;

macro_rules! gpudbg {
    ($($arg:tt)*) => {{ if $crate::gpu::GPU_VERBOSE { $crate::println!($($arg)*); } }};
}

pub const NVIDIA_VENDOR_ID: u16 = 0x10DE;

/// Set true (and rebuild) to additionally read PMC_BOOT_0 over MMIO. Needs the
/// dGPU powered (not Optimus-gated) and BAR0 mapped — risks a #PF otherwise.
const MMIO_PROBE: bool = false;

/// Kepler mobile GPUs we expect on this class of machine (device_id, name).
/// The PCI device ID alone pins the chip; PMC_BOOT_0 only confirms it. The test
/// rig is uncertain (T540p-bezel but maybe a W540 — see project_test_machine_
/// hardware): a T540p dGPU option is the GT 740M; a W540 carries a Quadro
/// K1100M/K2100M. BOTH are Kepler, so the M18 design holds either way — this
/// probe just settles which.
const KNOWN: &[(u16, &str)] = &[
    // GeForce (T540p dGPU option)
    (0x0FE4, "GK107M [GeForce GT 740M]   (Kepler, CC 3.0)"),
    (0x1290, "GK208M [GeForce GT 730M]   (Kepler, CC 3.5)"),
    (0x1291, "GK208M [GeForce GT 735M]   (Kepler, CC 3.5)"),
    (0x1292, "GK208M [GeForce GT 740M]   (Kepler, CC 3.5)"),
    (0x1293, "GK208M [GeForce GT 730M]   (Kepler, CC 3.5)"),
    (0x1295, "GK208M [GeForce 710M]      (Kepler, CC 3.5)"),
    (0x1299, "GK208BM [GeForce 920M]     (Kepler, CC 3.5)"),
    // Quadro (W540 workstation GPU)
    (0x0FF6, "GK107GLM [Quadro K1100M]   (Kepler, CC 3.0)"),
    (0x11FC, "GK106GLM [Quadro K2100M]   (Kepler, CC 3.0)"),
    (0x0FFB, "GK107GLM [Quadro K2000M]   (Kepler, CC 3.0)"),
    (0x0FFC, "GK107GLM [Quadro K1000M]   (Kepler, CC 3.0)"),
];

#[derive(Copy, Clone)]
pub struct GpuPciInfo {
    pub loc: pci::Location,
    pub device_id: u16,
    pub name: &'static str,
    pub bar0_phys: u64, // MMIO registers (16 MB on Kepler)
    pub bar0_size: u64,
    pub bar1_phys: u64, // VRAM aperture (64-bit BAR)
}

/// Probe for an NVIDIA GPU. Read-only config-space scan; safe on QEMU (→ None).
pub fn probe() -> Option<GpuPciInfo> {
    let mut found: Option<pci::Location> = None;
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
                // Surface every display-class (0x03) device so an unrecognized
                // GPU is still visible in the log.
                if class == 0x03 {
                    gpudbg!("[gpu-pci] display {:02X}:{:02X}.{} vendor=0x{:04X} device=0x{:04X} class=0x{:02X} sub=0x{:02X}",
                        bus, slot, func, vendor, device, class, sub);
                }
                if vendor == NVIDIA_VENDOR_ID && class == 0x03 && found.is_none() {
                    found = Some(loc);
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

    let loc = found?;
    let (bus, slot, func) = (loc.bus, loc.slot, loc.func);
    let device = pci::read_u16(bus, slot, func, pci::regs::DEVICE_ID);
    let name = KNOWN.iter().find(|&&(d, _)| d == device).map(|&(_, n)| n)
        .unwrap_or("NVIDIA GPU (unrecognized device id)");

    // Memory-space enable so a later MMIO map can read BAR0. (We do NOT set
    // bus-master here — no DMA until the real driver.)
    let cmd = pci::read_u32(bus, slot, func, pci::regs::COMMAND);
    pci::write_u32(bus, slot, func, pci::regs::COMMAND, cmd | 0x0002);

    // BAR0 = MMIO. BAR1 = VRAM aperture (NVIDIA BAR1 is 64-bit: 0x14 low + 0x18 high).
    let bar0_raw = pci::read_u32(bus, slot, func, pci::regs::BAR0);
    let bar0_phys = (bar0_raw & !0xF) as u64;
    pci::write_u32(bus, slot, func, pci::regs::BAR0, 0xFFFF_FFFF);
    let mask = pci::read_u32(bus, slot, func, pci::regs::BAR0);
    pci::write_u32(bus, slot, func, pci::regs::BAR0, bar0_raw);
    let bar0_size = if mask == 0 { 0 } else { (!(mask & !0xF) + 1) as u64 };

    let bar1_low = (pci::read_u32(bus, slot, func, 0x14) & !0xF) as u64;
    let bar1_high = pci::read_u32(bus, slot, func, 0x18) as u64;
    let bar1_phys = (bar1_high << 32) | bar1_low;

    gpudbg!("[gpu-pci] found {} @ {:02X}:{:02X}.{} device=0x{:04X}",
        name, bus, slot, func, device);
    gpudbg!("[gpu-pci]   BAR0(MMIO)=0x{:08X} size=0x{:X}  BAR1(VRAM)=0x{:010X}",
        bar0_phys, bar0_size, bar1_phys);

    if MMIO_PROBE && bar0_phys != 0 {
        // PMC_BOOT_0 @ MMIO offset 0 — the chip-identity register. nouveau:
        // chipset = (boot0 >> 20) & 0x1ff (GK208 = 0x108, GK107 = 0x0E7).
        let bar0_virt = crate::paging::phys_to_virt(bar0_phys);
        let boot0 = unsafe { core::ptr::read_volatile(bar0_virt as *const u32) };
        if boot0 == 0xFFFF_FFFF || boot0 == 0 {
            gpudbg!("[gpu] PMC_BOOT_0 read 0x{:08X} — GPU not responding (Optimus power-gated? needs ACPI enable)", boot0);
        } else {
            let chipset = (boot0 >> 20) & 0x1ff;
            let arch = match chipset {
                0x108 => "GK208 Kepler (CC 3.5)",
                0x0E7 | 0x0E4 => "GK107 Kepler (CC 3.0)",
                _ => "unknown",
            };
            gpudbg!("[gpu] PMC_BOOT_0=0x{:08X} -> chipset=0x{:03X} ({})", boot0, chipset, arch);
        }
    }

    gpudbg!("[gpu] M18 note: Kepler is PRE-signed-firmware → from-scratch compute is tractable here (unlike Maxwell+).");
    Some(GpuPciInfo { loc, device_id: device, name, bar0_phys, bar0_size, bar1_phys })
}
