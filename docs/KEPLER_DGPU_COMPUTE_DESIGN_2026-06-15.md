# Kepler dGPU compute — design (2026-06-15)

Groundwork for **Phase 12 / M18: local LLM inference on the T540p's discrete
GPU** — the capability that removes the last remote dependency and makes the OS
fully sovereign (the agent's brain runs on your own silicon). Design only; bring-
up validation needs the machine.

The card: **GeForce GT 740M = Kepler GK208**, CUDA compute capability **3.5**,
~384 CUDA cores (2 SMX), ~28 GB/s DDR3, ~2 GB, PCIe. Small — single-transformer-
layer / tiny-model scale, not a 7B. That's fine: it proves the *path*, and "the
agent thinks locally" doesn't require GPT-4 at home.

---

## 1. Why Kepler is the from-scratch sweet spot (the load-bearing fact)

**NVIDIA made GPU firmware signing MANDATORY starting with Maxwell GM20x (2015).**
On those and everything after (Pascal, Turing+GSP, Ampere, Ada), you cannot run
the graphics/compute engines without NVIDIA-signed firmware blobs you can't
produce — which is why from-scratch NVIDIA compute is effectively impossible on
modern cards without their proprietary firmware.

**Kepler (GK208) predates that.** Its engine microcontrollers (FECS / GPCCS — the
PGRAPH context-switch falcons) run **unsigned microcode**, and the whole register
interface is **documented by envytools** (the nouveau reverse-engineering project).
So GK208 is one of the *last* NVIDIA GPUs you can bring up to compute entirely
from scratch with public documentation. This is the single reason M18 is tractable
on this machine and would NOT be on a newer card.

> Pre-GSP, pre-signed-firmware, fully envytools-documented. The T540p's "old"
> dGPU is, for this project, the *right* dGPU.

**Primary documentation sources:**
- **envytools** (`envytools.readthedocs.io` / the git repo) — the authoritative
  register/ISA reference for Kepler (`nvc0`/`nve4` families). PFIFO, PGRAPH, the
  VM/MMU, the falcon ISA, and the Kepler SM ISA (SASS) all documented here.
- **nouveau** (Linux `drm/nouveau`) — a working reference driver for GK208
  bring-up order (devinit, instance memory, channel/pushbuffer, compute object).
- **`envyas`/`envydis`** — assembler/disassembler for Kepler SASS (for hand-
  writing or checking compute kernels without NVIDIA's toolchain).

---

## 2. The compute submission model (what we actually build)

GPU compute on Kepler is "post commands to a ring, kick a doorbell, read results
back from VRAM." The pieces, in dependency order:

1. **PCI + BARs.** Find the GPU (vendor 0x10DE), map BAR0 (MMIO registers), BAR1
   (a window into VRAM), enable bus-master. (We already PCI-scan; add 0x10DE.)
2. **devinit.** Run the VBIOS init tables (clocks, memory controller, straps)
   OR the minimal subset nouveau does. Brings the card from cold to "MMIO live."
3. **Instance memory + the GPU MMU.** Kepler has its own page tables (VM). Set up
   instance blocks + a GPU address space so the card can address pushbuffers and
   data buffers. Map host/VRAM buffers into the GPU VM.
4. **PFIFO channel.** Allocate a **GPFIFO channel**: a ring of (pushbuffer addr,
   length) entries the host fills, plus a USERD control area. This is the queue
   the GPU pulls work from. Bind the compute engine to the channel.
5. **PGRAPH + the Kepler compute object.** Instantiate the **Kepler compute
   class** (the `A0C0`-era compute object) on PGRAPH; load FECS/GPCCS microcode
   (unsigned on Kepler). This is the engine that runs SM kernels.
6. **Pushbuffer + kickoff.** Build a pushbuffer (a stream of methods: set up the
   grid/CTA dims, the kernel address, the constant buffers/params, then LAUNCH),
   append its (addr,len) to the GPFIFO ring, advance the put pointer, ring the
   channel doorbell. The GPU executes; we poll a semaphore/fence in VRAM for done.
7. **Readback.** Read the result buffer back through BAR1 / the VM mapping.

That loop — channel + pushbuffer + compute-class LAUNCH + fence — is the whole
game. Everything else is making the kernels good.

---

## 3. The ISA / kernel path

We need actual SM 3.5 machine code (SASS) for the compute kernels. Options, in
increasing ambition:

- **A. Hand-write SASS via `envyas`.** Write the few kernels we need (matmul,
  elementwise, softmax) in Kepler SASS, assemble with envyas, embed the binaries.
  Smallest path to a working forward pass. No compiler needed.
- **B. A minimal PTX→SASS step.** PTX is NVIDIA's virtual ISA; mapping a *subset*
  to Kepler SASS for our handful of kernels is a bounded codegen task (envytools
  documents the SASS). More flexible than hand-writing, much less than a full ptxas.
- **C. (later) lower from our own IR.** Since we have Cranelift in-tree (M27), a
  GPU backend is conceivable far down the line — overkill for M18.

**Start with A.** A hand-written tiled matmul + the glue kernels is enough to run
a transformer layer.

---

## 4. The compute target (what "it works" means)

A tiny transformer forward pass. The kernels that matter:
- **GEMM** (matmul) — the dominant cost: QKV projections, attention scores,
  the FFN. A tiled SM-3.5 GEMM (shared-memory blocking) is the one kernel to get
  right; everything else is cheap by comparison.
- **softmax**, **layernorm/RMSnorm**, **elementwise (GELU/residual)** — simple.
- **attention** = two GEMMs + a softmax.

Shapes for a *tiny* model (the GK208's ~2 GB + ~28 GB/s sets the ceiling): think
d_model ~256–512, a few layers, short context — enough to demonstrate local
inference, weights quantized (int8) to fit + cut bandwidth. The honest framing:
this proves "the agent can think on local silicon," not "run a frontier model."

**DEMO (the M18 done-line, from the roadmap):** load weights into VRAM, run a
single transformer layer forward pass on the GPU, read the output back, print it.

---

## 5. What's doable offline NOW vs gated on the machine

**Offline (design/build without the GPU):**
- The PCI 0x10DE discovery + BAR-map code (we already scan PCI).
- Study + transcribe the envytools register maps for GK208 (PFIFO, PGRAPH, VM)
  into a Rust register module (like `iwlwifi_csr.rs` was for the NIC).
- Design the channel / pushbuffer / GPFIFO data structures.
- Write + assemble the SASS kernels with envyas and unit-check them on a *host*
  with a real Kepler card or an emulator if available (the kernels are portable
  even if our bring-up isn't).
- The GEMM tiling math + the quantization scheme.

**Hardware-gated (needs the GT 740M powered + driven):**
- devinit, FECS/GPCCS ucode load, channel kickoff, the actual LAUNCH + readback.
- Everything past "MMIO is live."

---

## 6. Honest scope

This is **the hardest single item on the whole roadmap** — a from-scratch NVIDIA
compute driver is a multi-month effort even with envytools. But on GK208 it is
*possible* (pre-signed-firmware), which it is not on anything newer. Sequence it
*after* WiFi + self-extension land; treat the offline register-transcription +
SASS-kernel work as the slow-burn groundwork that de-risks the eventual bring-up.

Mirror of the iwlwifi playbook: transcribe the register map offline (`*_csr.rs`),
build the data structures, then iterate the bring-up sequence on hardware one
checkpoint at a time (PCI → devinit → channel → first pushbuffer → first LAUNCH).

---

## 7. Relationship to the rest of the OS

- Removes the **remote LLM dependency** → the agent thinks locally → "sovereign"
  becomes literal. This is the single most thesis-defining hardware direction.
- The compute path (channel/pushbuffer/fence) is **reusable** for any GPU compute
  (not just LLM) — e.g. the LegibleStudios CAD / tiny-skia rendering math.
- Independent of the iGPU *rendering* driver (M14) — different engine, different
  goal (compute vs display). They share only the PCI/VM groundwork.
