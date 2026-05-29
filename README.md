# Semantic OS

A bare-metal x86_64 kernel written in Rust to test a hypothesis:
**LLM data-leak risk should be enforced at the hardware/kernel boundary,
not in user-space sandboxes.**

The kernel replaces the file abstraction with **semantic objects**
(SUID-addressed) carrying an explicit **security tier**
(`Public | Internal | Sensitive | Secret`). When a user task asks the
kernel for an LLM-bound view of an object, the kernel applies tier-based
redaction *before* returning bytes — even when the same task is
permitted to read the object directly. The policy lives in Ring 0; user
code can't bypass it.

## The headline demo

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

## What runs today (2026-05-28)

### Kernel core
- Preemptive scheduler (LAPIC timer, FPU/SSE save-restore, per-task page tables)
- Ring 0 / Ring 3 separation via SYSCALL/SYSRET; full Linux x86-64 syscall ABI
- ELF loader, per-process address spaces, threads + futexes
- **4-tier security model** with kernel-mediated LLM redaction
- Persistent FS over a snapshot ring (Namespace → BlockDevice → disk)

### Drivers
- **Storage**: VirtIO block + **NVMe** + **AHCI/SATA** — three backends behind one `BlockDevice` trait
- **Network**: VirtIO-net + smoltcp + TLS 1.3 + cert-pinning. **Live HTTPS round-trip to api.anthropic.com** from bare metal.
- **USB**: xHCI controller (incl. CSZ=1 / 64-byte contexts for Intel), HID boot keyboard, **live Mass Storage with bulk endpoints** (INQUIRY + READ CAPACITY validated against `-device usb-storage`)
- **Audio**: Intel HD Audio controller + codec walk + 48 kHz 16-bit stereo PCM playback
- **Framebuffer**: M6 drawing API, M7 TTF rasterization (ttf-parser), M8 2D vector (tiny-skia)
- **Console**: TTY layer (cooked/raw mode, line editing, scrollback PageUp/PageDown), 2× scaled console font

### Apps & shell
- **`sem-sh`** native shell: pipes, redirection, `&&`/`||`, `$VAR`, `$PATH` (`/bin:/apps`), builtins: `echo cd ls cat which env grep ps free uptime ask fetch help agent edit`
- **`agent`** builtin → split-pane Claude TUI (conversation | activity panes) over the framebuffer, real keyboard input
- **`edit`** builtin → **modal vi-style text editor** with Rust syntax highlighting, `:w :q :wq`, `/search`, `h j k l + arrows`, `i a A o O x dd gg G`
- **Interactive mode** (`--features interactive`): land in the live sem-sh after demos with the real keyboard, instead of idling
- **Persistence**: `SYS_FSYNC` saves the namespace to disk; survived-reboot validation

### Protocol layers ready for hardware
- **802.11 frame builders** (Probe Request, Open Auth, Association, EAPOL-Key Msg2) + iwlwifi PCI device-ID table (7260 + AX211)
- **CDC-ECM** descriptor parser + MAC string decode (USB Ethernet fallback path)
- **USB Mass Storage** CBW/CSW + SCSI Block Commands (live on xHCI as of DEMO 69)
- **HID report descriptor parser** for gamepad (axes + buttons, signed Logical Min/Max)

### Self-hosting prep
- `semos-std`: `#[global_allocator]`, `io::{Read,Write}`, `fs::File`, `env`, `sync::{Mutex,Once}`, `thread::spawn + JoinHandle<T>`, `process::Command`, `net::TcpStream`, **`time::{Instant,Duration}`**, **`path::{Path,PathBuf}`**
- See [`docs/SELF_HOSTING_PLAN.md`](docs/SELF_HOSTING_PLAN.md) for the M25/26/27 roadmap toward rustc-on-metal

## Hardware target

Two-machine bring-up:
- **Stage 1: ThinkPad T540** (i7-4600M Haswell, 8 GB, 256 GB **SATA** SSD,
  Win10) — cheap, coreboot-friendly, removable Wi-Fi card. Validates the
  bootloader + kernel + iwlwifi (7260) + AHCI on real metal.
- **Stage 2: ThinkPad P1 Gen 6** — i7 Raptor Lake hybrid, Intel Iris Xe iGPU,
  NVIDIA RTX dGPU. Where GPU work (M14 iGPU / M18 dGPU compute) begins.

Debug-without-serial on metal works via three levels: framebuffer + scrollback,
**panic-dump to disk** (recover from Windows via `tools/read-panic-log.ps1` —
no third-party tool needed), and eventually network log streaming over Wi-Fi.

## Repo layout

```
kernel-core/        # platform-independent crate
                    #   semantic objects, redactor, ChaCha20 crypto, ramfs,
                    #   path namespace, scheduler, process table, syscall
                    #   dispatch, TCP/IP (smoltcp), TLS 1.3, snapshot FS
kernel-x86_64/      # x86_64 platform crate
                    #   GDT/TSS, IDT, paging, APIC, SYSCALL/SYSRET, FPU,
                    #   framebuffer + TTF + tiny-skia, context switch, PCI,
                    #   virtio block + net, NVMe, AHCI, HDA audio, xHCI,
                    #   agent, editor, panic_dump
x86_64-runner/      # Windows host tool — wraps the kernel ELF in a
                    # bootloader-0.11 disk image (UEFI + BIOS) for QEMU
user-programs/      # Real Rust no_std user binaries embedded in the
                    # kernel via include_bytes!. Each builds as its own
                    # crate with a non-PIE linker script + custom entry.
                    #   hello/, hello-std/, sem-demo/, sem-sh/, std-shim/
                    #   net-demo/, std-demo/, thread-demo/, vec-demo/
                    #   spawn-demo/, exfil-demo/
tools/              # read-panic-log.ps1 — PowerShell recovery for the
                    # disk-resident kernel panic dump (no third-party tool)
docs/               # ROADMAP.md (milestones), SELF_HOSTING_PLAN.md,
                    # PHASE_*.md briefs, architecture notes
```

See [`docs/ROADMAP.md`](docs/ROADMAP.md) for the milestone log (what's
done, in progress, what's hardware-gated).

## Build and run

Toolchain: Rust nightly pinned to `nightly-2026-02-01` (the version the
bootloader-0.11 crate requires). A single boot runs ~69 demos and prints
`PASS:` / `FAIL:` lines to the serial log, ending with `All demos complete`.

```sh
# 1. Build every user program — they're embedded into the kernel via
#    include_bytes!, so the kernel build below won't pick up changes
#    until these are (re)built first.
for p in hello hello-std sem-demo sem-sh net-demo std-demo \
         thread-demo vec-demo spawn-demo exfil-demo; do
  ( cd user-programs/$p && cargo build --release )
done

# 2. (optional) bake an Anthropic API key for the LIVE agent demos
#    (48 = 401 round-trip, 49 = agent tool loop, 54 = `ask`). Omit it
#    and those self-skip / return "no key" — the rest of the suite is
#    unaffected. The key only lands in the gitignored target/ binary.
# export ANTHROPIC_KEY=sk-ant-...

# 3. Build the kernel.
#    Add `--features interactive` to end boot by handing the keyboard
#    to a live sem-sh shell instead of idling.
( cd kernel-x86_64 && cargo build --release )

# 4. Wrap the kernel ELF into a bootable BIOS+UEFI image. MUST be from
#    x86_64-runner/ (it's a host tool); running it from kernel-x86_64/
#    leaves a STALE image.
( cd x86_64-runner && cargo run --release )

# 5. (one-time) disk images for the storage demos.
qemu-img create -f raw vdisk.img       16M   # virtio block (persistence)
qemu-img create -f raw nvme.img        32M   # NVMe (DEMO 62)
qemu-img create -f raw sata.img        64M   # AHCI/SATA (DEMO 67)
qemu-img create -f raw ustick.img     128M   # USB Mass Storage (DEMO 69)

# 6. Boot. The full flag set runs ALL demos including networked + live USB.
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

## Status

This is an **active-development kernel**, validated end-to-end in QEMU
and being prepared for first metal boot on a T540.

The interesting parts:

- **The policy model** (kernel-mediated LLM data flow, tier-based
  redaction at the syscall boundary) is real and demonstrable from
  Ring 3 (DEMO 4) and from the live agent shell (DEMO 56).
- **Live HTTPS to Anthropic** from bare metal (DEMO 48/49) — full TLS
  1.3, SPKI cert pinning, the works.
- **Three storage backends** (VirtIO + NVMe + AHCI) all live behind one
  trait. The T540 will exercise the AHCI path on real silicon; the P1
  adds NVMe.
- **Real apps on metal**: a modal editor with syntax highlighting, an
  agent TUI, persistence across reboot. Not a research toy.
- The "LLM" services do **rule-based** redaction today. A real on-device
  inference path (dGPU compute, post-P1) is the obvious next milestone
  for the security thesis.

## License

MIT or Apache-2.0, your choice.
