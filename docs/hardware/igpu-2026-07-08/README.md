# M14 iGPU oracle capture — 2026-07-08

Captured on the ThinkPad T540p-class Pop!_OS host for SemOS M14.

## Usable non-root findings

- iGPU: Intel 4th Gen Core Processor Integrated Graphics Controller / HD 4600
  - PCI BDF: `00:02.0`
  - PCI ID: `8086:0416`
  - Kernel driver: `i915`
  - BAR0 MMIO: `f1000000`, size `4M`
  - BAR2 graphics aperture: `e0000000`, size `256M`
- Internal panel connector: `/sys/class/drm/card1-eDP-1`
  - `status=connected`
  - `enabled=enabled`
  - `dpms=On`
  - advertised sysfs mode: `1920x1080`
  - raw EDID captured as `edid_card1-eDP-1.bin`
  - decoded summary: CMN `N156HGE-EA1`, native timing `1920x1080@60.007Hz`, pixel clock `151.6 MHz`
- Linux backlight provider: `/sys/class/backlight/intel_backlight`
  - `type=raw`
  - `max_brightness=4438`
  - captured `brightness=4438`
  - captured `actual_brightness=4438`

These facts are enough to continue SemOS M14-B/C/D development:

- M14-B read-only probe should target PCI device `8086:0416` at any BDF, observed
  here as `00:02.0`.
- M14-C should compare SemOS GOP framebuffer info against the Linux panel mode
  `1920x1080`.
- M14-D should start with the Intel/raw backlight path and clamp writes to a
  visible floor. Linux's exposed raw max is `4438`.

## Sandbox limitation

The agent environment could not run `sudo` or read `dmesg`:

- `sudo` failed because the runtime has `no new privileges` set.
- `dmesg` failed with `Operation not permitted`.

As a result, the root-only debugfs files currently contain sudo failure text or
are empty. `dmesg_i915.err` records the kernel-buffer permission error:

- `lspci_00_02_0_full.txt`
- `i915_display_info.txt`
- `i915_opregion.txt`
- `i915_frequency_info.txt`
- `i915_runtime_pm_status.txt`
- `i915_ddb_info.txt`
- `dmesg_i915.txt`

## Manual root follow-up

From a normal terminal on Pop!_OS, outside the agent sandbox, run:

```bash
cd /home/jeremieroy/Desktop/Software/SemOS
OUT=docs/hardware/igpu-2026-07-08
sudo lspci -nnvvxxxx -s 00:02.0 > "$OUT/lspci_00_02_0_full.txt"
for f in i915_display_info i915_opregion i915_frequency_info i915_runtime_pm_status i915_ddb_info; do
  sudo cat "/sys/kernel/debug/dri/0/$f" > "$OUT/$f.txt" 2>&1 || true
done
dmesg | grep -iE 'i915|drm|edid|eDP' | tail -n 200 > "$OUT/dmesg_i915.txt"
```

Then commit the updated directory.
