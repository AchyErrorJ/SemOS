#!/usr/bin/env python3
"""Extract SemOS UEFI boot files from bootloader-created FAT image.

Outputs:
  OUTDIR/BOOTX64.EFI
  OUTDIR/kernel-x86_64

This avoids loop mounts/mtools so it can run unprivileged. It knows the current
bootloader-0.11 image layout: GPT partition 1 is FAT16 and contains
/EFI/BOOT/BOOTX64.EFI plus /kernel-x86_64.
"""
import pathlib, struct, sys


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def u16(b): return struct.unpack_from('<H', b)[0]
def u32(b): return struct.unpack_from('<I', b)[0]
def u64(b): return struct.unpack_from('<Q', b)[0]


def short_name(entry):
    name = entry[:8].decode('ascii', 'replace').rstrip()
    ext = entry[8:11].decode('ascii', 'replace').rstrip()
    return name + ('.' + ext if ext else '')


def lfn_piece(entry):
    chars = entry[1:11] + entry[14:26] + entry[28:32]
    out = []
    for i in range(0, len(chars), 2):
        c = struct.unpack_from('<H', chars, i)[0]
        if c in (0x0000, 0xFFFF):
            break
        out.append(chr(c))
    return ''.join(out)


class Fat16:
    def __init__(self, data, start_lba):
        self.data = data
        self.start = start_lba * 512
        b = data[self.start:self.start + 512]
        self.bps = u16(b[11:13])
        self.spc = b[13]
        self.rsv = u16(b[14:16])
        self.fats = b[16]
        self.root_entries = u16(b[17:19])
        self.spf = u16(b[22:24])
        if self.bps != 512 or self.fats < 1 or self.root_entries == 0 or self.spf == 0:
            die('unexpected FAT16 geometry')
        self.fat_start = self.start + self.rsv * self.bps
        root_secs = ((self.root_entries * 32) + self.bps - 1) // self.bps
        self.root_start = self.start + (self.rsv + self.fats * self.spf) * self.bps
        self.root_len = root_secs * self.bps
        self.data_start = self.root_start + self.root_len

    def next_cluster(self, cl):
        return u16(self.data[self.fat_start + cl * 2:self.fat_start + cl * 2 + 2])

    def cluster_bytes(self, cl):
        pos = self.data_start + (cl - 2) * self.spc * self.bps
        return self.data[pos:pos + self.spc * self.bps]

    def read_chain(self, first_cluster, size=None):
        chunks = []
        cl = first_cluster
        guard = 0
        while 2 <= cl < 0xFFF8:
            chunks.append(self.cluster_bytes(cl))
            cl = self.next_cluster(cl)
            guard += 1
            if guard > 100000:
                die('FAT cluster chain loop')
        blob = b''.join(chunks)
        return blob[:size] if size is not None else blob

    def parse_dir(self, buf):
        entries = {}
        lfns = []
        for i in range(0, len(buf), 32):
            e = buf[i:i+32]
            if len(e) < 32 or e[0] == 0x00:
                break
            if e[0] == 0xE5:
                lfns = []
                continue
            attr = e[11]
            if attr == 0x0F:
                lfns.append(lfn_piece(e))
                continue
            name = ''.join(reversed(lfns)) if lfns else short_name(e)
            lfns = []
            first = (u16(e[20:22]) << 16) | u16(e[26:28])
            size = u32(e[28:32])
            entries[name.lower()] = (name, attr, first, size)
        return entries

    def root_entries_map(self):
        return self.parse_dir(self.data[self.root_start:self.root_start + self.root_len])

    def get_file(self, path):
        parts = [p.lower() for p in path.strip('/').split('/') if p]
        cur = self.root_entries_map()
        for part in parts[:-1]:
            if part not in cur:
                die(f'missing directory {part!r}')
            _, attr, cl, _ = cur[part]
            if not (attr & 0x10):
                die(f'{part!r} is not a directory')
            cur = self.parse_dir(self.read_chain(cl))
        leaf = parts[-1]
        if leaf not in cur:
            die(f'missing file {path}')
        _, attr, cl, size = cur[leaf]
        if attr & 0x10:
            die(f'{path} is a directory')
        return self.read_chain(cl, size)


def gpt_part1_start_lba(data):
    # protective MBR + GPT. Partition 1 entry starts at LBA 2 by convention here.
    if data[512:520] != b'EFI PART':
        die('image is not GPT')
    entries_lba = u64(data[512+72:512+80])
    entry_size = u32(data[512+84:512+88])
    e0 = data[entries_lba*512:entries_lba*512 + entry_size]
    if e0[:16] == b'\0' * 16:
        die('GPT partition 1 unused')
    return u64(e0[32:40])


def main(argv):
    if len(argv) != 3:
        print('usage: extract-semos-uefi.py semantic-os-x86_64.img OUTDIR', file=sys.stderr)
        return 2
    img = pathlib.Path(argv[1])
    out = pathlib.Path(argv[2])
    data = img.read_bytes()
    fat = Fat16(data, gpt_part1_start_lba(data))
    out.mkdir(parents=True, exist_ok=True)
    (out / 'BOOTX64.EFI').write_bytes(fat.get_file('/EFI/BOOT/BOOTX64.EFI'))
    (out / 'kernel-x86_64').write_bytes(fat.get_file('/kernel-x86_64'))
    print(f'extracted BOOTX64.EFI and kernel-x86_64 to {out}')


if __name__ == '__main__':
    raise SystemExit(main(sys.argv))
