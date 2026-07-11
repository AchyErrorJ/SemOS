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

### ✅ M4 + M5 — Physical memory discovery & allocator scaling (`memory.rs`, `fdt.rs`, `main.rs`)
`RAM_END` and `MAX_FRAMES` are gone. The frame pool is now built from the tree:
`memory_banks()` returns **every** `reg` entry of **every** `/memory` node (a node can
carry several `(addr,size)` pairs, and there can be several nodes — the old
`memory()` took the first pair of the first node and silently lost the rest).

**The bitmap is sized from the discovered RAM and carved out of that RAM.** One bit
per 4 KiB frame is 32 KiB per GiB — 4 KiB of metadata for QEMU's 128 MiB, ~2 MiB for
a 64 GiB Mac — and it is the only memory that must be written at boot. The rejected
alternative, threading a free list through the free frames themselves, needs no static
metadata but has to *touch every free frame* to link it: ~128 MiB of scattered writes
on a 64 GiB machine, every one of which requires that frame to already be mapped. That
would have dragged the whole Apple-aware MMU (M8) into this milestone.

Bit semantics: **1 = unavailable**, 0 = free. The bitmap spans one contiguous range
from the lowest bank base to the highest bank end, so a frame index is pure
arithmetic and the holes between banks are simply born set.

**Reservations — what the loader left live in RAM.** `finalize()` opens the banks, then
closes back up:
- the **kernel image and stack** (`[_kernel_start, _stack_top)`, a new linker symbol);
- **the DTB itself** — QEMU parks it at `0x4400_0000`, *inside* RAM and *above* the old
  `_stack_top` pool floor, so the previous allocator was free to hand the device tree
  out as scratch. It survived only because nothing had yet allocated enough frames to
  reach it;
- the header's **`off_mem_rsvmap` block** and **`/reserved-memory`** children — on Apple
  these cover firmware regions and the framebuffer m1n1 is still scanning out of;
- the bitmap's own frames.

Reservations round *outward* to whole frames: over-reserving costs a page, under-
reserving corrupts something still in use. `free()` also rejects any address that is
reserved or outside a bank, so a stray free of the DTB can't quietly add it to the pool.

**QEMU-verified — the frame count tracks the RAM size:**

| `-m` | bank | bitmap | allocatable |
|---|---|---|---|
| 128M | 128 MiB | 4 KiB | 27 311 frames (106 MiB) |
| 1G | 1024 MiB | 32 KiB | 256 680 frames (1002 MiB) |
| 2G | 2048 MiB | 64 KiB | 256 672 frames (1002 MiB) — 1024 MiB withheld |

The MMU self-test passes with no leaks in all three. The gap between the bank size and
the allocatable total is real and accounted for: `kernel-core`'s 16 MiB heap arena is a
`.bss` static, so it is *inside* the kernel image reservation.

**The DTB exclusion was verified directly, not inferred**: a temporary probe drained the
allocator to exhaustion and checked every frame it returned — 27 311 frames handed out,
**zero** inside the DTB or the kernel image.

**The `-m 2G` row is the M8 boundary showing through.** The boot identity map covers one
1 GiB RAM block (`mmu::IDENTITY_RAM_BASE..IDENTITY_RAM_END`). `mmu.rs` zeroes every frame
it allocates, so an unmapped frame is a data abort, not a bad pointer to debug later —
so RAM beyond the window is reserved and reported rather than handed out. Apple's RAM at
`0x8_0000_0000` is entirely outside it: **on a Mac, this code discovers the RAM correctly
and then withholds essentially all of it until M8 maps it.** That is the intended, honest
failure mode — it does not hand out memory it cannot address.

Unverified on Apple hardware: multi-bank trees, `/reserved-memory`, and the `off_mem_rsvmap`
path all parse, but QEMU `virt` presents a single bank and an empty reservation block, so
none of those branches has met a real tree.

### ⬜ M6 — Apple AIC interrupt controller (new `aic.rs`, `main.rs`)
`aic_init/ack/eoi/enable`. Replace the GICv2 reads in `irq_handler`. (Apple's own
controller — **not** ARM GIC.)

### ⬜ M7 — Timer (`main.rs` or new `timer.rs`)
If the FDT wires `arm,armv8-timer` to the AIC, keep the generic-timer logic but read
the IRQ from the FDT; otherwise implement the Apple timer MMIO.

### ✅ M8 — Apple-aware MMU identity map (`mmu.rs`)
The boot map was two hardcoded L1 entries: 1 GiB of device at 0, 1 GiB of RAM at
`0x4000_0000`. It is now **built from the RAM the tree described** — which is why the
banks are discovered *before* the MMU comes up rather than after.

- **Whole gigabytes get 1 GiB L1 blocks; a partial gigabyte gets a static L2 table of
  2 MiB blocks covering only the RAM itself.** That distinction is not pedantry: normal
  memory is *speculatively accessible*, so blanketing a 1 GiB block over a 128 MiB bank
  invites the CPU to speculatively read physical addresses that do not exist. QEMU
  shrugs; an Apple SoC answers with an SError. Device blocks (`nGnRnE`) are never
  speculated into, so MMIO still gets whole 1 GiB blocks.
- **The L2 tables are static.** The boot map is built before the frame allocator exists
  — the allocator needs the map in order to touch its own bitmap — so there is nowhere
  to allocate a page table from yet. Eight of them, enough for eight partial banks.
- **`TCR_EL1.IPS` is read from `ID_AA64MMFR0_EL1.PARange`** instead of being hardcoded to
  40-bit. Programming an IPS smaller than the addresses in the tables is how translations
  quietly fault, and Apple's PA space is larger than a Cortex-A53's.
- `T0SZ=25` (39-bit VA) is unchanged and *is* sufficient: 512 GiB of L1 slots covers
  Apple RAM at `0x8_0000_0000` even on a 192 GiB machine. The addressing was never the
  problem — the map was.
- RAM the map could not cover is still withheld from the allocator (`mapped_ram()`),
  so the failure mode stays "withhold and report", never "hand out memory we cannot reach".

**QEMU-verified — this is the test that failed before:**

| `-m` | allocatable before M8 | allocatable after M8 |
|---|---|---|
| 128M | 106 MiB | 106 MiB (now via the L2 path) |
| 1G | 1002 MiB | 1002 MiB |
| 2G | 1002 MiB — **1024 MiB withheld** | **2026 MiB, nothing withheld** |
| 4G | (would withhold ~3 GiB) | **4074 MiB, nothing withheld** |

The MMU self-test passes and the scheduler preempts normally in every case. Counting the
frames is not the same as reaching them, so that was checked directly too: a temporary
probe wrote and read back the **last page of the highest bank** — `0x1_3FFF_F000` on the
4 GiB machine, a physical address of 5 GiB, four L1 entries past anything the old map
had. Under the old map that access is a translation fault, which is precisely why `-m 2G`
used to withhold half the machine.

Unverified on Apple hardware: whether Apple MMIO needs the `nonposted-mmio` semantics
Asahi's DT advertises (device-nGnRnE may not be sufficient), and the real `PARange` — the
IPS read is hardware-driven, but a Cortex-A53 reports 40-bit, so the wider Apple value has
never actually been programmed.

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
M1 (FDT parse), M2 (UART-from-FDT), M4/M5 (memory-from-FDT) and M8 (FDT-driven MMU map)
are done and QEMU-verified. **The only thing left hardcoded to QEMU `virt` in the boot
path is the interrupt controller and timer**: `GICD_BASE`/`GICC_BASE` and `TIMER_INTID`
in `main.rs`. Apple does not have a GIC at all — it has AIC — so on a Mac the kernel now
boots, finds its console, discovers and maps all of RAM, and then writes GIC registers
into empty space and never takes a timer interrupt.

So the next milestone is **M6 — the Apple AIC** (`aic.rs`), followed by **M7** (timer IRQ
from the FDT rather than the hardcoded PPI 30) and **M9** (confirm the scheduler runs off
an AIC tick). M6 is the first milestone that cannot be verified under QEMU `virt` at all:
there is no AIC to talk to. That makes the Asahi RE checklist below load-bearing for the
first time — the register layout has to come from m1n1/Linux sources, and the first real
test is a Mac.

An **RK3588 SBC** remains the honest intermediate rig for everything up to here — a real
device tree from hardware nobody tuned this parser against, with multiple memory banks and
a populated `/reserved-memory`, none of which QEMU `virt` exercises. It has a GIC, so it
would also validate M6/M7's FDT-driven IRQ plumbing without the AIC.
