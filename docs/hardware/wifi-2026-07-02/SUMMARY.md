# WiFi oracle summary — 2026-07-02 Pop!_OS T540p

Branch: `wifi`

## Hardware / driver

```text
PCI: 04:00.0
Device: Intel Wireless 7260
Vendor/device: 8086:08b2
Subsystem: Intel Dual Band Wireless-AC 7260 / Wilkins Peak 2, 8086:c270
Revision: 0x83
Class: 0x028000
Linux driver: iwlwifi
Linux op mode: iwlmvm
Interface: wlp4s0, renamed from wlan0 by Linux
IRQ: 34
BAR0: 0xf2400000-0xf2401fff, 64-bit memory, size 8 KiB
PCI command: Mem+ BusMaster+
```

## Firmware

```text
Loaded firmware: 17.bfb58538.0 7260-17.ucode
Linux firmware file: /lib/firmware/iwlwifi-7260-17.ucode -> intel/iwlwifi/iwlwifi-7260-17.ucode
SHA256: 5d81a6003df0228a497ad27f916ba2c979614b4c439b0f45a5f2873dc0607fe8
```

Kernel log confirms:

```text
iwlwifi 0000:04:00.0: Detected Intel(R) Dual Band Wireless AC 7260
iwlwifi 0000:04:00.0: loaded firmware version 17.bfb58538.0 7260-17.ucode op_mode iwlmvm
iwlwifi 0000:04:00.0: base HW address: e8:2a:ea:60:ad:bf, OTP minor version: 0x0
ieee80211 phy0: Selected rate control algorithm 'iwl-mvm-rs'
iwlwifi 0000:04:00.0 wlp4s0: renamed from wlan0
```

## Live Linux state

Live capture had to be rerun outside the managed sandbox because netlink/D-Bus
were blocked inside the sandbox.

```text
Interface: wlp4s0
State: UP + LOWER_UP
NetworkManager: connected
Security: WPA2
Band/channel at capture: 5 GHz, channel 149
Advertised rate: 270 Mbit/s
Signal at capture: ~97/100
rfkill: WLAN soft blocked=no, hard blocked=no
Ethernet enp0s25: no carrier
```

`iw` was installed and low-level link/station captures are present:

```text
Interface: wlp4s0
Mode: managed
SSID at capture: 435Northshore
BSSID at capture: 24:2f:d0:ec:83:87
Channel: 149 / 5745 MHz
Channel width: 80 MHz, center1 5775 MHz
Tx power: 22.00 dBm
Signal: -42 dBm, avg about -41 dBm
Authenticated: yes
Associated: yes
Authorized: yes
WMM/WME: yes
MFP: no
Beacon interval: 100
DTIM period: 1
RX bitrate: 325.0 MBit/s VHT-MCS 7 80MHz short GI VHT-NSS 1
TX bitrate: 866.7 MBit/s VHT-MCS 9 80MHz short GI VHT-NSS 2
TX failed: 0
Beacon loss: 0
```

Committed `iw` oracle files:

```text
iw_dev_after_install.txt
iw_wlp4s0_info.txt
iw_wlp4s0_link.txt
iw_wlp4s0_station_dump.txt
```

## Privacy note

`live_wifi_oracle.txt`, `nmcli_wifi_list.txt`, `ip_addr.txt`, and related raw
captures may contain local SSIDs, BSSIDs, MAC addresses, and IP addresses. Do not
commit/push those raw files to a public repository without explicit approval or
redaction. This summary intentionally keeps only the driver-relevant facts.

## SemOS relevance

Linux proves the exact card/firmware path is healthy on this T540p:

1. PCI config and BAR0 are normal.
2. RF-kill is not blocking WLAN.
3. Firmware `7260-17.ucode` loads successfully.
4. Linux reaches `iwlmvm` operational state.
5. Association works on WPA2, 5 GHz channel 149.

This makes the SemOS WiFi resume concrete: compare SemOS's firmware ALIVE, scan,
join/auth/assoc, AP-ACK, EAPOL timing, and notification handling against this
known-good Linux path.
