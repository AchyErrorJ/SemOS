# T540p Bare-Metal Boot Test — SemOS self-dev loop (M1–M4)

Date drafted: 2026-08-23. Target: ThinkPad T540p (Pop!_OS dual boot).
Scope: boot the `autocompile` image on metal and run DEMO 80/83/87/88 with
keyboard approval. The T540p has **no serial port** — the approval gate polls
the PS/2 keyboard (commit 9fa71c9); audit lines read `tty=kbd`.

## Safety

- Demos write **ramfs only**. No SemOS disk writes at runtime.
- The sysroot blob probe is read-only and GPT-aware: it serves from the
  `SEMOS_SYSROOT` partition and refuses to touch LBA 0 of a partitioned disk.
- The only disk writes in this whole procedure are the ones *you* run from
  Pop!_OS in Phase 2 (`esp-install`, `dd` of the blob).
- Worst case at any point: power cycle, boot Pop!_OS, nothing changed.

## Phase 0 — netlog listener (Windows/WSL box, 192.168.1.138)

WSL is in mirrored networking mode; it shares the host LAN IP. Listener:

```bash
nc -u -l 9000 > /tmp/netlog9000.log      # in WSL on the Windows box
```

Watch live from Windows: `wsl -d Ubuntu tail -f /tmp/netlog9000.log`.
On the T540p, point netlog at `192.168.1.138`.

## Phase 1 — build (Pop!_OS on the T540p)

```bash
cd ~/SemOS && git pull                      # need >= 9fa71c9 (keyboard gate)
bash tools/build-semos-rustc-wsl.sh         # ~3.5 min; semos-rustc is baked into the kernel image
cd kernel-x86_64 && cargo build --release --features autocompile && cd ..
cd x86_64-runner && cargo +nightly run --release && cd ..

# Sysroot blob — only if out/sysroot.img is stale or missing:
bash user-programs/rustc-host/build-core-linux.sh
python3 tools/pack-sysroot-blob.py out/sysroot.img <NAME=FILE args>
```

Expect `out/sysroot.img` ≈ 59 MiB.

## Phase 2 — install to disk (Pop!_OS, one-time-ish)

1. **ESP image:** the usual `tools/esp-install` flow.
2. **Sysroot partition** (first time only — 128 MiB, GPT name is the key):

```bash
lsblk -o NAME,PARTLABEL,SIZE | grep -i semos     # already there?
# if NOT (verify the disk first with lsblk!):
sudo sgdisk -n 0:+128MiB -c 0:SEMOS_SYSROOT /dev/sda
```

3. **Write the blob** (after every sysroot rebuild):

```bash
sudo dd if=out/sysroot.img of=/dev/disk/by-partlabel/SEMOS_SYSROOT bs=4M conv=fsync
```

If `lsblk` shows no free space for the partition, STOP — don't resize anything
without a plan.

## Phase 3 — boot

Reboot → **F12** (ThinkPad boot menu) → SemOS entry. Watch the internal
display (framebuffer console; brightness keys work per M14 if dim).

## Phase 4 — PASS criteria

On screen, in order:

1. Kernel banner; sysroot probe reports the `SEMOS_SYSROOT` blob found.
2. `[DEMO 80] PASS: M1 hello loop` (on-device compile of hello.rs).
3. DEMO 83 → `Install /apps/calc? [y/N]` — press **y** on the laptop keyboard.
   Audit line must read `by=human tty=kbd`. Then `[DEMO 83] PASS`.
4. DEMO 87 → `Install /apps/wc? [y/N]` — **y** → `[DEMO 87] PASS`.
5. DEMO 88 (self-repair): crash detected → patch → verify →
   `Install /apps/head1 (repaired v2)? [y/N]` — **y** →
   `[DEMO 88] PASS: M4 self-repair — detect/diagnose/patch/verify/approve/repair end-to-end`
6. Lands in the interactive shell — typing works, system is live.

Negative-path check (optional, second boot): answer **n** at the DEMO 88
prompt → expect `[AUDIT] DENY ... reason=denied_or_timeout (fail-fast)` and
`PASS(partial)`.

## Failure handling

| Symptom | Meaning | Action |
|---|---|---|
| `[DEMO ..] FAIL` line | demo step failed | photo of screen; power off; debug from photo/netlog |
| Black screen, no output | likely display mode, not a crash | check netlog for how far it got; photo |
| Prompt never appears / no key response | keyboard poll issue | photo + netlog; do NOT power-mash — timeout auto-denies safely |
| Boot menu has no SemOS entry | ESP install didn't take | back in Pop!_OS: re-run esp-install, check `efibootmgr -v` |

Everything is recoverable by power-cycling into Pop!_OS. Bring the photo +
netlog excerpt back and we debug from there.

## Reference

- Self-dev loop plan: `docs/semos_selfdev_loop_plan.md` (M1–M4 all DONE in QEMU)
- Sysroot blob format & GPT rules: `docs/M27_DISK_SYSROOT_DESIGN.md`,
  `kernel-core/src/sysroot_blob.rs` (`probe`, `flash_from_usb`)
- Keyboard approval gate: `kernel-x86_64/src/main.rs` (`demo83_prompt_serial`)
- QEMU harnesses (what PASS looks like in detail): `tools/run-demo8{3,7,8}-qemu.sh`
