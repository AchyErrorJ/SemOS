//! M27 DEMO 80 — read-only sysroot blob served from a SATA/AHCI disk (Layer B).
//!
//! The host packer (`tools/pack-sysroot-blob.py`) writes a sector-aligned blob to
//! a raw disk image: a header sector (magic `SEMSYSR1`, a file count, then
//! `(name, lba, len)` records) followed by each file's bytes at its LBA. This
//! module reads LBA 0 of the first block device at boot; if the magic matches it
//! caches the file table. `SYS_SYSROOT_INFO` / `SYS_SYSROOT_READ` then let
//! `semos-rustc` stream a file's bytes (e.g. the host-built `libcore-*.rmeta`,
//! ~57 MB) from disk without holding it in kernel RAM — see the c3-selftest.
//!
//! Format (little-endian, 512-byte sectors):
//! ```text
//! sector 0:  +0  magic  b"SEMSYSR1"
//!            +8  count  u32
//!            +12 _resv  u32
//!            +16 records[count] (80 B each): name[64] NUL-pad, lba u64, len u64
//! ```

use crate::drivers::registry;

const SECTOR: usize = 512;
const MAGIC: &[u8; 8] = b"SEMSYSR1";
const NAME_LEN: usize = 64;
const REC_LEN: usize = 80;
const MAX_FILES: usize = 6; // (512 - 16) / 80

#[derive(Clone, Copy)]
struct SysrootFile {
    name: [u8; NAME_LEN],
    name_len: usize,
    lba: u64,
    len: u64,
}

struct SysrootTable {
    files: [SysrootFile; MAX_FILES],
    count: usize,
}

static mut SYSROOT: Option<SysrootTable> = None;

/// Scratch for sector-aligned reads. Single-threaded syscall model (mirrors
/// ahci.rs's static DATA scratch); `read()` loops over this for any buffer size.
const SCRATCH_SECTORS: usize = 64; // 32 KiB
static mut SCRATCH: [u8; SCRATCH_SECTORS * SECTOR] = [0u8; SCRATCH_SECTORS * SECTOR];

fn block_dev() -> Option<&'static dyn crate::drivers::traits::BlockDevice> {
    registry::get_block("sata0").or_else(registry::get_first_block)
}

/// Probe the first block device for a sysroot blob (call once at boot, after the
/// block driver is registered). No-op if no device, no blob, or a bad header.
pub fn probe() {
    let dev = match block_dev() {
        Some(d) => d,
        None => return,
    };
    let mut hdr = [0u8; SECTOR];
    if dev.read_blocks(0, &mut hdr).is_err() {
        return;
    }
    if &hdr[0..8] != MAGIC {
        return;
    }
    let count = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    if count == 0 || count > MAX_FILES {
        return;
    }
    let mut files = [SysrootFile { name: [0; NAME_LEN], name_len: 0, lba: 0, len: 0 }; MAX_FILES];
    let mut off = 16;
    for f in files.iter_mut().take(count) {
        f.name.copy_from_slice(&hdr[off..off + NAME_LEN]);
        f.name_len = f.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        f.lba = u64::from_le_bytes(hdr[off + 64..off + 72].try_into().unwrap());
        f.len = u64::from_le_bytes(hdr[off + 72..off + 80].try_into().unwrap());
        off += REC_LEN;
    }
    crate::platform::log("[sysroot] blob found: ");
    crate::platform::log_num(count as u64);
    crate::platform::log(" file(s)\n");
    for f in files.iter().take(count) {
        if let Ok(name) = core::str::from_utf8(&f.name[..f.name_len]) {
            crate::platform::log("[sysroot]   ");
            crate::platform::log(name);
            crate::platform::log(" (lba ");
            crate::platform::log_num(f.lba);
            crate::platform::log(", ");
            crate::platform::log_num(f.len);
            crate::platform::log(" bytes)\n");
        }
    }
    // SAFETY: single-threaded boot init; written once before any syscall reads it.
    unsafe {
        SYSROOT = Some(SysrootTable { files, count });
    }
}

/// Number of staged sysroot files (0 if no blob).
pub fn count() -> usize {
    // SAFETY: read-only access to a table written once at boot.
    unsafe { (*core::ptr::addr_of!(SYSROOT)).as_ref().map(|s| s.count).unwrap_or(0) }
}

/// Find a staged file by exact name (e.g. "libcore-<hash>.rmeta"); returns its
/// blob index, or `None`.
pub fn find(name: &str) -> Option<usize> {
    // SAFETY: read-only access to a table written once at boot.
    let table = unsafe { (*core::ptr::addr_of!(SYSROOT)).as_ref()? };
    (0..table.count).find(|&i| {
        let f = &table.files[i];
        core::str::from_utf8(&f.name[..f.name_len]).map(|n| n == name).unwrap_or(false)
    })
}

/// Byte length of file `idx`, or `None` if out of range / no blob.
pub fn file_len(idx: usize) -> Option<u64> {
    // SAFETY: read-only access to a table written once at boot.
    let table = unsafe { (*core::ptr::addr_of!(SYSROOT)).as_ref()? };
    if idx >= table.count {
        return None;
    }
    Some(table.files[idx].len)
}

/// Write file `idx`'s name into `out_name`; return its byte length, or `None`
/// if `idx` is out of range / no blob.
pub fn info(idx: usize, out_name: &mut [u8]) -> Option<u64> {
    // SAFETY: read-only access to a table written once at boot.
    let table = unsafe { (*core::ptr::addr_of!(SYSROOT)).as_ref()? };
    if idx >= table.count {
        return None;
    }
    let f = &table.files[idx];
    let n = f.name_len.min(out_name.len());
    out_name[..n].copy_from_slice(&f.name[..n]);
    Some(f.len)
}

/// Read up to `buf.len()` bytes of file `idx` starting at byte `offset`,
/// streaming sectors from disk. Returns bytes read (0 = EOF) or `None` on error.
pub fn read(idx: usize, offset: u64, buf: &mut [u8]) -> Option<usize> {
    // SAFETY: read-only access to a table written once at boot.
    let table = unsafe { (*core::ptr::addr_of!(SYSROOT)).as_ref()? };
    if idx >= table.count {
        return None;
    }
    let (file_lba, file_len) = {
        let f = &table.files[idx];
        (f.lba, f.len)
    };
    if offset >= file_len {
        return Some(0);
    }
    let want = ((file_len - offset) as usize).min(buf.len());
    let dev = block_dev()?;

    let mut produced = 0usize;
    while produced < want {
        // Disk byte position of the next unread chunk.
        let pos = file_lba * SECTOR as u64 + offset + produced as u64;
        let first_lba = pos / SECTOR as u64;
        let sub = (pos % SECTOR as u64) as usize;
        let remaining = want - produced;
        // How many sectors of scratch we can fill this pass.
        let max_bytes = SCRATCH_SECTORS * SECTOR - sub;
        let chunk = remaining.min(max_bytes);
        let nsect = (sub + chunk).div_ceil(SECTOR);
        // SAFETY: single-threaded; nsect <= SCRATCH_SECTORS. Build the slice
        // straight from the raw pointer to avoid the dangerous-implicit-autoref
        // lint that `(*addr_of_mut!(SCRATCH))[..]` trips.
        let scratch = unsafe {
            core::slice::from_raw_parts_mut(
                core::ptr::addr_of_mut!(SCRATCH) as *mut u8,
                nsect * SECTOR,
            )
        };
        if dev.read_blocks(first_lba, scratch).is_err() {
            return if produced > 0 { Some(produced) } else { None };
        }
        buf[produced..produced + chunk].copy_from_slice(&scratch[sub..sub + chunk]);
        produced += chunk;
    }
    Some(produced)
}
