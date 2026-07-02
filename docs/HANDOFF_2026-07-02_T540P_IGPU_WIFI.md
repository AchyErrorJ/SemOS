# Handoff — 2026-07-02 — T540p dual-boot, iGPU M14, WiFi resume path

## Machine/workflow reality

The ThinkPad T540p/W540-class laptop is now a **dual-boot metal target**:

- Pop!_OS / Linux for normal verification and hardware-oracle capture.
- SemOS from the internal ESP for metal testing.
- SemOS sysroot blob is staged on the internal raw `SEMOS_SYSROOT` partition.

Important workflow constraint: when doing metal boot work on the T540p, the
machine stops being the dev host. For sustained metal debugging, develop from
**Aesir** — the 3070 desktop/workstation — then boot the T540p into SemOS only
for validation. After a SemOS metal test, boot back into Pop!_OS on the T540p to
collect Linux oracle data or confirm behavior.

Recommended flow:

1. Develop/build on Aesir when the T540p needs repeated SemOS reboots.
2. Push/pull through GitHub.
3. On the T540p Pop!_OS side, pull latest, build if needed, install the SemOS ESP
   files, then reboot SemOS for the one metal test.
4. Keep Pop!_OS as the recovery/oracle OS.

Do **not** make the T540p the only active dev environment while testing boot or
hardware drivers; it slows every iteration.

## Current repo/build state

Branch: `main`  
Remote: `origin = git@github.com:AchyErrorJ/SemOS.git`

Known-good Linux build flow from repo root:

```sh
(cd kernel-x86_64 && cargo build --release)
(cd x86_64-runner && cargo run --release)
```

The second command regenerates:

```text
kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64.img
kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64-bios.img
```

ESP install helper:

```sh
bash tools/esp-install/install-semos-esp.sh
```

The helper copies:

```text
/boot/efi/EFI/SemOS/BOOTX64.EFI
/boot/efi/kernel-x86_64
```

The bootloader expects the kernel file name exactly `kernel-x86_64` at ESP root.

## M14 iGPU state

Added a planning doc:

```text
docs/M14_IGPU_HASWELL_PLAN.md
```

Updated roadmap:

```text
docs/roadmap/map - gpu.md
```

Captured Pop!_OS/i915 oracle data:

```text
docs/hardware/igpu-2026-07-02/
```

Key facts:

```text
iGPU: Intel HD 4600 / Haswell GT2
PCI: 00:02.0
Vendor/device: 8086:0416
Subsystem: Lenovo 17aa:221e
Linux driver: i915
BAR0 MMIO: f1000000, 64-bit non-prefetchable, size 4 MiB
BAR2 aperture: e0000000, 64-bit prefetchable, size 256 MiB
BAR4 I/O: 5000, size 64
Backlight: /sys/class/backlight/intel_backlight
Backlight type: raw
Backlight max/current at capture: 4438/4438
Internal panel: card1-eDP-1
Mode: 1920x1080
DPMS: On
```

Implemented a **read-only** SemOS iGPU probe:

```text
kernel-x86_64/src/igpu.rs
```

Wired it into boot after the PCI bus scan. It only reads PCI config space and
prints GOP framebuffer metadata via new framebuffer helpers. It does **not**:

- read iGPU MMIO;
- write iGPU MMIO;
- resize BARs;
- toggle PCI command bits;
- touch brightness;
- modeset;
- touch NVIDIA.

Expected metal output:

```text
[*] Probing Intel integrated graphics (read-only)...
[igpu] Intel HD 4600 / Haswell GT2 @ 00:02.0 device=0x0416 class=03/00/00
[igpu] subsystem vendor/device=0x17AA:0x221E
[igpu] PCI command: IO=yes MEM=yes BUSMASTER=yes (read-only probe; no writes)
[igpu] BAR0 MMIO BAR0: MMIO64 base=0x00000000F1000000 ...
[igpu] BAR2 aperture BAR2: MMIO64 base=0x00000000E0000000 ...
[igpu] BAR4 I/O BAR4: I/O base=0x5000 ...
[igpu] target match: Haswell GT2 / Intel HD 4600 (8086:0416)
[igpu] GOP framebuffer: 1920x1080 stride=... bpp=... bytes=... fmt=...
[igpu] native-control status: PCI inventory only; GOP framebuffer remains active
```

Next iGPU steps:

1. Boot SemOS on T540p and confirm the read-only probe output.
2. Add `fbinfo` shell command or demo for framebuffer metadata.
3. Use Linux as oracle for `intel_backlight` before writing a SemOS brightness
   path.
4. Only after brightness/app framebuffer work should native Haswell modesetting
   be considered.

## WiFi resume path

Native Intel 7260 WiFi was paused at the AP-ACK wall, not abandoned. Now that the
T540p has working Linux WiFi, it is reasonable to resume — **using Linux as the
oracle**, not by guessing.

This means "crack it" in the project sense: debug SemOS's Intel 7260 driver and
association path. Do not do password cracking or unauthorized network work.

Hardware facts from existing inventory:

```text
WiFi: Intel Wireless 7260
PCI: 04:00.0
Vendor/device: 8086:08b2
Subsystem: Intel Dual Band Wireless-AC 7260 / Wilkins Peak 2, 8086:c270
Linux driver: iwlwifi
```

Why Linux helps now:

- Linux confirms the card, firmware, RF-kill state, regulatory domain, AP mode,
  and successful association path.
- Linux can capture `dmesg`, `iw`, `iw dev ... link`, `iw event`, and possibly
  tracepoints while the same AP succeeds.
- SemOS can compare its command sequence, association request bytes, EAPOL timing,
  and status notifications against a known-good host.

Suggested Pop!_OS oracle capture before touching SemOS WiFi again:

```sh
mkdir -p docs/hardware/wifi-$(date +%F)
out=docs/hardware/wifi-$(date +%F)

lspci -nnvv -s 04:00.0 > "$out/lspci_04_00_0.txt" 2>&1 || true
lspci -nnk -s 04:00.0 > "$out/lspci_04_00_0_nnk.txt" 2>&1 || true
rfkill list > "$out/rfkill.txt" 2>&1 || true
iw dev > "$out/iw_dev.txt" 2>&1 || true
iw reg get > "$out/iw_reg_get.txt" 2>&1 || true
nmcli dev wifi list > "$out/nmcli_wifi_list.txt" 2>&1 || true
ip link > "$out/ip_link.txt" 2>&1 || true

# After connecting normally through Pop!_OS:
iw dev | tee "$out/iw_dev_after_connect.txt"
iface=$(iw dev | awk '/Interface/ {print $2; exit}')
[ -n "$iface" ] && iw dev "$iface" link > "$out/iw_link_after_connect.txt" 2>&1 || true
journalctl -k -b | grep -Ei 'iwlwifi|wlan|wifi|80211|firmware|rfkill' \
  > "$out/kernel_iwlwifi_current_boot.txt" 2>&1 || true
```

If root/debug access is available, useful extras:

```sh
sudo dmesg -T | grep -Ei 'iwlwifi|wlan|wifi|80211|firmware|rfkill' \
  > "$out/sudo_dmesg_iwlwifi.txt"

sudo cat /sys/kernel/debug/ieee80211/phy*/iwlwifi/iwlmvm/fw_ver \
  > "$out/iwlmvm_fw_ver.txt" 2>/dev/null || true
```

SemOS-side likely resume point:

- Review `kernel-x86_64/src/wireless/`.
- Find the exact AP-ACK wall notes in docs/roadmap/history.
- Compare SemOS association request and EAPOL handoff against Linux's connected
  parameters.
- Keep native WiFi work scoped: first reproduce probe/firmware ALIVE, then scan,
  then join, then AP ACK, then EAPOL/DHCP.

Practical note: for near-term network access while debugging WiFi, Ethernet or
USB tether remains easier. Native Intel 7260 should be treated as a hardware
milestone, not the only way to get online.

## Known caveats

- `cargo fmt` failed because `rustfmt` is not installed for
  `nightly-2026-02-01-x86_64-unknown-linux-gnu`. The code builds without fmt.
- Agent sandbox cannot run real `sudo`; user must run sudo/manual captures.
- Debugfs/i915 capture from this session was unavailable; non-sudo captures were
  enough for the read-only iGPU probe.
- Do not run environment-stopping commands from the agent when it is inside the
  target environment. Let the user reboot/shutdown manually.
