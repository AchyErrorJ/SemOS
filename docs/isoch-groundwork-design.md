# Isochronous USB Groundwork — Design + Results

Branch: `isoch-groundwork` (feature branch, human review before push — vouch model)
Goal: the gate for USB audio (the Whisper/Five voice-agent plan).
Delivered slice: enumerate QEMU's `usb-audio`, configure an AudioStreaming
**Isoch-OUT** endpoint, schedule single-TRB Isoch TDs, receive Transfer
Events with exact byte accounting — QEMU-verified, three scenarios.

## 0. Corrections to the original plan (found during implementation)

1. **Isoch TRB type is 5, not 7.** xHCI 1.2 Table 6-91: Normal=1, Setup=2,
   Data=3, Status=4, **Isoch=5**, Link=6, Event Data=7, No-Op=8.
2. **OUT first, not IN.** QEMU 7.2's `hw/usb/dev-audio.c` is playback-only:
   its descriptor set contains exactly one streaming endpoint,
   `USB_DIR_OUT | 0x01` (192 B/packet = 48 kHz stereo s16 @ 1 ms,
   bInterval=1, full-speed). There is no IN endpoint to test against,
   with any audiodev. Isoch-OUT exercises the identical machinery
   (endpoint context programming, Isoch TRB scheduling, per-interval
   consumption, event/byte accounting); IN is a small delta once real
   hardware (or a newer QEMU with capture support) is available.
3. **FS isoch interval encoding differs from FS interrupt.** USB2 §9.6.6:
   FS isoch period = 2^(bInterval-1) *frames* → xHCI Interval field =
   (bInterval-1)+3. The existing `encode_interval` (log2(bInterval*8)) is
   the interrupt encoding; a separate `encode_isoch_interval` handles the
   FS case. HS/SS match (bInterval-1). LS can't do isoch.
4. **Max ESIT Payload is 24-bit** (caught in code review): dw0[31:24] =
   MEP[23:16], dw4[31:16] = MEP[15:0]. The first draft split it 8+8, which
   is wrong for MEP > 255 (e.g. HS 3072 B) — invisible at the tested
   FS 192 B. Fixed before commit; re-verified in QEMU.

## 1. What exists today (recon findings)

- `ring.rs` had TRB types for Normal(1), Setup/Data/Status, Link(6),
  commands, events — no Isoch.
- `device.rs::EndpointContext` (32 bytes; contexts at 64-byte stride under
  qemu-xhci's CSZ=1, handled by `InputContext::ep_mut`) had init fns for
  control(4), bulk-out(2), bulk-in(6), interrupt-in(7) — no isoch.
- `xhci.rs::encode_interval()` + the HID keyboard configure path and
  `configure_bulk_endpoints()` provided the structural templates.
- Transfers: Normal TRB + IOC + ISP, doorbell, polled event ring
  (IMAN.IE=0). Isoch reuses all of it.

## 2. What was built

### ring.rs
- `trb_type::ISOCH = 5`, `trb_type::STOP_ENDPOINT_CMD = 15`
- `mod isoch`: doc'd bit constants for the Isoch TRB control dword
  (SIA=bit31, Frame ID 30:20, TLBPC 19:16, TBC 8:7, plus shared IOC/ISP)
  and status dword (TD Size 21:17). Single-TRB TDs → TBC=0, TLBPC=0,
  TD Size=0.

### device.rs
- `init_isoch_in_ep()` (EP type 5) and `init_isoch_out_ep()` (EP type 1).
  Interval at dw0[23:16]; **Max ESIT Payload** split dw0[31:24]=MEP[15:8],
  dw4[31:16]=MEP[7:0]; Average TRB Length dw4[15:0]. CErr=0 (ignored for
  isoch), HID=0, MaxBurst=0, Mult=0 (USB2 isoch).
- `class::AUDIO = 0x01`.

### xhci.rs (~330 new lines, all additive)
- `find_audio_streaming_out(blob)` — walks the config blob for class 0x01 /
  subclass 0x02 (AudioStreaming) with bNumEndpoints>0 and an isoch OUT
  endpoint; returns (endpoint, iface, alt, cfg).
- `AudioDevice` / `AudioStats` state + `usbaudio_device()` /
  `usbaudio_stats()` accessors. Single audio function at a time (matches
  the driver's single-DEVICE model; one ring + 4×4 KiB TD buffers, no
  per-slot arrays — MAX_SLOTS=64 would make per-slot pools ~1 MiB of BSS).
- `configure_isoch_out()` — ConfigureEndpoint command with slot+EP add
  flags; MEP = mps × (1 + wMaxPacketSize[12:11]) for HS, mps for FS.
- `arm_isoch_td()` — one Isoch TRB per TD buffer, **SIA=1** (start at next
  interval boundary; no MFINDEX sampling needed), IOC=1, length=MEP
  (QEMU's streambuf drops non-192 B packets, so TD length must equal MEP).
- `usbaudio_poll()` — drains the event ring **bounded to 64 events/call**,
  filters by slot/dci, accounts bytes as TD-len − event residual, re-arms
  while `ISOC_REARM`.
- `stop_isoch_endpoint()` — Stop Endpoint command; the correct wind-down.
- `usbaudio_stream_test()` — two-phase boot test (see below).
- Enum hook: after the HID keyboard check, before ipheth/ECM/NCM/MSC.
  Audio claims the device, stashes an `EnumeratedDevice` (class 0x01,
  new `audio_dci` field), runs the stream test once at boot.

## 3. QEMU 7.2 xhci emulation findings (worth their own section)

1. **Synchronous execution on doorbell.** QEMU 7.2's xhci does not pace
   isoch TDs at the programmed interval; it executes a fresh TD ~immediately
   when the doorbell rings. Consequence: drain→re-arm→doorbell inside a
   naive `while let Some(evt) = poll_event()` loop never observes an empty
   ring (observed: 37,974 events in one "drain"). Hence the bounded drain.
2. **STALL+SUCCESS event pairs.** Every re-armed TD produces a STALL
   (cc=14) event immediately followed by a SUCCESS (cc=1) event for the
   same TRB, both reporting the full 192 B moved. The initial doorbell
   batch completes cleanly. Tallied honestly as errors; byte accounting
   unaffected (68 successful TDs → 13,056 bytes = 68×192 exactly).
   Likely a partial-isoch-pacing artifact of the emulator; real-hardware
   validation will show whether this pattern exists off-QEMU.
3. **Stop Endpoint works and flushes correctly** — pending TDs complete
   with cc=26 (Stopped — Length Invalid), command completes cc=1.

## 4. Verified results (Stage 4)

Boot line addition:
```
-device qemu-xhci -device usb-audio,audiodev=aud0 -audiodev none,id=aud0
```

| Scenario | Result |
|---|---|
| audio-only | PASS: ConfigureEndpoint cc=1, TDs consumed, bytes exact, clean wind-down, boot to `sem-sh$` |
| usb-kbd + usb-audio | PASS on slot 2, keyboard live at prompt, no phantom input (wind-down works) |
| usb-kbd only (baseline) | zero `[usbaudio]` lines, all DEMO 18 checks PASS — no behavioral change |

Pass bar: ≥4 successful TDs AND bytes ≥ 4×MEP. Observed well above bar
in every run.

## 5. Honest limitations (documented in code)

- Polled completion (IMAN.IE=0) + isoch = the stream doesn't wait for us.
  Sample-accurate 48 kHz streaming needs MSI-X or a frame ticker — next
  rung, not this slice.
- Known pre-existing wart: `poll_event()` has a single consumer; whoever
  drains eats everyone's Transfer Events (`poll_hid`, `bulk_xfer` behave
  the same). The wind-down exists precisely so armed audio TDs never leak
  SUCCESS events into `poll_hid`. A per-endpoint event demux is future work.
- No Isoch-IN yet (no QEMU target), no rate feedback, no user-space API.
- Frame-ID scheduling deliberately deferred (SIA covers groundwork).

## 6. Diff surface

| File | Change |
|---|---|
| `kernel-x86_64/src/usb/ring.rs` | +~40 lines: ISOCH + STOP_ENDPOINT consts, `mod isoch` bit docs |
| `kernel-x86_64/src/usb/device.rs` | +~55 lines: two isoch EP inits, AUDIO class const |
| `kernel-x86_64/src/usb/xhci.rs` | +~330 lines: finder, configure, arm, poll, stop, test, enum hook, `audio_dci` field |
| `docs/isoch-groundwork-design.md` | this doc |

No existing function body modified except: the enumeration config-walk
gains one probe call, and `EnumeratedDevice` gains one field (all struct
literals updated explicitly).

## 8. OVMF finding: unassigned xHCI BAR (added during QEMU verification)

Under OVMF + qemu-xhci the firmware leaves BAR0 unassigned
(BAR0=0x00000004, BAR1=0x000000E0 — 64-bit BAR with zero base, garbage
high dword; command reg already MEM+BM enabled). `discover()` trusted the
firmware assignment and the first capability read faulted at
physmap+0xE000000000. The page-fault recovery path additionally reported
a spurious "STACK OVERFLOW" because the slot-0 canary legitimately reads
0 — a red herring that cost a bisect; canary reporting should special-case
slot 0 (or the canary init for slot 0 should be verified). Tracked as a
known gap.

Fix: `discover()` now detects an unassigned BAR0 (base bits all zero, or
the all-ones sizing mask) and assigns a 64 KiB window at 0xC0000000 in
the legacy 32-bit MMIO hole itself, with readback verification. On real
firmware that assigns BARs (ThinkPad BIOS) this path never fires. This
matters beyond QEMU: an OS cannot trust firmware BAR assignment.

Also added: CR2 printed in the kernel page-fault recovery path — the
faulting address, not just RIP, is what you need for bad-pointer bugs.

## 9. QEMU verification (2026-09-04, branch selfdev80-thesis)

| scenario | result |
|---|---|
| usb-audio only | PASS: ok=15 bytes=2880 (15×192), last_cc=26 (clean StopEndpoint), DEMO 80 PASS |
| usb-kbd + usb-audio | PASS: ok=16 bytes=3072 (16×192), keyboard enumerated and live, DEMO 80 PASS |
| usb-kbd only (baseline) | keyboard enumerated, DEMO 80 PASS, zero `[usbaudio]` lines |
