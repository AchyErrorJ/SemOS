//! USB Mass Storage class — Bulk-Only Transport (BBB) + SCSI command set.
//!
//! The path to reading a USB stick on the T540. A USB Mass Storage device
//! exposes one Communications interface with two bulk endpoints (one IN,
//! one OUT). Every command is a three-phase transaction:
//!
//!   1. Host → device: 31-byte **CBW** (Command Block Wrapper) carrying a
//!      SCSI CDB and the length/direction of the data phase.
//!   2. Data phase: bulk IN (device→host) or bulk OUT (host→device), per
//!      `bmCBWFlags`.
//!   3. Device → host: 13-byte **CSW** (Command Status Wrapper) reporting
//!      pass/fail and bytes-not-transferred.
//!
//! v1 = the byte-level protocol (CBW/CSW + the SCSI CDBs we'll use —
//! INQUIRY, READ CAPACITY (10), READ (10), WRITE (10), TEST UNIT READY).
//! Live xHCI bulk-endpoint TX/RX is the follow-up (same gating as CDC-ECM).
//! Validated against the USB MS BBB §5 + SCSI Block Commands spec layouts.

// ============================================================================
// USB class IDs that identify a Mass Storage / BBB / SCSI function
// ============================================================================
pub const CLASS_MASS_STORAGE: u8 = 0x08;
pub const SUBCLASS_SCSI: u8 = 0x06;
pub const PROTOCOL_BBB: u8 = 0x50; // Bulk-Only Transport

// ============================================================================
// Command Block Wrapper (host → device, 31 bytes)
// ============================================================================
pub const CBW_LEN: usize = 31;
pub const CBW_SIGNATURE: u32 = 0x4342_5355; // 'USBC' little-endian

/// Direction bit in `bmCBWFlags`. Set = data is device→host (IN); clear = OUT.
pub const CBW_FLAG_DATA_IN: u8 = 0x80;

/// Build a Command Block Wrapper into `out`. `cdb` is the SCSI command (1-16
/// bytes); it's zero-padded to 16 bytes in the CBW. Returns 31 on success.
pub fn build_cbw(
    out: &mut [u8],
    tag: u32,
    data_len: u32,
    data_in: bool,
    lun: u8,
    cdb: &[u8],
) -> Option<usize> {
    if out.len() < CBW_LEN || cdb.is_empty() || cdb.len() > 16 {
        return None;
    }
    out[0..4].copy_from_slice(&CBW_SIGNATURE.to_le_bytes());
    out[4..8].copy_from_slice(&tag.to_le_bytes());
    out[8..12].copy_from_slice(&data_len.to_le_bytes());
    out[12] = if data_in { CBW_FLAG_DATA_IN } else { 0 };
    out[13] = lun & 0x0F;
    out[14] = cdb.len() as u8;
    // CBWCB: copy CDB, zero-pad to 16 bytes.
    for b in &mut out[15..31] {
        *b = 0;
    }
    out[15..15 + cdb.len()].copy_from_slice(cdb);
    Some(CBW_LEN)
}

// ============================================================================
// Command Status Wrapper (device → host, 13 bytes)
// ============================================================================
pub const CSW_LEN: usize = 13;
pub const CSW_SIGNATURE: u32 = 0x5342_5355; // 'USBS' little-endian

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CswStatus {
    Passed,
    Failed,
    PhaseError,
    Reserved(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct Csw {
    pub tag: u32,
    pub residue: u32,
    pub status: CswStatus,
}

/// Parse a CSW. Returns None on signature mismatch or truncation.
pub fn parse_csw(buf: &[u8]) -> Option<Csw> {
    if buf.len() < CSW_LEN {
        return None;
    }
    let sig = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    if sig != CSW_SIGNATURE {
        return None;
    }
    let tag = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    let residue = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    let status = match buf[12] {
        0 => CswStatus::Passed,
        1 => CswStatus::Failed,
        2 => CswStatus::PhaseError,
        other => CswStatus::Reserved(other),
    };
    Some(Csw { tag, residue, status })
}

// ============================================================================
// SCSI command builders (CDBs to drop into a CBW)
// ============================================================================
pub mod scsi {
    pub const OP_TEST_UNIT_READY: u8 = 0x00;
    pub const OP_REQUEST_SENSE: u8 = 0x03;
    pub const OP_INQUIRY: u8 = 0x12;
    pub const OP_READ_CAPACITY_10: u8 = 0x25;
    pub const OP_READ_10: u8 = 0x28;
    pub const OP_WRITE_10: u8 = 0x2A;

    /// INQUIRY CDB (6 bytes): `OP, 0, 0, 0, alloc_len, 0`.
    pub fn inquiry(alloc_len: u8) -> [u8; 6] {
        [OP_INQUIRY, 0, 0, 0, alloc_len, 0]
    }

    /// READ CAPACITY (10) CDB — returns 8-byte response (last LBA + block size).
    pub fn read_capacity_10() -> [u8; 10] {
        [OP_READ_CAPACITY_10, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    }

    /// READ (10) CDB. LBA is big-endian in CDB[2..6], length in CDB[7..9].
    pub fn read_10(lba: u32, blocks: u16) -> [u8; 10] {
        let l = lba.to_be_bytes();
        let n = blocks.to_be_bytes();
        [OP_READ_10, 0, l[0], l[1], l[2], l[3], 0, n[0], n[1], 0]
    }

    /// WRITE (10) CDB — same layout as READ(10) with opcode 0x2A.
    pub fn write_10(lba: u32, blocks: u16) -> [u8; 10] {
        let l = lba.to_be_bytes();
        let n = blocks.to_be_bytes();
        [OP_WRITE_10, 0, l[0], l[1], l[2], l[3], 0, n[0], n[1], 0]
    }

    /// TEST UNIT READY — all zero CDB; used to check device-ready after attach.
    pub fn test_unit_ready() -> [u8; 6] {
        [OP_TEST_UNIT_READY, 0, 0, 0, 0, 0]
    }
}

// ============================================================================
// SCSI response parsers
// ============================================================================

/// Standard INQUIRY response (first 36 bytes).
#[derive(Clone, Copy, Debug)]
pub struct InquiryData {
    pub peripheral_type: u8, // 0 = direct access (disk)
    pub removable: bool,
    pub vendor: [u8; 8],    // ASCII, space-padded
    pub product: [u8; 16],
    pub revision: [u8; 4],
}

pub fn parse_inquiry(buf: &[u8]) -> Option<InquiryData> {
    if buf.len() < 36 {
        return None;
    }
    let mut vendor = [b' '; 8];
    vendor.copy_from_slice(&buf[8..16]);
    let mut product = [b' '; 16];
    product.copy_from_slice(&buf[16..32]);
    let mut revision = [b' '; 4];
    revision.copy_from_slice(&buf[32..36]);
    Some(InquiryData {
        peripheral_type: buf[0] & 0x1F,
        removable: (buf[1] & 0x80) != 0,
        vendor,
        product,
        revision,
    })
}

/// READ CAPACITY (10) response: u32 last_LBA + u32 block_size, both BE.
/// Returns `(block_count, block_size)` where block_count = last_LBA + 1.
pub fn parse_read_capacity_10(buf: &[u8]) -> Option<(u32, u32)> {
    if buf.len() < 8 {
        return None;
    }
    let last_lba = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    let block_size = u32::from_be_bytes(buf[4..8].try_into().unwrap());
    Some((last_lba.saturating_add(1), block_size))
}
