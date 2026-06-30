# SemOS ESP install helpers

These scripts install the already-validated SemOS UEFI boot files onto the
existing Pop!_OS ESP without repartitioning.

They intentionally keep Pop!_OS/systemd-boot untouched:

- copy SemOS loader to `/boot/efi/EFI/SemOS/BOOTX64.EFI`
- copy the kernel to `/boot/efi/kernel-x86_64`
- create a firmware boot entry labeled `SemOS` pointing directly at the SemOS EFI loader

The bootloader crate looks for the kernel file named exactly `kernel-x86_64` on
the same FAT filesystem, so that kernel file must currently live at the ESP root.

## Install

From repo root:

```sh
bash tools/esp-install/install-semos-esp.sh
```

The script prints disks and asks for `YES` before writing to the ESP.

After installing, reboot and press ThinkPad **F12**, then choose **SemOS**. Keep
Pop!_OS as the default entry until SemOS has been proven from the internal SSD.

## Uninstall

```sh
bash tools/esp-install/uninstall-semos-esp.sh
```

This removes `/EFI/SemOS`, `/kernel-x86_64`, and optionally removes firmware
entries whose label contains `SemOS`.
