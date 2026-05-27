//! Block-device-backed snapshot of kernel state.
//!
//! Provides a minimal `save_snapshot` / `load_snapshot` API on top of
//! the `BlockDevice` trait. Lays out a payload as:
//!
//! ```text
//!   sector 0:    SnapshotHeader  (magic | version | length | crc32-stub)
//!   sector 1+:   raw payload bytes (zero-padded to a sector boundary)
//! ```
//!
//! Sized for the demo: max payload `MAX_SNAPSHOT_BYTES`. Larger payloads
//! return `BufferTooSmall`. No encryption here — see
//! `crate::storage::persistence` for the crypto-aware multi-pool path,
//! which we'll layer on top of this once the basic disk plumbing is
//! proven.

use crate::drivers::traits::{BlockDevice, DriverError, DriverResult};

const SECTOR_SIZE: usize = 512;
const MAGIC: u64 = 0x_5365_6D4F_535F_4E41; // "SemOS_NA" ASCII LE — change w/ format

/// Maximum payload bytes a single snapshot can hold. 1 MiB so the whole
/// namespace — a handful of installed apps (≤256 KiB each) plus documents and
/// system files — persists. The caller buffers a payload of this size on the
/// heap (NOT the stack), so this can grow without stack-overflow risk; the
/// on-disk reservation is `PAYLOAD_SECTORS` (2048) + 1 header sector, well
/// within the 16 MiB vdisk.
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
const PAYLOAD_SECTORS: u64 = (MAX_SNAPSHOT_BYTES / SECTOR_SIZE) as u64;

#[repr(C)]
#[derive(Copy, Clone)]
struct SnapshotHeader {
    magic:     u64,    // MAGIC if a valid snapshot is present
    version:   u32,    // bump when on-disk format changes
    length:    u32,    // payload length in bytes (≤ MAX_SNAPSHOT_BYTES)
    checksum:  u32,    // simple sum-mod-2^32 of payload bytes; lightweight integrity check
    _reserved: u32,
}

/// Write `payload` to the device at sector 0..N. Pads to a sector
/// boundary. Returns InvalidParameter if too large.
pub fn save_snapshot(dev: &dyn BlockDevice, payload: &[u8]) -> DriverResult<()> {
    if dev.is_read_only() {
        return Err(DriverError::ReadOnly);
    }
    if payload.len() > MAX_SNAPSHOT_BYTES {
        return Err(DriverError::InvalidParameter);
    }
    if dev.block_size() != SECTOR_SIZE {
        return Err(DriverError::NotSupported);
    }

    // Build the header.
    let mut sum: u32 = 0;
    for &b in payload { sum = sum.wrapping_add(b as u32); }
    let header = SnapshotHeader {
        magic:     MAGIC,
        version:   1,
        length:    payload.len() as u32,
        checksum:  sum,
        _reserved: 0,
    };

    // Sector 0 = header (zero-padded).
    let mut sec0 = [0u8; SECTOR_SIZE];
    let h_bytes = unsafe {
        core::slice::from_raw_parts(
            &header as *const SnapshotHeader as *const u8,
            core::mem::size_of::<SnapshotHeader>(),
        )
    };
    sec0[..h_bytes.len()].copy_from_slice(h_bytes);
    dev.write_blocks(0, &sec0)?;

    // Sectors 1.. = payload (zero-padded to sector boundary).
    let total_payload_sectors =
        ((payload.len() + SECTOR_SIZE - 1) / SECTOR_SIZE) as u64;
    if total_payload_sectors > PAYLOAD_SECTORS {
        return Err(DriverError::OutOfBounds);
    }
    let mut sec_buf = [0u8; SECTOR_SIZE];
    for i in 0..total_payload_sectors {
        let start = (i as usize) * SECTOR_SIZE;
        let end = (start + SECTOR_SIZE).min(payload.len());
        let n = end - start;
        sec_buf[..n].copy_from_slice(&payload[start..end]);
        // Zero the tail of a partial last sector.
        for byte in &mut sec_buf[n..] { *byte = 0; }
        dev.write_blocks(1 + i, &sec_buf)?;
    }
    dev.flush()?;
    Ok(())
}

/// Read a previously-saved snapshot into `out`. Returns the payload
/// length on success, `Err(NotReady)` if no valid header is present
/// (e.g., a fresh disk), `Err(IoError)` if the checksum mismatches.
pub fn load_snapshot(dev: &dyn BlockDevice, out: &mut [u8]) -> DriverResult<usize> {
    if dev.block_size() != SECTOR_SIZE {
        return Err(DriverError::NotSupported);
    }
    let mut sec0 = [0u8; SECTOR_SIZE];
    dev.read_blocks(0, &mut sec0)?;

    // Pull the header off sector 0.
    let header: SnapshotHeader = unsafe {
        core::ptr::read_unaligned(sec0.as_ptr() as *const SnapshotHeader)
    };
    if header.magic != MAGIC {
        return Err(DriverError::NotReady);
    }
    let len = header.length as usize;
    if len > MAX_SNAPSHOT_BYTES || len > out.len() {
        return Err(DriverError::BufferTooSmall);
    }

    // Read payload sectors and copy into `out`.
    let payload_sectors = ((len + SECTOR_SIZE - 1) / SECTOR_SIZE) as u64;
    let mut sec_buf = [0u8; SECTOR_SIZE];
    for i in 0..payload_sectors {
        dev.read_blocks(1 + i, &mut sec_buf)?;
        let start = (i as usize) * SECTOR_SIZE;
        let end = (start + SECTOR_SIZE).min(len);
        let n = end - start;
        out[start..end].copy_from_slice(&sec_buf[..n]);
    }

    // Verify checksum.
    let mut sum: u32 = 0;
    for &b in &out[..len] { sum = sum.wrapping_add(b as u32); }
    if sum != header.checksum {
        return Err(DriverError::IoError);
    }
    Ok(len)
}
