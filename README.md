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

In a single boot, the kernel runs five demos. Two are particularly
load-bearing:

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
to-end from a Ring 3 user binary (`user-programs/sem-demo/`, real Rust
compiled to ELF) through `SYS_SEM_CREATE` → `SYS_SEM_READ` →
`SYS_LLM_CONTEXT`. Same caller, same byte buffers, two views — chosen
by the kernel based on intended downstream use, not caller capability.

A captured serial log of a full boot is in [`docs/boot-demo.log`](docs/boot-demo.log).

## What runs today

| Demo | Status | Path | What it proves |
|------|--------|------|----------------|
| DEMO 0 | active | `user-programs/hello/` | Real Rust no_std ELF crate loaded by the kernel from ramfs and run in Ring 3 (`SYS_WRITE` + `SYS_EXIT`). Toolchain works end-to-end. |
| DEMO 1 | active | `kernel-core/src/process/elf.rs::create_redact_elf` (hand-assembled) | Ring 3 binary calls `SYS_LLM_REDACT` on a string with PII. Kernel returns the redacted version. |
| DEMO 2 | active | kernel-side, `kernel-x86_64/src/main.rs::sem_demo_kernel` | Kernel-side `SemanticObject` at Sensitive tier: direct read returns verbatim, `build_from_suids` (the LLM context path) returns redacted. |
| DEMO 3 | active | same | Same with a Public-tier object — both views verbatim, no redaction. The contrast vs DEMO 2 is the policy made visible. |
| DEMO 4 | active | `user-programs/sem-demo/` | DEMO 2's policy, **from Ring 3**. Real Rust user crate creates a Sensitive object, reads it back verbatim (caller tier 2 ≥ object tier 2), then asks for `SYS_LLM_CONTEXT` and receives the kernel-redacted version. |
| DEMO 5 | code only, disabled in boot | `kernel-x86_64/src/main.rs::persistence_demo` | Round-trip a Sensitive `SemanticObject` through the VirtIO block driver to disk and back. The infrastructure is verified independently (see "Known issues"); the integration demo is gated on task #40. |
| DEMO 6 | code only, disabled in boot | `user-programs/exfil-demo/` | Adversarial: 8 PII-exfiltration attempts via the LLM channel (plain text baseline + 7 obfuscations: base64, [at]/[dot] brackets, whitespace splitting, reversal, hex, non-standard CC separators, split-across-objects). Each attempt creates a Sensitive object, asks the kernel for an LLM-bound view, and substring-checks the result for an attacker-chosen leak indicator. The expected outcome — 1 caught, 7 leaked — is the thesis-grade evidence for *why* rule-based redaction is a baseline and a real on-device intent-aware model is the next step. The crate compiles and the attacks are correct; the kernel currently can't run it reliably under task #40. |

Plus the platform-level pieces those exercise:

- **Preemptive scheduling** with Local APIC timer, FPU/SSE save/restore,
  per-task page tables.
- **Ring 0 / Ring 3 separation** via `SYSCALL`/`SYSRET`. The
  `syscall_entry` naked function preserves the full Linux x86-64 syscall
  ABI (rdi/rsi/rdx/r10/r8/r9 saved across `dispatch`) — see commit log
  for why this matters.
- **ELF loader** (`kernel-core/src/process/elf.rs`). Handles `ET_EXEC` +
  `PT_LOAD` segments with R/W/X permission bits.
- **Per-process address spaces** (`kernel-x86_64/src/paging.rs`). PML4
  is a shallow copy of the boot page tables (kernel mappings shared) +
  fresh user-space subtables. Reaped on slot reuse via
  `Platform::reap_slot`.
- **PCI bus enumeration** (`kernel-x86_64/src/pci.rs`) — scans bus 0 via
  `0xCF8`/`0xCFC`, locates VirtIO at `0x1AF4:0x1001`.
- **VirtIO Legacy block driver** (`kernel-x86_64/src/virtio/block.rs`) —
  init handshake, virtqueue setup (size 256, page-aligned 12 KB BSS),
  3-descriptor-chain read/write, poll completion. Implements
  `kernel_core::drivers::traits::BlockDevice` and registers as
  `"virtio0"`.
- **Snapshot persistence** (`kernel-core/src/storage/snapshot.rs`) — a
  thin `save_snapshot` / `load_snapshot` API on top of any
  `BlockDevice`. Verified end-to-end at the raw-sector level.
- **In-memory ramfs** (`kernel-core/src/fs/ramfs.rs`) with a real FD
  table and POSIX-shaped `open/read/write/close` syscalls.
- **Local LLM substrate**: rule-based redaction (`kernel-core/src/llm/redact.rs`),
  summarization, context builder. Stubbed for the demo; real on-device
  inference is the obvious next big step.

## Repo layout

```
kernel-core/        # platform-independent crate (~11 K LOC)
                    #   semantic objects, vector index, LLM context builder,
                    #   redactor, ChaCha20 crypto, ramfs, scheduler,
                    #   process table, syscall dispatch
kernel-x86_64/      # x86_64 platform crate (~3.3 K LOC)
                    #   GDT/TSS, IDT, paging, APIC, SYSCALL/SYSRET,
                    #   framebuffer, context switch, FPU save/restore,
                    #   PCI, VirtIO block, platform_impl
x86_64-runner/      # Windows host tool — wraps the kernel ELF in a
                    # bootloader-0.11 disk image (UEFI + BIOS) for QEMU
user-programs/      # Real Rust no_std user binaries, compiled to ELF
                    # and embedded in the ramfs at kernel build time.
                    # Each is its own crate with a custom linker script
                    # putting text at USER_CODE_BASE = 0x400000.
                    #   hello/    — DEMO 0: SYS_WRITE + SYS_EXIT
                    #   sem-demo/ — DEMO 4: SYS_SEM_CREATE/READ + SYS_LLM_CONTEXT
docs/               # README artifacts (boot logs, architecture notes)
```

See [`docs/architecture.md`](docs/architecture.md) for the module-level
map and the syscall table.

## Build and run all the demos

Toolchain: Rust nightly pinned to `nightly-2026-02-01` (the version the
bootloader-0.11 crate requires). A single boot runs **DEMOs 1–57** and prints
`PASS:` / `FAIL:` lines to the serial log, ending with `All demos complete`.

```sh
# 1. Build every user program — they're embedded into the kernel via
#    include_bytes!, so the kernel build below won't pick up changes until
#    these are (re)built first.
for p in hello hello-std sem-demo sem-sh net-demo std-demo \
         thread-demo vec-demo spawn-demo exfil-demo; do
  ( cd user-programs/$p && cargo build --release )
done

# 2. (optional) bake an Anthropic API key for the LIVE agent demos
#    (48 = 401 round-trip, 49 = agent tool loop, 54 = `ask`). Omit it and
#    those self-skip / return "no key" — the rest of the suite is unaffected.
#    The key only ever lands in the gitignored target/ binary, never in git.
# export ANTHROPIC_KEY=sk-ant-...

# 3. Build the kernel.
( cd kernel-x86_64 && cargo build --release )

# 4. Wrap the kernel ELF into a bootable BIOS+UEFI image.
#    NOTE: run this from x86_64-runner/ (it's a host tool); running it from
#    kernel-x86_64/ leaves a STALE image.
( cd x86_64-runner && cargo run --release )

# 5. (one-time) a virtio disk for the persistence/FS demos.
qemu-img create -f raw vdisk.img 16M

# 6. Boot. The full flag set runs ALL demos including the networked ones.
qemu-system-x86_64 -cpu max \
  -drive format=raw,file=kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64-bios.img \
  -drive if=virtio,format=raw,file=vdisk.img \
  -device qemu-xhci -device usb-kbd \
  -netdev user,id=net0 -device virtio-net-pci,netdev=net0 \
  -m 256M -serial file:serial.log -display none -no-reboot
```

Flag notes:
- **`-cpu max`** is required — the crypto stack uses `RDRAND`.
- **`-netdev user ... -device virtio-net-pci`** (SLIRP) enables the network
  demos: DNS (34), TLS round-trip to api.anthropic.com (16/48/49), `std::net`
  (36), and the shell `fetch` (55). Without it, those self-skip.
- **`-device qemu-xhci -device usb-kbd`** gives a USB keyboard for the TTY /
  shell-input demos (40/43/51).
- It runs headless (`-display none`); the serial log is the source of truth.

Check the result:

```sh
grep -c 'PASS:' serial.log        # ~154 with the network up
grep    'FAIL:' serial.log        # expect no output
grep 'All demos complete' serial.log
```

For GDB on the kernel: also pass `-gdb tcp::1240 -S`, then in another
shell `gdb -ex "set osabi none" -ex "set architecture i386:x86-64"
-ex "file kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64"
-ex "target remote :1240"`.

> The sections below predate the network/TLS stack, the native Claude agent,
> the `sem-sh` shell, and most of the current 57-demo suite — see
> [`docs/ROADMAP.md`](docs/ROADMAP.md) for the up-to-date milestone log
> (task #40's intermittent `#PF`, noted under "Known issues", was root-caused
> and fixed; the suite now boots `0 FAIL / 0 #DF`).

## Known issues

### Task #40 — intermittent kernel `#PF` at RIP=0

After certain demo combinations, a kernel-mode page fault fires with
`RIP=0`. The cause is *somewhere* on the path between a kernel-mode
interrupt push of an iret frame and the wrapper's `iretq` reading it
back; we've ruled out the obvious culprits (`context_switch`'s
`jmp [rsi+56]` reading 0, null function pointers in `Platform`'s
vtable, syscall-ABI register clobbers — that last one was a real bug,
fixed, and dropped the rate from ~20% to 0/30 at one point).

**The kernel recovers**: the page-fault handler kills the offending
task and the rest of the system keeps running. In the captured boot
log, you'll see DEMO 2 → recovery → DEMO 3 → recovery → DEMO 4 → DEMO 5
all complete despite three intermediate `task #40` events.

The `page_fault_handler` carries a verbose state dump (KERNEL_RSP,
TSS.RSP0, all `CONTEXTS[*]`, 32 quadwords around saved_RSP, the
`CTX_LOG` ring buffer of recent context switches) that fires only when
`instruction_pointer == 0`. It's intentional diagnostic, not
production code.

### DEMO 5 + DEMO 6 are code-complete but disabled in boot

The infrastructure is solid — VirtIO block driver, BlockDevice trait,
snapshot module, raw-sector persistence all verified independently;
exfil-demo crate compiles and the 8 attacks are correctly designed.
But the additional code paths reopen task #40's race window often
enough that under any of them the kernel can't reliably run the
Ring-3 demos that *are* working today (DEMOs 0/1/4). Toggling
DEMO 5 or DEMO 6 back on is a one-line change in
`kernel-x86_64/src/main.rs::init_loader_task` once task #40 is
properly fixed.

## Status, honestly

This is an **early-stage kernel**, not a daily driver. The interesting
parts are:

- The **policy model** (kernel-mediated LLM data flow, tier-based
  redaction at the syscall boundary) is real and demonstrable from
  Ring 3.
- ~33 of ~40 numbered syscalls have real handlers; ~10 are exercised by
  the boot demos. The rest are correct-on-paper but untested for
  absence of a user program that calls them.
- Platform plumbing (paging, scheduling, ELF loading, PCI/VirtIO,
  framebuffer, APIC) is real and works.
- The "LLM" services do **rule-based** redaction. A real on-device
  model is the obvious next milestone for the security thesis.

## License

MIT or Apache-2.0, your choice.
