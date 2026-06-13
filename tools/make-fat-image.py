#!/usr/bin/env python3
# Build a FAT32 disk image (MBR + one FAT32 partition) with a single file in the
# root, for QEMU-testing the SemOS read-only FAT reader (kernel-core/src/fs/fat.rs).
# Hand-rolled per the FAT32 spec (Microsoft fatgen103) for full control. Reports
# the byte checksum (sum of bytes mod 2^32) the kernel should compute on read-back.
#
# Usage: python tools/make-fat-image.py OUT.img SRC_FILE [NAME_ON_FAT_8.3]
import sys, os, struct

SECTOR = 512
SPC = 8            # sectors per cluster (4 KiB) — exercises multi-sector clusters
RESERVED = 32      # reserved sectors (boot + FSInfo + backup)
NUM_FATS = 2
PART_LBA = 2048    # 1 MiB-aligned partition start (like a Windows stick)
EOC = 0x0FFFFFFF   # end-of-chain
MEDIA = 0xF8

def name_to_83(name):
    base, _, ext = name.upper().partition(".")
    return (base[:8].ljust(8) + ext[:3].ljust(3)).encode("ascii")  # 11 bytes

def main():
    if len(sys.argv) < 3:
        print(__doc__); return 1
    out, src = sys.argv[1], sys.argv[2]
    fatname = sys.argv[3] if len(sys.argv) > 3 else os.path.basename(src)
    name83 = name_to_83(fatname)

    data = open(src, "rb").read()
    fsize = len(data)
    chk = sum(data) & 0xFFFFFFFF

    # File occupies `fclusters` clusters starting at cluster 3 (cluster 2 = root dir).
    cluster_bytes = SPC * SECTOR
    fclusters = max(1, (fsize + cluster_bytes - 1) // cluster_bytes)
    total_data_clusters = fclusters + 1 + 65536  # +root +slack, keeps it FAT32
    # FAT must hold (2 + total_data_clusters) 4-byte entries.
    fat_entries = total_data_clusters + 2
    fat_sectors = (fat_entries * 4 + SECTOR - 1) // SECTOR
    data_start = RESERVED + NUM_FATS * fat_sectors
    total_sectors = data_start + total_data_clusters * SPC

    vol = bytearray(total_sectors * SECTOR)

    # ---- Boot sector / BPB (sector 0) ----
    bs = vol  # write into the volume's sector 0
    bs[0:3] = b"\xEB\x58\x90"
    bs[3:11] = b"MSWIN4.1"
    struct.pack_into("<H", bs, 11, SECTOR)
    bs[13] = SPC
    struct.pack_into("<H", bs, 14, RESERVED)
    bs[16] = NUM_FATS
    struct.pack_into("<H", bs, 17, 0)        # root entries (0 on FAT32)
    struct.pack_into("<H", bs, 19, 0)        # total16 (0 → use total32)
    bs[21] = MEDIA
    struct.pack_into("<H", bs, 22, 0)        # FAT size 16 (0 on FAT32)
    struct.pack_into("<H", bs, 24, 0x3F)     # sectors/track
    struct.pack_into("<H", bs, 26, 0xFF)     # heads
    struct.pack_into("<I", bs, 28, PART_LBA) # hidden sectors (partition offset)
    struct.pack_into("<I", bs, 32, total_sectors)
    struct.pack_into("<I", bs, 36, fat_sectors)
    struct.pack_into("<H", bs, 40, 0)        # ext flags
    struct.pack_into("<H", bs, 42, 0)        # fs version
    struct.pack_into("<I", bs, 44, 2)        # root cluster
    struct.pack_into("<H", bs, 48, 1)        # FSInfo sector
    struct.pack_into("<H", bs, 50, 6)        # backup boot sector
    bs[64] = 0x80
    bs[66] = 0x29
    struct.pack_into("<I", bs, 67, 0x53454D53)  # volume id
    bs[71:82] = b"SEMSYS     "
    bs[82:90] = b"FAT32   "
    bs[510] = 0x55; bs[511] = 0xAA

    # ---- FSInfo (sector 1) + backup boot (sector 6) ----
    struct.pack_into("<I", vol, 1 * SECTOR + 0, 0x41615252)
    struct.pack_into("<I", vol, 1 * SECTOR + 484, 0x61417272)
    struct.pack_into("<I", vol, 1 * SECTOR + 488, 0xFFFFFFFF)
    struct.pack_into("<I", vol, 1 * SECTOR + 492, 0xFFFFFFFF)
    vol[1 * SECTOR + 510] = 0x55; vol[1 * SECTOR + 511] = 0xAA
    vol[6 * SECTOR : 6 * SECTOR + SECTOR] = bs[0:SECTOR]

    # ---- FAT(s) ----
    fat = [0] * fat_entries
    fat[0] = 0x0FFFFF00 | MEDIA
    fat[1] = EOC
    fat[2] = EOC  # root directory: one cluster
    for k in range(fclusters):
        c = 3 + k
        fat[c] = EOC if k == fclusters - 1 else (c + 1)
    fat_bytes = b"".join(struct.pack("<I", e) for e in fat)
    fat_bytes = fat_bytes.ljust(fat_sectors * SECTOR, b"\x00")
    for n in range(NUM_FATS):
        off = (RESERVED + n * fat_sectors) * SECTOR
        vol[off : off + len(fat_bytes)] = fat_bytes

    # ---- Root directory (cluster 2): one 8.3 entry for the file ----
    def cluster_off(c):
        return (data_start + (c - 2) * SPC) * SECTOR
    ent = bytearray(32)
    ent[0:11] = name83
    ent[11] = 0x20  # archive
    first = 3
    struct.pack_into("<H", ent, 20, (first >> 16) & 0xFFFF)
    struct.pack_into("<H", ent, 26, first & 0xFFFF)
    struct.pack_into("<I", ent, 28, fsize)
    ro = cluster_off(2)
    vol[ro : ro + 32] = ent  # entry, followed by zeros (end marker)

    # ---- File data (clusters 3..) ----
    fo = cluster_off(3)
    vol[fo : fo + fsize] = data

    # ---- Wrap in an MBR with one FAT32 (LBA) partition at PART_LBA ----
    total = PART_LBA * SECTOR + len(vol)
    img = bytearray(total)
    img[PART_LBA * SECTOR : PART_LBA * SECTOR + len(vol)] = vol
    e = 446
    img[e + 4] = 0x0C
    struct.pack_into("<I", img, e + 8, PART_LBA)
    struct.pack_into("<I", img, e + 12, total_sectors)
    img[510] = 0x55; img[511] = 0xAA
    open(out, "wb").write(img)

    print(f"wrote {out}: {total} bytes ({total // (1024*1024)} MiB)")
    print(f"  geometry: spc={SPC} fat_sectors={fat_sectors} data_start_lba={data_start} "
          f"part_lba={PART_LBA} clusters={total_data_clusters}")
    print(f"  file '{name83.decode()}' = {fsize} bytes, first_cluster=3, {fclusters} clusters")
    print(f"  CHECKSUM (kernel should match) = 0x{chk:08X}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
