#!/usr/bin/env bash
# Capture Intel iGPU oracle data on Pop!_OS for SemOS M14 native modeset.
# Run as a normal user; sudo is used only where required.

set -euo pipefail

DATE="$(date +%F)"
OUT="docs/hardware/igpu-${DATE}"
mkdir -p "${OUT}"

echo "Capturing iGPU oracle data to ${OUT}..."

{
    echo "date=$(date -Is)"
    echo "uname=$(uname -a)"
    echo "pwd=${PWD}"
} > "${OUT}/host_context.txt"

# PCI config space decode (normal user can read most of it)
lspci -nnvv -s 00:02.0 | tee "${OUT}/lspci_00_02_0.txt"
# Full config space dump including extended config (requires root)
if command -v sudo >/dev/null 2>&1; then
    sudo lspci -nnvvxxxx -s 00:02.0 | tee "${OUT}/lspci_00_02_0_full.txt" || true
fi

# PCI sysfs fields useful for SemOS BAR/class probing.
DEV="/sys/bus/pci/devices/0000:00:02.0"
if [ -d "$DEV" ]; then
    {
        echo "device=$DEV"
        for attr in vendor device subsystem_vendor subsystem_device class irq enable resource; do
            echo "--- $attr ---"
            cat "$DEV/$attr" 2>/dev/null || true
        done
    } > "${OUT}/sysfs_0000_00_02_0.txt"
fi

# i915 DRM debugfs (root-only on modern kernels; don't abort the capture).
for f in i915_display_info i915_opregion i915_frequency_info \
         i915_runtime_pm_status i915_ddb_info; do
    if [ -r "/sys/kernel/debug/dri/0/$f" ]; then
        cat "/sys/kernel/debug/dri/0/$f" > "${OUT}/${f}.txt" || true
    elif command -v sudo >/dev/null 2>&1; then
        sudo cat "/sys/kernel/debug/dri/0/$f" > "${OUT}/${f}.txt" 2>&1 || true
    else
        echo "not captured: /sys/kernel/debug/dri/0/$f is not readable and sudo is unavailable" > "${OUT}/${f}.txt"
    fi
done

# Backlight provider(s), especially important for M14-D.
for bl in /sys/class/backlight/*; do
    [ -e "$bl" ] || continue
    {
        echo "device=$bl"
        for attr in type max_brightness brightness actual_brightness bl_power; do
            if [ -r "$bl/$attr" ]; then
                printf '%s=' "$attr"
                cat "$bl/$attr"
            fi
        done
    } > "${OUT}/backlight_$(basename "$bl").txt"
done

lsmod | grep -E '(^i915|^drm|^video)' > "${OUT}/lsmod_display.txt" 2>&1 || true

# DRM connector status/modes plus panel EDID (raw binary + decoded text).
for conn in /sys/class/drm/card*-*; do
    [ -d "$conn" ] || continue
    name="$(basename "$conn")"
    {
        echo "connector=$conn"
        for attr in status enabled dpms modes; do
            echo "--- $attr ---"
            cat "$conn/$attr" 2>/dev/null || true
        done
    } > "${OUT}/drm_${name}.txt"

    edid="$conn/edid"
    [ -r "$edid" ] || continue
    # Sysfs EDID files often report size 0 even when reads return data. Copy,
    # then keep only non-empty captures.
    if cp "$edid" "${OUT}/edid_${name}.bin" 2>/dev/null && [ -s "${OUT}/edid_${name}.bin" ]; then
        if command -v edid-decode >/dev/null 2>&1; then
            edid-decode "${OUT}/edid_${name}.bin" > "${OUT}/edid_${name}.txt" 2>&1 || true
        fi
    else
        rm -f "${OUT}/edid_${name}.bin" "${OUT}/edid_${name}.txt"
    fi
done

# i915 boot messages. Often root-restricted on modern Linux, so don't let a
# permissions failure abort the capture.
(dmesg | grep -iE 'i915|drm|edid|eDP' | tail -n 200 > "${OUT}/dmesg_i915.txt") 2> "${OUT}/dmesg_i915.err" || true

echo "Done. Review ${OUT} before committing."
