//! VirtIO entropy device (virtio-rng) — the DEMO 100 agent-authored
//! driver artifact: written by the agent from the virtio spec (§5.5,
//! "Entropy Device"), delivered through the M22a self-rebuild forge
//! (docs/driver-forge-design.md).
//!
//! Legacy-transitional PCI (0x1AF4/0x1005), one request queue. To get
//! entropy: place a device-writable buffer in the queue, notify, poll the
//! used ring, read the bytes. QEMU's virtio-rng-pci sources real entropy
//! from the host (/dev/urandom), so a fetch can be checked for substance.
//!
//! Self-contained by design (its own tiny vring, like block.rs but 16
//! descriptors): the forge's first driver should touch nothing shared.

use core::arch::asm;
use crate::pci;
use crate::println;

const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
const VIRTIO_RNG_DEVICE_ID: u16 = 0x1005;

mod reg {
    pub const DEVICE_FEATURES: u16 = 0x00;
    pub const DRIVER_FEATURES: u16 = 0x04;
    pub const QUEUE_ADDRESS:   u16 = 0x08;
    pub const QUEUE_SIZE:      u16 = 0x0C;
    pub const QUEUE_SELECT:    u16 = 0x0E;
    pub const QUEUE_NOTIFY:    u16 = 0x10;
    pub const DEVICE_STATUS:   u16 = 0x12;
}
mod status {
    pub const ACK:        u8 = 1 << 0;
    pub const DRIVER:     u8 = 1 << 1;
    pub const FEATURES_OK: u8 = 1 << 3;
    pub const DRIVER_OK:  u8 = 1 << 2;
}

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

/// Queue holds 8 descriptors: desc 8*16=128 B + avail 22 B in page 0,
/// used ring (134 B) page-aligned in page 1.
const QUEUE_N: usize = 8;
#[repr(C, align(4096))]
struct Vring([u8; 8192]);
static mut VRING: Vring = Vring([0; 8192]);

/// Entropy staging buffer the device writes into.
const ENT_CAP: usize = 128;
#[repr(C, align(64))]
struct EntBuf([u8; ENT_CAP]);
static mut ENT: EntBuf = EntBuf([0; ENT_CAP]);

static mut IO_BASE: u16 = 0;
static mut READY: bool = false;
/// Avail/used ring cursors across fetches (a fetch must wait for used.idx
/// to pass the PREVIOUS fetch's value, not zero).
static mut AVAIL_IDX: u16 = 0;
static mut LAST_USED: u16 = 0;

fn phys_of(virt: u64) -> u64 {
    crate::paging::walk_active_pml4(virt).unwrap_or(0)
}

/// Probe + init. Returns true when the device is present and the queue is
/// live (FEATURES_OK skipped on the legacy path, like block.rs).
pub fn init() -> bool {
    let loc = match pci::find_first(VIRTIO_VENDOR_ID, VIRTIO_RNG_DEVICE_ID) {
        Some(l) => l,
        None => return false,
    };
    loc.enable_io_and_bus_master();
    let bar0 = loc.bar0();
    if bar0 & 1 == 0 {
        println!("[virtio-rng] BAR0 is MMIO; legacy I/O expected — abort");
        return false;
    }
    let io = (bar0 & 0xFFFC) as u16;
    unsafe {
        IO_BASE = io;
        // Spec handshake: reset, ACK, DRIVER, features=0, DRIVER_OK.
        outb(io + reg::DEVICE_STATUS, 0);
        outb(io + reg::DEVICE_STATUS, inb(io + reg::DEVICE_STATUS) | status::ACK);
        outb(io + reg::DEVICE_STATUS, inb(io + reg::DEVICE_STATUS) | status::DRIVER);
        let _features = inl(io + reg::DEVICE_FEATURES);
        outl(io + reg::DRIVER_FEATURES, 0);
        // NOTE: no FEATURES_OK step — match block.rs's proven legacy
        // handshake exactly (the FEATURES_OK write was one of two
        // differences from the working block path).

        // Queue 0 setup.
        outw(io + reg::QUEUE_SELECT, 0);
        let qsize = inw(io + reg::QUEUE_SIZE);
        if qsize < QUEUE_N as u16 {
            println!("[virtio-rng] queue too small ({}) — abort", qsize);
            return false;
        }
        let vring_phys = phys_of(unsafe { &raw const VRING } as u64);
        if vring_phys == 0 {
            println!("[virtio-rng] vring phys translate failed");
            return false;
        }
        outl(io + reg::QUEUE_ADDRESS, (vring_phys >> 12) as u32);
        outb(io + reg::DEVICE_STATUS, inb(io + reg::DEVICE_STATUS) | status::DRIVER_OK);
        READY = true;
        println!(
            "[virtio-rng] PCI 00:{:02X}.0  io_base=0x{:04X}  queue_size={} — entropy device live",
            loc.slot, io, qsize
        );
    }
    true
}

/// Fetch up to `out.len()` (≤ ENT_CAP) entropy bytes. Returns the byte
/// count (0 on failure/not initialized). Single-descriptor request, polled.
pub fn entropy(out: &mut [u8]) -> usize {
    unsafe {
        if !READY {
            return 0;
        }
        let n = out.len().min(ENT_CAP);
        for b in ENT.0.iter_mut() {
            *b = 0;
        }
        let ent_phys = phys_of(&raw const ENT as u64);
        if ent_phys == 0 {
            return 0;
        }
        let vring = &raw mut VRING as *mut u8;
        // Descriptor 0 at offset 0: addr(u64) len(u32) flags(u16) next(u16).
        (vring as *mut u64).write(ent_phys);
        (vring.add(8) as *mut u32).write(n as u32);
        (vring.add(12) as *mut u16).write(2); // WRITE (bit 1: device writes the buf)
        (vring.add(14) as *mut u16).write(0);
        // Avail ring at offset 16*QUEUE_N: flags(u16) idx(u16) ring[N](u16).
        let avail = vring.add(16 * QUEUE_N);
        (avail as *mut u16).write(0); // flags: no interrupts
        let slot = AVAIL_IDX % QUEUE_N as u16;
        (avail.add(4).add(2 * slot as usize) as *mut u16).write(0); // ring[slot] = desc 0
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        AVAIL_IDX = AVAIL_IDX.wrapping_add(1);
        (avail.add(2) as *mut u16).write(AVAIL_IDX);
        outw(IO_BASE + reg::QUEUE_NOTIFY, 0);

        // Used ring lives in page 1 of the vring (offset 4096). Wait for
        // used.idx to advance PAST the last fetch's value.
        let used = vring.add(4096);
        let used_idx = used.add(2) as *const u16;
        let mut spins = 0u64;
        while core::ptr::read_volatile(used_idx) == LAST_USED {
            spins += 1;
            if spins > 200_000_000 {
                println!("[virtio-rng] fetch timeout (status=0x{:02x})",
                    inb(IO_BASE + reg::DEVICE_STATUS));
                }
            core::hint::spin_loop();
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        LAST_USED = core::ptr::read_volatile(used_idx);
        out[..n].copy_from_slice(&ENT.0[..n]);
        n
    }
}

/// Substance check for the forge's health gate: a 64-byte fetch that is
/// non-empty, non-zero, and non-constant. QEMU's virtio-rng sources host
/// entropy, so anything else means the driver isn't really talking to
/// hardware.
pub fn entropy_ok() -> bool {
    let mut buf = [0u8; 64];
    let n = entropy(&mut buf);
    if n < 64 {
        return false;
    }
    let first = buf[0];
    let non_constant = buf.iter().any(|&b| b != first);
    let non_zero = buf.iter().any(|&b| b != 0);
    non_constant && non_zero
}
