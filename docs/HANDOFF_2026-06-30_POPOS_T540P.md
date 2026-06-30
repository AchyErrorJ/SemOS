# Handoff — T540p Pop!_OS workstation transition (2026-06-30)

## Situation

The ThinkPad T540p is being turned into the main **SemOS x86 workstation**:

- Pop!_OS is now installed on the internal 256 GB SSD.
- SemOS will be built from that Pop!_OS environment and added as a UEFI boot entry later.
- The user plans to upgrade to 16 GB RAM, add ~1 TB storage, and add ExpressCard RS-232 for serial logs.
- Native Intel 7260 WiFi is deliberately paused at the AP-ACK wall; near-term networking is Ethernet/iPhone tether/USB NIC, not PCI WiFi.

This file is for the next agent after the repo is cloned on Pop!_OS.

---

## Important repo state committed in this handoff

### 1. iwlwifi diagnostics / gated MCC work

Files:

- `kernel-x86_64/src/wireless/iwlwifi_fw_image.rs`
- `kernel-x86_64/src/wireless/iwlwifi_device.rs`

What changed:

- Firmware TLV API/capability bitmap parsing was added.
- `ParsedFw::lar_supported()` and `ParsedFw::mcc_update_v2()` were added.
- `maybe_mcc_update(&fw, b"US")` was wired after runtime PHY config, but **capability-gated**.
- The embedded `iwlwifi-7260-17.ucode` advertises:
  - `LAR_SUPPORT = 0`
  - `WIFI_MCC_UPDATE = 0`
  so `MCC_UPDATE_CMD (0xC8)` is skipped on the current blob. Do **not** send it unconditionally.
- Added `rx_survival_probe("post-auth-TX", 5000)` to distinguish:
  - RX path wedged after TX, vs.
  - RX flowing/reviving while AP simply does not ACK.
- Added auth TX antenna/rate sweep:
  - `1M-CCK antA`
  - `1M-CCK antB`
  - `1M-CCK antA|B`
  - `6M-OFDM antA`
  - `6M-OFDM antB`

Latest metal finding from the user: the firmware reaches `TX_RESP`, but the AP says **NO ACK / AP did not hear us**.

Conclusion: stop burning time on native iwlwifi for now.

### 2. Roadmap updates

Files:

- `docs/ROADMAP.md`
- `docs/MASTER_ROADMAP.md`
- `docs/roadmap/map - networking.md`
- `docs/KERNEL_SURFACE.md`
- `docs/PENDING_BOOT_VALIDATION.md`

What changed:

- Native PCI iwlwifi is marked **PAUSED at AP-ACK wall**.
- Active near-term metal networking is now:
  - built-in Ethernet if/when cable/driver path is used,
  - iPhone USB tether / phone bridge,
  - or USB Ethernet-class dongle / simple USB NIC.
- Security docs record that the iwlwifi opaque firmware remains inventoried but is no longer the active near-term network path.

---

## Pop!_OS workstation setup goals

After cloning this repo on Pop!_OS, do these first.

### 1. Capture hardware inventory from Linux

Run and save output, ideally into a `notes/` or `docs/hardware/` file:

```sh
lsblk -f
sudo parted -l
sudo bootctl status
lspci -nnk
lsusb
lsusb -t
dmesg -T
ip link
rfkill list
```

If NVIDIA tooling is installed:

```sh
nvidia-smi || true
glxinfo -B || true
```

This Linux baseline becomes the oracle for SemOS hardware work.

### 2. Install build prerequisites

On Pop!_OS:

```sh
sudo apt update
sudo apt install -y git curl build-essential python3 pkg-config gdisk gparted
```

Install Rust via rustup if not already present:

```sh
curl https://sh.rustup.rs -sSf | sh
source "$HOME/.cargo/env"
```

The repo has toolchain files; let rustup install what Cargo asks for.

### 3. Clone and build

```sh
git clone https://github.com/AchyErrorJ/SemOS.git
cd SemOS
cd kernel-x86_64
cargo build --release
cd ../x86_64-runner
cargo run --release
```

Expected outputs:

```text
kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64
kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img
kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64-bios.img
```

---

## SemOS dual-boot plan on Pop!_OS

Do **not** raw-write SemOS images over the whole SSD.

Mental model:

```text
ESP / EFI System Partition:
  shared boot files only

Pop!_OS partitions:
  Linux system/data

SEMOS_SYSROOT partition:
  raw SemOS compiler sysroot blob target
```

### ESP / UEFI boot entry

After Pop!_OS is stable, inspect the ESP before changing it:

```sh
lsblk -f
sudo bootctl status
sudo find /boot/efi -maxdepth 4 -type f | sort
```

Then create a safe install script to copy SemOS EFI boot files into something like:

```text
/boot/efi/EFI/SemOS/
/boot/efi/loader/entries/semos.conf
```

Do not guess the file layout until the generated UEFI image is inspected.

### SEMOS_SYSROOT partition

The sysroot code searches GPT partition names for exactly:

```text
SEMOS_SYSROOT
```

The safety behavior in `kernel-core/src/sysroot_blob.rs` is:

- no GPT: legacy raw LBA0 mode,
- GPT + `SEMOS_SYSROOT`: use that partition's first LBA,
- GPT + no `SEMOS_SYSROOT`: refuse to touch LBA0.

So on the Pop!_OS SSD, only flash sysroot after creating/naming a partition `SEMOS_SYSROOT`.

To rename partition N later:

```sh
sudo sgdisk -c N:SEMOS_SYSROOT /dev/sdX
```

Do **not** run this blindly; first inspect `lsblk -f` and `sudo parted -l`.

---

## Sysroot flashing reminder

SemOS can boot without the sysroot blob. The blob is needed for on-device rustc/sysroot work.

Current in-OS flow:

- Put `SYSROOT.IMG` on a FAT USB stick.
- Boot SemOS with the USB stick attached.
- Run the SemOS shell/syscall path that invokes `SYS_FLASH_SYSROOT`.
- It scans `usb0..usb3` for `SYSROOT.IMG` and writes it to `sata0` at:
  - `SEMOS_SYSROOT` partition first LBA if present, else
  - legacy raw LBA0 only if no GPT exists.

On a Pop!_OS disk, the correct success logs are:

```text
[sysroot] using SEMOS_SYSROOT partition at LBA ...
[sysroot] blob found: N file(s)
```

If it says GPT partitioned but no `SEMOS_SYSROOT`, stop and create/name the partition.

---

## Networking direction

Do not resume native Intel 7260 WiFi unless explicitly requested.

Near-term useful networking options:

1. Built-in Ethernet if the cable is available and a SemOS NIC driver is scoped.
2. iPhone USB tether / phone bridge.
3. USB Ethernet-class dongle or simple USB NIC.

Avoid USB WiFi dongles as the next SemOS target if possible; many are RTL8188/Realtek USB WiFi and would be another firmware + 802.11 driver project.

---

## Serial direction

The user is getting an ExpressCard RS-232 adapter. Once available:

- identify it under Pop!_OS with `lspci -nnk` / `dmesg -T`,
- determine whether it is standard 16550-compatible UART or needs a PCI serial driver quirk,
- add/enable SemOS serial output to that port,
- make it the main metal debug channel.

This is high leverage and should come before deep GPU work.

---

## Graphics direction

Suggested order after workstation + networking + serial:

1. Intel integrated graphics / framebuffer/modes/backlight first.
2. NVIDIA dGPU later as a research/compute track.

Pop!_OS with Linux drivers is the baseline oracle for PCI IDs, modes, power state, and dGPU behavior.

---

## First tasks for the next agent

1. Confirm Pop!_OS clone/build works.
2. Capture hardware inventory and disk layout.
3. Build + wrap SemOS from Pop!_OS.
4. Inspect ESP/systemd-boot layout.
5. Create a safe SemOS install-to-ESP script.
6. Create/verify `SEMOS_SYSROOT` partition; do not flash sysroot until the partition name is confirmed.
7. Decide next metal network path: Ethernet cable, iPhone tether, or USB NIC.
