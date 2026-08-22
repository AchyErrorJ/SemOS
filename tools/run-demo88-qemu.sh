#!/usr/bin/env bash
# DEMO 88 harness: same pipe-serial pattern as run-demo83.sh.
# Usage: run-demo88-qemu.sh [y|n|timeout]
ANS="${1:-y}"
IMG="$HOME/SemOS/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
LOG="$HOME/demo88-serial-$ANS.log"
rm -f /tmp/ser.in /tmp/ser.out "$LOG"
mkfifo /tmp/ser.in /tmp/ser.out

cat /tmp/ser.out > "$LOG" &
CATPID=$!

setsid nohup qemu-system-x86_64 -cpu max -m 2048 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,file="$HOME/OVMF_VARS.fd" \
  -drive format=raw,file="$IMG" \
  -drive id=sysdisk,file="$HOME/SemOS/out/sysroot.img",if=none,format=raw \
  -device ich9-ahci,id=ahci -device ide-hd,drive=sysdisk,bus=ahci.0 \
  -serial pipe:/tmp/ser -display none -no-reboot \
  < /dev/null > /dev/null 2>&1 &
QPID=$!
echo "qemu pid $QPID, log $LOG, answer=$ANS"

# Three prompts to answer this boot: /apps/calc (M2), /apps/wc (M3), /apps/head1 (M4).
seen=0
for i in $(seq 1 900); do
  n=$(grep -c "Install /apps/" "$LOG" 2>/dev/null)
  if [ "$n" -gt "$seen" ]; then
    seen=$n
    if [ "$ANS" != "timeout" ]; then
      printf "%s" "$ANS" > /tmp/ser.in
    fi
  fi
  if grep -qE "\[DEMO 88\] (PASS|FAIL)" "$LOG" 2>/dev/null; then break; fi
  if grep -q "\[DEMO 88\] FAIL" "$LOG" 2>/dev/null; then break; fi
  sleep 2
done
echo "answered $seen prompt(s) after ~$((i*2))s"

for i in $(seq 1 300); do
  grep -qE "\[DEMO 88\] (PASS|FAIL)" "$LOG" 2>/dev/null && break
  sleep 2
done

kill "$QPID" "$CATPID" 2>/dev/null
sleep 1
echo "===== verdict lines ====="
grep -E "DEMO 8[378]|AUDIT" "$LOG"
