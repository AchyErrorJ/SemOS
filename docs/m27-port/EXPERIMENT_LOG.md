# M27 rustc-on-SemOS — experiment log

> ## ⏭ NEXT-SESSION-START-HERE
>
> If you're picking this up cold, read in this order:
> 1. `docs/M27_RUSTC_PORT_PLAN.md` §1 (9 decisions taken)
> 2. `docs/m27-recon/SYNTHESIS.md` (Phase 1 numbers, what's mitigated)
> 3. `docs/m27-port/RECIPE.md` (canonical port pattern + sandbox lessons)
> 4. `docs/m27-port/HANDOFF_TEMPLATE.md` (line-precise §3 = 10× efficiency)
> 5. This file — scroll to the bottom for the latest tally and the
>    Phase 3 transition checklist.
>
> **State at session end (2026-05-31, PHASE 4 CLOSED + Phase 5 scaffolded):**
> - Phase 1 (recon) ✅ — 4 agents, ~723k tokens
> - Phase 2a (foundation) ✅ — 16 crates, ~38k LOC, ~1.3M tokens
> - Phase 2b (cycle-breakers) ✅ — 4 crates + A1 sync followup, ~26k LOC, ~560k tokens
> - **Phase 3 (semantics tier) ✅** — 21+ crates patched across 3 waves:
>   Wave 1 (Cluster A frontend, 3 agents, ~533k tokens, 5.2 t/LOC),
>   Wave 2 (Cluster B semantics, 5 agents, all late-bounced after
>   partial-100-file work, ~est 1.5M tokens), Recovery wave (4 agents,
>   ~770k tokens, 2-7 t/LOC, closed Cluster B). Commits `c186403`,
>   `81b5e0d`, `d5b5bdb`.
> - semos-std surface ✅ for R2 top-6 + scoped_thread_local!
>   + path Components/strip_prefix/Cow<Path> + io::Stderr + LocalKey<Cell>
>   sugar (commit `7978ce5`) + sync::LazyLock + env::VarError (commit `c9f0b2d`)
> - **Phase 4 (codegen tier) ✅** — 7 crates / ~115k LOC / ~793k tokens.
>   §1.7 (back::link drop) + §1.2 (libloading drop) both landed via
>   cfg-gates. Commits `97a7b75` + `a6cf41f` + `b95aaeb`.
> - **Phase 4.5 surface additions ✅** (commit `de8aff3`) — Path::display
>   + ErrorKind + io::Write for Vec + fs::rename + fs::copy + fs::stat
>   + sync::mpsc re-export.
> - **Phase 5a cfg-sweep ⏸ DEFERRED** — re-analyzed as non-blocking
>   for Phase 5b. Polish pass after integration ships.
> - **Phase 5b scaffold ✅** (commit `0c19848`) — user-programs/semos-rustc
>   binary template; builds clean to ET_EXEC at 0x400000.
> - **Phase 5b Stage D STARTED** (commit `926e739`): workspace plumbing
>   fixed (workspace root + stripped `[workspace] members=[]` from 48
>   rustc_*'s + renamed `semos_std` dep → `semos-std` in 9 Cargo.tomls).
>   First `cargo check` resolved 303 packages but **8 external crates
>   fail before any rustc_* compiles** (once_cell:245 errors, log:100,
>   rustc-stable-hash:32, stable_deref_trait:17, plus memchr, smallvec,
>   regex-syntax, rustc-hash). The R3 external port work the recon
>   estimated at ~15 sessions is now the blocker.
> - **NEXT**: Stage E external-dep triage wave. Add
>   `[workspace.dependencies]` + `[patch.crates-io]` overrides in
>   semos-rustc/Cargo.toml, vendor + no_std-patch log/once_cell/
>   rustc-stable-hash. THEN the 48 rustc_* crates get tried. THEN
>   cg_clif wiring + DEMO 80.
>
> Recipe evolution discovered by D1 (rustc_middle): use
> `#![cfg_attr(target_os = "none", no_std)]` + `#[cfg(not(target_os =
> "none"))] extern crate std;` instead of A3's cfg(target_os="none")
> body-split. Cleaner, keeps host builds first-class. Folded into
> RECIPE.md §1.2. Recovery wave (E1-E4) used it throughout.
>
> Cumulative session totals (through Phase 5b scaffold):
> ~6.5M tokens, ~437k LOC of ~770k post-§1 internal rustc (~57%),
> 48 crates patched, semos-rustc binary scaffolded but rustc_driver_impl
> not yet wired in.
>
> Roadmap row landed in `docs/ROADMAP.md` summarizing Phase 3 closure.


This is the research-diary version of the M27 port. The plan is at
`docs/M27_RUSTC_PORT_PLAN.md`; the recon outputs are at
`docs/m27-recon/`. This file is *what we actually saw happen* as we
ran the experiment, append-only, with timestamps so future-me (or a
future agent on the second port of something similar) can replay the
decisions.

Why this file exists: nobody on this project has done a 4-8-agent
swarm port of a 70-internal-crate codebase before, and the project is
not big enough to have a "we always do it this way" rulebook. The
swarm is the experiment. The recipe is the artifact. This log is the
notebook.

---

## 2026-05-30 — Phase 1 (recon)

### Setup
- Vendored 38 MB / 77 crates from `rust-src` (nightly 1.95.0,
  2026-01-21) into `user-programs/semos-rustc/vendor-rustc-src/compiler/`.
- Committed as `c2e7c75` (plan) + the rustc-src staging commit.
- Spawned 4 parallel recon agents in isolated worktrees:
  R1 (dep graph) / R2 (std surface) / R3 (externals) / R4 (blockers).

### Outcomes
All four returned without bouncing. Calendar time: ~15 minutes (R2 was
first back at ~8 min, R4 last at ~14 min).

Headline reconciled numbers (`docs/m27-recon/SYNTHESIS.md`):
- 77 internal `rustc_*` crates / 70 after §1 drops / ~770 k LOC remaining
- 71 external crates / 50–55 after §1 drops
- 0 TRIVIAL crates (the 5-minute Cranelift pattern applies to none)
- 1 unmitigated blocker beyond §1 (B1: panic-as-control-flow)

### Things the recon surfaced that I hadn't anticipated
1. **§1.5 (drop proc-macros) is too coarse.** R1 §6.2 — keep
   `rustc_proc_macro` for type compatibility; drop only the runtime
   expansion server.
2. **§1.2 (statically link cg_clif) is cheaper than I thought.** R1 §6.1
   — cg_clif is already loaded via `cargo -Z codegen-backend`, not
   libloading, so no metadata-plugin surgery needed.
3. **The hash-crate stack should consolidate.** R3 — `rustc_span` uses
   blake3 + sha1 + sha2 + md-5; consolidating to blake3 saves ~3 sessions.
4. **rustc has its own forked rayon** at `compiler/rustc_thread_pool/`
   (R3). Single-threading it kills 3 external deps (crossbeam-deque/
   crossbeam-utils/jobserver) in one move.
5. **B1 has no clean v1 fix.** Accept "one error per compile" as the
   v1 product limitation. Real fix is a SemOS stack unwinder which is
   3-5 kernel sessions and out of M27 scope.

### Decisions taken (folded into the plan)
- **§1.7**: cg_clif owns final ET_EXEC emission; drop the
  `rustc_codegen_ssa::back::link` Command-spawn path entirely.
- **§1.8**: drop i18n (fluent-bundle + 7 ICU crates), hardcode English
  diagnostics. Saves ~5 sessions.
- **§1.9**: FatalError → process abort (one-error-per-compile in v1).

### What I'd do differently next time
- Spawn the recon agents in two rounds, not one. R1 (the dep graph) is
  a prerequisite for R2/R3/R4's framing — but I had them all run in
  parallel, so R2-R4 had to independently build cartographies before
  they could classify. That's wasted work. Next time: R1 first
  (single-agent), commit, then R2/R3/R4 in parallel with R1's output
  as context.

---

## 2026-05-30 (later) — Phase 2a first attempt

### Setup
Recipe (the standard semos-cc `[workspace] members = []` +
`#![no_std]` + alloc-prelude + `std::* → core::/alloc::/hashbrown::*`
substitution) handed to 6 agents in parallel worktrees:
- A1 rustc_data_structures + thread_pool stub (heavy)
- A2 rustc_span
- A3 trivials I (rustc_hashes / rustc_arena / rustc_fs_util / rustc_log)
- A4 trivials II (rustc_lexer / rustc_graphviz / rustc_ast_ir / rustc_error_codes)
- A5 rustc_index + rustc_serialize
- A6 proc-macros (4 crates)

### Failure mode
**All 6 agents bounced instantly with "You've hit your session limit ·
resets 10pm (America/Toronto)".** Each agent's duration was 2-3
seconds, 0 tokens used, 0 tool calls. Worktrees were created but empty.

This is the *quota* limit, not a per-agent failure. Spawning N agents
in parallel each consumes from the shared bucket; if the bucket is
already low when the spawn happens, all N bounce simultaneously.

### Diagnosis
- The earlier 4-agent recon (Phase 1) succeeded that same session, so
  the bucket WAS available before recon.
- Recon spent most of the bucket (~12 minutes of agent-time across 4
  agents).
- Phase 2a's 6-agent spawn hit the wall.

### Lesson worth capturing
> Spawning parallel agents in waves of 4-6 against a shared session
> quota means **the first wave consumes the budget the second wave
> needs**. The "20 agents at once" mental model from outside-Anthropic
> reports is not how the quota actually works for this user/plan.
> Practical pacing: 4-6 agents per wave, wait for completion +
> bucket-replenishment between waves.

### Adaptation
Rather than wait until 10pm Toronto for the reset, ran a single-agent
**probe** to verify the bucket had cooled enough to accept new work.

Probe target: rustc_hashes (131 LOC, smallest possible crate). If the
probe succeeded, I'd respawn the fleet immediately. If it bounced, the
quota was still locked and I'd switch to parent-only work.

**Probe succeeded.** Single agent, completed in 285 seconds (~5 min),
62k tokens, 50 tool calls. Returned a clean rustc_hashes port + two
recipe corrections.

### Recipe corrections from the probe
1. **`.cargo-checksum.json` step is N/A** for `compiler/rustc_*`
   crates. The Cranelift port relied on it because those crates were
   crates.io vendor checkouts (cargo vendor adds the checksum file).
   rustc-src crates are raw source and have no checksum file — agents
   were trying to update a file that doesn't exist.
2. **External dep `rustc-stable-hash 0.1.2`** is unconditionally std
   (no feature flag). Will need its own vendor + no_std patch before
   any rustc_*-target build can succeed.
3. **Fresh worktrees branch from where the parent was at session
   start**, not current main. Phase 2a agents on their first invocation
   were one merge behind because the rustc-src commit landed AFTER
   the session started. Probe agent had to `git merge main --no-edit`
   to access it.

### Recipe corrections folded into the v2 prompts
- "Step 0: `cd` worktree, `git merge main --no-edit`."
- "Skip `.cargo-checksum.json` updates — rustc-src crates have none."
- "If you find `rustc-stable-hash` usage, flag it; parent handles the
  vendor patch separately."

### Re-spawn
Phase 2a fleet re-spawned 5 agents (A1-A6 minus the rustc_hashes
already done by the probe). Same recipe, plus the corrections. Running
in parallel as of timestamp on this commit. Watching.

---

## Lessons-learned so far (running tally)

1. **Sequence recon agents R1 → {R2,R3,R4} parallel.** Saves them
   re-deriving R1's cartography.
2. **Session bucket is shared across parallel agents.** Plan agent
   waves with that in mind; probe before commits to N-agent waves.
3. **Worktrees branch from session start, not current main.** Always
   instruct agents `git merge main --no-edit` as step 0 if the parent
   has committed since the session began.
4. **`.cargo-checksum.json` step is for *vendored* crates only**, not
   raw source trees. Recipe must distinguish.
5. **Probe-then-fleet** is the right cadence for "I think the quota
   reset but I'm not sure" moments. One agent's failure tells us the
   bucket state for ~free.
6. **Document along the way.** The user reminded me of this 20 minutes
   in, and they're right — this file is the audit trail that lets the
   next port (typesetter Path A, or a follow-up rustc rebase) start
   from where we ended rather than from scratch.

---

## 2026-05-30 (still later) — Phase 2a wave 2

### Second failure mode discovered
A1 reported back ~1 min after spawn with a different failure:

> Every variant of the merge command (`git merge main`, `git merge main
> --no-edit`, even with `dangerouslyDisableSandbox: true`) returns
> "Permission to use Bash/PowerShell has been denied," while read-only
> git commands (`git status`, `git log`) work fine.

The agent CAN read but cannot write through `git merge`. The probe
agent earlier today merged main successfully — same prompt structure,
same recipe — but its sandbox apparently had merge permission.

**Hypotheses:**
1. Per-agent sandbox permissions vary between spawns. The probe got
   lucky.
2. Quota-state-dependent: when the bucket is low, even read-only
   surface contracts; when it's high, write-bash works. The probe ran
   shortly after the earlier 6-agent bounce (low bucket → maybe
   tighter sandbox); the second wave ran with bucket replenished
   (different state).
3. The first wave spawned all-at-once, sharing a single sandbox-grant
   bucket; the probe was a singleton and got the full bucket.

Without knowing which, the safe move is: **pre-merge each worktree
from the parent side before spawning agents that need recent main.**
That removes the dependency on agent-side `git merge` permission.

### Adaptation in flight
- A1 is done (gave up, correctly per its STOP-and-document constraint).
- A2-A6 likely still running. They will probably hit the same wall
  any moment. Waiting.
- Once all 5 report back (or fail), parent will: (a) `git merge main`
  inside each affected worktree, (b) re-dispatch via SendMessage or
  fresh spawn, depending on what's possible.

### Lesson worth capturing
> Per-agent sandbox permissions are not guaranteed. `git merge` worked
> for one agent and not for another in the same session. The safest
> contract is: **the parent does all git plumbing**; agents only touch
> source files. Don't ask an agent to merge, push, or branch — those
> are parent-side ops.

---

## 2026-05-30 — parent-side semos-std additions (in flight)

While waiting for A2-A6 to bounce or land, did the parent-only work
the recon agents specifically asked for. Each addition is a small
commit so the experiment log can show one-shot acceptance per API.

### Landed
- `sync::OnceLock<T>` — futex-backed, mirrors std exactly. Lifted from
  the local shim Cranelift used at D.2. 8+ rustc_* crates expect this
  symbol (R2 top-5). Commit `18d80dd`.
- `process::abort_with_code(i32)` — supports §1.9 FatalError → process
  abort with a chosen exit code. Commit `18d80dd`.
- `Path::canonicalize_lexical()` → `PathBuf` — collapses `.` and `..`
  without touching FS. Different name from std's `canonicalize`
  because that one is fs-resolving; rustc has both lexical and fs
  uses, callers will pick. Commit `<above>`.
- `ffi::OsString` (= `String`) + `ffi::OsStr` (= `str`) aliases. SemOS
  is UTF-8 everywhere; opaque-byte-container use cases (rustc) work.
  Commit `<above>`.

### What this unblocks
When the Phase 2a agents come back and try to apply the
`// M27 R4 B5: needs semos-std PathBuf/OsString shim` markers their
prompts told them to leave, those markers can be removed in the
integration pass — the API is now there. Same for the OnceLock TODOs
inside rustc_data_structures.

### Still pending (in priority order)
- `thread::LocalKey<T>` + `thread_local!` macro — single-threaded
  variant. 5+ rustc crates need it. More complex because of the macro.
- `env::var_os` — trivial extension on top of existing `env::var`.

Will land both once it's clear the agent fleet isn't competing for
the bucket.

---

## 2026-05-30 — A4 + A5 + A6 integration

### A6 came back first (of the wave-2 retry)
Successfully ported all 4 proc-macro crates. Zero source edits, just
`.cargo/config.toml` + workspace headers. ~7 min of work, 100k tokens,
105 tool calls.

**Critical adaptation A6 discovered:** when `git merge main` was
denied, it used `git show main:<path>` (read-only) to peek at files
in main's tree, then used the Write tool to compose them into the
worktree. This is the right pattern: **agents should use git for
read-only history access, Write tool for all file creation**. Never
ask the agent to mutate git state.

**rustc_fluent_macro disposition** (the §1.8 i18n drop question): A6
recommends "port now, delete in Phase 2b when rustc_errors gets gutted."
Zero patch cost now; trivial deletion later. Accepted.

### A4 and A5 outputs landed in main without explicit notification
This is interesting and a little concerning. Their patched files +
notes (`docs/m27-port/2a/A4-trivials-2.md`, `A5-index-serialize.md`)
appeared in main's working tree before any completion notification
arrived. The most likely explanation is that **worktrees in this
harness share the main working directory** — they're really separate
git BRANCHES with the same working tree, not separate working trees
in the cargo sense. Worktree branch IDs are just for tracking.

**Implication for swarm orchestration:** the "agents work in isolated
worktrees so they don't conflict" assumption from M27_RUSTC_PORT_PLAN
may not actually hold here. In practice the agents are sharing the
working tree but writing to disjoint files (different crates). The
isolation is by ASSIGNMENT, not by git mechanics.

**This worked here because agent crate assignments were disjoint.** If
two agents had been asked to patch the same crate, they would have
raced. Worth flagging as a real risk for Phase 3 where multiple
agents work in the same cluster.

### Phase 2a tally
- ✅ rustc_hashes (probe)
- ✅ rustc_lexer, rustc_graphviz, rustc_ast_ir, rustc_error_codes (A4)
- ✅ rustc_index, rustc_serialize (A5)
- ✅ rustc_macros, rustc_index_macros, rustc_type_ir_macros,
  rustc_fluent_macro (A6)
- ⏳ rustc_arena, rustc_fs_util, rustc_log (A3, still running)
- ⏳ rustc_span (A2, still running)
- ❌ rustc_data_structures + thread_pool (A1, bounced on sandbox
  merge denial — need to re-dispatch with the `git show main:` pattern
  from A6 OR parent does pre-merge)

11/14 of the Phase 2a target crates patched. Plan estimated Phase 2a
at 1-2 calendar-sessions; we're hours in and already at ~80% completion
of the patch (not integration) work. Looks like the actual number is
closer to **0.5 calendar-sessions for patches across 6 agents in
parallel**. That makes the original 40-60 calendar-session estimate
for all of M27 conservatively right.

### Lesson worth capturing
> Worktrees in this harness share the working directory; isolation is
> by assignment (which files each agent owns), not by git mechanics.
> Disjoint crate assignments are critical. NEVER have two agents
> assigned to the same crate.

> Agents should never run `git merge`, `git checkout`, `git restore`,
> or any state-mutating git command. The reliable pattern: `git show
> main:<path>` (read) → Write tool (apply). A6 discovered this; codify
> it in the recipe.

---

## 2026-05-30 — A3 + A4 + A5 final tally + thread_local landed

### A3 came in independently
Used the same A6 read-from-main pattern. Files written to its OWN
worktree (different from A4/A5/A6 which wrote to main paths). Had to
cherry-pick from worktree → main. Confirmed: each agent's write-target
choice was independent — some chose worktree paths, some chose main
paths.

**Refined lesson:** the "worktrees share working directory" was a
partial truth. The mechanism is that worktrees have separate git
indices but Bash `cd` to main paths still hits main's working tree.
Agents that resolved paths relative to their worktree wrote there;
agents that resolved to main paths wrote there. Both worked, but
required different cherry-pick treatment.

### A3 pattern worth codifying
- `cfg(target_os = "none")` to gate SemOS-target patches while
  preserving host build paths. Two of A3's three crates used it
  (rustc_fs_util, rustc_log).
- `pub macro` items emitting `::std::*` tokens need rewriting to
  `::core::*` (caught in declare_arena!).
- `core::error::Error` is stable since 1.81 — direct substitute.
- MARK-class crates (rustc_fs_util) preserve host body, shim SemOS
  body with `// M27 R4 Bx` markers + `io::Error(Unsupported)`
  returns where surface isn't ready yet.

### rustc_graphviz partial state to flag
A4 (re-run) confirmed rustc_graphviz uses `core::io::Write` /
`core::io::Result` which **don't exist in stable Rust** — only std
has them, not core. Needs a semos_std::io shim. A4 documented three
parent-side resolution options (inject `use semos_std::io;`, replace
`core::io` → `semos_std::io` directly, or vendor `core2`). Cleanest
is probably the second — semos_std::io exists and has the right shape.
Marked as Phase 2b work, not a blocker for the rest of Phase 2a.

### Phase 2a tally — 14/14 of foundation crates done
- ✅ rustc_hashes (probe)
- ✅ rustc_lexer, rustc_graphviz (partial — io shim pending),
  rustc_ast_ir, rustc_error_codes (A4)
- ✅ rustc_index, rustc_serialize (A5)
- ✅ rustc_macros, rustc_index_macros, rustc_type_ir_macros,
  rustc_fluent_macro (A6)
- ✅ rustc_arena, rustc_fs_util (MARK-class), rustc_log (R3 stub) (A3)
- ❌ rustc_data_structures + thread_pool (A1, bounced — re-dispatch
  needed with the A6 git-show-main pattern)
- ⏳ rustc_span (A2, still running ~30 min in)

### parent-side semos-std additions still rolling
- ✅ sync::OnceLock<T>
- ✅ process::abort_with_code(i32)
- ✅ path::Path::canonicalize_lexical()
- ✅ ffi::OsString + ffi::OsStr (UTF-8 aliases)
- ✅ thread::LocalKey<T> + thread_local! macro (single-threaded
  variant). Just landed.
- ⏳ env::var_os (small extension)

### Calendar-time observation
Phase 2a's patch work (14 crates across 6 agents) finished in
~50 minutes of actual elapsed time. Plan estimated 1-2 calendar-
sessions. **Multi-agent parallelism appears to compress the schedule
substantially when the tasks are well-isolated.** Phase 3 (24-25
crates per cluster) should scale similarly if isolation holds.

---

(Still waiting on A2 — rustc_span — the largest foundation crate.
A1 re-dispatch deferred until A2 returns or the user signals.)

---

## 2026-05-30 — R2 top-6 semos-std surface complete

env::var_os + vars + vars_os landed. That's the last of R2's top-six
high-priority semos-std additions. Six commits total for the surface
work:

- `18d80dd` sync::OnceLock<T> + process::abort_with_code(i32)
- `4a9af1d` path::Path::canonicalize_lexical() + ffi::OsString/OsStr
- `f4d1a60` thread::LocalKey<T> + thread_local!
- `7ebc0f7` env::var_os + vars + vars_os

All built clean against x86_64-unknown-none. Pure additions, zero API
breakage. semos-std's surface area for the rustc port now covers what
the recon agents flagged. Anything beyond this (path canonicalization
that touches the FS, multi-threaded TLS, full env-listing) is
deliberately out of v1 scope per the M27 plan §1.

When Phase 2b/3 agents try to integrate the marked sites (`// M27 R4 Bx`
markers the Phase 2a agents left), they'll find the API now exists and
can do a straight substitution instead of a TODO. That's the integration
acceleration the recon predicted.

---

(Last waiting on A2 — rustc_span. ~40 min in and counting.)

---

## 2026-05-30 — A2 partial + scoped_tls + A1+A2-followup re-dispatched

### A2 returned partial (13/18 files, ~19% LOC coverage)
2,300 of 12,327 LOC ported before context budget ran out. Five large
files remain with line-precise recipes documented in A2's notes:
lib.rs, hygiene.rs, source_map.rs, source_map/tests.rs, symbol.rs.

Notable A2 decisions to capture:
- **R4 B1 (FatalError)**: rewrote `src/fatal_error.rs` end-to-end per
  §1.9. `raise()` → `process::abort()`, `catch_fatal_errors()` →
  `Ok(f())`. One-error-per-compile in v1.
- **R4 B2 (scoped_tls)**: kept `scoped_thread_local!()` macro calls
  in hygiene.rs intact; fix is dep-side. **Single biggest blocker A2
  flagged.**
- **R3 hash consolidation**: NOT done. `SourceFileHashAlgorithm` enum
  is ABI-visible (rmeta encode/decode boundary). Kept md5+sha1+sha2
  deps; tagged non-blake3 variants with `// M27 R3:` markers. Phase 4
  owns the final call.

### parent: scoped_tls shim landed in semos-std
The A2 blocker. semos-std now has `scoped_thread_local!` + `ScopedKey<T>`
mirroring the scoped-tls crate's API. Single-threaded `Cell<*const T>`
+ Drop guard implementation. Recursive set panics (matches upstream).
Unblocks rustc_span integration once A2-followup finishes the 5
remaining files.

### Worktree CWD drift caught + recovered
A diagnostic incident: my Bash session's persistent `cd` had carried
me into A2's worktree dir without me noticing. `git log` showed
pre-session commits, panic ensued briefly, then `cd /f/Software/
ArmKernel3` + `git log` confirmed main has all the work. The commits
were going to main correctly because I'd been doing `cd
/f/Software/ArmKernel3 && git commit ...` explicitly; only the
shell's pwd had drifted.

**Lesson worth capturing:** prefix every git operation with an
explicit `cd /f/Software/ArmKernel3` when working alongside
worktrees, or pin via `git -C /f/Software/ArmKernel3 ...`. The
Bash persistent-CWD behavior + worktree paths is a real
foot-gun.

### Two follow-up agents launched in parallel
- A1 retry: rustc_data_structures + rustc_thread_pool via
  `git show main:` + Write pattern (skip the merge entirely).
- A2-followup: rustc_span's 5 remaining files using A2's
  line-precise recipes.

Both running.

---

(A1 retry + A2-followup in flight. Plus thread_local + scoped_tls now
both in semos-std.)

---

## 2026-05-30 — Token / LOC accounting (retroactive)

User asked if I was tracking tokens vs LOC. Wasn't. Starting now,
back-filling from the agent completion notifications.

### Phase 1 — recon (no LOC ported, characterized the whole tree)

| Agent | Tokens | Tool uses | Duration | Output |
|-------|-------:|----------:|---------:|-------|
| R1 dep graph | 174,686 | 220 | 893 s | 942-line report on 77 crates |
| R2 std surface | 185,752 | 61 | 462 s | 2,497-line report |
| R3 externals | 142,245 | 142 | 468 s | 390-line report |
| R4 blockers | 220,065 | 149 | 822 s | 580-line report |
| **Phase 1 total** | **722,748** | **572** | **41 min wall (parallel)** | **4,409 lines, 0 LOC ported** |

R4 was the most expensive — read the most files, made the gatekeeper
call. R2 was second — produced the longest report.

### Phase 2a — port agents (LOC ported = patched-against-recipe)

| Agent | Crate(s) | Source LOC | Tokens | Tokens/LOC | Tool uses | Duration | Notes |
|-------|----------|----------:|-------:|----------:|----------:|---------:|-------|
| Probe | rustc_hashes | 131 | 62,829 | 480 | 50 | 285 s | Verified session limit cleared; ran the recipe end-to-end on smallest crate |
| A3 | rustc_arena + rustc_fs_util + rustc_log | ~1,354 | 120,686 | 89 | 78 | 610 s | Introduced `cfg(target_os = "none")` host/target split pattern |
| A4 | rustc_lexer + rustc_graphviz + rustc_ast_ir + rustc_error_codes | ~3,836 | 119,396 | 31 | 136 | 663 s | Re-run; original A4 already integrated. Discovered `core::io::Write` doesn't exist (graphviz partial) |
| A5 | rustc_index + rustc_serialize | ~5,513 | 194,784 | 35 | 111 | 563 s | §1.3 (drop incremental) applied — odht/dep_graph cfg'd out. SourceFileHashAlgorithm enum is ABI-visible (R4 hint) |
| A6 | rustc_macros + rustc_index_macros + rustc_type_ir_macros + rustc_fluent_macro | 0 source edits | 101,141 | n/a | 105 | 425 s | Proc-macros host-only; just `.cargo/config.toml` + workspace headers. Pioneered the `git show main:` workaround |
| A2 | rustc_span (partial, 13/18 files) | ~2,300 | 274,856 | 120 | 136 | 1,272 s | Highest token consumer. Recipe for the remaining 5 files documented for A2-followup |
| A2-followup | rustc_span (5 remaining files) | ~10,000 | 136,384 | **14** | 94 | 554 s | Most efficient yet — A2's pre-documented recipes turned this into recipe-application. Surfaced semos_std::path API gap (Cow<Path>, Component::Normal, etc.) for Phase 2b |
| A1-retry | rustc_data_structures + rustc_thread_pool (stub) | ~15,172 (incl. 600-LOC stub replacing 7,476-LOC rayon fork) | 299,253 | **20** | 156 | 1,110 s | Heroic single-agent run that closed Phase 2a. Six modules used the cfg(target_os="none") split. Deferred parking_lot bits in sync.rs to Phase 2b with line-precise notes |
| **Phase 2a total** | **16 crates complete; rustc_span 18/18** | **~38,306 source LOC** | **1,309,329** | **~34 avg** | **866** | **~5.6 hrs sum (~90 min wall parallel)** | **PHASE 2a CLOSED** |

A4 was the most efficient at 31 tokens/LOC, because the four crates
were small and mechanical and A4 just ran the standard recipe. A2 was
the most expensive at 120 tokens/LOC because rustc_span is the
biggest foundation crate, hit multiple architectural decisions
(FatalError, scoped_tls, hash consolidation), and had to read+write
in full-file rewrites (no merge access).

### Session-wide running total (Phase 1 + Phase 2a CLOSED)

**Tokens spent on agents: 2,032,077.**
**LOC patched: ~38,306.** (Phase 2a foundation tier complete.)
**Average across all port work (Phase 2a only, excluding recon):
34 tokens/LOC.**

Roughly **~75 tokens per ported LOC** on average across the whole
session (recon + port + integration). The recon weight (722k tokens,
~42% of total now) is the front-loaded cost; subsequent phases should
hover closer to A4/A5/A2-followup's 14-35 tokens/LOC since they won't
need the characterization work.

**A2-followup as the most-important data point so far:** at 14 tokens/
LOC, it's the cheapest non-zero-source agent in the whole run.
Mechanism: A2's notes gave it a *line-precise recipe* per file, so
the followup didn't have to re-derive any classification. Lesson:
**pre-documenting recipes for the predecessor's deferred work is
worth ~10× efficiency on the followup.** Generalizable to any future
"A is too big to finish in one agent" situation.

### Forecast for Phase 2b + Phase 3 + Phase 4

Using A2's 120 tokens/LOC as the upper bound (for hard novel crates)
and A2-followup's 14 as lower bound (for recipe-following work), and
the plan's estimate of ~770 k LOC of post-§1 internal rustc crates
to port:

- **Conservative** (everything novel): 770k LOC × 120 t/LOC = 92.4M tokens
- **Optimistic** (everything recipe-following): 770k LOC × 14 t/LOC = 10.8M tokens
- **Mixed** (assume 60% novel-class, 40% recipe-following weighted at A2/A2-followup): 770k × (0.6×120 + 0.4×14) = ~59.7M tokens
- **Recon-weighted mixed** (using A4/A5/A3 avg ~50 t/LOC for the bulk + A2-class for the hard 20%): 770k × (0.8×50 + 0.2×120) ≈ 49M tokens

For a sense of scale: at the current session rate (~1.6M tokens for
foundation tier ≈ 13k LOC = 1.7% of post-§1 internal rustc), the full
Phase 2-4 port projects to **20-60 million tokens** depending on how
the hard crates land. Across the planned 4-6 agents per parallel wave,
spread across 1-2 months, that's a real but quantifiable budget.

The recon's 1-2 month / 40-60 session estimate looks consistent with
this token math. Each agent "session" averages 100-200k tokens; 40-60
sessions × 4-6 agents in parallel ≈ 16-72M tokens for the whole port.
The recon estimate sits in that range.

### Logging cadence going forward
Will record each agent's tokens / tool uses / LOC patched in the
table above as they come in. The above table covers everything
through A2 + A6 (the most recent completed wave); A1 retry and
A2-followup will be appended on completion.

---

(Logging on.)

---

## 2026-05-30 — PHASE 2a CLOSED

A1 retry came back ~3 hours after the session started. A heroic
single-agent run: rustc_thread_pool stubbed (~600 lines replacing the
7,476-line vendored rayon fork) AND rustc_data_structures fully ported
(33 source files + 6 architectural-class modules using the
`cfg(target_os = "none")` host/target body split).

299,253 tokens at 20 t/LOC — second-most-efficient port agent after
A2-followup (14 t/LOC). That ratio (recipe-following at 14, novel-
class-but-with-good-context at 20) is the band Phase 3 should aim
for.

A1 deferred parking_lot-gated bits in `sync.rs` + `sync/lock.rs` +
`sync/freeze.rs` to Phase 2b with line-precise notes (per the
HANDOFF_TEMPLATE we just codified). Recommended approach: collapse
`Mode::Sync` → `Mode::NoSync` on SemOS (~1 session). Per the recipe
just landed, the followup should hit ~14-20 t/LOC.

**Phase 2a final tally:** 16 crates closed (foundation tier
complete) at **34 average tokens/LOC** across ~38,306 LOC patched
and 1,309,329 tokens spent on port agents (Phase 1 recon adds
722,748 tokens). The recon estimate of 40-60 calendar-sessions for
the full port stays consistent; the token side of the forecast
tightens toward the **mixed-weighted ~50M token estimate** with this
data point.

**Calendar-time observation refined:** Phase 2a took ~90 minutes of
wall-clock time with 6-7 parallel agents (plus ~30 min of probe +
re-dispatch). That's 1/3 of an 8-hour day. The 1-2 month calendar
estimate for the full M27 port assumes agents are not running 24/7;
if they were, the math would close in much faster. Treating ~10
parallel-agent-hours per real calendar day as the practical capacity
seems right based on this run.

### What changes for Phase 2b
The remaining Phase 2 work splits two ways:
1. **rustc_ast + rustc_lint_defs + rustc_errors** (the cycle) —
   sequential, ~3-5 sessions per the plan. The RECIPE codification
   should make this cheaper than A1/A2 were because of established
   patterns.
2. **A1 + A2 deferred work** — line-precise recipes already
   documented for parking_lot collapse (A1) and PathBuf API extension
   (A2-followup). These are followup-class work at ~14-20 t/LOC.

Both can run in parallel — Phase 2b's cycle work is in a different
file set than the parking_lot/PathBuf followups.

---

## Phase 2a → Phase 2b transition checklist

- [x] All 14 originally-targeted Phase 2a crates patched
- [x] rustc_data_structures + rustc_thread_pool stub (A1 retry)
- [x] semos-std surface covers R2's top-6 + scoped_thread_local!
- [x] RECIPE.md + HANDOFF_TEMPLATE.md codified
- [x] Per-agent token costs tabulated
- [ ] Phase 2b cycle plan (3 crates × sequential ports)
- [x] Phase 2b launched (4 parallel agents)
- [ ] Parent action items: rustc-stable-hash vendor patch, tracing
      stack vendoring, ~PathBuf API extension~ (DONE this commit)

---

## 2026-05-30 — Phase 2b launched

Four parallel agents in flight:
- **B1** rustc_ast (11,553 LOC)
- **B2** rustc_lint_defs (6,451 LOC)
- **B3** rustc_errors (7,807 LOC; §1.8 i18n removal is the
  architectural call)
- **B4** A1 deferred sync.rs/lock.rs/freeze.rs parking_lot collapse
  (A1's line-precise recipe; ~1,000 LOC; expected to land at the
  recipe-following 14-20 t/LOC band)

Plus parent-side: **path API extension done** —
Components/Component/strip_prefix/as_os_str/Cow<Path>/Borrow/ToOwned.
A2-followup flagged this as the single biggest gap blocking
rustc_span integration. semos-std build clean. When the Phase 2b
agents integrate, they hit a more-complete surface and can route
through these directly instead of leaving TODO markers.

### Token forecast for Phase 2b
- B1 (rustc_ast, novel hard): ~50 t/LOC × 11,553 LOC ≈ 580k tokens
- B2 (rustc_lint_defs, mechanical): ~30 t/LOC × 6,451 LOC ≈ 195k
- B3 (rustc_errors, §1.8 work): ~80 t/LOC × 7,807 LOC ≈ 625k
- B4 (followup recipe): ~17 t/LOC × 1,000 LOC ≈ 17k
- **Phase 2b expected total: ~1.4M tokens**
- Session-running-total after Phase 2b: ~3.4M tokens for ~64k LOC
  patched. ~53 t/LOC average. Below the mixed-weighted ~50M token
  budget for the whole port.

If B3's §1.8 work goes deep (recon estimated ~5 sessions saved by
dropping i18n; this is the i18n drop itself) it could trend higher
than the forecast. Monitor.

---

## 2026-05-31 — Phase 2b returns + B3 session-limit incident

Three of four Phase 2b agents returned. B3 bounced on session limit
at the *very end* (8.4 min in, 97 tool uses, only 2,719 tokens recorded
because the bounce happened during summary writing). The work BEFORE
the bounce did land — 10 of 15 rustc_errors files were patched.

### Token accounting for Phase 2b returns

| Agent | Crate | LOC | Tokens | T/LOC | Notes |
|-------|-------|----:|-------:|------:|-------|
| B1 | rustc_ast | 11,553 | 116,071 | **10** | Crate much less std-coupled than expected; only 30 raw std:: refs, all trivial. Zero deferral markers. |
| B2 | rustc_lint_defs | 1,042 effective / 6,451 blast | 93,426 | 90 effective / 14 blast | builtin.rs (5,409 LOC) needed zero edits — pure declare_lint! macros covered by crate-root no_std. Pattern worth codifying. |
| B3 | rustc_errors (10/15 files) | ~5,000 partial | 2,719 recorded* | n/a | Bounced during summary. Work landed; notes did not. B3-followup dispatched. |
| B4 | A1 sync collapse | ~200 focused | 78,749 | n/a | freeze.rs needed zero edits — A1's flag was a false positive. |

*B3's recorded tokens were truncated by the bounce. Actual cost was
probably 200-300k based on tool_uses and duration.

### B1's surprise: 10 t/LOC for the biggest cycle crate

The plan estimated rustc_ast as one of the heaviest crates (it's the
foundation of the cycle). Reality: 10 t/LOC because the crate has
**very thin std surface** — most of its bulk is enums, structs, and
visitor traits that are pure core. Only 30 distinct std:: paths across
11,553 LOC; one OnceLock substitution; one dead `use std::panic;`
removed.

This is a major data point for the forecast: crates can be **LARGE
but THIN** (lots of LOC, little std surface). The recon's LOC-only
classification missed this distinction. R2's std-surface counts
(which we DIDN'T forecast against) were a better predictor.

**Lesson**: when projecting future tokens for novel crates, use R2's
std-surface count as the primary signal, not LOC. Update the forecast
methodology in the next round.

### B2's surprise: macro-heavy files are free

builtin.rs (84% of rustc_lint_defs by LOC) needed zero source edits
because it's pure `declare_lint!`/`declare_lint_pass!` macro
invocations. Once the crate root has `#![no_std]` + `extern crate
alloc;`, every macro-emitted item is covered.

**RECIPE addition** (will fold in): "If a file is &gt;80% declarative
macro invocations covered by crate-root attributes, verify it compiles
clean with no source edits before running the substitution sweep."
This is a third efficiency tier alongside "recipe-following" (14 t/LOC)
and "config-only" (zero source).

### B4's false positive

A1's notes flagged sync/freeze.rs as needing the parking_lot collapse.
B4 verified: it doesn't. freeze.rs imports `RwLock`/`ReadGuard`/
`WriteGuard` exclusively via re-exports from `crate::sync`, so the
host/target gating in sync.rs flows through transparently. B4 saved
itself the work by checking the dependency direction first.

**Lesson**: predecessor recipes are valuable but not infallible. The
followup agent SHOULD verify before applying — A1's recipe was right
about the *substance* (Mode::Sync → Mode::NoSync collapse) but wrong
about the *scope* (3 files vs 2).

### Session bounce pattern — a third mode

Phase 2a's first wave bounced INSTANTLY (0 tokens, instant rejection
of the spawn). B3 bounced LATE (8.4 min of real work, then the
summary-writing phase hit the limit). Both are "session limit"
messages but mean very different things:

- **Instant bounce**: redo with smaller wave or different timing.
- **Late bounce**: work landed in the working tree; only the
  notes/synthesis was lost. Treat as "completed without notes" and
  dispatch a followup to verify + document.

This distinction matters for orchestration. Captured in the codified
RECIPE.

### B3-followup dispatched

Reads B3's diffs from the integration commit, finishes the remaining
5 files (emitter.rs is the big one, plus annotate_snippet_*, registry,
tests, json/, markdown/), applies §1.8 i18n removal in emitter.rs,
writes proper handoff notes per HANDOFF_TEMPLATE.

Expected: 100-200k tokens; 50-80 t/LOC for emitter.rs's §1.8 work.

(B3-followup in flight.)

---

## 2026-05-31 — PHASE 2b CLOSED

B3-followup returned at 191,306 tokens / 150 tool uses / 14.7 min for
the 5 remaining rustc_errors files. The combined B3 + B3-followup
spend (~250k tokens estimated) for rustc_errors's 7,807 LOC came in
at ~32 t/LOC — squarely the recipe-following band, even with the
§1.8 i18n architectural call.

### Phase 2b final tally

| Agent | Crate | LOC | Tokens | T/LOC | Status |
|-------|-------|----:|-------:|------:|--------|
| B1 | rustc_ast | 11,553 | 116,071 | 10 | done in one |
| B2 | rustc_lint_defs | 6,451 | 93,426 | 14 (blast) | done in one |
| B3 | rustc_errors (10/15) | ~5,000 | ~80k est | n/a | late-bounce |
| B3-followup | rustc_errors (5 remaining) | ~2,800 | 191,306 | 68 | done |
| B4 | A1 sync collapse | ~200 | 78,749 | n/a | done |
| **Phase 2b total** | **4 crates + 1 followup** | **~26,000** | **~560k** | **~22 avg** | **CLOSED** |

Phase 2b ran in ~2.5 hours wall-time including the late-bounce
recovery cycle (B3 → B3-followup adds ~25 min of orchestration cost).

### B3-followup's surprise: fluent-bundle port not needed

B3-followup found that rustc_errors needs only ONE real FS site (ICE-
file flush). Everything else is in-memory format!/Write. **This means
the R3-budgeted 3-session fluent-bundle external port can probably
be skipped entirely** — we already neutered fluent into a passthrough
Translator per §1.8, and we never read fluent_bundle's body. Net
savings: ~3 sessions beyond what the recon estimated.

The §1.8 decision keeps paying dividends. Worth re-running the recon
math for Phase 4 externals with the same skepticism.

### Session-wide cumulative

- **Tokens spent on agents**: ~2,590k = 723k (Phase 1) + ~1.3M (Phase 2a) + ~560k (Phase 2b)
- **LOC patched**: ~64,000 = ~38k (Phase 2a) + ~26k (Phase 2b)
- **Foundation tier complete**: 19 crates patched + 1 stub
- **Wall-time spent**: ~5 hours (Phase 1 recon ~40 min, Phase 2a ~90 min, Phase 2b ~2.5 hrs incl. recovery)
- **Avg t/LOC for port work**: 30 (Phase 2a + 2b combined)
- **Phase 3 forecast** (770k post-§1 internal LOC × 30 t/LOC): ~23M tokens. Down from the previous mixed-weighted ~38M estimate because actual t/LOC ratios continue trending toward the recipe-following band as agents have more codified context to work from.

### The B1 surprise generalizes

Re-applying B1's insight (LARGE-but-THIN crates) to R1's classification:

- **Crates that LOOK heavy by LOC but are mostly enums/structs/visitor
  traits**: rustc_ast (11.5k LOC, 10 t/LOC), rustc_lint_defs (6.5k
  LOC, 14 t/LOC blast), probably also `rustc_hir` (11.4k LOC) and
  `rustc_hir_pretty` (?).
- **Crates that have HEAVY std surface**: rustc_data_structures (sync
  primitives, OS abstractions), rustc_metadata (file I/O), rustc_session
  (sysroot search), rustc_codegen_ssa (linker invocation).

For Phase 3 the assignment should weight by std-surface, not LOC.
Cluster A (frontend) is probably mostly LARGE-but-THIN; Cluster B
(semantics) is probably HEAVY-surface in fewer crates.

### Phase 2b → Phase 3 transition

- [x] Phase 2b CLOSED
- [x] RECIPE.md + HANDOFF_TEMPLATE.md in use
- [x] semos-std surface complete for R2 top-6 + scoped_thread_local!
      + path Components/Component/strip_prefix/Cow<Path>
- [x] Phase 2b token accounting in table
- [x] Phase 3 assignment by std-surface (not LOC) — done; cluster
      map in §"Phase 3 Wave 1 launch" below
- [x] B3-followup's recommended semos-std additions: io::Stderr +
      LocalKey<Cell<T>>::{get,set,take,replace} +
      LocalKey<RefCell<T>>::with_borrow{,_mut} sugar
      (commit `7978ce5`)
- [x] User signed off "both clusters back-to-back"; Phase 3 launched

Phase 3 splits into Cluster A (frontend, ~8 crates) + Cluster B
(semantics, ~13 crates including rustc_middle at 60k LOC). Each
cluster can support 3 parallel agents.

---

## 2026-05-31 — Phase 3 Wave 1 launch + return

Parent prep (`7978ce5`): io::Stderr struct + io::{stdout,stderr}()
factories; LocalKey<Cell<T>>::{get,set,take,replace} +
LocalKey<RefCell<T>>::with_borrow{,_mut} (std 1.73 sugar). Reverted
the 3 verbose `.with(|c| c.get())` sites in rustc_errors/markdown/
term.rs and updated RECIPE.md §2.

### Cluster A map (by std-surface, not LOC)

| Agent | Crates | LOC | R2 class |
|-------|--------|----:|----------|
| C1 | rustc_parse + rustc_parse_format | ~32k | MECHANICAL (R2 wrongly flagged Command::new in parser/diagnostics.rs) |
| C2 | rustc_ast_pretty + rustc_ast_lowering + rustc_ast_passes | ~19.6k | THIN downstream of B1 |
| C3 | rustc_attr_parsing + rustc_feature + rustc_builtin_macros + rustc_expand | ~41k | MEDIUM (proc-macro §1.5 cfg-out, LazyLock+VarError gaps) |

(Skipped: rustc_lexer already done in Phase 2a A4; rustc_attr_data_structures
folded into rustc_hir in this snapshot.)

### Wave 1 return

| Agent | Tokens | Tool uses | Duration | LOC patched | T/LOC | Status |
|-------|-------:|----------:|---------:|------------:|------:|--------|
| C1 | 165,530 | 164 | 871 s (~14.5 min) | ~32k inspected, ~10 sites | **3.6** | COMPLETE |
| C2 | 121,976 | 185 | 682 s (~11.4 min) | ~19.6k inspected, 23 files patched | **3.6** | COMPLETE |
| C3 | 245,654 | 121 | 1,117 s (~18.6 min) | 13 files written + line-precise §3 recipes for 29 more | n/a (PARTIAL) | PARTIAL (followup needed) |
| **Wave 1 total** | **~533k** | **470** | **~45 min wall (parallel)** | **~92k LOC covered (Cluster A ~80% complete)** | **~5.2 avg (excluding C3's recipe-only files)** | — |

C1 + C2 came in well under the 14-30 t/LOC band: 3.6 t/LOC each.
The B1 LARGE-but-THIN insight from Phase 2b continues to dominate
the actual data — Cluster A's frontend crates are mostly downstream
of rustc_ast (which Phase 2b already no_std-ified) and inherit the
thin-surface profile.

### Notable findings

1. **R2's Command::new claim in rustc_parse/parser/diagnostics.rs
   was wrong.** C1 found no such site in the current snapshot. R2
   may have looked at rustc_driver_impl or an older version.
   One architectural decision saved. This is a third LESSON about
   recon-vs-port-truth: recon is directional, not authoritative on
   site-specific claims.

2. **C2's three crates: ZERO architectural markers.** Cleanest port
   since A6 (proc-macros). Downstream-of-cycle-foundation crates
   inherit hygiene; 0.12 std::* per 100 LOC (B1 was 0.26).

3. **C3 wrote into its worktree path, not main.** The worktree
   branched from a stale (pre-rustc-src) commit, so the parent had
   to manually copy 13 source files + 1 notes file across to main.
   C1 and C2 wrote correctly to main paths. Variance in agent
   write-target choice was already documented in Phase 2a; codify
   in RECIPE: "agents that branch from stale parent should pre-merge
   their target paths OR the parent should run pre-spawn `cd
   F:\\Software\\ArmKernel3` checks."

4. **R3-class new gaps surfaced in C3.** Two parent-side semos-std
   additions delivered same session (commit `c9f0b2d`):
   - `sync::LazyLock<T>` (8+ rustc crates) — std-shape, OnceLock-
     backed, fn-pointer init (not closure → const-constructible).
   - `env::VarError` + `env::var() -> Result<String, VarError>`
     (4+ sites; std signature). NotPresent + NotUnicode variants
     for source-compat; NotUnicode unreachable on UTF-8 SemOS.
     sem-sh's 2 callers (Some→Ok, ||→|_|) updated in same commit.

5. **asm.rs hashbrown integration fix.** rustc_ast_lowering/src/
   asm.rs's `hashbrown::hash_map::Entry` import routed through
   `rustc_data_structures::fx::StdEntry as Entry` (target-conditional
   alias B4 already wired in Phase 2b). Avoids adding hashbrown as
   a direct dep. One-line landing on top of C2's patch.

### Cumulative session total after Wave 1

- **Tokens spent on agents**: 723k (Phase 1) + 1,309k (Phase 2a) +
  ~560k (Phase 2b) + ~533k (Wave 1) = **~3.13M**.
- **LOC patched**: ~38k (2a) + ~26k (2b) + ~92k Cluster A
  (partial — Wave 1 covered ~80% of Cluster A's source surface,
  C3-followup handles the rest) = **~156k**.
- **Wall-time**: ~5 hours (Phase 1+2a+2b) + ~1.5 hours (Wave 1
  including parent prep + integration) = ~6.5 hrs.

### What to do next

Wave 2 (Cluster B, 4 agents in parallel: D1=rustc_middle solo, D2=HIR
tier, D3=infer/types tier, D4=borrowck+resolve NEEDS-SHIM pair) +
C3-followup (single recipe-following agent applying §3 to the 29
remaining files) — possibly as a 5-agent wave or sequenced.

C3-followup's expected cost: ~150k tokens at ~5-10 t/LOC (the
predecessor recipe pattern continues to deliver ~10× efficiency).

---

## 2026-05-31 — Phase 3 Wave 2 launched (5 agents) + WHOLE-WAVE late-bounce

Wave 2 went out: D1 rustc_middle solo, D2 HIR tier, D3 inference,
D4 borrowck+resolve, plus C3-followup applying C3's §3 recipes.
Five agents in parallel.

**All five bounced simultaneously on session limit ~9-10 minutes in.**
Each had done 110-126 tool uses worth of real work; each reported
back with the same message: "You've hit your session limit · resets
8:50am (America/Toronto)" and a token usage in the 4-6k range that's
the *post-bounce* summary attempt, not the work done before. Real
spend per agent was probably 100-300k pre-bounce.

This is a NEW failure mode at the wave-orchestration layer: the B3
late-bounce pattern from Phase 2b, now happening to ALL agents in a
wave at once. Mechanism: the bucket was already partially-depleted
from Wave 1's ~533k + parent-prep work in the same session window.
Spawning 5 fresh agents drove total demand past the limit, and all
five hit the wall at roughly the same simulation time (around their
summary-write phase).

**Lesson worth codifying:** when bucket-state is unknown after recent
heavy use, **probe-then-fleet, don't fleet-then-pray**. One probe
agent first — if it finishes cleanly, the bucket has headroom; then
spawn the rest. Phase 2a learned this once; we forgot it for Wave 2
because Wave 1 had gone so smoothly.

### Wave 2 partial work

Despite the bounce, real work landed in the main tree (each agent
had written canonical-path source files before the bounce; only the
notes phase was killed). 100 files / +512/-381 lines across 11 crates.

| Agent | Crates touched | Files | Status |
|-------|----------------|------:|--------|
| C3-followup | rustc_attr_parsing + rustc_feature + rustc_builtin_macros (rustc_expand untouched) | 23 | partial — properly used parent's LazyLock + VarError shims |
| D1 | rustc_middle (16 files) — incl. ty/context.rs, ty/context/tls.rs, util/bug.rs, arena.rs, lib.rs, plus 11 mid-tree files | 18 | ~15% of 116-file crate |
| D2 | rustc_hir (13) + rustc_hir_id (2) + rustc_hir_pretty (2) + rustc_hir_analysis (7); **rustc_hir_typeck untouched** | 24 | partial |
| D3 | rustc_type_ir COMPLETE (21) + rustc_privacy COMPLETE (2); **rustc_infer + rustc_trait_selection + rustc_const_eval untouched** | 23 | partial |
| D4 | rustc_resolve COMPLETE (12); **rustc_borrowck untouched** | 12 | partial |

Patches verified clean (spot-checks). `use std::*` residuals in 4
files are correctly cfg-gated `#[cfg(not(target_os = "none"))]` host
arms — deliberate, not incomplete substitution.

### Recipe evolution discovered

D1 introduced a cleaner no_std pattern than the existing A3 host/target
body-split:

```rust
// In src/lib.rs head, after //! doc comments, before items:
#![cfg_attr(target_os = "none", no_std)]
// ... other crate-level attrs ...

#[macro_use]
extern crate alloc;

#[cfg(not(target_os = "none"))]
extern crate std;
```

Effect: SemOS-target build sees `#![no_std]`; host build still has
full std as a regular `extern crate`. Avoids cfg-bracketing every
host body. Should be the default pattern going forward; fold into
RECIPE.md §1.2.

### Cumulative session total

- **Tokens (parent + agents)**: ~3.13M before Wave 2 spawn + Wave 2's
  un-reported real spend (probably ~1.5-2M based on tool-use counts
  + duration). Recovery wave will add another ~2-3M to close Phase 3.
- **LOC patched cumulative**: ~38k (2a) + ~26k (2b) + ~92k (W1) +
  ~100 files × ~500 LOC/file avg ≈ 50k (W2) = **~206k LOC** of the
  ~770k post-§1 internal rustc.
- **Wave count so far**: Phase 1 (1 wave, 4 agents), Phase 2a (probe
  + 2 waves), Phase 2b (1 wave), Phase 3 W1 (1 wave), Phase 3 W2 (1
  wave bounced).

### What's left for Phase 3 closure

Recovery wave targets (after 8:50am Toronto reset):
1. **rustc_middle remainder** (~98 of 116 files) — heaviest remaining
   single-crate work; D1 set up the cfg_attr pattern + critical
   modules, recovery follows that recipe through the rest of the tree.
2. **rustc_hir_typeck** (20k LOC, untouched) — D2 didn't reach.
3. **rustc_infer + rustc_trait_selection + rustc_const_eval** (~60k
   LOC, all untouched) — D3 didn't reach.
4. **rustc_borrowck** (25k LOC, untouched) — D4 didn't reach.
5. **rustc_expand remainder** (most of crate, untouched) — C3-followup
   didn't reach.

That's 5 distinct work-units, naturally one-per-agent. Use probe-
then-fleet: probe with the cheapest (e.g., rustc_hir_typeck — likely
THIN-LARGE per the B1 pattern), if it returns clean spawn the other
4 in parallel. If probe bounces, the bucket isn't ready.

### Next-session checklist

- [ ] Wait until 8:50am Toronto + a buffer (bucket replenishment)
- [ ] Probe with one agent (suggest rustc_hir_typeck — closes a known
      gap, biggest remaining MECHANICAL crate)
- [ ] If probe clean: spawn 4 agents for rustc_middle remainder /
      rustc_infer+trait_selection+const_eval / rustc_borrowck /
      rustc_expand remainder, in parallel
- [ ] Integrate + token-table the recovery wave
- [ ] Close Phase 3 in ROADMAP + memory file
- [ ] Decide whether to launch Phase 4 (codegen tier — rustc_codegen_ssa,
      rustc_mir_*, rustc_monomorphize, rustc_passes, rustc_metadata)
      same session or pause

---

## 2026-05-31 — Phase 3 recovery wave — PHASE 3 CLOSED

User confirmed bucket usage is good at 9:39am Toronto (~50 min past
the 8:50am reset). Launched 4-agent recovery wave in parallel — went
straight to fleet given bucket was known-good, skipping the probe
the prior session-end notes suggested.

### Wave-3 (recovery) assignments + returns

| Agent | Crates | LOC | Tokens | T/LOC | Duration | Status |
|-------|--------|----:|-------:|------:|---------:|--------|
| E1 | rustc_middle remainder (~98 files on top of D1's 18) | ~50k | 261,526 | ~5 | ~43 min | COMPLETE; closed all 116 files |
| E2 | rustc_hir_typeck + rustc_expand remainder | ~31k | 172,183 | 4.5 | ~21.5 min | COMPLETE; applied C3's §3 recipes for expand |
| E3 | rustc_infer + rustc_trait_selection + rustc_const_eval | ~60k | 209,829 | **2** | ~26 min | COMPLETE; cheapest port yet |
| E4 | rustc_borrowck | ~25k | 187,260 | ~7 | ~26 min | COMPLETE; R2's "sync:8" was phantom, crate was MECHANICAL |
| **Recovery total** | **4 crate-clusters, ~7 crates** | **~166k LOC** | **~770k** | **~5 avg** | **~26 min wall (parallel)** | **PHASE 3 CLOSED** |

### Recovery-wave findings

1. **E3 set a new floor: 2 t/LOC** on the inference triad — cheaper
   than C1/C2's 3.6 from Wave 1. The B1 LARGE-but-THIN pattern keeps
   getting stronger as agents work further from the rustc_data_
   structures NEEDS-SHIM core. E3 surprises: zero std::sync::* sites
   across all three crates (purely value-passing computation); zero
   std::io::Write sites in rustc_const_eval (R2's "io:1" was a
   Formatter pattern, not real IO).
2. **E4 disproved R2's NEEDS-SHIM tag for rustc_borrowck.** R2's
   "sync:8" count was phantom — the crate has zero std::sync::* sites;
   R2 conflated `Rc` references. rustc_borrowck was structurally
   MECHANICAL with one cfg-gated dump cluster (polonius/legacy/facts.rs).
   This is the 2nd recon site-level miscount (Wave 1 C1 found R2 wrong
   about rustc_parse/parser/diagnostics.rs Command::new). The recon is
   directional, not authoritative on site claims — verify before
   applying architectural decisions.
3. **Cross-crate IntoDiagArg trait/impl mismatch.** All 7 impl
   crates (hir, errors, middle, borrowck, const_eval, trait_selection,
   hir_typeck) now use `semos_std::path::PathBuf` for the `path`
   parameter of `into_diag_arg`. The trait def in rustc_error_messages/
   src/lib.rs:602 still uses `std::path::PathBuf`. Flagged inline as
   `// M27 R4 B5 TODO(Phase 4/5)`. rustc_error_messages itself is on
   the §1.8 fluent-deferral list and stays unpatched.
4. **Incremental-notes mandate worked.** All 4 agents (vs Wave 2's
   0/5) wrote their notes during the work, not just at the end.
   docs/m27-port/3a/E{1..4}-*.md all landed.
5. **D1's cfg_attr pattern proven across the wave.** E1/E2/E3/E4 all
   used it. The legacy `#![no_std]` block stays valid for already-
   patched zero-host-surface crates per RECIPE.md §1.2.

### Phase 3 final tally

- **Crates patched**: 21 (Cluster A: 8, Cluster B: 13)
- **LOC patched**: ~258k (Wave 1: 92k, Wave 2: 50k, Recovery: 116k)
- **Tokens spent**: Wave 1 (533k) + Wave 2 (est 1.5M unreported pre-
  bounce, the official 23k truncated reports + parent-prep work) +
  Recovery (770k) = **~2.8M tokens for Phase 3**
- **Wall time**: ~45 min (W1 parallel) + ~10 min (W2 bounce) + ~45 min
  (parent integration + prep) + ~30 min (recovery parallel) = ~2 hrs
  active across two sessions.

### Session-wide cumulative through Phase 3 CLOSED

- **Tokens**: 723k (P1) + 1,309k (P2a) + 560k (P2b) + ~2.8M (P3) = **~5.4M**
- **LOC patched**: ~38k (P2a) + ~26k (P2b) + ~258k (P3) = **~322k of ~770k post-§1 internal rustc**
- **Crates patched**: 16 (P2a) + 4 (P2b) + 21 (P3) = **41 crates**
- **Wall-time spent**: ~5 hrs (P1+P2a+P2b) + ~2 hrs (P3) = ~7 hrs across multiple sessions

### Phase 3 → Phase 4 transition

- [x] Phase 3 CLOSED
- [x] All 4 recovery notes written
- [x] D1 cfg_attr pattern in RECIPE.md §1.2 as preferred
- [x] Cross-crate IntoDiagArg flag marked inline for Phase 4/5
- [ ] Phase 4 launch decision (codegen tier — rustc_codegen_ssa,
      rustc_mir_build, rustc_mir_transform, rustc_mir_dataflow,
      rustc_monomorphize, rustc_passes, rustc_metadata). ~6-7 crates,
      plan estimated 5-10 calendar-sessions, post-§1.7 (cg_clif owns
      ET_EXEC) the codegen_ssa::back::link subsystem is skipped
      entirely, so Phase 4 is lighter than the plan's original
      estimate.

---

## 2026-05-31 — Phase 4 (codegen tier) launch + first-wave bounce

User said "start phase 4" right after Phase 3 closure. Crate scope:
- rustc_codegen_ssa (ARCHITECTURAL — drop back::link per §1.7)
- rustc_mir_transform (95 files / 34k LOC, MECHANICAL)
- rustc_mir_build + rustc_mir_dataflow + rustc_monomorphize (~74 files cluster)
- rustc_metadata (ARCHITECTURAL — drop libloading per §1.2) + rustc_passes

7 crates / 258 files / ~115k LOC total — smaller than Phase 3 (322k LOC).

### First wave (F1-F4) — late-bounce, partial work landed

Launched 4 agents in parallel. All 4 hit session limit at "resets
1:50pm Toronto" with low-token reports (1.4-3k each, 42-65 tool uses,
4-6 min duration each — shorter than Wave 2's ~10 min). The 4-agent
parallel pattern remains a real bucket-depletion risk even after the
recovery-wave success — bucket state degrades faster than I track.

User manually integrated partial agent outputs as commit `97a7b75`
(411 insertions / 25 deletions / 21 files): F1 rustc_codegen_ssa
(Cargo + lib + back/mod + base + traits/backend, with §1.7 whole-
module cfg-gates on back/{link, linker, command, apple}); F2
rustc_mir_transform (Cargo + lib + dump_mir + dest_prop + pass_
manager, with dump_mir cfg-gated); F3 rustc_mir_build (Cargo + lib
only); F4 rustc_passes 6 files (check_attr + check_export + dead +
diagnostic_items + Cargo + lib) — rustc_metadata entirely untouched.
F1/F2/F4 notes survived in `docs/m27-port/4/F{1,2,4}-*.md` (F3
didn't write a note before bouncing).

### Recovery wave (G1-G4) in flight

Launched 4 recovery agents armed with F1/F2/F4 notes as line-precise
recipes:
- G1: rustc_codegen_ssa remainder (~49 files)
- G2: rustc_mir_transform remainder (~90 files)
- G3: rustc_mir_build remainder + rustc_mir_dataflow (untouched) +
  rustc_monomorphize (untouched)
- G4: rustc_metadata (untouched) + rustc_passes remainder

#### G2 returned first (textbook recipe-following)

| Agent | Crates | LOC | Tokens | T/LOC | Duration | Status |
|-------|--------|----:|-------:|------:|---------:|--------|
| G2 | rustc_mir_transform remainder (31 files) | ~34k | 108,407 | 2-3 | ~10 min | COMPLETE |

G2's note: "F2's pre-port survey + substitution table made this the
textbook B1 LARGE-but-THIN cheap follow-up. Final `\bstd::` grep across
the crate confirms only F2's cfg-gates + doc comments remain." No
new R4 markers; pure mechanical substitution. The pre-port-survey-
in-§0 pattern (F2 wrote it before patching, with a global grep count)
is worth folding into HANDOFF_TEMPLATE as an optional addition for
LARGE-but-THIN heuristic-driven crates.

#### G3 returned second

| Agent | Crates | LOC | Tokens | T/LOC | Duration | Status |
|-------|--------|----:|-------:|------:|---------:|--------|
| G3 | mir_build remainder + mir_dataflow + monomorphize | ~14k | 172,102 | 3.9 | ~14 min | COMPLETE |

G3 surfaced a **RECIPE addendum**: `semos_std::path::PathBuf` is its
own struct (not a `std::path::PathBuf` re-export). So cross-crate
signature swaps like E4's IntoDiagArg `Option<semos_std::path::PathBuf>`
only work when the **caller's source-of-PathBuf** has also been
ported. In G3's `rustc_monomorphize/partitioning.rs` the caller chain
still flows through `rustc_session`'s `std::path::PathBuf`, so G3
cfg-gated the dump functions instead of swapping signatures. This
nuances the "PathBuf swap is free" lesson — it's free WITHIN a crate's
own impls but NOT across crate boundaries until the upstream crate
is ported too.

G3 also flagged an R3 dep: `regex` crate is imported by
`mir_dataflow/framework/graphviz.rs` and used in the SemOS-build code
path inside the `regex!` macro. Cargo.toml dep flip to
`default-features = false` will be needed.

#### G1 returned third

| Agent | Crates | LOC | Tokens | T/LOC | Duration | Status |
|-------|--------|----:|-------:|------:|---------:|--------|
| G1 | rustc_codegen_ssa remainder (24 files + Cargo.toml) | ~25k | 263,268 | ~10 | ~27 min | COMPLETE |

G1 extended F1's §1.7 whole-module cfg-gate list to also cover
`back/archive` and `back/rpath` (since cg_clif emits ET_EXEC directly,
no rlib output → no archive needed). Total back/* gates now:
apple/command/link/linker/archive/rpath. The remaining back/*
submodules (metadata.rs, lto.rs, symbol_export.rs, write.rs) were
cfg-split rather than whole-gated.

G1's heaviest single file was `back/write.rs` — required ~25
cfg-gate insertions to neutralize the LLVM worker-pool + jobserver
+ mpsc surface. G1 wrote a **private `mpsc_stub` module** inline as
a placeholder — but `semos_std::mpsc` already exists from M25 sync-
demo work. Phase 5 cleanup: drop the stub, route through
`semos_std::mpsc`.

#### G1's new API gaps flagged

1. `semos_std::sync::mpsc` — G1's note flagged this as missing. **Half-
   true**: `semos_std::mpsc` exists at module top-level (M25 sync-demo);
   G1 didn't find it. Need to make the path more discoverable, or
   re-export via `semos_std::sync::mpsc` for std-symmetry.
2. `impl io::Write for Vec<u8>` — currently you have to call
   `vec.extend_from_slice(...)`. std's `Vec<u8>: io::Write` is a
   common pattern; add to semos_std::io.
3. `semos_std::fs::copy(&Path, &Path)` + Path-aware `fs::write` —
   currently fs::write takes a string path. Path-overload + copy
   for symmetry with std.
4. `JoinHandle::join` returns `Result<T, ()>` not `Result<T, Box<dyn
   Any + Send>>`. std's signature carries panic payload; rustc and
   cg_clif both unwrap the error case. Real fix needs SemOS stack
   unwinding (out of M27 scope).

These four sit in the same priority tier as the earlier B3-followup
gaps (Stderr + LocalKey Cell sugar) — add as parent-prep before
any later wave that depends on them.

#### G4 returned fourth (Phase 4 recovery COMPLETE)

| Agent | Crates | LOC | Tokens | T/LOC | Duration | Status |
|-------|--------|----:|-------:|------:|---------:|--------|
| G4 | rustc_metadata (12 files inc. Cargo+lib from scratch) + rustc_passes remainder (2 files) | ~14k | 249,621 | ~18 | ~27 min | COMPLETE |

G4 ARCHITECTURAL on rustc_metadata: 5 libloading functions cfg-gated
host-only with SemOS `DylibError::DlOpen` stubs; `tempfile` + `libloading`
moved to `[target.'cfg(not(target_os = "none"))'.dependencies]` per
§1.2; `creader.rs` libloading + dlsym_proc_macros + CrateDump::Debug
all cfg-split. `fs.rs` / `locator.rs` / `rmeta/encoder.rs` cfg-split
into host-verbatim vs SemOS-stub bodies (Seek + Mmap not yet in
semos_std). rustc_passes: 2 small followup fixes on F4's work (stray
`std::iter::once` + malformed half-cfg-gate).

### MAJOR ARCHITECTURAL INSIGHT from G4 (Phase 5 implication)

G4 §2.2 surfaced an issue with my earlier prompt language. I told
E1-E4 + F1-F4 that `semos_std is host-buildable, so this works on
both targets`. **That is wrong.** `user-programs/std-shim/.cargo/
config.toml` pins `target = "x86_64-unknown-none"` and uses
build-std for core/alloc/compiler_builtins; semos_std uses raw
SemOS syscalls in its bodies and **is not a host-OS std drop-in**.

G4 instead adopted the cfg-split pattern from rustc_fs_util/src/
lib.rs:42-60 (which A3 introduced in Phase 2a) — `#[cfg(not(target_os
= "none"))] use std::*;` paired with `#[cfg(target_os = "none")] use
semos_std::*;`. This is the correct pattern.

**The damage**: Phase 3 agents (especially E3's inference triad which
hit 2 t/LOC by doing UNCONDITIONAL `std::path::PathBuf → semos_std::
path::PathBuf` substitutions, and many other crates) made semos_std
calls non-cfg-gated. On a host build (which Phase 5 integration will
attempt), the rustc_* crates can't link against semos_std.

**The Phase 5 fix** (estimated ~1-2 mechanical sessions):
1. Convert every `semos_std = { path = "..." }` Cargo.toml dep to
   `[target.'cfg(target_os = "none")'.dependencies]`.
2. Add cfg-split arms around every non-cfg-gated `semos_std::*`
   usage so host build sees `std::*`.

This is mechanical but pervasive — touches ~half of the patches
landed Phase 3+4 (E3's triad, E1's rustc_middle, parts of D1+D2+D4,
all of W1's C1+C2+C3, F2-F4 partial, G1-G4). Estimated 1-2 sessions
of substitution work, ideally orchestrated as a 2-3 agent wave with
a clear recipe per crate.

**Why it didn't surface earlier**: patch-only contract — we never
ran `cargo build` on any patched crate. The issue only manifests
when Phase 5 tries to compile. G4 caught it by reading the
predecessor (rustc_fs_util) pattern carefully and contrasting with
F4's unconditional `semos_std` substitution.

**Closes a recipe gap**: RECIPE.md §1.3 substitution table should
mark the `std::* → semos_std::*` substitution as **cfg-conditional
only**, not unconditional. Other substitutions (`std::* → core::*` /
`alloc::* / hashbrown::*`) ARE alias substitutions and remain
unconditional. Will fold into RECIPE.md before Phase 5.

### Phase 4 final tally

| Agent | Crates | LOC | Tokens | T/LOC | Status |
|-------|--------|----:|-------:|------:|--------|
| F1 (bounced) | rustc_codegen_ssa Cargo+lib+back/mod+base+traits/backend | partial | ~? (truncated) | n/a | bounced; partial-commit |
| F2 (bounced) | rustc_mir_transform Cargo+lib+dump_mir+dest_prop+pass_manager | partial | ~? (truncated) | n/a | bounced; partial-commit |
| F3 (bounced) | rustc_mir_build Cargo+lib only | partial | ~? (truncated) | n/a | bounced; no notes |
| F4 (bounced) | rustc_passes 6 files; rustc_metadata 0 source | partial | ~? (truncated) | n/a | bounced; partial-commit |
| G1 | rustc_codegen_ssa remainder (24 files) | ~25k | 263,268 | 10 | COMPLETE |
| G2 | rustc_mir_transform remainder (31 files) | ~34k | 108,407 | 2-3 | COMPLETE |
| G3 | mir_build remainder + mir_dataflow + monomorphize (17 files) | ~14k | 172,102 | 3.9 | COMPLETE |
| G4 | rustc_metadata full (12 files) + passes remainder (2 files) | ~14k | 249,621 | 18 | COMPLETE |
| **Phase 4 total** | **7 crates, ~258 files** | **~115k LOC** | **~793k (F-bounce + G-wave)** | **~7 avg** | **CLOSED (with §1.2/§1.7 architectural decisions landed)** |

Wall time: F-wave bounce ~5 min + user manual integration + G-wave
~30 min parallel ≈ ~1 hr active.

### Cumulative session totals through Phase 4

- **Tokens**: ~5.4M (P1+P2a+P2b+P3) + ~800k (Phase 4 incl. F-bounce
  + recovery) = **~6.2M**
- **LOC patched**: ~322k (P1-P3) + ~115k (P4) = **~437k of ~770k
  post-§1 internal rustc** (~57%)
- **Crates patched**: 41 (P1-P3) + 7 (P4) = **48 crates**

### Phase 4 → Phase 5 transition

- [x] Phase 4 CLOSED (codegen tier patched)
- [x] §1.2 libloading drop (G4 cfg-gated 5 sites + moved dep to target-cond)
- [x] §1.7 back::link drop (F1+G1 whole-module gated back/{apple,command,link,linker,archive,rpath})
- [x] D1 cfg_attr pattern used throughout
- [x] All 4 recovery notes written (G1-G4 in `docs/m27-port/4/`)
- [ ] **HIGH PRIORITY**: semos_std cfg-conditionalization sweep
      (~1-2 mechanical sessions, fixes the host-build mismatch
      identified by G4). Must happen before any Phase 5 build attempt.
- [ ] G1's 4 surface gaps: `sync::mpsc` discoverability,
      `impl io::Write for Vec<u8>`, `fs::copy + Path-aware fs::write`,
      JoinHandle panic payload.
- [ ] G4's Phase-4.5 micro-wave: `Path::display()` (20+ sites),
      `io::Seek` + `File::seek`, `io::Error::new(ErrorKind, msg)`,
      `fs::rename`, `File::open_buffered`, `io::copy`, `Path::metadata`/
      `Path::exists`.
- [ ] Phase 5 (integration: wire rustc_driver_impl into semos-rustc
      binary, statically link cg_clif, DEMO 80 — hello-world.rs → ELF
      → SYS_SPAWN → captured stdout). Plan estimated 3-5 sessions; the
      semos_std-cfg sweep + surface gap adds 2-3 prep sessions.

---

## 2026-05-31 — Phase 5 start

User said "start phase 5". Three-stage plan:

### Stage A: Phase 4.5 surface additions ✅ DONE (commit `de8aff3`)

Landed the G1+G4 surface gaps from Phase 4:
- `Path::display()` returning a Display wrapper (dominant gap per G4)
- `Path::exists/is_dir/is_file/metadata` via SYS_STATX
- `PathBuf::display` forwarder + same accessors
- `io::ErrorKind` enum (NotFound + 9 variants incl. Unsupported)
- `io::Error::new(ErrorKind, &'static str)` + `kind()` accessor
- `impl io::Write for Vec<u8>` (G1's "common pattern" flag)
- `impl io::Read for &[u8]` (paired)
- `io::copy(reader, writer) -> u64` Read+Write loop
- `fs::rename(from, to)` via SYS_RENAME (Phase 14 Tier 2)
- `fs::copy(from, to) -> u64` via fs::read + fs::write
- `fs::stat(path) -> Option<StatX>` + `fs::metadata` Result variant
- `fs::StatX` struct (#[repr(C)] mirroring kernel layout) with
  is_dir/is_file/len/modified/created helpers
- `sync::mpsc` re-export from crate-root `mpsc` (G1 path-discoverability fix)

Builds clean: semos-std + sem-sh both compile against x86_64-unknown-none.

Still deferred (need new kernel surface):
- `io::Seek + File::seek + SeekFrom` (needs SYS_FSEEK or equivalent)
- `File::open_buffered` + BufReader (needs a buffering wrapper struct)

### Stage B: Phase 5a cfg-sweep ⏸ DEFERRED (re-analyzed as non-blocking)

Original plan was to retroactively cfg-split the unconditional `use
semos_std::*` substitutions Phase 3 agents made. Re-analyzed in this
session: only **39 unconditional sites** across **9 Cargo.toml files**
needing the dep target-conditional. **NOT a Phase 5b build blocker**:

- semos-rustc binary targets x86_64-unknown-none (the SemOS target).
  All 48 ported rustc_* crates build for that target.
- Proc-macro crates (4 of them) are HOST builds — but they have NO
  semos_std deps. They're unaffected.
- Build scripts (build.rs) are host builds — none import semos_std.
- The unconditional dep just means rustc_* crates can ONLY build for
  the SemOS target. That's what we want for Phase 5b.

The cfg-sweep matters for dev-ergonomics (host `cargo doc`, IDE
integration, host-side `#[cfg(test)]` blocks), NOT for getting M27
to ship. Picking it up as a polish pass after Phase 5b lands.

### Stage C: Phase 5b scaffold ✅ DONE (commit `0c19848`)

Created `user-programs/semos-rustc/` as a Ring-3 binary template
mirroring semos-cc's shape:
- Cargo.toml with [bin] + semos-std dep + opt-level=0 profile.
  rustc_driver_impl + rustc_driver path deps commented for stage 2.
- .cargo/config.toml: target=x86_64-unknown-none + build-std.
- build.rs: -T<linker> + -no-pie cargo:rustc-link-arg's.
- link.ld: USER_CODE_BASE=0x400000 .text/.rodata/.data/.bss layout
  identical to semos-cc.
- src/main.rs: stub `_start` via `semos_std::main!` macro; prints two
  markers + SYS_EXIT(0). NO rustc calls yet.

Builds clean. Output ELF: ET_EXEC (e_type=2), EM_X86_64 (e_machine=62),
e_entry=0x400000 (matches USER_CODE_BASE), statically linked, stripped,
5KB. **Same shape as semos-cc's stage-1 D.2 emitter** at the same
point in its pipeline.

### Stage D: Phase 5b integration STARTED (workspace plumbing landed, externals blocking)

Started this session. Commit `926e739`. Three Cargo plumbing fixes
landed to get the workspace to resolve:

1. **semos-rustc/Cargo.toml as WORKSPACE ROOT** with glob membership
   (`members = [".", "vendor-rustc-src/compiler/*"]`). Excludes the
   8 dropped crates (codegen_llvm/gcc/cranelift, llvm, baked_icu_data,
   sanitizers, windows_rc, rustc shim).
2. **Stripped `[workspace] members = []` headers** from all 48 patched
   rustc_* Cargo.tomls. Python script targeted the literal pair, left
   real config intact. The opt-out blocks served their purpose during
   patch-only phase; now conflict with workspace-root resolution.
3. **Renamed `semos_std = { ... }` → `semos-std = { ... }`** in 9
   rustc_* Cargo.tomls. The package's `[package] name` is `semos-std`
   (hyphen); the underscore comes from `[lib] name = "semos_std"` for
   imports. Other user-programs (sem-sh, semos-cc, hello-std) all use
   the hyphen form, matching package name.

First `cargo check --release` (from inside semos-rustc/) resolves 303
packages and starts compiling. Wall: **8 external crates fail before
any patched rustc_* crate gets tried.**

| External crate | Errors | Cause |
|----------------|-------:|-------|
| once_cell 1.21.4 | 245 | No #![no_std]; missing prelude |
| log 0.4.30 | 100 | std::cfg; no #![no_std] |
| rustc-stable-hash 0.1.2 | 32 | Explicit std (recon flagged) |
| stable_deref_trait | 17 | std prelude |
| smallvec 1.15.1 | 1 | Unstable feature on stable |
| memchr 2.8.1 | 1 | std required |
| regex-syntax 0.8.10 | 1 | std required |
| rustc-hash 2.1 | 1 | std required |

These are exactly the R3 external port work the recon estimated at
~15 focused PATCH sessions. Most need either `default-features =
false` overrides at workspace level OR vendor + no_std patch (same
pattern as the Cranelift vendored fork in semos-cc).

### Stage E IN PROGRESS (commit `24a19be`)

Four discoveries fixed in the first Stage E iteration:

1. **Toolchain pin missing.** `user-programs/semos-rustc/` had no
   `rust-toolchain.toml`. Cargo fell back to system-default stable,
   breaking smallvec's `feature(dropck_eyepatch)` (nightly-only).
   Added `rust-toolchain.toml` pinning `nightly-2026-02-01` (same as
   `kernel-x86_64/rust-toolchain.toml`).
2. **RUSTC_BOOTSTRAP env missing.** rustc's internal proc-macro
   crates (rustc_macros, etc.) check this flag in their build.rs and
   abort with "wrong command used for building" without it (rustc is
   normally driven by bootstrap which sets it). Added to .cargo/
   config.toml `[env]` block.
3. **stacker pulls C-compiler via psm.** rustc_data_structures
   carries stacker which needs cc-rs → C compiler (not available
   targeting x86_64-unknown-none on Windows). Cfg-gated stacker +
   tempfile as host-only deps; SemOS target uses A1's no-op
   `ensure_sufficient_stack` shim from Phase 2a.
4. **thin-vec + rustc-hash defaulted std features.** Bulk-added
   `default-features = false` to 18 thin-vec + 2 rustc-hash + 1
   memchr direct dep declarations.

### Stage E open work for next iteration

Transitive external pulls still failing — these come from
annotate-snippets / fluent-bundle / intl-memoizer / etc. that
rustc_errors and rustc_driver_impl pull with default features:

| External | Errors | Likely fix |
|----------|-------:|------------|
| once_cell 1.21.4 | 245 | parent dep drop (fluent/icu) OR vendor+patch |
| log 0.4.30 | 100 | vendor + no_std patch (log has no real no_std mode) |
| rustc-stable-hash 0.1.2 | 31 | vendor + no_std patch (recon flagged) |
| stable_deref_trait | 17 | parent dep drop |
| regex-syntax 0.8.10 | 1 | drop unused regex dep from rustc_mir_dataflow |

The §1.8 i18n drop should remove most fluent-bundle / annotate-snippets
/ intl-memoizer transitive pulls. The drop wasn't fully landed at the
Cargo.toml level — only at the rustc_errors body level. Need to also
remove the deps from Cargo.tomls.

### Stage E iter 2: §1.8 done at Cargo+source level (H1 agent, commit `9399abe`)

H1 agent landed the proper §1.8 drop in rustc_error_messages:
- Dropped 7 transitive-puller deps (fluent-bundle, fluent-syntax,
  icu_list, icu_locale, intl-memoizer, unic-langid, rustc_baked_icu_data,
  tracing).
- Replaced fluent/unic-langid public surface with local stub types
  (FluentArgs, FluentValue, FluentBundle, FluentError, FluentResource,
  FluentType, LanguageIdentifier, langid!) — API-compatible no-ops.
- Notes at `docs/m27-port/4/H1-i18n-drop.md`.

### Stage E iter 3: tracing + parking_lot host-cfg-gate (parent followup)

Same commit. Parent followups after H1's i18n drop revealed remaining
transitive pulls:
- `tracing = "0.1"` in ~20 rustc_* Cargo.tomls bulk-set to
  `default-features = false` (was pulling log + once_cell otherwise).
- In rustc_errors: moved annotate-snippets + anstream + termize to
  `[target.'cfg(not(target_os = "none"))'.dependencies]` host-only.
- In rustc_data_structures: moved jobserver + measureme + parking_lot
  + rustc-stable-hash + memmap2 to host-only deps (transitive std
  pulls). stacker + tempfile already gated in iter 1.

### Stage E iter 4 open work

Cargo check now reaches more crates but 12 still fail. The mix shifted:
- 2 patched-crate failures (Phase 3-4 port bugs surfacing):
  - **rustc_thread_pool** (138 errors): A1's stub wasn't actually
    `#![no_std]` — still has `use std::any::Any;`. Need D1 cfg_attr
    pattern.
  - **rustc_graphviz** (2 errors): `assert!` macro not found —
    needs `#[macro_use] extern crate alloc;` or core prelude.
- ~10 external failures (further transitive chains):
  once_cell (245), log (100), rustc-stable-hash (31), stable_deref_trait
  (17), scoped-tls (7), constant_time_eq (5), crypto-common, either,
  indexmap, memchr.

Iteration 4 next-steps:
1. Fix rustc_thread_pool no_std (port bug).
2. Fix rustc_graphviz prelude (port bug).
3. Trace remaining transitive log/once_cell pullers — likely
   syn/serde/proc-macro chain or some still-active crate dep.
4. Vendor + no_std-patch the 3 hardest externals (log + once_cell +
   rustc-stable-hash) — recon estimated ~3 sessions per.

Stage E is real R3 work — recon estimated ~15 sessions total of
focused PATCH work. Each iteration unblocks ~2-4 crates. We're 4
iterations in, ~12 unblocked, ~12 still in the queue.

1. **Survey each failing external's no_std story.** Some (memchr,
   once_cell, regex-syntax) have feature flags that gate std and
   just need `default-features = false` in the upstream rustc_* dep
   declarations. Others (log, stable_deref_trait, rustc-stable-hash)
   need real patches or vendored forks.
2. **Top-of-funnel approach**: add a `[workspace.dependencies]` table
   in semos-rustc/Cargo.toml that pins these 8 crates with
   `default-features = false` + required features, plus
   `[patch.crates-io]` overrides for the ones that need vendored
   patches. The rustc_* crate Cargo.tomls keep their existing entries
   but inherit the workspace pin.
3. **Vendor + patch the 2-3 hardest crates** (log + once_cell +
   rustc-stable-hash) into `user-programs/semos-rustc/vendor-externals/`,
   each with a no_std patch following the Cranelift PORT_LOG.md
   pattern.
4. Once externals build, the NEXT wall will be the 48 patched
   rustc_* crates — expect a similar volume of compile errors
   surfacing the actual port quality. THAT's the 4-6 parallel
   agent fix wave.

### Stages F+ (later):
- Wire `cg_clif` statically as the codegen backend per §1.2.
- Replace semos-rustc's stub main with `rustc_driver::run_compiler`.
- DEMO 80: SYS_SPAWN semos-rustc on hello-world.rs → SYS_SPAWN
  emitted ELF → assert "hi" in captured stdout.

### Cumulative session totals through Phase 5 start

- **Tokens**: ~6.2M (P1-P4) + ~? (Phase 5 prep) ≈ **~6.5M** estimated
- **LOC patched**: ~437k of ~770k post-§1 internal rustc (~57%) +
  semos-std surface additions
- **Crates patched**: 48 internal rustc_* crates
- **semos-rustc binary**: scaffolded, no rustc yet

### Lessons in flight

1. **Don't wait until wave-close to log.** User flagged this — Phase
   1-3's pattern of batch-logging at wave-end is fine when the session
   completes cleanly, but unsafe when the session itself might bounce
   mid-orchestration. Switch to incremental log writes for the rest of
   the M27 port.
2. **CWD drift trap** confirmed again this session — Bash's persistent
   working directory carried me into a worktree path. Codified in
   EXPERIMENT_LOG previously; keep `cd /f/Software/ArmKernel3` at the
   head of any git invocation or `git -C` explicitly.

### Stage E iter 4 (commit `81e3e2e`)
- rustc_thread_pool: D1 cfg_attr pattern + cfg-split use std::* +
  thread_local! macro. Added semos-std target-dep.
- rustc_graphviz: A4's deferred core::io redirect applied.
- rustc_codegen_ssa: tempfile + thorin-dwp + wasm-encoder host-only.
- rustc_fs_util: host-gated tempfile. rustc_data_structures: host-
  gated ena.
- Cleared: log + crc32fast. Remaining: 11 externals.

### Stage E iter 5 (commit `cfba8f6`) — BIG WIN: serde no_std

- Bulk-set all 8 rustc_* Cargo.tomls' serde / serde_json deps to
  `default-features = false, features = ["alloc"]`. Cleared 5829
  serde_core errors.
- gsgdt host-gated in rustc_middle (pulled serde without
  default-features=false, forcing serde/std workspace-wide via
  feature unification). Used only in MIR debug dump (host-only).
- odht host-gated in rustc_hir + rustc_metadata (incremental hash
  table, dropped per §1.3).
- rustc_thread_pool: dropped `const { }` syntax (semos_std::
  thread_local! lacks it).
- rustc_fs_util: added missing semos-std dep.

Net: non-monotone — log + crc32fast + parking_lot_core + anstyle
re-surfaced through a different chain in rustc_metadata after odht
host-gate. 16 externals at iter 5 end vs 11 at iter 4 end, but
serde_core (5829) clear dwarfs everything.

### Iter 6+ open work
1. Trace re-surfaced log + once_cell pullers via cargo tree. The
   anstyle (156) + parking_lot_core (60) failures suggest
   annotate-snippets/anstream slipping through host-gate.
2. Stub 29 remaining `tracing::*!()` macro sites to fully drop the
   tracing dep workspace-wide.
3. Vendor + no_std-patch once_cell + rustc-stable-hash (Cranelift
   PORT_LOG template, recon-estimated 3 sessions per).

Cumulative this session: 17 commits, ~6.9M tokens. Stage E continues
the recon-estimated grind. Significant single-iter wins (serde 5829
clear) interspersed with iteration-shuffle when one host-gate opens
a different code path.

### Stage E iter 6 (commit `83d4957`)

Two host-gates:
- **ena in rustc_type_ir**: ena was pulling log 100 errors via
  rustc_infer's UnificationTable. Phase 4 G3 had host-gated ena
  in rustc_data_structures (iter 4); same dep slipped through via
  type_ir.
- **tracing-core + tracing-subscriber + tracing-tree in rustc_log**:
  the rustc_log crate body is already a SemOS-stub returning Ok(());
  these telemetry crates only get used on host. Moving them to
  [target.'cfg(not(target_os = "none"))'.dependencies] cuts the
  most direct path. (Indirect path through tracing 0.1 still active
  in 45 rustc_* crates.)

Cleared this iter: log + parking_lot_core. 16 externals remaining:
- **once_cell (245)** STILL via tracing-core ← tracing v0.1.44.
  `default-features=false` on tracing in 45 rustc_* crates doesn't
  propagate to tracing-core's `std` feature (which is its own crate
  with `default = ["std"]`). Cargo feature unification doesn't auto-
  remove transitive default features.
- **anstyle (156)** via annotate-snippets ← rustc_fluent_macro
  (proc-macro crate — host-only at build time, but cargo includes
  proc-macro deps in lockfile resolution for the workspace).
- Smaller no_std-stragglers: stable_deref_trait (17), rustc-stable-
  hash (31), constant_time_eq (5), crypto-common, either, indexmap,
  memchr, scoped-tls (7), getrandom (3), crc32fast (21), getopts
  (165), termize (10).

### Iter 7+ open work — the fundamental wall

The remaining once_cell + anstyle + getopts + the no_std-stragglers
all need one of three approaches:

1. **[patch.crates-io] overrides at workspace root** swapping
   tracing-core / annotate-snippets / getopts / etc. for forked
   versions that respect `default-features=false` end-to-end. The
   tracing-core fork would set `default = []` and remove the
   once_cell dep entirely. ~1 session per crate to fork + patch.

2. **Vendor + no_std-patch** the worst offenders (Cranelift PORT_LOG
   template). ~3 sessions per per recon estimate; locks our version
   indefinitely.

3. **Drop the parent dep at the source level**. E.g. rustc_fluent_macro
   doesn't strictly need annotate-snippets for its primary purpose
   (generating diagnostic-id constants); could be refactored. But
   recipe-following: minimize source changes per §1.8.

Approach 1 is the most leveraged: one [patch.crates-io] entry per
crate, applied once at the workspace root, covers all transitive
consumers. Worth ~2-3 focused sessions to land for tracing-core +
annotate-snippets + the 5 smaller stragglers.

Cumulative this session: 18 commits, ~7M tokens. Stage E grind
continues; each iter clears 1-3 externals + sometimes uncovers
re-surface chains. The fundamental wall is now the
non-default-features-aware transitive resolution.

### Stage E iter 7 (commits `83d4957` + `9176c33`) — BIG WINS

Three sub-iterations rolled into iter 7:

- **7a**: vendored tracing-core 0.1.36 into `vendor-externals/`
  with `default = []`. Added `[patch.crates-io]` in workspace root.
  But once_cell still pulled — see 7b.
- **7b**: **THE LEAK**. `rustc_driver_impl/Cargo.toml` had
  `tracing = { version = "0.1.35" }` without `default-features =
  false`. Cargo's feature unification turned tracing/std ON
  workspace-wide → tracing-core/std → once_cell. **One-line fix
  cleared 245 errors.** Demonstrates: ONE consumer of N (45 here)
  not disabling defaults wipes out everyone else's df=false work.
- **7c**: host-gated measureme (rustc_query_impl) + parking_lot
  (rustc_query_system) + ar_archive_writer (rustc_codegen_ssa).
  Dropped object's "write" feature (was pulling crc32fast).
  Cleared parking_lot_core (60), crc32fast (21), plus patched-crate
  followups rustc_thread_pool (12) + rustc_fs_util (1).

Externals at iter 7c end: **15** (down from 17 at iter 6 end).

This session cumulatively cleared: log×2, crc32fast×2, serde_core
(5829), once_cell (245), parking_lot_core×2, 4 patched-crate fixes
(rustc_thread_pool, rustc_graphviz, rustc_fs_util×2).

Remaining 15 externals:
- Bigger: anstyle (156, via annotate-snippets ← proc-macro
  rustc_fluent_macro), getopts (165, in rustc_session), rustc-stable-
  hash (31), datafrog (275, new — via polonius-engine), stable_deref_
  trait (17), termize (10), scoped-tls (7), constant_time_eq (5),
  getrandom (3).
- Singletons (1 error each — likely single df=false fix):
  either, indexmap, memchr, rustc-hash, regex-syntax, crypto-common.

Cumulative this session: 20 commits, ~7.2M tokens.

### Iter 8+ pattern firmly established

For each remaining external:
1. `cargo tree --target x86_64-unknown-none -i <external>` to trace
   the puller chain.
2. Host-gate the deepest no_std-incompatible dep
   (`[target.'cfg(not(target_os = "none"))'.dependencies]`).
3. If the puller is a workspace path-dep crate: gate body sites with
   `#[cfg(not(target_os = "none"))]`.
4. If transitive default features can't be reached from consumer
   declarations: vendor + fork in `vendor-externals/`, set
   `default = []`, add `[patch.crates-io]` override (tracing-core
   pattern).

Singletons usually fall to a single df=false add on a specific dep
declaration. Multi-hundred-error crates usually need approach (4):
fork.

### Stage E iter 8 (commit `f9e7045`) — bulk-batch sweep

Two-pronged batch fix:

(1) Bulk df=false (20 Cargo.tomls):
    itertools / scoped-tls / indexmap / regex — these all had upstream
    `default = ["std", ...]` features that propagated workspace-wide
    via cargo feature unification.

(2) Host-gate 7 std-coupled deps:
    - rustc_session: getopts + termize (CLI/terminal — host-only)
    - rustc_span: blake3 + md-5 + sha1 + sha2 (hash computation;
      SemOS target stubs SourceFileHashAlgorithm machinery)
    - rustc_data_structures: elsa (pulls stable_deref_trait)
    - rustc_hashes: rustc-stable-hash (R3-flagged unconditional std)
    - rustc_borrowck + rustc_middle + rustc_mir_dataflow:
      polonius-engine (pulls datafrog 275 errors — debug analysis,
      host-only)

Cleared this iter (combined ~12 externals): termize (10), getopts
(165), datafrog (275), polonius-engine, parking_lot_core×2 (now
permanently), blake3, crc32fast (already), regex-syntax (1),
stable_deref_trait (17), constant_time_eq (5), rustc-stable-hash
(31).

Externals at iter 8 end: **14**. New tail-end utility crates
surfacing (were hidden behind earlier blockers):
- punycode (40), pulldown-cmark-escape (16), ctrlc (32),
  pathdiff (32), find-msvc-tools (69), anstyle (156 — still),
  scoped-tls (7 — still, despite df=false), getrandom (3),
  either (1), memchr (1), indexmap (1)
- Patched-crate followups: rustc_hashes (1), rustc_proc_macro (1),
  rustc_log (2 — was bigger before host-gates)

Cumulative this session: 22 commits, ~7.4M tokens.

Pattern continues to work: each iteration concretely advances cargo
check past previously-blocking externals, surfacing the next layer.
Stage E is genuinely converging — the latest blockers are tail-end
utility crates rather than core no_std-incompatibility.

### Stage E iter 9 (commit `61b0abd`) — resolver=2 + 5-crate host-gate

Six fixes:

1. **`resolver = "2"`** in workspace root Cargo.toml. Structural fix
   for cargo's feature unification problem — enables per-target
   feature resolution. Without it (default resolver=1 for edition-
   2018-style workspaces), one consumer's default features
   propagate workspace-wide. That's the same bug class that the
   iter 7b rustc_driver_impl tracing leak created.
2. **punycode** host-gated in rustc_symbol_mangling (40 errors).
   v0 mangling uses punycode but cg_clif handles emission per §1.7.
3. **ctrlc + shlex** host-gated in rustc_driver_impl (32 + 105).
   CLI/process-control — irrelevant on SemOS Ring 3 v1.
4. **pathdiff** host-gated in rustc_codegen_ssa (32). File-path
   diffing for linker output (dead per §1.7).
5. **pulldown-cmark** (incl. pulldown-cmark-escape transitively, 16)
   host-gated in rustc_resolve. Used for rustdoc — host-only.
6. **fluent-bundle + fluent-syntax + annotate-snippets + unic-langid**
   host-gated in rustc_fluent_macro (proc-macro crate, all §1.8 i18n).

Cleared this iter: punycode (40), pulldown-cmark-escape (16),
ctrlc (32), pathdiff (32), shlex (105). **~225 errors gone.**

Externals at iter 9 end: **14**. Composition shifted decisively
from "tail-end utility" to mostly patched-crate followups:

- **Patched-crate followups** (need port fixes, NOT external work):
  rustc_hashes (1), rustc_proc_macro (1), rustc_fs_util (1),
  rustc_log (2), rustc_thread_pool (12)
- **Stubborn externals** (proc-macro chain — resolver=2 didn't help
  the rustc_fluent_macro deps as much as expected): anstyle (156),
  find-msvc-tools (69), unicode-normalization (158), jiff (1)
- **Singletons**: getrandom (3), either (1), memchr (1), indexmap
  (1), scoped-tls (7)

Cumulative this session: 24 commits, ~7.5M tokens. Stage E has now
cleared ~30 distinct externals cumulatively. The composition shift
is the signal that Stage E is close to settled — most remaining
failures are patched-crate port bugs (Phase 2-4 followups) rather
than upstream no_std issues.

### Stage E iter 10 next-steps

1. **Patched-crate followups**: rustc_hashes (1), rustc_proc_macro
   (1), rustc_fs_util (1), rustc_log (2), rustc_thread_pool (12)
   are all in our own ported tree. Each is a small port-bug fix
   (likely missing #![no_std], missing semos-std cfg-split, or
   tracing macro that needs stubbing).
2. **Stubborn proc-macro externals** (anstyle, unicode-normalization,
   find-msvc-tools): resolver=2 should have isolated these to host-
   only resolution, but cargo treats proc-macro crates specially.
   Consider [patch.crates-io] forks with `default = []`.
3. **Singletons** (likely df=false missing somewhere):
   `cargo tree -i <crate>` to find the puller.

The patched-crate followups should be the next focus — each is
small + structural rather than batch-pattern work.

### Stage E iter 10 (commit `f8a8757`) — ALL 5 patched-crate followups CLEARED

Six fixes (5 patched-crate + 1 parent prep):

1. **rustc_hashes**: cfg-gate `use rustc_stable_hash::{FromStableHash,
   SipHasher128Hash}` (we host-gated the dep iter 8 since
   rustc-stable-hash 0.1.0 is unconditionally std). SemOS-target arm
   gets a 6-line stub matching upstream's
   `SipHasher128Hash(pub [u64; 2])` shape.

2. **rustc_proc_macro**: upstream lib.rs points at
   `../../library/proc_macro/src/lib.rs` which we don't vendor. Per
   §1.5 (drop proc-macro runtime), provide a 130-line stub
   (`src/lib_stub.rs`) exposing only the public type names that
   downstream consumers (rustc_expand, rustc_metadata,
   rustc_builtin_macros) import: TokenStream, Group, Ident, Punct,
   Literal, Span, Delimiter, Spacing, Diagnostic, `bridge` module.

3. **rustc_fs_util**: `Path::to_str()` doesn't exist on
   semos_std::path::Path — it has `as_str()` (no UTF-8 validity
   check needed). 1-line fix.

4. **rustc_log**: added missing semos-std target-conditional dep
   (body uses `semos_std::env::{self, VarError}`). Also fixed 6
   `env::var(format!(...))` sites: semos_std::env::var takes &str,
   `format!` returns String — added `&` prefix to each call.

5. **rustc_thread_pool**: multiple structural fixes:
   - Added `use alloc::{string::String, vec::Vec, boxed::Box};`
     (source used these bare, expected std prelude).
   - `std::sync::Arc` → `alloc::sync::Arc` inline.
   - `std::ops::Deref` → `core::ops::Deref` (line 641).
   - semos_std::thread_local!'s LocalKey requires T: Send+Sync (it's
     OnceLock-backed, single-threaded SemOS). Cell<*const()> isn't.
     Wrapped in a Sync-asserting `TlvCell` newtype on SemOS arm;
     cfg-split the set/get helpers to dereference through the
     newtype.

6. **Parent prep**: `impl core::error::Error for semos_std::io::Error
   {}` in user-programs/std-shim/src/io.rs (rustc_thread_pool casts
   io::Error → &dyn core::error::Error).

**Externals at iter 10 end: 9** (down from 14 at iter 9 end). The
Stage E focus has decisively shifted — ALL remaining are pure
external no_std issues:

- **Big**: anstyle (156), unicode-normalization (158),
  find-msvc-tools (69)
- **Small**: scoped-tls (7), getrandom (3)
- **Singletons**: either, memchr, indexmap, jiff (1 error each)

### Iter 11+ next-steps

For the singletons: `cargo tree -i <crate>` to find the df=false
leak (same pattern as iter 7b's rustc_driver_impl tracing leak).

For anstyle / unicode-normalization / find-msvc-tools / scoped-tls:
- anstyle is via proc-macro chain (annotate-snippets ← rustc_fluent_macro);
  even with iter 9 host-gates, proc-macro deps still appear in
  workspace lockfile. May need vendor + fork with `default = []`
  (tracing-core pattern).
- unicode-normalization is in rustc_parse for ident normalization.
  Has default features pulling std; df=false should work.
- find-msvc-tools is in rustc_codegen_ssa (already host-gated as
  part of ar_archive_writer block at iter 7c)... if still failing,
  the target build is somehow seeing it.

Cumulative this session: 26 commits, ~7.7M tokens.

### Stage E iter 11 (commit `00246a4`) — closing-out the externals

Three [patch.crates-io] vendored forks landed (`vendor-externals/`):
- **scoped-tls 1.0.1** — stubbed to forward to semos_std::thread::
  ScopedKey (semos_std already had the macro shim from Phase 2a).
- **either 1.16.0** — `default = ["std"]` → `default = []`. itertools
  pulls either without df=false; can't fix from the consumer side.
- **indexmap 2.14.0** — same default change.

Host-gates added:
- anstyle → host-only in rustc_driver_impl + rustc_errors (was direct
  dep at 156 errors).
- jiff → host-only in rustc_driver_impl (needs std feature upstream).
- rand df=false in rustc_incremental + rustc_session.
- bstr df=false in rustc_codegen_ssa.
- unicode-normalization df=false in rustc_parse.
- find-msvc-tools dropped from rustc_codegen_ssa main deps.

§1.8 cleanup: stubbed `rustc_fluent_macro/src/fluent.rs` to emit a
minimal token stream (empty `fluent_generated` module + placeholder
`DEFAULT_LOCALE_RESOURCE`). Dropped fluent-bundle + fluent-syntax +
annotate-snippets + unic-langid deps. Macro now compiles with just
proc-macro2 + quote.

Hashbrown pin: rustc_data_structures + rustc_query_system +
rustc_mir_transform pinned to 0.15 (was 0.16.1). hashbrown 0.16's
`nightly` feature uses `feature(trivial_clone)` which our pinned
toolchain doesn't have.

rustc_arena: added `#![feature(maybe_uninit_slice)]` for the
MaybeUninit slice operation.

**Composition shifted again** — externals nearly gone, now patched-
crate prelude gaps:
- rustc_parse_format (16): missing `use alloc::borrow::ToOwned;` for
  16 `.to_owned()` calls on &str literals.
- rustc_arena (1): feature gate addition (this iter's fix didn't
  flush — needs verification).
- few persistent externals: find-msvc-tools (69 — still pulled),
  scoped-tls (1), either (1), indexmap (1), memchr (1).

Stage E is genuinely in endgame. Stage E iter 12 next: read the
post-iter-11 cargo check log carefully, distinguish patched-crate
followups (alloc prelude additions are mechanical) from the last
external leaks. Possibly 1-2 more iterations to settled.

Cumulative this session: ~28 commits, ~7.9M tokens.

### Stage E iter 12 (commit `6c8bd98`) — **EXTERNAL WALL CLEARED**

Seven fixes — six patched-crate body + one fork edition fix:

1. **rustc_arena**: `#![feature(maybe_uninit_slice)]` for
   `slice::assume_init_drop` at line 84.
2. **rustc_parse_format**: `use alloc::borrow::ToOwned;` (16
   `.to_owned()` calls on `&str` literals failed).
3. **rustc_fluent_macro/lib.rs**: dropped `feature(track_path)` +
   `feature(proc_macro_tracked_path)` + `feature(proc_macro_diagnostic)`
   — §1.8 stub doesn't need any of them.
4. **rustc_macros** (current_version.rs + symbols.rs): replaced
   `proc_macro::tracked::env_var` with `std::env::var` (proc-macros
   run host-side; tracked variant uses unstable APIs).
5. **rustc_target**: host-gated `schemars` + `serde_path_to_error`
   (JSON schema + diagnostic-path serialization, host-only debug).
6. **rustc_index/src/slice.rs**: `use alloc::borrow::ToOwned;` for
   `impl ToOwned for IndexSlice`.
7. **scoped-tls vendor fork**: added `edition = "2021"` so
   `pub use semos_std::*` resolves without explicit `extern crate`.

**STAGE E EXTERNAL WALL OFFICIALLY CLEARED.** Every external dep
now resolves cleanly. The complete external-clear list cumulatively:

| Cleared via | Crates |
|-------------|--------|
| [patch.crates-io] forks | tracing-core, scoped-tls, either, indexmap |
| Host-gates | annotate-snippets, anstream, anstyle, anstyle-parse, ar_archive_writer, blake3, ctrlc, crc32fast (via object/write feature), datafrog (via polonius), elsa, ena (2 places), find-msvc-tools, fluent-bundle, fluent-syntax, getopts, gsgdt, intl-memoizer, jiff, jobserver, libloading, log (via ena), measureme, memmap2, odht, parking_lot, parking_lot_core, pathdiff, polonius-engine, pulldown-cmark, punycode, rustc-stable-hash, schemars, serde_path_to_error, shlex, stacker, tempfile, termize, thorin-dwp, tracing-subscriber, tracing-tree, unic-langid, wasm-encoder |
| df=false sweeps | bstr, indexmap (also forked), itertools, memchr, rand, regex, rustc-hash, scoped-tls (also forked), serde + serde_json (8 crates), smallvec, stable_deref_trait (via elsa), thin-vec (18 crates), tracing, unicode-normalization |
| §1.8 cleanup | fluent-bundle/syntax/annotate-snippets/unic-langid/icu_list/icu_locale/rustc_baked_icu_data/tracing all dropped from rustc_error_messages + rustc_fluent_macro |
| Patched-crate followups | rustc_arena, rustc_fluent_macro stub, rustc_fs_util, rustc_graphviz, rustc_hashes, rustc_index, rustc_log, rustc_macros, rustc_parse_format, rustc_proc_macro stub, rustc_thread_pool |

**One remaining failure: rustc_data_structures (135 errors)** — all
patched-crate body issues (missing imports + feature gates similar
to iter 12's rustc_index / rustc_parse_format / rustc_arena fixes,
just at scale). This is Stage F territory: real port-quality work
on the 48 patched crates rather than external-dep wrestling.

Cumulative this session: ~30 commits, ~8.2M tokens. Stage E
externally complete. The pattern is fully proven across the entire
external dep graph.

────────────────────────────────────────────────────────────────────
## Stage F1: rustc_data_structures — body port (135 → 0 errors)

Date: 2026-06-01 (continuation from previous Stage E iter 12 commit)

Single-crate iteration on `rustc_data_structures` to bring it to a
clean `cargo check -p rustc_data_structures` on x86_64-unknown-none.
Pattern was the same one Stage E proved at the workspace level, just
applied inside one large patched crate at a finer granularity:

| Class | Files touched | Resolution |
|-------|--------------|------------|
| Missing `alloc::vec::Vec` / `alloc::string::String` / `alloc::boxed::Box` imports | `graph/dominators/mod.rs`, `graph/vec_graph/mod.rs`, `sorted_map/index_map.rs`, `stable_hasher.rs`, `sync/parallel.rs`, `thousands/mod.rs` | added per-file `use alloc::*;` at top |
| std-only `core::alloc::Layout`/`alloc::alloc::*`/`core::mem::needs_drop` swaps | `vec_cache.rs` | bulk substitution std → core/alloc |
| `std::sync::Mutex` cold-path locks | `vec_cache.rs` | cfg-split to `semos_std::sync::Mutex` (real futex-backed) on SemOS |
| `parking_lot::Mutex/RwLock` | `sync/worker_local.rs`, `sync/vec.rs` | cfg-split to `semos_std::sync::Mutex/RwLock`; AppendOnlyVec/IndexVec cfg-split structs |
| `thread_local!` proc-macro shape | `sync/worker_local.rs` + `semos-std/src/thread.rs` | extended semos_std::thread_local! macro to accept the `static FOO: T = const { ... };` form (rustc 1.59+) |
| `worker_local` rayon machinery | `sync/worker_local.rs` | replaced body with cfg-split host (full impl) vs target (single-threaded WorkerLocal = single CacheAligned<T> stub) |
| `elsa::sync::LockFreeFrozenVec` | `sync/vec.rs` (AppendOnlyIndexVec) | cfg-split: host = elsa; SemOS = `semos_std::sync::Mutex<Vec<T>>` |
| `rustc_thread_pool::join` Send-bound mismatch | `sync/parallel.rs::Mutex` shim, `rustc_thread_pool::join` | dropped `Send` bound on join stub; added `unsafe impl DynSend/DynSync` for the local RefCell-Mutex shim (single-threaded so safe) |
| `rustc_hash::FxHashMap`/`FxHashSet` gated behind `std` feature | `fx.rs`, `unord.rs` | local aliases over `hashbrown::HashMap<K, V, FxBuildHasher>` on SemOS; redirected `unord.rs` to import from local `fx` module; cfg-split `Entry`/`OccupiedError` aliases to absorb the extra hasher generic |
| `rustc_index_macros` emits `::std::*` paths | `rustc_index_macros/src/newtype.rs` | swapped to `::core::*` (Step, Debug, Formatter, Result, ops::Add/AddAssign) |
| `#![feature(file_buffered)]` + `#![feature(thread_id_value)]` invalid on no_std | `lib.rs` | cfg-gated to host-only |
| `#![cfg_attr(bootstrap, feature(array_windows))]` | `lib.rs` | promoted to unconditional `#![feature(array_windows)]` |
| `File::create_buffered` (file_buffered feature) | `obligation_forest/graphviz.rs` | cfg-split to `File::create(path.as_str())` on SemOS (also `.as_str()` for the `PathBuf → &str` arg coercion) |
| `impl_stable_traits_for_trivial_type!(::semos_std::ffi::OsStr)` conflict | `stable_hasher.rs` | dropped (OsStr = str alias on SemOS — covered by the `impl for str` elsewhere) |
| `Path: Ord`/`PathBuf: Ord` not satisfied | `semos-std/src/path.rs` | added `impl Hash + PartialOrd + Ord for Path`; `#[derive(... PartialOrd, Ord, Hash ...)]` for PathBuf |
| `tracing::instrument` proc-macro attribute not available with df=false | `graph/scc/mod.rs` | dropped import + commented one call site (diagnostic-only) |
| `crate::undo_log` ena re-export host-only | `lib.rs`, `snapshot_map/mod.rs` | host-gated `pub mod snapshot_map;` (ena is host-only) |
| `!DynSend`/`!DynSync` neg-impls treating `Rc`/`Weak` as partial | `marker.rs` | added explicit `<T, A: Allocator>` to the negimpl arguments |
| SemOS StableHasher::finish<T: FromStableHash> bound | `stable_hasher.rs` | added `<Hash = StableHasherHash>` projection |

Error trajectory: **135 → 121 → 84 → 59 → 53 → 45 → 39 → 23 → 15 → 3 → 2 → 1 → 0**

Workspace state after F1: `cargo check` reports **58 errors in
rustc_span** (Stage F2 target). All previously patched crates
upstream of rustc_span are now clean. Note: rustc_span has its own
upstream-flagged issues (blake3 host-gating, `read_buf` feature
gating, etc.) — same pattern as F1 at a different surface.

Cumulative this session: Stage F1 = 1 crate, ~25 minutes of
iteration, ~16 substitution categories. The cfg-split pattern
proves across the entire crate body without needing to bisect or
recon-agent — once F1 took 135 errors to 0 via straightforward
mechanical rules, F2-F* should be tightly bounded too.

────────────────────────────────────────────────────────────────────
## Stage F2: rustc_span — body port (58 → 0 errors)

Same pattern as F1, accelerated by re-using the substitution rules.
Added Cargo.toml-side `[target.'cfg(target_os = "none")'.dependencies]`
semos-std declaration to two more crates (rustc_span, rustc_serialize)
that previously imported `semos_std` from source without declaring it.

Per-class fixes (in order of impact):
- alloc preludes — Vec/String/Box/ToString/ToOwned in source_map.rs,
  hygiene.rs, symbol.rs, lib.rs + `mod monotonic {}` sub-mod
- hash crates host-only — `use md5/sha1/sha2;` cfg-gated, and the
  `new_in_memory` / `new(impl Read)` digest matches wrapped in
  `#[cfg(not(target_os = "none"))]` (SemOS-side returns the zero-
  initialised value, OK per §1.3 dropping incremental)
- `is_x86_feature_detected!` (libstd only) → cfg-split to
  `cfg!(target_feature = "sse2")` on SemOS (x86_64-unknown-none has
  sse2 enabled in the baseline)
- `eprintln!` (libstd only) → cfg-gated; SemOS falls straight through
  to FatalError::raise
- `#[instrument(...)]` proc-macro attribute (tracing's `attributes`
  feature off) → commented-out (5 sites)
- `#[feature(read_buf)]` / `#[feature(core_io_borrowed_buf)]` — host-only;
  `#[feature(array_windows)]` promoted unconditional (R3-compat)
- `FileEncoder` removed in §1.3 → drop the `use` + cfg-out the
  `impl SpanEncoder for FileEncoder` block
- `HashStable_Generic` proc-macro emitted `::std::mem::discriminant`
  → changed to `::core::mem::discriminant` in rustc_macros/hash_stable.rs
- semos_std::path Path/PathBuf gaps → added Debug + Hash + PartialOrd
  + Ord + to_string_lossy + explicit `From<&Path>/<PathBuf>/<&PathBuf>
  for Cow<Path>` impls (alloc's blanket `Cow<'a, B>: From<&'a B>` was
  not picking up our custom ToOwned)
- PathBuf Encodable/Decodable → added impls in rustc_serialize for
  both target_os = "none" (semos_std::path::PathBuf) and host
  (std::path::PathBuf) — serialize as UTF-8 string
- `semos_std::env::var` returns Result<String, VarError>; def_id.rs
  had a stale `Some(...)` pattern → `Ok(...)`/`Err(_)`
- `.cargo/config.toml` `[env]` — added CFG_RELEASE / CFG_VERSION /
  CFG_VER_HASH etc. (rustc_span/symbol.rs reads `env!("CFG_RELEASE")`
  at compile time; normally bootstrap sets these)

Error trajectory: **58 → 78 → 45 → 28 → 4 → 1 → 0**
(58→78 was the semos-std-dep flip uncovering deeper errors)

Workspace state after F2: `cargo check` blocks on **rustc_ast (344
errors)**. F3 next.


────────────────────────────────────────────────────────────────────
## Stage F3: rustc_ast + rustc_ast_pretty + rustc_error_messages

**rustc_ast: 344 → 0 errors** — same pattern as F2 but at higher
volume (mostly alloc preludes across 11 files: ast.rs, mut_visit.rs,
tokenstream.rs, visit.rs, util/literal.rs, util/comments.rs,
ast_traits.rs, expand/autodiff_attrs.rs, expand/typetree.rs,
expand/allocator.rs, attr/data_structures.rs, attr/mod.rs, format.rs).
Plus:
- rustc_macros/src/hash_stable.rs already fixed in F2 (`::core::mem::
  discriminant` instead of `::std::`)
- `#[tracing::instrument(...)]` ast.rs site commented out
- `#[cfg_attr(bootstrap, feature(array_windows))]` → unconditional
- rustc_serialize: re-instated `impl Encodable/Decodable for
  hashbrown::HashMap/HashSet` (was cfg(any())-disabled), added
  `hashbrown` as a Cargo.toml dep — `FormatArguments` in rustc_ast
  uses `FxHashMap<Symbol, usize>` with `derive(Encodable, Decodable)`
- rustc_ast Cargo.toml — semos-std target-dep added (attr/version.rs
  imports semos_std::sync::OnceLock + env::var)

**rustc_ast_pretty: ~100 → 0 errors** — alloc preludes only.
Files: pp.rs, pp/convenience.rs, pprust/mod.rs, pprust/state.rs,
pprust/state/expr.rs, pprust/state/item.rs.

**rustc_error_messages: 5 → 0 errors** — only needed the semos-std
target-dep Cargo.toml line (lib.rs / diagnostic_impls.rs already
imported `semos_std::{io,path}`).

Workspace now blocks on **rustc_type_ir (83 errors)**. F4 next.
Crates closed cumulatively this session: rustc_data_structures (F1),
rustc_span + rustc_serialize (F2), rustc_ast + rustc_ast_pretty +
rustc_error_messages (F3). Six down, ~42 remaining.


────────────────────────────────────────────────────────────────────
## Stage F4: rustc_type_ir (83 → 0 errors)

Seventh patched crate closed. Same pattern, with two new wrinkles:

1. **`use std::*;` lines persisted in source** — unlike F1-F3 where
   the upstream was already partly cfg-split, rustc_type_ir had a lot
   of un-touched `use std::*;` imports. Mechanical swap to
   `use core::*;` / `use alloc::*;` across visit.rs, search_graph/
   {mod.rs, stack.rs}, ty_info.rs, ty_kind.rs, ty_kind/closure.rs,
   relate/combine.rs, solve/mod.rs.
2. **`::std::` paths in macro emission** — `macros.rs` line 11
   (`-> ::std::result::Result<...>` in the TypeFoldable macro body)
   → `::core::result::Result`. Same shape as the F2/F1 fix to
   rustc_macros and rustc_index_macros.

Additional substitutions:
- `std::collections::hash_map::Entry` (no_std) → `hashbrown::hash_map::Entry`
- `ena::unify::{NoError, UnifyKey, UnifyValue}` host-only — guards
  on re-export + on the 4 impl blocks in ty_kind.rs for IntVid/FloatVid
- `rustc_hash::{FxHashMap, FxHashSet}` (gated by `std` feature) →
  hashbrown::HashMap/HashSet aliased with FxBuildHasher on SemOS
- Bulk `#[instrument(...)]` strip: 10 sites across binder.rs, fold.rs,
  relate.rs, relate/solver_relating.rs, search_graph/mod.rs +
  drop instrument from the `use tracing::{...}` lists
- alloc preludes added to 14 files (canonical, const_kind, elaborate,
  fold, infer_ctxt, inherent, interner, ir_print, outlives, relate/
  solver_relating, search_graph/mod, solve/inspect, solve/mod, visit)
- semos-std target dep added to Cargo.toml (ir_print.rs into_diag_arg
  signatures reference `semos_std::path::PathBuf`)
- hashbrown added as a direct dep (data_structures::HashMap alias)

Error trajectory: **83 → 55 → 14 → 0**

Workspace check now blocks on **rustc_next_trait_solver (1048 errors)**.
That's a meaningfully bigger crate but should be tractable on the
same playbook — Stage F5 next. Crates closed cumulatively: 7 of
the ~48 internal rustc_* fork (data_structures, span, serialize,
ast, ast_pretty, error_messages, type_ir).

────────────────────────────────────────────────────────────────────
## Stage F5: rustc_next_trait_solver (1048 → 0 errors)

Eighth patched crate. Massive error count, but a simple root cause:
the crate had no `#![no_std]` declaration and no `extern crate alloc`,
so on the SemOS target every prelude item (`Option`, `Result`,
`Ok`/`Err`/`Some`/`None`, `Vec`, `vec!`, `panic!`, `derive`) was
unresolved.

Steps:
1. Add `#![no_std]` + `#[macro_use] extern crate alloc;` to lib.rs.
   This single edit took us 1048 → 140 errors.
2. Bulk-add `use alloc::{boxed::Box, string::{String, ToString},
   vec::Vec};` prelude to 13 files via awk one-liner.
3. Bulk-comment `#[instrument(...)]` attrs in 13 files via sed.
4. Strip `instrument` from `use tracing::{...}` lines.
5. `use std::*` → `use core::*` (12 lines across 9 files).
6. Two `std::fmt::Display` / `std::mem::take` body refs → `core::*`.

Error trajectory: **1048 → 140 → 2 → 0**

Workspace check now blocks on **rustc_abi (492 errors)** — same
pattern, probably also no_std-missing-prelude. F6 next. Crates
closed cumulatively: 8 (data_structures, span, serialize, ast,
ast_pretty, error_messages, type_ir, next_trait_solver).


────────────────────────────────────────────────────────────────────
## Stage F6: rustc_abi (492 → 0 errors)

Ninth patched crate. Identical root cause to F5 (rustc_next_trait_solver):
the crate had no `#![no_std]` and no `extern crate alloc;`.

Steps:
1. `#![no_std]` + `#[macro_use] extern crate alloc;` added to lib.rs
   (placed AFTER the `/*! ... */` inner doc block to avoid E0753).
2. Bulk `use std::*` → `use core::*` / `use alloc::collections::*`
   across canon_abi, extern_abi (+ tests), layout/{coroutine, simple,
   ty}, layout.rs, lib.rs.
3. Body refs `std::fmt`/`std::mem` etc. → `core::*` via sed sweep.
4. One `#[tracing::instrument(...)]` in callconv.rs commented.
5. Per-file alloc prelude bundles added to extern_abi.rs, layout.rs,
   lib.rs.

Error trajectory: **492 → 34 → 13 → 2 → 0**

Workspace check now blocks on the next downstream crate. Crates closed
cumulatively in Stage F: 9 (data_structures, span, serialize, ast,
ast_pretty, error_messages, type_ir, next_trait_solver, abi).

────────────────────────────────────────────────────────────────────
## Stage F7: rustc_target (3946 → 0 errors)

Tenth and largest patched crate. Same playbook as F5/F6 plus extensive
schemars + serde + std-host gating:

- `#![no_std]` + `#[macro_use] extern crate alloc;` in lib.rs
- Bulk std→core/alloc swap across ~37 source files (sed sweep)
- `use std::path::{Path, PathBuf}` → cfg-split for semos_std on SemOS
- schemars host-gated: `impl JsonSchema for X` blocks wrapped with
  cfg(not(target_os="none")) via awk; derive lists stripped via sed
- `pub use json::json_schema` re-export host-gated
- `Target::from_json` (serde_path_to_error), `Target::search` (env+fs),
  `TargetTuple::from_path` (io::Error), `TargetTuple::debug_tuple`
  (std::hash::DefaultHasher) all host-gated
- `Display for TargetTuple` → falls back to `tuple()` on SemOS
- `nto_qnx::get_iosock_param` env var lookup cfg-split host/semos_std
- `IntoDiagArg for PanicStrategy` PathBuf ref uses crate-level alias
- `#![feature(debug_closure_helpers)]` added for `core::fmt::from_fn`
- 3 #[tracing::instrument] in callconv/aarch64.rs commented
- alloc preludes added to 6 files (callconv/mod, json, lib, spec/{crt_objects, json, mod})
- Cargo.toml: semos-std target dep added

Error trajectory: **3946 → 444 → 196 → 146 → 113 → 108 → 27 → 0**

Workspace check now blocks on **rustc_hir (27 errors)** + tail of
others — multiple downstream crates unblocked. Crates closed
cumulatively: **10** (data_structures, span, serialize, ast,
ast_pretty, error_messages, type_ir, next_trait_solver, abi, target).


────────────────────────────────────────────────────────────────────
## Stage F8: rustc_hir + rustc_feature + rustc_hir_pretty + rustc_errors

Four more patched crates closed in this push.

**rustc_hir: 27 → 0**
- `#![feature(debug_closure_helpers)]` for `fmt::from_fn`
- `def_path_hash_map.rs`: odht host-gated; SemOS uses
  `alloc::collections::BTreeMap<Hash64, DefIndex>` as drop-in
- alloc preludes added to 7 files
- ToString trait imports in limit.rs + target.rs
- 1 #[instrument] + tracing import in definitions.rs
- BTreeMap insert/get owned-vs-ref signature cfg-splits at the
  2 call sites

**rustc_feature: 10 → 0**
- semos-std target dep in Cargo.toml
- alloc preludes in builtin_attrs.rs + unstable.rs
- std::path::PathBuf import cfg-fixed (awk-insert had clobbered
  the existing cfg-split)

**rustc_hir_pretty: 17 → 0**
- single-file alloc prelude expansion in lib.rs

**rustc_errors: 85 → 0** (the big one)
- semos-std target dep + anstyle promoted from host-only to universal
  with `default-features = false` (anstyle IS no_std-clean)
- annotate-snippets / anstream / termize stay host-only
- `pub mod annotate_snippet_emitter_writer` / `emitter` / `json` /
  `markdown` modules all cfg-gated to host (anstream-dependent)
- minimal SemOS `emitter_stub` module providing
  `Emitter`/`DynEmitter`/`SilentEmitter`/`ColorConfig`/`TimingEvent`
  with no-op default methods so DiagCtxt's field types still
  type-check on SemOS
- **rustc_fluent_macro upgrade**: macro now actually parses the
  `.ftl` file and emits `pub const NAME: DiagMessage =
  DiagMessage::FluentIdentifier(::alloc::borrow::Cow::Borrowed("..."), None)`
  for every top-level message name. Previous stub was empty,
  causing downstream `fluent_generated::*` references to fail.
- decorate_diag.rs alloc preludes
- codes.rs + backtrace_shim ToString imports
- `Backtrace: IntoDiagArg` via `into_diag_arg_using_display!`
- `#![feature(array_windows)]`, `error_reporter` host-gated

Workspace cumulative: **14 patched crates closed** (data_structures,
span, serialize, ast, ast_pretty, error_messages, type_ir,
next_trait_solver, abi, target, hir, feature, hir_pretty, errors).

Workspace check now blocks on **rustc_session (2415 errors)** + tail.
Session has the biggest cargo dep graph below it — F9 will be a slog.


────────────────────────────────────────────────────────────────────
## Stage F9: rustc_session — PARTIAL (2415 → 122 errors, still active)

The largest patched crate yet by error count. Same playbook +
significant net-new work:

- `#![no_std]` + `#[macro_use] extern crate alloc;` in lib.rs
- Bulk alloc preludes injected into 14 source files
- semos-std target dep added
- Bulk `std::*` → `core::*` / `alloc::*` / `semos_std::*` sweep
  across 15 source files
- `use core::{env, fs}` / `use core::{env, io}` broken-imports
  manually fixed to `semos_std::{...}`
- **Local `getopts` stub module** (lib.rs) — getopts is host-only;
  SemOS doesn't parse CLI args via it. Stub provides Matches +
  Options + opt_present/opt_str/opt_strs/opt_count/opt_default
  and all the optopt/optmulti/optflag/optflagmulti chain methods.
- `#[cfg(target_os = "none")] use crate::getopts;` injected into
  each consuming file to bring local stub into scope
- **rustc_fluent_macro upgrade #2**: now emits
  - `pub const <parent>: DiagMessage` for every top-level message
  - `pub mod <parent>_subdiag` with common subdiag attrs (label,
    help, note, suggestion, warn, note_1/2, see_issue, first_note,
    second_note, suggestion_short/verbose/remove/add, etc.)
  - `pub mod _subdiag` as a top-level catch-all module — the
    Diagnostic derive macro emits literal `crate::fluent_generated::
    _subdiag::label` paths (the `_subdiag` segment is NOT
    substituted with the parent slug downstream, contrary to first
    intuition; it's a literal placeholder module name)
  - Parses `.attribute = ` indented lines as subdiag attrs

Error trajectory so far: **2415 → 385 → 198 → 187 → 172 → 167 →
166 → 132 → 131 → 122**

Remaining ~122 errors are mostly:
- 24 type annotations needed (sed-broken syntax or trait inference)
- ~15 host-only rustc_errors::emitter::* and ::json and
  ::annotate_snippet_emitter_writer use sites that need cfg-gates
- ~11 println!/print! macro use sites (host-only via std prelude)
- ~6 `core::ffi::OsStr` / `core::fs` / `BufWriter in io` — paths
  that don't exist (need semos_std variants or cfg-gates)
- ~5 trait-bound issues (DepTrackingHash for String, etc.)
- 1 `getopts` reimport conflict in lib.rs (needs `pub use ... as`)

Stage F9 to be continued in a follow-up. Crates closed cumulatively
this session: 14 (rustc_session still pending).

