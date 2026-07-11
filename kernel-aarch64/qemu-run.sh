#!/usr/bin/env bash
# Cargo runner for `cargo run`. QEMU only takes the Linux arm64 boot path — which
# places the DTB in guest RAM and passes its address in x0 — for a RAW image. Given
# an ELF it boots bare-metal and the kernel sees x0 = 0, so objcopy first.
set -euo pipefail

ELF="$1"
shift || true

RUST_SYSROOT=$(rustc --print sysroot)
OBJCOPY=""
for path in "$RUST_SYSROOT"/lib/rustlib/*/bin/llvm-objcopy; do
    if [ -x "$path" ]; then
        OBJCOPY="$path"
        break
    fi
done

if [ -z "$OBJCOPY" ]; then
    echo "error: llvm-objcopy not found under $RUST_SYSROOT/lib/rustlib/*/bin/" >&2
    echo "hint: rustup component add llvm-tools" >&2
    exit 1
fi

RAW="${ELF}.bin"
rm -f "$RAW"
"$OBJCOPY" -O binary "$ELF" "$RAW"

exec qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a53 \
    -nographic \
    -kernel "$RAW" \
    "$@"
