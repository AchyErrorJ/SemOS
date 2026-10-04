#!/bin/bash
# DEMO 97 harness (HDA capture, audio-IN): two boots.
#   boot 1: intel-hda + hda-duplex (duplex codec: DAC+ADC) → DEMO 97 PASS
#   boot 2: intel-hda + hda-output (playback-only codec)  → DEMO 97 SKIP
set -u
ROOT=/tmp/SemOS-main
IMG="$ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
SYSIMG="$ROOT/out/sysroot.img"
QEMUR="$HOME/qemu-root/usr"
export LD_LIBRARY_PATH="$QEMUR/lib/x86_64-linux-gnu:$HOME/qemu-root/lib/x86_64-linux-gnu"
QEMU="$QEMUR/bin/qemu-system-x86_64"
OVMF_CODE="$HOME/qemu-root/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS=/tmp/OVMF_VARS_AUDIOIN.fd

[ -f "$IMG" ] || { echo "missing $IMG"; exit 1; }

boot() { # $1=codec-device $2=logfile $3=success-regex
  cp "$HOME/qemu-root/usr/share/OVMF/OVMF_VARS_4M.fd" "$OVMF_VARS"
  rm -f /tmp/ain.out /tmp/ain.ser "$2"
  mkfifo /tmp/ain.out
  cat /tmp/ain.out > "$2" &
  local catpid=$!
  setsid nohup "$QEMU" -cpu max -m 2048 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$OVMF_VARS" \
    -drive format=raw,file="$IMG" \
    -drive id=sysdisk,file="$SYSIMG",if=none,format=raw \
    -device ich9-ahci,id=ahci -device ide-hd,drive=sysdisk,bus=ahci.0 \
    -audiodev none,id=snd0 \
    -device intel-hda -device "$1",audiodev=snd0 \
    -serial file:/tmp/ain.ser -display none -no-reboot \
    < /dev/null > /dev/null 2>&1 &
  local qpid=$!
  local seen=1
  for i in $(seq 1 150); do
    if grep -aqE "$3" /tmp/ain.ser 2>/dev/null; then seen=0; break; fi
    if ! kill -0 $qpid 2>/dev/null; then echo "qemu exited early"; break; fi
    sleep 2
  done
  kill $qpid $catpid 2>/dev/null
  wait $qpid 2>/dev/null
  cat /tmp/ain.ser > "$2"
  return $seen
}

echo "===== boot 1: hda-duplex (ADC present) ====="
boot hda-duplex /tmp/audioin-duplex.log 'DEMO 97\] (PASS|FAIL|SKIP)' \
  && echo "boot1 done" || { echo "boot1 TIMEOUT"; tail -5 /tmp/audioin-duplex.log; exit 1; }

echo "===== boot 2: hda-output (informational — QEMU's hda-output also has an ADC) ====="
boot hda-output /tmp/audioin-output.log 'DEMO 97\] (PASS|FAIL|SKIP)' \
  && echo "boot2 done" || echo "boot2 TIMEOUT (non-fatal)"

echo
grep -a 'hda\]\|DEMO 97' /tmp/audioin-duplex.log
echo ---
grep -a 'hda\]\|DEMO 97' /tmp/audioin-output.log
ok=0
grep -aq 'DEMO 97\] PASS' /tmp/audioin-duplex.log && ok=1
echo
[ $ok -eq 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL/INCOMPLETE"
