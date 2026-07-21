# SemOS ESP install helpers

These scripts install the already-validated SemOS UEFI boot files onto the
existing Pop!_OS ESP without repartitioning.

## Normal update: build, wrap, and copy in one command

Once SemOS has already been installed on the ESP, use:

```sh
bash tools/esp-install/build-and-flash.sh
```

That script:

1. rebuilds the normal embedded user programs (`sem-sh`, demos, etc.);
2. builds `kernel-x86_64` in release mode;
3. runs `x86_64-runner` to regenerate the UEFI + BIOS images;
4. extracts the UEFI image;
5. backs up `/boot/efi/kernel-x86_64`;
6. copies the new kernel to the ESP and runs `sync`.

It asks for `YES` before the ESP write. Useful options:

```sh
bash tools/esp-install/build-and-flash.sh --dry-run
bash tools/esp-install/build-and-flash.sh --build-only
bash tools/esp-install/build-and-flash.sh --no-build
bash tools/esp-install/build-and-flash.sh --yes
```

Use `--full` only for first-time setup (or when the EFI loader itself changes):

```sh
bash tools/esp-install/build-and-flash.sh --full
```

The full path delegates to `install-semos-esp.sh` and creates the firmware boot
entry. The default update path replaces only the kernel, avoiding duplicate
NVRAM entries.

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
