# Roadmap — GPU (render + compute)

> Part of the [Master Roadmap](../MASTER_ROADMAP.md). Sibling themes:
> [networking](map%20-%20networking.md) · [self-extension](map%20-%20self-extension.md) ·
> [phone](map%20-%20phone.md) · [platform](map%20-%20platform.md). Historical log: [ROADMAP.md](../ROADMAP.md).

Hardware-gated on real silicon. The T540p/W540 has an Intel iGPU (Haswell HD 4600)
+ a discrete **NVIDIA Quadro / GeForce GT 740M (Kepler GK208)**. Two tracks: iGPU
**rendering** (CAD view, video, games) and dGPU **compute** (the sovereign endgame
— local LLM inference removes the last remote dependency).

---

## Phase 11 — M14 iGPU rendering `[  ]`

Intel iGPU (Iris Xe on the P1 / HD 4600 on Haswell) — the LegibleStudios CAD view,
video, games. Reference: the Linux `i915` driver. Prerequisite for QuickSync
hardware video decode (see media in [platform.md](map%20-%20platform.md)).

---

## Phase 12 — M18 NVIDIA dGPU COMPUTE `[  ]`

**The thesis-defining hardware direction:** PTX/SASS submission → **local LLM
inference on the dGPU**, removing the remote-API dependency → fully sovereign.
"tinygrad-NV style," compute-only, **no graphics driver**.

**Why this card is tractable.** The T540p's dGPU is a **GeForce GT 740M = Kepler
GK208**, CUDA compute capability 3.0/3.5 — **PRE-GSP** (GSP firmware is Turing+,
2018) and **PRE-signed-firmware** (signing started Maxwell GM20x). It boots via the
older falcon/PMU model that nouveau/envytools document well — **no GSP upload, no
signed-firmware wall**. From-scratch compute on *this* card is more tractable than
on a modern card. Caveat: SM 3.x, ~384 cores, ~2 GB — single-layer / tiny-model
scale, not a big LLM.

**Design landed (offline):**
[`KEPLER_DGPU_COMPUTE_DESIGN_2026-06-15.md`](../KEPLER_DGPU_COMPUTE_DESIGN_2026-06-15.md)
— envytools/nouveau are the refs; submission = PFIFO channel + pushbuffer + Kepler
compute class; hand-write SASS via envyas; target = a tiny-transformer GEMM.
**PCI probe landed** (`gpu.rs`, image 21:40 2026-06-15): read-only PCI scan
(0x10DE / class 0x03) + BAR report at boot (MMIO chip-ID read gated off for
Optimus power-gate safety). Confirms the actual GPU + BARs when booted.

**Offline-doable now:** transcribe the envytools GK208 register map into a
`gpu_regs.rs` (like `iwlwifi_csr.rs`); design the channel/pushbuffer structs;
falcon/PMU boot study; SM 3.x ISA (SASS) / PTX-for-Kepler; the matmul kernels a
tiny model needs. **Bring-up validation needs the machine.**

> **Test-machine note:** the bezel reads "T540p" but the chassis is likely a
> **W540** (has the discrete Quadro). To verify via PCI scan for NVIDIA 0x10DE —
> affects which exact GPU/BARs the compute track targets.

---

## Open decision

**dGPU compute vs WiFi-first for "sovereign":** local inference on the NVIDIA card
is the most thesis-defining hardware direction (removes the last remote
dependency), but it's a big lift. The offline design + register-map work proceeds
in parallel; bring-up waits for an evening on the machine. The P1 Gen 6 (RTX dGPU)
is where larger-scale GPU work eventually moves.
