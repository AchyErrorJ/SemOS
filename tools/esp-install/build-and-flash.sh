#!/usr/bin/env bash
set -euo pipefail

# One-shot: build user programs + kernel, wrap the UEFI/BIOS images, and update
# the SemOS kernel on the ESP (with a timestamped backup).
#
# Default is a *kernel-only* ESP update: the EFI loader rarely changes, so we
# replace only /boot/efi/kernel-x86_64. Use --full to (re)install the loader and
# create the firmware boot entry via install-semos-esp.sh (first-time setup).
#
# Usage (from anywhere):
#   bash tools/esp-install/build-and-flash.sh              # build + kernel-only flash
#   bash tools/esp-install/build-and-flash.sh --no-build   # flash the current image
#   bash tools/esp-install/build-and-flash.sh --build-only # build + wrap, no ESP writes
#   bash tools/esp-install/build-and-flash.sh --full       # build + full install (loader + efibootmgr)
#   bash tools/esp-install/build-and-flash.sh --dry-run    # show planned ESP actions only
#   bash tools/esp-install/build-and-flash.sh --yes        # skip the confirmation prompt
#
# Env:
#   ESP=/boot/efi            ESP mount point (default /boot/efi)
#   USER_PROGRAMS="a b c"    override the rebuilt user-program set
#   ANTHROPIC_KEY=sk-...     optional, baked into the kernel for live agent demos

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

ESP="${ESP:-/boot/efi}"
EXTRACT="$REPO_ROOT/tools/esp-install/extract-semos-uefi.py"
INSTALL="$REPO_ROOT/tools/esp-install/install-semos-esp.sh"
IMG="$REPO_ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
KERNEL_ELF="$REPO_ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64"

# User programs embedded into the kernel via include_bytes! that build with the
# plain kernel toolchain. The special trees (compiler/, cg-clif-hello, semos-cc,
# semos-rustc) are NOT rebuilt here — they are expensive / prebuilt, and the
# kernel link will fail loudly if their artifacts are missing.
DEFAULT_PROGRAMS="hello hello-std sem-demo sem-sh net-demo std-demo thread-demo vec-demo spawn-demo exfil-demo sync-demo ptr-guard-test fb-demo"
PROGRAMS="${USER_PROGRAMS:-$DEFAULT_PROGRAMS}"

DO_BUILD=1
DO_FLASH=1
FULL_INSTALL=0
DRY_RUN=0
ASSUME_YES=0

for arg in "$@"; do
  case "$arg" in
    --no-build)    DO_BUILD=0 ;;
    --build-only)  DO_FLASH=0 ;;
    --full)        FULL_INSTALL=1 ;;
    --dry-run)     DRY_RUN=1 ;;
    --yes|-y)      ASSUME_YES=1 ;;
    -h|--help)
      sed -n '3,28p' "$0"
      exit 0 ;;
    *)
      echo "unknown option: $arg" >&2
      echo "run with --help for usage" >&2
      exit 2 ;;
  esac
done

log() { printf '\n=== %s ===\n' "$*"; }

if [[ "$DO_BUILD" == "1" ]]; then
  log "Building user programs (release)"
  for p in $PROGRAMS; do
    if [[ -d "user-programs/$p" ]]; then
      echo "  - $p"
      ( cd "user-programs/$p" && cargo build --release )
    else
      echo "  ! skipping missing user program: $p" >&2
    fi
  done

  log "Building kernel (release)"
  ( cd kernel-x86_64 && cargo build --release )

  log "Wrapping bootable UEFI + BIOS images"
  ( cd x86_64-runner && cargo run --release )
fi

if [[ ! -f "$IMG" ]]; then
  echo "missing wrapped image: $IMG" >&2
  echo "run without --no-build, or build manually first." >&2
  exit 1
fi

printf '\nArtifacts:\n'
stat -c '  %n  (%s bytes, %y)' "$KERNEL_ELF" "$IMG" 2>/dev/null || true

if [[ "$DO_FLASH" == "0" ]]; then
  log "Build complete (--build-only); no ESP changes made"
  exit 0
fi

# Full install path: hand off to the loader+efibootmgr installer.
if [[ "$FULL_INSTALL" == "1" ]]; then
  log "Full install (loader + firmware boot entry) via install-semos-esp.sh"
  exec bash "$INSTALL"
fi

# Kernel-only ESP update.
if [[ ! -d "$ESP" ]]; then
  echo "ESP mount not found: $ESP (set ESP=/path)" >&2
  exit 1
fi
if [[ ! -e "$ESP/kernel-x86_64" ]]; then
  echo "No existing $ESP/kernel-x86_64 — run a --full install first." >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
python3 "$EXTRACT" "$IMG" "$TMP"

TS="$(date +%Y%m%d-%H%M%S)"
printf '\nPlanned kernel-only ESP update:\n'
printf '  ESP:            %s\n' "$ESP"
printf '  new kernel:     %s (%s bytes)\n' "$TMP/kernel-x86_64" "$(stat -c '%s' "$TMP/kernel-x86_64")"
printf '  target:         %s/kernel-x86_64\n' "$ESP"
printf '  backup:         %s/kernel-x86_64.bak-%s\n' "$ESP" "$TS"
printf '  (EFI loader unchanged; use --full to also update it)\n'

if [[ "$DRY_RUN" == "1" ]]; then
  log "Dry run — no files written"
  exit 0
fi

if [[ "$ASSUME_YES" != "1" ]]; then
  read -r -p "Proceed with copying the new kernel to the ESP? Type YES: " ans
  [[ "$ans" == "YES" ]] || { echo "aborted"; exit 1; }
fi

sudo cp -a "$ESP/kernel-x86_64" "$ESP/kernel-x86_64.bak-$TS"
sudo cp -f "$TMP/kernel-x86_64" "$ESP/kernel-x86_64"
sync

log "Done"
printf 'Reboot, press F12, choose SemOS. At the shell run: netinfo\n'
