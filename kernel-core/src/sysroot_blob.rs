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

/// GPT partition name the blob lives in (UTF-16LE in the entry). Create this
/// partition in Linux (`gdisk` / `parted name`) so the blob can share an SSD
/// with the OS without overwriting its partition table. If the disk has no GPT
/// at all we fall back to legacy raw-whole-disk mode (blob at absolute LBA 0).
const TARGET_PART_NAME: &str = "SEMOS_SYSROOT";

/// Absolute LBA where the blob's sector 0 lives. Set by [`probe`]/[`flash_from_usb`]
/// from [`resolve_base_lba`]; 0 = legacy whole-disk mode.
static mut BASE_LBA: u64 = 0;

/// Resolve where the sysroot blob should live on `dev`:
/// - **No GPT** (LBA 1 not `EFI PART`): `Some(0)` — legacy raw whole-disk mode.
/// - **GPT present, `SEMOS_SYSROOT` partition found**: `Some(first_lba)`.
/// - **GPT present, partition absent**: `None` — the disk is partitioned (likely
///   an OS disk); refuse to touch LBA 0 and clobber its GPT.
fn resolve_base_lba(dev: &dyn crate::drivers::traits::BlockDevice) -> Option<u64> {
    let mut hdr = [0u8; SECTOR];
    if dev.read_blocks(1, &mut hdr).is_err() {
        // Can't read the GPT header sector; assume an unpartitioned scratch disk.
        return Some(0);
    }
    if &hdr[0..8] != b"EFI PART" {
        return Some(0); // no GPT → legacy whole-disk blob at LBA 0
    }
    let entries_lba = u64::from_le_bytes(hdr[72..80].try_into().unwrap());
    let num_entries = u32::from_le_bytes(hdr[80..84].try_into().unwrap()) as usize;
    let entry_size = u32::from_le_bytes(hdr[84..88].try_into().unwrap()) as usize;
    if entry_size == 0 || entry_size > SECTOR {
        return None; // malformed; don't risk a raw write
    }
    let per_sector = SECTOR / entry_size;
    let mut sec = [0u8; SECTOR];
    let mut scanned = 0usize;
    let mut lba = entries_lba;
    while scanned < num_entries {
        if dev.read_blocks(lba, &mut sec).is_err() {
            break;
        }
        for i in 0..per_sector {
            if scanned >= num_entries {
                break;
            }
            scanned += 1;
            let e = &sec[i * entry_size..i * entry_size + entry_size];
            // All-zero type GUID = unused entry.
            if e[0..16].iter().all(|&b| b == 0) {
                continue;
            }
            let first_lba = u64::from_le_bytes(e[32..40].try_into().unwrap());
            // Name: 72 bytes UTF-16LE at offset 56, NUL-padded.
            if part_name_matches(&e[56..128.min(entry_size)], TARGET_PART_NAME) {
                return Some(first_lba);
            }
        }
        lba += 1;
    }
    None // GPT present but no SEMOS_SYSROOT partition — refuse raw LBA 0
}

/// Compare a GPT partition name (UTF-16LE, NUL-padded) against an ASCII target.
fn part_name_matches(raw: &[u8], target: &str) -> bool {
    let mut tb = target.bytes();
    let mut chunks = raw.chunks_exact(2);
    loop {
        match (chunks.next(), tb.next()) {
            (Some(c), Some(t)) => {
                let code = u16::from_le_bytes([c[0], c[1]]);
                if code != t as u16 {
                    return false;
                }
            }
            // Target consumed: the rest of the name must be NUL padding.
            (Some(c), None) => {
                if u16::from_le_bytes([c[0], c[1]]) != 0 {
                    return false;
                }
            }
            (None, Some(_)) => return false, // name shorter than target
            (None, None) => return true,
        }
    }
}

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
        None => {
            crate::platform::log("[sysroot] no block device registered\n");
            return;
        }
    };
    let base = match resolve_base_lba(dev) {
        Some(b) => b,
        None => {
            crate::platform::log("[sysroot] disk is GPT-partitioned but has no SEMOS_SYSROOT partition; not probing LBA 0\n");
            return;
        }
    };
    // SAFETY: single-threaded boot init; written once before any syscall reads it.
    unsafe { BASE_LBA = base; }
    if base != 0 {
        crate::platform::log("[sysroot] using SEMOS_SYSROOT partition at LBA ");
        crate::platform::log_num(base);
        crate::platform::log("\n");
    }
    let mut hdr = [0u8; SECTOR];
    if dev.read_blocks(base, &mut hdr).is_err() {
        crate::platform::log("[sysroot] header sector read FAILED on the SATA disk\n");
        return;
    }
    if &hdr[0..8] != MAGIC {
        // The disk the AHCI driver reads (first ATA port) has no sysroot blob at
        // LBA 0. Dump the first bytes so we can tell what it is: all-00 = empty/
        // wrong disk; an MBR/GPT (often ends 55 AA, "EFI PART" at LBA 1) = a
        // partitioned/OS disk and the blob (if any) is on a DIFFERENT SATA port.
        crate::platform::log("[sysroot] no SEMSYSR1 magic at LBA 0; first 16 bytes:");
        for b in hdr.iter().take(16) {
            crate::platform::log(" ");
            crate::platform::log_hex_byte(*b);
        }
        crate::platform::log("\n");
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

/// M27 DEMO 80 Stage 3: copy `sysroot.img` off a FAT-formatted USB stick
/// ("usb0") onto the SATA disk ("sata0") at LBA 0, then verify the SEMSYSR1
/// magic landed. Streams in <=4 KiB chunks (no large RAM). Returns the number
/// of bytes copied, or a static error string.
pub fn flash_from_usb() -> Result<u64, &'static str> {
    let sata = registry::get_block("sata0").ok_or("no sata0 (SATA disk not found)")?;
    // Where the blob goes: a SEMOS_SYSROOT partition if present, else raw LBA 0.
    // If the disk is GPT-partitioned but has no such partition, refuse — writing
    // LBA 0 would destroy its partition table.
    let base = resolve_base_lba(sata).ok_or(
        "sata0 is GPT-partitioned with no SEMOS_SYSROOT partition; refusing to overwrite LBA 0",
    )?;

    // Scan every USB mass-storage slot (boot stick + SanDisk + ...) and pick the
    // one that actually has SYSROOT.IMG in its FAT root — so the user can boot
    // with both drives plugged in and not worry which is which.
    let mut chosen: Option<(crate::fs::fat::Fat32<'static>, u32, u32, &'static str)> = None;
    for name in ["usb0", "usb1", "usb2", "usb3"] {
        let dev = match registry::get_block(name) {
            Some(d) => d,
            None => continue,
        };
        let mut fat = match crate::fs::fat::Fat32::mount(dev) {
            Some(f) => f,
            None => continue, // not FAT32 (empty slot, raw disk, etc.)
        };
        if let Some((cluster, size)) = fat.find_file(b"SYSROOT IMG", "sysroot.img") {
            crate::platform::log("[flash] found SYSROOT.IMG on ");
            crate::platform::log(name);
            crate::platform::log("\n");
            chosen = Some((fat, cluster, size, name));
            break;
        }
    }
    let (mut fat, cluster, size, _name) =
        chosen.ok_or("SYSROOT.IMG not found on any USB stick (usb0..usb3)")?;

    crate::platform::log("[flash] copying SYSROOT.IMG (");
    crate::platform::log_num(size as u64);
    crate::platform::log(" bytes) -> sata0...\n");

    let mut write_err = false;
    let mut written: u64 = 0;
    let copied = fat.read_file(cluster, size, |off, chunk| {
        // `off` is sector-aligned for every chunk; pad a short final chunk up to
        // a full sector so write_blocks (multiple-of-512) is satisfied.
        let mut sect = [0u8; 4096];
        let n = chunk.len();
        let padded = n.div_ceil(SECTOR) * SECTOR;
        sect[..n].copy_from_slice(chunk);
        if sata.write_blocks(base + off / SECTOR as u64, &sect[..padded]).is_err() {
            write_err = true;
            return false;
        }
        written += n as u64;
        true
    });
    if write_err {
        return Err("SATA write_blocks failed");
    }
    if !copied {
        return Err("FAT read failed mid-copy");
    }

    // Verify: read back the header sector and confirm the magic.
    let mut sec = [0u8; SECTOR];
    sata.read_blocks(base, &mut sec).map_err(|_| "SATA verify read failed")?;
    if &sec[0..8] != MAGIC {
        return Err("verify failed: SEMSYSR1 magic not present on sata0 after copy");
    }
    // SAFETY: single-threaded syscall context; subsequent reads use this base.
    unsafe { BASE_LBA = base; }
    crate::platform::log("[flash] done; SEMSYSR1 verified on sata0. Reboot to load it.\n");
    Ok(written)
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
    // SAFETY: read-only access to a value written once at boot by probe().
    let base = unsafe { BASE_LBA };

    let mut produced = 0usize;
    while produced < want {
        // Disk byte position of the next unread chunk (relative to the blob's
        // base LBA — 0 in legacy whole-disk mode, the partition start otherwise).
        let pos = (base + file_lba) * SECTOR as u64 + offset + produced as u64;
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
