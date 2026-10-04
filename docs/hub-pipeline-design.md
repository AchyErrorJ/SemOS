# Hub pipeline (DEMO 98) — design

Status: implemented, QEMU-verified. 2026-09-09.

The north-star loop — "the hub hears a command and acts on it" — closed
end-to-end in QEMU. This is the product half of the voice-assistant /
smart-hub story; the audio-in stack (DEMO 97 + USB isoch-IN) is the mic
that will eventually feed the gateway.

## Architecture

```
voice gateway / phone / ASR upstream          device shim (lamp, switch, …)
        │ TCP :9001  ▲ reply                         ▲ UDP :<port>
        ▼            │                               │
   hub task (kernel) ── intent match ── action dispatch
        ▲
        │ /var/lib/hub/intents/*.intent  (SemFS-journaled)
   semos install <pkg> — the hub's vocabulary is DATA installed by packages
```

- **Intents are data, not code.** A semos-pkg package can carry an
  `intentbytes` block (registry format v1.1); install registers it into the
  journaled namespace. Spec: `patterns=a,b|action=udp:PORT:$CMD|reply=text`
  (`$CMD` expands to the received command). One intent covers every
  phrasing of a device family ("lights on" / "lights off" / "lights 50%").
- **The kernel executes actions.** Guests compiled by semos-rustc have five
  sys stubs (no net), so the action tool is declarative: the hub — running
  in the kernel with hardware authority — fires the UDP datagram itself.
  UDP because the kernel TCP stack is single-connection and the hub holds
  the gateway channel; netlog already proved the primitive.
- **Persistence for free**: the intent vocabulary rides the SemFS journal,
  so a hard kill loses nothing — boot 2 installs nothing and the hub still
  knows "lights".
- **Governance unchanged**: intents only enter via `semos install`, which
  is console-gated + human-approved; `hub start/stop` are console-only
  (SYS_HUB 144); `hub intents` is read-only.

## QEMU verification (DEMO 98)

`tools/run-hub-qemu.sh` (feature `hub-test` = autocompile + net-extra;
virtio-net over slirp; host shim `tools/hub-shim.py` plays gateway + lamp):

- boot 1: `semos update` → `semos install lights` (DAG resolve, on-device
  compile, byte-exact selftest, serial-approved install, intent
  registration) → `hub start` → gateway sends "lights on" / "lights off";
  each: intent match → UDP datagram → `LAMP: ON` / `LAMP: OFF` on the host
  → `OK` reply on the channel → `[DEMO 98] PASS` per command. Hard kill.
- boot 2 (mirror wiped, journal intact): `hub start` only —
  `[DEMO 98] boot 2: intent vocabulary restored from journal (no
  reinstall)` — and both commands dispatch again. VERDICT: PASS.

## What v1 deliberately does NOT build

- No on-device ASR/wake word (the gateway abstracts it; the mic path is
  DEMO 97 / isoch-IN, and the seam between them is the gateway protocol).
- No TLS on the gateway channel (the stack exists; v1 is LAN-plaintext).
- No intent *code* actions (arbitrary guest ELF per utterance) — the
  declarative action templates are the v1 safety story; vouched code
  execution per intent is a deliberate later step, same gate as /apps.
- No multi-device topology/registry (devices are (port, template) pairs).
