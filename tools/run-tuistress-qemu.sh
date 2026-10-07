#!/bin/bash
# DEMO 101 harness (agent TUI stream stress): one boot of a
# `--features tui-stress-test` kernel; the feeder replays a flooding agent
# stream through the TUI and prints pixel-readback verdicts over serial.
# Uses the system QEMU + OVMF (the other run-*-qemu.sh scripts assume a
# ~/qemu-root build that this dev machine doesn't have).
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMG="$ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
SYSIMG="$ROOT/out/sysroot.img"
QEMU=qemu-system-x86_64
OVMF_CODE=/usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS=/tmp/OVMF_VARS_TUISTRESS.fd
LOG=/tmp/tuistress.log

[ -f "$IMG" ] || { echo "missing $IMG — build + wrap first:"; echo "  (cd kernel-x86_64 && cargo build --release --features tui-stress-test) && (cd x86_64-runner && cargo run --release)"; exit 1; }
[ -f "$SYSIMG" ] || { echo "missing $SYSIMG"; exit 1; }

cp /usr/share/OVMF/OVMF_VARS_4M.fd "$OVMF_VARS"
rm -f /tmp/tui.in /tmp/tui.out "$LOG"
mkfifo /tmp/tui.in /tmp/tui.out
cat /tmp/tui.out > "$LOG" &
CATPID=$!
setsid nohup "$QEMU" -cpu max -m 2048 \
  -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
  -drive if=pflash,format=raw,file="$OVMF_VARS" \
  -drive format=raw,file="$IMG" \
  -drive id=sysdisk,file="$SYSIMG",if=none,format=raw \
  -device ich9-ahci,id=ahci -device ide-hd,drive=sysdisk,bus=ahci.0 \
  -serial pipe:/tmp/tui -display none -no-reboot \
  < /dev/null > /dev/null 2>&1 &
QPID=$!

seen=1
for i in $(seq 1 150); do
  if grep -aqE 'DEMO 101\] (PASS|FAIL).*divider intact|DEMO 101\] =>' "$LOG" 2>/dev/null; then seen=0; break; fi
  if ! kill -0 $QPID 2>/dev/null; then echo "qemu exited early"; break; fi
  sleep 2
done
sleep 2
kill $QPID $CATPID 2>/dev/null
wait $QPID 2>/dev/null

echo
grep -a 'DEMO 101' "$LOG" || tail -30 "$LOG"
echo
grep -aq 'DEMO 101\] =>' "$LOG" && ! grep -aq 'DEMO 101\] FAIL' "$LOG" \
  && echo "VERDICT: PASS" || echo "VERDICT: FAIL/INCOMPLETE"
