#!/usr/bin/env bash
# DEMO 83 harness: boot SemOS in QEMU with bidirectional serial FIFOs,
# answer the "Install /apps/calc? [y/N]" prompt with $1 (y|n|timeout),
# and dump the verdict lines. Usage: run-demo83.sh [y|n|timeout]
ANS="${1:-y}"
IMG="$HOME/SemOS/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
LOG="$HOME/demo83-serial-$ANS.log"
rm -f /tmp/ser.in /tmp/ser.out "$LOG"; mkfifo /tmp/ser.in /tmp/ser.out

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

answered=0
for i in $(seq 1 600); do
  if grep -q "Install /apps/calc?" "$LOG" 2>/dev/null; then
    if [ "$ANS" != "timeout" ]; then
      printf "%s" "$ANS" > /tmp/ser.in
    fi
    answered=1
    break
  fi
  if grep -q "\[DEMO 83\] FAIL" "$LOG" 2>/dev/null; then break; fi
  sleep 2
done
echo "prompt seen, answered=$answered after ~$((i*2))s"

for i in $(seq 1 300); do
  grep -qE "\[DEMO 83\] (PASS|FAIL)" "$LOG" 2>/dev/null && break
  sleep 2
done

kill "$QPID" "$CATPID" 2>/dev/null
sleep 1
echo "===== verdict lines ====="
grep -E "DEMO 8[03]|AUDIT" "$LOG"
