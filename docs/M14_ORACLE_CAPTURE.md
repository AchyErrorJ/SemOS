# M14 iGPU Oracle Capture

Before implementing native Haswell modesetting, capture the Linux/i915 runtime
state from Pop!_OS. This data reveals the active pipe, transcoder, plane,
DPLL, and panel timings that SemOS will need to replicate.

## Quick capture

Boot into Pop!_OS on the T540p, open a terminal in the SemOS repo, and run:

```bash
bash tools/capture-igpu-oracle.sh
```

This creates `docs/hardware/igpu-YYYY-MM-DD/` with:

- `lspci_00_02_0.txt` — PCI config space decode
- `lspci_00_02_0_full.txt` — full config dump (root only)
- `i915_display_info.txt` — active pipe/plane/transcoder/DPLL state
- `i915_opregion.txt` — BIOS OpRegion / VBT mailbox
- `i915_frequency_info.txt` — clock/PWM info
- `i915_runtime_pm_status.txt` — power-well state
- `i915_ddb_info.txt` — display data buffer allocation
- `edid_eDP-1.bin` / `.txt` — panel native timings
- `dmesg_i915.txt` — i915 boot probe lines

## Manual commands

If you prefer to run the commands manually:

```bash
DATE=$(date +%F)
OUT=docs/hardware/igpu-${DATE}
mkdir -p "${OUT}"

lspci -nnvv -s 00:02.0 | tee "${OUT}/lspci_00_02_0.txt"
sudo lspci -nnvvxxxx -s 00:02.0 | tee "${OUT}/lspci_00_02_0_full.txt"

for f in i915_display_info i915_opregion i915_frequency_info \
         i915_runtime_pm_status i915_ddb_info; do
    sudo cat "/sys/kernel/debug/dri/0/$f" > "${OUT}/${f}.txt" 2>&1 || true
done

for edid in /sys/class/drm/card1-eDP-1/edid /sys/class/drm/card0-eDP-1/edid; do
    [ -s "$edid" ] || continue
    name="edid_$(basename "$(dirname "$edid")")"
    cp "$edid" "${OUT}/${name}.bin"
    edid-decode "$edid" > "${OUT}/${name}.txt" 2>&1 || true
done

dmesg | grep -iE 'i915|drm|edid|eDP' | tail -n 200 | tee "${OUT}/dmesg_i915.txt"
```

## Next step

Commit the captured directory and continue with `docs/DISPLAY_HASWELL_NATIVE_MODESET.md`.
