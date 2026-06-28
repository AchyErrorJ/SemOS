# Semantic OS — Ring-0 LLM-Mediation Security Concern

**Date:** June 2026
**Status:** Open architectural concern. Not blocking immediate work. Worth resolving before any production claim.

---

## The thesis being protected

Semantic OS's security model rests on a specific claim: **the LLM agent loop runs in ring-0 with kernel privileges, and this is safe because the kernel's attack surface is small and carefully audited.**

The argument: a typical Linux kernel exposes ~400 syscalls, dozens of subsystems, decades of accumulated vulnerabilities. Semantic OS, written from scratch in Rust with a deliberately narrow syscall surface, exposes orders of magnitude less attack surface. Therefore an LLM running with kernel privileges in Semantic OS is meaningfully different from an LLM running with kernel privileges on Linux. The kernel's smallness is what makes the architecture defensible.

This is the *ring-0 LLM-mediation thesis*. It's a real argument.

---

## The flaw

The flaw is found in the proposed roadmap expansion path. To make the OS *useful* for daily development — running rustc, browsing the web, hosting Legible Studio — the obvious shortcut is to widen the syscall surface to match Linux/POSIX expectations. Port a libc. Implement enough of `read`/`write`/`open`/`mmap`/`fork`/`exec`/etc. to let unmodified software run.

Once you do that, **the kernel's attack surface stops being meaningfully different from Linux's.** Importing Linux syscall semantics re-imports Linux attack surface. You can write the implementation in Rust, but the attack-surface argument was never about the implementation language — it was about the size and shape of the privileged interface.

Concretely:

1. A bug in a Linux-shaped `mmap` implementation in Rust is still an `mmap`-shaped bug — same edge cases, same class of vulnerability, same exploit techniques apply.
2. The LLM agent's prompts and outputs become inputs to a large attack surface, not a small one.
3. The "ring-0 LLM is safe because the kernel is small" claim no longer holds.
4. *The kernel's distinguishing security property quietly evaporates while the kernel is becoming useful.*

This is not a bug in the code. It's a contradiction in the architectural commitments: **the kernel cannot be simultaneously useful (in the Linux-compatible sense) and safe (in the small-attack-surface sense) without explicit design choices that mediate between the two.**

---

## Why this matters now

It doesn't block the immediate work (Phase 14 self-hosting via rustc port). The current rustc port has been done as a `target_os = "none"` build against a minimal `semos-std` surface, not against a libc. That's the correct path: the rustc port treats Semantic OS as its own platform, not as a Linux clone. The semos-std surface is being grown deliberately, one capability at a time, in response to specific rustc requirements.

The concern surfaces when later phases imply broader compatibility:

- **Phase 15 (web browser)** — vendoring html5ever or similar pulls in transitive dependencies that expect a richer std surface
- **Phase 17 (cargo integration in the agent)** — cargo itself expects POSIX-ish process management, environment variables, file locking, signal handling
- **Phase 18 (package manager / crates.io install)** — installed crates were written for Linux/macOS/Windows; running them requires either patches per-crate or a broader compatibility layer
- **Phase 19 (media + games)** — ffmpeg, decoders, game engines all assume POSIX

Each of these pulls toward "make Semantic OS act more like Linux." Each erodes the attack-surface argument.

---

## What the contradiction actually means

The kernel currently lives in a productive tension:

| Useful | Safe |
|---|---|
| Wide syscall surface | Narrow syscall surface |
| Compatibility with existing software | All software written for this platform |
| Easy to port external code | Difficult to port external code |
| Standard tooling works | Custom tooling required |
| Familiar mental model | Novel mental model |
| LLM has lots of capabilities | LLM has few capabilities, well-audited |

The roadmap expansion proposal pushes everything in the left column without acknowledging that *each step in that direction is a step away from the kernel's distinguishing claim.*

The choice isn't "useful vs safe" as a binary. It's "where on the spectrum and how do we mark the trade-offs explicitly."

---

## Proposed mitigations (in order of cost)

### 1. Make the trade-off explicit in the architecture

Before any new compatibility-driven capability is added to the kernel, the roadmap entry for it must answer:

- Does this widen the syscall surface? If yes, by how much, and what new attack-surface category does it introduce?
- Is there a Semantic-OS-native API that achieves the use case with a smaller surface?
- What protection ring should this live in? Does it need kernel privileges, or can it be a Ring-3 capability accessed via a narrow syscall?

This is process, not technology. It costs nothing except discipline. **This is the minimum mitigation and should be adopted now.**

### 2. Maintain a "kernel surface inventory"

Track, in a single document, the current set of privileged operations the kernel exposes. Every new syscall is an explicit addition with an audit-time entry. This becomes the document that supports the safety claim: *"these are the N things the LLM can do at ring-0, here is the audit of each."*

When N grows large, the safety claim weakens. Better to *see* the growth than to wake up one day and discover the surface is Linux-sized.

### 3. Separate the LLM agent from the kernel

The most aggressive mitigation: **do not run the LLM at ring-0.** Move the agent loop to a userspace process with explicit capabilities granted via syscalls. The kernel remains small; the agent's blast radius is bounded by the capabilities it's been granted.

This is a meaningful refactor of M22 and the related agent infrastructure. It's also the most defensible architecture for the security thesis, because the kernel's safety claim no longer depends on the agent loop's behavior — only on the syscall surface, which is auditable.

The current ring-0 placement was chosen for performance and integration simplicity. Those are real benefits. But the security thesis is in tension with that choice, and the tension is the actual problem.

### 4. Capability-based access control

A more sophisticated version of (3): expose capabilities to the agent via opaque tokens that can be granted, revoked, and audited. The agent doesn't have "filesystem access" — it has a capability token for a specific file or directory, and the kernel enforces the boundary. This is the Plan 9 / seL4 / Fuchsia model.

This is months of work. It's also the architecture most consistent with the security thesis as stated.

### 5. Formal verification of the syscall layer

If the kernel stays small enough to be formally verified (think seL4-class, 10K LOC of kernel code), the safety claim can be made rigorously rather than rhetorically. This is years of work for one person; it's also the strongest possible version of the claim.

Realistic only if the kernel is held at a fixed small size and not allowed to grow into a daily-driver feature set.

---

## What this means for the roadmap

The Phase 15-19 expansion is exciting but it's also where the security thesis breaks if pursued naively. Three honest options:

### Option A: Hold the security thesis, accept the constraint

The kernel stays small. The LLM stays ring-0. The OS is useful for a narrow set of workloads (running Legible, running the agent loop, running a few carefully-ported tools). Web browser, package manager, video playback, retro games — not on this OS. Those happen on the dual-boot host (macOS or Linux), accessed via the shared exFAT partition.

The OS is a *focused* tool, not a general-purpose computer. The security thesis stays defensible.

### Option B: Move the LLM out of ring-0

Refactor M22 and the agent infrastructure so the agent loop runs as a userspace process. Then expand the syscall surface freely without breaking the security thesis, because the security thesis no longer depends on what the agent can reach.

This is the option that lets the roadmap expansion happen without contradiction. It requires reworking the existing agent integration. It's the right answer if the OS is meant to be general-purpose.

### Option C: Drop the security thesis and reframe

If the LLM runs at ring-0 and the syscall surface grows to Linux-sized, the OS is no longer distinguishable from "a small Linux written in Rust." That's a fine project, but the pitch changes. The OS becomes a *clean implementation* rather than a *security-distinguished implementation*. Different value prop, different audience.

This option is honest but it's also a strategic step down from where the project sits today.

---

## Recommendation

**Adopt mitigation 1 immediately.** Every roadmap entry from now on includes the syscall-surface question. No new privileged capability is added without an explicit note about whether it widens the attack surface and why.

**Adopt mitigation 2 within the next month.** Build the surface inventory before more capabilities accrete. Use the act of writing the inventory to feel the weight of what's already there.

**Choose between Options A, B, and C before Phase 15 begins.** The web browser is the first feature whose implementation strategy is shaped by this choice. Picking the option after Phase 15 starts means undoing work; picking before means the work is shaped correctly from day one.

If forced to recommend one: **Option B**. Move the LLM out of ring-0. The performance cost is recoverable; the architectural integrity is not. Other secure OS projects (seL4, Fuchsia, even Linux with sandboxing) have demonstrated that capability-mediated userspace agents can be fast enough. The ring-0 placement was a shortcut that solved a real problem but at a cost the project is now growing into.

---

## What this doesn't say

This document is not a claim that the current implementation is insecure. It's a claim that the current *architectural justification* for security is in tension with the *direction* the roadmap wants to go. The kernel today is fine. The kernel after Phase 15-19 may not be, unless the trade-off is made deliberately.

This is also not a claim that the security thesis is the most important property. For many users and use cases, "useful" matters more than "small attack surface." That's a legitimate choice. It just isn't the choice the project has been claiming to make, and the gap between claim and direction is what this document is naming.

---

## Open questions

1. Is the ring-0 placement of the LLM agent considered essential, or was it a path-of-least-resistance choice?
2. What's the actual current syscall surface? (The first action of mitigation 2.)
3. Are there workloads in the current roadmap that genuinely cannot work in userspace, or is everything in Phase 15-19 implementable as Ring-3 apps with narrow syscall grants?
4. Has anyone outside the project reviewed the security claim? If not, who would be the right reviewer to invite?

---

*This document is a working artifact. Update it as decisions are made and as the kernel evolves.*
