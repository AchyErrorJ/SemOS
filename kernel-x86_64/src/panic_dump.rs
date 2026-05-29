//! Persist a panic snapshot to disk for post-mortem from another OS.
//!
//! On metal-without-serial, a panic that scrolls off the framebuffer is
//! invisible — by the time you can read the screen, the relevant context
//! may already be gone. This module writes the entire 64 KB scrollback ring
//! plus a 384-byte panic-message buffer to the **last 130 sectors of the
//! attached block device**, so you can boot Windows on the same machine,
//! open the disk in HxD (or use `dd` for Windows), seek to the end, and
//! find a `PANICLOG` magic followed by the kernel's last words.
//!
//! Tries `sata0` first (T540 path), then `nvme0` (P1 / QEMU), then
//! `virtio0` (QEMU). Best-effort — never panics itself; if no device
//! is registered or the write fails, the existing framebuffer print is
//! still the user's last resort.
//!
//! # On-disk layout (at the end of the disk)
//!
//! ```text
//!   block_count - 130   PanicHeader (one sector, 412 used of 512)
//!   block_count - 129   scrollback bytes 0..511    (chronological order)
//!   block_count - 128   scrollback bytes 512..1023
//!   ...
//!   block_count -  2    scrollback bytes 65024..65535
//!   block_count -  1    (slack — reserved)
//! ```
//!
//! `PanicHeader` (little-endian):
//! ```text
//!   [0..8]    magic = b"PANICLOG"
//!   [8..12]   version = 1u32
//!   [12..20]  tick at panic (kernel ticks since boot, 100 Hz)
//!   [20..24]  scrollback_len: u32 (bytes valid in the 128 sectors below)
//!   [24..28]  reason_len: u32 (bytes valid in reason[])
//!   [28..412] reason[384] — formatted panic message (truncated if needed)
//! ```

use core::ptr::read_volatile;

pub const PANIC_MAGIC: &[u8; 8] = b"PANICLOG";
pub const PANIC_VERSION: u32 = 1;
const REASON_MAX: usize = 384;
const SCROLLBACK_BYTES: usize = 64 * 1024;
const SCROLLBACK_SECTORS: u64 = (SCROLLBACK_BYTES / 512) as u64; // 128
const TOTAL_DUMP_SECTORS: u64 = 1 /*header*/ + SCROLLBACK_SECTORS + 1 /*slack*/;
const HEADER_USED_BYTES: usize = 8 + 4 + 8 + 4 + 4 + REASON_MAX;

const _: () = assert!(HEADER_USED_BYTES <= 512, "PanicHeader must fit in one sector");

/// Tiny `core::fmt::Write` adapter that writes into a fixed buffer. Used by
/// the panic handler to format `PanicInfo` without touching the heap.
pub struct BufWriter<'a> {
    pub buf: &'a mut [u8],
    pub n: usize,
}

impl<'a> core::fmt::Write for BufWriter<'a> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = self.buf.len() - self.n;
        let take = s.len().min(room);
        self.buf[self.n..self.n + take].copy_from_slice(&s.as_bytes()[..take]);
        self.n += take;
        Ok(())
    }
}

/// Best-effort: write a panic snapshot to the end of the first available
/// block device. Returns `Some(device_name)` if a write was attempted (success
/// or failure logged via println), or `None` if no device was found.
pub fn dump(reason: &[u8]) -> Option<&'static str> {
    use kernel_core::drivers::registry;

    let candidates: [&'static str; 3] = ["sata0", "nvme0", "virtio0"];
    for name in candidates {
        if let Some(dev) = registry::get_block(name) {
            if write_to(dev, name, reason) {
                return Some(name);
            }
        }
    }
    None
}

fn write_to(
    dev: &dyn kernel_core::drivers::traits::BlockDevice,
    name: &'static str,
    reason: &[u8],
) -> bool {
    let bs = dev.block_size();
    if bs != 512 {
        return false; // v1 supports 512-byte sectors only
    }
    let blocks = dev.block_count();
    if blocks < TOTAL_DUMP_SECTORS + 1 {
        return false; // not enough room for the dump area
    }
    let start_lba = blocks - TOTAL_DUMP_SECTORS;

    // --- Header sector ---
    let mut hdr = [0u8; 512];
    hdr[0..8].copy_from_slice(PANIC_MAGIC);
    hdr[8..12].copy_from_slice(&PANIC_VERSION.to_le_bytes());
    let tick = kernel_core::platform::ticks();
    hdr[12..20].copy_from_slice(&tick.to_le_bytes());
    let scrollback_len = read_scrollback_total();
    hdr[20..24].copy_from_slice(&(scrollback_len as u32).to_le_bytes());
    let reason_len = reason.len().min(REASON_MAX);
    hdr[24..28].copy_from_slice(&(reason_len as u32).to_le_bytes());
    hdr[28..28 + reason_len].copy_from_slice(&reason[..reason_len]);

    if dev.write_blocks(start_lba, &hdr).is_err() {
        crate::println!("[panic-dump] {}: header write failed", name);
        return false;
    }

    // --- Scrollback (128 sectors, oldest → newest) ---
    for i in 0..SCROLLBACK_SECTORS {
        let mut sec = [0u8; 512];
        copy_scrollback_window(i as usize * 512, &mut sec);
        if dev.write_blocks(start_lba + 1 + i, &sec).is_err() {
            crate::println!("[panic-dump] {}: scrollback sector {} failed", name, i);
            return false;
        }
    }
    // Optional fsync — best-effort.
    let _ = dev.flush();
    crate::println!("[panic-dump] {} {} sectors written at LBA {}..{}; magic \"PANICLOG\"",
        name, TOTAL_DUMP_SECTORS, start_lba, start_lba + TOTAL_DUMP_SECTORS - 1);
    true
}

/// Total bytes ever written to the scrollback ring (capped at the ring size).
fn read_scrollback_total() -> usize {
    let head = crate::framebuffer::scrollback_head();
    (head as usize).min(SCROLLBACK_BYTES)
}

/// Copy a 512-byte window from the scrollback ring at offset `off` from the
/// oldest recorded byte, into `out`. Bytes past the end of recorded data are
/// zero-filled.
fn copy_scrollback_window(off: usize, out: &mut [u8; 512]) {
    let head = crate::framebuffer::scrollback_head();
    let recorded = (head as usize).min(SCROLLBACK_BYTES);
    if off >= recorded {
        return; // out stays zero
    }
    let oldest = head.wrapping_sub(recorded as u64);
    let take = (recorded - off).min(512);
    let buf_ptr = crate::framebuffer::scrollback_buf_ptr();
    for i in 0..take {
        let pos = ((oldest + (off as u64) + i as u64) as usize) & (SCROLLBACK_BYTES - 1);
        out[i] = unsafe { read_volatile(buf_ptr.add(pos)) };
    }
}
