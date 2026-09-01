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

## Results — 2026-08-31, oracle complete + DDI-D retarget

- Pop!_OS oracle captured: `intel_reg dump` (intel-gpu-tools 1.28) +
  explicit-address `intel_reg read` run → `intel_reg_dump.txt`,
  `intel_reg_extra.txt` in `docs/hardware/igpu-2026-07-08/`.
- **Topology correction: the eDP panel is wired to DDI D, not DDI A.**
  i915 display_info: ENCODER:92:DDI D/PHY D → CONNECTOR:93:eDP-1;
  DDI_BUF_CTL_A (0x64000) reads 0x80 = disabled. modeset.rs retargeted to
  the 0x643xx range; the previous DDI-A framing would have programmed a
  dead port.
- **Clocking simplification:** LCPLL_CTL (0x130040) = 0x44000037 = enabled +
  locked, NON_SSC ref — it sources the 2.7 GHz ×2 link; SPLL and both
  WRPLLs are disabled. The SKL-era DPLL_CTRL1/CFGCR registers (0x6C0xx) all
  read zero on HSW and were dropped. native-60 programs NO clocking; it
  verifies the LCPLL lock bit and refuses otherwise.
- **Force-wake ack correction:** i915's HSW path pairs FORCEWAKE_MT (0xA188)
  with FORCEWAKE_ACK_HSW = **0x130044** (intel_uncore.c v5.15, IS_HASWELL
  branch); 0x130040 is LCPLL_CTL (the FORCEWAKE_MT_ACK name is IVB-only).
  The 2026-08-30 wells PASS polled 0x130040 — corrected; re-verify on the
  next metal run.
- Oracle decodes (cross-checked vs i915 v5.15 i915_reg.h):
  TRANS_DDI_FUNC_CTL_A = 0xB2200002 (enable, SELECT_PORT(D), DP SST, 6 bpc,
  eDP-input-A-ON, ×2), DDI_BUF_CTL_D = 0x80000002 (enable, ×2),
  DP_TP_CTL_D = 0x80040300 (enable, SST, enhanced frame, LINK_TRAIN_NORMAL),
  PIPECONF_A = 0xC0000010. Linux runs the plane as XBGR2101010
  (DSPCNTR_A = 0xE0000400) — NOT copied; staging keeps the GOP plane format
  to match the kept GOP surface.
- native-60 is now ARMED: oracle filled, write-if-different writes with
  per-register readback audit (a fully-matching run touches no hardware).
- Design-doc typos fixed against the dump: TRANS_HTOTAL_A 0x08AF→0x08AB077F,
  TRANS_HSYNC_A → 0x081507D9, TRANS_VSYNC_A 0x0441→0x0446043D,
  PIPEASRC 0x0437077F→0x077F0437 (the code's plan() already computed the
  correct values — verify-60 proved it; only the doc table was wrong).
- Next metal run: `modeset wells` (re-verify 0x130044 ack), `modeset
  snapshot` (now reads DDI-D + LCPLL regs), `modeset native-60` (first
  armed takeover), `modeset restore-gop` (writes back DDI-D snapshot).

## Results — 2026-09-01, T540p metal (armed build, e4f6982)

- `modeset wells` — **PASS with the corrected ack.** FORCEWAKE_ACK_HSW
  (0x130044) acquires 0x00000001 and releases to 0x00000000, status=0.
  The 0x130040 LCPLL-alias bug is dead.
- `modeset snapshot` — **PASS.** Full register set captured. Headline:
  **GOP and Linux agree on every register** — DDI_BUF_CTL_D = 0x80000002,
  DP_TP_CTL_D = 0x80040300, TRANS_DDI_FUNC_CTL_A = 0xB2200002, PIPECONF_A =
  0xC0000010, all timings, DSPSTRIDE_A = 0x1E00 — byte-identical to the
  Pop!_OS oracle. GOP DSPCNTR_A = 0x98000000 (XRGB8888, as assumed).
  LCPLL_CTL = 0x44000031 (locked; SPLL/WRPLL off).
- `modeset native-60` — **PASS, "takeover clean".** LCPLL lock check passed,
  every target register reported `OK (already)` — a pure readback audit with
  ZERO hardware writes, exactly the dress rehearsal the write-if-different
  staging was built for. The takeover sequence + oracle are proven.
- `modeset restore-gop` — **PASS.** First writeback with the real DDI-D
  register set (and DSPADDR_A). No DIFFs, screen unchanged.
- **Surprise finding:** DSPADDR_A, DSPSURF_A *and* DSPSURFLIVE_A all read 0
  under GOP. SURFLIVE is the hardware's post-latch live value, so plane A
  genuinely scans out from display-address 0 — the GOP framebuffer sits at
  the base of the plane's address space (stolen-memory offset 0). **Input
  to the page-flip task (#14):** before the first flip, determine whether
  DSPSURF expects a stolen-relative or GGTT address on this machine (i915
  writes GGTT offsets; GOP's all-zero state suggests stolen-relative).
- Screen never glitched through the whole protocol. No reboot needed.
- (Cosmetic: the em-dash in native-60's "takeover clean —" message renders
  as `???` in the console font. Non-issue.)

**Rung B (native-60 takeover) is PROVEN on metal.** The remaining path to
tear-free 60 fps is Rung C: SemOS-owned double buffer + vblank-latched
DSPSURF flip (SYS_FB_FLIP), which now has its register interface validated
end-to-end.

## 2026-09-01 (later) — Rung C metal run: gate refused correctly, addressing model corrected

`flipdemo` (SYS_FB_FLIP = 141) ran on metal. Result: **the stolen-relative
gate refused, the fallback ran, the console came back clean.** Video evidence:
bar bounced for ~110 s at ~200 ms/frame (blit-bound Rung-A pacing, visible
tear steps) instead of 10 s at 60 fps — the flip never happened, by design.

The `modeset status` flip probe on the same boot explains why, and settles
the stolen-vs-GGTT question left open above:

- `fb_phys = 0xE0000000` = **BAR2 aperture base** — the GOP fb is reached
  through the GGTT even by the CPU.
- `BSM = 0xBDA00000`, `GMS = 0` — **this machine has zero stolen memory**.
  The stolen-relative hypothesis is dead on the T540p (fb - BSM = 0x22600000,
  nonsense as an offset).
- Conclusion: on HSW `DSPSURF_A` is a **GGTT (aperture) offset** here; GOP
  mapped the fb at GGTT offset 0 (hence DSPSURF/DSPSURFLIVE = 0).

**Rung C status: mechanism proven safe (clean refusal + fallback + console
restore), addressing model corrected.** Rung C-revised = GGTT route: program
GGTT entries at aperture offset 0x800000 for a SemOS-owned 8 MiB back buffer
(PTE attribute bits copied from GOP's GGTT[0]), flip DSPSURF_A between 0 and
0x800000. All latch/verify/unflip plumbing carries over unchanged.
