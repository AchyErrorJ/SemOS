#!/usr/bin/env bash
# QEMU wrapper for the SemOS M14 kernel image.
# Usage: ./run-qemu.sh [bios|uefi]
# Defaults to BIOS boot and captures serial output to serial.log.
# Works in WSL/Linux (native qemu) and Git Bash on Windows (fallback).

set -e

MODE="${1:-bios}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if command -v qemu-system-x86_64 >/dev/null 2>&1; then
    QEMU="qemu-system-x86_64"
    ACCEL=(-enable-kvm -cpu host)
    # Fall back to TCG if KVM is unavailable (e.g. nested virt disabled).
    if [[ ! -e /dev/kvm ]]; then
        ACCEL=(-cpu max)
    fi
else
    QEMU="C:/Program Files/qemu/qemu-system-x86_64.exe"
    ACCEL=(-cpu max)
fi

if [[ "$MODE" == "uefi" ]]; then
    IMG="$ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
else
    IMG="$ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64-bios.img"
fi

if [[ ! -f "$IMG" ]]; then
    echo "Image not found: $IMG"
    echo "Build it first with: cd x86_64-runner && cargo run --release"
    exit 1
fi

echo "Booting SemOS ($MODE) from $IMG"
"$QEMU" "${ACCEL[@]}" \
  -drive format=raw,file="$IMG" \
  -m 1024M \
  -serial file:serial.log \
  -display none \
  -no-reboot
