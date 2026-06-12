#!/usr/bin/env python3
# M27 DEMO 80 — pack the host-built sysroot crate metadata into a raw, sector-
# aligned blob image for a SATA/AHCI disk the kernel reads at boot (Layer A).
#
# Blob layout (512-byte sectors, little-endian):
#   sector 0 (header):
#     +0   magic   b"SEMSYSR1" (8 bytes)
#     +8   count   u32   number of file records
#     +12  _resv   u32   0
#     +16  records[count], 80 bytes each:
#            +0   name  [u8; 64]  utf-8, NUL-padded (e.g. "libcore-<hash>.rmeta")
#            +64  lba   u64       absolute LBA on the disk where this file starts
#            +72  len   u64       file length in bytes
#   then each file's bytes at its `lba`, padded up to a sector boundary.
#
# The kernel reads LBA 0, checks the magic, and serves files via
# SYS_SYSROOT_INFO / SYS_SYSROOT_READ. Up to 6 records fit in the 512-byte header.
#
# Usage:
#   python tools/pack-sysroot-blob.py OUT.img NAME1=FILE1 [NAME2=FILE2 ...]
# Example:
#   python tools/pack-sysroot-blob.py sysroot.img \
#     libcore-53344cc650ffcdf9.rmeta=.../deps/libcore-53344cc650ffcdf9.rmeta \
#     libcompiler_builtins-fb74582bf62b1baa.rmeta=.../deps/libcompiler_builtins-fb74582bf62b1baa.rmeta
import struct, sys, os

SECTOR = 512
MAGIC = b"SEMSYSR1"
NAME_LEN = 64
REC_LEN = 80  # 64 + 8 + 8
MAX_RECORDS = (SECTOR - 16) // REC_LEN  # 6

def sectors(n):  # bytes -> sector count (ceil)
    return (n + SECTOR - 1) // SECTOR

def main(argv):
    if len(argv) < 3:
        print(__doc__); return 1
    out = argv[1]
    entries = []
    for spec in argv[2:]:
        name, _, path = spec.partition("=")
        if not path:
            print(f"bad spec (need NAME=FILE): {spec}"); return 1
        data = open(path, "rb").read()
        nb = name.encode()
        if len(nb) > NAME_LEN:
            print(f"name too long (>{NAME_LEN}): {name}"); return 1
        entries.append((nb, data))
    if len(entries) > MAX_RECORDS:
        print(f"too many files (>{MAX_RECORDS})"); return 1

    # Layout: header at sector 0, files sector-aligned after it.
    lba = 1
    records = []
    for nb, data in entries:
        records.append((nb, lba, len(data)))
        lba += sectors(len(data))
    total_sectors = lba

    img = bytearray(total_sectors * SECTOR)
    # header
    img[0:8] = MAGIC
    struct.pack_into("<II", img, 8, len(entries), 0)
    off = 16
    for nb, file_lba, file_len in records:
        name_field = nb + b"\x00" * (NAME_LEN - len(nb))
        img[off:off+NAME_LEN] = name_field
        struct.pack_into("<QQ", img, off + NAME_LEN, file_lba, file_len)
        off += REC_LEN
    # file bodies
    for (nb, data), (_, file_lba, _) in zip(entries, records):
        start = file_lba * SECTOR
        img[start:start+len(data)] = data

    with open(out, "wb") as f:
        f.write(img)

    print(f"wrote {out}: {total_sectors} sectors ({total_sectors*SECTOR} bytes)")
    for nb, file_lba, file_len in records:
        print(f"  {nb.decode():40s} lba={file_lba:<8d} len={file_len}")
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv))
