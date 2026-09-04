//! SemFS journal — write-through durability for the SUID object store.
//!
//! Design: `docs/semfs-journal-design.md`. Append-only log of full-state
//! object records on a raw block-device region; CRC-tail-gated replay;
//! A/B superblocks. v1 policy is write-through: every registry mutation
//! appends synchronously and rolls back on disk error. The format
//! reserves `op=COMMIT` + a per-record COMMITTED bit so a group-commit
//! policy can land later without a format break (`CommitPolicy`).
//!
//! Lock order: REGISTRY → JOURNAL (insert/remove hooks hold the registry
//! lock, then take the journal lock). Mount-time replay is the inverse
//! order but runs single-threaded before any task can issue syscalls —
//! do NOT call `mount()` after the scheduler admits user tasks.

use crate::drivers::traits::BlockDevice;
use crate::sync::Mutex;
use super::object::{
    ContentType, ObjectContent, RelationType, SemanticObject, MAX_LINKS,
};
use super::suid::SUID;
use crate::memory::pools::SecurityTier;

// ---------------------------------------------------------------- format

const REC_MAGIC: &[u8; 4] = b"SFRO";
const SB_MAGIC: &[u8; 4] = b"SFSB";
const FORMAT_VERSION: u16 = 1;

const OP_UPSERT: u8 = 1;
const OP_TOMBSTONE: u8 = 2;
#[allow(dead_code)] // reserved for group-commit (design §6)
const OP_COMMIT: u8 = 3;

const FLAG_COMMITTED: u8 = 1;

/// Fixed header size (see design §1.2): 60 bytes of fields, padded.
const HEADER_LEN: usize = 64;
/// Per-link serialized size: target SUID (16) + relationship (1).
const LINK_LEN: usize = 17;

const SECTOR: usize = 512;

/// Commit policy. v1: WriteThrough — every record carries COMMITTED and
/// is durable before the mutation returns. GroupCommit is the designed-
/// for escape hatch (design §6); intentionally unimplemented.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CommitPolicy {
    WriteThrough,
    #[allow(dead_code)]
    GroupCommit { interval_ticks: u64 },
}

struct Journal {
    dev: &'static dyn BlockDevice,
    /// First LBA of the log region (records).
    log_start: u64,
    /// Total log sectors (format-time constant).
    log_sectors: u64,
    /// Next free LBA for appends.
    head: u64,
    /// Next commit sequence number.
    next_seq: u64,
    policy: CommitPolicy,
    /// True while `mount()` replays — suppresses the insert/remove hooks
    /// so replayed objects are not re-journaled.
    replaying: bool,
    /// True once mounted; hooks no-op until then.
    enabled: bool,
}

static JOURNAL: Mutex<Option<Journal>> = Mutex::new(None);

// ---------------------------------------------------------------- crc32

/// IEEE CRC-32 (reflected, poly 0xEDB88320), table built at compile time.
const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
static CRC_TABLE: [u32; 256] = crc_table();

struct Crc(u32);
impl Crc {
    fn new() -> Self { Crc(0xFFFF_FFFF) }
    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.0 = CRC_TABLE[((self.0 ^ b as u32) & 0xFF) as usize] ^ (self.0 >> 8);
        }
    }
    fn finish(&self) -> u32 { !self.0 }
}
fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc::new();
    c.update(data);
    c.finish()
}

// ------------------------------------------------------------ superblock

/// Superblock serialization (design §1.3). One 512 B sector.
struct Superblock {
    generation: u64,
    log_start: u64,
    log_sectors: u64,
    high_water: u64,
    clean_shutdown: bool,
}

fn encode_superblock(sb: &Superblock, out: &mut [u8; SECTOR]) {
    for b in out.iter_mut() { *b = 0; }
    out[0..4].copy_from_slice(SB_MAGIC);
    out[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    out[6] = if sb.clean_shutdown { 1 } else { 0 };
    out[8..16].copy_from_slice(&sb.generation.to_le_bytes());
    out[16..24].copy_from_slice(&sb.log_start.to_le_bytes());
    out[24..32].copy_from_slice(&sb.log_sectors.to_le_bytes());
    out[32..40].copy_from_slice(&sb.high_water.to_le_bytes());
    let crc = crc32(&out[0..40]);
    out[40..44].copy_from_slice(&crc.to_le_bytes());
}

fn decode_superblock(buf: &[u8]) -> Option<Superblock> {
    if &buf[0..4] != SB_MAGIC { return None; }
    if u16::from_le_bytes([buf[4], buf[5]]) != FORMAT_VERSION { return None; }
    let stored = u32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]);
    if crc32(&buf[0..40]) != stored { return None; }
    Some(Superblock {
        generation: u64::from_le_bytes(buf[8..16].try_into().ok()?),
        log_start: u64::from_le_bytes(buf[16..24].try_into().ok()?),
        log_sectors: u64::from_le_bytes(buf[24..32].try_into().ok()?),
        high_water: u64::from_le_bytes(buf[32..40].try_into().ok()?),
        clean_shutdown: buf[6] != 0,
    })
}

// ---------------------------------------------------------------- record

/// Serialized header length for a given link count.
fn record_header_len(link_count: usize) -> usize {
    HEADER_LEN + link_count * LINK_LEN
}

fn relation_to_u8(r: RelationType) -> u8 {
    match r {
        RelationType::Contains => 0,
        RelationType::References => 1,
        RelationType::DerivedFrom => 2,
        RelationType::SimilarTo => 3,
        RelationType::Supersedes => 4,
        RelationType::Custom => 255,
    }
}
fn relation_from_u8(v: u8) -> RelationType {
    match v {
        1 => RelationType::References,
        2 => RelationType::DerivedFrom,
        3 => RelationType::SimilarTo,
        4 => RelationType::Supersedes,
        255 => RelationType::Custom,
        _ => RelationType::Contains,
    }
}

fn content_type_to_u8(c: ContentType) -> u8 { c as u8 }
fn content_type_from_u8(v: u8) -> ContentType {
    match v {
        1 => ContentType::Text,
        2 => ContentType::Vector,
        3 => ContentType::Structured,
        4 => ContentType::Reference,
        _ => ContentType::Binary,
    }
}

fn tier_to_u8(t: SecurityTier) -> u8 {
    match t {
        SecurityTier::Public => 0,
        SecurityTier::Internal => 1,
        SecurityTier::Sensitive => 2,
        SecurityTier::Secret => 3,
    }
}
fn tier_from_u8(v: u8) -> SecurityTier {
    match v {
        1 => SecurityTier::Internal,
        2 => SecurityTier::Sensitive,
        3 => SecurityTier::Secret,
        _ => SecurityTier::Public,
    }
}

/// Total on-disk size of a record (before 512 B alignment).
fn record_len(link_count: usize, content_len: usize) -> usize {
    record_header_len(link_count) + content_len + 4 // + crc
}

/// Serialize an object record and append it at `head_lba`.
/// Returns sectors consumed.
fn append_record(
    j: &Journal,
    obj: Option<&SemanticObject>,
    tombstone_suid: Option<SUID>,
    seq: u64,
    content_override: Option<&[u8]>,
) -> Result<u64, ()> {
    let (suid, link_count, content): (SUID, usize, &[u8]) = match obj {
        Some(o) => {
            let c: &[u8] = match content_override {
                Some(b) => b,
                None => match o.content.as_bytes() {
                    Some(b) => b,
                    None => &[], // Empty / Vector (not persisted, design §7)
                },
            };
            (o.suid, o.link_count as usize, c)
        }
        None => (tombstone_suid.unwrap_or_else(|| SUID::from_bytes(&[0; 16])), 0, &[]),
    };

    let mut header = [0u8; HEADER_LEN];
    header[0..4].copy_from_slice(REC_MAGIC);
    header[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    header[6] = if obj.is_some() { OP_UPSERT } else { OP_TOMBSTONE };
    header[7] = FLAG_COMMITTED; // v1 write-through: always committed
    header[8..16].copy_from_slice(&seq.to_le_bytes());
    header[16..32].copy_from_slice(&suid.to_bytes());
    if let Some(o) = obj {
        header[32] = tier_to_u8(o.tier);
        header[33] = o.owner;
        header[34] = content_type_to_u8(o.content_type);
        header[35] = 0; // ObjectFlags: v1 stores 0 (flags are session-state)
        header[36..44].copy_from_slice(&o.created_at.to_le_bytes());
        header[44..52].copy_from_slice(&o.modified_at.to_le_bytes());
    }
    header[52..56].copy_from_slice(&(content.len() as u32).to_le_bytes());
    header[56..60].copy_from_slice(&(link_count as u32).to_le_bytes());

    // Links serialized into a small stack buffer (≤ 16 × 17 = 272 B).
    let mut links_buf = [0u8; MAX_LINKS * LINK_LEN];
    if let Some(o) = obj {
        for (i, slot) in o.get_links().iter().enumerate() {
            if let Some(l) = slot {
                let base = i * LINK_LEN;
                links_buf[base..base + 16].copy_from_slice(&l.target.to_bytes());
                links_buf[base + 16] = relation_to_u8(l.relationship);
            }
        }
    }
    let links_bytes = &links_buf[..link_count * LINK_LEN];

    // Two passes: CRC the logical byte stream first (header + links +
    // content — order-independent of sector boundaries), then stream the
    // same bytes plus the CRC tail out in 512 B chunks.
    let mut crc = Crc::new();
    crc.update(&header);
    crc.update(links_bytes);
    crc.update(content);
    let crc_val = crc.finish();

    let mut sec = [0u8; SECTOR];
    let mut fill = 0usize;
    let mut lba = j.head;

    macro_rules! feed {
        ($data:expr, $do_crc:expr) => {{
            let data: &[u8] = $data;
            let mut off = 0usize;
            while off < data.len() {
                let take = core::cmp::min(SECTOR - fill, data.len() - off);
                sec[fill..fill + take].copy_from_slice(&data[off..off + take]);
                fill += take;
                off += take;
                if fill == SECTOR {
                    j.dev.write_blocks(lba, &sec).map_err(|_| ())?;
                    lba += 1;
                    fill = 0;
                }
            }
        }};
    }

    feed!(&header, true);
    feed!(links_bytes, true);
    feed!(content, true);
    feed!(&crc_val.to_le_bytes(), false); // tail: appended raw
    if fill > 0 {
        for b in &mut sec[fill..] { *b = 0; }
        j.dev.write_blocks(lba, &sec).map_err(|_| ())?;
        lba += 1;
    }
    Ok(lba - j.head)
}

// ---------------------------------------------------------------- hooks

/// Registry-insert hook. Called with the registry lock held, BEFORE the
/// object is stored. On disk error the caller must abort the insert
/// (write-through: durable and visible are the same event).
pub fn on_insert(obj: &SemanticObject) -> bool {
    let mut guard = JOURNAL.lock();
    let j = match guard.as_mut() {
        Some(j) if j.enabled && !j.replaying => j,
        _ => return true, // not mounted: RAM-only, success
    };
    let seq = j.next_seq;
    match append_record(j, Some(obj), None, seq, None) {
        Ok(sectors) => {
            j.head += sectors;
            j.next_seq += 1;
            true
        }
        Err(()) => false,
    }
}

/// Registry-remove hook. Appends a tombstone. Same write-through
/// semantics: on disk error the caller re-inserts the object.
pub fn on_remove(suid: &SUID) -> bool {
    let mut guard = JOURNAL.lock();
    let j = match guard.as_mut() {
        Some(j) if j.enabled && !j.replaying => j,
        _ => return true,
    };
    let seq = j.next_seq;
    match append_record(j, None, Some(*suid), seq, None) {
        Ok(sectors) => {
            j.head += sectors;
            j.next_seq += 1;
            true
        }
        Err(()) => false,
    }
}

/// Content-mutation hook for the get_mut() path (SYS_FWRITE positional
/// writes). Appends the UPSERT built from the object's metadata plus the
/// NEW content — BEFORE the caller assigns it — preserving the
/// write-through invariant (durable strictly before visible).
pub fn on_update(obj: &SemanticObject, new_content: &[u8]) -> bool {
    let mut guard = JOURNAL.lock();
    let j = match guard.as_mut() {
        Some(j) if j.enabled && !j.replaying => j,
        _ => return true,
    };
    let seq = j.next_seq;
    match append_record(j, Some(obj), None, seq, Some(new_content)) {
        Ok(sectors) => {
            j.head += sectors;
            j.next_seq += 1;
            true
        }
        Err(()) => false,
    }
}

/// True once a journal is mounted (used by fsync: a no-op success when
/// write-through makes every mutation already durable).
pub fn is_mounted() -> bool {
    let guard = JOURNAL.lock();
    matches!(guard.as_ref(), Some(j) if j.enabled)
}

// ---------------------------------------------------------------- mount

/// Result of a mount attempt.
pub enum MountOutcome {
    /// Log replayed; this many records applied. `torn` is true when
    /// replay stopped at a corrupted/partial tail (rather than a clean
    /// zeroed tail) — the torn record was dropped and the head reset.
    Mounted { records: usize, torn: bool },
    /// Disk unformatted (no valid superblock); formatted fresh.
    Formatted,
    /// No journal (I/O error probing). System continues RAM-only.
    Unavailable,
}

/// Mount the journal on `dev`, log region at LBA `sb_lba + 2`
/// (`sb_lba`/`sb_lba+1` hold superblocks A/B), `log_sectors` long.
/// Replays into the global registry. Must run after
/// `init_global_registry()` and `Namespace::init()`, before user tasks.
pub fn mount(dev: &'static dyn BlockDevice, sb_lba: u64, log_sectors: u64) -> MountOutcome {
    let mut sec = [0u8; SECTOR];

    // Probe both superblocks; pick the valid one with the higher generation.
    let mut best: Option<Superblock> = None;
    for slot in 0..2u64 {
        if dev.read_blocks(sb_lba + slot, &mut sec).is_err() {
            return MountOutcome::Unavailable;
        }
        if let Some(sb) = decode_superblock(&sec) {
            let better = match &best {
                Some(b) => sb.generation > b.generation,
                None => true,
            };
            if better { best = Some(sb); }
        }
    }

    let (log_start, log_len, mut next_seq, generation) = match best {
        Some(sb) => (sb.log_start, sb.log_sectors, sb.high_water + 1, sb.generation),
        None => {
            // Unformatted: write fresh superblocks, empty log.
            let sb = Superblock {
                generation: 1,
                log_start: sb_lba + 2,
                log_sectors,
                high_water: 0,
                clean_shutdown: false,
            };
            encode_superblock(&sb, &mut sec);
            if dev.write_blocks(sb_lba, &sec).is_err() { return MountOutcome::Unavailable; }
            if dev.write_blocks(sb_lba + 1, &sec).is_err() { return MountOutcome::Unavailable; }
            {
                let mut guard = JOURNAL.lock();
                *guard = Some(Journal {
                    dev,
                    log_start: sb_lba + 2,
                    log_sectors,
                    head: sb_lba + 2,
                    next_seq: 1,
                    policy: CommitPolicy::WriteThrough,
                    replaying: false,
                    enabled: true,
                });
            }
            return MountOutcome::Formatted;
        }
    };

    // Replay: scan records, apply, stop at first bad/torn record.
    let mut applied = 0usize;
    {
        let mut guard = JOURNAL.lock();
        *guard = Some(Journal {
            dev,
            log_start,
            log_sectors: log_len,
            head: log_start,
            next_seq,
            policy: CommitPolicy::WriteThrough,
            replaying: true,
            enabled: true,
        });
    }

    let mut head = log_start;
    let log_end = log_start + log_len;
    let mut torn = false;
    'scan: loop {
        if head >= log_end { break; }
        if dev.read_blocks(head, &mut sec).is_err() { break; }
        if &sec[0..4] != REC_MAGIC {
            // All-zero sector = clean tail; anything else = torn garbage.
            if sec.iter().any(|&b| b != 0) { torn = true; }
            break;
        }
        let content_len = u32::from_le_bytes([sec[52], sec[53], sec[54], sec[55]]) as usize;
        let link_count = u32::from_le_bytes([sec[56], sec[57], sec[58], sec[59]]) as usize;
        if link_count > MAX_LINKS { torn = true; break; }
        let total = record_len(link_count, content_len);
        let sectors = (total + SECTOR - 1) / SECTOR;
        if sectors == 0 || head + sectors as u64 > log_end { torn = true; break; }

        // Read the full record into a heap buffer and CRC it.
        let buf_ptr = crate::memory::heap::allocate(sectors * SECTOR, 8);
        if buf_ptr.is_null() { break; }
        let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, sectors * SECTOR) };
        let io_ok = dev.read_blocks(head, buf).is_ok();
        if !io_ok { torn = true;
            crate::memory::heap::deallocate(buf_ptr, sectors * SECTOR, 8);
            break;
        }
        let crc_stored_off = record_header_len(link_count) + content_len;
        let crc_ok = crc_stored_off + 4 <= buf.len()
            && crc32(&buf[..crc_stored_off])
                == u32::from_le_bytes([
                    buf[crc_stored_off],
                    buf[crc_stored_off + 1],
                    buf[crc_stored_off + 2],
                    buf[crc_stored_off + 3],
                ]);
        if !crc_ok { torn = true;
            crate::memory::heap::deallocate(buf_ptr, sectors * SECTOR, 8);
            break; // torn tail: truncate here
        }

        let op = sec[6];
        let seq = u64::from_le_bytes(sec[8..16].try_into().unwrap_or([0; 8]));
        let mut suid_bytes = [0u8; 16];
        suid_bytes.copy_from_slice(&sec[16..32]);
        let suid = SUID::from_bytes(&suid_bytes);

        match op {
            OP_UPSERT => {
                let mut obj = SemanticObject::new(
                    suid, tier_from_u8(sec[32]), sec[33]);
                obj.content_type = content_type_from_u8(sec[34]);
                obj.created_at = u64::from_le_bytes(sec[36..44].try_into().unwrap_or([0; 8]));
                obj.modified_at = u64::from_le_bytes(sec[44..52].try_into().unwrap_or([0; 8]));
                // links
                let link_base = HEADER_LEN;
                for i in 0..link_count {
                    let off = link_base + i * LINK_LEN;
                    let mut t = [0u8; 16];
                    t.copy_from_slice(&buf[off..off + 16]);
                    let _ = obj.add_link(
                        SUID::from_bytes(&t), relation_from_u8(buf[off + 16]));
                }
                let content_off = record_header_len(link_count);
                let content = &buf[content_off..content_off + content_len];
                if !content.is_empty() {
                    obj.content = match ObjectContent::from_bytes(content) {
                        Some(c) => c,
                        None => ObjectContent::Empty,
                    };
                }
                // Apply: replace any older state (last-writer-wins by seq).
                let mut reg = super::registry::global_registry();
                let _ = reg.remove(&suid);
                let _ = reg.insert(obj);
                drop(reg);
                if seq >= next_seq { next_seq = seq + 1; }
                applied += 1;
            }
            OP_TOMBSTONE => {
                let mut reg = super::registry::global_registry();
                let _ = reg.remove(&suid);
                drop(reg);
                if seq >= next_seq { next_seq = seq + 1; }
                applied += 1;
            }
            _ => {
                torn = true;
                // Unknown op (incl. reserved COMMIT in v1): stop — the
                // log may be from a newer format version.
                crate::memory::heap::deallocate(buf_ptr, sectors * SECTOR, 8);
                break 'scan;
            }
        }
        crate::memory::heap::deallocate(buf_ptr, sectors * SECTOR, 8);
        head += sectors as u64;
    }

    {
        let mut guard = JOURNAL.lock();
        if let Some(j) = guard.as_mut() {
            j.head = head;
            j.next_seq = next_seq;
            j.replaying = false;
        }
    }

    // Refresh the non-winning superblock slot so both are valid and the
    // generation advances (crash between slots still leaves the other).
    let sb = Superblock {
        generation: generation + 1,
        log_start,
        log_sectors: log_len,
        high_water: next_seq.saturating_sub(1),
        clean_shutdown: false,
    };
    encode_superblock(&sb, &mut sec);
    let _ = dev.write_blocks(sb_lba, &sec);
    let _ = dev.write_blocks(sb_lba + 1, &sec);

    MountOutcome::Mounted { records: applied, torn }
}
