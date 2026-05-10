//! VirtIO Legacy block device driver.
//!
//! Spec: VirtIO 0.9.5 / 1.0 §4.1.4 (legacy interface).
//! Probes PCI vendor 0x1AF4 device 0x1001, reads BAR0 (an I/O port
//! range), runs the device-init handshake, sets up one virtqueue, and
//! services read/write requests via descriptor chains.
//!
//! # Register layout (BAR0, I/O ports relative to BAR0 base)
//!
//! Common header:
//! | offset | size | name              | direction |
//! |--------|------|-------------------|-----------|
//! | 0x00   | u32  | device features   | RO (host) |
//! | 0x04   | u32  | driver features   | RW        |
//! | 0x08   | u32  | queue address (PFN, phys/4096) | RW |
//! | 0x0C   | u16  | queue size        | RO        |
//! | 0x0E   | u16  | queue select      | RW        |
//! | 0x10   | u16  | queue notify      | RW (write to kick) |
//! | 0x12   | u8   | device status     | RW        |
//! | 0x13   | u8   | ISR status        | RC (read clears) |
//!
//! Block-device-specific config begins at offset 0x14 (when MSI-X is
//! disabled, which is the default):
//!   u64 capacity (in 512-byte sectors)
//!
//! # Status register bits
//!
//! | bit | name        | meaning                          |
//! |-----|-------------|----------------------------------|
//! | 0   | ACK         | guest acknowledges device        |
//! | 1   | DRIVER      | guest knows how to drive it      |
//! | 2   | DRIVER_OK   | guest is fully set up            |
//! | 7   | FAILED      | guest gave up                    |

use core::arch::asm;
use crate::pci;

/// Vendor / device IDs we accept.
const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
const VIRTIO_BLOCK_DEVICE_ID: u16 = 0x1001;

/// Register offsets relative to BAR0.
mod reg {
    pub const DEVICE_FEATURES:  u16 = 0x00;
    pub const DRIVER_FEATURES:  u16 = 0x04;
    pub const QUEUE_ADDRESS:    u16 = 0x08; // PFN (phys/4096)
    pub const QUEUE_SIZE:       u16 = 0x0C;
    pub const QUEUE_SELECT:     u16 = 0x0E;
    pub const QUEUE_NOTIFY:     u16 = 0x10;
    pub const DEVICE_STATUS:    u16 = 0x12;
    pub const _ISR_STATUS:      u16 = 0x13;
    pub const CONFIG_CAPACITY:  u16 = 0x14; // u64 — only valid when MSI-X disabled
}

/// Status bits.
mod status {
    pub const ACK:        u8 = 1 << 0;
    pub const DRIVER:     u8 = 1 << 1;
    pub const DRIVER_OK:  u8 = 1 << 2;
    pub const FAILED:     u8 = 1 << 7;
}

// ============================================================================
// I/O port helpers (BAR0 is an I/O range on legacy VirtIO)
// ============================================================================

#[inline]
unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}
#[inline]
unsafe fn outw(port: u16, value: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
}
#[inline]
unsafe fn outl(port: u16, value: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
}
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}
#[inline]
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}
#[inline]
unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}

// ============================================================================
// Virtqueue layout
// ============================================================================
//
// Legacy VirtIO ring for queue size N:
//   offset 0                       : desc[N]   (16 bytes each)
//   offset 16*N                    : avail     (6 + 2*N bytes)
//   next 4-byte boundary           : (padding to next page)
//   next page boundary             : used      (6 + 8*N bytes)
//
// We pin queue size at QUEUE_SIZE = 256 (QEMU's default) and allocate
// 3 contiguous, page-aligned pages of BSS:
//   page 0: desc table (256 * 16 = 4096 bytes)
//   page 1: avail ring (518 bytes) + padding
//   page 2: used ring  (2054 bytes)
//
// `static mut` in BSS is allocated as contiguous physical memory by the
// bootloader, which is what VirtIO DMA needs.

const QUEUE_SIZE: usize = 256;
const VRING_BYTES: usize = 4096 * 3;

#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqDesc {
    addr:  u64,
    len:   u32,
    flags: u16,
    next:  u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqAvail {
    flags:  u16,
    idx:    u16,
    ring:   [u16; QUEUE_SIZE],
    used_event: u16, // used only when EVENT_IDX feature negotiated
}

#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqUsedElem {
    id:  u32, // descriptor head index
    len: u32, // bytes written
}

#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqUsed {
    flags: u16,
    idx:   u16,
    ring:  [VirtqUsedElem; QUEUE_SIZE],
    avail_event: u16,
}

/// Descriptor flags.
mod desc_flag {
    pub const NEXT:  u16 = 1 << 0; // descriptor chained via .next
    pub const WRITE: u16 = 1 << 1; // device writes (vs reads) this buffer
}

/// Backing storage for the entire virtqueue. Aligned to a page so the
/// device can use the lowest-page-number-of-base format that legacy
/// expects (QUEUE_ADDRESS = phys / 4096).
#[repr(C, align(4096))]
struct VringStorage([u8; VRING_BYTES]);

static mut VRING: VringStorage = VringStorage([0; VRING_BYTES]);

// ============================================================================
// Block-request structures (fixed scratch buffers used for every request)
// ============================================================================

const SECTOR_SIZE: usize = 512;

/// VirtIO block-request header. The device reads it to learn the op type
/// and target sector. Zero copies of any user data — that's in the data
/// descriptor.
#[repr(C)]
#[derive(Copy, Clone)]
struct BlkReqHeader {
    type_: u32,    // 0 = read, 1 = write
    reserved: u32,
    sector: u64,
}

const VIRTIO_BLK_T_IN:  u32 = 0; // read
const VIRTIO_BLK_T_OUT: u32 = 1; // write

const VIRTIO_BLK_S_OK:    u8 = 0;
const _VIRTIO_BLK_S_IOERR: u8 = 1;
const _VIRTIO_BLK_S_UNSUPP: u8 = 2;

/// Aligned scratch space for one in-flight request:
///   - header  (struct BlkReqHeader)
///   - data    (one sector — the user's buffer is copied into this so we
///              don't depend on the user buffer's physical contiguity)
///   - status  (one byte, written by the device)
#[repr(C, align(4096))]
struct ReqScratch {
    header: BlkReqHeader,
    _pad1:  [u8; 4096 - core::mem::size_of::<BlkReqHeader>()],
    data:   [u8; SECTOR_SIZE],
    _pad2:  [u8; 4096 - SECTOR_SIZE],
    status: u8,
    _pad3:  [u8; 4095],
}

static mut REQ: ReqScratch = ReqScratch {
    header: BlkReqHeader { type_: 0, reserved: 0, sector: 0 },
    _pad1: [0; 4096 - core::mem::size_of::<BlkReqHeader>()],
    data:  [0; SECTOR_SIZE],
    _pad2: [0; 4096 - SECTOR_SIZE],
    status: 0,
    _pad3: [0; 4095],
};

/// Cached physical addresses of the three sub-buffers in REQ. Computed
/// once during `init_queue` after the page tables are stable.
static mut HEADER_PHYS: u64 = 0;
static mut DATA_PHYS:   u64 = 0;
static mut STATUS_PHYS: u64 = 0;

// ============================================================================
// Driver state
// ============================================================================

/// I/O base port read from BAR0 (lower 16 bits, with bit 0 = "this is an I/O BAR" mask).
static mut IO_BASE: u16 = 0;
/// Block count (sectors of 512 bytes), read from device config.
static mut CAPACITY_SECTORS: u64 = 0;
/// Physical base of the virtqueue (page-aligned).
static mut VRING_PHYS: u64 = 0;
/// Pointers into the vring at queue setup time. Computed once.
static mut DESC_VIRT:  *mut VirtqDesc  = core::ptr::null_mut();
static mut AVAIL_VIRT: *mut VirtqAvail = core::ptr::null_mut();
static mut USED_VIRT:  *mut VirtqUsed  = core::ptr::null_mut();
/// Free-descriptor counter so the driver doesn't overrun the table.
/// We use a simple "next index" allocator; descriptors return to the
/// pool when the device finishes with them.
static mut NEXT_DESC_IDX: u16 = 0;
/// Last seen used.idx — increments when device finishes a chain.
static mut LAST_USED_IDX: u16 = 0;

#[inline]
unsafe fn read_status() -> u8 { inb(IO_BASE + reg::DEVICE_STATUS) }
#[inline]
unsafe fn write_status(s: u8) { outb(IO_BASE + reg::DEVICE_STATUS, s) }

/// Probe + initialize. Returns true if a device was found and brought up
/// to the DRIVER stage of the status handshake. `init_queue()` is the
/// next step; not done here yet.
pub fn init() -> bool {
    let loc = match pci::find_first(VIRTIO_VENDOR_ID, VIRTIO_BLOCK_DEVICE_ID) {
        Some(l) => l,
        None => {
            crate::println!("[virtio-blk] no device on PCI bus 0");
            return false;
        }
    };

    // Enable I/O space + bus mastering so BAR0 ports respond and the
    // device can DMA later.
    loc.enable_io_and_bus_master();

    // BAR0 lower bit set => this is an I/O BAR. Strip the type bits.
    let bar0 = loc.bar0();
    if bar0 & 1 == 0 {
        crate::println!("[virtio-blk] BAR0 is MMIO (0x{:08X}); legacy I/O expected — abort", bar0);
        return false;
    }
    let io_base = (bar0 & 0xFFFC) as u16;
    unsafe { IO_BASE = io_base; }

    // Spec-mandated init handshake.
    // 1. Reset the device by writing 0 to status.
    unsafe { write_status(0); }
    // 2. ACK that we found it.
    unsafe { write_status(read_status() | status::ACK); }
    // 3. Tell the device we know how to drive it.
    unsafe { write_status(read_status() | status::DRIVER); }

    // Read the host's offered features. We don't enable any of them
    // (driver_features=0) — the legacy block protocol works without
    // negotiating optional features.
    let host_features = unsafe { inl(io_base + reg::DEVICE_FEATURES) };
    unsafe { outl(io_base + reg::DRIVER_FEATURES, 0); }

    // Block-device config: capacity in sectors.
    let cap_lo = unsafe { inl(io_base + reg::CONFIG_CAPACITY) } as u64;
    let cap_hi = unsafe { inl(io_base + reg::CONFIG_CAPACITY + 4) } as u64;
    let capacity = (cap_hi << 32) | cap_lo;
    unsafe { CAPACITY_SECTORS = capacity; }

    crate::println!(
        "[virtio-blk] PCI 00:{:02X}.0  io_base=0x{:04X}  host_features=0x{:08X}  capacity={} sectors ({} KiB)",
        loc.slot, io_base, host_features, capacity, capacity / 2,
    );

    if !init_queue(io_base) {
        unsafe { write_status(read_status() | status::FAILED); }
        return false;
    }

    // Final handshake: tell the device we're ready.
    unsafe { write_status(read_status() | status::DRIVER_OK); }
    crate::println!("[virtio-blk] queue 0 ready ({} descriptors, ring at phys 0x{:X})",
        QUEUE_SIZE, unsafe { VRING_PHYS });

    true
}

/// Set up queue 0: select it, verify the device's queue size matches
/// our compile-time `QUEUE_SIZE`, locate the vring's physical address,
/// and program QUEUE_ADDRESS as a page-frame-number.
fn init_queue(io_base: u16) -> bool {
    // Select queue 0.
    unsafe { outw(io_base + reg::QUEUE_SELECT, 0); }

    // Read the device's queue size — must match what we allocated.
    let dev_size = unsafe { inw(io_base + reg::QUEUE_SIZE) };
    if dev_size as usize != QUEUE_SIZE {
        crate::println!(
            "[virtio-blk] device queue size {} != driver QUEUE_SIZE {} — abort",
            dev_size, QUEUE_SIZE
        );
        return false;
    }

    // Translate VRING's virtual address to a physical address. VRING
    // is in BSS at a high virtual address; the bootloader's
    // physical-memory map doesn't cover it, so we walk page tables.
    let vring_virt = unsafe {
        let p = &raw const VRING;
        p as u64
    };
    let phys = match crate::paging::walk_active_pml4(vring_virt) {
        Some(p) => p,
        None => {
            crate::println!("[virtio-blk] failed to translate vring virt 0x{:X} to phys", vring_virt);
            return false;
        }
    };
    if phys & 0xFFF != 0 {
        crate::println!("[virtio-blk] vring phys 0x{:X} not page-aligned", phys);
        return false;
    }

    // Compute pointers. Layout: desc on page 0, avail on page 1
    // (header at start), used on page 2 (header at start).
    unsafe {
        VRING_PHYS  = phys;
        let base_virt = vring_virt;
        DESC_VIRT  = (base_virt + 0)              as *mut VirtqDesc;
        AVAIL_VIRT = (base_virt + 4096)           as *mut VirtqAvail;
        USED_VIRT  = (base_virt + 4096 * 2)       as *mut VirtqUsed;

        // Tell the device where the queue lives (PFN of desc table).
        outl(io_base + reg::QUEUE_ADDRESS, (phys / 4096) as u32);
    }

    // Cache the physical addresses of REQ's three sub-buffers — the
    // descriptor chain references each one by phys addr.
    unsafe {
        let req_base = (&raw const REQ) as u64;
        let header_virt = req_base + memoffset_header() as u64;
        let data_virt   = req_base + memoffset_data()   as u64;
        let status_virt = req_base + memoffset_status() as u64;
        let resolve = |v: u64, page_offset: u64| -> Option<u64> {
            let page_virt = v & !0xFFF;
            crate::paging::walk_active_pml4(page_virt).map(|p| p + page_offset)
        };
        HEADER_PHYS = resolve(header_virt, header_virt & 0xFFF).unwrap_or(0);
        DATA_PHYS   = resolve(data_virt,   data_virt   & 0xFFF).unwrap_or(0);
        STATUS_PHYS = resolve(status_virt, status_virt & 0xFFF).unwrap_or(0);
        if HEADER_PHYS == 0 || DATA_PHYS == 0 || STATUS_PHYS == 0 {
            crate::println!("[virtio-blk] failed to translate REQ buffers to phys");
            return false;
        }
    }

    true
}

// Compile-time-friendly offsets within REQ (manual because offset_of! is
// nightly-volatile and the layout is repr(C) with explicit padding).
const fn memoffset_header() -> usize { 0 }
const fn memoffset_data() -> usize { 4096 }
const fn memoffset_status() -> usize { 4096 * 2 }

// ============================================================================
// I/O: read_sector / write_sector
// ============================================================================

/// Read one 512-byte sector into `buf[..512]`. Synchronous: blocks
/// until the device finishes (polls `used.idx`). Returns false on
/// device-reported error or driver-state error.
pub fn read_sector(sector: u64, buf: &mut [u8]) -> bool {
    if buf.len() < SECTOR_SIZE { return false; }
    if !submit_and_wait(VIRTIO_BLK_T_IN, sector) { return false; }
    unsafe {
        let req_base = &raw const REQ as *const u8;
        let data = req_base.add(memoffset_data());
        core::ptr::copy_nonoverlapping(data, buf.as_mut_ptr(), SECTOR_SIZE);
    }
    true
}

/// Write one 512-byte sector from `buf[..512]`. Synchronous.
pub fn write_sector(sector: u64, buf: &[u8]) -> bool {
    if buf.len() < SECTOR_SIZE { return false; }
    unsafe {
        let req_base = &raw mut REQ as *mut u8;
        let data = req_base.add(memoffset_data());
        core::ptr::copy_nonoverlapping(buf.as_ptr(), data, SECTOR_SIZE);
    }
    submit_and_wait(VIRTIO_BLK_T_OUT, sector)
}

/// Build a 3-descriptor chain (header / data / status), submit it, and
/// poll for completion. Always uses descriptors 0, 1, 2 — only one
/// request at a time.
fn submit_and_wait(req_type: u32, sector: u64) -> bool {
    if unsafe { IO_BASE == 0 || DESC_VIRT.is_null() } {
        return false;
    }
    let io_base = unsafe { IO_BASE };

    unsafe {
        // Fill the request header.
        REQ.header.type_ = req_type;
        REQ.header.reserved = 0;
        REQ.header.sector = sector;
        REQ.status = 0xFF; // sentinel — will be 0/1/2 after device finishes

        // Build the descriptor chain at indexes 0, 1, 2.
        let desc = DESC_VIRT;
        // 0: header — read-only by device, chains to 1
        (*desc.add(0)).addr  = HEADER_PHYS;
        (*desc.add(0)).len   = core::mem::size_of::<BlkReqHeader>() as u32;
        (*desc.add(0)).flags = desc_flag::NEXT;
        (*desc.add(0)).next  = 1;
        // 1: data — direction depends on op; chains to 2
        let data_flags = if req_type == VIRTIO_BLK_T_IN {
            desc_flag::NEXT | desc_flag::WRITE  // device writes (read op)
        } else {
            desc_flag::NEXT                       // device reads (write op)
        };
        (*desc.add(1)).addr  = DATA_PHYS;
        (*desc.add(1)).len   = SECTOR_SIZE as u32;
        (*desc.add(1)).flags = data_flags;
        (*desc.add(1)).next  = 2;
        // 2: status — write-only by device, end of chain
        (*desc.add(2)).addr  = STATUS_PHYS;
        (*desc.add(2)).len   = 1;
        (*desc.add(2)).flags = desc_flag::WRITE;
        (*desc.add(2)).next  = 0;

        // Add chain head (idx 0) to the avail ring at the next slot.
        let avail = AVAIL_VIRT;
        let slot = (*avail).idx as usize % QUEUE_SIZE;
        (*avail).ring[slot] = 0;
        // Memory barrier so the device sees ring[slot] before idx update.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        (*avail).idx = (*avail).idx.wrapping_add(1);
        // Another barrier before the notify write.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

        // Kick: write queue index 0 to the notify register.
        outw(io_base + reg::QUEUE_NOTIFY, 0);

        // Poll for completion. used.idx changes when the device finishes
        // a chain. Bound the wait so a misbehaving/missing device doesn't
        // hang the kernel forever.
        let used = USED_VIRT;
        let target = (*avail).idx;
        let mut spins: u32 = 0;
        while (*used).idx != target {
            core::hint::spin_loop();
            spins = spins.wrapping_add(1);
            if spins > 100_000_000 {
                crate::println!("[virtio-blk] timeout waiting for device (sector {})", sector);
                return false;
            }
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

        REQ.status == VIRTIO_BLK_S_OK
    }
}

/// Total capacity in 512-byte sectors. Valid only after `init()`.
pub fn capacity_sectors() -> u64 {
    unsafe { CAPACITY_SECTORS }
}

// ============================================================================
// BlockDevice trait impl + registry registration
// ============================================================================

use kernel_core::drivers::traits::{BlockDevice, DriverError, DriverResult};

pub struct VirtioBlock;

impl BlockDevice for VirtioBlock {
    fn read_blocks(&self, block: u64, buf: &mut [u8]) -> DriverResult<()> {
        if buf.len() < SECTOR_SIZE {
            return Err(DriverError::BufferTooSmall);
        }
        let n = buf.len() / SECTOR_SIZE;
        for i in 0..n {
            let off = i * SECTOR_SIZE;
            if !read_sector(block + i as u64, &mut buf[off..off + SECTOR_SIZE]) {
                return Err(DriverError::IoError);
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
            let off = i * SECTOR_SIZE;
            if !write_sector(block + i as u64, &buf[off..off + SECTOR_SIZE]) {
                return Err(DriverError::IoError);
            }
        }
        Ok(())
    }

    fn block_size(&self) -> usize { SECTOR_SIZE }
    fn block_count(&self) -> u64 { unsafe { CAPACITY_SECTORS } }
    fn name(&self) -> &'static str { "virtio0" }
}

/// Singleton instance — VirtioBlock has no state (state is in the
/// module's `static mut`s), so a unit struct is enough.
pub static VIRTIO_BLOCK: VirtioBlock = VirtioBlock;

/// Register the block device with kernel-core's driver registry. Must
/// be called after `init()` so the device is fully set up.
pub fn register_with_kernel_core() -> bool {
    kernel_core::drivers::registry::register_block("virtio0", &VIRTIO_BLOCK)
}
