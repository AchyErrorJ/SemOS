//! Minimal read-only FAT32 reader — M27 DEMO 80 Stage 2.
//!
//! Just enough to find one file in the root directory by its 8.3 short name and
//! stream its bytes off a [`BlockDevice`]. Used by `flash-sysroot` to pull
//! `sysroot.img` off a FAT-formatted USB stick and copy it onto the SATA disk.
//!
//! Supports both an MBR-partitioned volume (the usual USB layout: one FAT32
//! partition, type 0x0B/0x0C) and a "superfloppy" volume (FAT BPB at LBA 0).
//! Long filenames (LFN) are skipped; matching is on the upper-case 8.3 entry,
//! which is present even for lower-case files that fit 8.3 (e.g. SYSROOT.IMG).

use crate::drivers::traits::BlockDevice;

const SECTOR: usize = 512;
const ATTR_LFN: u8 = 0x0F;
const ATTR_DIRECTORY: u8 = 0x10;
const EOC: u32 = 0x0FFF_FFF8; // end-of-chain marker (>= this = last cluster)

pub struct Fat32<'a> {
    dev: &'a dyn BlockDevice,
    spc: u32,        // sectors per cluster
    fat_start: u64,  // LBA of the first FAT
    data_start: u64, // LBA of cluster 2
    root_cluster: u32,
    // One-sector FAT cache so chain walks don't re-read the FAT each hop.
    fat_cache_lba: u64,
    fat_cache: [u8; SECTOR],
}

impl<'a> Fat32<'a> {
    /// Parse the partition table / BPB on `dev` and compute the geometry.
    /// Returns `None` if it isn't a 512-byte-sector FAT32 volume.
    pub fn mount(dev: &'a dyn BlockDevice) -> Option<Fat32<'a>> {
        let mut sec0 = [0u8; SECTOR];
        dev.read_blocks(0, &mut sec0).ok()?;
        if sec0[510] != 0x55 || sec0[511] != 0xAA {
            return None;
        }

        // Superfloppy (BPB at LBA 0) advertises "FAT32   " at offset 82;
        // otherwise treat LBA 0 as an MBR and find the first FAT32 partition.
        let part_lba: u64 = if &sec0[82..90] == b"FAT32   " {
            0
        } else {
            let mut found = None;
            for i in 0..4 {
                let e = 446 + i * 16;
                if sec0[e + 4] == 0x0B || sec0[e + 4] == 0x0C {
                    found = Some(u32::from_le_bytes([
                        sec0[e + 8],
                        sec0[e + 9],
                        sec0[e + 10],
                        sec0[e + 11],
                    ]) as u64);
                    break;
                }
            }
            found?
        };

        // Read the BPB at the partition start.
        let mut bpb = [0u8; SECTOR];
        dev.read_blocks(part_lba, &mut bpb).ok()?;
        let bps = u16::from_le_bytes([bpb[11], bpb[12]]) as u32;
        let spc = bpb[13] as u32;
        let reserved = u16::from_le_bytes([bpb[14], bpb[15]]) as u32;
        let num_fats = bpb[16] as u32;
        let fat_size = u32::from_le_bytes([bpb[36], bpb[37], bpb[38], bpb[39]]);
        let root_cluster = u32::from_le_bytes([bpb[44], bpb[45], bpb[46], bpb[47]]);
        // We assume 512-byte sectors throughout (BlockDevice reads 512 B blocks).
        if bps as usize != SECTOR || spc == 0 || fat_size == 0 || root_cluster < 2 {
            return None;
        }

        let fat_start = part_lba + reserved as u64;
        let data_start = fat_start + (num_fats as u64) * (fat_size as u64);
        Some(Fat32 {
            dev,
            spc,
            fat_start,
            data_start,
            root_cluster,
            fat_cache_lba: u64::MAX,
            fat_cache: [0u8; SECTOR],
        })
    }

    fn cluster_lba(&self, cluster: u32) -> u64 {
        self.data_start + (cluster as u64 - 2) * self.spc as u64
    }

    /// Follow the FAT to the next cluster after `cluster`. Returns the next
    /// cluster value (>= EOC means end-of-chain), or `None` on read error.
    fn next_cluster(&mut self, cluster: u32) -> Option<u32> {
        let fat_off = cluster as u64 * 4;
        let lba = self.fat_start + fat_off / SECTOR as u64;
        let off = (fat_off % SECTOR as u64) as usize;
        if self.fat_cache_lba != lba {
            self.dev.read_blocks(lba, &mut self.fat_cache).ok()?;
            self.fat_cache_lba = lba;
        }
        Some(
            u32::from_le_bytes([
                self.fat_cache[off],
                self.fat_cache[off + 1],
                self.fat_cache[off + 2],
                self.fat_cache[off + 3],
            ]) & 0x0FFF_FFFF,
        )
    }

    /// Find a file in the root directory by its 11-byte 8.3 short name
    /// (upper-case, space-padded, e.g. `b"SYSROOT IMG"`). Returns
    /// `(first_cluster, size_bytes)`.
    pub fn find_file(&mut self, name83: &[u8; 11]) -> Option<(u32, u32)> {
        let mut cluster = self.root_cluster;
        loop {
            if cluster < 2 || cluster >= EOC {
                return None;
            }
            let base = self.cluster_lba(cluster);
            for s in 0..self.spc as u64 {
                let mut sec = [0u8; SECTOR];
                self.dev.read_blocks(base + s, &mut sec).ok()?;
                let mut i = 0;
                while i + 32 <= SECTOR {
                    let first = sec[i];
                    let attr = sec[i + 11];
                    if first == 0x00 {
                        return None; // 0x00 = no more entries anywhere
                    }
                    if first != 0xE5 && attr != ATTR_LFN && (attr & ATTR_DIRECTORY) == 0 {
                        if &sec[i..i + 11] == &name83[..] {
                            let hi = u16::from_le_bytes([sec[i + 20], sec[i + 21]]) as u32;
                            let lo = u16::from_le_bytes([sec[i + 26], sec[i + 27]]) as u32;
                            let size =
                                u32::from_le_bytes([sec[i + 28], sec[i + 29], sec[i + 30], sec[i + 31]]);
                            return Some(((hi << 16) | lo, size));
                        }
                    }
                    i += 32;
                }
            }
            let nc = self.next_cluster(cluster)?;
            cluster = nc;
        }
    }

    /// Stream the file's first `size` bytes in <=4 KiB chunks into `sink`.
    /// `sink(file_offset, &chunk)` returns false to abort. Returns true on a
    /// complete read.
    pub fn read_file<F: FnMut(u64, &[u8]) -> bool>(
        &mut self,
        first_cluster: u32,
        size: u32,
        mut sink: F,
    ) -> bool {
        let mut cluster = first_cluster;
        let mut remaining = size as u64;
        let mut file_off = 0u64;
        let mut buf = [0u8; 4096]; // 8 sectors per BlockDevice read
        while remaining > 0 {
            if cluster < 2 || cluster >= EOC {
                return false;
            }
            let base = self.cluster_lba(cluster);
            let mut sec_in_cluster = 0u64;
            while sec_in_cluster < self.spc as u64 && remaining > 0 {
                let avail_sectors = (self.spc as u64 - sec_in_cluster).min((buf.len() / SECTOR) as u64);
                let want = (avail_sectors * SECTOR as u64).min(remaining) as usize;
                let read_sectors = want.div_ceil(SECTOR);
                if self
                    .dev
                    .read_blocks(base + sec_in_cluster, &mut buf[..read_sectors * SECTOR])
                    .is_err()
                {
                    return false;
                }
                if !sink(file_off, &buf[..want]) {
                    return false;
                }
                file_off += want as u64;
                remaining -= want as u64;
                sec_in_cluster += read_sectors as u64;
            }
            if remaining == 0 {
                break;
            }
            match self.next_cluster(cluster) {
                Some(nc) => cluster = nc,
                None => return false,
            }
        }
        true
    }
}
