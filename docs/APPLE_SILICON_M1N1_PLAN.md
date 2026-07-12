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

### ✅ M3 — m1n1 boot entry: drop from EL2 to EL1 before the kernel runs (`main.rs`)

Done: `_start` preserves the DTB pointer and passes it to `kmain`; `kmain` parses the tree
and prints its contents.

Remaining, now completed: **m1n1 enters at EL2**, so `_start` now checks `CurrentEL` and,
if it is 2, drops to **EL1h** before any Rust code runs. The kernel touches only `_EL1`
registers (`VBAR_EL1`, `SCTLR_EL1`, `TTBR0_EL1`, `CPACR_EL1`, `CNTV_CTL_EL0`, etc.), so
running at EL2 with `HCR_EL2.E2H` set would silently redirect those writes to their `_EL2`
equivalents, installing vectors the CPU never uses and configuring the wrong translation
regime.

The drop sets:

- `HCR_EL2.RW=1` so EL1 is AArch64.
- `HCR_EL2.{E2H,TGE}=0` so EL1 is a real EL1, not the VHE host mode, and
  `IRQ/FIQ/SError` are routed to EL1.
- `CNTHCTL_EL2` to let EL1 read the counter and use the EL1 physical timer without
  trapping.
- `CPTR_EL2` to not trap FP/SIMD (already enabled at EL1 later, but we must not trap it
  from EL2 either).
- `SCTLR_EL1` to a safe reset value (MMU and caches off) before `eret` into EL1h.

The E2H bit is read back after the clear because some cores have E2H RES1. If it stuck at
1, the kernel would access `CNTHCTL_EL2`/`CPTR_EL2` in their *VHE* layouts at different
bit positions; guessing wrong there costs the timer or traps every FP instruction.

**QEMU-verified** with `-M virt,virtualization=on`, which enters the kernel at EL2 exactly
like m1n1 does. `kmain` reports `CurrentEL = EL1`, the MMU comes up, the memory allocator
initializes, and the scheduler preempts just as it does when QEMU enters at EL1 by default.

One caveat for the real m1n1 handoff: m1n1 may leave the MMU on at EL1. The early BSS-zero
loop runs with whatever mapping it inherits; if BSS is inside the mapped RAM window (it
will be, for a normal payload placement), there is no problem. If not, it faults before
`kmain` — but we will see that on screen because the framebuffer mirror is wired into the
panic handler.

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

### ✅ M2b — Framebuffer console (`fb.rs`, `font.rs`) — the kernel's only voice on a Mac
**Why this exists.** On a MacBook the hardware UART that M2 drives is not on any port you
can plug a cable into, and m1n1's serial console is a **USB gadget** that needs a *second
machine* on the other end of a USB-C cable. With one Mac, a UART-only kernel boots
completely blind — every diagnostic we have built would go nowhere. m1n1 has already
brought the display up (it prints its own log there) and passes the framebuffer on in the
device tree, so that is where we speak.

`Fdt::simple_framebuffer()` reads the `/chosen` `simple-framebuffer` node (`reg`, `width`,
`height`, `stride`, `format`). The **mirror is installed at the bottom, in `uart_put`**, so
every existing caller — the FDT log, the memory report, the panic handler — reaches the
screen without knowing a screen exists.

- **Brought up *before* the MMU**, since the framebuffer is physical memory and translation
  is still off. That puts the MMU and memory logs — the two things most likely to go wrong
  on a Mac — on screen instead of into the void.
- **Mapped and reserved.** Its pages go into the boot map (it is the console; unmapped is
  fatal) and are reserved from the frame allocator. m1n1 normally lists it in
  `/reserved-memory`, but if that listing were ever missing, the allocator would hand the
  screen out as scratch and the console would dissolve into whatever landed there.
- **Two pixel formats, and Apple is the reason.** `x8r8g8b8` is the common case, but Apple
  panels commonly run **10 bits per channel** (`x2r10g10b10`) — writing 8-bit-packed
  pixels there produces a dim, wrongly-coloured mess rather than an obvious failure.
- 8x8 bitmap font (public-domain `font8x8`), not the x86 side's TrueType rasterizer: this
  has to work before the allocator exists and must not depend on anything that can fail.
  Auto-scales (`height/400`, clamped 1–4) so 8px glyphs are readable on a Retina panel.

**QEMU-verified**, by splicing a `simple-framebuffer` node into QEMU's own dumped DTB and
feeding it back with `-dtb` — `virt` has no framebuffer of its own. The kernel renders into
real memory, and a temporary probe read the pixels back rather than trusting a screen nobody
is looking at: **1380 lit pixels**, and the packed values are exact in both formats.
The 10-bit one is the one that matters: `FG` = `(0xC8, 0xD0, 0xD8)` widens to
`(0x323, 0x343, 0x363)` and packs to **`0x323D0F63`** — exactly what was read back.

Unverified on Apple hardware: that m1n1's node is where and what we think it is (it should
be, it is the same `simple-framebuffer` binding), and the real panel's format.

### 🔨 M6 + M7 — Apple AIC2 + timer (`aic.rs`, `main.rs`) — written, not yet run on a Mac
**Target is an M1 Pro (`t6000`), so this is AIC *2*, not the AIC1 of the original M1.**
Different `compatible` (`apple,aic2`), different register layout. Both facts below come
from Linux's `drivers/irqchip/irq-apple-aic.c`, not from anything QEMU can show us.

**The finding that reshaped the milestone: on Apple the timer is not an AIC interrupt
at all.** The ARMv8 generic timer is delivered straight to the CPU as an **FIQ**, and is
identified by reading `CNTP_CTL_EL0` — there is no controller register to ack. The AIC
handles *device* interrupts only. Two consequences:

- Our vector table sent the FIQ slots to `exc_handler`, which prints and halts. **On a
  Mac the first timer tick would have halted the kernel.** FIQ slots now branch to the
  same `irq_entry` trampoline as IRQ, and boot clears `PSTATE.F` as well as `PSTATE.I`
  (`daifclr, #3`) — with F still set, the M1 Pro would simply never tick.
- A preemptive scheduler on Apple therefore does **not** require the AIC. It requires
  the FIQ vector. The AIC is only needed once there are device drivers.

**One handler serves both machines.** The tick is no longer driven off an INTID —
it is driven off `CNTP_CTL_EL0.ISTATUS`, which is the timer's own statement that it
fired and is true whether the interrupt arrived as a GIC IRQ (QEMU) or a bare FIQ
(Apple). The controller-specific work is then just the handshake: GIC needs its
IAR/EOIR, AIC needs its event register drained.

**AIC2's register offsets are not constants.** Only `IRQ_CFG` (0x2000) is fixed; the
mask registers sit after a variable-length IRQ-config array sized by `AIC2_INFO3.MAX_IRQ`,
read at probe. Hardcoding offsets from another SoC compiles, boots, and silently never
delivers an interrupt. Note also that **reading the event register *is* the ack** — it
acks *and masks* — so EOI is an unmask, and anything we have no driver for simply stays
masked instead of re-firing forever. The event register is a **second `reg` range** in the
tree (the die count is not discoverable from the capability registers), which is why
`Fdt::compatible_regs()` now returns all of a node's `reg` entries; the kernel refuses to
drive an AIC2 whose tree gives only one.

**QEMU-verified (the GIC half, which is more than it sounds):** the GIC bases now come
from the FDT (`dist @0x0800_0000`, `cpu @0x0801_0000`) instead of constants, and both demo
tasks still preempt normally — *with the timer detected via `ISTATUS`*. That is the exact
code path Apple will use, so the detection logic is genuinely exercised.

**Not verified, and cannot be under QEMU `virt`:** FIQ *delivery*, the AIC2 register
arithmetic, and the `apple,aic2` bring-up. There is no AIC to talk to. First real test is
a Mac. Single-die only — `t6002` (M1 Ultra) needs `die_stride` and is refused rather than
silently driven as die 0.

### ⬜ M9 — Validate scheduler + context switch on Apple
Reuse `context.rs`; confirm the first **FIQ** timer tick drives `timer_schedule`.

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
