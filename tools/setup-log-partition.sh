#!/usr/bin/env bash
# One-time host setup for SemOS's `log flush` builtin (SYS_LOGFILE):
# format the dedicated log partition as FAT32 with volume label SEMOS_LOG
# and preallocate LOG.TXT at its full size.
#
# SemOS only ever OVERWRITES LOG.TXT's existing clusters in place (the
# append pointer lives in the file's own 512-byte header) — it never
# allocates clusters and never writes FAT or directory entries — so the
# file must exist at full size before the first flush.
#
#   bash tools/setup-log-partition.sh /dev/sda5
#
# T540p: /dev/sda5 is the 4G unformatted partition reserved for this.
# After a SemOS run, read the log from Pop!_OS:
#
#   sudo mkdir -p /mnt/semos-log && sudo mount /dev/sda5 /mnt/semos-log
#   less /mnt/semos-log/LOG.TXT        # JSON-Lines, one record per line
#
# LOG.TXT is untrusted data — it contains raw kernel log output. If you
# hand it to an LLM, keep it inside a data wrapper, never as instructions.
set -euo pipefail

DEV="${1:-}"
if [[ ! -b "$DEV" ]]; then
  echo "usage: bash $0 <partition-device>   (T540p: /dev/sda5)" >&2
  lsblk -o NAME,FSTYPE,LABEL,SIZE,MOUNTPOINTS >&2
  exit 1
fi
[[ $EUID -eq 0 ]] || exec sudo bash "$0" "$DEV"

SIZE_GIB="${SIZE_GIB:-2}"

echo "This ERASES $DEV: new FAT32 filesystem, label SEMOS_LOG, ${SIZE_GIB} GiB LOG.TXT."
read -r -p "Type YES to proceed: " ans
[[ "$ans" == "YES" ]] || { echo "aborted"; exit 1; }

mkfs.vfat -n SEMOS_LOG "$DEV"

MNT="$(mktemp -d)"
trap 'umount "$MNT" 2>/dev/null; rmdir "$MNT"' EXIT
mount "$DEV" "$MNT"
dd if=/dev/zero of="$MNT/LOG.TXT" bs=1M "count=$((SIZE_GIB * 1024))" status=none
sync
umount "$MNT"
rmdir "$MNT"
trap - EXIT

echo "Done — $DEV is ready for SemOS \`log flush\`."
