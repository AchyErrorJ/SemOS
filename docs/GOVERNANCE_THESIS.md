# Governance Thesis — SemOS doesn't trust the machine; it survives it

**Date:** 2026-10-01
**Status:** Working thesis. Reorients the project. **Supersedes** the framing of
"the OS that writes itself" as the headline claim — self-extension is demoted
from thesis to demonstrated capability (M27, DEMO 80, M1–M4, DEMO 93 all stand).
Companions: [`semos-security-thesis.md`](semos-security-thesis.md) (the *how* —
why a ring-0-adjacent agent is tractable in a small from-scratch system),
[`designer-os-thesis.md`](designer-os-thesis.md) (the *who* — the machine
proposes, the human disposes), [`TRUSTING_THE_MACHINE.md`](TRUSTING_THE_MACHINE.md)
(AI-code trust practices). This document is the *what instead of trust*.

---

## The claim

> Machine-written code cannot be trusted. SemOS assumes every byte of
> agent-authored code is hostile until proven otherwise — and proves it only
> by surviving a trial window with bounded authority.

Trust is not a property of code; it is a property of the *system around* the
code. The naive version of this project's thesis asked an undecidable question —
*is the machine's output correct?* — and paid for it in sysroot blobs,
opt-level=0 miscompiles, and maintenance tax on a capability nothing else
needed. The replaceable question is engineering: **given that the code may be
malicious or broken, what is the worst it can do — and can we bound it,
attribute it, detect it, and undo it?**

This is not a retreat. It is the discovery that the most valuable things built
under the old thesis were never about self-sufficiency:

- **Hash-bound vouch** — attribution: every installed byte traces to a diff, an
  approval, and eventually a prompt.
- **Trial/keep/auto-revert promotion** — time-boxed authority: bad code gets a
  window, then loses it.
- **A/B boot slots** — survivability: bad code cannot brick the machine.
- **SemFS journal** — legibility: agent actions leave a durable trail that
  survives a hard kill.
- **Security tiers** — bounded authority: the capability fence on agent code.

The governance machinery *was* the thesis all along. This document names it.

---

## Threat model: the machine's code is the adversary

The agent is not an external attacker; it is an *insider* that is supposed to
be making syscalls. Treat its output as a potentially compromised insider from
the moment of compilation.

What agent-authored code may attempt:

1. **Escalation** — exploit kernel-surface bugs (bad pointer validation,
   TOCTOU) to leave its tier and acquire ambient authority.
2. **Persistence** — install itself outside the journaled, trial-gated path so
   promotion and rollback never see it.
3. **Exfiltration / coercion** — use the network or the LLM context channel to
   move data or instructions across the trust boundary (the redactor exists
   precisely here).
4. **Approval capture** — produce output that persuades the human approver
   (misleading naming, benign-looking diffs, urgency). The human in the loop is
   part of the attack surface; the vouch chain must make post-hoc attribution
   easy precisely because pre-hoc judgment is fallible.

Note what is *not* in the model: we do not attempt to decide whether agent
code is "good." That question is undecidable and we stop pretending otherwise.

---

## Design principles — the six questions

Every new syscall, loader path, and agent capability must answer these before
landing (this extends the four surface questions in
[`map - self-extension.md`](roadmap/map%20-%20self-extension.md)):

1. **Authority** — what can this artifact touch, and is that the *minimum*?
   (Principle of least authority. No ambient access; every right is explicit.)
2. **Attribution** — does every installed byte carry a vouch chain back to a
   diff and a prompt?
3. **Detection** — does this action leave a durable, tamper-evident trail in
   the journal / logfile partition?
4. **Reversibility** — can this change be rolled back without human surgery?
   (A/B slots, trial revert.)
5. **Time** — is authority granted for a window with automatic expiry, or
   forever by default? (Forever is a bug.)
6. **TCB** — how much kernel code does this feature add to the part that must
   be right? (Smallness is a security property, per the security thesis.)

A feature that fails a question needs a design change, not an exception.

---

## The TCB problem: the kernel surface is the whole game

The entire governance framework rests on one assumption: **Ring 3 agent code
cannot acquire authority the tier system didn't grant.** The 2026-07-17 code
review ([`reviews/SEMOS_CODE_REVIEW_2026-07-17.md`](reviews/)) found that
assumption currently false: Critical arbitrary kernel read/write primitives
reachable from Ring 3 via `SYS_WRITE` / `SYS_LLM_CONTEXT` unvalidated pointers,
plus systemic High findings (TOCTOU, redactor fail-open, `static mut` under
preemption).

Under the old thesis these were bugs. Under this one they are **the thesis
collapsing at its foundation**: if agent code has ambient authority over the
machinery of governance, the tiers and vouches are theater. Priority follows:

- **P0 — close the escalation paths.** Validate every user pointer at the
  syscall boundary; eliminate TOCTOU between check and use; make the redactor
  fail-closed; remove `static mut` from preemptible paths. Nothing else on the
  roadmap outranks this.
- **P0 — audit the tier enforcement.** Each tier's authority must be derivable
  from code, not intention. Any gap found is a P0.
- **P1 — assume the boundary will be crossed anyway.** Detection and rollback
  must work even after a successful escape (journaling of *kernel-side* agent
  activity, not just userland installs).

---

## What we take from container runtimes — and what we refuse

Worth stealing:

- **Artifact identity.** Content-addressed, immutable images; a container's
  digest *is* its identity. SemOS already has the stronger version: the
  hash-bound vouch chain. Keep it that way — never install an artifact whose
  bytes don't match its vouched hash.
- **Default-deny.** Containers start with nothing and are granted rights
  explicitly. New agent artifacts must start at the lowest tier with minimal
  authority, escalating only through `SYS_VOUCH` (human taste as a capability —
  see the designer thesis).
- **Declarative manifests.** The `semos-pkg` DAG resolver is this; keep
  manifests small and reviewable — they are what the human actually approves.
- **Assume escape.** Container practice's healthiest habit: treat every
  isolation boundary as permeable and design the surrounding system for "when,
  not if."

Worth refusing:

- **The boundary itself.** Namespaces + cgroups over a shared kernel isolate a
  process's *view*, not its *attack surface* — the same syscalls reach the same
  kernel. Against this threat model (hostile code at the syscall boundary), a
  container boundary is decorative. The industry's own mitigations concede the
  point: gVisor interposes a *second kernel* between app and host kernel; Kata
  uses real hardware VMs. **SemOS's tiers are its container runtime.** The work
  is not to add containers; it is to make the tier boundary real (the P0s
  above) and to keep the kernel-core — SemOS's equivalent of the gVisor
  interposition layer — small enough to actually audit.

A container runtime would let us *kid ourselves* that approval is now safe.
The governance framework must remain honest that no isolation layer makes
machine-written code trustworthy — only survivable.

---

## Roadmap reorientation

**Promoted to the frontier (thesis-critical):**

| Item | Why |
|---|---|
| P0: close Critical/High review findings (pointer validation, TOCTOU, fail-closed redactor, `static mut`) | The TCB must be real or nothing else is |
| P0: tier-enforcement audit | Authority must be derivable from code |
| P1: trial-window hardening — measurable keep criteria, watchdog-driven auto-revert | Time-boxed authority must not depend on the agent behaving |
| P1: provenance completeness — vouch chain extended to prompt/session identity | Attribution is a thesis pillar and currently partial |
| P1: kernel-side journaling of agent activity | Detection must cover escalation attempts, not just installs |
| **Adversarial milestone** — an agent *instructed* to escalate, persist, and exfiltrate must fail observably; journal attributes it; slot rolls back | The thesis is falsifiable; prove it the way you proved DEMO 80 |

**Maintenance mode (demonstrated, keep alive, stop paying tax):**

- M27 / on-device rustc — load the sysroot from disk properly, then leave it
  alone. It is the feasibility proof, not the product.
- M22 self-rebuild machinery (slots, vouch, trial) — stands as-is; M22a/c/d
  continue as governance work, not self-sufficiency work.

**Deferred (honest deprioritization):**

- M29–M33 information access / browser stack — years to "usable," zero
  contribution to the thesis.
- Phone phases 16–18 — re-evaluate after the adversarial milestone; a paired
  phone is an exfiltration path and should be treated as one in the threat
  model before it is built.
- WiFi resume (M73–M75) — unchanged; same exfiltration caveat as the phone.

---

## What would prove this thesis wrong

- A red-team agent that escalates tiers without a vouch, and the journal
  *doesn't* show it → detection pillar broken.
- A bad install that survives auto-revert → reversibility pillar broken.
- The P0 audit finding that tier authority can't be made derivable without
  rewriting the loader → the architecture's boundary is the wrong shape, and
  the honest move is a capability-system redesign, not more patches.

Until one of those happens, the claim stands: **the machine proposes; the human
disposes; the kernel assumes the machine lied — and the system survives anyway.**
