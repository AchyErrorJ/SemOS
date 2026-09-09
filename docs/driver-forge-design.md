# The driver forge (DEMO 100) — design

Status: implemented, QEMU-verified. 2026-09-09.

"How does one OS work on all devices? …what if the OS could write the
drivers in real time?" — this demo is that question, answered with the
machinery we already built. The self-dev loop (spec → compile → isolated
test → human gate → atomic install) is a forge with the word "tool"
replaced by "driver"; M22a's A/B slot machinery is what makes ring-0
artifacts safe to forge.

## The loop

agent reads the device spec (virtio spec §5.5, Entropy Device)
  → writes the driver (kernel-x86_64/src/virtio/rng.rs — this commit's
    driver is the agent-authored artifact)
  → candidate kernel B (tag RNG-B) built with the driver
  → delivered to the drop zone (host) — M22a's honest v1 delivery path
  → `rebuild stage` → hash-bound human vouch → trial boot
  → trial health gate EXTENDED: entropy proof (the driver works)
  → `rebuild keep` → PROMOTED. Kernel A never had the driver (baseline).

Every safety property is inherited, none re-proven: staging is
hash-verified, the vouch binds the sha256, a broken candidate auto-reverts
(DEMO 95), a torn one never boots (DEMO 96).

## Target device: virtio-rng

Deliberately small and *useful*: a legacy-transitional virtio PCI device
(0x1AF4/0x1005) with one request queue. Driver: probe, feature handshake,
queue setup, enqueue a device-writable buffer, notify, poll the used ring,
read entropy. QEMU's `virtio-rng-pci` produces REAL entropy (host
/dev/urandom), so the health gate can demand substance: 64 bytes read,
non-zero, non-constant — an agent-written driver provably doing hardware
work. And the payoff is real: SemOS's TLS/crypto gets an entropy source
authored by the agent.

## The health gate as the oracle

Driver synthesis's hard part was never codegen — it's oracles ("how do you
know the driver is right?"). The forge's answer, in order:
1. emulation: QEMU's device model (the driver is written against it),
2. substance checks: real entropy bytes, not "device probed OK",
3. the M22a health gate + human vouch before the kernel is kept.

## DEMO 100 beats (harness: tools/run-driverforge-qemu.sh)

- boot 1 (kernel A, no driver): baseline — the tree has no virtio-rng
  support; feeder stages the candidate and arms the trial (vouch 'y').
- boot 2 (kernel B, trial): health gate = journal + namespace + fenced
  spawn + **entropy proof** → HEALTHY → keep → PROMOTED →
  `[DEMO 100] PASS: agent-written virtio-rng driver live`.
- boot 3 (kernel B, promoted): entropy proof again after a plain reboot —
  the forged driver is just part of the kernel now.

## What v1 deliberately does NOT build

- On-device kernel compile (M22a's standing scope note — the candidate is
  built off-device and delivered as a blob; the forge loop is identical
  either way).
- Userspace drivers / IOMMU fencing (the A/B trial + health gate is the v1
  containment; a microkernel-style driver process is the v2 containment).
- Generic "any datasheet" ingestion (one spec, hand-fed, this time).
