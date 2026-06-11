# Semantic OS — Kernel Surface Inventory

**Status:** Working document. Created 2026-06-11 to satisfy the security-thesis
gate before iwlwifi firmware upload (Stage 2b). See
`docs/semos-security-thesis.md` — this is the artifact that backs the
auditability claim (Discipline 1) and the declared-firmware-trust commitment
(Commitment 6).

**Scope of this first version:** the firmware-blob inventory and the DMA /
IOMMU posture are populated in full (they are the blocking items for the WiFi
firmware-upload milestone). The syscall surface section is a grounded stub —
the full per-syscall audit (the "weekend of work" in the thesis) is tracked as
open work below.

---

## 1. Device firmware blobs (Commitment 6 — declared trust)

Every opaque vendor firmware blob the kernel loads onto a peripheral processor
gets an entry here: device identity, blob identity (hash + version), the bus
access the device has (notably IOMMU containment), and the blast radius if the
firmware is malicious or compromised.

### 1.1 — Intel Wireless 7260 (iwlwifi)

| Field | Value |
|---|---|
| Device | Intel Wireless 7260, PCI `8086:08B2`, location `04:00.0` (behind PCIe bridge, bus 4) on the ThinkPad T540p |
| Blob | `iwlwifi-7260-17.ucode` |
| Version | TLV format, API ver 17 |
| Size | 1,049,340 bytes |
| SHA-256 | `5d81a6003df0228a497ad27f916ba2c979614b4c439b0f45a5f2873dc0607fe8` |
| Vendored | In-tree at repo root (`iwlwifi-7260-17.ucode`); never fetched at build time |
| Source | linux-firmware (GPL-redistributable) |
| Loaded by | `kernel-x86_64/src/wireless/iwlwifi_*` (Stage 2b, pending) |
| Runs on | The NIC's own processor, not the host CPU |
| **Bus access** | **Bus-master DMA, NOT IOMMU-contained — full physical memory** |
| **Blast radius** | **Read/write of all physical RAM, including the kernel and the ring-0 LLM agent context.** A compromised or malicious firmware can exfiltrate or corrupt any kernel state via DMA. |
| Containment status | **None.** IOMMU/VT-d is not implemented (see §2). Mitigation is deferred to the VT-d subsystem. |
| Risk acceptance | Declared trust, accepted to proceed with the WiFi track. Documented per Commitment 6 ("declared trust, not silent trust"). To be revisited when §2 lands. |

This is the project's first and (as of 2026-06-11) only opaque-firmware trust
boundary. It is, by size, the largest single unauditable component in the
system — megabytes of code nobody on the project can read. The thesis names
this as the system's largest attack-surface weakness; this entry is the
honest statement of it.

---

## 2. DMA / IOMMU posture

**Current state: the kernel has no IOMMU/VT-d driver. No DMA device is
contained.** Every bus-master device has unrestricted read/write access to all
physical memory.

This is not unique to the NIC. The following devices already operate as
bus-master DMA with no containment today:

| Device | Driver | Directed by |
|---|---|---|
| xHCI USB | `usb/xhci.rs` | from-scratch host code |
| EHCI USB | `usb/ehci.rs` | from-scratch host code |
| AHCI/SATA | `ahci.rs` | from-scratch host code |
| NVMe | `nvme.rs` | from-scratch host code |
| virtio-net | `virtio/net.rs` | from-scratch host code |
| **iwlwifi NIC** | `wireless/iwlwifi_*` | **opaque vendor firmware** |

The risk delta the security thesis correctly identifies: for every row except
the last, the DMA engine is commanded by **our own auditable code**. For the
NIC, it is commanded by **opaque firmware**. The hardware DMAs in every case;
the difference is who directs it.

**IOMMU containment is therefore a cross-cutting subsystem**, not a WiFi
feature — it would harden all six drivers. It is comparable in size to the
paging subsystem (DMAR ACPI table parse, root/context-entry tables,
second-level DMA page tables per device, fault handling). It is tracked as its
own roadmap item.

Open hardware question (thesis Q3): does the T540p (Haswell + HM87/QM87)
expose VT-d via a DMAR ACPI table with BIOS VT-d enabled? Non-vPro HM87 SKUs
may have it fused off. To be answered when the VT-d subsystem is scoped.

---

## 3. Syscall surface (stub — full audit pending)

As of 2026-06-11 the kernel exposes **66 syscalls**, numbered 0–117 (with
gaps), defined in `kernel-core/src/syscall/mod.rs`. The thesis target is "tens
of operations, not hundreds" — the surface is within that bound but the
per-operation audit (capability required, intended use, blast radius) is not
yet written.

**Format for each entry (to be filled in):**

| # | Name | Signature | Capability required | Intended use | Blast radius if misused |
|---|---|---|---|---|---|

The first three, as the format example:

| # | Name | Signature | Capability | Intended use | Blast radius |
|---|---|---|---|---|---|
| 0 | `SYS_WRITE` | `(fd, buf, len) -> n` | fd ownership | write to an open fd / stdout | bounded to the fd's target |
| 1 | `SYS_READ` | `(fd, buf, len) -> n` | fd ownership | read from an open fd / stdin | bounded to the fd's target |
| 2 | `SYS_EXIT` | `(code)` | none | terminate the calling task | self only |

**Open work (thesis Discipline 1 / Q1):** complete the table for all 66
syscalls. This is the "weekend of work" the thesis budgets. Not blocking the
WiFi firmware upload (the firmware entry above is the gate); blocking the
broader auditability claim.

---

## 4. Change log

- **2026-06-11** — Document created. §1.1 (iwlwifi firmware) and §2 (DMA/IOMMU
  posture) populated in full as the gate for Stage 2b firmware upload. §3
  syscall surface stubbed. Decision pending (user): proceed to Stage 2b under
  declared trust, or build VT-d first.
- **2026-06-11 (decision)** — The dev machine is a **ThinkPad T540p** (HM87,
  non-vPro). Empirically confirmed via the ACPI probe: 26 ACPI tables present,
  **no DMAR table** → the platform exposes **no VT-d**, even after a VT-x BIOS
  toggle. Per the thesis ("IOMMU where the platform allows; declared trust
  where it doesn't"), the **decision is to proceed with iwlwifi under declared
  trust on this machine**, with the blast radius in §1.1 as the accepted,
  documented vulnerability. This is the thesis's stated fallback, not a
  compromise of it. Containment is deferred to the production targets, which
  pivoted to **Apple Silicon (M1/M2, DART IOMMU)** and eventually **N1X (ARM
  SMMU)** — both ARM64, both with their own IOMMUs where the real containment
  work will land. The Intel-specific VT-d driver is therefore shelved; the
  T540p is an x86 dev rig and the iwlwifi chip driver is dev-rig-only (the
  portable assets are the 802.11 / WPA2 / NetDevice layers). **The kernel's
  documented vulnerability while iwlwifi runs on the T540p: a compromised NIC
  firmware can read/write all physical memory (no IOMMU). Accepted, here, by
  decision.**
