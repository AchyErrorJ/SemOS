//! VirtIO Legacy network device driver.
//!
//! Spec: VirtIO 0.9.5 / 1.0 §5.1 (legacy network device). Sibling of
//! [`super::block`] — same handshake, same I/O-port BAR layout, same
//! virtqueue structures. Differences from block:
//!
//! - PCI vendor 0x1AF4 device **0x1000** (vs 0x1001 for block).
//! - **Two** virtqueues: queue 0 = RX, queue 1 = TX (vs single queue).
//! - Device config begins with a 6-byte MAC address (we enable
//!   `VIRTIO_NET_F_MAC` so it's populated).
//! - Each TX/RX buffer is a single descriptor: 10-byte virtio-net
//!   header followed by the Ethernet frame, all in one contiguous
//!   region (we don't negotiate MRG_RXBUF, so the header is 10 bytes,
//!   not 12; we don't negotiate any offload features either).
//!
//! Polled I/O, no interrupts. Matches the existing virtio-block style
//! and is fine for Track B development pace (the brief calls out
//! polling as adequate for v1 across the stack).
//!
//! # Use site
//!
//! `init()` then `register_with_kernel_core()`, just like block. After
//! that the device is reachable through
//! `kernel_core::drivers::registry::get_net("virtio-net0")`.

use core::arch::asm;
use crate::pci;

// ============================================================================
// PCI identifiers and register layout
// ============================================================================

const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
const VIRTIO_NET_DEVICE_ID: u16 = 0x1000;

/// Register offsets relative to BAR0 (legacy I/O ports). Same common
/// header as block; the device-specific config starts at 0x14.
mod reg {
    pub const DEVICE_FEATURES:  u16 = 0x00;
    pub const DRIVER_FEATURES:  u16 = 0x04;
    pub const QUEUE_ADDRESS:    u16 = 0x08;
    pub const QUEUE_SIZE:       u16 = 0x0C;
    pub const QUEUE_SELECT:     u16 = 0x0E;
    pub const QUEUE_NOTIFY:     u16 = 0x10;
    pub const DEVICE_STATUS:    u16 = 0x12;
    pub const _ISR_STATUS:      u16 = 0x13;
    /// MAC address — 6 bytes — only valid if VIRTIO_NET_F_MAC negotiated.
    pub const CONFIG_MAC:       u16 = 0x14;
}

mod status {
    pub const ACK:        u8 = 1 << 0;
    pub const DRIVER:     u8 = 1 << 1;
    pub const DRIVER_OK:  u8 = 1 << 2;
    pub const FAILED:     u8 = 1 << 7;
}

/// Bitmap of feature bits we ask the device to enable. Only one:
/// MAC means the device populates `CONFIG_MAC` with the assigned
/// hardware address. We deliberately skip MRG_RXBUF, CSUM, GSO, etc.
const VIRTIO_NET_F_MAC: u32 = 1 << 5;

// ============================================================================
// I/O port helpers (duplicated from block.rs — small enough to keep local
// to each driver rather than threading a shared helpers module)
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
// Virtqueue layout (identical to block.rs — same legacy ring format)
// ============================================================================

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
    used_event: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqUsedElem {
    id:  u32,
    len: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct VirtqUsed {
    flags: u16,
    idx:   u16,
    ring:  [VirtqUsedElem; QUEUE_SIZE],
    avail_event: u16,
}

mod desc_flag {
    pub const NEXT:  u16 = 1 << 0;
    pub const WRITE: u16 = 1 << 1;
}

#[repr(C, align(4096))]
struct VringStorage([u8; VRING_BYTES]);

/// Backing for queue 0 (RX) and queue 1 (TX). Two independent vrings.
static mut VRING_RX: VringStorage = VringStorage([0; VRING_BYTES]);
static mut VRING_TX: VringStorage = VringStorage([0; VRING_BYTES]);

// ============================================================================
// Packet buffers
// ============================================================================
//
// Each buffer is a single contiguous region:
//   [10 bytes virtio-net header][up to 2038 bytes Ethernet frame]
// The device reads the entire buffer on TX (header + frame) and writes
// it on RX. We don't chain descriptors — one descriptor per buffer.

/// virtio-net header size when MRG_RXBUF is NOT negotiated. (Adds 2 bytes
/// for `num_buffers` when MRG_RXBUF is on — we keep it off.)
pub const VIRTIO_NET_HDR_LEN: usize = 10;

/// Total size of one buffer (header + max Ethernet frame, rounded up).
const BUFFER_SIZE: usize = 2048;

/// Number of RX buffers we pre-post to the device. The device fills
/// them as packets arrive; we re-post each one after we drain it.
const RX_BUFFER_COUNT: usize = 16;

#[repr(C, align(4096))]
struct RxStorage([[u8; BUFFER_SIZE]; RX_BUFFER_COUNT]);

#[repr(C, align(4096))]
struct TxStorage([u8; BUFFER_SIZE]);

static mut RX_BUFFERS: RxStorage = RxStorage([[0; BUFFER_SIZE]; RX_BUFFER_COUNT]);
static mut TX_BUFFER:  TxStorage = TxStorage([0; BUFFER_SIZE]);

// Cached physical addresses of every RX buffer + the single TX buffer.
// Computed once during init_queues.
static mut RX_BUFFER_PHYS: [u64; RX_BUFFER_COUNT] = [0; RX_BUFFER_COUNT];
static mut TX_BUFFER_PHYS: u64 = 0;

// ============================================================================
// Driver state
// ============================================================================

static mut IO_BASE: u16 = 0;
static mut MAC: [u8; 6] = [0; 6];

/// Per-queue vring layout pointers (computed once at init_queues time).
static mut RX_VRING_PHYS: u64 = 0;
static mut TX_VRING_PHYS: u64 = 0;
static mut RX_DESC:  *mut VirtqDesc  = core::ptr::null_mut();
static mut RX_AVAIL: *mut VirtqAvail = core::ptr::null_mut();
static mut RX_USED:  *mut VirtqUsed  = core::ptr::null_mut();
static mut TX_DESC:  *mut VirtqDesc  = core::ptr::null_mut();
static mut TX_AVAIL: *mut VirtqAvail = core::ptr::null_mut();
static mut TX_USED:  *mut VirtqUsed  = core::ptr::null_mut();

/// Last-seen `used.idx` for the RX queue. Increments when the device
/// hands us a completed receive. `(RX_USED.idx - LAST_RX_USED)` is the
/// number of unprocessed receives.
static mut LAST_RX_USED: u16 = 0;

#[inline]
unsafe fn read_status() -> u8 { inb(IO_BASE + reg::DEVICE_STATUS) }
#[inline]
unsafe fn write_status(s: u8) { outb(IO_BASE + reg::DEVICE_STATUS, s) }

// ============================================================================
// Initialisation
// ============================================================================

/// Probe + initialise the virtio-net device.
/// Returns `true` if it was found and brought up to DRIVER_OK.
pub fn init() -> bool {
    let loc = match pci::find_first(VIRTIO_VENDOR_ID, VIRTIO_NET_DEVICE_ID) {
        Some(l) => l,
        None => {
            crate::println!("[virtio-net] no device on PCI bus 0");
            return false;
        }
    };

    loc.enable_io_and_bus_master();

    let bar0 = loc.bar0();
    if bar0 & 1 == 0 {
        crate::println!("[virtio-net] BAR0 is MMIO (0x{:08X}); legacy I/O expected — abort", bar0);
        return false;
    }
    let io_base = (bar0 & 0xFFFC) as u16;
    unsafe { IO_BASE = io_base; }

    // Handshake: reset → ACK → DRIVER.
    unsafe {
        write_status(0);
        write_status(read_status() | status::ACK);
        write_status(read_status() | status::DRIVER);
    }

    // Feature negotiation: enable only MAC.
    let host_features = unsafe { inl(io_base + reg::DEVICE_FEATURES) };
    let negotiated = host_features & VIRTIO_NET_F_MAC;
    unsafe { outl(io_base + reg::DRIVER_FEATURES, negotiated); }

    if negotiated & VIRTIO_NET_F_MAC == 0 {
        crate::println!("[virtio-net] device doesn't offer F_MAC; cannot read MAC — abort");
        unsafe { write_status(read_status() | status::FAILED); }
        return false;
    }

    // Read the 6-byte MAC from the device-specific config region.
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = unsafe { inb(io_base + reg::CONFIG_MAC + i as u16) };
    }
    unsafe { MAC = mac; }

    crate::println!(
        "[virtio-net] PCI 00:{:02X}.0  io_base=0x{:04X}  host_features=0x{:08X}  mac={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        loc.slot, io_base, host_features,
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );

    if !init_queues(io_base) {
        unsafe { write_status(read_status() | status::FAILED); }
        return false;
    }

    unsafe { write_status(read_status() | status::DRIVER_OK); }
    crate::println!("[virtio-net] queues ready (RX phys 0x{:X}, TX phys 0x{:X})",
        unsafe { RX_VRING_PHYS }, unsafe { TX_VRING_PHYS });

    // Pre-post all RX buffers so the device has somewhere to land
    // incoming packets the moment it goes live.
    unsafe { prepost_rx_buffers(); }

    true
}

/// Bring up queue 0 (RX) and queue 1 (TX): point QUEUE_ADDRESS at each
/// vring's physical page-frame and resolve all the per-queue pointers.
fn init_queues(io_base: u16) -> bool {
    // Helper: resolve a 4 KiB-aligned virtual address to its physical
    // address via the active PML4. Returns None if not mapped.
    let resolve = |virt: u64| -> Option<u64> { crate::paging::walk_active_pml4(virt) };

    // Queue 0 — RX
    unsafe { outw(io_base + reg::QUEUE_SELECT, 0); }
    let dev_size_rx = unsafe { inw(io_base + reg::QUEUE_SIZE) };
    if dev_size_rx as usize != QUEUE_SIZE {
        crate::println!("[virtio-net] RX queue size {} != driver {}", dev_size_rx, QUEUE_SIZE);
        return false;
    }
    let rx_vring_virt = unsafe { (&raw const VRING_RX) as u64 };
    let rx_phys = match resolve(rx_vring_virt) {
        Some(p) if p & 0xFFF == 0 => p,
        _ => { crate::println!("[virtio-net] RX vring phys translation failed"); return false; }
    };
    unsafe {
        RX_VRING_PHYS = rx_phys;
        RX_DESC  = (rx_vring_virt + 0)        as *mut VirtqDesc;
        RX_AVAIL = (rx_vring_virt + 4096)     as *mut VirtqAvail;
        RX_USED  = (rx_vring_virt + 4096 * 2) as *mut VirtqUsed;
        outl(io_base + reg::QUEUE_ADDRESS, (rx_phys / 4096) as u32);
    }

    // Queue 1 — TX
    unsafe { outw(io_base + reg::QUEUE_SELECT, 1); }
    let dev_size_tx = unsafe { inw(io_base + reg::QUEUE_SIZE) };
    if dev_size_tx as usize != QUEUE_SIZE {
        crate::println!("[virtio-net] TX queue size {} != driver {}", dev_size_tx, QUEUE_SIZE);
        return false;
    }
    let tx_vring_virt = unsafe { (&raw const VRING_TX) as u64 };
    let tx_phys = match resolve(tx_vring_virt) {
        Some(p) if p & 0xFFF == 0 => p,
        _ => { crate::println!("[virtio-net] TX vring phys translation failed"); return false; }
    };
    unsafe {
        TX_VRING_PHYS = tx_phys;
        TX_DESC  = (tx_vring_virt + 0)        as *mut VirtqDesc;
        TX_AVAIL = (tx_vring_virt + 4096)     as *mut VirtqAvail;
        TX_USED  = (tx_vring_virt + 4096 * 2) as *mut VirtqUsed;
        outl(io_base + reg::QUEUE_ADDRESS, (tx_phys / 4096) as u32);
    }

    // Resolve packet-buffer physical addresses.
    unsafe {
        let rx_base = (&raw const RX_BUFFERS) as u64;
        for i in 0..RX_BUFFER_COUNT {
            let v = rx_base + (i * BUFFER_SIZE) as u64;
            let page_virt = v & !0xFFF;
            let page_off  = v & 0xFFF;
            let phys = match resolve(page_virt) {
                Some(p) => p + page_off,
                None => { crate::println!("[virtio-net] RX buf {} phys translation failed", i); return false; }
            };
            RX_BUFFER_PHYS[i] = phys;
        }
        let tx_v = (&raw const TX_BUFFER) as u64;
        let tx_page = tx_v & !0xFFF;
        let tx_off  = tx_v & 0xFFF;
        TX_BUFFER_PHYS = match resolve(tx_page) {
            Some(p) => p + tx_off,
            None => { crate::println!("[virtio-net] TX buf phys translation failed"); return false; }
        };
    }

    true
}

/// Build descriptor entries for all RX buffers and add them to the RX
/// avail ring so the device has somewhere to land packets.
unsafe fn prepost_rx_buffers() {
    let desc = RX_DESC;
    let avail = RX_AVAIL;
    for i in 0..RX_BUFFER_COUNT {
        (*desc.add(i)).addr  = RX_BUFFER_PHYS[i];
        (*desc.add(i)).len   = BUFFER_SIZE as u32;
        // WRITE = device fills this buffer on RX.
        (*desc.add(i)).flags = desc_flag::WRITE;
        (*desc.add(i)).next  = 0;
        let slot = (*avail).idx as usize % QUEUE_SIZE;
        (*avail).ring[slot] = i as u16;
        // Bump idx after writing the ring entry so the device sees them
        // in a consistent state. fence ensures the descriptor + ring
        // writes are visible before the idx update.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        (*avail).idx = (*avail).idx.wrapping_add(1);
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    // Kick RX queue (queue 0) so the device knows we've added buffers.
    outw(IO_BASE + reg::QUEUE_NOTIFY, 0);
    LAST_RX_USED = 0;
}

// ============================================================================
// Public send / recv
// ============================================================================

/// Send one Ethernet frame. `frame` is the raw Ethernet bytes starting
/// with the destination MAC. Returns `false` on any failure (frame too
/// big, queue not initialised, TX timeout).
pub fn send_frame(frame: &[u8]) -> bool {
    if frame.len() > BUFFER_SIZE - VIRTIO_NET_HDR_LEN { return false; }
    if unsafe { IO_BASE == 0 || TX_DESC.is_null() } { return false; }

    unsafe {
        // Fill the TX buffer: zero header + payload.
        let tx = (&raw mut TX_BUFFER) as *mut u8;
        for i in 0..VIRTIO_NET_HDR_LEN { *tx.add(i) = 0; }
        core::ptr::copy_nonoverlapping(
            frame.as_ptr(),
            tx.add(VIRTIO_NET_HDR_LEN),
            frame.len(),
        );

        // Build a single-descriptor chain at TX desc[0].
        let desc = TX_DESC;
        (*desc).addr  = TX_BUFFER_PHYS;
        (*desc).len   = (VIRTIO_NET_HDR_LEN + frame.len()) as u32;
        (*desc).flags = 0; // device reads
        (*desc).next  = 0;

        let avail = TX_AVAIL;
        let slot = (*avail).idx as usize % QUEUE_SIZE;
        (*avail).ring[slot] = 0;
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        (*avail).idx = (*avail).idx.wrapping_add(1);
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

        // Kick TX queue (index 1).
        outw(IO_BASE + reg::QUEUE_NOTIFY, 1);

        // Poll for completion (TX is fast in QEMU; bound the spin).
        let used = TX_USED;
        let target = (*avail).idx;
        let mut spins: u32 = 0;
        while (*used).idx != target {
            core::hint::spin_loop();
            spins = spins.wrapping_add(1);
            if spins > 100_000_000 {
                crate::println!("[virtio-net] TX timeout");
                return false;
            }
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
    }

    true
}

/// Receive one Ethernet frame into `out` (just the frame, no virtio-net
/// header). Returns `Ok(len)` on success, `Ok(0)` if no frame is ready.
pub fn recv_frame(out: &mut [u8]) -> usize {
    if unsafe { RX_DESC.is_null() } { return 0; }
    unsafe {
        let used = RX_USED;
        if (*used).idx == LAST_RX_USED {
            return 0;
        }
        // Pop the next completed RX descriptor.
        let slot = LAST_RX_USED as usize % QUEUE_SIZE;
        let elem = (*used).ring[slot];
        let desc_idx = elem.id as usize;
        let total_len = elem.len as usize;

        if desc_idx >= RX_BUFFER_COUNT || total_len < VIRTIO_NET_HDR_LEN {
            // Bogus completion — re-post the buffer and skip.
            repost_rx_buffer(desc_idx);
            LAST_RX_USED = LAST_RX_USED.wrapping_add(1);
            return 0;
        }

        let frame_len = total_len - VIRTIO_NET_HDR_LEN;
        let copy_len = frame_len.min(out.len());
        let src = (&raw const RX_BUFFERS) as *const u8;
        let buf_base = src.add(desc_idx * BUFFER_SIZE);
        core::ptr::copy_nonoverlapping(
            buf_base.add(VIRTIO_NET_HDR_LEN),
            out.as_mut_ptr(),
            copy_len,
        );

        // Re-post the buffer so the device can use it again.
        repost_rx_buffer(desc_idx);
        LAST_RX_USED = LAST_RX_USED.wrapping_add(1);

        copy_len
    }
}

/// Re-post a single RX buffer to the RX avail ring after we've drained
/// it. The descriptor (with addr + len + flags) is left as-is — only
/// the avail ring slot + idx need updating.
unsafe fn repost_rx_buffer(desc_idx: usize) {
    let avail = RX_AVAIL;
    let slot = (*avail).idx as usize % QUEUE_SIZE;
    (*avail).ring[slot] = desc_idx as u16;
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    (*avail).idx = (*avail).idx.wrapping_add(1);
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    // Kick RX queue so device knows there's a fresh buffer.
    outw(IO_BASE + reg::QUEUE_NOTIFY, 0);
}

/// Is a frame ready in the RX queue?
pub fn rx_has_data() -> bool {
    if unsafe { RX_USED.is_null() } { return false; }
    unsafe { (*RX_USED).idx != LAST_RX_USED }
}

/// MAC address of the device. All zeros if `init()` hasn't run yet.
pub fn mac_address() -> [u8; 6] { unsafe { MAC } }

// ============================================================================
// NetDevice trait impl + registry registration
// ============================================================================

use kernel_core::drivers::traits::{NetDevice, DriverError, DriverResult};

pub struct VirtioNet;

impl NetDevice for VirtioNet {
    fn send(&self, packet: &[u8]) -> DriverResult<()> {
        if send_frame(packet) { Ok(()) } else { Err(DriverError::IoError) }
    }

    fn recv(&self, buf: &mut [u8]) -> DriverResult<usize> {
        let n = recv_frame(buf);
        if n == 0 {
            // Trait contract: WouldBlock when nothing is ready.
            Err(DriverError::WouldBlock)
        } else {
            Ok(n)
        }
    }

    fn poll(&self) -> bool { rx_has_data() }

    fn mac_address(&self) -> [u8; 6] { mac_address() }

    fn mtu(&self) -> usize {
        // Standard Ethernet MTU. We don't advertise jumbo frames.
        1500
    }

    fn link_up(&self) -> bool {
        // We didn't negotiate VIRTIO_NET_F_STATUS, so we don't have a
        // per-link bit. QEMU's virtio-net is always "up" — say so.
        true
    }

    fn name(&self) -> &'static str { "virtio-net0" }
}

pub static VIRTIO_NET: VirtioNet = VirtioNet;

/// Register with kernel-core's driver registry. Must be called after `init()`.
pub fn register_with_kernel_core() -> bool {
    kernel_core::drivers::registry::register_net("virtio-net0", &VIRTIO_NET)
}
