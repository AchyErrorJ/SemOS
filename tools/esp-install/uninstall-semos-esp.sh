#!/usr/bin/env bash
set -euo pipefail

# Cautious SemOS ESP uninstaller. Removes copied SemOS files and optionally
# removes firmware boot entries whose label contains SemOS.

ESP="${ESP:-/boot/efi}"
printf 'This will remove:\n  %s/EFI/SemOS\n  %s/kernel-x86_64\n\n' "$ESP" "$ESP"
read -r -p "Proceed? Type YES: " ans
if [[ "$ans" != "YES" ]]; then
  echo "aborted"
  exit 1
fi

sudo rm -rf "$ESP/EFI/SemOS"
sudo rm -f "$ESP/kernel-x86_64"
sync

echo
echo "SemOS-related firmware entries:"
sudo efibootmgr | grep -i semos || true
echo
read -r -p "Remove all firmware entries with label matching SemOS? Type YES: " ans2
if [[ "$ans2" == "YES" ]]; then
  while read -r bootnum; do
    [[ -n "$bootnum" ]] || continue
    sudo efibootmgr -b "$bootnum" -B
  done < <(sudo efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f][0-9A-Fa-f]\).*SemOS.*/\1/ip')
fi

sudo efibootmgr -v
