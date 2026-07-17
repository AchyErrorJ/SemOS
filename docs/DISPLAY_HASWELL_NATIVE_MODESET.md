# Native Haswell modeset for T540p internal eDP (M14-I)

Status: **design / partial implementation**  
Target: ThinkPad T540p internal eDP panel, Intel HD 4600 (`8086:0416`)  
Panel: CMN N156HGE-EA1, native 1920x1080@60.007 Hz, pixel clock 151.6 MHz  

This document is the design gate for M14-I: taking the T540p display path
away from UEFI GOP and giving SemOS direct ownership of the Haswell display
engine. No register-write metal test should happen until the oracle capture
section below is refreshed from Pop!_OS.

## Scope

In scope for M14-I:

- A single, shell-gated `modeset native-60` command.
- Power-well / force-wake sequencing for the display and DDI A blocks.
- DPLL 0 configuration for 151.6 MHz.
- DDI A (eDP) buffer and transport configuration.
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

## Register map

All offsets are relative to BAR0 (`f1000000` on the T540p). Registers are
32-bit unless noted.

### Power / force-wake

| Name | Offset | Purpose |
|------|--------|---------|
| `PWR_WELL_CTL` | `0x45400` | Request power wells |
| `PWR_WELL_CTL2` | `0x45404` | Power-well status |
| `FORCEWAKE_MEDIA` | `0xA254` | Render/media force-wake request |
| `FORCEWAKE_ACK_MEDIA` | `0xA258` | Render/media force-wake acknowledge |

### DPLL 0

| Name | Offset | Purpose |
|------|--------|---------|
| `DPLL_CTRL1` | `0x6C058` | DPLL reference / mode select |
| `DPLL_CFGCR1` | `0x6C080` | DPLL 0 fractional divider |
| `DPLL_CFGCR2` | `0x6C084` | DPLL 0 configuration |

### DDI A (internal eDP)

| Name | Offset | Purpose |
|------|--------|---------|
| `DDI_BUF_CTL_A` | `0x64000` | DDI buffer / port enable |
| `DP_TP_CTL_A` | `0x64040` | DisplayPort transport control |
| `DP_TP_STATUS_A` | `0x64044` | DisplayPort transport status |
| `DDI_BUF_TRANS_A` | `0x64E00` | Voltage-swing translation table (9 entries) |

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

> **TODO:** Refresh the Pop!_OS oracle capture and fill in the values below.
> Run:
>
> ```bash
> OUT=docs/hardware/igpu-2026-07-08
> sudo lspci -nnvvxxxx -s 00:02.0 > "$OUT/lspci_00_02_0_full.txt"
> for f in i915_display_info i915_opregion i915_frequency_info \
>          i915_runtime_pm_status i915_ddb_info; do
>   sudo cat "/sys/kernel/debug/dri/0/$f" > "$OUT/$f.txt" 2>&1 || true
> done
> dmesg | grep -iE 'i915|drm|edid|eDP' | tail -n 200 > "$OUT/dmesg_i915.txt"
> ```
>
> Then update this section with the actual Linux/i915 register values.

| Register | Expected value | Notes |
|----------|----------------|-------|
| `PWR_WELL_CTL` | `0x????????` | TBD from oracle |
| `PWR_WELL_CTL2` | `0x????????` | TBD from oracle |
| `DPLL_CTRL1` | `0x????????` | TBD from oracle |
| `DPLL_CFGCR1` | `0x????????` | TBD from oracle |
| `DPLL_CFGCR2` | `0x????????` | TBD from oracle |
| `DDI_BUF_CTL_A` | `0x????????` | TBD from oracle |
| `DP_TP_CTL_A` | `0x????????` | TBD from oracle |
| `TRANS_DDI_FUNC_CTL_A` | `0x????????` | TBD from oracle |
| `PIPECONF_A` | `0x????????` | TBD from oracle |
| `DSPCNTR_A` | `0x????????` | TBD from oracle |
| `DSPSTRIDE_A` | `0x????????` | TBD from oracle |
| `DSPSURF_A` | `0x????????` | TBD from oracle |

## Timing register values

From the EDID and `modeset.rs` `T540P_EDP_1080P60`:

| Register | Value | How derived |
|----------|-------|-------------|
| `TRANS_HTOTAL_A` | `0x08AF077F` | (2220-1) << 16 \| (1920-1) |
| `TRANS_HBLANK_A` | `0x08AF077F` | same as HTOTAL |
| `TRANS_HSYNC_A` | `0x08B9077F` | (2280-1) << 16 \| (2010-1) |
| `TRANS_VTOTAL_A` | `0x04710437` | (1138-1) << 16 \| (1080-1) |
| `TRANS_VBLANK_A` | `0x04710437` | same as VTOTAL |
| `TRANS_VSYNC_A` | `0x0441043D` | (1095-1) << 16 \| (1086-1) |
| `PIPEASRC` | `0x0437077F` | (1920-1) << 16 \| (1080-1) |

## Native modeset sequence

`modeset native-60` performs the following steps:

1. Validate target GPU is `8086:0416` with BAR0 MMIO.
2. Read and store a `DisplaySnapshot`.
3. Enable required power wells via `PWR_WELL_CTL`; poll `PWR_WELL_CTL2`.
4. Request force-wake if required; poll `FORCEWAKE_ACK_MEDIA`.
5. Configure DPLL 0 for 151.6 MHz using oracle values.
6. Configure DDI A buffer and DP transport using oracle values.
7. Write transcoder timing registers from the table above.
8. Configure `PIPECONF_A` and enable pipe A; poll for enable ack.
9. Allocate / initialize the SemOS-native framebuffer.
10. Write `DSPSURF_A`, `DSPSTRIDE_A`, and `DSPCNTR_A` for plane A.
11. Write `TRANS_DDI_FUNC_CTL_A` to enable transcoder A on DDI A.
12. Read back modified registers and print OK/DIFF against oracle.

## Restore sequence

`modeset restore-gop` performs the inverse:

1. If no snapshot is stored, print "reboot to return to GOP" and return.
2. Disable pipe A.
3. Restore original `DSPSURF_A`, `PIPECONF_A`, `TRANS_DDI_FUNC_CTL_A`,
   `DDI_BUF_CTL_A`, DPLL, and power-well state from the snapshot.
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

1. The root-only `i915_display_info.txt` in the current oracle directory is
   missing. The exact DPLL/DDI/pipe values cannot be finalized until it is
   refreshed.
2. eDP link training may require additional writes beyond the transcoder/DDI
   enable path. If the panel stays black despite matching registers, link
   training becomes the next spike.
3. Force-wake / power-well sequence errors can hang the GPU. The first metal
   test should be done with an easy reboot path.

## Files

- `kernel-x86_64/src/display/modeset.rs` — register constants, snapshot,
  native-60, restore-gop.
- `kernel-x86_64/src/display/mod.rs` — native framebuffer allocator.
- `kernel-x86_64/src/platform_impl.rs` — syscall dispatch to modeset ops.
- `user-programs/sem-sh/src/main.rs` — shell subcommands.
- `docs/DISPLAY_HASWELL_NATIVE_MODESET.md` — this document.
