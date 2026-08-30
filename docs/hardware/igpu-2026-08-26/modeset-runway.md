# M14-I runway — metal verification checklist (T540p)

Date: __________  Kernel commit: __________

Purpose: prove the guarded modeset tool on hardware *before* any native-60
takeover attempt. Every step is either read-only or writes values that are
already in the registers (snapshot-restore of the live GOP config). If the
screen ever goes black or garbled: **reboot** — GOP always comes back — and
record the last op below.

## 0. Baseline

- [ ] `fbinfo` — record the GOP mode: ______ × ______ stride ______
      Native panel is 1920×1080 (CMN N156HGE-EA1). Match? YES / NO
      (This closes the open roadmap question: "does the bootloader GOP
      request deliver 1920×1080 on the T540p?")

## 1. Read-only state

- [ ] `modeset status` — 13 registers print, no hang.
- [ ] `modeset snapshot` — prints 24 registers + "saved for restore-gop".
- [ ] `modeset verify-60` — mismatch count: ______
      0 = GOP timings already equal the EDID model (ideal; poke-60 is then a
      provable no-op). >0 = GOP is running different timings — **stop, do not
      run poke-60**, paste the DIFF lines here:

```
(paste)
```

## 2. Timing poke (only if verify-60 was clean)

- [ ] `modeset poke-60` — writes 7 timing regs, then re-verifies.
      Screen unchanged? YES / NO
- [ ] `modeset verify-60` again — still 0 mismatches? YES / NO

## 3. Force-wake smoke

- [x] `modeset wells` — dumps 4 power regs, then "force-wake acquire OK" +
      "force-wake released". Any timeout message? YES / NO → **PASS 2026-08-30**

## 4. Restore path

- [ ] `modeset restore-gop` — expect "OK — snapshot written back".
      Any DIFF lines? YES / NO (paste if yes)
- [ ] Screen looks normal (shell readable, no flicker)? YES / NO
- [ ] `fb-demo` — red rectangle renders correctly? YES / NO
      (Proves the framebuffer surface path is intact after the restore.)
- [ ] `brightness 80` then `brightness 50` — backlight still works? YES / NO

## 5. Notes / anomalies

```
```

## Next gate

The native-60 takeover stays blocked until the Pop!_OS oracle capture is
refreshed with sudo (root-only files in docs/hardware/igpu-2026-07-08/ are
sudo-error stubs — the exact commands are in that directory's README.md and
in docs/DISPLAY_HASWELL_NATIVE_MODESET.md). After capture: fill the
"Oracle-derived target values" table in the design doc, then implement
native-60's 12-step sequence.

## Results — 2026-08-26, T540p metal

- `modeset snapshot` — saved for restore-gop. PASS
- `modeset verify-60` — **0 mismatches**: live GOP timings already equal the
  EDID 1920×1080@60.007 oracle (TRANS_HTOTAL_A = 0x08AB077F → 2220×1920).
  **Closes the open roadmap question: GOP delivers native 1080p.**
  `poke-60` therefore unnecessary (user typed `poke 60` without `modeset`;
  harmless — shell tried to spawn a `poke` program).
- `modeset wells` — PWR_WELL_CTL = 0xC0000000 (DDI-A/eDP well: request+state
  set, BIOS has it on as expected), PWR_WELL_CTL2 = 0x40000000. **Force-wake
  acquire FAILED: ack timeout.** Root cause: FORCEWAKE_MEDIA 0xA254 / ACK
  0xA258 are gen6-era (Sandy Bridge) offsets — on Haswell those MMIO
  addresses are dead (reads return 0, writes land nowhere). Fixed in
  modeset.rs: now FORCEWAKE_MT 0xA188 / FORCEWAKE_MT_ACK 0x130040 with the
  i915 masked-write encoding (acquire = 0x00010001, release = 0x00010000,
  hold bit FORCEWAKE_KERNEL = 0x1).
- `modeset wells` RE-TEST (2026-08-30, fixed build) — **PASS.** FORCEWAKE_MT
  acquire/release round-trips on metal; the HSW offsets + masked-write
  encoding were the whole fix. Nothing further needed from this smoke test —
  the runway (snapshot / verify-60 / restore-gop / wells) is fully green and
  native-60 is blocked only on the sudo oracle capture (dri/1, see above).
- `modeset restore-gop` — full snapshot writeback, no DIFF lines, screen
  unchanged and readable. PASS (and note: it worked with force-wake never
  held — consistent with display registers not requiring force-wake on HSW;
  force-wake only matters for GT/render-domain registers).
- Screen never glitched through the whole protocol. No reboot needed.
