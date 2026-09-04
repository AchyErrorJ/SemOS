# SemFS Journal v1 — durable write for the SUID object store

> Status: design + v1 implementation. Verification target: QEMU (AHCI raw
> disk). Companion to `THESIS.md` (provenance §, P-3) and
> `M27_DISK_SYSROOT_DESIGN.md` (the read-only blob this complements).

## 0. What this is and is not

SemOS storage is a **SUID-addressed semantic object store** in RAM
(`kernel-core/src/semantic/`) with a hierarchical path namespace layered
on top (directories are objects holding `(name, SUID)` entries) and a
read-only FAT32 bolt-on for foreign disks. `SYS_FWRITE` / `SYS_MKDIR` /
atomic rename already mutate the in-RAM store; AHCI/virtio-blk already
implement `write_blocks`. **The only missing layer is durability: nothing
ever flushes the object store to a block device.** This design adds
exactly that layer — and nothing else. It is not a POSIX filesystem, not
FAT-write, not a page cache.

## 1. Format: append-only object log

The disk is a log of full-state object records. No in-place updates, ever.

### 1.1 Layout

```
LBA 0      superblock A (one 512 B sector)
LBA 1      superblock B (one 512 B sector)
LBA 2..N   log records, 512 B aligned, appended monotonically
```

Two superblocks, ping-ponged, so a torn superblock write always leaves a
valid older one. The log region size is fixed at format time.

### 1.2 Record (v1)

```
offset  field
0       magic = "SFRO" (4 B)                     SemFS Record Object
4       format_version = 1 (u16)
6       op (u8): 1=UPSERT, 2=TOMBSTONE, 3=COMMIT (reserved, §6)
7       flags (u8): bit0 = COMMITTED (v1: always 1; §6)
8       seq (u64)        — monotonic commit sequence
16      suid_high (u64), suid_low (u64)   — 16 B SUID
32      tier (u8), owner (u8), content_type (u8), obj_flags (u8)
36      created_at (u64), modified_at (u64)  — ticks; informational
52      content_len (u32)
56      link_count (u8) + reserved (7 B)
64      links: link_count × (target SUID 16 B, relationship u8) (≤ 16)
...     content bytes (content_len)
tail    crc32 (u32, IEEE) over everything before it
padded  to 512 B multiple
```

Design choices and why:

- **Full-state UPSERT, not deltas.** Every mutation appends the object's
  complete new state. Replay is last-writer-wins by `seq` — no delta
  algebra, no partial-application crashes. The 2 MiB `MAX_CONTENT_SIZE`
  ceiling makes full-state cheap enough at hub write volumes.
- **TOMBSTONE** for delete: an object with a tombstone seq is absent
  after replay. Rename = UPSERT of the new directory object content (the
  namespace stores entries in directory objects, so rename needs no
  special record).
- **CRC tail-gated replay.** Replay applies records in order and *stops
  at the first record whose CRC fails or whose tail is unwritten*; the
  log head is reset there. A torn append therefore loses at most the
  in-flight mutation. This is the whole crash story: the write path is a
  single sequential append, so the failure mode set is {clean, torn tail}.
- **SUIDs on disk are the SUIDs in RAM.** Content-addressed identity
  survives reboot — the vouch model (THESIS.md §3 I-8, P-3) can bind a
  hash-approved artifact at `/apps/greet` across power cycles, because
  the bytes that come back after replay are byte-identical.

### 1.3 Superblock

```
0   magic = "SFSB" (4 B)
4   format_version = 1 (u16), clean_shutdown (u8), reserved (u8)
8   generation (u64)        — higher wins between A and B
16  log_start_lba (u64), log_len_sectors (u64)
32  replayed_seq_high_water (u64)   — last seq known durably applied
40  crc32 over the above
```

Updated on clean shutdown and by future GC. Boot picks the valid
superblock with the higher generation; if both CRC-fail, the disk is
treated as unformatted (v1: log scan from `log_start_lba` with zero
trust in the superblock is the fallback).

## 2. Write path (write-through)

Every object-store mutation — `registry::insert` / `registry::remove`,
which is where `SYS_FWRITE`, `SYS_MKDIR`, rename, and the `/apps`
install path all bottom out — synchronously:

1. serializes the object into a record (`op=UPSERT`, `COMMITTED=1`),
2. appends it at the log head via `BlockDevice::write_blocks`,
3. only then returns success to the caller.

**If the disk write fails, the in-RAM mutation is rolled back** (the
insert is removed / the old object restored) and the syscall returns
error. Durable and visible are the same event; there is no window where
the user sees state the disk doesn't have. That is the definition of
write-through here, and it's the right default for a hub whose workloads
are small and whose correctness is the product.

Sync points: the AHCI write path already waits for command completion
(`rw_one` polls), so "write returned Ok" means the device accepted the
data. v1 does not issue FLUSH CACHE explicitly — noted as a v1.1
hardening item (QEMU raw files and the target SSD both make this low-risk
in practice, but the thesis says what it says about declared trust).

## 3. Boot path (mount/replay)

After block-device registration, before any userland write can happen:

1. probe superblocks A/B, pick winner (valid CRC, higher generation);
2. scan the log from `log_start_lba`: decode header → CRC → apply
   (UPSERT into registry / TOMBSTONE removes), tracking high-water seq;
3. stop at first bad/absent record; set log head there;
4. boot-seeded objects (ramfs built-ins) and replayed objects coexist:
   the journal wins on SUID collision, because replay runs after seeding
   and its `seq` is above anything seeded this boot. Seeded-but-never-
   persisted objects stay RAM-only until first mutation (then they join
   the log). This matches M22c: `/apps` persists, system seeds refresh.

v1 capacity: log region sized at format (QEMU image: 32 MiB). GC
(segment compaction) is explicitly out of scope for v1; the doc section
exists so the format doesn't preclude it (§6).

## 4. Security posture (per THESIS.md four surface questions)

- **New syscall?** No. One new internal hook at registry insert/remove,
  plus a boot mount. `SYS_FWRITE`'s existing gates (pointer guard,
  tier checks, MAX_FILE_CONTENT) are unchanged and remain the only way
  userland reaches the write path.
- **Smallest shape?** The journal only ever stores what the registry
  already holds. No new user-visible capability.
- **Capability check?** Inherited from the existing mutation paths. The
  journal itself is kernel-internal; no Ring-3 address can name it.
- **Blast radius?** A journal bug can corrupt persisted state but cannot
  widen any tier: replay re-inserts objects with their *stored* tier,
  and the stored tier was itself set by a gated mutation. Mount runs
  before userland; replay failures degrade to read-only-RAM boot, not to
  a crash loop.

## 5. Failure modes verified by DEMO

| case | expected |
|---|---|
| write file, reboot | byte-exact content + same SUID after replay |
| rename (dir object rewrite), reboot | new path resolves, old doesn't |
| delete, reboot | object stays gone (TOMBSTONE) |
| host corrupts final log sector, reboot | replay truncates at corruption; all prior records intact; new appends continue from truncation point |
| both superblocks' CRC broken | mount as unformatted, no panic |

## 6. Group-commit readiness (the "be prepared" clause)

Write-through is v1's policy, but the format carries the hooks for a
policy change without a format break:

- `op=3 COMMIT` record (reserved): a marker carrying the `seq` it
  commits. Replay under group-commit would apply only records covered by
  a later COMMIT and drop the uncommitted tail.
- `flags.COMMITTED` bit per record: v1 sets it on every record; a
  deferred policy would buffer records uncommitted and flip visibility at
  COMMIT.
- `CommitPolicy` enum in the implementation (`WriteThrough` now,
  `GroupCommit { interval }` later) — the append path already funnels
  through one function, so the policy switch is local.

What v1 deliberately does NOT build: the in-memory dirty buffer, the
timer/quiesce trigger, COMMIT records on disk. Those land only if
write-through ever shows up in a measurement that matters.

## 7. QEMU test plan

Boot disk layout: existing UEFI image + sysroot disk, plus a third raw
drive (`out/semfs.img`, 32 MiB zero-filled) on the AHCI bus. Kernel picks
the journal disk by probe order (the disk that is neither the boot disk
nor the sysroot blob disk — v1: explicit "last AHCI port" rule,
documented; real-hardware partitioning is a later milestone).

DEMO 91 (persistence): boot 1 — sem-sh writes `/apps/journal-test.txt`
with known content, sync-return, hard-kill QEMU (no clean shutdown);
boot 2 — a boot-time check reads the path and verifies byte-exact
content and SUID equality. `[DEMO 91] PASS/FAIL`.

DEMO 92 (torn tail): between boots, the host truncates/corrupts the
final sector of `semfs.img`; boot must replay cleanly up to the last
good record and keep accepting appends. `[DEMO 92] PASS/FAIL`.

Both run headless in QEMU with the existing serial harness pattern.
Machine (T540p) validation stays on `main` per the branch split.

## 8. Implementation notes (v1, QEMU-verified 2026-09-05)

Lessons that only surfaced under test:

1. **CRC must cover the logical byte stream, not sector flushes.** The
   first append implementation updated the CRC only when a 512 B buffer
   flushed — every sub-512 B record was persisted with CRC 0 and replay
   (correctly) rejected the entire log. Two-pass structure now: CRC
   header+links+content, then stream. The replay side catching this is
   the format working as designed.
2. **Mount ordering is load-bearing.** Seeds created before mount were
   journaled *as directory entries* (when a later mutation rewrote the
   root dir) but never journaled *as objects* — after reboot the
   directory referenced a SUID the registry didn't have, and reads of
   `/hello.rs` failed. The journal now mounts immediately after
   `init_global_registry()`, before `Namespace::init()` and all seeds.
   Every object that a persisted directory can reference is itself
   persisted. Rule: **never let a persisted directory reference an
   unpersisted object.**
3. **Write-through is four hook sites, not one.** `registry::insert` /
   `remove` cover create/delete, but content mutation also flows through
   `SYS_FWRITE`'s splice path and the namespace's `write_file` /
   `add_child` / `remove_child` (all `get_mut`-based). Each appends the
   new state BEFORE assigning it (`journal::on_update`) — durable
   strictly before visible, with the in-RAM value untouched on disk
   error.
4. **fsync collapses to a no-op** when the journal is mounted — the
   legacy whole-tree `Namespace::save` snapshot remains only as the
   no-journal fallback.

Verification (branch selfdev80-thesis, QEMU 7.2 UEFI + virtio-blk
journal disk, three-boot protocol, hard kills):

| demo | result |
|---|---|
| DEMO 91 | PASS: marker A byte-exact after hard-kill reboot |
| DEMO 92 | PASS: host-corrupted record CRC → replay truncated at the record, A intact, appends continue (marker C) |
| DEMO 80 (coexistence) | PASS: on-device compile reading `/hello.rs` from the replayed namespace; `/tmp/hello.elf` itself journaled |
| no-journal baseline | boots RAM-only when virtio0 absent (graceful) |
