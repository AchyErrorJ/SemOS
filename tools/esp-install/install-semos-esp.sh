#!/usr/bin/env bash
set -euo pipefail

# Cautious SemOS ESP installer.
# - Does NOT repartition anything.
# - Does NOT overwrite Pop!_OS bootloader files.
# - Copies SemOS to /boot/efi/EFI/SemOS/BOOTX64.EFI and /boot/efi/kernel-x86_64.
# - Adds a firmware NVRAM boot entry with efibootmgr.
#
# Run from repo root:
#   bash tools/esp-install/install-semos-esp.sh

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMG="$REPO_ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
EXTRACT="$REPO_ROOT/tools/esp-install/extract-semos-uefi.py"
ESP="${ESP:-/boot/efi}"
LABEL="${LABEL:-SemOS}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if [[ ! -f "$IMG" ]]; then
  echo "missing image: $IMG" >&2
  echo "build first: (cd kernel-x86_64 && cargo build --release) && (cd x86_64-runner && cargo run --release)" >&2
  exit 1
fi

python3 "$EXTRACT" "$IMG" "$TMP"

printf '\nPlanned install:\n'
printf '  ESP:                 %s\n' "$ESP"
printf '  SemOS EFI loader:    %s/EFI/SemOS/BOOTX64.EFI\n' "$ESP"
printf '  SemOS kernel file:   %s/kernel-x86_64\n' "$ESP"
printf '  Firmware boot label: %s\n' "$LABEL"
printf '\nCurrent disks:\n'
lsblk -o NAME,PATH,MODEL,SIZE,TRAN,TYPE,FSTYPE,LABEL,MOUNTPOINTS

if [[ ! -d "$ESP" ]]; then
  echo "ESP mount not found: $ESP" >&2
  exit 1
fi

read -r -p "Proceed with copying files to ESP and adding efibootmgr entry? Type YES: " ans
if [[ "$ans" != "YES" ]]; then
  echo "aborted"
  exit 1
fi

# Need sudo for ESP writes and efibootmgr.
ORIGINAL_BOOTORDER="$(sudo efibootmgr | sed -n 's/^BootOrder: //p' || true)"

sudo mkdir -p "$ESP/EFI/SemOS"
TS="$(date +%Y%m%d-%H%M%S)"
# Existence checks go through sudo: the ESP is mounted root-only
# (fmask/dmask 0077) on Pop!_OS, so unprivileged -e tests are blind and the
# backups below would be silently skipped.
if sudo test -e "$ESP/EFI/SemOS/BOOTX64.EFI"; then
  sudo cp -a "$ESP/EFI/SemOS/BOOTX64.EFI" "$ESP/EFI/SemOS/BOOTX64.EFI.bak-$TS"
fi
if sudo test -e "$ESP/kernel-x86_64"; then
  sudo cp -a "$ESP/kernel-x86_64" "$ESP/kernel-x86_64.bak-$TS"
fi
sudo cp -f "$TMP/BOOTX64.EFI" "$ESP/EFI/SemOS/BOOTX64.EFI"
sudo cp -f "$TMP/kernel-x86_64" "$ESP/kernel-x86_64"
sync

# Determine disk+partition backing the ESP mount. Expected: /dev/sda + partition 1.
ESP_SRC="$(findmnt -n -o SOURCE --target "$ESP")"
PKNAME="$(lsblk -no PKNAME "$ESP_SRC" | head -n1)"
PARTNUM="$(lsblk -no PARTN "$ESP_SRC" | head -n1)"
if [[ -z "$PKNAME" || -z "$PARTNUM" ]]; then
  echo "could not determine ESP disk/partition from $ESP_SRC" >&2
  exit 1
fi
DISK="/dev/$PKNAME"

printf '\nAdding firmware boot entry:\n'
printf '  disk: %s partition: %s loader: \\EFI\\SemOS\\BOOTX64.EFI label: %s\n' "$DISK" "$PARTNUM" "$LABEL"
sudo efibootmgr --create --disk "$DISK" --part "$PARTNUM" --label "$LABEL" --loader '\EFI\SemOS\BOOTX64.EFI'

# Keep the existing default first: append SemOS to the old BootOrder instead of
# letting --create make it default. The entry still appears in the F12 menu.
SEMOS_BOOTNUM="$(sudo efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f]\).*'"$LABEL"'.*/\1/p' | tail -n1)"
if [[ -n "$SEMOS_BOOTNUM" && -n "$ORIGINAL_BOOTORDER" ]]; then
  if [[ ",$ORIGINAL_BOOTORDER," != *",$SEMOS_BOOTNUM,"* ]]; then
    sudo efibootmgr -o "$ORIGINAL_BOOTORDER,$SEMOS_BOOTNUM"
  else
    sudo efibootmgr -o "$ORIGINAL_BOOTORDER"
  fi
elif [[ -n "$SEMOS_BOOTNUM" ]]; then
  sudo efibootmgr -o "$SEMOS_BOOTNUM"
fi

printf '\nDone. Current boot entries:\n'
sudo efibootmgr -v
printf '\nNext boot: use F12 and select SemOS first. Pop!_OS should remain first/default in BootOrder.\n'
