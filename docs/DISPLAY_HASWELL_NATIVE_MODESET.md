# Native Haswell modeset for T540p internal eDP (M14-I)

Status: **armed** — oracle complete 2026-08-31 (intel_reg dump + explicit-address reads)  
Target: ThinkPad T540p internal eDP panel, Intel HD 4600 (`8086:0416`)  
Panel: CMN N156HGE-EA1, native 1920x1080@60.007 Hz, pixel clock 151.6 MHz  

This document is the design gate for M14-I: taking the T540p display path
away from UEFI GOP and giving SemOS direct ownership of the Haswell display
engine. The Pop!_OS oracle capture is refreshed and every target register
value below is measured, not guessed.

## Scope

In scope for M14-I:

- A single, shell-gated `modeset native-60` command.
- Power-well sequencing for the display and DDI blocks.
- LCPLL lock verification (the 2.7 GHz eDP link clock — never programmed).
- DDI D (eDP) buffer and transport configuration.
- Transcoder A timing for 1920x1080@60.007 Hz.
- Pipe A enable.
- Primary plane A pointing at a SemOS-allocated framebuffer.
- A `modeset restore-gop` command and reboot fallback.

Out of scope:

- Automatic boot-time takeover.
- Multi-monitor / non-eDP connectors.
- Render / media / 3D.
- DPMS, suspend/resume, panel power sequencing.
- Any GPU other than HD 4600 (`8086:0416`).

## Safety rules

1. **Shell-gated only.** `modeset native-60` is a manual command; the kernel
   never performs a native modeset at boot.
2. **Device whitelist.** All display MMIO writes are refused unless the probed
   device is `8086:0416`.
3. **Oracle-first.** Before the first metal write, refresh the Pop!_OS oracle
   capture and record the live register values in this document.
4. **Save before overwrite.** `native-60` calls `snapshot()` and stores the
   original state so `restore-gop` can roll back.
5. **Reboot fallback.** If the screen goes black, a normal reboot returns to
   UEFI GOP.

## Hardware facts

| Item | Value | Source |
|------|-------|--------|
| GPU | Intel HD 4600 / Haswell GT2 | `lspci_00_02_0.txt` |
| PCI ID | `8086:0416` | `igpu.rs` |
| BDF | `00:02.0` | `lspci_00_02_0.txt` |
| BAR0 MMIO | `f1000000`, size 4 MiB | `lspci_00_02_0.txt` |
| BAR2 aperture | `e0000000`, size 256 MiB | `lspci_00_02_0.txt` |
| Panel | CMN N156HGE-EA1 | EDID descriptor |
| Native timing | 1920x1080@60.007 Hz | EDID DTD |
| Pixel clock | 151.6 MHz | EDID DTD |
| hactive | 1920 | EDID DTD |
| hblank | 300 | EDID DTD |
| hsync offset | 90 | EDID DTD |
| hsync width | 60 | EDID DTD |
| vactive | 1080 | EDID DTD |
| vblank | 58 | EDID DTD |
| vsync offset | 6 | EDID DTD |
| vsync width | 9 | EDID DTD |
| htotal | 2220 | computed |
| vtotal | 1138 | computed |
| eDP link | 2.7 GHz × 2 lanes (port_clock=270000) | `i915_display_info.txt` (2026-08-30 capture) |
| Panel depth | 6 bpc (18 bpp), dithering ON | `i915_display_info.txt` ("dither=yes, bpp=18") |
| Linux plane format | **XB30** (2:10:10:10), NOT XRGB8888 | `i915_display_info.txt` [FB:94] |
| Linux timings | 151600, 1920 2010 2070 2220 / 1080 1086 1095 1138 | matches the EDID table above exactly |
| **eDP port** | **DDI D**, not DDI A | `i915_display_info.txt` (ENCODER:92:DDI D/PHY D → eDP-1); `DDI_BUF_CTL_A` (0x64000) reads 0x80 = disabled |
| Link clock | LCPLL enabled+locked, NON_SSC ref; SPLL/WRPLL disabled | `intel_reg_extra.txt` (LCPLL_CTL=0x44000037) |
| PCH transcoders | Unused — CPU transcoder A drives DDI D directly | `intel_reg_dump.txt` (TRANS_* @ 0xExxxx all 0, FDI/PCH all 0) |
| SKL DPLL regs | Do not exist on HSW — read as 0 | `intel_reg_extra.txt` (0x6C058/0x6C040/0x6C044/0x6C080/0x6C084) |

**XB30 caveat (found 2026-08-30):** i915 runs primary plane A in 2:10:10:10
with dithering down to the 18 bpp panel. The SemOS staging keeps the GOP
surface, which is XRGB8888 — so `DSPCNTR_A` from the oracle must have its
format bits adapted (or the surface converted), not copied verbatim.

## Register map

All offsets are relative to BAR0 (`f1000000` on the T540p). Registers are
32-bit unless noted.

### Power / force-wake

| Name | Offset | Purpose |
|------|--------|---------|
| `PWR_WELL_CTL` | `0x45400` | Request power wells |
| `PWR_WELL_CTL2` | `0x45404` | Power-well status |
| `FORCEWAKE_MT` | `0xA188` | Multi-threaded force-wake request (masked write) |
| `FORCEWAKE_ACK_HSW` | `0x130044` | MT force-wake acknowledge — the register i915's HSW path polls (`intel_uncore.c`, IS_HASWELL branch) |

> **Alias trap (found 2026-08-31):** `0x130040` is named `FORCEWAKE_MT_ACK`
> in i915 but is used only on IVB; on HSW that address is `LCPLL_CTL`.
> An earlier build polled 0x130040 as the ack and passed — corrected to
> 0x130044, re-verified by `modeset wells` on the next metal run.
> (The gen6-era 0xA254/0xA258 registers do not exist on HSW at all.)

### HSW clocking (LCPLL) — verified, never programmed

| Name | Offset | Purpose |
|------|--------|---------|
| `LCPLL_CTL` | `0x130040` | LC PLL control: bit 31 disable, bit 30 lock, bits 29:28 ref select |
| `SPLL_CTL` | `0x46020` | System PLL (disabled on this machine) |
| `WRPLL_CTL1` / `WRPLL_CTL2` | `0x46040` / `0x46060` | Programmable PLLs (disabled on this machine) |

The 2.7 GHz eDP link is sourced by LCPLL. The SKL-era `DPLL_CTRL1` /
`DPLL_CFGCR1` / `DPLL_CFGCR2` (`0x6C058` / `0x6C080` / `0x6C084`) read as
zero on Haswell and were dropped from the plan.

### DDI D (internal eDP)

| Name | Offset | Purpose |
|------|--------|---------|
| `DDI_BUF_CTL_D` | `0x64000 + 3×0x100` | DDI buffer / port enable |
| `DP_TP_CTL_D` | `0x64040 + 3×0x100` | DisplayPort transport control |
| `DP_TP_STATUS_D` | `0x64044 + 3×0x100` | DisplayPort transport status (read-only) |

The per-port voltage-swing translation table (`DDI_BUF_TRANS`, entry 0
selected by the oracle value) is left exactly as the firmware trained it —
never written.

### Transcoder A

| Name | Offset | Purpose |
|------|--------|---------|
| `TRANS_HTOTAL_A` | `0x60000` | (htotal-1) << 16 \| (hactive-1) |
| `TRANS_HBLANK_A` | `0x60004` | (htotal-1) << 16 \| (hactive-1) |
| `TRANS_HSYNC_A` | `0x60008` | (hsync_end-1) << 16 \| (hsync_start-1) |
| `TRANS_VTOTAL_A` | `0x6000C` | (vtotal-1) << 16 \| (vactive-1) |
| `TRANS_VBLANK_A` | `0x60010` | (vtotal-1) << 16 \| (vactive-1) |
| `TRANS_VSYNC_A` | `0x60014` | (vsync_end-1) << 16 \| (vsync_start-1) |
| `PIPEASRC` | `0x6001C` | (hactive-1) << 16 \| (vactive-1) |
| `TRANS_DDI_FUNC_CTL_A` | `0x60400` | DDI selection / transcoder enable |

### Pipe A

| Name | Offset | Purpose |
|------|--------|---------|
| `PIPE_DSL_A` | `0x70000` | Current display scanline (read-only) |
| `PIPECONF_A` | `0x70008` | Pipe configuration + enable |

### Primary plane A

| Name | Offset | Purpose |
|------|--------|---------|
| `DSPCNTR_A` | `0x70180` | Plane control + pixel format |
| `DSPSTRIDE_A` | `0x70188` | Surface stride in bytes |
| `DSPSURF_A` | `0x7019C` | Surface physical base address |
| `DSPTILEOFF_A` | `0x701A0` | Tile offset |
| `DSPPOS_A` | `0x7018C` | Position |
| `DSPSIZE_A` | `0x70190` | Size |

## Oracle-derived target values

Captured 2026-08-30/31 on Pop!_OS (kernel 7.0, i915 at debugfs minor 1):
`intel_reg dump` → `intel_reg_dump.txt`, then explicit-address reads for the
registers the intel-gpu-tools 1.28 spec doesn't cover → `intel_reg_extra.txt`
(both in `docs/hardware/igpu-2026-07-08/`). Decodes cross-checked against
i915 v5.15 `i915_reg.h` field definitions.

| Register | Value | Decode |
|----------|-------|--------|
| `PWR_WELL_CTL` | `0xC0000000` | **Metal-verified** (SemOS snapshot, 2026-08-26): DDI/eDP well request+state on |
| `PWR_WELL_CTL2` | `0x40000000` | **Metal-verified** (SemOS snapshot, 2026-08-26) |
| `LCPLL_CTL` | `0x44000037` | Enabled (bit 31 clear), **locked** (bit 30), NON_SSC ref — 2.7 GHz link source. Verify-only |
| `SPLL_CTL` | `0x14000000` | Disabled (bit 31 clear) — unused |
| `WRPLL_CTL1` / `WRPLL_CTL2` | `0x00202418` | Disabled (bit 31 clear) — unused |
| `DDI_BUF_CTL_D` | `0x80000002` | Enable, trans-select 0, ×2 lanes |
| `DP_TP_CTL_D` | `0x80040300` | Enable, SST, enhanced frame, LINK_TRAIN_NORMAL (link already trained) |
| `DP_TP_STATUS_D` | `0x00000000` | Status, read-only |
| `TRANS_DDI_FUNC_CTL_A` | `0xB2200002` | Enable, **SELECT_PORT(D)**, DP SST, 6 bpc, −VSync/−HSync, eDP-input-A-ON, ×2 lanes |
| `PIPECONF_A` | `0xC0000010` | Enable, active, progressive |
| `DSPCNTR_A` | `0xE0000400` | Linux: enable + XBGR2101010 — **not copied** (see XB30 caveat); staging keeps the GOP snapshot's plane control |
| `DSPSTRIDE_A` | `0x00001E00` | 7680 B = 1920 px × 4 (matches SemOS metal) |
| `DSPSURF_A` | `0x01180000` | Linux's stolen-memory surface (informational; staging keeps the GOP surface) |

## Timing register values

From the EDID and `modeset.rs` `T540P_EDP_1080P60`:

| Register | Value | How derived |
|----------|-------|-------------|
| `TRANS_HTOTAL_A` | `0x08AB077F` | (2220-1) << 16 \| (1920-1) — matches dump + SemOS metal |
| `TRANS_HBLANK_A` | `0x08AB077F` | same as HTOTAL |
| `TRANS_HSYNC_A` | `0x081507D9` | (2070-1) << 16 \| (2010-1) — matches dump |
| `TRANS_VTOTAL_A` | `0x04710437` | (1138-1) << 16 \| (1080-1) |
| `TRANS_VBLANK_A` | `0x04710437` | same as VTOTAL |
| `TRANS_VSYNC_A` | `0x0446043D` | (1095-1) << 16 \| (1086-1) — matches dump |
| `PIPEASRC` | `0x077F0437` | (1920-1) << 16 \| (1080-1) — matches dump |

## Native modeset sequence

`modeset native-60` performs the following steps (armed since the
2026-08-31 oracle; every target write is **write-if-different** — a register
already holding the oracle value is skipped, so a fully-matching run touches
no hardware and is a pure readback audit):

1. Validate target GPU is `8086:0416` with BAR0 MMIO.
2. Read and store a `DisplaySnapshot` (auto-taken if missing — restore-gop
   is armed before any write).
3. Enable required power wells via `PWR_WELL_CTL`; poll `PWR_WELL_CTL2`.
4. ~~Request force-wake~~ **Skipped on HSW**: display registers
   (0x6xxxx/0x7xxxx) don't require it — proven by restore-gop writing the
   full plane/transcoder set with no hold. Force-wake gates GT/render only.
5. **Verify LCPLL lock** (`LCPLL_CTL` bit 30 set); program no clocking at
   all — the firmware's LCPLL already sources the 2.7 GHz link. Abort if
   unlocked rather than guess.
6. Configure DDI D buffer and DP transport using oracle values.
7. Write transcoder timing registers from the table above.
8. Configure `PIPECONF_A` and enable pipe A.
9. **Staging keeps the GOP surface and plane format**: `DSPCNTR_A`,
   `DSPSURF_A` and `DSPSTRIDE_A` are rewritten with the snapshot values, so
   the format bits always match the surface being scanned out and a clean
   takeover keeps showing the same pixels. (Linux's `DSPCNTR_A` is
   XBGR2101010 — wrong for the XRGB8888 GOP surface.) The SemOS-owned double
   buffer + flip landed independently of the takeover — see "Rung C: page
   flip" below; it runs on the GOP-configured pipe.
10. Write `TRANS_DDI_FUNC_CTL_A` from oracle (port D, DP SST, 6 bpc, ×2).
11. Readback verdict from the per-write audit: any DIFF is reported with
    the restore-gop escape route.

## Rung C: page flip (SYS_FB_FLIP = 141)

Tear-free 60 fps without any modeset at all: the GOP-configured pipe is
already correct, so presenting is just repointing plane A's surface.
DSPSURF_A writes are **vblank-latched** on Haswell — the swap happens at the
next frame boundary, atomically, no matter when in the frame it lands.

**Buffer plan** (validated by the 2026-09-01 metal run: DSPADDR/DSPSURF/
DSPSURFLIVE all read 0 under GOP, so display-offset 0 is the framebuffer
itself):

- Buffer 0 = the GOP framebuffer (display offset 0) — also the console's home.
- Buffer 1 = GOP fb + 8 MiB (`0x800000`, 4 KiB-aligned; 1080p32 needs 0x7E9000).
- Gate: `fb_phys == BSM` (PCI 0:2.0 reg 0x5C) proves stolen-relative plane
  addressing; GMS (host bridge 0:0.0 reg 0x52, bits [7:3] × 32 MiB) must be
  ≥ 16 MiB. `modeset status` prints the probe (fb phys, BSM, GMS).
- If the gate fails the machine can't flip this way — the GGTT route is
  future work; SYS_FB_FLIP refuses and apps fall back to vblank-paced blits
  (Rung A), drawing into the visible buffer as before.

**Metal result 2026-09-01: the stolen-relative gate refused — correctly.**
The probe showed `fb_phys = 0xE0000000` (= BAR2 aperture base), `BSM =
0xBDA00000`, `GMS = 0` (**zero stolen memory on this machine**). The GOP
framebuffer is not in stolen memory at all: it lives at **GGTT offset 0**,
and even the CPU reaches it through the aperture — so on this machine
`DSPSURF_A` is a **GGTT (aperture) offset**, not a stolen offset. flipdemo
fell back to Rung-A paced blits (≈200 ms/frame, visible tear steps) and the
console came back clean on exit — the safety wiring is metal-proven.

**Rung C-revised (GGTT route, next spike):** buffer 1 must exist *in the
GGTT*, not in stolen memory: allocate a SemOS-owned 8 MiB physical back
buffer, program GGTT entries at aperture offset `0x800000` to point at it
(attribute bits copied from GOP's own entry 0), then flip `DSPSURF_A`
between `0` and `0x800000`. The vblank latch, DSPSURFLIVE verify-then-commit,
and unflip safety all carry over unchanged; only the gate changes
(`fb_phys == BAR2 base` + GGTT armed, instead of `== BSM`).

**Protocol:** the app draws into the *current draw target* (SYS_FB_BLIT's
destination follows it), then SYS_FB_FLIP points scanout at the just-drawn
buffer and flips the draw target to the previously visible one. Startup edge
case: buffer 0 is both draw target and visible before the first flip, so the
first frame draws in place (Rung A behavior) — every later frame is a true
double-buffered present. Verify-then-commit: `flip_scanout` reads
DSPSURFLIVE_A one frame after the write and rolls back on mismatch.

**Safety:** flip requires an active `fb_claim`; `fb_claim(0)` and
`reset_tty_flags` (process exit/crash) unflip scanout to buffer 0 and reset
the draw target, so a dead game can never leave the console invisible.
Reboot still returns to GOP regardless.

**Demo:** `flipdemo` (user-programs/flipdemo) bounces a bar full-screen for
~10 s: render into a user buffer → one full-frame SYS_FB_BLIT into the hidden
buffer → SYS_FB_FLIP. Clean bar edges = no tearing. Metal 2026-09-01: ran the
fallback path (gate refused, see above) — bar bounced ~5 fps with visible
tear steps, exit restored the console with scrollback intact.

## Restore sequence

`modeset restore-gop` performs the inverse:

1. If no snapshot is stored, print "reboot to return to GOP" and return.
2. Disable pipe A.
3. Restore original `DSPSURF_A`, `PIPECONF_A`, `TRANS_DDI_FUNC_CTL_A`,
   `DDI_BUF_CTL_D`, `DP_TP_CTL_D`, and power-well state from the snapshot.
   (Clocking is never touched, so there is nothing to restore there.)
4. Point plane A back at the original GOP framebuffer.
5. Print confirmation.

## Verification plan

### QEMU / build

- `cargo check` in `kernel-x86_64/` succeeds.
- `cargo check` in `user-programs/sem-sh/` succeeds.
- On QEMU the new `modeset snapshot`/`native-60`/`restore-gop` commands report
  "no Intel display controller found" safely.

### Metal (T540p)

1. Boot SemOS.
2. `modeset status` and `modeset verify-60` show expected timing.
3. `modeset snapshot` captures the live state.
4. `modeset native-60` switches to the SemOS framebuffer.
5. `fb-demo` animates correctly on the new framebuffer.
6. `modeset restore-gop` returns to GOP.
7. If black screen: reboot returns to GOP.

## Open questions / risks

1. ~~Missing register oracle~~ — **resolved 2026-08-31** (intel_reg dump +
   explicit-address reads). Bonus finding: the panel is on DDI D, not DDI A;
   the SKL DPLL register block was dropped; LCPLL needs no programming.
2. eDP link training may require additional writes beyond the transcoder/DDI
   enable path. The staging deliberately never re-trains (DP_TP_CTL_D keeps
   LINK_TRAIN_NORMAL); if the panel stays black despite matching registers,
   link training becomes the next spike.
3. Force-wake / power-well sequence errors can hang the GPU. The first metal
   test should be done with an easy reboot path.
4. The `FORCEWAKE_ACK_HSW` (0x130044) correction is i915-sourced but only
   re-verified on metal by the next `modeset wells` run.

## Files

- `kernel-x86_64/src/display/modeset.rs` — register constants, snapshot,
  native-60, restore-gop.
- `kernel-x86_64/src/display/mod.rs` — native framebuffer allocator.
- `kernel-x86_64/src/platform_impl.rs` — syscall dispatch to modeset ops.
- `user-programs/sem-sh/src/main.rs` — shell subcommands.
- `docs/DISPLAY_HASWELL_NATIVE_MODESET.md` — this document.
