#!/bin/bash
# DEMO 100 harness (driver forge): three boots; the harness plays the M22b
# loader (chooses the boot image) and answers the human gates 'y' over
# serial. virtio-rng is attached on every boot.
#   boot 1 (kernel A, NO rng driver): feeder stages the RNG-B candidate
#           (drop zone -> inactive slot) and arms the trial (hash-bound
#           vouch, serial 'y').
#   host:      extract the staged slot -> candidate image
#   boot 2 (kernel B, trial): TRIAL boot -> health gate (journal + ns +
#           fenced spawn + ENTROPY PROOF from the agent-written driver)
#           -> HEALTHY -> keep ('y') -> PROMOTED -> [DEMO 100] PASS
#   boot 3 (kernel B, promoted): entropy proof again after a plain reboot.
set -u
ROOT=/tmp/SemOS-main
OUTD="$ROOT/out"
DISK="$OUTD/forge-disk.img"
IMGA="$OUTD/image-a.img"
IMGB="$OUTD/image-b.img"
CAND=/tmp/forge-candidate.img
QEMUR="$HOME/qemu-root/usr"
export LD_LIBRARY_PATH="$QEMUR/lib/x86_64-linux-gnu:$HOME/qemu-root/lib/x86_64-linux-gnu"
QEMU="$QEMUR/bin/qemu-system-x86_64"
OVMF_CODE="$HOME/qemu-root/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS=/tmp/OVMF_VARS_FORGE.fd

for f in "$IMGA" "$IMGB" "$OUTD/sysroot.img"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done

rm -f "$DISK"
python3 "$ROOT/tools/make-rebuild-disk.py" "$DISK" "$IMGB" RNG-B || exit 1

boot() { # $1=boot-image $2=logfile $3=success-regex
  cp "$HOME/qemu-root/usr/share/OVMF/OVMF_VARS_4M.fd" "$OVMF_VARS"
  rm -f /tmp/forge.in /tmp/forge.out "$2"
  mkfifo /tmp/forge.in /tmp/forge.out
  cat /tmp/forge.out > "$2" &
  local catpid=$!
  setsid nohup "$QEMU" -cpu max -m 2048 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$OVMF_VARS" \
    -drive format=raw,file="$1" \
    -drive id=sysdisk,file="$OUTD/sysroot.img",if=none,format=raw \
    -device ich9-ahci,id=ahci -device ide-hd,drive=sysdisk,bus=ahci.0 \
    -drive if=virtio,format=raw,file="$DISK" \
    -device virtio-rng-pci \
    -serial pipe:/tmp/forge -display none -no-reboot \
    < /dev/null > /dev/null 2>&1 &
  local qpid=$!
  local seen=1 gate_count=0
  for i in $(seq 1 600); do
    local n
    n=$(grep -ac "trial kernel" "$2" 2>/dev/null || true); n=${n:-0}
    if [ "$n" -gt "$gate_count" ]; then
      echo "  (gate seen — answering 'y' over serial)"
      printf 'y' > /tmp/forge.in
      gate_count=$n
    fi
    if grep -aqE "$3" "$2" 2>/dev/null; then seen=0; break; fi
    if ! kill -0 $qpid 2>/dev/null; then echo "qemu exited early"; break; fi
    sleep 2
  done
  sleep 3
  kill $qpid $catpid 2>/dev/null
  wait $qpid 2>/dev/null
  return $seen
}

extract_slot() {
  python3 - "$DISK" "$1" "$2" "$3" <<'PYEOF'
import sys
disk, lba, out, n = sys.argv[1], int(sys.argv[2]), sys.argv[3], int(sys.argv[4])
with open(disk, "rb") as f:
    f.seek(lba * 512)
    data = f.read(n)
open(out, "wb").write(data)
print("extracted %d bytes from LBA %d -> %s" % (len(data), lba, out))
PYEOF
}

echo "===== boot 1 (kernel A, no rng driver): stage candidate ====="
boot "$IMGA" /tmp/forge-boot1.log 'rebuild: staged slot' \
  && echo "boot1 OK" || { echo "boot1 FAIL"; grep -a 'rebuild\|rng' /tmp/forge-boot1.log | tail; exit 1; }
grep -aq 'virtio-rng' /tmp/forge-boot1.log \
  && echo "  (NOTE: rng lines in boot1 — unexpected on kernel A)" \
  || echo "  (baseline confirmed: kernel A has no virtio-rng driver)"

echo "===== boot 1b (kernel A): arm the trial (hash-bound vouch) ====="
# The feeder types boot-next from the STAGED state; we kill right after
# 'boot-next armed' — before its scripted re-stage begins.
boot "$IMGA" /tmp/forge-boot1b.log 'boot-next armed' \
  && echo "boot1b OK" || { echo "boot1b FAIL"; grep -a 'rebuild' /tmp/forge-boot1b.log | tail; exit 1; }

echo "===== host: extract staged slot -> candidate image ====="
# The stage targets the inactive slot; kernel A conventionally runs slot A,
# so the candidate lands in slot B (LBA 524288).
extract_slot 524288 "$CAND" "$(stat -c%s "$IMGB")"

echo "===== boot 2 (kernel B trial): health gate + keep (DEMO 100) ====="
boot "$CAND" /tmp/forge-boot2.log 'DEMO 94\] PASS' \
  && echo "boot2 OK" || { echo "boot2 FAIL"; grep -a 'rebuild\|rng\|DEMO' /tmp/forge-boot2.log | tail; exit 1; }

echo "===== boot 3 (kernel B promoted): entropy proof after plain reboot ====="
boot "$CAND" /tmp/forge-boot3.log 'first fetch: 16 bytes' \
  && echo "boot3 OK" || { echo "boot3 FAIL"; grep -a 'rng' /tmp/forge-boot3.log | tail; exit 1; }

echo
grep -ah 'TRIAL boot\|health gate\|PROMOTED\|DEMO 9[45]\] PASS\|virtio-rng\|entropy' /tmp/forge-boot*.log
ok=0
! grep -aq 'virtio-rng' /tmp/forge-boot1.log \
  && grep -aq 'TRIAL boot' /tmp/forge-boot2.log \
  && grep -aq 'DEMO 100\] PASS' /tmp/forge-boot2.log \
  && grep -aq 'DEMO 94\] PASS' /tmp/forge-boot2.log \
  && grep -aq 'virtio-rng.*entropy device live' /tmp/forge-boot3.log \
  && grep -aq 'first fetch: 16 bytes' /tmp/forge-boot3.log && ok=1
echo
[ $ok -eq 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL/INCOMPLETE"
