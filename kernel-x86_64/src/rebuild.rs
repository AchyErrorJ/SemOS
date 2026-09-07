//! M22a self-rebuild machinery (docs/self-rebuild-design.md): SRBL slot
//! record, candidate staging, hash-bound human vouch, trial/health/keep/
//! revert state machine. Auto-revert is the default failure direction;
//! promotion is the only transition that requires a human.
//!
//! Disk layout on virtio0 (all fixed compile-time LBAs — no write-target
//! arithmetic anywhere, closing threat vector 4):
//!
//! ```text
//!   LBA 16              semos-pkg registry mirror (SEMREG01, M43/M44)
//!   LBA 8190/8191       SRBL slot record, copies A/B (generation wins)
//!   LBA 8192..40960     SemFS journal (log bounded at 32768 sectors so the
//!                       regions above are never journal traffic)
//!   LBA 262144..524288  slot A image region (128 MiB)
//!   LBA 524288..786432  slot B image region (128 MiB)
//!   LBA 786432..1048576 drop zone: [4B "SRBI"][u64 len][16B tag][payload]
//!                       (LAST 128 MiB — a drop zone between journal and
//!                       slots once overran into slot A and corrupted a
//!                       staged image; keep the three big regions disjoint)
//! ```

use kernel_core::crypto::sha256::Sha256;
use kernel_core::drivers::registry;
use kernel_core::semantic::journal::crc32;

// ---------------------------------------------------------------------------
// Fixed LBAs and record layout
// ---------------------------------------------------------------------------

pub const SLOT_RECORD_LBA_A: u64 = 8190;
pub const SLOT_RECORD_LBA_B: u64 = 8191;
pub const DROP_ZONE_LBA: u64 = 786432;
pub const SLOT_A_BASE: u64 = 262144;
pub const SLOT_B_BASE: u64 = 524288;
pub const SLOT_CAPACITY_SECTORS: u64 = 262144; // 128 MiB per slot
/// SemFS journal must stay below the drop zone: cap its log at 16 MiB.
pub const JOURNAL_LOG_CAP: u64 = 32768;

const SRBL_MAGIC: &[u8; 4] = b"SRBL";
const SRBL_VERSION: u32 = 1;
const SRBI_MAGIC: &[u8; 4] = b"SRBI";
const SECTOR: usize = 512;
/// Stage streaming chunk (1 MiB; read_blocks/write_blocks loop sectors
/// internally — one virtqueue round trip per sector either way).
const CHUNK: usize = 1024 * 1024;

pub const SRBL_EMPTY: u8 = 0;
pub const SRBL_STAGED: u8 = 1;
pub const SRBL_TRIAL: u8 = 2;
pub const SRBL_HEALTHY: u8 = 3;
pub const SRBL_PROMOTED: u8 = 4;
pub const SRBL_REVERTED: u8 = 5;

fn state_name(s: u8) -> &'static str {
    match s {
        SRBL_EMPTY => "EMPTY",
        SRBL_STAGED => "STAGED",
        SRBL_TRIAL => "TRIAL",
        SRBL_HEALTHY => "HEALTHY",
        SRBL_PROMOTED => "PROMOTED",
        SRBL_REVERTED => "REVERTED",
        _ => "?",
    }
}

/// Parsed slot record.
#[derive(Clone)]
pub struct SlotRecord {
    pub generation: u64,
    pub state: u8,
    /// Which slot region holds the candidate (0 = A, 1 = B).
    pub trial_slot: u8,
    /// Build tag of the candidate (16 bytes, zero-padded).
    pub trial_tag: [u8; 16],
    pub image_len: u64,
    pub image_sha256: [u8; 32],
}

impl SlotRecord {
    fn empty() -> Self {
        SlotRecord {
            generation: 0,
            state: SRBL_EMPTY,
            trial_slot: 0,
            trial_tag: [0; 16],
            image_len: 0,
            image_sha256: [0; 32],
        }
    }

    fn encode(&self) -> [u8; SECTOR] {
        let mut b = [0u8; SECTOR];
        b[0..4].copy_from_slice(SRBL_MAGIC);
        b[4..8].copy_from_slice(&SRBL_VERSION.to_le_bytes());
        b[8..16].copy_from_slice(&self.generation.to_le_bytes());
        b[16] = self.state;
        b[17] = self.trial_slot;
        b[18..34].copy_from_slice(&self.trial_tag);
        b[34..42].copy_from_slice(&self.image_len.to_le_bytes());
        b[42..74].copy_from_slice(&self.image_sha256);
        let c = crc32(&b[0..74]);
        b[74..78].copy_from_slice(&c.to_le_bytes());
        b
    }

    fn decode(b: &[u8; SECTOR]) -> Option<Self> {
        if &b[0..4] != SRBL_MAGIC {
            return None;
        }
        if u32::from_le_bytes(b[4..8].try_into().ok()?) != SRBL_VERSION {
            return None; // unknown format: read-only, never "repair"
        }
        let stored = u32::from_le_bytes(b[74..78].try_into().ok()?);
        if crc32(&b[0..74]) != stored {
            return None;
        }
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&b[18..34]);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&b[42..74]);
        Some(SlotRecord {
            generation: u64::from_le_bytes(b[8..16].try_into().ok()?),
            state: b[16],
            trial_slot: b[17],
            trial_tag: tag,
            image_len: u64::from_le_bytes(b[34..42].try_into().ok()?),
            image_sha256: hash,
        })
    }
}

// ---------------------------------------------------------------------------
// Record I/O (write-through, both copies, generation wins — SemFS pattern)
// ---------------------------------------------------------------------------

fn dev() -> Option<&'static dyn kernel_core::drivers::BlockDevice> {
    registry::get_block("virtio0")
}

/// Read both SRBL copies; the valid one with the higher generation wins.
/// None when both are absent/corrupt/unknown-version (never auto-format:
/// a missing record just means "no self-rebuild in progress").
pub fn load_record() -> Option<SlotRecord> {
    let d = dev()?;
    let mut best: Option<SlotRecord> = None;
    for lba in [SLOT_RECORD_LBA_A, SLOT_RECORD_LBA_B] {
        let mut sec = [0u8; SECTOR];
        if d.read_blocks(lba, &mut sec).is_err() {
            continue;
        }
        if let Some(r) = SlotRecord::decode(&sec) {
            let better = match &best {
                Some(b) => r.generation > b.generation,
                None => true,
            };
            if better {
                best = Some(r);
            }
        }
    }
    best
}

/// Persist a record: bump the generation, write copy A then copy B. A power
/// cut between the two leaves the older copy intact — generation decides.
fn store_record(rec: &SlotRecord) -> bool {
    let Some(d) = dev() else { return false };
    let mut next = rec.clone();
    next.generation = next.generation.wrapping_add(1);
    let sec = next.encode();
    if d.write_blocks(SLOT_RECORD_LBA_A, &sec).is_err() {
        return false;
    }
    if d.write_blocks(SLOT_RECORD_LBA_B, &sec).is_err() {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Boot-time trial detection (runs unconditionally — cheap when no record)
// ---------------------------------------------------------------------------

static TRIAL_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn trial_active() -> bool {
    TRIAL_ACTIVE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Called from boot after the SemFS journal mount. If the record says a
/// trial is armed: when OUR build tag matches, we're the candidate —
/// arm the health gate; when it doesn't, the previous trial never reached
/// HEALTHY on this machine, so last-known-good is running and the record
/// flips to REVERTED. Auto-revert is the default direction.
pub fn boot_check() {
    let Some(rec) = load_record() else { return };
    if rec.state != SRBL_TRIAL {
        return;
    }
    let my_tag = crate::SEMOS_BUILD_TAG.as_bytes();
    let tag_len = my_tag.len().min(16);
    if rec.trial_tag[..tag_len] == my_tag[..tag_len]
        && (tag_len == 16 || rec.trial_tag[tag_len] == 0)
    {
        TRIAL_ACTIVE.store(true, core::sync::atomic::Ordering::Relaxed);
        crate::println!(
            "[rebuild] TRIAL boot: I am the candidate (slot {}, tag {})",
            rec.trial_slot,
            crate::SEMOS_BUILD_TAG
        );
    } else {
        let mut r = rec.clone();
        r.state = SRBL_REVERTED;
        let _ = store_record(&r);
        crate::println!(
            "[rebuild] stale TRIAL (tag never proved healthy) — last-known-good is running; record -> REVERTED"
        );
        crate::println!("[DEMO 95] PASS: stale TRIAL reverted to last-known-good");
    }
}

/// Current record state (for the test feeder). None = no record / no disk.
pub fn current_state() -> Option<u8> {
    load_record().map(|r| r.state)
}

/// Health gate passed: TRIAL -> HEALTHY. Internal (kernel task) path —
/// not a syscall; only the trial kernel itself can mark itself healthy.
pub fn mark_healthy() {
    let Some(mut rec) = load_record() else { return };
    if rec.state != SRBL_TRIAL {
        return;
    }
    rec.state = SRBL_HEALTHY;
    if store_record(&rec) {
        crate::println!("[rebuild] health gate PASS — record -> HEALTHY");
    }
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------

fn slot_base(slot: u8) -> u64 {
    if slot == 0 { SLOT_A_BASE } else { SLOT_B_BASE }
}

/// Which slot we are running from, inferred from the record (see design §3:
/// the initial image is conventionally slot A; the trial/promoted slot is
/// trial_slot; a stale TRIAL means we're the OTHER slot).
fn running_slot(rec: Option<&SlotRecord>) -> u8 {
    match rec {
        Some(r) => match r.state {
            SRBL_TRIAL => {
                if trial_active() { r.trial_slot } else { 1 - r.trial_slot }
            }
            SRBL_HEALTHY | SRBL_PROMOTED => r.trial_slot,
            SRBL_REVERTED => 1 - r.trial_slot,
            _ => 0, // EMPTY/STAGED: initial image is slot A by convention
        },
        None => 0,
    }
}

/// Hash `bytes` bytes starting at `lba`, streaming in CHUNK pieces.
fn hash_region(lba: u64, bytes: u64) -> Option<[u8; 32]> {
    let d = dev()?;
    let mut h = Sha256::new();
    let mut buf = alloc::vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < bytes {
        let want = core::cmp::min(CHUNK as u64, bytes - done);
        let padded = (want as usize).div_ceil(SECTOR) * SECTOR;
        d.read_blocks(lba + done / SECTOR as u64, &mut buf[..padded]).ok()?;
        h.update(&buf[..want as usize]);
        done += want;
    }
    Some(h.finalize())
}

// ---------------------------------------------------------------------------
// Command surface (SYS_REBUILD backing)
// ---------------------------------------------------------------------------

pub const REBUILD_OP_STATUS: u64 = 1;
pub const REBUILD_OP_STAGE: u64 = 2;
pub const REBUILD_OP_BOOT_NEXT: u64 = 3;
pub const REBUILD_OP_KEEP: u64 = 4;
pub const REBUILD_OP_REVERT: u64 = 5;

fn print_status() {
    match load_record() {
        None => crate::println!("rebuild: no slot record (no self-rebuild in progress)"),
        Some(r) => {
            crate::println!(
                "rebuild: state={} generation={} trial_slot={} image_len={}",
                state_name(r.state),
                r.generation,
                r.trial_slot,
                r.image_len
            );
            crate::print!("rebuild: candidate sha256 = ");
            for b in &r.image_sha256[..8] {
                crate::print!("{:02x}", b);
            }
            crate::println!("…");
            crate::println!(
                "rebuild: running slot {} ({})",
                running_slot(Some(&r)),
                if trial_active() { "TRIAL candidate" } else { "last-known-good" }
            );
        }
    }
}

/// Stage the drop-zone payload into the inactive slot region, hashing as we
/// go; record -> STAGED with the hash bound. Streaming, no giant buffers.
fn do_stage() -> u64 {
    let Some(d) = dev() else {
        crate::println!("rebuild: stage: no virtio0");
        return u64::MAX;
    };
    // Drop-zone header.
    let mut head = [0u8; SECTOR];
    if d.read_blocks(DROP_ZONE_LBA, &mut head).is_err() || &head[0..4] != SRBI_MAGIC {
        crate::println!("rebuild: stage: no candidate in the drop zone (SRBI missing)");
        return u64::MAX;
    }
    let len = u64::from_le_bytes(head[8..16].try_into().unwrap());
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&head[16..32]);
    if len == 0 || len > SLOT_CAPACITY_SECTORS * SECTOR as u64 {
        crate::println!(
            "rebuild: stage: candidate {} bytes exceeds slot capacity — refused before sector 1",
            len
        );
        return u64::MAX;
    }
    let rec = load_record();
    let target = 1 - running_slot(rec.as_ref());
    crate::println!(
        "rebuild: staging {} bytes from drop zone -> slot {} …",
        len,
        target
    );

    // Stream: drop zone -> slot region, hashing the logical stream.
    let mut h = Sha256::new();
    let mut buf = alloc::vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < len {
        let want = core::cmp::min(CHUNK as u64, len - done);
        let padded = (want as usize).div_ceil(SECTOR) * SECTOR;
        let sector_off = done / SECTOR as u64;
        if d
            .read_blocks(DROP_ZONE_LBA + 1 + sector_off, &mut buf[..padded])
            .is_err()
        {
            crate::println!("rebuild: stage: drop-zone read failed at byte {}", done);
            return u64::MAX;
        }
        if d
            .write_blocks(slot_base(target) + sector_off, &buf[..padded])
            .is_err()
        {
            crate::println!("rebuild: stage: slot write failed at byte {}", done);
            return u64::MAX;
        }
        h.update(&buf[..want as usize]);
        done += want;
        if done % (16 * CHUNK as u64) == 0 || done == len {
            crate::println!("rebuild: stage: {} / {} bytes", done, len);
        }
    }
    let hash = h.finalize();

    // Verify what actually landed on the disk (torn write detection).
    let Some(readback) = hash_region(slot_base(target), len) else {
        crate::println!("rebuild: stage: readback hash failed");
        return u64::MAX;
    };
    if readback != hash {
        crate::println!("rebuild: stage: readback hash MISMATCH — not staged");
        return u64::MAX;
    }

    let mut next = rec.unwrap_or_else(SlotRecord::empty);
    next.state = SRBL_STAGED;
    next.trial_slot = target;
    next.trial_tag = tag;
    next.image_len = len;
    next.image_sha256 = hash;
    if !store_record(&next) {
        crate::println!("rebuild: stage: slot record write failed");
        return u64::MAX;
    }
    crate::print!("rebuild: staged slot {} ({} bytes, sha256 ", target, len);
    for b in &hash[..8] {
        crate::print!("{:02x}", b);
    }
    crate::println!("…) — `rebuild boot-next` to arm a trial");
    0
}

/// Arm the trial: re-verify the staged image byte-for-byte (a torn/corrupted
/// slot is caught HERE, before any reboot), then the hash-bound human gate,
/// then record -> TRIAL.
fn do_boot_next() -> u64 {
    let Some(rec) = load_record() else {
        crate::println!("rebuild: boot-next: no slot record");
        return u64::MAX;
    };
    if rec.state != SRBL_STAGED {
        crate::println!(
            "rebuild: boot-next: state is {}, need STAGED",
            state_name(rec.state)
        );
        return u64::MAX;
    }
    crate::println!("rebuild: re-verifying staged image ({} bytes)…", rec.image_len);
    let Some(hash) = hash_region(slot_base(rec.trial_slot), rec.image_len) else {
        crate::println!("rebuild: boot-next: re-verify read failed");
        return u64::MAX;
    };
    if hash != rec.image_sha256 {
        crate::println!("rebuild: boot-next: staged image hash MISMATCH — refusing to arm");
        crate::println!("[DEMO 96] PASS: torn candidate rejected at vouch");
        return u64::MAX;
    }
    let mut prompt = alloc::string::String::from("  Boot trial kernel slot ");
    prompt.push_str(if rec.trial_slot == 0 { "A" } else { "B" });
    prompt.push_str(" sha256=");
    for b in &rec.image_sha256[..8] {
        prompt.push_str(&alloc::format!("{:02x}", b));
    }
    prompt.push_str("…? [y/N] ");
    let (approved, tty) = crate::demo_approval_prompt(&prompt, 18600);
    if !approved {
        crate::println!("[AUDIT] DENY trial boot reason=denied_or_timeout (fail-fast)");
        return u64::MAX;
    }
    crate::println!(
        "[AUDIT] APPROVE trial boot slot={} sha256bound=yes by=human tty={}",
        rec.trial_slot,
        tty
    );
    let mut next = rec.clone();
    next.state = SRBL_TRIAL;
    if !store_record(&next) {
        crate::println!("rebuild: boot-next: slot record write failed");
        return u64::MAX;
    }
    crate::println!("rebuild: boot-next armed — next boot trials slot {}", rec.trial_slot);
    0
}

/// Human keep: HEALTHY -> PROMOTED (console + gate).
fn do_keep() -> u64 {
    let Some(rec) = load_record() else {
        crate::println!("rebuild: keep: no slot record");
        return u64::MAX;
    };
    if rec.state != SRBL_HEALTHY {
        crate::println!(
            "rebuild: keep: state is {}, need HEALTHY (trial must prove itself first)",
            state_name(rec.state)
        );
        return u64::MAX;
    }
    let (approved, tty) =
        crate::demo_approval_prompt("  Keep trial kernel as last-known-good? [y/N] ", 18600);
    if !approved {
        crate::println!("[AUDIT] DENY keep reason=denied_or_timeout (fail-fast)");
        return u64::MAX;
    }
    crate::println!("[AUDIT] APPROVE keep slot={} by=human tty={}", rec.trial_slot, tty);
    let mut next = rec.clone();
    next.state = SRBL_PROMOTED;
    if !store_record(&next) {
        return u64::MAX;
    }
    crate::println!("rebuild: PROMOTED — slot {} is the new last-known-good", rec.trial_slot);
    crate::println!("[DEMO 94] PASS: candidate staged, trialed, healthy, human-promoted");
    0
}

/// Human revert: any in-progress state -> REVERTED.
fn do_revert() -> u64 {
    let Some(rec) = load_record() else {
        crate::println!("rebuild: revert: no slot record");
        return u64::MAX;
    };
    let mut next = rec.clone();
    next.state = SRBL_REVERTED;
    if !store_record(&next) {
        return u64::MAX;
    }
    crate::println!("rebuild: REVERTED — next boot selects last-known-good");
    0
}

/// SYS_REBUILD backing. Console gate for mutations is at the dispatcher.
pub fn run_rebuild(op: u64) -> u64 {
    match op {
        REBUILD_OP_STATUS => {
            print_status();
            0
        }
        REBUILD_OP_STAGE => do_stage(),
        REBUILD_OP_BOOT_NEXT => do_boot_next(),
        REBUILD_OP_KEEP => do_keep(),
        REBUILD_OP_REVERT => do_revert(),
        _ => {
            crate::println!("rebuild: unknown op {}", op);
            u64::MAX
        }
    }
}
