# Pending boot validation (drives down — started 2026-06-15)

Running list of changes built into the image but **NOT yet boot-tested**, because
the T540p's boot drives died 2026-06-15. When new drives arrive, flash the latest
image and walk this list top-to-bottom. **Last image actually booted: 17:39**
(WiFi reorder that cleared the `0x90A` off-channel-TX fault → `error table clean`,
but `consumed=0`). Everything below is unvalidated.

Update this file as more changes land while the drives are out.

> **2026-06-28 decision: native iwlwifi is paused.** The historical WiFi boot
> validation items below are preserved as context, but they are no longer the
> next-session gate. Latest metal result: auth TX reaches firmware `TX_RESP`, but
> the AP reports **NO ACK / did not hear us** across protected time-event/quota,
> RX-survival probe, and antenna/rate sweeps. Near-term validation moves to a USB
> network dongle / tether path.


---

## How to validate
Flash the **latest** image (highest timestamp), boot the T540p, then:
1. Watch the early boot for the WPA2 crypto KAT lines (no command needed).
2. For networking, prioritize USB dongle / tether validation (`usbinfo`, `usbenum`,
   then the relevant USB NIC/tether command path) rather than native `wifi connect`.
3. Only run `wifi connect <n> <pass>` if deliberately resuming the paused iwlwifi
   AP-ACK investigation; capture `auth attempt`, `TX_RESP`, and `[rxsurv]` lines.

---

## Image 18:25 — WiFi data-queue enable via direct SCD registers
**What changed:** `enable_data_queue` rewritten to configure the SCD hardware
registers directly (the exact sequence `tx_init` uses for the working cmd queue),
instead of `SCD_QUEUE_CFG (0x1d)` which didn't schedule the queue.
**Verify:** in `wifi connect`, the `TX diag` line →
- ✅ success = **`consumed=1`** (SCD_rdptr 0→1) + ideally a `0x1C` TX-status notif.
- ⛔ still `consumed=0` → register-config alone isn't scheduling queue 1; next
  suspect is the SCD `EN_CTRL` bit (address looked off vs iwm). This is THE gate
  for the whole on-air path.

## Image 19:29 — loader keystone + WPA2 PMK/PTK/MIC KATs + RSN IE
**What changed:**
- `handle_spawn` hardcoded `/bin` program table removed → any ramfs/namespace ELF
  runs by name (self-extension keystone).
- `wpa2::ptk()` + `eapol_mic()` added with cross-impl KATs.
- `RSN_IE_WPA2_PSK_CCMP` + `build_association_request_wpa2()`.
**Verify:**
- Early boot prints `[wpa2] self-test: SHA1 PASS, PMK PASS, PTK PASS, EAPOL-MIC PASS`.
- (loader) regression check: `sem-sh` still spawns existing programs by name
  (e.g. an existing `/bin/<demo>` still runs) — the table removal shouldn't break
  anything that worked.

## Image 20:05 — WiFi WPA2 4-way wiring
**What changed:** `finalize_eapol_mic`, `build_eapol_msg4`, `parse_eapol_key`,
`eapol_self_test`; `send_assoc`; `handshake_step` (Msg1→PTK→Msg2, Msg3→verify→Msg4);
ConnState gained snonce/kck/kek/tk.
**Verify:**
- Early boot adds `[wpa2] EAPOL self-test: Msg2 len PASS, MIC PASS`.
- (No live handshake yet — gated on `consumed=1` from 18:25. These are KATs only.)

## Image 20:17 — tier-0 fence (deny-by-default for created tools)
**What changed:** `spawn_namespace_elf` caps any namespace executable to tier 0
unless `ObjectFlags::VOUCHED_EXEC` is set. Baked `/bin` programs unaffected.
**Verify:**
- Regression: existing baked programs still run normally (they bypass the fence).
- If anything currently installs to an absolute namespace path and relies on a
  higher tier, expect a `unvouched tool fenced to tier 0: <path>` log + possible
  reduced behavior — that is the intended fence, not a bug. Note any such case so
  we decide whether it needs vouching.

## Image 20:40 — vouch v1 (kernel side)
**What changed:** `SYS_VOUCH=126` + ephemeral vouch table (suid→tier+SHA-256) +
authority marker (only the interactive sem-sh task may vouch) + `spawn_namespace_elf`
now grants a tool's tier from a vouch *with a hash recheck*. `interactive_session`
sets the shell as the vouch authority. (Shell `vouch`/`vouches` builtins NOT yet
added — needs the two-step user-program build; kernel mechanism is in place.)
**Verify:** regression only for now — existing programs still spawn; no spurious
`[vouch]` logs at boot. Full test waits on the shell builtins + a real tool to vouch.

## Image 21:09 — vouch v1 shell builtins (vouch now fully usable)
**What changed:** `SYS_VOUCH=126` in std-shim; sem-sh `vouch <path> [tier]` +
`unvouch <path>` builtins (console-only gate enforced kernel-side). Two-step build
(sem-sh → kernel → image). **Verify:**
- `vouch` with no args prints usage (builtin is wired).
- Real test (needs a created tool): compile a tool to `/apps/X`, run `X` → it's
  tier-0 (fenced); `vouch /apps/X 1` → `X` now runs at tier 1; `unvouch /apps/X`
  → back to tier 0. And confirm the **agent cannot vouch** (only the interactive
  shell is the authority).

## Image 21:40 — NVIDIA dGPU probe (M18 step 1, read-only)
**What changed:** new `gpu.rs` — PCI scan for NVIDIA (0x10DE, class 0x03) + BAR
report at boot (`[*] Probing NVIDIA dGPU...`). MMIO chip-ID read gated off
(`MMIO_PROBE=false`, Optimus-power-gate / unmapped-BAR risk). **Verify:**
- Boot log shows `[gpu-pci] found <chip> @ bb:ss.f device=0x.... BAR0=.. BAR1=..`
  → **settles which GPU is actually in the machine** (expect GK208 GT 740M,
  device id ~0x1292). Capture the exact device id + BARs — that's real M18 data.
- Safe / read-only; should not affect anything else. (If it ever #PFs, that's the
  optional MMIO read — but it's gated off, so config-space only.)

---

## Quick pass/fail summary to capture on first boot
- [ ] USB network dongle/tether enumerates (`usbinfo`/`usbenum` show the device and endpoints)
- [ ] USB network path can send/receive at least one frame or static-IP ping/fetch equivalent
- [ ] `[wpa2] self-test ... PTK PASS, EAPOL-MIC PASS` (historical regression)
- [ ] `[wpa2] EAPOL self-test: Msg2 ... MIC PASS` (historical regression)
- [ ] existing `/bin` programs still spawn by name (loader regression, 19:29)
- [ ] no unexpected `fenced to tier 0` on normal programs (20:17)

Historical WiFi note: the `consumed=0` queue wall is no longer current; the latest
state reaches TX_RESP, but AP no-ACKs the auth frame. Native PCI WiFi is paused.
If deliberately resuming it, collect `auth attempt`, `TX_RESP`, and `[rxsurv]`
logs and compare against the AP-ACK wall in the roadmap.
