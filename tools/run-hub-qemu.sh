#!/bin/bash
# DEMO 98 harness (hub pipeline): two boots; host shim plays voice gateway
# (TCP :9001) + lamp (UDP :9002). QEMU slirp: guest reaches it at 10.0.2.2.
#   boot 1: feeder runs `semos update` + `semos install lights` (serial 'y'
#           at the gate) + `hub start`; shim sends "lights on"/"lights off";
#           hub dispatches intents → UDP to lamp → replies on the channel.
#           HARD KILL.
#   host:   wipe the mirror region (LBA 0..8192); journal untouched.
#   boot 2: `hub start` only — intents come back from SemFS replay; shim
#           drives the same two commands (persistence beat, no reinstall).
set -u
ROOT=/tmp/SemOS-main
IMG="$ROOT/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img"
SYSIMG="$ROOT/out/sysroot.img"
DISK="$ROOT/out/hub-semfs.img"
QEMUR="$HOME/qemu-root/usr"
export LD_LIBRARY_PATH="$QEMUR/lib/x86_64-linux-gnu:$HOME/qemu-root/lib/x86_64-linux-gnu"
QEMU="$QEMUR/bin/qemu-system-x86_64"
OVMF_CODE="$HOME/qemu-root/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS=/tmp/OVMF_VARS_HUB.fd
SHIM_LOG=/tmp/hub-shim.log

[ -f "$IMG" ] || { echo "missing $IMG"; exit 1; }
[ -f "$SYSIMG" ] || { echo "missing $SYSIMG"; exit 1; }

echo "fresh mirror+journal disk (registry at LBA 16)"
rm -f "$DISK"
python3 "$ROOT/tools/make-registry-image.py" "$DISK" || exit 1

rm -f "$SHIM_LOG"
setsid nohup python3 "$ROOT/tools/hub-shim.py" > "$SHIM_LOG" 2>&1 &
SHIM=$!
sleep 1

boot() { # $1=logfile $2=success-regex
  cp "$HOME/qemu-root/usr/share/OVMF/OVMF_VARS_4M.fd" "$OVMF_VARS"
  rm -f /tmp/hub.in /tmp/hub.out "$1"
  mkfifo /tmp/hub.in /tmp/hub.out
  cat /tmp/hub.out > "$1" &
  local catpid=$!
  setsid nohup "$QEMU" -cpu max -m 2048 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$OVMF_VARS" \
    -drive format=raw,file="$IMG" \
    -drive id=sysdisk,file="$SYSIMG",if=none,format=raw \
    -device ich9-ahci,id=ahci -device ide-hd,drive=sysdisk,bus=ahci.0 \
    -drive if=virtio,format=raw,file="$DISK" \
    -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
    -serial pipe:/tmp/hub -display none -no-reboot \
    < /dev/null > /dev/null 2>&1 &
  local qpid=$!
  local seen=1 gate_count=0
  for i in $(seq 1 600); do
    local n
    n=$(grep -ac "Install /apps/" "$1" 2>/dev/null || true); n=${n:-0}
    if [ "$n" -gt "$gate_count" ]; then
      echo "  (approval gate seen — answering 'y' over serial)"
      printf 'y' > /tmp/hub.in
      gate_count=$n
    fi
    if grep -aqE "$2" "$1" 2>/dev/null; then seen=0; break; fi
    if ! kill -0 $qpid 2>/dev/null; then echo "qemu exited early"; break; fi
    sleep 2
  done
  sleep 3
  kill $qpid $catpid 2>/dev/null  # HARD KILL
  wait $qpid 2>/dev/null
  return $seen
}

echo "===== boot 1: install lights -> hub start -> commands ====="
boot /tmp/hub-boot1.log 'DEMO 98\] PASS' \
  && echo "boot1 OK" || { echo "boot1 TIMEOUT/FAIL"; grep -a "hub\|semos-pkg\|DEMO 98" /tmp/hub-boot1.log | tail -15; kill $SHIM 2>/dev/null; exit 1; }
# let the shim finish the second command + the reply flush
sleep 12

echo "===== host: wipe mirror region; journal untouched ====="
dd if=/dev/zero of="$DISK" bs=512 count=8192 conv=notrunc status=none

echo "===== boot 2: journaled vocabulary, no reinstall ====="
boot /tmp/hub-boot2.log 'DEMO 98\] PASS' \
  && echo "boot2 OK" || { echo "boot2 TIMEOUT/FAIL"; grep -a "hub\|DEMO 98" /tmp/hub-boot2.log | tail -15; kill $SHIM 2>/dev/null; exit 1; }
sleep 12
kill $SHIM 2>/dev/null

echo
echo "--- guest beats ---"
grep -a 'hub\]\|DEMO 98\|intent registered\|installed /apps/lights' /tmp/hub-boot1.log /tmp/hub-boot2.log
echo "--- shim (host) beats ---"
grep -a 'LAMP:\|GATEWAY:' "$SHIM_LOG"
ok=0
grep -aq 'DEMO 98\] PASS' /tmp/hub-boot1.log \
  && grep -aq 'DEMO 98\] PASS' /tmp/hub-boot2.log \
  && grep -aq 'DEMO 98\] boot 2: intent vocabulary restored' /tmp/hub-boot2.log \
  && grep -aq 'LAMP: ON' "$SHIM_LOG" \
  && grep -aq 'LAMP: OFF' "$SHIM_LOG" \
  && grep -aq "GATEWAY: reply: 'OK" "$SHIM_LOG" && ok=1
echo
[ $ok -eq 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL/INCOMPLETE"
