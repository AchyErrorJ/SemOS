#!/usr/bin/env bash
# Capture Intel I217-LM / e1000e Ethernet oracle data on Pop!_OS for SemOS.
# Run as a normal user; sudo is used only where required.

set -euo pipefail

DATE="$(date +%F)"
OUT="docs/hardware/e1000e-${DATE}"
mkdir -p "${OUT}"

echo "Capturing e1000e (Intel I217-LM) oracle data to ${OUT}..."

{
    echo "date=$(date -Is)"
    echo "uname=$(uname -a)"
    echo "pwd=${PWD}"
} > "${OUT}/host_context.txt"

# Try to locate the PCI Ethernet controller. The T540p I217-LM is at 00:19.0.
LOC="${1:-00:19.0}"
SYSFS_DEV="/sys/bus/pci/devices/0000:${LOC}"

# PCI config space decode (normal user can read most of it)
lspci -nnvv -s "${LOC}" | tee "${OUT}/lspci_${LOC//:/_}.txt" || true
# Full config space dump including extended config (requires root)
if command -v sudo >/dev/null 2>&1; then
    sudo lspci -nnvvxxxx -s "${LOC}" | tee "${OUT}/lspci_${LOC//:/_}_full.txt" || true
fi

# PCI sysfs fields useful for SemOS BAR/class probing.
if [ -d "$SYSFS_DEV" ]; then
    {
        echo "device=$SYSFS_DEV"
        for attr in vendor device subsystem_vendor subsystem_device class irq enable resource resource0 resource1 boot_vga; do
            echo "--- $attr ---"
            cat "$SYSFS_DEV/$attr" 2>/dev/null || true
        done
    } > "${OUT}/sysfs_${LOC//:/_}.txt"
fi

# Find the kernel network interface associated with this PCI device.
IFACE=""
if [ -d "$SYSFS_DEV/net" ]; then
    # The net/ directory contains a subdirectory named after the interface.
    IFACE="$(find "$SYSFS_DEV/net" -mindepth 1 -maxdepth 1 -type d -print -quit | xargs basename 2>/dev/null || true)"
fi
if [ -z "$IFACE" ] && [ -d "$SYSFS_DEV" ]; then
    IFACE="$(find "/sys/bus/pci/devices/0000:${LOC}/net" -mindepth 1 -maxdepth 1 -type d -print -quit 2>/dev/null | xargs basename 2>/dev/null || true)"
fi
if [ -z "$IFACE" ]; then
    echo "Could not determine network interface for ${LOC}; skipping interface-specific captures." | tee "${OUT}/iface_note.txt"
else
    echo "interface=${IFACE}" | tee "${OUT}/interface.txt"

    # Interface flags, MAC, MTU, counters.
    ip link show dev "${IFACE}" > "${OUT}/ip_link_${IFACE}.txt" 2>&1 || true

    # ethtool basic info (requires root for some fields).
    if command -v ethtool >/dev/null 2>&1; then
        ethtool "${IFACE}" > "${OUT}/ethtool_${IFACE}.txt" 2>&1 || true
        sudo ethtool -i "${IFACE}" > "${OUT}/ethtool_driver_${IFACE}.txt" 2>&1 || true
        sudo ethtool -m "${IFACE}" > "${OUT}/ethtool_module_${IFACE}.txt" 2>&1 || true
        # Dump PHY and MAC registers. This is the most useful oracle for
        # writing a bare-metal driver. Requires root.
        sudo ethtool --register-dump "${IFACE}" > "${OUT}/ethtool_regs_${IFACE}.txt" 2>&1 || true
        sudo ethtool --eeprom-dump "${IFACE}" > "${OUT}/ethtool_eeprom_${IFACE}.txt" 2>&1 || true
    fi

    # MAC address from sysfs (authoritative).
    cat "/sys/class/net/${IFACE}/address" > "${OUT}/mac_address_${IFACE}.txt" 2>&1 || true

    # MII register dump via mii-tool or ethtool -d.
    if command -v mii-tool >/dev/null 2>&1; then
        sudo mii-tool -v "${IFACE}" > "${OUT}/mii_tool_${IFACE}.txt" 2>&1 || true
    fi

    # Running DHCP/address config so we know the expected network params.
    ip addr show dev "${IFACE}" > "${OUT}/ip_addr_${IFACE}.txt" 2>&1 || true
    ip route show dev "${IFACE}" > "${OUT}/ip_route_${IFACE}.txt" 2>&1 || true
fi

# Module info for e1000e.
modinfo e1000e > "${OUT}/modinfo_e1000e.txt" 2>&1 || true
lsmod | grep -E '^e1000e' > "${OUT}/lsmod_e1000e.txt" 2>&1 || true

# e1000e boot/driver messages.
(dmesg | grep -iE 'e1000e|intel.*ethernet|I217' | tail -n 200 > "${OUT}/dmesg_e1000e.txt") 2> "${OUT}/dmesg_e1000e.err" || true

echo "Done. Review ${OUT} before committing."
