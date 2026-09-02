# Semantic OS

A bare-metal x86_64 (and aarch64) kernel written in Rust toward one idea:
**an agent-native, self-extending, sovereign OS** — an LLM agent writes its own
modules, compiles them *on the machine*, and loads them into the running system,
with **security tiers as the capability fence** on agent-written code.

The original hypothesis still anchors the security model: **LLM data-leak risk
should be enforced at the hardware/kernel boundary, not in user-space sandboxes.**
The kernel replaces the file abstraction with **semantic objects**
(SUID-addressed) carrying an explicit **security tier**
(`Public | Internal | Sensitive | Secret`). When a user task asks the
kernel for an LLM-bound view of an object, the kernel applies tier-based
redaction *before* returning bytes — even when the same task is
permitted to read the object directly. The policy lives in Ring 0; user
code can't bypass it. The same tier check (`current_task_max_tier()`) gates
LLM/semantic/process/namespace syscalls pervasively, and a child can never
exceed its spawner's clearance — so agent-authored tools spawned at **tier 0**
are sandboxed by construction until a human **vouches** them (`SYS_VOUCH`).

Lineage: **Oberon** (self-extension via dynamically loaded modules) × **Genode**
(capability-scoped component isolation), with an LLM in the author's seat.

> **One sentence:** the first OS where an AI writes the system, runs on hardware
> you fully own, and a human holds the only key that grants it power.
>
> **Deliberately NOT:** a Linux replacement (no POSIX, no other people's
> binaries), a daily driver, or "provably secure" (the claim is smallness +
> auditability + human-gated trust).

See [`docs/MASTER_ROADMAP.md`](docs/MASTER_ROADMAP.md) for the full thesis and
the active/gated/next view. Any agent writing code should first read
[`docs/semos-security-thesis.md`](docs/semos-security-thesis.md) and
[`docs/provenance-commitment.md`](docs/provenance-commitment.md).

---

## Status: research / education project — read this first

SemOS is a working research system, public so its claims can be checked. It is
**not a production OS and not yet defending anything that matters**. Known and
declared weaknesses:

- **No IOMMU on the dev machines** (T540p/W540 have no DMAR table → no VT-d).
  Every bus-master device (xHCI, EHCI, AHCI, NVMe, NIC, iGPU) DMAs
  unconstrained. The tier model does not defend against DMA attacks on this
  hardware.
- **One declared-trust exception:** the Intel `iwlwifi` firmware blob
  (included at the repo root under Intel's own licence — see License).
- **Security guarantees hold modulo kernel correctness and covert channels.**
  Full per-syscall audit is open work (`docs/KERNEL_SURFACE.md` §3). Timing
  side-channels are not mitigated.
- **User binaries must build at `opt-level=0`** — a codegen sensitivity at
  higher optimization levels is an open bug.
- No isochronous USB transfers yet (USB audio capture is planned work).
  USB completion is polled, not interrupt-driven, by design.
- No dynamic linking, no JIT, read-only FAT32 base (persistence via a
  snapshot ring), panic = abort. Design choices — but choices with costs.

If you find more: that's the point of this being public. Please file an issue.

## The headline demos

In a single boot, the kernel runs **69 self-tests** end-to-end —
~165 PASS lines, 0 FAIL, 0 #DF. The two load-bearing security demos:

```
================================================================
  SemOS DEMO 2: SemanticObject + LLM context (Sensitive tier)
================================================================
  DIRECT READ:  Sensitive: email=user@example.com card=4111-1111-1111-1111
  LLM CONTEXT:  Sensitive: email=[EMAIL] card=[CARD]

================================================================
  SemOS DEMO 4: Ring 3 sem-demo (Sensitive obj, direct vs LLM)
================================================================
  DIRECT READ:  Sensitive: email=alice@example.com card=4111-1111-1111-1111
  LLM CONTEXT:  Sensitive: email=[EMAIL] card=[CARD]
```

DEMO 2 runs in the kernel; DEMO 4 runs the **same security policy** end-
to-end from a Ring 3 user binary through `SYS_SEM_CREATE` →
`SYS_SEM_READ` → `SYS_LLM_CONTEXT`. Same caller, same byte buffers, two
views — chosen by the kernel based on intended downstream use, not
caller capability.

DEMO 56 is the live agent version: a sandboxed shell at security tier 0
provably **cannot read Secret files** and **cannot modify Public ones**
even when the LLM driving it tries.

## The self-dev loop (M1–M4, QEMU-verified 2026-08)

The thesis, closed end-to-end — the agent writes code, the machine compiles
it, a human approves it, the system gains a feature:

- **M1 — hello loop:** agent writes a program → on-device compile → human
  approval gate → installed to `/apps` → runnable. Closed end-to-end.
- **M2 — bug fix:** the agent *fixes a bug* in an existing tool; human
  approves the `/apps` install.
- **M3 — feature add:** the agent ships a new `wc`; human approves.
- **M4 — self-repair (DEMO 88):** detects a crash, patches the source,
  verifies the fix, repairs the installed tool — again behind the human
  approval gate (which accepts the PS/2 keyboard on bare metal as well as
  serial, so the human-in-the-loop works on real hardware, not just QEMU).

The approval gate is the point: `SYS_VOUCH`/`SYS_VOUCH_SESSION` are reachable
**only from the interactive console** — the agent cannot elevate its own code.

## What runs today (2026-09-01)

> **On metal:** SemOS boots on real hardware (ThinkPad T540p/W540) — not
> just QEMU. On-device `rustc` compiled and ran a program on bare metal (DEMO 80);
> the iwlwifi WiFi join and USB enumeration were brought up against real silicon.
> Every boot opens with a **build stamp** (git hash · UTC build time ·
> toolchain) on the first serial line, so the exact image is identifiable even
> if a later init stage hangs.

### Self-extension keystone
- **Any ramfs/namespace ELF runs by name** — `spawn_namespace_elf` + `$PATH`
  resolve arbitrary tools; the hardcoded `/bin` spawn table is gone.
- **Tier-0 fence**: agent-authored modules spawn powerless; the security tier
  is the capability boundary, enforced on every gated syscall.
- **Vouch mechanisms**: `SYS_VOUCH` (126) / `SYS_VOUCHES` (127) bind one tool's
  bytes to a tier; `SYS_VOUCH_SESSION` (133) opens a time-boxed,
  password-gated elevation session. Interactive-console authority only.
- **On-device rustc** (M27 / DEMO 80): full parse → typeck/borrowck → Cranelift
  codegen → ELF, reading `*.rlib` from a disk-staged sysroot blob, run on metal.

### Kernel core
- Preemptive scheduler (LAPIC timer, FPU/SSE save-restore, per-task page tables)
- Ring 0 / Ring 3 separation via SYSCALL/SYSRET
- ELF loader, per-process address spaces, threads + futexes
- **4-tier security model** with kernel-mediated LLM redaction
- Persistent FS over a snapshot ring (Namespace → BlockDevice → disk)

### Drivers
- **Storage**: VirtIO block + **NVMe** + **AHCI/SATA** + **USB Mass Storage** — behind one `BlockDevice` trait. Plus a **read-only FAT32 reader** and a raw sector-aligned **sysroot blob** loader (the on-disk store for the compiler's `*.rlib`s).
- **Network**: VirtIO-net + smoltcp + TLS 1.3 + cert-pinning. **Live HTTPS round-trip to api.anthropic.com** from bare metal. **SemNet**: a WireGuard data plane (blake2s + Noise_IK + transport) so SemOS can join an existing tailnet as a node.
- **WiFi**: **iwlwifi (Intel 7260)** firmware bring-up → calibration → MAC → **live scan with real SSIDs** + interactive `wifi` / `wifi connect` shell commands; WPA2 PMK/PTK/EAPOL-MIC crypto built and KAT-passing. The in-progress frontier is the first on-air data frame (hardware-gated, not QEMU-testable).
- **USB** (~8,400 LoC, all Rust, in-kernel): xHCI controller (incl. CSZ=1 / 64-byte contexts for Intel), **multi-slot enumeration + single-tier cascaded hubs**, HID boot keyboard, **live Mass Storage with bulk endpoints**. Standalone EHCI path enumerates an **iPhone tether** (ipheth) + CDC-ECM/NCM.
- **Audio**: Intel HD Audio controller + codec walk + 48 kHz 16-bit stereo PCM playback (output only — no capture path yet).
- **Display**: firmware-framebuffer drawing API, TTF rasterization, 2D vector (tiny-skia), fast blit for tear-free presents. **Haswell iGPU native modeset is the active frontier**: Rung B (native-60 timing) **proven on metal**; Rung C (`SYS_FB_FLIP` page flip) staged.
- **Console**: TTY layer (cooked/raw mode, line editing, scrollback), 2× scaled console font.

### Apps & shell
- **`sem-sh`** native shell: pipes, redirection, `&&`/`||`, `$VAR`, `$PATH` (`/bin:/apps`), builtins: `echo cd ls cat which env grep ps free uptime ask fetch help agent edit`
- **`agent`** builtin → split-pane Claude TUI (3-pane layout with wrap-on-redraw scrollback) over the framebuffer, real keyboard input
- **`edit`** builtin → **modal vi-style text editor** with Rust syntax highlighting
- **Userland game kit**: `SYS_KB_POLL` + `SYS_FB_CLAIM` give Ring-3 programs direct input + framebuffer — shipped with a playable `snake`
- **Persistence**: `SYS_FSYNC` saves the namespace to disk; survived-reboot validation

### On-device rustc (M27 — self-hosting)
- The full upstream `rustc` (incl. the **Cranelift codegen backend**, ported to
  `no_std`) builds for the SemOS target and runs in Ring 3. On bare metal it has
  taken a `hello.rs` through the entire pipeline to a working ELF, reading the
  sysroot `*.rlib`s from a disk-staged blob via `SYS_SYSROOT_READ`.
- Precisely scoped: SemOS compiles its own **programs** on-device; the kernel
  itself is still host-built. Closing that last gap is the Phase-22 self-rebuild
  capstone on the roadmap.
- `semos-std`: `#[global_allocator]`, `io::{Read,Write,Seek}`, `fs::{File,rename}`, `env`, `sync::{Mutex,Once}`, `thread::spawn + JoinHandle<T>`, `process::Command`, `net::TcpStream`, `time::{Instant,Duration}`, `path::{Path,PathBuf}`
- See [`docs/SELF_HOSTING_PLAN.md`](docs/SELF_HOSTING_PLAN.md) for the rustc-on-metal roadmap.

### Beyond the kernel
- **`sheaf/`** — Phase-0 userland prototype of the **Sheaf bundle filesystem**: content-addressed bundles, hand-rolled TOML/SHA-256/tar, `.agent` profiles with requested-grants dry-run. The package model (`docs/THREAT_MODEL_AND_PACKAGE_MODEL.md`) becoming code.
- **`companion-ios/`** — native Swift/SwiftUI companion app implementing the phone side of the pairing protocol (`docs/pairing-v1.md`), foundation for phone-as-peripheral (Phase 18).

### ARM port
- A standalone **aarch64** kernel (`kernel-aarch64/`) boots → UART → vectors → MMU
  → GICv2 → timer → preemptive scheduler and runs the **same `kernel-core`**
  (sha256 KAT passes). "Two backends, one portable core." QEMU-testable:
  `cd kernel-aarch64 && cargo run --release`.

## Hardware target

Two-machine bring-up:
- **Stage 1: ThinkPad T540p / W540** (Haswell, discrete Quadro on the W540) —
  cheap, coreboot-friendly, removable Wi-Fi card. **Boots on real metal today**;
  validated the kernel + iwlwifi (7260) join + USB enumeration + AHCI + on-device
  rustc + native iGPU modeset timing.
- **Stage 2: ThinkPad P1 Gen 6** — i7 Raptor Lake hybrid, Intel Iris Xe iGPU,
  NVIDIA RTX dGPU. Where GPU compute (M18 dGPU / local inference) begins.

### Dev/boot workflow (no more disk flashing)

SemOS is self-contained: the whole OS (bootloader + kernel + every user program
via `include_bytes!`) is one image; the filesystem is in-memory ramfs, so there
is no install state to preserve. The intended loop is **UEFI dual-boot**:

1. Install Linux on the SSD with an EFI System Partition (ESP).
2. Copy SemOS's UEFI payload into a folder on the ESP (e.g. `EFI/semos/BOOTX64.EFI`)
   and register a boot-menu entry — the firmware menu then lists both.
3. Each rebuild = overwrite that one `.efi` from Linux. Seconds, not a USB reflash.

> **Caution:** the compiler **sysroot blob is written raw to LBA 0 of a whole
> SATA disk** (no partition table) by `flash-sysroot`. Keep it on a **separate
> disk** from your OS partitions, or it will clobber the GPT. (Partition-offset
> support to let it live safely inside a partition is tracked in the roadmap.)

Debug-without-serial on metal works via: framebuffer + scrollback,
**panic-dump to disk** (recover via `tools/read-panic-log.ps1`), and kernel-log
streaming over UDP (`SYS_NETLOG` 132 → LAN listener).

## Repo layout

```
kernel-core/        # platform-independent crate
                    #   semantic objects, redactor, crypto, ramfs,
                    #   path namespace, scheduler, process table, syscall
                    #   dispatch (75 syscalls, numbered 0–141 with gaps),
                    #   TCP/IP (smoltcp), TLS 1.3, WireGuard, snapshot FS
kernel-x86_64/      # x86_64 platform crate
                    #   GDT/TSS, IDT, paging, APIC, SYSCALL/SYSRET, FPU,
                    #   framebuffer + TTF + tiny-skia, iGPU modeset, PCI,
                    #   virtio block + net, NVMe, AHCI, HDA audio, xHCI/EHCI,
                    #   iwlwifi, agent, editor, panic_dump, netlog
kernel-aarch64/     # standalone aarch64 platform crate running the same
                    # kernel-core (UART, GICv2, MMU, timer, scheduler)
x86_64-runner/      # host tool — wraps the kernel ELF in a
                    # bootloader-0.11 disk image (UEFI + BIOS) for QEMU
                    # (WSL2/Linux build scripts in tools/)
user-programs/      # Real Rust no_std user binaries embedded in the
                    # kernel via include_bytes! (19 ELFs today, incl.
                    # sem-sh, semos-cc, and the 90 MB semos-rustc).
                    # Each builds as its own crate with a non-PIE
                    # linker script + custom entry.
sheaf/              # Phase-0 prototype of the Sheaf bundle filesystem
                    # (content-addressed bundles, .agent grant profiles)
companion-ios/      # Swift/SwiftUI companion app (pairing protocol,
                    # phone-as-peripheral foundation)
compiler/           # host-side Cranelift IR→ELF emitter (host twin of
                    # user-programs/semos-cc)
tools/              # build scripts (incl. WSL2), QEMU demo runners,
                    # sysroot blob packer, panic-log recovery
docs/               # MASTER_ROADMAP.md (index + thesis), ROADMAP.md
                    # (milestone log), security thesis, provenance,
                    # threat model, vendoring briefs, hardware notes
```

See [`docs/ROADMAP.md`](docs/ROADMAP.md) for the milestone log (what's
done, in progress, what's hardware-gated).

## Build and run

Toolchain: Rust nightly pinned to `nightly-2026-02-01` (the version the
bootloader-0.11 crate requires; `rust-toolchain.toml` pins it). A single boot
runs ~69 demos and prints `PASS:` / `FAIL:` lines to the serial log, ending
with `All demos complete`.

```sh
# 1. Build every user program — they're embedded into the kernel via
#    include_bytes!, so the kernel build below won't pick up changes
#    until these are (re)built first. (Or: tools/build-user-programs.sh)
for p in hello hello-std sem-demo sem-sh net-demo std-demo \
         thread-demo vec-demo spawn-demo exfil-demo sync-demo; do
  ( cd user-programs/$p && cargo build --release )
done

# 2. Generate the tiny Cranelift-built SemOS ELF used by DEMO 72.
( cd compiler && cargo run --release )

# 3. (optional) bake an Anthropic API key for the LIVE agent demos
#    (48 = 401 round-trip, 49 = agent tool loop, 54 = `ask`). Omit it
#    and those self-skip / return "no key" — the rest of the suite is
#    unaffected. The key only lands in the gitignored target/ binary.
#    (Both .anthropic-key and .kimi-key are gitignored.)
# export ANTHROPIC_KEY=sk-ant-...

# 4. Build the kernel.
#    Add `--features interactive` to end boot by handing the keyboard
#    to a live sem-sh shell instead of idling.
( cd kernel-x86_64 && cargo build --release )

# 5. Wrap the kernel ELF into a bootable BIOS+UEFI image. MUST be from
#    x86_64-runner/ (it's a host tool); running it from kernel-x86_64/
#    leaves a STALE image.
( cd x86_64-runner && cargo run --release )

# 6. (one-time) disk images for the storage demos.
qemu-img create -f raw vdisk.img       16M   # virtio block (persistence)
qemu-img create -f raw nvme.img        32M   # NVMe (DEMO 62)
qemu-img create -f raw sata.img        64M   # AHCI/SATA (DEMO 67)
qemu-img create -f raw ustick.img     128M   # USB Mass Storage (DEMO 69)

# 7. Boot. The full flag set runs ALL demos including networked + live USB.
qemu-system-x86_64 -cpu max \
  -drive format=raw,file=kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64-bios.img \
  -drive if=virtio,format=raw,file=vdisk.img,cache=writethrough \
  -drive id=nvm,if=none,format=raw,file=nvme.img \
  -device nvme,drive=nvm,serial=semos01 \
  -drive id=satadrv,if=none,format=raw,file=sata.img \
  -device ich9-ahci,id=ahci -device ide-hd,drive=satadrv,bus=ahci.0 \
  -device intel-hda -device hda-output \
  -device qemu-xhci -device usb-kbd \
  -netdev user,id=net0 -device virtio-net-pci,netdev=net0 \
  -m 256M -serial file:serial.log -display none -no-reboot
```

Flag notes:
- **`-cpu max`** is required — the crypto stack uses `RDRAND`.
- **`-netdev user ... -device virtio-net-pci`** (SLIRP) enables the network
  demos: DNS (34), TLS round-trip to api.anthropic.com (16/48/49),
  `std::net` (36), and the shell `fetch` (55). Without it, those self-skip.
- **`-device qemu-xhci -device usb-kbd`** gives the HID boot keyboard for
  TTY/shell demos. Swap `usb-kbd` for `-drive id=ustick,if=none,...` +
  `-device usb-storage,drive=ustick` to exercise the **live USB Mass
  Storage** path (DEMO 69) — but only one USB device at a time today
  (multi-device enumeration is a follow-up).
- **`-device nvme`**, **`-device ich9-ahci + ide-hd`**, **`-device intel-hda
  + hda-output`** light up DEMOs 62 (NVMe), 67 (AHCI), 63 (HDA) — each
  cleanly self-skips if the corresponding device isn't attached.
- It runs headless (`-display none`); the serial log is the source of
  truth. Drop `-display none` for a visible QEMU window with the
  framebuffer console.

Check the result:

```sh
grep -c 'PASS:'              serial.log   # ~165 with everything attached
grep    'FAIL:'              serial.log   # expect no output
grep   'All demos complete'  serial.log
```

For GDB on the kernel: also pass `-gdb tcp::1240 -S`, then in another
shell `gdb -ex "set osabi none" -ex "set architecture i386:x86-64"
-ex "file kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64"
-ex "target remote :1240"`.

## Provenance

SemOS is built by one developer directing AI assistance — stated plainly
because it is also the thesis: this codebase is what the agent-era development
loop produces when a human holds the design and the keys. The coherence of the
whole (one mind, one style, readable end-to-end) is the claim, and it is
checkable: read the tree. Authorship and trust tracking are documented in
[`docs/provenance-commitment.md`](docs/provenance-commitment.md).

## Contributing

Welcome, with one contract: **the roadmap's open frontiers are the
contribution list.** Current priorities:

1. Isochronous USB transfers (xHCI) — the gate for USB audio capture
2. Interrupt-driven xHCI completion (MSI-X) — currently polled by design
3. The `opt-level=0` codegen bug in user binaries
4. Per-syscall audit coverage (`docs/KERNEL_SURFACE.md` §3)
5. Haswell iGPU: Rung C page-flip on metal (GGTT route), then rendering
6. First on-air WiFi data frame (hardware-gated; needs a T540p/W540)

Design-first PRs for the above — every milestone answers four surface
questions before work starts: **new syscall? smallest shape? capability check?
blast radius?** (See `docs/MASTER_ROADMAP.md`.) Issues for everything else.
Response times are what they are — this is a research project, not a service.

## Status

This is an **active-development kernel**, validated end-to-end in QEMU
**and booting on real metal** (ThinkPad T540p/W540) — where it has brought up
WiFi, USB enumeration, native iGPU modeset timing, and on-device `rustc`.

The interesting parts:

- **The policy model** (kernel-mediated LLM data flow, tier-based
  redaction at the syscall boundary) is real and demonstrable from
  Ring 3 (DEMO 4) and from the live agent shell (DEMO 56).
- **The self-dev loop is closed** (M1–M4): agent writes → machine compiles →
  human approves → system gains a feature. With a repair loop (DEMO 88).
- **Live HTTPS to Anthropic** from bare metal (DEMO 48/49) — full TLS
  1.3, SPKI cert pinning, the works. Plus a WireGuard data plane.
- **Three storage backends** (VirtIO + NVMe + AHCI) all live behind one
  trait, plus USB Mass Storage.
- **Real apps on metal**: a modal editor with syntax highlighting, an
  agent TUI, a playable snake, persistence across reboot.
- The "LLM" services do **rule-based** redaction today. A real on-device
  inference path (dGPU compute, post-P1) is the obvious next milestone
  for the security thesis.

## License

SemOS is dual-licensed under **MIT or Apache-2.0**, at your option
(`LICENSE-MIT` / `LICENSE-APACHE`).

**Exception:** `iwlwifi-7260-17.ucode` at the repo root is Intel's WiFi
firmware blob, redistributed under Intel's own terms
([`LICENCE.iwlwifi_firmware`](LICENCE.iwlwifi_firmware)) — it is not covered
by the MIT/Apache grant and is the system's one declared-trust binary
exception.
