# M27 rustc-on-SemOS — research project plan

Drafted 2026-05-30. Replaces the "Big lift — separate research project"
hedge in ROADMAP/SELF_HOSTING_PLAN with a concrete attack plan
parallelizable across multiple agents.

This is not "another session of D.x." It's a sustained effort against
a 60-80 internal-crate codebase that was never designed for embedded /
no_std use. Scope realism is the load-bearing piece — if you skip
sections 1-3 below and jump to "have agents start porting," the work
silently stalls inside week two.

## 0. The honest scope

rustc's compiler/ tree has ~70 internal crates. They're all path-deps,
not crates.io. The architecture assumes:
- Dynamic loading of codegen backends (`librustc_metadata` uses
  libloading-equivalent to load `librustc_codegen_llvm.so` or
  `librustc_codegen_cranelift.so` at runtime). **SemOS has no dlopen.**
- Threaded compilation (rayon-based parallel queries). **semos-std has
  thread::spawn but no rayon work-stealing pool.**
- A persistent on-disk incremental cache. **Not fundamentally blocking
  but assumes seek-write semantics our FS doesn't yet fully support.**
- LLVM as the default codegen backend. Linking against LLVM is a C++
  toolchain dependency we don't want and can't easily do.
- File-system layout assumptions (sysroot, crate search paths, rustlib).

The smallest viable rustc-on-SemOS therefore drops:
1. **LLVM** entirely — only cg_clif as backend (already vendored).
2. **Dynamic codegen plugin loading** — statically link cg_clif.
3. **Incremental compilation** — single-shot compiles only.
4. **Rayon-parallel queries** — run the query system single-threaded.
5. **`rustc_metadata` plugin model** — single codegen, single target.

Even with those drops we still need to port ~50-60 internal crates plus
several external deps (jiff, anstyle, tracing, etc.). The Cranelift
port (14 crates, mostly small) took one intensive session. rustc's
crates are larger and more std-coupled. Realistic estimate: **30-60
sessions** of focused work, parallelizable across 4-8 agents.
Calendar-time: **1-2 months full-time, possibly longer.**

If that estimate alone is intolerable, stop here and pivot to D.3 or
accept that rustc-on-SemOS stays aspirational. The rest of this
document assumes you've accepted the scope.

## 1. Decision points up front (do NOT defer)

These choices set the shape of everything downstream. Make them before
spawning any agents.

### 1.1 Cut the LLVM backend
**Decision:** drop rustc_codegen_llvm entirely. Only cg_clif.
**Implication:** fork rustc's build to remove the llvm-sys dep and the
codegen_backends feature gating. The fork lives in
`user-programs/semos-rustc/vendor/`. Original rustc upstream is
intentionally not tracked — we own this fork.

### 1.2 Statically link cg_clif
**Decision:** rather than loading cg_clif as a dylib at runtime, link
it as a regular cargo dep of rustc_driver_impl. Requires removing the
plugin-loading abstraction in rustc_codegen_ssa + rustc_metadata.
**Implication:** the codegen backend can't be swapped at runtime. For
SemOS that's fine — we only ever target SemOS via cg_clif.

### 1.3 Drop incremental compilation
**Decision:** define `INCR_COMP_DISABLED=true` in rustc_session's config
parsing OR cfg-out the whole rustc_incremental crate.
**Implication:** every compile is a clean compile. Much simpler dep
graph (no on-disk cache schema, no file locking, no version-keyed
hashing). Acceptable cost for a v1.

### 1.4 Drop rayon
**Decision:** patch rustc_data_structures and rustc_query_impl to use
a single-threaded "rayon" shim that runs everything sequentially.
**Implication:** compile times worse, but no thread pool needed.

### 1.5 Drop proc-macros (initially)
**Decision:** the v1 SemOS rustc compiles programs that don't use
proc-macros. proc-macro expansion needs dynamic loading + a sandboxed
subprocess; both are infeasible on day one.
**Implication:** can't compile most real Rust crates from crates.io.
Useful for the M25 hello-world goal but not for general use.

### 1.6 Single target
**Decision:** rustc-on-SemOS only ever produces `x86_64-unknown-none`
output (the SemOS target). No cross-compilation.
**Implication:** the target-triple parsing, target-spec loading, sysroot
search paths all get hardcoded.

These six decisions probably halve the work. They also commit you to a
fork that diverges from upstream rustc — you'll need to rebase
periodically if upstream lands important bug fixes.

## 2. Phase plan (5 phases, gated by decision points)

Each phase has a stop condition. If you hit it, don't push through —
re-plan.

### Phase 1 — Recon (parallel, 3-4 agents, ~1 session calendar time)

Goal: map the dep graph, classify crates by porting cost, identify the
crates that are fundamentally infeasible without major rework.

- **Agent R1 — dep graph cartography.** Vendor `rustc-src` into
  `user-programs/semos-rustc/vendor/`. Run `cargo tree --workspace
  --no-default-features` and produce a complete reverse-dep map of all
  rustc_* crates. Tag each crate's lines-of-code, public API size, and
  whether it has its own build.rs.
- **Agent R2 — std-surface audit.** For each rustc_* crate, grep for
  `std::process::Command`, `std::fs::`, `std::sync::{Mutex,RwLock}`,
  `std::thread::`, `std::path::`, `std::collections::HashMap` usage.
  Produce a per-crate report: "this crate has N std-surface dependencies
  that aren't in semos-std." Identifies the porting cost concretely.
- **Agent R3 — externals identification.** List every external (non-
  rustc_*) crate the tree depends on. For each: is it on the SemOS
  vendor list (semos-std, hashbrown, libm, smallvec, etc.)? Is it
  no_std-compatible (per its Cargo.toml `categories`)? Or is it a
  hard wall (rayon, libloading, llvm-sys)?
- **Agent R4 — fundamental-block audit.** Look for crates that *can't*
  be ported without major architectural surgery: anything using
  threadpool semantics, anything using dlopen, anything that shells
  out via Command. Produce a list with mitigation options per crate.

Each agent writes to a sub-doc:
- `docs/m27-recon/R1_dep_graph.md`
- `docs/m27-recon/R2_std_surface.md`
- `docs/m27-recon/R3_externals.md`
- `docs/m27-recon/R4_blockers.md`

**Stop condition for Phase 1:** if R4 identifies more than 3 "no clean
mitigation" blockers, the project's strategy needs rethinking. The
decision points above already address LLVM, libloading, and rayon —
if there's a fourth, it has to be addressed before Phase 2 starts.

### Phase 2 — Foundation crates (sequential, me + 1 agent, ~3-5 sessions)

Goal: port the bottom of the dep graph — the crates that almost every
other rustc crate depends on. These have to be solid before parallel
work above them makes sense.

Likely foundation crates:
- `rustc_data_structures` — sync primitives, sharded maps, hash sets.
  This is where rayon abstraction lives. Single-threaded shim goes here.
- `rustc_serialize` — serde-like for rustc's metadata format.
- `rustc_span` — source-location tracking.
- `rustc_errors` — diagnostic emission.
- `rustc_macros` — proc-macros used internally (paradoxical: we don't
  support proc-macros at runtime, but rustc *itself* needs them at
  build time, on the host).
- `rustc_index` — newtyped index containers.
- `rustc_arena` — bump allocators.

These are sequential because mistakes here propagate upward. Me +
1 agent, alternating: agent drafts patches, I integrate and validate.

**Stop condition for Phase 2:** if `rustc_data_structures` can't be
made single-threaded without rewriting the query system, the whole
project's strategy needs rethinking.

### Phase 3 — Middle layer (parallel, 4-6 agents, ~10-15 sessions)

Goal: port the middle of the dep graph. These crates know about Rust
the language but not about codegen.

Cluster A (3 agents, frontend):
- `rustc_lexer`
- `rustc_parse`, `rustc_parse_format`
- `rustc_ast`, `rustc_ast_pretty`, `rustc_ast_lowering`, `rustc_ast_passes`
- `rustc_attr_parsing`, `rustc_attr_data_structures`

Cluster B (3 agents, semantics):
- `rustc_hir`, `rustc_hir_pretty`, `rustc_hir_analysis`, `rustc_hir_typeck`
- `rustc_infer`, `rustc_trait_selection`, `rustc_type_ir`
- `rustc_const_eval`, `rustc_middle`

Each agent gets ~5-7 crates. Same recipe as Cranelift port:
- Add `[workspace]` header (`members = []` to avoid dev-dep resolution)
- Substitute `std::*` → `core::*` / `alloc::*` / `hashbrown::*`
- Add `#![no_std]` + `extern crate alloc;` to crate root
- Fix module gates / cfg attributes
- Iterate against the build, patching transitive blockers

**Stop condition for Phase 3:** if any single crate takes more than
3 sessions of focused work, escalate — it's probably hitting an
unmitigated fundamental block.

### Phase 4 — Codegen layer (parallel, 2-3 agents, ~5-8 sessions)

Goal: port the codegen plumbing (rustc_codegen_ssa) and wire cg_clif
in statically.

- `rustc_codegen_ssa` — the IR-agnostic codegen orchestration.
- `rustc_mir_*` — MIR construction, optimization, dataflow.
- `rustc_monomorphize`, `rustc_borrowck`, `rustc_passes`
- `rustc_metadata` (with the plugin-load model torn out per decision 1.2)

This phase intersects heavily with the Cranelift port we already did.
Reuse it.

**Stop condition for Phase 4:** if removing the codegen plugin model
from rustc_metadata causes a cascade that affects 10+ other crates,
the static-link decision needs revisiting.

### Phase 5 — Integration + DEMO (me, ~3-5 sessions)

Goal: glue everything together, drive rustc_driver from `semos-rustc`,
get hello-world.rs to compile on SemOS.

- Wire `rustc_driver_impl` into a `user-programs/semos-rustc/` binary
  similar to semos-cc.
- Statically link cg_clif as the codegen backend (per decision 1.2).
- DEMO 80 (or wherever the numbering's at): semos-rustc compiles
  `fn main() { println!("hello"); }` to a SemOS ELF, runs it, asserts
  output.

**Stop condition for Phase 5:** if compile times exceed 5 minutes for
hello-world (likely given opt-level=0 mandatory + no incremental),
we ship anyway and revisit performance later.

## 3. Agent orchestration

For parallel phases (1, 3, 4):

- Each agent works in its own git worktree (use `isolation: "worktree"`
  on the Agent tool). No worktree sharing.
- Agents commit their patches to the worktree branch. Integration
  happens via me cherry-picking + rebasing.
- Shared dep crates (rustc_span etc.) are owned by ONE agent per phase
  to avoid merge conflicts. If multiple agents need it, the second
  agent waits.
- Each agent gets a copy of the "Cranelift port recipe" doc
  (`user-programs/semos-cc/PORT_LOG.md`'s lessons-learned section) at
  spawn time.
- Each agent reports back in under 800 lines, structured: "what I
  patched, what surprised me, what's blocked."

## 4. Risk register

| Risk | Probability | Mitigation |
|------|-------------|------------|
| rustc_data_structures can't be made single-threaded cleanly | medium | abort criterion at Phase 2; fall back to MicroPython or a Lua DSL as SemOS's scripting story |
| Some rustc_* crate uses unstable internal APIs that break between nightly toolchain versions | high | pin to one nightly version up-front (1.95.0 per current toolchain); accept that we can't follow upstream |
| Cranelift's no_std port (just done) has runtime bugs that surface during rustc's heavier workloads | medium | re-run DEMO 73 nightly; treat any new Cranelift panic as a P0 blocker |
| Calendar time exceeds 3 months and momentum dies | high | enforce per-phase stop conditions; pivot to D.3 + re-scope M27 if Phase 3 stalls |
| The smallest viable rustc still wants more memory than SemOS allows | medium | already bumped MAX_PT_FRAMES + USER_PROC_STACK_SIZE; may need further bumps to 256 MiB pool + 4 MiB stack |
| LLVM removal cascades through more code than expected | medium | budget Phase 4 generously; if it fails, fall back to "rustc-on-SemOS only emits IR, hand off to semos-cc for final codegen" |
| proc-macro deps in test cases mean even hello-world fails | low | hello-world.rs explicitly uses no proc-macros; document the limitation |

## 5. Definition of done

M27 ships when:

1. `semos-rustc` exists at `user-programs/semos-rustc/` and builds
   against `semos-std`.
2. A DEMO can do: write `fn main() { println!("hi"); }` to a SemOS
   file, invoke semos-rustc on it, get a SemOS ELF, SYS_SPAWN that ELF,
   capture "hi" on stdout. End-to-end on-target.
3. The compile completes within memory + time budgets the kernel can
   accommodate (we'll discover these during Phase 5).

This is M27's "done when" bullet #2 + #3 in the current roadmap. Bullet
#4 (`rustc_driver`'s std deps fully satisfied) stays aspirational —
the decisions in §1 explicitly punt on some of them, and that's fine
for v1.

## 6. What to do today

Spawn the four Phase 1 recon agents. Each in its own worktree, each
producing one sub-doc. Total elapsed: 1 session (multi-hour if any).

Phase 1 outputs determine whether Phase 2 starts at all. If R4
identifies a blocker we can't mitigate, we pivot — without spending
30+ sessions discovering it ourselves.
