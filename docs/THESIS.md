# SemOS Governance Thesis — Agent-Authored Code Under Checkable Intent

**Date:** 2026-09-04
**Status:** Working architectural document. Consolidates `semos-security-thesis.md`
(the security claim) and `provenance-commitment.md` (the provenance claim) into a
single enforceable contract for the phase the project entered when on-device
compilation landed: **the OS is coded by agents; the human governs.**
Supersedes nothing; binds everything.

---

## 1. The claim

> An agent can write SemOS because the human never has to trust the agent's
> account of what it wrote. Intent lives in machine-checkable artifacts —
> demos, invariants, hashes — and approval binds to those artifacts, never to
> the agent's narration of them.

The security thesis says the system is small enough for one person to audit.
The provenance thesis says every artifact can name its origin. This document
adds the third leg the self-dev loop makes necessary: **the governance thesis** —
every change to the system is approved against evidence the agent cannot fake.

The three are one claim pointed at three directions: security governs what may
run, provenance records what made each artifact, governance decides what may
change. Remove any leg and the other two collapse into rhetoric.

---

## 2. The root assumption

**The LLM will not always tell the truth about what its code does.** Not because
it is malicious — because it is a generator. It can be wrong, overconfident,
prompt-injected through a doc it read, or simply describing the code it meant to
write rather than the code it wrote. A governance model that assumes truthful
self-report is a governance model built on sand.

Therefore: **agent narration is untrusted input, everywhere.** It may inform the
human; it may never *be* the evidence. Every approval surface in the system —
kernel gates, code review, release decisions — shows kernel-verified or
tool-verified facts: the literal bytes, the literal diff, the hash, the demo
verdict. Where an agent's description appears at all, it is labeled as
unverified annotation.

---

## 3. Security invariants

Numbered, each with its enforcement mechanism and the test that proves it.
A red invariant rejects the change automatically — no human time is spent.
New invariants are appended (never renumbered) with their test in the same
commit.

- **I-1 — Every syscall is tier-gated.** No operation reaches dispatch without a
  `current_task_max_tier()` (or stricter) check appropriate to its blast radius.
  *Enforced by:* the pervasive tier gates in `kernel-core/src/syscall/`.
  *Proven by:* surface audit against `docs/KERNEL_SURFACE.md`; every syscall
  entry in the inventory names its gate.

- **I-2 — Privilege never grows across spawn.** `spawn_tier = max_tier.min(
  caller_tier)`: a child can never exceed its spawner's clearance. *Proven by:*
  `spawn-demo` and the spawn-path unit checks; any change to the spawn path
  ships a demo showing a tier-0 parent cannot produce a tier-1 child.

- **I-3 — Console authority is task identity, not request content.**
  `is_vouch_authority()` keys on the atomics + current task index of the
  interactive console. `SYS_VOUCH`, `SYS_VOUCH_SESSION`, `SYS_SELFDEV`,
  `SYS_PAIR`/`SYS_UNPAIR` are unreachable by the agent no matter what it asks
  or how it asks. *Proven by:* the `vouch-test` feeder flow (grant →
  wrong-password denial → expiry → re-grant) and an adversarial demo invoking
  the gated syscalls from a non-console task, expecting `DENIED`.

- **I-4 — The kernel never reads caller memory unvalidated.** Every pointer
  crossing the syscall boundary goes through the pointer guard
  (`caller_slice`/`read_caller_str` and kin). *Proven by:* `ptr-guard-test`
  (P0 regression) on every kernel change.

- **I-5 — Agent-authored executables default to tier 0.** Namespace
  executables run at the vouch ceiling; fresh compiler output is untrusted
  until vouched, and even vouched runs capped below the caller's tier, until
  reboot. *Proven by:* `exfil-demo` (an adversarial Ring-3 program that
  attempts tier escape and must fail) plus DEMO 80's compile→spawn→verify
  loop running at the default ceiling.

- **I-6 — Approval gates fail fast.** Timeout denies. Ctrl+C denies. The gate
  drains pre-committed input before asking, so nothing typed earlier can
  answer later. *Enforced by:* `demo_approval_prompt` semantics.
  *Proven by:* DEMO 83/87/88 harness runs, including the deny paths.

- **I-7 — A session vouch is a window, not an approval.** `SYS_VOUCH_SESSION`
  raises the exec *floor* for a bounded time and tier. It approves nothing.
  Agent-authored code that matters is vouched per-artifact (`SYS_VOUCH` on the
  bytes) or approved at a gate showing ground truth. The session grant is
  password-held (first call sets, constant-time compare after), capped by the
  caller's clearance, TTL'd, revocable with duration 0, and auditable by any
  task via `SYS_GET_VOUCH`/`SYS_VOUCHES`. *Proven by:* `vouch-test`.

- **I-8 — Provenance is write-once, read-only after.** The field that says
  what produced an artifact is written at creation and has no update path —
  the operation that would let an artifact lie about its origin does not
  exist. *Per:* `provenance-commitment.md`.

- **I-9 — Firmware is declared trust, inventoried and bounded.** Every blob
  the kernel loads has a surface-inventory entry (hash, version, bus access,
  IOMMU status, blast radius). *Per:* security thesis commitment 6.

- **I-10 — Serial is output-only unless a feature explicitly arms input.**
  Headless command injection happens only through kernel feeder tasks compiled
  under an off-by-default feature flag (`vouch-test`, `selfdev80-test`). A
  default build takes input from the physical keyboard alone. *Proven by:*
  the default-build boot log showing no feeder spawn.

---

## 4. Provenance rules

- **P-1 — Every artifact that outlives its session names its origin:** kernel
  build (tag), trust state, and producing component. Boot banner, panic dumps,
  sysroot blobs, vouched binaries, persisted snapshots.

- **P-2 — Agent-authored artifacts carry extended provenance:** agent identity,
  model identifier, the change-request reference, and the evidence bundle
  (demo logs) that backed approval. When the on-device pipeline matures, this
  lands as a provenance header written by the *installer*, not by the agent.

- **P-3 — Approval binds to the hash.** The gate displays the artifact (or its
  canonical digest with the full text one keystroke away); approval records
  the digest; installation verifies the installed bytes equal the approved
  digest. The human approves *these bytes*, and only these bytes install.

---

## 5. Governance rules (how changes land)

- **G-1 — Agents work on feature branches only.** Never directly on main.
  The human reviews a diff, not a codebase.

- **G-2 — No demo, no review.** Every change ships a demo or test that fails
  without it, in the DEMO-numbered style the project already runs. The
  headless QEMU harness (feeder-typed commands, serial verdict,
  `[DEMO n] PASS|FAIL`) is the CI primitive.

- **G-3 — Green before human.** CI runs the full demo suite plus the new demo
  on every proposal. The human reviews only green proposals: diff + evidence
  bundle. A red invariant (§3) is an automatic reject.

- **G-4 — Two-key rule on the trusted core.** Changes to the syscall table,
  the vouch/authority machinery, the pointer guard, the sysroot trust path,
  or the approval gate require explicit human sign-off *even when green* —
  enforced with CODEOWNERS. Agents build the world; they do not quietly move
  the foundation.

- **G-5 — Docs travel with the diff.** A change that contradicts a design
  document must update the document in the same commit. Intent drift becomes
  visible in the diff instead of accumulating silently.

- **G-6 — The agent never writes the approval prompt.** Approval text is
  generated from verified facts (paths, hashes, diffs). Agent-supplied
  summaries, if shown, are marked unverified.

- **G-7 — Pushes to the shared remote happen only on explicit human
  sign-off.** Review artifact first, push second. (This session's practice —
  branch, diff export, review — is the standing model.)

- **G-8 — No forks for governance.** Personal exploration happens on
  long-lived branches. A fork fragments the evidence trail this document
  depends on.

---

## 6. Known gaps (honest list, tracked)

- **task_exit_stub wedges the machine.** A kernel task that returns hlt-loops
  without marking its slot `Exited`; observed (2026-09-04, QEMU) to stop timer
  interrupts entirely. This currently blocks the `vouch-test` feeder from
  completing, which blocks the standing proof of I-3/I-7. Fix: mark the slot
  exited in the stub. Until then, feeder tasks park in a sleep loop.
- **Per-session capability tokens** (security thesis commitment 5) are
  designed, not built. The session vouch is the interim mechanism; G-6/P-3
  are the compensating rules.
- **The approval gate currently prints fixed kernel strings.** Before
  agent-authored installs go live, the gate must migrate to P-3 hash binding.
- **The finer per-syscall capability set** (CapSet) remains a later
  refinement; tier granularity is the accepted interim risk.

---

## 7. What this document does not claim

- Not that agents write correct code. That agents' code is *admitted*
  correctly.
- Not that evidence replaces judgment. The human still decides; the system
  guarantees the decision is about reality.
- Not that this is solved. §6 is the honest edge, and it gets shorter only
  by landing the work, never by editing the list.
