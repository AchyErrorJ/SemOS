# iPhone Sensor Offload Plan (LiDAR / cameras → SemOS)

Status: drafted 2026-06-10, the day ipheth enumeration + bulk data path landed.

## Ground truth about what iOS exposes over USB

| Capability | Without an iOS app? | Mechanism |
|---|---|---|
| Tethered internet (done) | YES | ipheth vendor interface (0xFF/0xFD/0x01), config 4 |
| Stored photos/videos (incl. portrait depth maps in HEIC) | YES | PTP — the class 0x06 interface in config 1; bulk-only protocol, implementable on our EHCI stack |
| File transfer / backups / app install ("iTunes-like") | YES (heavy) | usbmuxd + lockdownd pairing (TLS, plists, pair records) over the 0xFF/0xFE/0x02 interface in config 3 |
| **Live camera, LiDAR, IMU, any sensor stream** | **NO — hard iOS boundary** | Sensors are only reachable through ARKit/AVFoundation running ON the phone. No UVC device mode, no raw USB endpoints. Continuity Camera is Apple-proprietary, Mac-only |

Conclusion: for live LiDAR point clouds, an **iOS companion app is unavoidable** —
but the "iTunes-like host app" is NOT the required piece. The required piece is an
app on the phone that captures and streams; the host side just needs a byte pipe.

## The key insight: we already have the byte pipe

The ipheth tether is a full IP link between the phone and SemOS
(phone = 172.20.10.1, SemOS = 172.20.10.9). An iOS app can open a plain
TCP/UDP socket to SemOS over that link. **No usbmuxd, no lockdownd pairing
crypto, no Apple MFi.** This is the cheapest path to "bare-metal OS ingesting
iPhone LiDAR over a USB cable" — which appears to be genuinely novel.

Bandwidth check: ARKit depth is 256×192 @ up to 60 fps. Depth (f32→u16) +
confidence + camera pose ≈ 100 KB/frame → 3–6 MB/s at 30–60 fps ≈ 25–50 Mbps.
USB 2.0 HS tether realistically delivers 100–200 Mbps. Comfortable headroom;
full RGB frames alongside would need JPEG/HEVC compression on-phone.

## Plan

### Phase A — validate the tether data path (SemOS, this week)
1. W540 test of the just-landed bulk path: boot with hotspot on → `[net] smoltcp
   interface up ... on ipheth0` → first RX frame flips `confirmed_pairing`.
2. Prove traffic: DNS query to 172.20.10.1, then shell `fetch` through the
   phone (real internet over the tether). Add a `tether` shell builtin if
   useful (carrier check 0x45 + counters).

### Phase B — point-cloud ingest service (SemOS)
3. UDP listener on the SemOS side (socket slots already exist in net/state.rs;
   may need to bump SOCKET_SLOTS). Simple framing: magic + frame seq + pose
   (4×4 f32) + W×H u16 depth + u8 confidence.
4. Renderer demo: top-down / orbit projection of incoming points into the
   framebuffer via gfx2d — "DEMO: live LiDAR on bare metal".

### Phase C — iOS companion app
5. Minimal SwiftUI + ARKit app: ARSession with sceneDepth, per-frame
   depthMap + confidenceMap + camera transform → UDP to 172.20.10.9.
   Needs a free Apple developer account + Xcode sideload (7-day resign) or a
   $99 account. Hotspot ON during capture (the tether IS the transport).
6. Stretch: RGB keyframes (HEVC), IMU stream, tap-to-anchor.

### Phase D — optional native-USB depth (post-MVP)
7. PTP class driver over EHCI bulk: pull camera-roll HEICs without any app
   (portrait depth maps included) — "photos in, zero phone-side code".
8. usbmuxd/lockdownd host implementation (the original 6-session plan):
   pairing, AFC file access, app-launch integration. Only if app-store-less
   operation or file-level integration becomes a goal.

## Other W540 devices enumerated 2026-06-10 (driver triage)

| Device | What | Driver path | Verdict |
|---|---|---|---|
| 0x14CD:6500 | SD card reader (USB MSC) | mass_storage.rs exists but is xHCI-coupled; EHCI bulk primitives all exist | ~1 session, gives SD slot |
| 0x04CA:7035 | Integrated webcam | UVC class — standard but streams over ISOCHRONOUS; EHCI driver has no periodic schedule yet | Big (periodic schedule + iTDs + UVC); park until wanted |
| 0x8087:07DC | Intel Bluetooth | USB-HCI transport + full BT stack | Subsystem-sized; park |
| 0x138A:0017 | Validity fingerprint | Proprietary, reverse-engineered only | Low value; skip |
| Dock GbE (RTL8153, behind dock hub) | Wired ethernet | Bulk-only protocol — fits the EHCI engine today | Good follow-up: reliable wired net without the phone |
