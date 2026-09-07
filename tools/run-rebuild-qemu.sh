#!/bin/bash
# DEMO 94/95/96 harness (M22a self-rebuild): six boots; the harness plays
# the future M22b chainloader by choosing which UEFI image boots, and
# answers the human gates 'y' over the serial pipe.
#   boot1 (A): rebuild status + stage (drop zone B-image -> slot B) -> STAGED
#   host:      corrupt one byte inside the slot-B region
#   boot2 (A): boot-next -> hash mismatch, refused (DEMO 96) -> re-stage ->
#              boot-next -> gate 'y' -> TRIAL armed
#   boot3 (candidate B): TRIAL boot -> health gate -> HEALTHY -> keep gate
#              'y' -> PROMOTED (DEMO 94)
#   boot4 (B, promoted): drop zone <- sabotage image; stage -> slot A,
#              boot-next -> TRIAL armed
#   boot5 (sabotage): trial boot, health gate deliberately FAILS
#   boot6 (B): stale TRIAL -> auto-REVERTED (DEMO 95)
set -u
ROOT=/tmp/SemOS-main
OUTD="$ROOT/out"
DISK="$OUTD/rebuild-disk.img"
IMGA="$OUTD/image-a.img"
IMGB="$OUTD/image-b.img"
IMGSAB="$OUTD/image-sab.img"
CAND=/tmp/rebuild-candidate.img
SAB=/tmp/rebuild-sabotage.img
QEMUR="$HOME/qemu-root/usr"
export LD_LIBRARY_PATH="$QEMUR/lib/x86_64-linux-gnu:$HOME/qemu-root/lib/x86_64-linux-gnu"
QEMU="$QEMUR/bin/qemu-system-x86_64"
OVMF_CODE="$HOME/qemu-root/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS=/tmp/OVMF_VARS_REBUILD.fd

for f in "$IMGA" "$IMGB" "$IMGSAB" "$OUTD/sysroot.img"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done

python3 "$ROOT/tools/make-rebuild-disk.py" "$DISK" "$IMGB" REBUILD-B || exit 1

boot() { # $1=boot-image $2=logfile $3=success-regex
  cp "$HOME/qemu-root/usr/share/OVMF/OVMF_VARS_4M.fd" "$OVMF_VARS"
  rm -f /tmp/rb.in /tmp/rb.out "$2"
  mkfifo /tmp/rb.in /tmp/rb.out
  cat /tmp/rb.out > "$2" &
  local catpid=$!
  setsid nohup "$QEMU" -cpu max -m 2048 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$OVMF_VARS" \
    -drive format=raw,file="$1" \
    -drive id=sysdisk,file="$OUTD/sysroot.img",if=none,format=raw \
    -device ich9-ahci,id=ahci -device ide-hd,drive=sysdisk,bus=ahci.0 \
    -drive if=virtio,format=raw,file="$DISK" \
    -serial pipe:/tmp/rb -display none -no-reboot \
    < /dev/null > /dev/null 2>&1 &
  local qpid=$!
  local seen=1 gate_count=0
  for i in $(seq 1 600); do
    local n
    n=$(grep -ac "trial kernel" "$2" 2>/dev/null || true); n=${n:-0}
    if [ "$n" -gt "$gate_count" ]; then
      echo "  (gate seen — answering 'y' over serial)"
      printf 'y' > /tmp/rb.in
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

extract_slot() { # $1=slot-base-lba $2=out-file $3=byte-len
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

echo "===== boot 1 (A): stage candidate -> slot B ====="
boot "$IMGA" /tmp/rb-boot1.log 'rebuild: staged slot' \
  && echo "boot1 OK" || { echo "boot1 FAIL"; grep -a rebuild /tmp/rb-boot1.log | tail; exit 1; }

echo "===== host: corrupt one byte in slot B ====="
python3 - "$DISK" <<'PYEOF'
import sys
off = 524288 * 512 + 1024 * 1024
with open(sys.argv[1], "r+b") as f:
    f.seek(off); b = f.read(1); f.seek(off); f.write(bytes([b[0] ^ 0xFF]))
print("flipped a byte at slot-B +1MiB")
PYEOF

echo "===== boot 2 (A): DEMO 96 refuse, re-stage, arm trial ====="
boot "$IMGA" /tmp/rb-boot2.log 'boot-next armed' \
  && echo "boot2 OK" || { echo "boot2 FAIL"; grep -a 'rebuild\|DEMO 96' /tmp/rb-boot2.log | tail; exit 1; }
grep -aq 'DEMO 96\] PASS' /tmp/rb-boot2.log && echo "  (DEMO 96 PASS seen)"

echo "===== host: extract slot B -> candidate image ====="
extract_slot 524288 "$CAND" "$(stat -c%s "$IMGB")"

echo "===== boot 3 (candidate B): trial -> health -> keep (DEMO 94) ====="
boot "$CAND" /tmp/rb-boot3.log 'DEMO 94\] PASS' \
  && echo "boot3 OK" || { echo "boot3 FAIL"; grep -a 'rebuild\|DEMO' /tmp/rb-boot3.log | tail; exit 1; }

echo "===== host: drop zone <- sabotage image ====="
python3 "$ROOT/tools/make-rebuild-disk.py" "$DISK" "$IMGSAB" REBUILD-SAB || exit 1

echo "===== boot 4 (B promoted): stage sabotage -> slot A, arm trial ====="
boot "$CAND" /tmp/rb-boot4.log 'boot-next armed' \
  && echo "boot4 OK" || { echo "boot4 FAIL"; grep -a rebuild /tmp/rb-boot4.log | tail; exit 1; }

echo "===== host: extract slot A -> sabotage image ====="
extract_slot 262144 "$SAB" "$(stat -c%s "$IMGSAB")"

echo "===== boot 5 (sabotage): health gate deliberately fails ====="
boot "$SAB" /tmp/rb-boot5.log 'health gate FAILED' \
  && echo "boot5 OK" || { echo "boot5 FAIL"; grep -a rebuild /tmp/rb-boot5.log | tail; exit 1; }

echo "===== boot 6 (B): stale TRIAL auto-reverts (DEMO 95) ====="
boot "$CAND" /tmp/rb-boot6.log 'DEMO 95\] PASS' \
  && echo "boot6 OK" || { echo "boot6 FAIL"; grep -a 'rebuild\|DEMO' /tmp/rb-boot6.log | tail; exit 1; }

echo
ok=0
grep -aq 'DEMO 96\] PASS' /tmp/rb-boot2.log \
  && grep -aq 'DEMO 94\] PASS' /tmp/rb-boot3.log \
  && grep -aq 'health gate FAILED (sabotage build)' /tmp/rb-boot5.log \
  && grep -aq 'DEMO 95\] PASS' /tmp/rb-boot6.log && ok=1
grep -ah 'DEMO 9[456]\] PASS\|staged slot\|boot-next armed\|PROMOTED\|REVERTED\|TRIAL boot\|health gate' /tmp/rb-boot*.log
echo
[ $ok -eq 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL/INCOMPLETE"
