# SemOS Self-Dev Loop: Final Push Plan

**Date:** 2026-08-17  
**Status:** Session vouch mechanism designed, implementation pending  
**Goal:** Agent writes code → compiles on-device → spawns → verifies, end-to-end, with human approval gates on actions that matter.

---

## 1. The Security Model: Session Vouch + Per-Action Approval

### 1.1 Session Vouch (`SYS_VOUCH`)

At shell start or on explicit request, the human grants a **session ceiling** — the maximum tier the agent can request during this session.

```
sem-sh$ vouch --tier 2 --duration 8h
[Human confirms via physical key / password / YubiKey / etc.]
Session elevated. Ceiling: tier 2 (Sensitive). Expires: 05:09 tomorrow.
```

- **Tier 0 (Public):** Default. Agent can read Public objects, compile code, write to scratch.
- **Tier 1 (Internal):** Vouched. Agent can read Internal objects, write to `/tmp`, spawn non-persistent processes.
- **Tier 2 (Sensitive):** Vouched. Agent can read/write Sensitive objects (mic capture, user documents), write to `/apps/`, spawn persistent daemons.
- **Tier 3 (Secret):** Vouched + additional auth. Kernel policy, crypto keys, raw disk access.

### 1.2 Per-Action Approval

The session vouch unlocks the *ceiling*. The agent must still request approval for each significant action. Think `sudo -k` inverted: the door is open, but you still knock for each room.

| Action | Auto-approve in vouched session? | Approval UI |
|--------|-------------------------------|-------------|
| Compile `.rs` → ELF (no spawn) | ✅ Yes | Silent |
| Read any tier ≤ session ceiling | ✅ Yes | Silent |
| Write to `/tmp/`, `/var/tmp/`, scratch | ✅ Yes | Silent |
| Write to `/apps/<name>` (persistent install) | ❌ Ask | "Install `/apps/five-daemon`? [y/N/details]" |
| `SYS_SPAWN` / `SYS_EXEC` (run compiled code) | ❌ Ask | "Spawn `/apps/five-daemon`? [y/N/once/always]" |
| `SYS_LLM_*` with network egress | ❌ Ask | "Allow network call to `api.anthropic.com`? [y/N]" |
| `SYS_PERSIST` / `SYS_RESTORE` (cross-boot state) | ❌ Ask | "Persist snapshot to disk? [y/N]" |
| `SYS_VOUCH` (request elevation) | ❌ Ask + human auth | "Request tier 3 vouch? [y/N]" |

### 1.3 Userland Model: Oberon-Style Module Loading

**Keep userland. Make it thinner if you want, but don't rip it out.**

Without userland, the self-dev loop becomes a loaded gun. Agent writes code → runs it in Ring 0 → no isolation → one bad pointer and your filesystem, display driver, and LLM redaction policy are all in the same memory space.

The tier model works *because* there's a boundary. Tier 0 (agent) → Ring 3 (userland) is the sandbox. Tier 3 (you, vouched) → Ring 0 is where policy lives.

**Consider Oberon-style module loading within userland:** Single address space for user programs, but still Ring 3. Agent compiles a module, the `agent` TUI loads it into its own address space, runs it, unloads it. No `SYS_SPAWN` process isolation overhead, but pagetable protection from the kernel remains.

The T540p boots with 17 ELFs in the image today. `semos-rustc` already emits Ring-3 binaries that run. Don't throw away working isolation to save a context switch.

**Rule:** Keep the wall. Give the agent a bigger room inside it.

### 1.4 Audit Trail

Every vouch grant, every approval, every denial is logged:

```
[2026-08-17T21:10:00Z] VOUCH session tier=2 duration=8h by=user tty=/dev/ttyS0
[2026-08-17T21:15:32Z] APPROVE spawn /apps/five-daemon by=agent tier=2 reason=session_vouch
[2026-08-17T21:20:01Z] DENY write /apps/kernel_module.rs by=agent tier=2 reason=path_in_kernel_space
```

---

## 2. The Self-Dev Loop Architecture

### 2.1 Goal State

```
┌─────────────┐    write      ┌──────────────┐    compile     ┌─────────────┐
│   Agent     │ ────────────> │   Scratch    │ ─────────────> │    ELF      │
│  (tier 0)   │   Rust code   │   (/tmp)     │  semos-rustc   │  (/tmp)     │
└─────────────┘               └──────────────┘                └──────┬──────┘
       ^                                                             │
       │                                                             │ spawn
       │                                                             ▼
       │                                                    ┌─────────────┐
       │    verify output / logs / test results              │   Process   │
       │ <────────────────────────────────────────────────── │   (Ring 3)  │
       │                                                     └─────────────┘
       │
       │ install? (human approves)
       ▼
┌─────────────┐
│  /apps/<name> │  ← persistent, survives reboot
└─────────────┘
```

### 2.2 Flow Detail

**Step 1 — Write**
- Agent generates Rust source for a module/feature/fix.
- Writes to `/tmp/agentgen/<uuid>/src/main.rs`.
- Agent can also write supporting files (`Cargo.toml`, submodules).
- **Tier:** 0 (Public) for scratch writes. No approval needed.

**Step 2 — Compile**
- Agent invokes `semos-rustc` on the scratch source.
- Output: `/tmp/agentgen/<uuid>/out/main` (static ELF).
- **Tier:** 0 (Public). Compilation is pure compute, no side effects.
- **Failure mode:** If compile fails, agent reads stderr, fixes code, recompiles. Loop tightens.

**Step 3 — Request Spawn (Human Approval)**
- Agent: "I want to spawn `/tmp/agentgen/<uuid>/out/main` to test."
- System prompts human: "Spawn `/tmp/agentgen/abc123/out/main`? [y/N/once/always/view]"
- Human can view source before approving.
- **Tier:** min(requested, session_ceiling). If session is tier 2, spawn runs at tier 2.

**Step 4 — Run & Observe**
- Spawned process runs in Ring 3, isolated address space.
- Agent observes: stdout/stderr (via pipe), return code, `SYS_SYSINFO`, or custom test protocol.
- If process panics or misbehaves, agent analyzes and iterates.

**Step 5 — Install (Human Approval)**
- If tests pass, agent: "Install to `/apps/five-daemon`?"
- System prompts: "Copy `/tmp/agentgen/abc123/out/main` → `/apps/five-daemon`? [y/N]"
- On approve: atomic move (write to `/apps/.staging/`, rename).
- **Tier:** 2 (Sensitive) required for `/apps/` writes.

**Step 6 — Persist (Optional, Human Approval)**
- Agent: "Persist system state for next boot?"
- Human approves `SYS_PERSIST`.
- Snapshot saved to disk. Survives reboot.

---

## 3. Implementation Checklist

### 3.1 Session Vouch

- [ ] Implement `SYS_VOUCH(tier, duration)` syscall
  - [ ] Validate human auth (password, key, etc.)
  - [ ] Store `(tier, expiry, auth_method)` in process control block
  - [ ] Expose via `SYS_GET_VOUCH()` for agents to query their ceiling
- [ ] Add `vouch` command to `sem-sh`
- [ ] Auto-downgrade on session expiry or reboot
- [ ] Prevent vouch escalation without re-auth (tier 3 requires fresh vouch)

### 3.2 Per-Action Approval

- [ ] Build approval gate into `SYS_SPAWN`, `SYS_WRITE` (for protected paths), `SYS_LLM_*` (network), `SYS_PERSIST`
  - [ ] Check: does action require approval per policy table?
  - [ ] If yes: block syscall, notify agent, surface prompt to human
  - [ ] Human response: approve (once / always / deny / view details)
  - [ ] Resume syscall with approval token or return EPERM
- [ ] Design TUI prompt for `sem-sh` / `agent` UI
  - [ ] Non-blocking: agent continues, approval is async
  - [ ] Visual indicator in prompt: `[V2]` for tier-2 vouched session

### 3.3 Self-Dev Loop Wiring

- [ ] Agent code generation prompt/templates
  - [ ] "Write a Rust module that does X, compatible with semos-std"
  - [ ] Auto-generate `Cargo.toml` with correct dependencies
- [ ] Scratch workspace management
  - [ ] `/tmp/agentgen/` directory, auto-cleanup on reboot
  - [ ] Git-like history: each generation is a commit, agent can diff/rollback
- [ ] Compile-oracle
  - [ ] Agent calls `semos-rustc` via `SYS_SPAWN`, captures output
  - [ ] Parse stderr, map errors to source lines, suggest fixes
- [ ] Test harness
  - [ ] Agent writes tests alongside code
  - [ ] Auto-run on spawn, verify exit code / output
- [ ] Install pipeline
  - [ ] Atomic: write to staging, verify hash, rename into `/apps/`
  - [ ] Prevent overwrite of critical system binaries without tier 3

### 3.4 Dogfooding Milestones

| Milestone | Definition of Done |
|-----------|-------------------|
| M1: Hello Loop | ✅ DONE 2026-08-19 (QEMU): `demo80_autocompile` compiles `/hello.rs` → runs the ELF fenced at tier 0 via `sem-sh -c` → verifies captured stdout byte-for-byte. Human-approve-on-spawn still open (spawn fence covers it for now) |
| M2: Bug Fix | ✅ DONE 2026-08-19 (QEMU): `demo83_bugfix` seeds a bug report + buggy `calc.rs` in `/tmp/agentgen/m2/`, reproduces the failing selftest, writes the fix, recompiles, verifies byte-exact PASS, prompts `Install /apps/calc? [y/N]` on serial (fail-fast: deny on n/timeout), installs via `/apps/.staging` rename, and smoke-runs it by bare name fenced at tier 0. Both approve and deny runs verified |
| M3: Feature Add | ✅ DONE 2026-08-21 (QEMU): `demo87_featureadd` seeds a feature spec + `wc.rs` + sample data in `/tmp/agentgen/m3/`, compiles with the on-device rustc, tests in isolation (byte-exact `3 15 79` — first guest file read via new `sys_open`/`sys_fread`/`sys_close` aot_semos stubs), prompts `Install /apps/wc? [y/N]` (fail-fast), installs via `/apps/.staging`, smoke-runs bare `wc` fenced at tier 0. Both approve and deny runs verified. Required two real bug fixes: syscall_entry clobbered callee-saved `r15` on every syscall (user RSP stashed in r15 before it was saved), and the `sys_fread` stub was 7 bytes (missing imm32 zero) |
| M4: Self-Repair | Agent detects its own failure (panic log), writes patch, compiles, installs — minimal human intervention |

---

## 4. Decisions (Made August 17, 2026)

1. **Human auth method for vouch:** T540p fingerprint sensor **or** password typed into `sem-sh`. Fingerprint preferred when available; password fallback.
2. **Approval latency:** **Fail fast.** If human is AFK, agent receives `EPERM` or equivalent immediately. No queues, no waiting. Agent can retry or escalate to human via other channel.
3. **Tier of compiled code:** **Inherits agent's tier at compile time.** An ELF compiled by a tier-0 agent is tier-0. If the agent is vouched to tier 2, compiled ELFs are tier 2. Installation to `/apps/` still requires human approval regardless.
4. **Rollback:** **Boot from known-good snapshot.** `SYS_PERSIST` creates snapshots; `SYS_RESTORE` or boot-time menu selects a previous snapshot. Agent does not auto-rollback — human chooses.
5. **Network policy:** **API keys baked into tier-3 storage, but agent requests per-call.** Keys exist in kernel space; agent must request `SYS_LLM_QUERY` etc., which triggers the per-call approval gate if network egress is involved. No ambient access.

---

## 5. Deferred (Post-Loop)

- USB audio / UAC driver / isochronous transfers
- Whisper / Five daemon / wake-word detection
- ARM port / Apple Silicon boot
- Display driver (Haswell modeset)
- Netlog boot-test

**Rule:** Nothing above enters the kernel until the self-dev loop is dogfooded for one real bug fix.

---

*Compiled for the final push. The loop is the product.*
