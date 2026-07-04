#!/usr/bin/env bash
# Capture Intel iGPU oracle data on Pop!_OS for SemOS M14 native modeset.
# Run as a normal user; sudo is used only where required.

set -euo pipefail

DATE="$(date +%F)"
OUT="docs/hardware/igpu-${DATE}"
mkdir -p "${OUT}"

echo "Capturing iGPU oracle data to ${OUT}..."

# PCI config space decode (normal user can read most of it)
lspci -nnvv -s 00:02.0 | tee "${OUT}/lspci_00_02_0.txt"
# Full config space dump including extended config (requires root)
if command -v sudo >/dev/null 2>&1; then
    sudo lspci -nnvvxxxx -s 00:02.0 | tee "${OUT}/lspci_00_02_0_full.txt" || true
fi

# i915 DRM debugfs
for f in i915_display_info i915_opregion i915_frequency_info \
         i915_runtime_pm_status i915_ddb_info; do
    if [ -r "/sys/kernel/debug/dri/0/$f" ]; then
        cat "/sys/kernel/debug/dri/0/$f" > "${OUT}/${f}.txt"
    elif command -v sudo >/dev/null 2>&1; then
        sudo cat "/sys/kernel/debug/dri/0/$f" > "${OUT}/${f}.txt" 2>&1 || true
    fi
done

# Panel EDID (raw binary + decoded text)
for edid in /sys/class/drm/card1-eDP-1/edid /sys/class/drm/card0-eDP-1/edid; do
    [ -s "$edid" ] || continue
    name="edid_$(basename "$(dirname "$edid")")"
    cp "$edid" "${OUT}/${name}.bin"
    if command -v edid-decode >/dev/null 2>&1; then
        edid-decode "$edid" > "${OUT}/${name}.txt" 2>&1 || true
    fi
done

# i915 boot messages
dmesg | grep -iE 'i915|drm|edid|eDP' | tail -n 200 | tee "${OUT}/dmesg_i915.txt"

echo "Done. Review ${OUT} before committing."
