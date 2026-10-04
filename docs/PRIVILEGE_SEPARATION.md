# Privilege Separation — making the uid layer real

**Date:** 2026-10-04
**Status:** DESIGN LANDED, IMPLEMENTATION NOT STARTED. The working session was
interrupted before any code was written. This document is the handoff: design
decisions are firm, file pointers are verified against main @ `51aeab3`.
Root motivation: the governance thesis ([`GOVERNANCE_THESIS.md`](GOVERNANCE_THESIS.md))
— the uid axis of authority is currently decorative because every task runs as
SYSTEM. Recorded as the open decision in the
[review appendix](reviews/SEMOS_CODE_REVIEW_2026-07-17.md).

---

## The problem

- `scheduler::alloc_task_slot` (kernel-core/src/scheduler/mod.rs:348-351) gives
  every spawned task the **spawner's uid**. The bootstrap task is SYSTEM
  (uid 0, scheduler/mod.rs:331) and nothing ever drops privilege, so on a
  normal boot *every* task — shell, user programs, tier-0 agent tools — is uid 0.
- Consequence: every `requester_id == SYSTEM`/`ADMIN` check in the syscall layer
  is inert. A tier-0 agent tool can call `SYS_LLM_SET_POLICY` and install a
  system-wide policy (the `862a2af` gating is shape-correct but matches
  everything). The tiers gate *content clearance*; the uid layer is supposed to
  gate *authority*, and it currently gates nothing.

## The model

Three principals, mirroring the SYS_VOUCH "designated authority" idea
(privilege comes from *how you were spawned*, not from what you ask for):

| uid | name | who |
|---|---|---|
| 0 | SYSTEM | kernel, in-kernel demos (direct dispatch) |
| 1 | ADMIN | the human's interactive seat: sem-sh console session |
| 254 | GUEST | everything spawned by unprivileged code |

## Required changes (in order)

1. **Ring-3 spawn defaults to GUEST.** In the user-reachable spawn path
   (`handle_spawn` → process/mod.rs:779/818 `alloc_task_slot` call sites, or
   the exec flow), a Ring-3 spawner's children get GUEST unless the spawner
   is ADMIN/SYSTEM. Kernel-side spawns keep explicit control via
   `alloc_task_slot_with_user` (scheduler/mod.rs:357) — do **not** change
   global default behavior, only the Ring-3-reachable path.
2. **The interactive console is ADMIN.** Pin sem-sh's uid at spawn in
   kernel-x86_64/src/main.rs (it currently inherits SYSTEM). Kernel init and
   demos stay SYSTEM.
3. **SYS_SETUID rules made explicit.** `security::users::can_setuid_to` must
   encode: GUEST cannot setuid; ADMIN may setuid to GUEST or itself, never
   SYSTEM; only SYSTEM confers SYSTEM. Verify current rules against this and
   fix + document the rule table in the function's doc comment.
4. **Fallout audit.** Grep `current_user_id()` gates; the policy
   install/read paths (handle_llm_set_policy ~mod.rs:3397, policy read
   ~3508) become live: GUEST requesters rejected from system/app policy
   ranges, confined to their own user-policy namespace — that is the intended
   behavior, not a bug. Check file/object ownership checks too.
5. **Demos.** DEMO 8 (policy syscalls, legacy_demos.rs:4984+) and DEMO 11
   (user_identity_test, legacy_demos.rs:1530+) assumed everything-is-SYSTEM.
   In-kernel demos dispatch directly and stay SYSTEM — prefer adjusting demo
   *setup* (e.g. setuid to ADMIN where a real console session would) over
   weakening kernel checks.
6. **Regression coverage.** Unit tests for the setuid rule table. Metal/QEMU
   acceptance: `ps`-style check that the shell is ADMIN and a spawned program
   is GUEST; a GUEST program must be refused system-policy install. The
   ptr-guard-test pattern (user-programs/ptr-guard-test, registered in
   kernel-x86_64/src/main.rs:524-525/655-658) is the model for the syscall
   regression harness.

## Guardrails

- No weakening of any gate to make a demo or the agent harness pass. If
  pinning the console to ADMIN breaks a systemic assumption, stop and
  reassess the design rather than papering over it.
- Tiers and uids are orthogonal axes; this change must not touch tier logic.
- Verify: `cargo test -p kernel-core` (125 tests must stay green), one
  `cargo build --release -p kernel-x86_64` (83 pre-existing warnings), QEMU
  boot if feasible.

## Done-when

Review appendix entry flips from "open design decision" to resolved; a
GUEST-context policy-install attempt fails in the regression harness; the
console runs as ADMIN on metal.
