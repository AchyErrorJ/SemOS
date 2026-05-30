# M27 Phase 1 — synthesis of R1/R2/R3/R4

Drafted 2026-05-30 after all four recon agents returned. **Verdict:
PROCEED to Phase 2** with the three new decisions in §A added to the
plan before any port work starts.

## Headline numbers (cross-agent reconciliation)

|                                       | R1            | R2          | R3          | R4         |
|---------------------------------------|---------------|-------------|-------------|------------|
| Internal rustc_* crates total         | 77            | 76          | —           | —          |
| Internal after §1 drops               | 70 (~770 k LOC) | 65         | —           | —          |
| External (non-rustc_*) crate count    | ~95 visible   | —           | 71 distinct | —          |
| External after §1 drops               | —             | —           | 50–55       | —          |
| Architectural-class crates (R2)       | —             | 13 → 5 after §1 | —      | —          |
| New blockers beyond §1                | 0             | 0           | 0           | 1 (B1)     |

The 77/76 internal-count split is a counting edge: R1 includes
`rustc_thread_pool` (the vendored rayon fork), R2 excludes it because
R3 confirms §1.4 cascade-drops it. Use **77 / 70-after-drops** as
canonical.

## The new decisions Phase 1 surfaced

R2 + R4 independently proposed the same §1.7. R3 proposed a separate
§1.8. R4 surfaced a third item that needs to become §1.9. Adopt all
three before Phase 2 spawns:

### §1.7 — cg_clif emits ET_EXEC directly; drop the external-linker path
- **R4 finding**: rustc has no in-process linker; `rustc_codegen_ssa/
  src/back/link.rs:1593` calls `Command::new(linker).spawn()`.
- **Mitigation**: don't port `rustc_codegen_ssa::back::link` at all.
  cg_clif already produces full ET_EXEC bytes (proven in semos-cc D.1/
  D.2). Have semos-rustc bypass the SSA link step and consume cg_clif's
  output directly.
- **Implication**: ~one fewer subsystem to port. Stops a Command-spawn
  rabbit hole.

### §1.8 — drop i18n entirely; hardcode English diagnostics
- **R3 finding**: fluent-bundle + unic-langid + intl-memoizer + 4 ICU
  crates = ~7 externals, ~5 sessions of port work, none of it on the
  critical path for "compile a Rust program."
- **Mitigation**: `rustc_errors` ships an English-only path; gut the
  fluent loader, return hardcoded English message strings.
- **Implication**: ~5 sessions saved, diagnostic quality regresses but
  is still usable.

### §1.9 — FatalError → abort the process; accept the limitation
- **R4 finding**: this is the **one unresolved blocker** (B1). rustc
  uses `panic_any(FatalError)` + `catch_unwind` for compiler errors as
  control flow. semos-std panics abort. 68 occurrences in 20 files.
- **Mitigation v1**: accept that any FatalError terminates the whole
  rustc invocation. Document the user-facing limitation: "one error
  per compile." Real fix requires a SemOS stack-unwinder (3-5 kernel
  sessions, out-of-scope for M27).
- **Implication**: rustc on SemOS is one-error-per-compile in v1. Live
  with it; revisit when stack-unwinding becomes a kernel priority.

## Revised scope estimate

Original plan §0 said 30-60 sessions across 4-8 agents.

Reconciling against R1/R2/R3/R4:

- **Internal crate work (Phase 2-4)**: 70 crates needing port. R2
  classifies post-§1: 5 ARCHITECTURAL, 8 NEEDS-SHIM, 57 MECHANICAL.
  At 1-3 sessions per MECHANICAL crate and 5-8 per ARCHITECTURAL/
  NEEDS-SHIM, that's **~85-130 crate-sessions**. Parallelized 4-wide
  across Phase 3: **~25-35 calendar-sessions** with 4 agents.
- **External crate work**: 50-55 crates remaining. R3 estimates ~15
  sessions of focused PATCH work + ~5 reusable from Cranelift.
  Parallelizable: **~5-7 calendar-sessions** with 2-3 agents.
- **semos-std surface additions** (OnceLock, thread_local, env::var,
  PathBuf::canonicalize, OsString — the top 5 from R2): **~4-6 sessions**
  on my side, doable in parallel with external porting.
- **Foundation (Phase 2a + 2b)**: R1 split it into 14 zero-rustc-dep
  trivially-parallel crates + 3 sequential (ast/lint_defs/errors due to
  the ast↔errors cycle). **~6-10 sessions** with 2 agents.
- **Integration (Phase 5)**: still budget 3-5 sessions on my side.

**New total: 40-60 calendar-sessions parallelized 4-6 wide. Calendar:
1-2 months if sustained.**

Within range of the original estimate but with much more precise
breakdown. The "we don't know what we're walking into" risk is now
"we know we're walking into ~85-130 mostly-mechanical patches with one
unfixable-in-v1 limitation (one-error-per-compile)."

## Revisions to Phase 2 (per R1 + R2 + R4)

**Phase 2 splits into 2a + 2b** (R1 §6.4: there's a cycle through
`rustc_errors → rustc_ast`):

### Phase 2a — Pure foundation (parallel, ~6 agents, 1-2 calendar-sessions)
14 zero-rustc-dep crates per R1's count. Trivially parallel because no
crate in this set depends on any other internal rustc_* crate.
Candidates (R1's foundation tier):
- `rustc_data_structures` (the big one — has the rayon shim work)
- `rustc_serialize`, `rustc_span`, `rustc_errors`, `rustc_macros`
- `rustc_index`, `rustc_arena`
- Plus the ~7 zero-rustc-dep utility crates R1 itemized

Critical for Phase 2a: the rayon-shim in `rustc_data_structures` is
~1-2 sessions of careful work. R4 B2 (TLS shim) and B4 (rustc_thread_pool
stub) sit here too. Budget: **2-3 sessions** for the heavy crates,
**0.5-1 session** each for the trivial ones, **parallel**.

### Phase 2b — Cycle-breakers (sequential, me + 1 agent, ~3-5 sessions)
`rustc_ast` + `rustc_lint_defs` + `rustc_errors` need to land together
because of the cycle R1 found. Sequential treatment with careful
integration.

## Revisions to Phase 3 (per R1)

Two clusters of 24+ crates each, mostly per the original plan but with
R1's more specific groupings:

**Cluster A (frontend + lexer)**: `rustc_lexer`, `rustc_parse`,
`rustc_parse_format`, `rustc_ast_pretty`, `rustc_ast_lowering`,
`rustc_ast_passes`, `rustc_attr_parsing`, `rustc_attr_data_structures`,
`rustc_feature`, `rustc_builtin_macros` (the front-end macros, not the
runtime proc-macro server). **~8 crates × 1-3 sessions / 3 agents ≈
3-8 calendar-sessions.**

**Cluster B (semantics — the 47% of LOC)**: `rustc_hir`,
`rustc_hir_pretty`, `rustc_hir_analysis`, `rustc_hir_typeck`,
`rustc_infer`, `rustc_trait_selection`, `rustc_type_ir`,
`rustc_const_eval`, `rustc_middle`, `rustc_borrowck`, `rustc_privacy`,
`rustc_resolve`, plus stragglers. **~13 crates × 2-5 sessions / 3 agents
≈ 10-20 calendar-sessions.** `rustc_middle` alone is budgeted 5-8
sessions per R1 §6.5 (60 k LOC, can't be split).

## Revisions to Phase 4 (per R1)

Streamlined per R1 §6.1 (cg_clif loaded via cargo `-Z codegen-backend`,
no metadata-plugin surgery) and the new §1.7 (cg_clif owns final ET_EXEC
emission, no `rustc_codegen_ssa::back::link` port).

Phase 4 crates: `rustc_codegen_ssa` (just the IR-agnostic codegen
orchestration, NOT the linker back-end), `rustc_mir_build`,
`rustc_mir_transform`, `rustc_mir_dataflow`, `rustc_monomorphize`,
`rustc_passes`, `rustc_metadata` (with §1.2 + §1.7 simplifications).

**~6-7 crates × 2-4 sessions / 2-3 agents ≈ 5-10 calendar-sessions.**

## What was confirmed worth keeping in semos-std plan

R2's top-5 semos-std additions, in adopted priority order (combined
with R4's blockers):

1. **`sync::OnceLock<T>` + `OnceCell<T>` (8+ rustc crates need it)** —
   lift the local shim we wrote in `cranelift-codegen/src/isa/x64/abi.rs`
   into semos-std. ~0.5 session.
2. **`thread::LocalKey<T>` + `thread_local!` macro + scoped-tls** —
   single-threaded shim, since SemOS Ring 3 is currently single-
   threaded per process. **1-2 sessions.** R4 B2.
3. **`env::var{,_os}` reading from a const table or namespace** — 10+
   rustc crates. **1 session.**
4. **`path::PathBuf::canonicalize` (lexical-only)** — 3 crates.
   **0.5 session.**
5. **`OsString` / `OsStr`** — R4 B5. **1 session.**
6. **`process::abort_with_code(i32)`** — to support the §1.9 FatalError
   path cleanly (R2 / R4). **0.5 session.**

Total: **~5-7 sessions** of semos-std work, doable in parallel with the
external-crate port.

## Stop conditions inherited / refined

From original plan, restated for clarity:
- **Phase 2**: if `rustc_data_structures` can't be made single-threaded
  with the rayon-shim approach, re-strategize. R4 B4 says the stub is
  ~50 lines so this risk is low.
- **Phase 3**: if any single crate takes more than 3 sessions of focused
  work, escalate. **Exception**: `rustc_middle` gets 5-8 sessions per
  R1 §6.5.
- **Phase 4**: per original plan.
- **New (from R4 B1)**: if anyone tries to "fix" FatalError without
  the SemOS unwinder, escalate. The §1.9 accept-the-abort decision is
  load-bearing.

## What to do now

1. **Commit Phase 1 outputs** (this doc + R1-R4 reports).
2. **Amend `docs/M27_RUSTC_PORT_PLAN.md`** with §1.7/§1.8/§1.9 + the
   Phase 2a/2b split + the revised scope estimate.
3. **Pause for explicit user sign-off** before Phase 2 spawns. The
   commit to the next 40-60 sessions across 4-6 agents is real money.
4. **If signed off, spawn Phase 2a** — 6 agents on the zero-rustc-dep
   foundation tier.

Phase 2a's parallel agents will need the same recipe doc + worktree
isolation pattern we used here. Recipe is `user-programs/semos-cc/
PORT_LOG.md` plus this synthesis's §1.7/§1.8/§1.9 decisions plus the
specific gotcha that **no rustc_* crate has `#![no_std]` already** —
every port starts from std-by-default.
