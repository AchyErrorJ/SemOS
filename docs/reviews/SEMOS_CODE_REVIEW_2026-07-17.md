# SemOS — Code Review

**Repo:** `AchyErrorJ/SemOS` (private) · **Reviewed:** 2026-07-17, tip `03abfc6` (M14-H: vsync-paced present)
**Scope:** full static read of first-party code — `kernel-core` (~26.4k LoC), `kernel-x86_64` (~37.1k), `kernel-aarch64` (~1.3k), `compiler/src`, `user-programs` (excluding vendored `semos-rustc`), plus docs and repo hygiene. No build/run was attempted (nightly-2026-02-01 + QEMU not available in review environment).

---

## 1. Overall assessment

This is a genuinely impressive piece of systems work. A bare-metal x86_64 kernel with preemptive scheduling, Ring 0/3 separation, four storage backends behind one trait, xHCI + EHCI USB, iwlwifi bring-up to the association stage, TLS 1.3 with cert pinning and live HTTPS from metal, an aarch64 port sharing the same `kernel-core`, and on-device `rustc` — that is far past hobby-OS territory. The **architecture is sound**: platform-independent core + thin platform crates is exactly the right split, and it demonstrably works (same sha256 KAT passes on both architectures).

The security *model* — tier-tagged semantic objects, kernel-mediated LLM views, child-can-never-exceed-parent clearance, human-only vouch — is coherent and the gating is genuinely pervasive in the syscall layer. The problem is that **the model is undermined by a handful of pointer-validation holes in that same layer**, two of which are arbitrary kernel read/write primitives callable from Ring 3. Those are the first things to fix, because every higher-level guarantee (tiers, vouch, redaction) sits behind them.

Severity summary:

| # | Finding | Severity |
|---|---------|----------|
| 1 | `SYS_WRITE` → unvalidated user pointer → arbitrary kernel read / kernel panic | **Critical** |
| 2 | `SYS_LLM_CONTEXT` → unvalidated `out_ptr` → arbitrary kernel write | **Critical** |
| 3 | Pointer validation exists but most syscall paths bypass it; no mapping checks; TOCTOU throughout | **High (systemic)** |
| 4 | Redactor tier-inversion bug (Secret requester → full redaction) + fail-open default | **High** |
| 5 | `static mut` shared state + blocking syscalls under a preemptive scheduler | **High** |
| 6 | Pattern redaction is trivially bypassable / over-broad | **Medium** (documented as placeholder) |
| 7 | Non-constant-time secret comparisons (vouch hash, WPA2 MIC) | **Medium** |
| 8 | Layout-dependent stack-overflow corruption worked around, not fixed | **Medium** |
| 9 | 7,159-line `main.rs`; games and driver demos compiled into Ring 0 | **Low/Medium** |
| 10 | Vendored forks (~1.7M LoC) with no update policy; firmware blob without license file | **Low** |

---

## 2. Critical findings

### 2.1 `SYS_WRITE` gives Ring 3 an arbitrary kernel-memory read — and a one-line kernel panic

**Where:** `kernel-core/src/syscall/mod.rs` — `console_write` (407–418), `handle_fwrite` (1257+), `pipe_write_blocking` (437–439), the `FdEntry::Path` branch (1277).

`handle_fwrite` services a user task's `SYS_WRITE`. For the default FD 1 (`FdEntry::Console`) it calls `console_write(buf_ptr, buf_len)`, which does:

```rust
// For kernel-mode callers, skip user validation (ptr may be in kernel space)
let slice = core::slice::from_raw_parts(buf_ptr as *const u8, len);
```

No validation at all — the comment knowingly trades safety for the convenience of kernel-mode callers. The `FdEntry::Path` (1277) and pipe (439) branches do the same `from_raw_parts` on the raw user pointer.

Consequences, from any Ring 3 task — **including a tier-0 sandboxed agent tool**:

- `SYS_WRITE(1, 0xffff_8000_0000_0000, 4096)` prints 4 KiB of kernel memory to the TTY/serial. Kernel addresses are mapped in every task's CR3 (the syscall entry switches stacks but not address spaces), so the read succeeds. Repeat with an offset walk → full kernel memory disclosure: the vouch table, Secret-tier object contents in the registry, TLS key material. The entire tier model collapses.
- Pass a canonical-but-unmapped address → `#PF` while CS ring 0 → `page_fault_handler`/`gp_handler` treats kernel faults as fatal (`interrupts.rs:295-298`, `loop { hlt }`). One syscall → full system halt. Trivial DoS from the least-privileged context in the system.

**Fix:** validate at the *dispatch boundary*, once, for every handler — the kernel-mode callers that need to print kernel buffers should use a separate `console_write_kernel()` that user-reachable paths can never reach. Same for the file/pipe branches.

### 2.2 `SYS_LLM_CONTEXT` gives Ring 3 an arbitrary kernel-memory write

**Where:** `kernel-core/src/syscall/mod.rs`, `handle_llm_context` (2530–2599).

```rust
let suids = unsafe {
    core::slice::from_raw_parts(suid_pairs_ptr as *const (u64, u64), n)  // unvalidated read
};
...
let out = if out_ptr != 0 { out_ptr as *mut u8 } else { core::ptr::null_mut() };
...
core::ptr::copy_nonoverlapping(content.as_ptr(), out.add(offset), entry_len);
```

`out_ptr` is checked only for non-null and the *offset* is capped at 32 KiB — but the base pointer itself is never range-checked. A Ring 3 task passes `out_ptr = <kernel address>` and the kernel copies attacker-influenced content (redacted object bytes + length prefixes) there. Targets: `VOUCH_TABLE` (grant yourself tier 3), the current task's `max_tier` field in the scheduler table (self-elevate), IDT entries. This is full privilege escalation from tier 0, worse than 2.1 because it writes.

**Fix:** `validate_user_ptr(out_ptr, …)` before the loop, and re-derive `out.add(offset)` bounds against `USER_ADDR_LIMIT`, not just `32768`.

### 2.3 The validation that exists isn't used, and what it checks isn't enough

`validate_user_ptr` (364–372) only checks `ptr < USER_ADDR_LIMIT && ptr+len <= USER_ADDR_LIMIT`. Two gaps:

1. **Adoption, not mechanism, is the main bug.** `read_user_slice`/`write_to_user` exist, but the hot paths (write/file/pipe/console, `handle_llm_context`, `handle_vouch`'s path read at 1939–1941, `handle_spawn`'s argument reads) call `from_raw_parts` directly. A syscall-layer audit rule — "no `from_raw_parts` on a syscall argument outside the validated helpers" — would have caught every finding in §2 mechanically. Consider a `UserPtr<T>` newtype that only the validator can unwrap; make raw pointer construction unrepresentable in handlers.
2. **Range ≠ mapped.** The helpers document this ("caller must ensure the memory is actually mapped", 376–378) but nothing ensures it. A canonical, in-range, unmapped pointer still panics the kernel. Options: walk the task's page tables in the validator, or install a #PF recovery path (Linux-style `copy_from_user` with exception-table fixup) so a bad user pointer fails the syscall instead of halting the machine.

**TOCTOU is pervasive** and follows from the same design: user memory is dereferenced directly, so anything checked-then-used can change between check and use by another thread in the same address space (threads + futexes exist). Examples: path strings validated then re-read at spawn; the vouch flow reads the path, resolves, then hashes content. Copy syscall inputs into kernel buffers once, then operate on the copies.

---

## 3. High findings

### 3.1 Redactor: tier-inversion bug and a fail-open default

`kernel-core/src/llm/context_redact.rs`, `determine_redaction_profile` (129–145):

```rust
PolicyResult::Allow(_) => match context.requester_tier {
    SecurityTier::Secret    => RedactionProfile::Minimal,   // ← bug
    SecurityTier::Sensitive => RedactionProfile::Standard,
    ...
```

`RedactionProfile::Minimal` is implemented as *maximal* redaction — it replaces everything with `"[CONTENT REDACTED - INSUFFICIENT PRIVILEGE]"` (208–214). So the highest-clearance requester gets a fully blanked document, while lower tiers get usable, pattern-redacted text. Either the mapping is inverted (Secret should get the lightest touch) or the profile name is a trap. Given the comment "No specific redaction required," the intent was clearly light redaction for Secret — this is a logic bug, not just naming.

Two more issues in the same function:

- `_ => self.default_profile` (144): any *unrecognized* policy result falls back to `Standard` — fail-open. Unknown policy outcomes should deny/redact-maximally.
- `PolicyResult::Deny => RedactionProfile::Minimal` is the one correct use of the name, which is further evidence the name is the problem: rename to `Full`/`Blank` and add `RedactionProfile::None` (or `Passthrough`) for trusted requesters.

### 3.2 `static mut` shared state + blocking syscalls + preemption

The syscall layer leans on global mutable singletons: `CONTEXT_SCRATCH` (2540, justified by "syscalls are serialized"), `VOUCH_TABLE` (1909), `GLOBAL_CONTEXT_REDACTOR` (404), the global semantic registry, `global_policy_engine`. Two problems:

1. **The serialization assumption is not obviously true.** Syscall handling can block — `pipe_write_blocking` literally parks the task (`BlockReason::PipeWrite`). While parked, the scheduler runs another task, which can enter its own syscall and touch the same `CONTEXT_SCRATCH` / registry. That's a data race through `&'static mut` — UB in Rust's model, and a correctness bug (interleaved LLM-context output) even if it never crashes.
2. `addr_of!(VOUCH_TABLE)`/`addr_of_mut!` avoids the *lint*, but handing out `&'static mut` from `global_registry()` / `global_context_redactor()` to multiple call sites still aliases.

**Fix:** a `SpinLock<T>`/`Mutex<T>` wrapper around each singleton (there's already `sync::Mutex` in semos-std — a kernel equivalent must exist), or per-CPU/per-task scratch. At minimum, audit every "syscalls are serialized" claim against every blocking point in the syscall layer.

### 3.3 (Documented) — pattern redaction is a demo, not a boundary

You're upfront that redaction is rule-based pending real inference, so treat this as hardening notes rather than a gotcha:

- **Case-sensitive prefixes:** `MRN`, `ACCT`, `PATIENT` only match uppercase (`context_redact.rs:304,324,334`). `mrn123456` sails through. The email/SSN/card matchers in `redact.rs` are similarly literal.
- **Separators:** card matcher accepts only `-`/space between digits; `.` or mixed separators break it. No Luhn check → high false-positive rate on any 13+ digit run (order IDs, timestamps), and the 7–15-digit phone matcher overlaps the card matcher.
- **Name redactor** flags *any* capitalized word of 2–20 chars (`find_name_pattern`) — over-redacts ordinary prose (`The` → `[NAME]`) while missing lowercase handles.
- **Chunk boundaries:** redaction runs per-buffer (`CONTEXT_SCRATCH` is 4 KiB, `MAX_ENTRY_SIZE` temp buffers in the medical/financial paths). PII split across two entries — or across the 4096 boundary — won't match either half. A real implementation needs whole-object passes (or a sliding overlap window) before redaction.
- Redaction *shortens* content, and `handle_llm_context` writes `(len, bytes)` records — fine — but if any consumer ever redacts in place or length-preservingly, leftover bytes leak. Worth a test.

---

## 4. Medium findings

**4.1 Non-constant-time secret comparisons.** The TLS record-tag compare is constant-time (`crypto_shim.rs:205`, `subtle::ConstantTimeEq` — good), but the same discipline isn't applied elsewhere: the vouch SHA-256 recheck (`syscall/mod.rs:1855`) uses `==` on `[u8; 32]`, and the WPA2 EAPOL MIC check (`wireless/wpa2.rs:228`) compares the MIC with `==`. The vouch case is low-risk (both values are kernel-internal), but the MIC is attacker-influenced bytes compared against a derived secret — timing oracles there are a real class. One `ct_eq` helper used everywhere closes the category.

**4.2 The stack-overflow heisenbug is worked around, not fixed.** The comment at `scheduler/mod.rs:35-55` describes overflow of a 16 KiB task stack writing into the *previous slot's* iret frame and corrupting a context-switch return address — surfacing as a stuck-bit `#GP`. The mitigation (64 KiB stacks + canaries) reduces probability; it doesn't remove the mechanism, and the comment itself notes the failure is *layout-dependent* — any future code change can shift frames again. Real fixes: guard pages (noted as "future work"), `-Zstack-probes`/probe strokes on large frames, and a CI check that fails on stack-frame sizes above a threshold (`-Wframe-larger-than`).

**4.3 Kernel TCB bloat.** `main.rs` is 7,159 lines and hosts init, every driver demo, the interactive session, and task bodies; `tetris.rs` (641) and `pong.rs` (580) are compiled into Ring 0. Every line in Ring 0 is attack surface behind the findings of §2. The demos are your regression harness, so they earn their place during bring-up — but games and one-shot driver demos belong in `user-programs/` behind the same syscall boundary as everything else, and `main.rs` wants splitting into `init/`, `demo/`, `session/` modules.

**4.4 Vendored forks — briefs exist, but don't cover the two biggest trees.** *(Corrected post-publication: the review initially claimed no vendoring docs; that was wrong.)* `docs/EMBEDDED_TLS_VENDORING_BRIEF.md` and `docs/SMOLTCP_VENDORING_BRIEF.md` record upstream state for the two most security-sensitive forks — exactly the right practice. The gap is coverage: `compiler/vendor/` (Cranelift 0.122 et al., ~550k LoC) and `user-programs/semos-rustc` (~1.13M LoC) have no equivalent briefs, and there's no single index tying the briefs together. Extend the existing brief pattern to the two big trees and add a one-line index in `docs/`; the habit is already there, it just needs to be uniform.

---

## 5. Repo hygiene (low)

- `iwlwifi-7260-17.ucode` (1 MB Intel firmware) is committed **without the Intel license file** that redistribution terms require alongside it. Add the license text next to the blob (or fetch-at-build).
- Committed debris: `.claude-cg-5b-baseline.log`, `gdb-watch.log`, `.claude/`, `debug.gdb` / `debug_pf.gdb` / `watch.gdb` at root. The `.gitignore` is otherwise solid (`.anthropic-key`, `*.img`, serial logs all covered) — extend it to `*.log` at root or move these under `docs/`.
- Secret scan came back clean — the `ANTHROPIC_KEY` env-var flow is honored, no keys in tree. 
- Build requires a sibling-repo layout for nothing here (unlike LegibleStudios) — SemOS is self-contained, good. The pinned nightly is documented; consider a `rust-toolchain.toml` comment noting *what* breaks on other nightlies beyond "bootloader-0.11 requires it."

---

## 6. What's genuinely good — keep doing it

- **The vouch design is right.** Authority-task gating (the agent structurally cannot call `SYS_VOUCH` because `VOUCH_AUTHORITY_TASK` never matches its spawn chain), clearance ceiling (`grant > caller → deny`), SHA-256 content binding rechecked at spawn (closes bait-and-switch), RAM-only grants that reset every boot, and a read-only audit list. That is a thoughtful capability design — it just needs §2's pointer holes closed so nothing routes around it.
- **Tier gating really is pervasive.** Every semantic-object syscall (`SEM_READ/WRITE/LIST/QUERY/SEARCH…`) re-derives `current_task_max_tier()` and clamps; spawn caps `spawn_tier ≤ caller_tier`; `SYS_LLM_CONTEXT` excludes tier-3 objects entirely. The policy isn't declared once and forgotten.
- **Exception hygiene:** user-mode `#PF`/`#GP` kills the faulting task instead of the kernel (`interrupts.rs:287-294`). Extend the same idea to syscall-time faults (§2.3).
- **The demo suite as a regression harness** (~69 demos, ~165 PASS assertions, hardware-dependent demos self-skip), the boot **build stamp**, panic-dump-to-disk with a PowerShell recovery tool, and hardware oracle captures — this is a mature bring-up methodology most hobby kernels never get to.
- **Honest docs.** The scheduler comment narrating the 16 KiB stack bug, the "hardware-gated, not QEMU-testable" WiFi status, the roadmap's gated/active split — this makes the codebase far easier to trust and to review.
- `kernel-core` / platform split paying off already in the aarch64 port; `include_bytes!`-embedded user programs keeping the whole OS one image; the UEFI-dual-boot dev loop (seconds per iteration).

---

## 7. Suggested fix order

1. **P0 — close the two primitives.** Validate `out_ptr`/`suid_pairs_ptr` in `handle_llm_context`; route all user writes through validated copies; split `console_write` into user/kernel variants. Add a regression demo: tier-0 task attempts `SYS_WRITE` of a kernel address → must get `u64::MAX`, and the machine must survive.
2. **P0 — syscall-layer pointer audit.** Grep every `from_raw_parts` in `kernel-core/src/syscall/`; each must trace to a validated source. Consider the `UserPtr<T>` newtype so this stays true by construction.
3. **P1 — kill the serialization assumption.** Lock or per-task the scratch/registry/redactor globals; then re-examine every blocking syscall under preemption.
4. **P1 — fix the redactor inversion + rename `Minimal` → `Full`;** make unknown policy results deny; add unit tests pinning each tier → expected profile.
5. **P2 — copy-in/copy-out for all syscall buffers** (fixes TOCTOU as a class); constant-time compare helper everywhere; guard pages + frame-size CI for the stack bug.
6. **P2 — extend the vendoring-brief pattern** (already established for embedded-tls and smoltcp) to `compiler/vendor` and `semos-rustc`, plus a one-line index; add the Intel firmware license file.
7. **P3 — split `main.rs`,** move tetris/pong/driver demos to user programs as the syscall surface hardens.

---

*Review produced by static analysis only; line numbers reference commit `03abfc6`. Happy to go deeper on any subsystem (iwlwifi TX path, xHCI, the ELF loader, TLS shim) in a follow-up.*
