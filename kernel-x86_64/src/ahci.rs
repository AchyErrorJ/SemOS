//! AHCI/SATA block-device driver (M9' — the path the T540's internal SSD
//! needs, since the T540 chassis is SATA-only).
//!
//! Brings up an AHCI controller on the PCI bus (class 0x01 / subclass 0x06 /
//! prog-if 0x01, vendor-agnostic), enables AHCI mode, picks the first port
//! reporting a SATA device, programs the per-port command list + FIS receive
//! area, runs ATA Identify Device to learn block count, and exposes the
//! device as the `sata0` BlockDevice — usable like virtio0 or nvme0. QEMU's
//! `-device ich9-ahci` + `-device ide-hd,drive=...,bus=ahci.0` is the test
//! path; real T540 ports the same code unchanged.
//!
//! Scope v1: one port, command slot 0 only, polled (no MSI-X), single-LBA
//! READ/WRITE DMA EXT (BlockDevice layer loops for multi-block). PRDT has
//! one entry pointing at a page-aligned scratch buffer. BOH/legacy handoff
//! skipped (QEMU ich9-ahci advertises no BOH; real hardware adds a small
//! CAP2.BOH dance — that's a follow-up when the T540 lands).
//!
//! # MMIO register map (relative to ABAR / BAR5)
//!
//! Global:
//! | off    | reg  | meaning                                            |
//! |--------|------|----------------------------------------------------|
//! | 0x00   | CAP  | host capabilities                                  |
//! | 0x04   | GHC  | global host control (HR bit0, IE bit1, AE bit31)   |
//! | 0x0C   | PI   | ports implemented (32-bit bitmap)                  |
//!
//! Per-port (offset 0x100 + 0x80*port):
//! | off  | reg  | meaning                                              |
//! |------|------|------------------------------------------------------|
//! | 0x00 | CLB  | command list base (low 32 / high 32 at +4)           |
//! | 0x08 | FB   | FIS receive base (low 32 / high 32 at +4)            |
//! | 0x10 | IS   | port interrupt status (write-1-to-clear)             |
//! | 0x18 | CMD  | command/status (ST bit0, FRE bit4, FR bit14, CR bit15)|
//! | 0x20 | TFD  | task file data (low byte = ATA status)               |
//! | 0x24 | SIG  | device signature (0x00000101 = ATA)                  |
//! | 0x28 | SSTS | SATA status (DET bits 3:0; 3 = comm established)     |
//! | 0x30 | SERR | error                                                |
//! | 0x38 | CI   | command issue (one bit per slot; set to start, HBA   |
//! |      |      | clears on completion)                                |

use core::ptr::{read_volatile, write_volatile};
use crate::pci;
use crate::paging;
use crate::println;

mod reg {
    pub const CAP: u64 = 0x00;
    pub const GHC: u64 = 0x04;
    pub const PI: u64 = 0x0C;
}
mod pr {
    pub const CLB: u64 = 0x00;
    pub const FB: u64 = 0x08;
    pub const IS: u64 = 0x10;
    pub const CMD: u64 = 0x18;
    pub const TFD: u64 = 0x20;
    pub const SIG: u64 = 0x24;
    pub const SSTS: u64 = 0x28;
    pub const SERR: u64 = 0x30;
    pub const CI: u64 = 0x38;
}

const GHC_HR: u32 = 1 << 0;
const GHC_AE: u32 = 1u32 << 31;

const CMD_ST: u32 = 1 << 0;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

const SIG_ATA: u32 = 0x0000_0101;

const TFD_BSY: u32 = 1 << 7;
const TFD_DRQ: u32 = 1 << 3;
const TFD_ERR: u32 = 1 << 0;

// ATA commands.
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_IDENTIFY: u8 = 0xEC;

// FIS types.
const FIS_TYPE_REG_H2D: u8 = 0x27;

const SECTOR_SIZE: usize = 512;

// --- DMA-visible static storage (page-aligned BSS = contiguous physical) ---
#[repr(C, align(1024))]
struct CommandList([u8; 1024]); // 32 headers × 32 bytes
#[repr(C, align(256))]
struct FisRxArea([u8; 256]);
#[repr(C, align(128))]
struct CommandTable([u8; 256]); // CFIS(64) + ACMD(16) + reserved(48) + PRDT entry(16) + slack
#[repr(C, align(4096))]
struct Page([u8; 4096]);

static mut CLIST: CommandList = CommandList([0; 1024]);
static mut FISRX: FisRxArea = FisRxArea([0; 256]);
static mut CTAB: CommandTable = CommandTable([0; 256]);
static mut DATA: Page = Page([0; 4096]);
static mut IDENT: Page = Page([0; 4096]);

// --- driver state ---
static mut MMIO: u64 = 0;
static mut PORT: u32 = 0xFFFF_FFFF; // chosen port, or sentinel if none
static mut LBA_COUNT: u64 = 0;

#[inline]
unsafe fn rd32(off: u64) -> u32 {
    read_volatile((MMIO + off) as *const u32)
}
#[inline]
unsafe fn wr32(off: u64, v: u32) {
    write_volatile((MMIO + off) as *mut u32, v);
}

/// Per-port register offset.
#[inline]
fn port_base(port: u32) -> u64 {
    0x100 + 0x80 * port as u64
}

fn phys_of_page(p: *const Page) -> u64 {
    paging::walk_active_pml4(p as u64).unwrap_or(0)
}
fn phys_of_clist(p: *const CommandList) -> u64 {
    paging::walk_active_pml4(p as u64).unwrap_or(0)
}
fn phys_of_fisrx(p: *const FisRxArea) -> u64 {
    paging::walk_active_pml4(p as u64).unwrap_or(0)
}
fn phys_of_ctab(p: *const CommandTable) -> u64 {
    paging::walk_active_pml4(p as u64).unwrap_or(0)
}

/// Wait while a per-port register has bits set. Returns true on success,
/// false on timeout.
unsafe fn wait_clear(off: u64, mask: u32) -> bool {
    let mut spins: u64 = 0;
    while rd32(off) & mask != 0 {
        spins += 1;
        if spins > 200_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Build the command header at slot 0 of CLIST pointing at CTAB. `write` =
/// data direction (H2D = write to disk), `prdtl` = PRDT entry count.
unsafe fn write_cmd_header(write: bool, cfl_dwords: u8, prdtl: u16) {
    let hdr = (&raw mut CLIST.0[0]) as *mut u32;
    // DW0: PRDTL(31:16) | flags | CFL(4:0). W bit (6) = 1 for H2D writes.
    let w_bit: u32 = if write { 1 << 6 } else { 0 };
    let dw0: u32 = ((prdtl as u32) << 16) | w_bit | (cfl_dwords as u32 & 0x1F);
    write_volatile(hdr, dw0);
    write_volatile(hdr.add(1), 0); // PRDBC = 0 (HBA updates this)
    let ctab_phys = phys_of_ctab(&raw const CTAB);
    write_volatile(hdr.add(2), ctab_phys as u32);
    write_volatile(hdr.add(3), (ctab_phys >> 32) as u32);
    // DW4..7 reserved.
    for i in 4..8 {
        write_volatile(hdr.add(i), 0);
    }
}

/// Build the PRDT entry 0 of CTAB pointing at `phys` with `bytes`. AHCI
/// requires (bytes - 1) in the count field (0-based) and even byte count.
unsafe fn write_prdt0(phys: u64, bytes: u32) {
    let prdt = ((&raw mut CTAB.0[0x80]) as *mut u8) as *mut u32; // entry 0
    write_volatile(prdt, phys as u32);
    write_volatile(prdt.add(1), (phys >> 32) as u32);
    write_volatile(prdt.add(2), 0);
    // DBC (bits 21:0): byte count - 1. Bit 31 = Interrupt on Completion (off).
    write_volatile(prdt.add(3), bytes.saturating_sub(1) & 0x003F_FFFF);
}

/// Build a Register H2D FIS for an LBA48 transfer (or Identify when `lba`+
/// `count` are 0). Zeroes the CFIS area first.
unsafe fn write_cfis(cmd: u8, lba: u64, count: u16) {
    // Clear the whole command table front (CFIS + ACMD + reserved).
    for b in &mut CTAB.0[..0x80] {
        *b = 0;
    }
    let f = (&raw mut CTAB.0[0]) as *mut u8;
    write_volatile(f, FIS_TYPE_REG_H2D);
    write_volatile(f.add(1), 1 << 7); // C bit = 1 (command, not control)
    write_volatile(f.add(2), cmd);
    // Features 7:0
    write_volatile(f.add(3), 0);
    // LBA 0:23 in bytes 4-6
    write_volatile(f.add(4), (lba & 0xFF) as u8);
    write_volatile(f.add(5), ((lba >> 8) & 0xFF) as u8);
    write_volatile(f.add(6), ((lba >> 16) & 0xFF) as u8);
    // Device = LBA mode
    write_volatile(f.add(7), 1 << 6);
    // LBA 24:47 in bytes 8-10
    write_volatile(f.add(8), ((lba >> 24) & 0xFF) as u8);
    write_volatile(f.add(9), ((lba >> 32) & 0xFF) as u8);
    write_volatile(f.add(10), ((lba >> 40) & 0xFF) as u8);
    // Features 15:8 = 0
    write_volatile(f.add(11), 0);
    // Count 7:0, 15:8
    write_volatile(f.add(12), (count & 0xFF) as u8);
    write_volatile(f.add(13), ((count >> 8) & 0xFF) as u8);
    // ICC, Control, reserved
    write_volatile(f.add(14), 0);
    write_volatile(f.add(15), 0);
}

/// Issue command slot 0 on the active port and poll CI until it clears.
unsafe fn issue_and_wait() -> bool {
    let base = port_base(PORT);
    // Clear stale port IS bits.
    wr32(base + pr::IS, !0u32);
    // Wait for BSY/DRQ to clear.
    if !wait_clear(base + pr::TFD, TFD_BSY | TFD_DRQ) {
        println!("[ahci] timeout waiting for TFD before issue");
        return false;
    }
    // Kick command slot 0.
    wr32(base + pr::CI, 1);
    // Poll CI until cleared.
    let mut spins: u64 = 0;
    while rd32(base + pr::CI) & 1 != 0 {
        spins += 1;
        if spins > 200_000_000 {
            println!("[ahci] CI poll timeout (TFD=0x{:08X}, SERR=0x{:08X})",
                rd32(base + pr::TFD), rd32(base + pr::SERR));
            return false;
        }
        // Check for error.
        if rd32(base + pr::IS) & (1u32 << 30) != 0 {
            println!("[ahci] task file error during command (TFD=0x{:08X})",
                rd32(base + pr::TFD));
            return false;
        }
        core::hint::spin_loop();
    }
    if rd32(base + pr::TFD) & TFD_ERR != 0 {
        println!("[ahci] command finished with ERR (TFD=0x{:08X})", rd32(base + pr::TFD));
        return false;
    }
    true
}

/// Bring up an AHCI controller + one port. Returns true if `sata0` is ready.
pub fn init() -> bool {
    let loc = match pci::find_by_class(0x01, 0x06, 0x01) {
        Some(l) => l,
        None => {
            println!("[ahci] no AHCI controller on PCI bus 0");
            return false;
        }
    };
    loc.enable_io_and_bus_master();

    // ABAR is BAR5 (PCI config offset 0x24). For AHCI it's almost always a
    // 32-bit MMIO BAR — read it directly and mask the type bits.
    let bar5 = pci::read_u32(loc.bus, loc.slot, loc.func, 0x24);
    if bar5 & 1 != 0 {
        println!("[ahci] BAR5 is I/O space (0x{:08X}); AHCI must be MMIO", bar5);
        return false;
    }
    let phys_base = (bar5 as u64) & 0xFFFF_FFF0;
    unsafe { MMIO = paging::phys_to_virt(phys_base); }

    unsafe {
        // 1. Enable AHCI mode. We deliberately do NOT issue an HBA reset —
        //    HR severs the SATA PHY links and the controller doesn't always
        //    re-establish them without a follow-up SCTL.DET cycle (QEMU's
        //    ich9-ahci is one such; would also bite on real BIOSes that
        //    handed off to us with the link already up). Real-hardware
        //    follow-up: optional HR + per-port SCTL.DET=1 → wait → DET=0
        //    → poll SSTS.DET=3, alongside the BIOS/OS handoff (CAP2.BOH).
        wr32(reg::GHC, rd32(reg::GHC) | GHC_AE);

        let cap = rd32(reg::CAP);
        let pi = rd32(reg::PI);
        let np = ((cap & 0x1F) + 1) as u32; // NP = number of ports - 1
        println!("[ahci] PCI 00:{:02X}.0  MMIO=0x{:016X}  CAP=0x{:08X}  PI=0x{:08X}  NP={}",
            loc.slot, phys_base, cap, pi, np);

        // 2. Find the first implemented port with an ATA SATA device.
        //    HR cycled the link state, so SSTS.DET=3 ("comm established") may
        //    take a moment to come back. Poll each implemented port briefly
        //    before declaring it empty.
        let mut chosen: u32 = 0xFFFF_FFFF;
        for p in 0..32 {
            if pi & (1 << p) == 0 {
                continue;
            }
            let pb = port_base(p);
            // Poll DET briefly per port. Empty ports usually read DET=0
            // immediately; a slow link-up sits at DET=1 ("device present,
            // comm not yet established") and shouldn't take long. Use a
            // short budget so an HBA with many empty ports doesn't add
            // seconds to boot under TCG.
            let mut spins: u64 = 0;
            let mut det = 0u32;
            loop {
                det = rd32(pb + pr::SSTS) & 0xF;
                if det == 3 {
                    break;
                }
                // No device → don't burn the full budget waiting for it.
                if det == 0 && spins > 200_000 {
                    break;
                }
                spins += 1;
                if spins > 2_000_000 {
                    break;
                }
                core::hint::spin_loop();
            }
            if det != 3 {
                continue;
            }
            let sig = rd32(pb + pr::SIG);
            if sig == SIG_ATA {
                chosen = p;
                println!("[ahci] port {} SSTS=0x{:08X} SIG=0x{:08X} → ATA SATA",
                    p, rd32(pb + pr::SSTS), sig);
                break;
            }
        }
        if chosen == 0xFFFF_FFFF {
            println!("[ahci] no ATA device on any implemented port");
            return false;
        }
        PORT = chosen;

        // 3. Stop the port: clear ST + FRE, wait for CR + FR to drop, then
        //    point CLB / FB at our DMA structures and turn FRE / ST back on.
        let pb = port_base(PORT);
        let cmd = rd32(pb + pr::CMD);
        wr32(pb + pr::CMD, cmd & !(CMD_ST | CMD_FRE));
        if !wait_clear(pb + pr::CMD, CMD_CR | CMD_FR) {
            println!("[ahci] port {} stuck running after stop", PORT);
            return false;
        }
        let cl = phys_of_clist(&raw const CLIST);
        let fb = phys_of_fisrx(&raw const FISRX);
        if cl == 0 || fb == 0 {
            println!("[ahci] failed to translate CL/FB to phys");
            return false;
        }
        wr32(pb + pr::CLB, cl as u32);
        wr32(pb + pr::CLB + 4, (cl >> 32) as u32);
        wr32(pb + pr::FB, fb as u32);
        wr32(pb + pr::FB + 4, (fb >> 32) as u32);
        // Clear stale errors / interrupts.
        wr32(pb + pr::SERR, !0u32);
        wr32(pb + pr::IS, !0u32);
        // Re-enable FIS receive then command engine.
        wr32(pb + pr::CMD, (rd32(pb + pr::CMD) | CMD_FRE) | CMD_ST);
    }

    // 4. Identify the device → block count.
    if !unsafe { identify_device() } {
        return false;
    }

    println!(
        "[ahci] sata0 ready: port {}, {} blocks of {} B ({} MiB)",
        unsafe { PORT },
        unsafe { LBA_COUNT },
        SECTOR_SIZE,
        unsafe { LBA_COUNT * SECTOR_SIZE as u64 } / (1024 * 1024),
    );
    true
}

unsafe fn identify_device() -> bool {
    // Zero the result page.
    for b in IDENT.0.iter_mut() {
        *b = 0;
    }
    let ident_phys = phys_of_page(&raw const IDENT);
    write_cmd_header(false, 5, 1); // CFL = 5 DWORDs, 1 PRDT entry, read direction
    write_prdt0(ident_phys, 512);
    write_cfis(ATA_CMD_IDENTIFY, 0, 0);
    if !issue_and_wait() {
        println!("[ahci] Identify Device failed");
        return false;
    }
    // Words 100-103 = 64-bit total LBAs (LBA48). Fall back to 60-61 (32-bit
    // LBA28) if the LBA48 field looks zeroed.
    let w = |i: usize| -> u16 {
        u16::from_le_bytes([IDENT.0[i * 2], IDENT.0[i * 2 + 1]])
    };
    let lba48 = (w(100) as u64)
        | ((w(101) as u64) << 16)
        | ((w(102) as u64) << 32)
        | ((w(103) as u64) << 48);
    let lba28 = (w(60) as u64) | ((w(61) as u64) << 16);
    LBA_COUNT = if lba48 > 0 { lba48 } else { lba28 };
    LBA_COUNT > 0
}

unsafe fn rw_one(lba: u64, write: bool) -> bool {
    let data_phys = phys_of_page(&raw const DATA);
    write_cmd_header(write, 5, 1);
    write_prdt0(data_phys, SECTOR_SIZE as u32);
    let cmd = if write { ATA_CMD_WRITE_DMA_EXT } else { ATA_CMD_READ_DMA_EXT };
    write_cfis(cmd, lba, 1);
    issue_and_wait()
}

// ============================================================================
// BlockDevice impl + registry
// ============================================================================

use kernel_core::drivers::traits::{BlockDevice, DriverError, DriverResult};

pub struct Sata;

impl BlockDevice for Sata {
    fn read_blocks(&self, block: u64, buf: &mut [u8]) -> DriverResult<()> {
        if buf.len() < SECTOR_SIZE {
            return Err(DriverError::BufferTooSmall);
        }
        let n = buf.len() / SECTOR_SIZE;
        for i in 0..n {
            unsafe {
                if !rw_one(block + i as u64, false) {
                    return Err(DriverError::IoError);
                }
                let off = i * SECTOR_SIZE;
                core::ptr::copy_nonoverlapping(DATA.0.as_ptr(), buf[off..].as_mut_ptr(), SECTOR_SIZE);
            }
        }
        Ok(())
    }

    fn write_blocks(&self, block: u64, buf: &[u8]) -> DriverResult<()> {
        if buf.len() < SECTOR_SIZE {
            return Err(DriverError::BufferTooSmall);
        }
        let n = buf.len() / SECTOR_SIZE;
        for i in 0..n {
            unsafe {
                let off = i * SECTOR_SIZE;
                core::ptr::copy_nonoverlapping(buf[off..].as_ptr(), DATA.0.as_mut_ptr(), SECTOR_SIZE);
                if !rw_one(block + i as u64, true) {
                    return Err(DriverError::IoError);
                }
            }
        }
        Ok(())
    }

    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }
    fn block_count(&self) -> u64 {
        unsafe { LBA_COUNT }
    }
    fn name(&self) -> &'static str {
        "sata0"
    }
}

pub static SATA: Sata = Sata;

pub fn register_with_kernel_core() -> bool {
    kernel_core::drivers::registry::register_block("sata0", &SATA)
}
