#!/usr/bin/env python3
# M22a self-rebuild demo (docs/self-rebuild-design.md): build the 512 MiB
# demo disk and/or write a candidate image into its drop zone.
#
# Layout (must match kernel-x86_64/src/rebuild.rs):
#   LBA 8190/8191       SRBL slot record (written by the guest)
#   LBA 8192..40960     SemFS journal (bounded)
#   LBA 786432..1048576 drop zone (last 128 MiB): [4B "SRBI"][4B pad]
#                       [u64 len @ byte 8][16B tag @ byte 16]; payload at
#                       the next sector. Never place it between journal
#                       and slots — an overrun once corrupted slot A.
#   LBA 262144/524288   slot A / slot B image regions (128 MiB each)
#
# Usage:
#   make-rebuild-disk.py DISK                     create empty 512 MiB disk
#   make-rebuild-disk.py DISK CANDIDATE_IMG TAG   write drop zone payload
import os
import struct
import sys

SECTOR = 512
DISK_SECTORS = 1048576  # 512 MiB
DROP_ZONE_LBA = 786432
DROP_ZONE_PAYLOAD = DROP_ZONE_LBA + 1
SLOT_CAPACITY = 262144 * SECTOR


def main():
    if len(sys.argv) not in (2, 4):
        print("usage: make-rebuild-disk.py DISK [CANDIDATE_IMG TAG]",
              file=sys.stderr)
        return 1
    disk = sys.argv[1]
    if not os.path.exists(disk):
        with open(disk, "wb") as f:
            f.truncate(DISK_SECTORS * SECTOR)
        print("created 512 MiB disk:", disk)
    if len(sys.argv) == 4:
        img, tag = sys.argv[2], sys.argv[3]
        payload = open(img, "rb").read()
        if len(payload) > SLOT_CAPACITY:
            print("candidate exceeds slot capacity", file=sys.stderr)
            return 1
        tag16 = tag.encode()[:16].ljust(16, b"\0")
        blob = b"SRBI" + b"\0" * 4 + struct.pack("<Q", len(payload)) + tag16
        with open(disk, "r+b") as f:
            f.seek(DROP_ZONE_LBA * SECTOR)
            f.write(blob)
            f.seek(DROP_ZONE_PAYLOAD * SECTOR)
            f.write(payload)
        print("drop zone: %s (%d bytes, tag %s) -> LBA %d"
              % (os.path.basename(img), len(payload), tag, DROP_ZONE_LBA))
    return 0


if __name__ == "__main__":
    sys.exit(main())
