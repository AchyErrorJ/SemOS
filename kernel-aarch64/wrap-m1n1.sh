#!/usr/bin/env bash
# Wrap the kernel into an m1n1 payload blob for a real Apple Silicon Mac.
#
# m1n1 scans a blob concatenated onto its own image and recognizes payloads by
# magic. Three things matter and none is obvious:
#
#  1. It identifies a kernel by "ARM\x64" at offset 0x38 — the arm64 Linux Image
#     header. Anything else is "Unknown payload ... No valid payload found".
#
#  2. **The kernel must be gzipped.** For an *uncompressed* inline payload m1n1
#     does `memcpy(dst, kernel, size ? size : kernel->image_size)` with size == 0,
#     so it copies image_size bytes — our whole 21 MB memory footprint — out of a
#     221 KB file, reading far off the end of the blob. Its own comment concedes
#     this: "Kernel blobs unfortunately do not have an accurate file size header,
#     so this will fail for in-line payloads." The gzip path decompresses to a
#     properly sized allocation, then reserves the extra image_size bytes so our
#     BSS is not handed to m1n1's own allocator (which would put the devicetree
#     it prepares *inside* our BSS, for us to zero on entry).
#
#  3. The DTB's root `compatible` must match the machine (apple,j314s = M1 Pro
#     14"), or m1n1 dies with "Kernel found but no devicetree".
#
# Layout: m1n1.bin + semos-Image.gz + machine.dtb
set -euo pipefail

M1N1="${1:?usage: wrap-m1n1.sh <m1n1.bin> <machine.dtb> [out.bin]}"
DTB="${2:?need a devicetree matching the target machine}"
OUT="${3:-semos-boot.bin}"

ELF="target/aarch64-unknown-none/release/semantic-os-aarch64"
[ -f "$ELF" ] || { echo "error: build first (cargo build --release)" >&2; exit 1; }

RUST_SYSROOT=$(rustc --print sysroot)
OBJCOPY=$(ls "$RUST_SYSROOT"/lib/rustlib/*/bin/llvm-objcopy 2>/dev/null | head -1)
NM=$(ls "$RUST_SYSROOT"/lib/rustlib/*/bin/llvm-nm 2>/dev/null | head -1)
[ -x "$OBJCOPY" ] && [ -x "$NM" ] || { echo "error: llvm tools not found" >&2; exit 1; }

RAW="target/semos-Image"
"$OBJCOPY" -O binary "$ELF" "$RAW"

# image_size must cover BSS and the stack, not just the bytes in the file — m1n1
# memcpy's and reserves this many bytes, and the kernel writes into all of it.
# The linker cannot bake this in: a PIE has no way to emit the absolute value.
IMAGE_END=$("$NM" "$ELF" | awk '/ _image_end$/ {print $1}')
[ -n "$IMAGE_END" ] || { echo "error: _image_end symbol missing" >&2; exit 1; }

python3 - "$RAW" "$IMAGE_END" <<'PY'
import struct, sys
raw, image_end = sys.argv[1], int(sys.argv[2], 16)
d = bytearray(open(raw, 'rb').read())
assert d[0x38:0x3c] == b'ARM\x64', "Image magic missing at 0x38"
# Round up so the loader's 2 MiB-aligned allocation always covers us.
size = (image_end + 0xFFFF) & ~0xFFFF
struct.pack_into('<Q', d, 0x10, size)
open(raw, 'wb').write(d)
print(f"  image_size = 0x{size:x} ({size // 1024} KiB); file = {len(d) // 1024} KiB")
PY

gzip -9 -c "$RAW" > "$RAW.gz"

cat "$M1N1" "$RAW.gz" "$DTB" > "$OUT"
echo "  wrote $OUT ($(wc -c < "$OUT") bytes) = m1n1 + Image.gz ($(wc -c < "$RAW.gz") bytes) + $(basename "$DTB")"
