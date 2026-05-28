//! NVMe block-device driver (M9).
//!
//! Brings up an NVM Express controller on the PCI bus (class 0x01 / subclass
//! 0x08 / prog-if 0x02 — vendor-agnostic) and exposes namespace 1 as a
//! `BlockDevice` named `nvme0`, so `storage::snapshot` can use it like the
//! VirtIO disk. QEMU models NVMe (`-device nvme,drive=...`), so this is fully
//! testable here.
//!
//! Scope (v1): one admin queue pair + one I/O queue pair (qid 1), polled
//! completions (no MSI-X/interrupts), Identify Controller + Identify Namespace,
//! and single-block Read/Write via PRP1 (one LBA per command, like the VirtIO
//! driver — the BlockDevice layer loops for multi-block). Queues + buffers are
//! page-aligned BSS (contiguous physical memory, which NVMe DMA requires);
//! their physical addresses come from walking the active page tables.
//!
//! # MMIO register map (relative to BAR0/1, the 64-bit memory BAR)
//! | off    | reg  | meaning                                            |
//! |--------|------|----------------------------------------------------|
//! | 0x00   | CAP  | capabilities (MQES bits15:0, DSTRD bits35:32)      |
//! | 0x08   | VS   | version                                            |
//! | 0x14   | CC   | controller config (EN, IOSQES, IOCQES, MPS, CSS)   |
//! | 0x1C   | CSTS | status (RDY bit0, CFS bit1)                        |
//! | 0x24   | AQA  | admin queue attrs (ACQS bits27:16, ASQS bits11:0)  |
//! | 0x28   | ASQ  | admin submission queue base (phys)                 |
//! | 0x30   | ACQ  | admin completion queue base (phys)                 |
//! | 0x1000 | SQ0TDBL | doorbells start here; stride = 4 << DSTRD       |

use core::ptr::{read_volatile, write_volatile};
use crate::pci;
use crate::paging;
use crate::println;

// --- MMIO register offsets ---
mod reg {
    pub const CAP: u64 = 0x00;
    pub const CC: u64 = 0x14;
    pub const CSTS: u64 = 0x1C;
    pub const AQA: u64 = 0x24;
    pub const ASQ: u64 = 0x28;
    pub const ACQ: u64 = 0x30;
    pub const DOORBELL_BASE: u64 = 0x1000;
}

// CC register fields.
const CC_EN: u32 = 1 << 0;
// IOSQES=6 (2^6=64B), IOCQES=4 (2^4=16B), MPS=0 (4KiB), CSS=0 (NVM command set).
const CC_IOCQES: u32 = 4 << 20;
const CC_IOSQES: u32 = 6 << 16;
const CSTS_RDY: u32 = 1 << 0;

// NVMe opcodes.
const OP_DELETE_SQ: u32 = 0x00;
const OP_CREATE_SQ: u32 = 0x01;
const OP_CREATE_CQ: u32 = 0x05;
const OP_IDENTIFY: u32 = 0x06;
const OP_NVM_WRITE: u32 = 0x01;
const OP_NVM_READ: u32 = 0x02;

const QDEPTH: usize = 8; // entries per queue (admin + I/O)
const SQE_BYTES: usize = 64;
const CQE_BYTES: usize = 16;
const IO_QID: u16 = 1;

// --- DMA-visible storage (page-aligned BSS = contiguous physical pages) ---
#[repr(C, align(4096))]
struct Page([u8; 4096]);

static mut ADMIN_SQ: Page = Page([0; 4096]); // 8 * 64 = 512B used
static mut ADMIN_CQ: Page = Page([0; 4096]); // 8 * 16 = 128B used
static mut IO_SQ: Page = Page([0; 4096]);
static mut IO_CQ: Page = Page([0; 4096]);
static mut IDENT: Page = Page([0; 4096]); // identify result buffer
static mut DATA: Page = Page([0; 4096]); // one-block scratch for read/write

// --- driver state ---
static mut MMIO: u64 = 0; // virtual base of the BAR (via phys_to_virt)
static mut DSTRD: u32 = 0;
static mut LBA_SIZE: usize = 512;
static mut NS_BLOCKS: u64 = 0;

// per-queue ring cursors + expected completion phase
static mut ASQ_TAIL: u16 = 0;
static mut ACQ_HEAD: u16 = 0;
static mut ADMIN_PHASE: u16 = 1;
static mut IOSQ_TAIL: u16 = 0;
static mut IOCQ_HEAD: u16 = 0;
static mut IO_PHASE: u16 = 1;
static mut CID: u16 = 0;

#[inline]
unsafe fn rd32(off: u64) -> u32 {
    read_volatile((MMIO + off) as *const u32)
}
#[inline]
unsafe fn wr32(off: u64, v: u32) {
    write_volatile((MMIO + off) as *mut u32, v);
}
#[inline]
unsafe fn rd64(off: u64) -> u64 {
    let lo = read_volatile((MMIO + off) as *const u32) as u64;
    let hi = read_volatile((MMIO + off + 4) as *const u32) as u64;
    (hi << 32) | lo
}
#[inline]
unsafe fn wr64(off: u64, v: u64) {
    write_volatile((MMIO + off) as *mut u32, v as u32);
    write_volatile((MMIO + off + 4) as *mut u32, (v >> 32) as u32);
}

/// Doorbell offset for submission-queue `y` tail / completion-queue `y` head.
#[inline]
unsafe fn sq_doorbell(y: u16) -> u64 {
    reg::DOORBELL_BASE + (2 * y as u64) * (4 << DSTRD)
}
#[inline]
unsafe fn cq_doorbell(y: u16) -> u64 {
    reg::DOORBELL_BASE + (2 * y as u64 + 1) * (4 << DSTRD)
}

fn phys_of(page: *const Page) -> u64 {
    // Page-aligned BSS: the page's physical frame, walked from the active PML4.
    paging::walk_active_pml4(page as u64).unwrap_or(0)
}

/// Build a 64-byte submission entry from 16 dwords and copy it into `sq[slot]`.
unsafe fn write_sqe(sq: *mut Page, slot: usize, cdw: &[u32; 16]) {
    let base = (sq as *mut u8).add(slot * SQE_BYTES) as *mut u32;
    for (i, &w) in cdw.iter().enumerate() {
        write_volatile(base.add(i), w);
    }
}

/// Read the status (phase bit + status code) of completion entry `slot`.
/// Returns (phase, status_code).
unsafe fn read_cqe_status(cq: *const Page, slot: usize) -> (u16, u16) {
    let dw3 = read_volatile(((cq as *const u8).add(slot * CQE_BYTES + 12)) as *const u32);
    let status_field = (dw3 >> 16) as u16; // bits 31:16
    let phase = status_field & 1;
    let code = status_field >> 1;
    (phase, code)
}

/// Submit one command to the admin (`admin=true`) or I/O queue and poll its
/// completion. Returns the status code (0 = success), or 0xFFFF on timeout.
unsafe fn submit(admin: bool, mut cdw: [u32; 16]) -> u16 {
    CID = CID.wrapping_add(1);
    cdw[0] = (cdw[0] & 0x0000_FFFF) | ((CID as u32) << 16);

    let (sq, cq, tail, head, phase, qid) = if admin {
        (
            &raw mut ADMIN_SQ,
            &raw const ADMIN_CQ,
            &raw mut ASQ_TAIL,
            &raw mut ACQ_HEAD,
            &raw mut ADMIN_PHASE,
            0u16,
        )
    } else {
        (
            &raw mut IO_SQ,
            &raw const IO_CQ,
            &raw mut IOSQ_TAIL,
            &raw mut IOCQ_HEAD,
            &raw mut IO_PHASE,
            IO_QID,
        )
    };

    let slot = *tail as usize;
    write_sqe(sq, slot, &cdw);

    // Advance + ring the SQ tail doorbell.
    *tail = ((*tail + 1) % QDEPTH as u16) as u16;
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    wr32(sq_doorbell(qid), *tail as u32);

    // Poll the completion queue at our current head for the expected phase.
    let chead = *head as usize;
    let mut spins: u64 = 0;
    loop {
        let (p, code) = read_cqe_status(cq, chead);
        if p == *phase {
            // Advance CQ head; flip phase on wrap; ring CQ head doorbell.
            let nh = (*head + 1) % QDEPTH as u16;
            if nh == 0 {
                *phase ^= 1;
            }
            *head = nh;
            wr32(cq_doorbell(qid), *head as u32);
            return code;
        }
        spins += 1;
        if spins > 200_000_000 {
            println!("[nvme] completion timeout (admin={})", admin);
            return 0xFFFF;
        }
        core::hint::spin_loop();
    }
}

/// Probe + bring up the controller. Returns true if `nvme0` (namespace 1) is
/// ready for I/O.
pub fn init() -> bool {
    let loc = match pci::find_by_class(0x01, 0x08, 0x02) {
        Some(l) => l,
        None => {
            println!("[nvme] no NVMe controller on PCI bus 0");
            return false;
        }
    };
    loc.enable_io_and_bus_master();

    let phys_base = match pci::mmio_bar64(loc) {
        Some(b) => b,
        None => {
            println!("[nvme] BAR0 is I/O space; NVMe must be MMIO — abort");
            return false;
        }
    };
    unsafe { MMIO = paging::phys_to_virt(phys_base); }

    let cap = unsafe { rd64(reg::CAP) };
    let mqes = (cap & 0xFFFF) as u32 + 1; // max queue entries
    unsafe { DSTRD = ((cap >> 32) & 0xF) as u32; }
    println!(
        "[nvme] PCI 00:{:02X}.0  MMIO=0x{:016X}  MQES={}  DSTRD={}",
        loc.slot, phys_base, mqes, unsafe { DSTRD }
    );
    if (QDEPTH as u32) > mqes {
        println!("[nvme] device max queue {} < our depth {} — abort", mqes, QDEPTH);
        return false;
    }

    unsafe {
        // 1. Disable the controller and wait for CSTS.RDY = 0.
        let cc = rd32(reg::CC);
        wr32(reg::CC, cc & !CC_EN);
        let mut spins = 0u64;
        while rd32(reg::CSTS) & CSTS_RDY != 0 {
            spins += 1;
            if spins > 200_000_000 {
                println!("[nvme] controller stuck ready during reset");
                return false;
            }
            core::hint::spin_loop();
        }

        // 2. Program admin queue attributes + base addresses.
        let asq_phys = phys_of(&raw const ADMIN_SQ);
        let acq_phys = phys_of(&raw const ADMIN_CQ);
        if asq_phys == 0 || acq_phys == 0 {
            println!("[nvme] failed to translate admin queues to phys");
            return false;
        }
        let aqa = (((QDEPTH as u32 - 1) & 0xFFF) << 16) | ((QDEPTH as u32 - 1) & 0xFFF);
        wr32(reg::AQA, aqa);
        wr64(reg::ASQ, asq_phys);
        wr64(reg::ACQ, acq_phys);

        // 3. Enable: NVM command set, 4KiB pages, 64B SQ / 16B CQ entries.
        wr32(reg::CC, CC_IOCQES | CC_IOSQES | CC_EN);

        // 4. Wait for CSTS.RDY = 1.
        spins = 0;
        while rd32(reg::CSTS) & CSTS_RDY == 0 {
            spins += 1;
            if spins > 200_000_000 {
                println!("[nvme] controller never became ready (CSTS=0x{:08X})", rd32(reg::CSTS));
                return false;
            }
            core::hint::spin_loop();
        }
    }

    // 5. Identify namespace 1 → block size + block count.
    if !identify_namespace() {
        return false;
    }

    // 6. Create the I/O completion + submission queue pair (qid 1).
    if !create_io_queues() {
        return false;
    }

    println!(
        "[nvme] nvme0 ready: namespace 1 = {} blocks of {} B ({} MiB)",
        unsafe { NS_BLOCKS },
        unsafe { LBA_SIZE },
        unsafe { NS_BLOCKS * LBA_SIZE as u64 } / (1024 * 1024),
    );
    true
}

/// Identify namespace 1 and pull NSZE (block count) + the active LBA format's
/// block size out of the 4 KiB result.
fn identify_namespace() -> bool {
    unsafe {
        let ident_phys = phys_of(&raw const IDENT);
        // Zero the result buffer.
        for b in IDENT.0.iter_mut() {
            *b = 0;
        }
        let mut cdw = [0u32; 16];
        cdw[0] = OP_IDENTIFY;
        cdw[1] = 1; // nsid = 1
        cdw[6] = ident_phys as u32; // PRP1 low
        cdw[7] = (ident_phys >> 32) as u32; // PRP1 high
        cdw[10] = 0; // CNS = 0 → Identify Namespace
        let code = submit(true, cdw);
        if code != 0 {
            println!("[nvme] Identify Namespace failed (status 0x{:X})", code);
            return false;
        }

        let buf = &IDENT.0;
        // NSZE: u64 at offset 0.
        let nsze = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        // FLBAS: byte 26, low 4 bits = current LBA format index.
        let flbas = (buf[26] & 0x0F) as usize;
        // LBAF entries: 4 bytes each starting at offset 128. LBADS = byte 2.
        let lbads = buf[128 + flbas * 4 + 2] as u32;
        let lba_size = 1usize << lbads;
        NS_BLOCKS = nsze;
        LBA_SIZE = if lba_size >= 512 && lba_size <= 4096 { lba_size } else { 512 };
        true
    }
}

/// Create the I/O completion queue (qid 1) then the I/O submission queue.
/// The CQ must exist before the SQ that targets it.
fn create_io_queues() -> bool {
    unsafe {
        let iocq_phys = phys_of(&raw const IO_CQ);
        let iosq_phys = phys_of(&raw const IO_SQ);
        if iocq_phys == 0 || iosq_phys == 0 {
            println!("[nvme] failed to translate I/O queues to phys");
            return false;
        }
        let qsm1 = (QDEPTH as u32 - 1) << 16;

        // Create I/O Completion Queue: PRP1 = CQ phys, CDW10 = (size-1)<<16|qid,
        // CDW11 = PC(bit0)=1, interrupts disabled (we poll).
        let mut cdw = [0u32; 16];
        cdw[0] = OP_CREATE_CQ;
        cdw[6] = iocq_phys as u32;
        cdw[7] = (iocq_phys >> 32) as u32;
        cdw[10] = qsm1 | IO_QID as u32;
        cdw[11] = 1; // PC = 1
        if submit(true, cdw) != 0 {
            println!("[nvme] Create I/O CQ failed");
            return false;
        }

        // Create I/O Submission Queue: PRP1 = SQ phys, CDW10 = (size-1)<<16|qid,
        // CDW11 = PC(bit0)=1 | (cqid<<16).
        let mut cdw = [0u32; 16];
        cdw[0] = OP_CREATE_SQ;
        cdw[6] = iosq_phys as u32;
        cdw[7] = (iosq_phys >> 32) as u32;
        cdw[10] = qsm1 | IO_QID as u32;
        cdw[11] = 1 | ((IO_QID as u32) << 16);
        if submit(true, cdw) != 0 {
            println!("[nvme] Create I/O SQ failed");
            return false;
        }
        true
    }
}

/// Read/write one LBA via the I/O queue. `write` selects the direction; data
/// moves through the page-aligned DATA scratch (PRP1). Returns success.
unsafe fn rw_one(lba: u64, write: bool) -> bool {
    let data_phys = phys_of(&raw const DATA);
    let mut cdw = [0u32; 16];
    cdw[0] = if write { OP_NVM_WRITE } else { OP_NVM_READ };
    cdw[1] = 1; // nsid
    cdw[6] = data_phys as u32;
    cdw[7] = (data_phys >> 32) as u32;
    cdw[10] = lba as u32; // SLBA low
    cdw[11] = (lba >> 32) as u32; // SLBA high
    cdw[12] = 0; // NLB = 0 → 1 block (0-based)
    submit(false, cdw) == 0
}

// ============================================================================
// BlockDevice impl + registry
// ============================================================================

use kernel_core::drivers::traits::{BlockDevice, DriverError, DriverResult};

pub struct Nvme;

impl BlockDevice for Nvme {
    fn read_blocks(&self, block: u64, buf: &mut [u8]) -> DriverResult<()> {
        let bs = unsafe { LBA_SIZE };
        if buf.len() < bs {
            return Err(DriverError::BufferTooSmall);
        }
        let n = buf.len() / bs;
        for i in 0..n {
            unsafe {
                if !rw_one(block + i as u64, false) {
                    return Err(DriverError::IoError);
                }
                let off = i * bs;
                core::ptr::copy_nonoverlapping(DATA.0.as_ptr(), buf[off..].as_mut_ptr(), bs);
            }
        }
        Ok(())
    }

    fn write_blocks(&self, block: u64, buf: &[u8]) -> DriverResult<()> {
        let bs = unsafe { LBA_SIZE };
        if buf.len() < bs {
            return Err(DriverError::BufferTooSmall);
        }
        let n = buf.len() / bs;
        for i in 0..n {
            unsafe {
                let off = i * bs;
                core::ptr::copy_nonoverlapping(buf[off..].as_ptr(), DATA.0.as_mut_ptr(), bs);
                if !rw_one(block + i as u64, true) {
                    return Err(DriverError::IoError);
                }
            }
        }
        Ok(())
    }

    fn block_size(&self) -> usize {
        unsafe { LBA_SIZE }
    }
    fn block_count(&self) -> u64 {
        unsafe { NS_BLOCKS }
    }
    fn name(&self) -> &'static str {
        "nvme0"
    }
}

pub static NVME: Nvme = Nvme;

/// Register `nvme0` with kernel-core's driver registry (after `init`).
pub fn register_with_kernel_core() -> bool {
    kernel_core::drivers::registry::register_block("nvme0", &NVME)
}

/// Block count of namespace 1 (valid after `init`).
pub fn block_count() -> u64 {
    unsafe { NS_BLOCKS }
}

// Silence unused-const warnings for opcodes kept for completeness.
const _: u32 = OP_DELETE_SQ;
