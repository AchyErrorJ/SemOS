# T540p Intel I217-LM Ethernet — First Cable Validation

**Prepared:** 2026-07-20  
**Status:** first metal run captured 2026-07-21; link/RX work, TX descriptor
fetch/completion is under repair  
**Target:** Lenovo T540p/W540-class Intel I217-LM (`8086:153a`, PCI `00:19.0`)  
**Driver:** `kernel-x86_64/src/e1000e.rs` (`e1000e0`, polled RX/TX)

This is the exact checklist for the next time an Ethernet cable is available
(the current cable is reserved for the Orange Pi / LegiView Firmware work).

## What was pre-staged

- `SYS_NETINFO = 116` (read-only; number was freed when Ring-0 games moved out).
- `sem-sh` builtin: `netinfo`.
- e1000e read-only diagnostic dump:
  - link/speed/duplex;
  - `CTRL`, `STATUS`, `RCTL`, `TCTL`;
  - hardware `RDH/RDT/TDH/TDT`;
  - software ring indices and descriptor-done counts;
  - TX/RX calls, packets, bytes, drops/timeouts/would-block/bad descriptor
    counters and last RX length/error.
- Net-stack snapshot:
  - active device/MAC/link;
  - IPv4/prefix/gateway/DNS;
  - DHCP started/lease state;
  - poll call/work counters.
- The real e1000e boot path now starts DHCP immediately. Previously it left
  smoltcp on QEMU SLIRP's fallback `10.0.2.15/24`, which is not a valid
  assumption on a physical LAN.
- The interactive session now calls `kernel_core::net::poll()` once per shell
  wait tick so DHCP/ARP/socket timers progress while the prompt is idle.

## Build order (important)

`sem-sh` is embedded into the kernel with `include_bytes!`, so rebuild it
**before** the kernel:

```sh
cd user-programs/sem-sh
cargo build --release

cd ../../kernel-x86_64
cargo build --release
```

Then rebuild/install the boot image using the normal T540p UEFI workflow.

## Test setup

1. Connect the T540p directly to a normal DHCP-capable router/switch.
2. Prefer a known-good cable/port.
3. Keep serial logging available if possible, but `netinfo` is designed to
   provide the important state directly at the framebuffer shell.
4. Boot SemOS and wait at least 5 seconds at the shell prompt for DHCP.

## Expected boot evidence

Look for:

```text
[*] Probing Intel e1000e Ethernet device...
[e1000e] PCI 00:19.0 ... ven=0x8086 dev=0x153A
[e1000e] MAC xx:xx:xx:xx:xx:xx
[e1000e] link UP ...
[registry] Registered net device: e1000e0
[e1000e] registered with driver registry as 'e1000e0'
[net] smoltcp interface up: 10.0.2.15/24 via 10.0.2.2 on e1000e0
[net] DHCP client started
[net] DHCP lease: <LAN address>/<prefix> via <router> dns <dns>
```

The `10.0.2.15` line is only the temporary fallback before the DHCP lease.

## Shell validation

### 1. Baseline

```sh
netinfo
```

Healthy after DHCP:

- `stack=UP device=e1000e0 link=UP`
- `DHCP started=yes lease=yes`
- IPv4 is the LAN's address, **not** `10.0.2.15`
- `RCTL` receiver-enable and `TCTL` transmitter-enable bits are set
- TX/RX timeout/bad-descriptor counters are zero

### 2. HTTP round trip

```sh
fetch http://example.com/
```

Then:

```sh
netinfo
```

Expected counter movement:

- `tx ok` and `tx bytes` increase (ARP, DNS, TCP);
- `rx ok` and `rx bytes` increase;
- `polls worked` increases;
- `tx timeouts=0`, `bad_desc=0`.

### 3. Repeated traffic

```sh
fetch http://example.com/
fetch http://example.com/
netinfo
```

This checks descriptor recycling, not just one lucky packet.

## Failure triage

### Link is DOWN

`netinfo`:

- inspect `STATUS`, `CTRL`, speed/duplex;
- verify cable/router port LEDs;
- try another cable/port;
- compare with the Linux capture under
  `docs/hardware/e1000e-2026-07-08/`.

If Linux links but SemOS remains down, next work is PHY/PCH configuration
(SemOS currently relies mostly on BIOS-negotiated PHY state).

### Link UP, DHCP lease stays `no`

Use counters/rings:

- TX calls rise but TX OK does not → TX descriptor/doorbell issue.
- TX OK rises, RX stays zero → receive filter/ring/PHY path issue.
- RX calls/would-block rise, but hardware `RDH` never moves → NIC is not DMAing.
- Hardware `RDH` moves but software sees no descriptor `DD` → descriptor
  format/cache/physical-address issue.
- RX packets arrive but DHCP still fails → inspect first Ethernet frames
  (next diagnostic step: bounded ARP/DHCP hex dump).

### `fetch` DNS failure after DHCP

- Confirm DNS in `netinfo`.
- If DNS is `0.0.0.0`, DHCP did not provide one; test router IP as DNS or add a
  configured fallback.
- If TX/RX counters move, focus on DHCP/DNS parsing rather than the NIC.

### TX timeout

- Compare software `submit/reclaim` with hardware `TDH/TDT`.
- Confirm TX descriptor `DD` count.
- If `TDT` advances and `TDH` does not, inspect TCTL, descriptor physical
  address, bus mastering, and PCH-specific transmit configuration.

## Acceptance gate

Ethernet is considered working only when one T540p boot demonstrates:

1. I217-LM probe + MAC + link UP;
2. DHCP lease received;
3. `fetch http://example.com/` returns an HTTP response;
4. two consecutive fetches work;
5. `netinfo` shows TX and RX packet/byte growth with zero TX timeout and zero
   bad RX descriptors;
6. the result and representative `netinfo` output are added to this document.

## First metal result — 2026-07-21

The first cable boot proved:

```text
stack=UP device=e1000e0 link=UP registered_devices=2
MAC=54:EE:75:16:92:F9
DHCP started=yes lease=no

link=UP speed=1000 Mb/s duplex=full STATUS=0x00080483
RCTL=0x04008002 TCTL=0x0103F0FA

TX hw head=0 tail=1  sw submit=1 reclaim=0  DD=0/16
tx calls=1 ok=0 bytes=0 drops=1 timeouts=1

rx calls=223 ok=189 bytes=20360 wouldblock=34
bad_desc=0 truncated=0 last_len=82 last_errors=0x00
```

Interpretation:

- PCI/MMIO, bus mastering, MAC, PHY link, receive DMA/ring recycling, and
  smoltcp polling work (189 clean RX frames).
- The TX tail doorbell reaches hardware (`TDT=1`), but hardware never advances
  the head (`TDH=0`) or writes descriptor done (`DD=0`).
- This rules out a general DMA-address or link problem and localizes the fault
  to I217/PCH-LPT transmit-engine initialization.

The next build programs the PCH-LPT requirements used by Linux e1000e before
`TCTL.EN`: TXDCTL full-descriptor writeback + required bit 22, TARC0/TARC1
arbitration bits, and TIPG IPGT=8 for 1 Gb/s. `netinfo` now also prints
TIPG/TXDCTL0/TXDCTL1/TARC0/TARC1 for confirmation.

## Second metal result — 2026-07-21

The first PCH-register build applied its writes, but TX still did not advance:

```text
TIPG=0x00602008
TXDCTL0=0x00410000 TXDCTL1=0x00410000
TARC0=0x0D800403 TARC1=0x45000403

TX hw head=0 tail=4 sw submit=4 reclaim=0 DD=0/16
tx calls=8 ok=0 bytes=0 drops=4 timeouts=4

rx calls=48377 ok=290 bytes=28934
bad_desc=0 truncated=0 last_errors=0x00
```

The registers exposed two implementation mistakes:

1. `TXDCTL_FULL_TX_DESC_WB` is `0x01010000` (GRAN bit 24 plus WTHRESH=1);
   the first build only set WTHRESH, producing `0x00410000` after bit 22.
2. SemOS replaced TCTL with `0x0103F0FA`, while the Linux oracle capture has
   `TCTL=0x3103F0FA`. Replacing the register cleared hardware-specific bit 29
   and omitted MULR bit 28.

The next build corrects full-descriptor writeback, changes TCTL programming to
read-modify-write (preserving NVM/hardware bits), enables MULR, and clears the
paired TARC1 bit 28. `netinfo` also gains TDBAL/TDBAH/TDLEN, the last submitted
descriptor contents, TX buffer physical address, and MAC TX statistics.
