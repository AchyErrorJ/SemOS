# SemOS on Apple Silicon via m1n1 — bring-up plan

Boot SemOS's `kernel-aarch64` on Apple Silicon Macs using the **m1n1** bootloader
(the same stage-2 loader Asahi Linux uses). iBoot → m1n1 → SemOS, with m1n1
handing us a flattened device tree (FDT) and an early UART. The research *is* the
point; each Apple-specific driver is a milestone, not a chore.

Status legend: ✅ done · 🔨 in progress · ⬜ todo

## What the aarch64 port already has (reuse, arch-independent)

- MMU / page tables (`mmu.rs`), physical frame allocator (`memory.rs`)
- Scheduler + context switch (`context.rs`) — reuse unchanged
- `kernel-core` portable core (same crate as x86_64) via the `Platform` seam
- `NetDevice` trait already defined in `kernel-core/src/drivers/traits.rs`

The port is currently **hardcoded to QEMU `virt`** (PL011 UART @ `0x0900_0000`,
GICv2 @ `0x0800_0000`/`0x0801_0000`, generic timer IRQ 30, `RAM_END`
`0x4800_0000`, 128 MiB frame cap). The Apple work = replace those constants with
**FDT-driven runtime discovery**.

## Milestones

### ✅ M1 — FDT parser  (`kernel-aarch64/src/fdt.rs`)
Minimal no_std/no-alloc DTB parser: header + structure-block walk, bounds-checked.
API: `Fdt::from_ptr`, `memory() -> (base,size)`, `find_compatible(&str)`,
`stdout_path_uart()`, `totalsize()`. **QEMU-verified** on `virt` aarch64 — it parses
QEMU's real device tree and prints:

```
[fdt] /memory base=0x40000000 size=0x8000000
[fdt] stdout UART @0x9000000
[fdt] found arm,armv8-timer node
```

All three cross-checked against `dtc`'s dump of the same tree (`-M virt,dumpdtb=`).
Verified under QEMU only — nothing here has run on Apple hardware yet.

The live boot exposed three bugs that all compiled cleanly, and are now fixed:

1. **`parse_prop()` read every field one word early** — it took the `FDT_PROP` token
   as the length and the length as the name offset. The walk died on the root's first
   property (`interrupt-parent`), so `walk_nodes()` never visited a single node and
   `memory()` / `stdout_path_uart()` / `find_compatible()` all silently returned
   `None`. The parser was a total no-op.
2. **`walk_nodes()` never decremented `depth`** — the inner property loop consumed
   `FDT_END_NODE` itself, skipping the outer loop's `depth -= 1`, so siblings appeared
   progressively deeper. `memory()` matches `depth == 2`, which only root's first child
   could ever reach.
3. **FP/SIMD and `VBAR_EL1` were set up too late** (`main.rs`) — at `-O` LLVM
   auto-vectorizes the walker's array init with NEON, so the first *working* walk
   trapped with `EC=0x07` (FP access disabled); with `VBAR_EL1` still zero the trap
   vectored to PC `0x200` and spun forever, printing nothing. Both are now enabled at
   the top of `kmain`, before any nontrivial Rust runs.

**Boot it with a raw binary, not the ELF.** Handed an ELF via `-kernel`, QEMU boots
bare-metal: no DTB in guest RAM (it is all zeros at reset) and `x0 = 0`. Only a raw
image makes QEMU take the Linux arm64 boot path and actually hand over a tree. The
cargo runner (`kernel-aarch64/qemu-run.sh`) objcopies to raw before booting, so
`cargo run --release` now works. Worth remembering when the m1n1 handoff is wired up:
m1n1 *does* pass the DTB in `x0`, so that path exercises the parser for real.

### 🔨 M3 — m1n1 boot entry (partial)
Done: `_start` now preserves the DTB pointer (`x0`) across BSS-zero and passes it
to `kmain(dtb)`; `kmain` parses the tree and prints `/memory`, stdout UART, and the
timer node. **Remaining:** if m1n1 enters at EL2, drop to EL1 before MMU enable;
confirm the linker load address matches m1n1's payload placement.

### ✅ M2 — Apple UART early console (`serial.rs`)
The console is now **discovered, not compiled in**. `Fdt::stdout_uart()` resolves
`/chosen/stdout-path` (following an `/aliases` indirection) to a node and returns the
whole node — `reg` *and* `compatible`, because on Apple the address alone is not
enough to know how to drive it. `serial::init_from_fdt()` classifies that node and
retargets the console:

- **`arm,pl011`** — DR at +0x00, TXFF at bit 5 of FR (+0x18).
- **`apple,s5l-uart`** — the Samsung S3C descendant every Apple Mac uses: TX holding
  register at +0x20, TX-buffer-empty at bit 1 of UTRSTAT (+0x10), 32-bit access only.

`uart_put`/`uart_str` are unchanged, so every existing caller (including the panic
handler) follows the console to the discovered device. Neither path reprograms baud
or line control: QEMU and m1n1 both hand over a UART that is already transmitting,
and re-initializing a live console only risks dropping bytes. If the tree names no
UART we recognize, the pre-FDT PL011 guess stays and the kernel says so.

**QEMU-verified, and verified to be *load-bearing*.** The honest test is not that
`virt` still prints — the discovered base (`0x900_0000`) is the same address the old
constant had, so a no-op parser would look identical. Poisoning the compiled-in
default to a dead offset inside the PL011 page makes the boot silent until
`init_from_fdt` runs, and output then begins at exactly the retarget line:

```
  [uart] console from FDT: arm,pl011 @0x0000000009000000
  [fdt] found arm,armv8-timer node
  ...
  MMU ON — translation active.
```

Two things that cost real debugging time, worth keeping:

1. **A wrong UART base is a hang, not a silent console.** QEMU raises an external
   abort on unassigned physical addresses, and the boot banner prints *before*
   `VBAR_EL1` is set, so that abort vectors into nothing and spins with zero output —
   the same failure signature as M1's bug #3. When the Apple base turns out wrong,
   look for a fault, not a dead device.
2. **The console has to be in the MMU map.** `enable_identity_mmu()` now takes the
   console base and maps its 1 GiB block as device memory. Apple's UART sits near
   `0x2_3520_0000`, far outside the fixed 2 GiB QEMU window; without that block the
   first print after `SCTLR_EL1.M` goes high is a data abort — precisely when the
   kernel loses the ability to tell you why. T0SZ=25 gives a 39-bit VA, so the L1
   table has a slot for anything under 512 GiB.

Unverified on Apple hardware: the s5l register offsets are from m1n1/Linux, not from
a Mac that has booted this code.

### ⬜ M4 — Physical memory discovery (`main.rs`, `memory.rs`)
Parse `/memory` from the FDT; compute the range after kernel + stack + FDT; feed
`memory::init(base, size)`.

### ⬜ M5 — Allocator scaling (`memory.rs`)
Raise/`dynamic`-ize `MAX_FRAMES` (currently 32 768 = 128 MiB) + multi-bank support
for real Apple RAM.

### ⬜ M6 — Apple AIC interrupt controller (new `aic.rs`, `main.rs`)
`aic_init/ack/eoi/enable`. Replace the GICv2 reads in `irq_handler`. (Apple's own
controller — **not** ARM GIC.)

### ⬜ M7 — Timer (`main.rs` or new `timer.rs`)
If the FDT wires `arm,armv8-timer` to the AIC, keep the generic-timer logic but read
the IRQ from the FDT; otherwise implement the Apple timer MMIO.

### ⬜ M8 — Apple-aware MMU identity map (`mmu.rs`)
Replace hardcoded 2 GiB QEMU map with FDT-driven RAM + MMIO (UART, AIC, timer, FDT)
mappings; set TCR/IPS for the Apple SoC's PA range (likely 48-bit).

### ⬜ M9 — Validate scheduler + context switch on Apple
Reuse `context.rs`; confirm the first AIC timer IRQ drives `timer_schedule`.

### ⬜ M10 — Driver registry (new `drivers/mod.rs`, `main.rs`)
Global `Option<&'static dyn NetDevice>` + `register_net_device()` / `net_device()`.

### ⬜ M11A — Network: virtio-net (stepping stone, do first)
virtio transport (MMIO/PCI) discovered from FDT → virtio-net → `NetDevice`. Lights
up the whole stack on ARM *without* the Apple DMA fight. **Validate under QEMU virt
aarch64 before real hardware.**

### ⬜ M11B — Network: real Apple hardware (the research)
1. Apple PCIe host controller (ECAM, enumeration, MSI) — `apple_pcie.rs`
2. **DART** IOMMU (map DMA IOVAs) — `apple_dart.rs`
3. Apple NIC driver + `NetDevice` — `apple_nic.rs`
(Wired Ethernet only on Studio/Mini/Pro; MacBooks are Broadcom WiFi = much harder.)

## Asahi verification checklist (RE research — confirm before coding the Apple bits)
1. Real m1n1 **FDT dump** for the target Mac (node names / `reg` ranges)
2. **AIC** register layout + EOI/ack flow
3. m1n1 entry EL (EL1 vs EL2) and MMU on/off at handoff
4. Apple UART `compatible` string, clock, register width
5. **PCIe/DART** version + the NIC `compatible` string

## Immediate next step
M1 (FDT parse) and M2 (UART-from-FDT) are done and QEMU-verified, so the console no
longer depends on a compiled-in constant. Next is **M4/M5 — memory from the FDT**:
`RAM_END` (`0x4800_0000`) and `MAX_FRAMES` (32 768 = 128 MiB) are the last hardcoded
QEMU facts in the boot path, and Apple RAM starts at `0x8_0000_0000` with orders of
magnitude more of it. Then M11A (virtio-net) on QEMU aarch64.

An **RK3588 SBC** would still be the honest intermediate rig — a real device tree from
hardware nobody tuned this parser against — before a Mac.
