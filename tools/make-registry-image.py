#!/usr/bin/env python3
# semos-pkg (M43/M44): build the package-mirror image.
#
# The mirror is a raw length-prefixed blob at LBA 16 on the SemOS virtio
# disk — the legacy snapshot region below the SemFS journal (LBA 8192),
# so registry and journal share one disk without overlapping:
#
#   [8B magic "SEMREG01"][u64 payload length][payload][zero pad to 512]
#
# The payload is the whole registry (index + package payloads) in the
# line-oriented format from docs/semos-pkg-design.md §1:
#
#   SEMOS-REGISTRY 1\n
#   pkg <name> <ver> kind=<lib|bin> deps=<csv|-> bytes=<N> expect=<M>\n
#   <N source bytes><M expected-stdout bytes>
#   ...
#   end\n
#
# EXPECT strings are stated explicitly here on purpose: if a package source
# drifts, the kernel's byte-exact selftest at install time is what catches
# it (that check is the point of the format).
#
# Usage: python3 tools/make-registry-image.py DISK_IMG
#   Creates DISK_IMG (32 MiB) if missing; injects/refresh the registry at
#   LBA 16. Refuses if the payload would reach the journal region.
import os
import struct
import sys

LBA = 16
JOURNAL_LBA = 8192
SECTOR = 512
MAGIC = b"SEMREG01"

PKG_DIR = os.path.join(os.path.dirname(__file__), "..",
                       "user-programs", "semos-rustc", "test-sources", "pkg")

COWSAY_OUT = (b" ______\n< moo! >\n ------\n        \\   ^__^\n"
              b"         \\  (oo)\\_______\n            (__)\\       )\\/\\\n"
              b"                ||----w |\n                ||     ||\n")
FORTUNE_LINE = b"fortune: a journaled thought survives the crash\n"
MOTD_OUT = b"message of the day:\n" + FORTUNE_LINE

# name, version, kind, deps, source file, expected selftest stdout
PACKAGES = [
    ("fortune", "1.0.0", "lib", [], "fortune.rs", b""),
    ("cowsay", "1.0.0", "bin", [], "cowsay.rs", COWSAY_OUT),
    ("motd", "1.0.0", "bin", ["fortune"], "motd.rs", MOTD_OUT),
]


def build_payload():
    out = bytearray(b"SEMOS-REGISTRY 1\n")
    for name, ver, kind, deps, src_file, expect in PACKAGES:
        src = open(os.path.join(PKG_DIR, src_file), "rb").read()
        deps_s = ",".join(deps) if deps else "-"
        out += ("pkg %s %s kind=%s deps=%s bytes=%d expect=%d\n"
                % (name, ver, kind, deps_s, len(src), len(expect))).encode()
        out += src
        out += expect
    out += b"end\n"
    return bytes(out)


def main():
    if len(sys.argv) != 2:
        print("usage: make-registry-image.py DISK_IMG", file=sys.stderr)
        return 1
    img = sys.argv[1]
    if not os.path.exists(img):
        with open(img, "wb") as f:
            f.truncate(32 * 1024 * 1024)
        print("created fresh 32 MiB disk:", img)
    payload = build_payload()
    blob = MAGIC + struct.pack("<Q", len(payload)) + payload
    padded = (len(blob) + SECTOR - 1) // SECTOR * SECTOR
    if LBA + padded // SECTOR >= JOURNAL_LBA:
        print("payload too large: would reach journal region", file=sys.stderr)
        return 1
    with open(img, "r+b") as f:
        f.seek(LBA * SECTOR)
        f.write(blob + b"\0" * (padded - len(blob)))
    print("registry: %d packages, %d payload bytes -> LBA %d (%d sectors)"
          % (len(PACKAGES), len(payload), LBA, padded // SECTOR))
    return 0


if __name__ == "__main__":
    sys.exit(main())
