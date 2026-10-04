# Audio IN (voice-assistant mic path) — design

Status: HDA capture implemented, QEMU-verified (DEMO 97); USB isoch-IN
implemented, T540p-test-pending. 2026-09-09.

North star: SemOS runs the smart-home hub, and the hub *listens*. This is
the input half of the audio stack; the isoch groundwork (USB audio OUT,
DEMO-verified in QEMU) is the output half.

## 0. Two capture paths, different verifiability

| path | where it runs | QEMU-verifiable? |
|---|---|---|
| **HDA capture** (Intel HDA controller + codec ADC) | T540p onboard mic/line-in; QEMU `intel-hda` + `hda-duplex` | YES (DEMO 97) |
| **USB isoch-IN** (xHCI isochronous IN TDs) | USB mic/headset on the T540p | NO — QEMU 7.2's usb-audio is playback-only, so capture never delivers frames |

v1 does both: HDA first (fully provable in QEMU), USB isoch-IN as the same
machinery mirrored to the IN direction (compile-verified, marked
machine-test-pending — first real USB mic on the T540p is the test).

## 1. HDA capture (DEMO 97)

The existing HDA driver (M15) brings up controller + CORB/RIRB, walks the
first codec for a DAC + output pin, and plays a sine (validation = "LPIB
advanced while RUN"). Capture mirrors it exactly, in reverse:

- **Widget walk**: in addition to DAC + output pin, find the ADC (widget
  type 0x1, Audio Input) and an input-capable pin (pin widget whose pin
  caps report IN). QEMU's `hda-duplex` codec provides both.
- **Input stream**: stream descriptor index 0 (input SDs come first, at
  MMIO 0x80; output SDs start at 0x80 + 0x20*ISS). Same programming as
  output: CBL, LVI=0, FMT (48 kHz / 16-bit / stereo), BDL → a page-aligned
  capture buffer, stream tag 2, RUN.
- **Codec verbs**: ADC Set Converter Format + Set Converter Stream/Channel
  (tag 2), unmute the ADC input amp, pin power-up + IN_EN (0x707 bit 5).
- **Validation without ears**: the capture buffer is pre-filled with a
  canary pattern (0xAA); after ~1 s of RUN, PASS requires
  (a) LPIB advanced (DMA-in happened at the expected cadence:
  48 000 × 4 B/s), and
  (b) the canary is gone — the controller overwrote the buffer with real
  codec data (with QEMU's `none` audiodev backend that data is silence,
  i.e. zeros — the point is the DMA path, not the content).

QEMU: `-device intel-hda -device hda-duplex,audiodev=snd0 -audiodev none,id=snd0`.
With plain `hda-output` (no ADC), the capture test skips cleanly.

## 2. USB isoch-IN (machine-test pending)

Mirrors the isoch groundwork's OUT slice:

- Endpoint selection: AudioStreaming interface, isoch endpoint with
  direction IN (bEndpointAddress bit 7 set) — the groundwork already scans
  descriptors; IN uses DCI = ep*2+1 (OUT used ep*2).
- Isoch TDs: single-TRB, TBC=0/TLBPC=0, SIA=1, DIR bit set for IN; the
  controller writes received frames into the TD's DMA buffer.
- Event harvest: on completion, copy the frame into a static capture ring
  and re-arm; tallies `in_ok` / `in_bytes` alongside the OUT counters.
- Cadence check on real hardware: bInterval service, N frames/s, bytes =
  frames × frame-size; a USB mic delivering non-silence (clap test) is the
  content proof QEMU can't give.

## 3. What v1 deliberately does NOT build

- No resampling/mixing/AGC (one fixed format: 48 kHz, 16-bit, stereo).
- No ring-3 audio API yet — capture lives in the kernel as validated DMA;
  the voice-assistant pipeline (wake word → ASR) consumes it later.
- No USB isoch-IN *verification* until the T540p (QEMU can't source audio).
- No HDA jack-detection / auto mic selection (first ADC wins).
