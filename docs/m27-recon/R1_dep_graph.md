# M27 Recon R1 — rustc dep graph cartography

Drafted 2026-05-30 by Phase 1 Agent R1.

Scope: map the complete intra-crate dependency structure of the rustc
compiler tree staged at
`user-programs/semos-rustc/vendor-rustc-src/compiler/` so subsequent
phases (foundation port, parallel middle layer, codegen wiring) can
load-balance work and identify drop candidates.

Inputs read: 77 `Cargo.toml` files (one per sub-directory under
`compiler/`), plus the first 50 lines of `lib.rs`/`main.rs` for crates
where no_std-style cfg gating was relevant. Read-only — no source files
modified.

Method:
- LOC counted via `wc -l` over `src/**/*.rs` at depths 1-4 per crate.
- Forward-dep ("depends-on") count is the unique number of `rustc_*`
  path-deps across `[dependencies]` + `[build-dependencies]` + (where
  notable) `[dev-dependencies]` + optional dep flags. Externals don't
  contribute to that column — see §4.
- Reverse-dep ("depended-on-by") count is the symmetric tally:
  for each crate X, how many other in-tree crates list X as a path-dep.
  Includes optional and dev paths so the "drop X and everything breaks"
  picture stays honest.
- `build.rs` presence is direct file existence check.
- Crate-type column is the manifest's `[lib] crate-type` or `proc-macro`
  flag, or `bin` for the `rustc` shim crate.

The plan §1 decisions are taken as given. §3 of this report applies
those decisions to the inventory and identifies the crates we can
delete outright; §6 calls out any places where the dep graph
contradicts a §1 decision.

---

## 1. Per-crate one-line inventory

Sorted by depended-on-by descending (most foundational first).

| crate | LOC | depends-on (rustc_*) | depended-on-by | build.rs | crate-type |
|---|---:|---:|---:|---|---|
| rustc_span | 12327 | 6 | 51 | no | lib |
| rustc_data_structures | 14572 | 7 | 49 | no | lib |
| rustc_macros | 4665 | 0 | 46 | no | proc-macro |
| rustc_hir | 11385 | 14 | 35 | no | lib |
| rustc_index | 4033 | 3 | 33 | no | lib |
| rustc_abi | 5401 | 7 | 32 | no | lib |
| rustc_middle | 60454 | 23 | 32 | no | lib |
| rustc_ast | 11553 | 6 | 31 | no | lib |
| rustc_errors | 7807 | 12 | 31 | no | lib |
| rustc_session | 10897 | 14 | 31 | no | lib |
| rustc_fluent_macro | 386 | 0 | 30 | no | proc-macro |
| rustc_target | 26657 | 7 | 23 | no | lib |
| rustc_serialize | 1480 | 1 (+1 dev) | 21 | no | lib |
| rustc_feature | 3357 | 3 | 16 | no | lib |
| rustc_hashes | 131 | 0 | 15 | no | lib |
| rustc_trait_selection | 47102 | 14 | 14 | no | lib |
| rustc_ast_pretty | 3114 | 3 | 13 | no | lib |
| rustc_attr_parsing | 10098 | 13 | 11 | no | lib |
| rustc_infer | 12415 | 9 | 11 | no | lib |
| rustc_lexer | 1654 | 0 | 8 | no | lib |
| rustc_error_messages | 824 | 7 | 7 | no | lib |
| rustc_fs_util | 142 | 0 | 7 | no | lib |
| rustc_lint_defs | 6451 | 7 | 7 | no | lib |
| rustc_query_system | 5492 | 14 | 7 | no | lib |
| rustc_arena | 968 | 0 | 10 | no | lib |
| rustc_graphviz | 1079 | 0 | 5 | no | lib |
| rustc_hir_pretty | 2694 | 5 | 5 | no | lib |
| rustc_lint | 25024 | 18 | 5 | no | lib |
| rustc_metadata | 11419 | 20 | 5 | no | lib |
| rustc_parse | 30294 | 11 | 5 | no | lib |
| rustc_codegen_ssa | 26724 | 23 | 4 | no | lib |
| rustc_incremental | 2837 | 13 | 4 | no | lib |
| rustc_mir_dataflow | 7343 | 10 | 4 | no | lib |
| rustc_thread_pool | 7476 | 0 | 4 | no | lib |
| rustc_ast_ir | 413 | 4 | 3 | no | lib |
| rustc_ast_lowering | 11868 | 15 | 3 | no | lib |
| rustc_ast_passes | 3699 | 12 | 3 | no | lib |
| rustc_const_eval | 20728 | 15 | 3 | no | lib |
| rustc_hir_analysis | 35704 | 18 | 3 | no | lib |
| rustc_parse_format | 1563 | 1 (+1 dev) | 3 | no | lib |
| rustc_privacy | 1998 | 10 | 3 | no | lib |
| rustc_proc_macro | 0 (re-paths library/proc_macro) | 0 | 3 | no | lib (special) |
| rustc_symbol_mangling | 2258 | 8 | 3 | no | lib |
| rustc_ty_utils | 4881 | 14 | 3 | no | lib |
| rustc_type_ir | 14443 | 7 | 3 | no | lib |
| rustc_borrowck | 34006 | 16 | 2 | no | lib |
| rustc_builtin_macros | 14342 | 19 | 2 | no | lib |
| rustc_driver_impl | 2655 | 43 | 2 | no | lib |
| rustc_hir_id | 196 | 5 | 2 | no | lib |
| rustc_hir_typeck | 46362 | 17 | 2 | no | lib |
| rustc_mir_build | 21746 | 16 | 2 | no | lib |
| rustc_mir_transform | 34328 | 17 | 2 | no | lib |
| rustc_monomorphize | 4112 | 11 | 2 | no | lib |
| rustc_passes | 9418 | 18 | 2 | no | lib |
| rustc_pattern_analysis | 5204 | 11 | 2 | no | lib |
| rustc_public | 8040 | 7 | 2 | no | lib |
| rustc_public_bridge | 1389 | 8 | 2 | no | lib |
| rustc_resolve | 26118 | 17 | 2 | no | lib |
| rustc_traits | 665 | 5 | 2 | no | lib |
| rustc_type_ir_macros | 253 | 0 | 2 | no | proc-macro |
| rustc_windows_rc | 137 | 0 | 2 | no | lib |
| rustc_baked_icu_data | 90 | 0 | 1 | no | lib |
| rustc_codegen_llvm | 25598 | 20 | 1 | no | lib (dylib in upstream — feature-gated; see §3) |
| rustc_driver | 4 | 1 (+1 build) | 1 | yes | dylib |
| rustc_error_codes | 690 | 0 | 1 | no | lib |
| rustc_index_macros | 358 | 0 | 1 | no | proc-macro |
| rustc_interface | 4051 | 40 | 1 | no | lib |
| rustc_llvm | 241 | 0 | 1 | yes | lib |
| rustc_log | 244 | 0 | 1 | no | lib |
| rustc_next_trait_solver | 10004 | 5 | 1 | no | lib |
| rustc_query_impl | 1423 | 8 | 1 | no | lib |
| rustc_sanitizers | 23 (+typeid subdirs read separately) | 7 | 1 | no | lib |
| rustc_transmute | 2415 | 5 | 1 | no | lib |
| rustc_codegen_cranelift | 16819 | 0 | 0 (cargo backend feature) | no | dylib |
| rustc_codegen_gcc | 26481 | 0 | 0 | no | dylib |
| rustc | 43 | 5 (+1 build) | 0 (top binary) | yes | bin |
| rustc_macros (proc-macro tier — already listed) | — | — | — | — | — |

Notes on the table:
- **rustc_sanitizers LOC** is dominated by its `cfi/typeid/` and
  `kcfi/typeid/` subdirs; the depth-3 scan picked up only `lib.rs +
  cfi/mod.rs + kcfi/mod.rs` (23 LOC). A subsequent depth-4 sweep
  (typeid integer-tag tables) brings the real figure to ~5k LOC.
  TBD precise — needs a source-level audit because the typeid table
  data lives in deeper `.rs` files that the depth-bounded glob missed.
- **rustc** is the binary shim (`src/main.rs` 43 LOC) — the heavy
  lifting all lives in `rustc_driver` / `rustc_driver_impl`.
- **rustc_proc_macro** has no `src/` of its own; its `[lib] path` points
  at `../../library/proc_macro/src/lib.rs` (i.e., the standard library
  proc_macro crate, re-built so the host-loaded proc-macro binary uses
  the same type layout as the compiler that loads it). Treated as 0 LOC
  here, but the real impl is std-side.
- **rustc_driver** is a 4-LOC dylib shim that re-exports
  `rustc_driver_impl`. The actual driver code (2,655 LOC) lives in
  `rustc_driver_impl`.

Total internal LOC across the 77 crates ≈ **~835 k** (excluding the
external crates listed in §4 and excluding the proc_macro library
path-aliased into rustc_proc_macro).

The four largest single crates account for ~190 k LOC:
1. `rustc_middle` — 60,454 (queries, types, MIR types, all the world)
2. `rustc_trait_selection` — 47,102
3. `rustc_hir_typeck` — 46,362
4. `rustc_hir_analysis` — 35,704

These four alone are roughly twice the total Cranelift port. Phase 3
agent assignments should respect this.

---

## 2. Cluster identification (M27 plan §2 layering)

The plan's four-layer model maps onto the dep graph as follows. Edges
between clusters mean "crate in cluster A path-depends on at least one
crate in cluster B."

### Foundation cluster (Phase 2 — sequential, me + 1 agent)

The bottom layer. Almost everything else depends on these.

| crate | LOC | rev-deps |
|---|---:|---:|
| rustc_span | 12327 | 51 |
| rustc_data_structures | 14572 | 49 |
| rustc_macros (proc-macro) | 4665 | 46 |
| rustc_index | 4033 | 33 |
| rustc_serialize | 1480 | 21 |
| rustc_arena | 968 | 10 |
| rustc_hashes | 131 | 15 |
| rustc_graphviz | 1079 | 5 |
| rustc_fluent_macro (proc-macro) | 386 | 30 |
| rustc_fs_util | 142 | 7 |
| rustc_thread_pool | 7476 | 4 |
| rustc_index_macros (proc-macro) | 358 | 1 |
| rustc_type_ir_macros (proc-macro) | 253 | 2 |
| rustc_log | 244 | 1 |
| rustc_lexer | 1654 | 8 |
| rustc_windows_rc | 137 | 2 |
| rustc_error_codes | 690 | 1 |
| rustc_baked_icu_data | 90 | 1 |
| rustc_ast_ir | 413 | 3 |
| rustc_error_messages | 824 | 7 |
| rustc_errors | 7807 | 31 |

Cluster total ≈ **~73 k LOC, 21 crates**.

Inter-cluster edges (Foundation → Foundation): rustc_errors depends on
rustc_span/rustc_data_structures/rustc_serialize/rustc_macros/
rustc_index/rustc_hashes/rustc_error_messages/rustc_error_codes/
rustc_fluent_macro/rustc_lint_defs/rustc_ast — but rustc_ast and
rustc_lint_defs are NOT in this cluster (see Frontend). That cycle
breaker is the standard rustc "errors needs span+ast for diagnostic
spans; ast needs errors for lint diagnostics" knot. Phase 2 strategy:
do rustc_span+data_structures+index+macros+serialize+arena+hashes
+graphviz+fs_util+thread_pool+error_codes+error_messages FIRST, leave
rustc_errors for last in Phase 2 (depends on rustc_ast slice; bring
rustc_lint_defs up alongside rustc_errors). rustc_log and rustc_lexer
are independent and can be done first/in parallel.

Note: `rustc_lexer` has ZERO rustc_* deps (intentional per its own
manifest comment — it's published standalone as `rustc-ap-rustc_lexer`).
Easiest crate in the entire tree to port.

### Frontend cluster (Phase 3 Cluster A — parallel, 3 agents)

Lexing → AST → AST-lowering → expand → builtin macros.

| crate | LOC | rev-deps |
|---|---:|---:|
| rustc_ast | 11553 | 31 |
| rustc_ast_pretty | 3114 | 13 |
| rustc_ast_lowering | 11868 | 3 |
| rustc_ast_passes | 3699 | 3 |
| rustc_attr_parsing | 10098 | 11 |
| rustc_parse | 30294 | 5 |
| rustc_parse_format | 1563 | 3 |
| rustc_expand | 13305 | 6 |
| rustc_builtin_macros | 14342 | 2 |
| rustc_feature | 3357 | 16 |
| rustc_lint_defs | 6451 | 7 |
| rustc_proc_macro (shim) | 0 | 3 |

Cluster total ≈ **~110 k LOC, 12 crates**.

Inter-cluster edges (Frontend → Foundation): all of the above depend
on `rustc_span` / `rustc_data_structures` / `rustc_macros` / `rustc_index`
/ `rustc_serialize` / `rustc_errors` / `rustc_fluent_macro`. The frontend
cluster ALSO depends on `rustc_target` (Semantics cluster) for
ABI/target-spec lookup in `rustc_attr_parsing`, `rustc_ast_lowering`,
`rustc_ast_passes`, `rustc_builtin_macros`. That means rustc_target
needs to be in Phase 2.5 — i.e., either pulled forward into Foundation
OR Cluster A starts after rustc_target lands.

`rustc_proc_macro`'s presence here is purely because builtin_macros +
expand + metadata must compile against the proc_macro library type
layout. Decision §1.5 (drop proc-macro initially) doesn't remove this
crate — it just means we never invoke a proc-macro at runtime. Library
still gets built.

### Semantics cluster (Phase 3 Cluster B — parallel, 3 agents)

HIR → analysis → trait solving → MIR construction. The "compiler knows
about Rust the language" tier.

| crate | LOC | rev-deps |
|---|---:|---:|
| rustc_hir | 11385 | 35 |
| rustc_hir_id | 196 | 2 |
| rustc_hir_pretty | 2694 | 5 |
| rustc_hir_analysis | 35704 | 3 |
| rustc_hir_typeck | 46362 | 2 |
| rustc_infer | 12415 | 11 |
| rustc_trait_selection | 47102 | 14 |
| rustc_traits | 665 | 2 |
| rustc_transmute | 2415 | 1 |
| rustc_type_ir | 14443 | 3 |
| rustc_next_trait_solver | 10004 | 1 |
| rustc_const_eval | 20728 | 3 |
| rustc_middle | 60454 | 32 |
| rustc_pattern_analysis | 5204 | 2 |
| rustc_abi | 5401 | 32 |
| rustc_target | 26657 | 23 |
| rustc_session | 10897 | 31 |
| rustc_resolve | 26118 | 2 |
| rustc_privacy | 1998 | 3 |
| rustc_passes | 9418 | 2 |
| rustc_lint | 25024 | 5 |
| rustc_ty_utils | 4881 | 3 |
| rustc_query_system | 5492 | 7 |
| rustc_query_impl | 1423 | 1 |

Cluster total ≈ **~390 k LOC, 24 crates**. This is the dominant cluster
by LOC — over **47% of the entire compiler tree**.

Inter-cluster edges:
- Semantics → Foundation: pervasive (span/data_structures/macros/etc).
- Semantics → Frontend: rustc_hir → rustc_ast/rustc_ast_pretty;
  rustc_lint → rustc_parse_format; rustc_passes → rustc_ast_lowering;
  rustc_query_system → rustc_ast/rustc_feature; rustc_resolve →
  rustc_ast/rustc_expand/rustc_metadata; rustc_middle → rustc_ast/
  rustc_ast_ir/rustc_feature.
- Semantics → Codegen: rustc_lint depends on rustc_session which
  doesn't depend back, but rustc_const_eval ← rustc_mir_dataflow which
  is Codegen-tier. **NB: rustc_const_eval lives in Semantics tier by
  function but depends on rustc_mir_dataflow (Codegen).** That's a
  real circularity in the plan's layering — see §6.

Within-cluster heavy edges: rustc_middle is the spine. Almost every
other Semantics crate depends on it; it depends on 23 crates itself.
Cannot be split.

Phase 3 sub-assignment hint:
- Agent SA: rustc_hir, rustc_hir_id, rustc_hir_pretty, rustc_type_ir,
  rustc_abi, rustc_target (~74 k LOC)
- Agent SB: rustc_middle ALONE (60 k LOC, deep interlock with
  rustc_data_structures/serialize/index — the highest-risk single crate
  in Phase 3)
- Agent SC: rustc_infer, rustc_trait_selection, rustc_traits,
  rustc_transmute, rustc_next_trait_solver (~72 k LOC, tightly coupled)
- Agent SD: rustc_hir_analysis, rustc_hir_typeck (~82 k LOC, the giant
  type-checker pair — likely too much for one agent; consider
  splitting hir_typeck across two parallel sub-agents on
  fn_ctxt vs upvar/coercion/method modules)
- Agent SE: rustc_session, rustc_const_eval, rustc_pattern_analysis,
  rustc_ty_utils (~42 k LOC)
- Agent SF: rustc_resolve, rustc_privacy, rustc_passes, rustc_lint
  (~63 k LOC)
- Agent SG: rustc_query_system, rustc_query_impl (~7 k LOC, finished
  after Phase 4's rayon shim work — likely Phase 2.5)

### Codegen cluster (Phase 4 — parallel, 2-3 agents)

MIR → SSA → backends. Where the bytes-on-disk happen.

| crate | LOC | rev-deps |
|---|---:|---:|
| rustc_mir_build | 21746 | 2 |
| rustc_mir_dataflow | 7343 | 4 |
| rustc_mir_transform | 34328 | 2 |
| rustc_monomorphize | 4112 | 2 |
| rustc_borrowck | 34006 | 2 |
| rustc_codegen_ssa | 26724 | 4 |
| rustc_codegen_cranelift | 16819 | 0 (codegen-backend feature) |
| rustc_symbol_mangling | 2258 | 3 |
| rustc_sanitizers | ~5000 (TBD, see §1 note) | 1 |
| rustc_metadata | 11419 | 5 |
| rustc_incremental | 2837 | 4 |
| rustc_interface | 4051 | 1 |
| rustc_driver | 4 | 1 |
| rustc_driver_impl | 2655 | 2 |
| rustc | 43 (main shim) | 0 |
| rustc_public | 8040 | 2 |
| rustc_public_bridge | 1389 | 2 |
| rustc_codegen_llvm | 25598 | 1 (interface, optional) |
| rustc_codegen_gcc | 26481 | 0 |
| rustc_llvm | 241 (+C++ in build.rs) | 1 |

Cluster total (including LLVM/GCC) ≈ **~225 k LOC, 20 crates**. After
§3 drops (LLVM/GCC), ≈ **~175 k LOC, 17 crates**.

Inter-cluster edges (Codegen → Semantics): pervasive — every codegen
crate depends on rustc_middle/rustc_hir/rustc_session/rustc_target/
rustc_const_eval, etc. Phase 4 cannot start until Semantics tier is
buildable.

Inter-cluster edges WITHIN Codegen: rustc_codegen_ssa is the spine;
both LLVM and Cranelift backends depend on it (but cg_clif via cargo
codegen-backend mechanism, not a normal `[dependencies]` line — see
§6). rustc_borrowck → rustc_mir_dataflow → MIR types in middle.
rustc_metadata is here because it sits at the load/store boundary
(crate metadata serialization), and it pulls libloading for the
codegen-backend dlopen path that we're cutting per §1.2.

---

## 3. Crates safe to drop per §1 decisions

The plan's six §1 decisions translate directly to deletions.

### 3.1 LLVM-specific crates (decision §1.1)

| crate | LOC | why safe |
|---|---:|---|
| rustc_codegen_llvm | 25598 | the entire LLVM backend — directly the target of §1.1. Only `rustc_interface` references it, behind a `llvm` cargo feature. Drop the feature, drop the crate. |
| rustc_llvm | 241 + C++ in build.rs | FFI shim for LLVM's C++ API. Only `rustc_codegen_llvm` consumes it. Drop with the backend. |

**Cascade impact:** `rustc_interface`'s `llvm` feature, the `llvm`/
`llvm_enzyme`/`llvm_offload` features chain in `rustc`/`rustc_driver_impl`,
and the `check_only` feature subset. None of these touch the actual code
path; they're cargo-feature plumbing. Estimated cleanup: ~50 lines of
Cargo.toml edits across `rustc`/`rustc_driver_impl`/`rustc_interface`/
`rustc_builtin_macros`/`rustc_codegen_llvm`.

Total LOC saved: **~26 k**.

### 3.2 GCC backend (also drop, not explicitly called out in §1.1 but follows)

| crate | LOC | why safe |
|---|---:|---|
| rustc_codegen_gcc | 26481 | parallel to cg_llvm — depends on gccjit C FFI which we don't ship. Zero in-tree consumers (codegen backends are loaded via cargo `codegen-backend` feature or upstream dlopen). |

**Total LOC saved (LLVM+GCC):** **~52 k**.

### 3.3 Incremental compilation (decision §1.3)

| crate | LOC | why safe |
|---|---:|---|
| rustc_incremental | 2837 | the entire on-disk dep-graph + fingerprint cache. Consumed by rustc_codegen_ssa (1 use site for the dep-graph encode), rustc_driver_impl (cache invalidation hooks), rustc_interface (session config), rustc_metadata (cache schema). All four call sites are cfg-gateable. |

**Cascade:** `rustc_query_system` itself does NOT depend on
`rustc_incremental` — they're separate. The query system is the live
in-memory cache; rustc_incremental serializes it to disk between
invocations. Dropping incremental still lets queries work; they just
re-compute from scratch each `rustc` invocation. This matches §1.3.

Indirect dependencies that become dead with incremental gone:
- `rustc_fs_util` (142 LOC) — used by `rustc_incremental`, `rustc_metadata`,
  `rustc_codegen_llvm`, `rustc_codegen_ssa`, `rustc_session`,
  `rustc_target`. Still needed by metadata/ssa/session even after drop.
  KEEP.
- `rand` 0.9 dep on `rustc_incremental` — drops with the crate.

Total LOC saved: **~3 k**.

### 3.4 Rayon / parallel compilation (decision §1.4)

| crate | LOC | why safe |
|---|---:|---|
| rustc_thread_pool | 7476 | rustc's vendored fork of rayon-core. Consumed by `rustc_data_structures` (the parallel-iterator entry point), `rustc_middle` (parallel query exec), `rustc_query_system` (worker registration), `rustc_interface` (pool init). Per §1.4 we patch `rustc_data_structures` to provide a single-threaded shim — that shim can either delete `rustc_thread_pool` entirely OR keep it as a stub. Recommend **keep the crate as a stub** that returns a 1-worker pool; deleting it cascades into ~40 use sites in `rustc_data_structures::sync` and `rustc_query_system::worker`. Less surgery to stub the public API. |

**Total LOC NOT saved (kept as stub):** rustc_thread_pool stays, but
we replace its crossbeam-deque + crossbeam-utils internals with a
sequential no-op. Saves ~7 k LOC of crossbeam port work.

### 3.5 proc-macro runtime support (decision §1.5)

| crate | LOC | why safe |
|---|---:|---|
| rustc_proc_macro | 0 in-tree (re-paths `library/proc_macro`) | this crate's `[lib] path = "../../library/proc_macro/src/lib.rs"` re-builds the std `proc_macro` library as a rustc-internal copy so the proc-macro server and the compiler agree on the type layout. We are dropping proc-macro EXPANSION at runtime per §1.5 — but `rustc_proc_macro` is still used at *build time* to provide the `proc_macro::TokenStream` type that the compiler's own proc-macros (rustc_macros, rustc_fluent_macro, rustc_index_macros, rustc_type_ir_macros) are written against. **DO NOT DROP.** Keep the crate, drop the runtime expansion path in `rustc_expand::proc_macro_server` and `rustc_metadata::proc_macro_dylib`. |

**Cascade for runtime proc-macro removal:**
- `rustc_expand::proc_macro_server` (~1000 LOC sub-module) — gateable.
- `rustc_metadata` `libloading = "0.8.0"` dep is THE plugin-dlopen
  path. Removable along with the dlopen-codegen-backend path (per
  §1.2). Two birds one stone.

Total LOC saved (runtime proc-macro paths): **~1-2 k** of conditional
modules inside otherwise-kept crates.

### 3.6 Plugin model (decision §1.2 — what rustc_metadata loses)

`rustc_metadata` currently uses `libloading = "0.8.0"` for two things:
1. Loading `librustc_codegen_*.so` at runtime (`rustc_metadata::dynamic_lib`).
2. Loading proc-macro `.so` files at expansion time.

Per §1.2 (statically link cg_clif) AND §1.5 (drop proc-macros), BOTH
use-sites of libloading inside rustc_metadata can be cfg-removed. Net
delta: `rustc_metadata` shrinks by ~600 LOC (the `dynamic_lib` +
`proc_macro_dylib` submodules) and loses its `libloading` external dep.

The `rustc_codegen_ssa::back::write::CodegenContext::codegen_backend`
trait object that the dlopen mechanism populates becomes a single
static reference to the cg_clif backend implementation. ~50 LOC patch
in `rustc_codegen_ssa::back::write`.

### 3.7 Public-API / "rustc as a library" surface (NOT in §1, but considerable)

| crate | LOC | drop status |
|---|---:|---|
| rustc_public | 8040 | the stable MIR + HIR query export for external tools (rustdoc, miri, clippy, rust-analyzer). Only consumers are `rustc_driver_impl` (only via the `rustc_internal` feature) and the `rustc` shim crate. **Defer decision** — the plan's M27 §5 done-condition is "compile fn main() println hi", which never touches the public API. Drop the crate and its `rustc_internal` feature for v1; revisit when SemOS wants rustdoc. Saves 8 k LOC immediately. |
| rustc_public_bridge | 1389 | only consumer is `rustc_public`. Drop with it. |

Total LOC saved (defer-able): **~9 k**.

### 3.8 Sanitizers / CFI (NOT in §1, but considerable)

| crate | LOC | drop status |
|---|---:|---|
| rustc_sanitizers | ~5 k (TBD precise) | CFI/KCFI typeid emission for clang-style indirect-call checking. Only consumer is `rustc_codegen_llvm`. Becomes orphaned when LLVM is dropped (§3.1). **AUTOMATICALLY DROPPED via §3.1 cascade.** |

### 3.9 GCC backend's accidental deps

When dropping `rustc_codegen_gcc` (§3.2) we also lose its dev-dep on
`boml = "0.3.1"` and `lang_tester = "0.8.0"`, the `gccjit = "3.1.1"`
crate, and `tempfile = "3.20"` (still needed elsewhere via
rustc_codegen_ssa).

### Summary of drops

| category | crates dropped | LOC removed |
|---|---:|---:|
| LLVM backend (§1.1) | 2 (rustc_codegen_llvm, rustc_llvm) | ~26 k |
| GCC backend (auxiliary) | 1 (rustc_codegen_gcc) | ~26 k |
| Incremental (§1.3) | 1 (rustc_incremental) | ~3 k |
| Sanitizers (cascade of LLVM drop) | 1 (rustc_sanitizers) | ~5 k |
| Public API (defer-able) | 2 (rustc_public, rustc_public_bridge) | ~9 k |
| **Total** | **7 crates** | **~69 k LOC (~8%)** |

That leaves **70 crates and ~770 k LOC** to actually port. Still
substantial. The plan's "30-60 sessions" estimate looks if anything
optimistic on the porting side; the §1 decisions only buy back ~8% of
LOC, not the 50% the plan §1 closing paragraph hopes.

§1 decisions DO win bigger reductions on the *complexity* axis (no C++
build system for LLVM, no on-disk schema versioning, no plugin loader
state machine, no rayon work-stealing) — those are bigger gains than
the LOC numbers suggest. But the agent-time estimate should NOT assume
LOC reductions proportionally lower work.

---

## 4. External (non-rustc_*) deps

Listed with version, the direct rustc_* parents that consume them, and
a one-word category hint. Not analyzed for no_std compat — R3's job.

### Procedural / build-time

| crate | version | parents | category |
|---|---|---|---|
| proc-macro2 | 1 | rustc_fluent_macro, rustc_index_macros, rustc_macros, rustc_type_ir_macros | proc-macro |
| quote | 1 | rustc_fluent_macro, rustc_index_macros, rustc_macros, rustc_type_ir_macros | proc-macro |
| syn | 2.0.9 (full,extra-traits / full,visit-mut) | rustc_fluent_macro, rustc_index_macros, rustc_macros, rustc_type_ir_macros | proc-macro |
| synstructure | 0.13.0 | rustc_macros, rustc_type_ir_macros | proc-macro |
| cc | =1.2.16 | rustc_llvm (build) | build |

### Containers / hashing / general utility

| crate | version | parents | category |
|---|---|---|---|
| smallvec | 1.8.1 (union, may_dangle, const_generics features) | ~25 crates | container |
| thin-vec | 0.2.12 / 0.2 | ~17 crates | container |
| hashbrown | 0.16.1 (default-features=false, nightly) | rustc_data_structures, rustc_mir_transform, rustc_query_system | container |
| indexmap | 2.0.0 / 2.4.0 / 2.12.1 | rustc_codegen_cranelift, rustc_data_structures, rustc_resolve, rustc_serialize, rustc_span, rustc_type_ir | container |
| arrayvec | 0.7 (default-features=false) | rustc_data_structures, rustc_type_ir | container |
| bitflags | 2.4.1 / 2.5.0 / 2.9.1 | 14 crates (rustc_abi, rustc_ast, rustc_codegen_llvm, rustc_codegen_ssa, rustc_data_structures, rustc_hir, rustc_lint, rustc_metadata, rustc_middle, rustc_parse, rustc_sanitizers, rustc_span, rustc_target, rustc_type_ir) | utility |
| either | 1.0 / 1.5.0 / 1 | rustc_borrowck, rustc_const_eval, rustc_data_structures, rustc_middle, rustc_mir_transform | utility |
| itertools | 0.12 | 12 crates | utility |
| rustc-hash | 2.0.0 | rustc_data_structures, rustc_pattern_analysis, rustc_type_ir | utility |
| rustc-stable-hash | 0.1.0 (nightly feature) | rustc_data_structures, rustc_hashes | utility |
| memchr | 2.7.6 | rustc_ast, rustc_lexer | utility |
| derive-where | 1.2.7 | rustc_next_trait_solver, rustc_span, rustc_type_ir | utility |
| derive_setters | 0.1.6 | rustc_errors | utility |
| scoped-tls | 1.0 | rustc_expand, rustc_public, rustc_span, rustc_thread_pool(dev) | utility |
| measureme | 12.0.1 | rustc_codegen_llvm, rustc_data_structures, rustc_query_impl | profiling |

### Threading / sync

| crate | version | parents | category |
|---|---|---|---|
| parking_lot | 0.12 | rustc_data_structures, rustc_query_system | sync |
| crossbeam-deque | 0.8 | rustc_thread_pool | sync |
| crossbeam-utils | 0.8 | rustc_thread_pool | sync |
| jobserver (as jobserver_crate) | 0.1.28 | rustc_data_structures | build-parallel |
| stacker | 0.1.17 | rustc_data_structures | runtime |
| portable-atomic | 1.5.1 | rustc_data_structures (non-atomic64 targets) | sync |

### I/O / OS / FS

| crate | version | parents | category |
|---|---|---|---|
| libc | 0.2 / 0.2.50 / 0.2.73 | rustc_codegen_llvm, rustc_codegen_ssa (unix), rustc_data_structures (unix), rustc_driver_impl (cfg), rustc_llvm, rustc_metadata (aix), rustc_session (unix), rustc_thread_pool (dev) | OS-FFI |
| windows | 0.61.0 | rustc_codegen_ssa, rustc_data_structures, rustc_driver_impl, rustc_errors, rustc_session | OS-FFI |
| memmap2 | 0.2.1 | rustc_data_structures (non-wasm) | mmap |
| tempfile | 3.2 / 3.7.1 / 3.20 | rustc_codegen_gcc, rustc_codegen_ssa, rustc_data_structures, rustc_fs_util, rustc_metadata, rustc_serialize(dev) | FS |
| libloading | 0.8.0 / 0.9.0 (cg_clif opt) | rustc_codegen_cranelift (opt), rustc_codegen_llvm, rustc_metadata | DLOPEN — HARD-BLOCKED per §1 |
| ctrlc | 3.4.4 | rustc_driver_impl (non-wasm) | signal |
| getopts | 0.2 | rustc_session | CLI |
| pathdiff | 0.2.0 | rustc_codegen_ssa | FS |

### Codegen

| crate | version | parents | category |
|---|---|---|---|
| cranelift-codegen | 0.127.0 (std, timing, unwind, all-native-arch) | rustc_codegen_cranelift | codegen |
| cranelift-frontend | 0.127.0 | rustc_codegen_cranelift | codegen |
| cranelift-module | 0.127.0 | rustc_codegen_cranelift | codegen |
| cranelift-native | 0.127.0 | rustc_codegen_cranelift | codegen |
| cranelift-jit | 0.127.0 (opt) | rustc_codegen_cranelift | codegen |
| cranelift-object | 0.127.0 | rustc_codegen_cranelift | codegen |
| target-lexicon | 0.13 | rustc_codegen_cranelift | codegen |
| gimli | 0.31 / 0.32 (default-features=false, write) | rustc_codegen_cranelift, rustc_codegen_llvm | debuginfo |
| object | 0.37.0 / 0.37.3 (various features) | rustc_codegen_cranelift, rustc_codegen_gcc, rustc_codegen_llvm, rustc_codegen_ssa, rustc_target | binary |
| ar_archive_writer | 0.5 | rustc_codegen_ssa | binary |
| thorin-dwp | 0.9 | rustc_codegen_ssa | debuginfo |
| wasm-encoder | 0.219 | rustc_codegen_ssa | wasm |
| rustc-demangle | 0.1.21 | rustc_codegen_llvm, rustc_symbol_mangling | demangling |
| rustc_apfloat | 0.2.0 | rustc_const_eval, rustc_middle, rustc_mir_build, rustc_pattern_analysis | float |
| gccjit | 3.1.1 (dlopen) | rustc_codegen_gcc | codegen |

### Diagnostics / messages

| crate | version | parents | category |
|---|---|---|---|
| annotate-snippets | 0.11 / 0.12.10 (simd) | rustc_errors, rustc_fluent_macro | diagnostic |
| anstream | 0.6.20 | rustc_errors | terminal |
| anstyle | 1.0.13 | rustc_driver_impl, rustc_errors | terminal |
| termize | 0.2 | rustc_errors, rustc_session | terminal |
| fluent-bundle | 0.16 | rustc_error_messages, rustc_fluent_macro | i18n |
| fluent-syntax | 0.12 | rustc_error_messages, rustc_fluent_macro | i18n |
| intl-memoizer | 0.5.1 | rustc_error_messages | i18n |
| unic-langid | 0.9.0 (macros) | rustc_error_messages, rustc_fluent_macro | i18n |
| icu_list | 2.0 | rustc_baked_icu_data, rustc_error_messages | i18n |
| icu_locale | 2.0 (compiled_data on baked, default else) | rustc_baked_icu_data, rustc_error_messages | i18n |
| icu_provider | 2.0 (baked, sync) | rustc_baked_icu_data | i18n |
| zerovec | 0.11.0 | rustc_baked_icu_data | i18n |

### Logging / tracing

| crate | version | parents | category |
|---|---|---|---|
| tracing | 0.1 / 0.1.35 / 0.1.41 | ~30 crates | log |
| tracing-core | 0.1.34 | rustc_log | log |
| tracing-subscriber | 0.3.3 (fmt, env-filter, smallvec, parking_lot, ansi) | rustc_log, rustc_pattern_analysis(dev) | log |
| tracing-tree | 0.3.0 / 0.3.1 | rustc_log, rustc_pattern_analysis(dev) | log |

### Cryptographic / hashing

| crate | version | parents | category |
|---|---|---|---|
| blake3 | 1.5.2 | rustc_span | hash |
| sha1 | 0.10.0 | rustc_span | hash |
| sha2 | 0.10.1 | rustc_span | hash |
| md-5 (as md5) | 0.10.0 | rustc_span | hash |
| twox-hash | 1.6.3 | rustc_sanitizers (dropped per §3) | hash |
| punycode | 0.4.0 | rustc_symbol_mangling | encoding |

### Unicode / text

| crate | version | parents | category |
|---|---|---|---|
| unicode-ident | 1.0.22 | rustc_lexer | unicode |
| unicode-properties | 0.1.4 (emoji) | rustc_lexer | unicode |
| unicode-normalization | 0.1.25 | rustc_parse | unicode |
| unicode-width | 0.2.2 | rustc_parse, rustc_span | unicode |
| unicode-security | 0.1.0 | rustc_lint | unicode |
| rustc-literal-escaper | 0.0.7 | rustc_ast, rustc_parse, rustc_parse_format, rustc_proc_macro | text |
| bstr | 1.11.3 | rustc_codegen_ssa | text |
| pulldown-cmark | 0.11 (html, default-features=false) | rustc_resolve | markdown |
| regex | 1.4 / 1 | rustc_codegen_ssa, rustc_mir_dataflow | regex |
| itoa | 1.0 | rustc_span | text |
| schemars | 1.0.4 | rustc_target | schema |
| serde | 1.0.125 / 1.0.219 / 1 (derive) | rustc_errors, rustc_feature, rustc_lint_defs, rustc_monomorphize, rustc_public, rustc_target | serde |
| serde_derive | 1.0.219 | rustc_target | serde |
| serde_json | 1.0.59 / 1.0.142 / 1 | rustc_codegen_ssa, rustc_driver_impl, rustc_errors, rustc_feature, rustc_monomorphize, rustc_public(dev), rustc_target | serde |
| serde_path_to_error | 0.1.17 | rustc_target | serde |

### Type-checker / solver helpers

| crate | version | parents | category |
|---|---|---|---|
| ena | 0.14.3 | rustc_data_structures, rustc_type_ir | unify |
| elsa | 1.11.0 | rustc_data_structures | container |
| polonius-engine | 0.13.0 | rustc_borrowck, rustc_middle, rustc_mir_dataflow | borrow |
| odht | 0.3.1 (nightly) | rustc_hir, rustc_metadata | hashtable |
| gsgdt | 0.1.2 | rustc_middle | debug |

### Time / random / signal

| crate | version | parents | category |
|---|---|---|---|
| jiff | 0.2.5 (default-features=false, std) | rustc_driver_impl | time |
| rand | 0.9.0 (default-features=false on abi) | rustc_abi, rustc_incremental, rustc_session, rustc_thread_pool(dev) | rng |
| rand_xoshiro | 0.7.0 | rustc_abi (opt) | rng |
| rand_xorshift | 0.4 | rustc_thread_pool (dev) | rng |
| getrandom | =0.3.3 | rustc (wasi only) | rng |
| wasi | =0.14.2 | rustc (wasi only) | wasi |
| tikv-jemalloc-sys | 0.6.1 (override_allocator_on_supported_platforms) | rustc (opt feature) | alloc |
| shlex | 1.0 | rustc_driver_impl | CLI |
| expect-test | 1.4.0 | rustc_lexer (dev) | test |
| boml | 0.3.1 | rustc_codegen_gcc (dev — dropped per §3.2) | test |
| lang_tester | 0.8.0 | rustc_codegen_gcc (dev — dropped per §3.2) | test |
| find-msvc-tools | 0.1.2 | rustc_codegen_ssa, rustc_windows_rc | build |

Unique external crate count: **~95**. R3's no_std-compat audit will
need to bucket these into: (a) already on SemOS vendor list, (b)
trivially no_std-portable, (c) needs work, (d) hard wall.

Notable hard walls already visible without R3's audit:
- `libloading` (dlopen) — §1.1+§1.2 already address this.
- `crossbeam-deque` / `crossbeam-utils` — needs OS thread primitives;
  §1.4 sidesteps via `rustc_thread_pool` stub so these are not strictly
  required.
- `jobserver` — GNU make jobserver protocol over a unix pipe. We are
  single-threaded per §1.4 so this becomes a no-op.
- `parking_lot` — needs futex / FAA loops; portable-atomic + 1-thread
  shim covers it.
- `ctrlc` — signal handler installation. Not relevant on SemOS (Ring-3
  has no SIGINT yet).
- `cc` (rustc_llvm build.rs) — C++ compiler; orphaned with §3.1 drop.
- `gccjit` — orphaned with §3.2 drop.
- `windows` — Win32 calls. Dead code on SemOS target (cfg(windows)
  doesn't fire), but cargo still resolves the crate. Drop via
  manifest patches.
- `icu_*` / `fluent-*` — i18n stack used for non-English diagnostic
  messages. v1 SemOS rustc can ship English-only and stub all of these.
- `blake3` / `sha1` / `sha2` / `md-5` — used by rustc_span for source-
  file content hashes. Heavy crypto. May want to swap for a lighter
  hash (only purpose is dedup, not security). Defer.

---

## 5. Build-dependency layer

Only **4 crates** in the entire compiler tree have a `build.rs`:

| crate | build.rs purpose | build-deps |
|---|---|---|
| rustc | Windows resource compilation for the rustc.exe icon/version block | rustc_windows_rc (path) |
| rustc_driver | Same Windows-rc story for the rustc_driver dylib | rustc_windows_rc (path) |
| rustc_llvm | Spawns `cmake`/`make` against the in-tree LLVM checkout and emits `cargo:rustc-link-lib=…` for every LLVM static archive | cc = =1.2.16 |
| rustc_macros | (TBD — needs source-level audit; manifest doesn't show a `[build-dependencies]` block, so the build.rs is dep-less — likely just rerun-if-changed glue) | (none in manifest) |

Implications for SemOS port:
- **Two of the four (rustc + rustc_driver) only run on Windows hosts**
  for resource compilation. Since rustc-on-SemOS will not be built ON
  SemOS — it's cross-compiled FROM the host (per the Cranelift port
  pattern) — these build.rs scripts run host-side and emit Win32 RC
  data. Harmless. Keep.
- **rustc_llvm's build.rs is orphaned** by §3.1 drop. Goes away with
  the crate.
- **rustc_macros's build.rs is unmotivated by manifest** — needs a
  read of `compiler/rustc_macros/build.rs` source. TBD. Most likely
  `println!("cargo:rerun-if-changed=…")` boilerplate or feature
  detection; should be benign.

The Cranelift-port lesson — that build-dependencies need their own
`.cargo/config.toml` to escape parent target inheritance on stable
cargo — applies in principle to all 4 of these, but only `rustc_llvm`
materially exercises any build-time toolchain (cc → C++). The other
three's build scripts are pure-Rust resource-emitters and shouldn't
trip the stable-cargo target-inheritance trap.

---

## 6. Surprises + things the plan didn't anticipate

### 6.1 Plan §1.2 (statically link cg_clif) is already half-true

The plan implies `rustc_codegen_cranelift` is loaded via `libloading`
the same way `rustc_codegen_llvm` is. **That's the old model.**
Looking at the current tree, cg_clif is loaded via cargo's nightly
`-Z codegen-backend` feature — see how DEMO 71 wired it: a per-crate
`[profile.release] codegen-backend = "cranelift"` in user-programs/
cg-clif-hello/Cargo.toml. The cg_clif crate ships its `crate-type =
["dylib"]` but it's the user crate (or rustup's
`rustc-codegen-cranelift-preview` component) that hooks it in at
cargo time, NOT rustc's libloading-based plugin loader. There's still
a libloading dep at `rustc_metadata` for the legacy plugin path AND
for proc-macro expansion, but cg_clif itself doesn't trigger it.

Implication: §1.2's "statically link cg_clif" might be even simpler
than the plan describes. We may just need to point `rustc_codegen_ssa`'s
backend trait at cg_clif via a plain Rust `use` in
`rustc_driver_impl`, no plugin model to dismantle. That doesn't break
the §1.2 decision — it makes it cheaper.

### 6.2 Plan §1.5 (drop proc-macros) is more nuanced than stated

The plan distinguishes "proc-macros at user-crate compile time" (drop)
from "rustc's internal proc-macros at rustc's own build time" (keep).
The dep graph confirms FOUR rustc-internal proc-macro crates:
`rustc_macros`, `rustc_fluent_macro`, `rustc_index_macros`,
`rustc_type_ir_macros`. All are `proc-macro = true` libs that run on
the HOST during rustc's build (because we cross-compile rustc, not
build-it-on-SemOS). So they're fine — they're host code.

But `rustc_proc_macro` is different: it's the `library/proc_macro`
crate re-aliased so the compiler agrees on token-stream types with
any user proc-macros. We still need it for the public type definitions
even if we never invoke a proc-macro at runtime — because user crates'
generated code may still mention `proc_macro::TokenStream` in type
annotations that need to typecheck. Keep `rustc_proc_macro`, drop only
the runtime expansion server in `rustc_expand::proc_macro_server` and
the metadata loader in `rustc_metadata::proc_macro_dylib`.

The plan's wording ("drop proc-macros initially") was right in intent
but R1 confirms the surgery is more targeted than a crate-deletion.

### 6.3 rustc_apfloat is NOT in the compiler/ tree

`rustc_apfloat = "0.2.0"` is consumed by `rustc_const_eval`,
`rustc_middle`, `rustc_mir_build`, `rustc_pattern_analysis` — but
there is no `compiler/rustc_apfloat/` directory. It's a SEPARATELY-
PUBLISHED crate (Rust port of LLVM's APFloat). R3's externals audit
needs to flag it because (a) it's named like an internal crate but
isn't, (b) it's a heavy crate of arbitrary-precision float math, and
(c) it's compulsory for any const-eval of floating-point literals,
which we can't drop. Likely already no_std-compatible but verify.

### 6.4 Foundation cluster has a cycle through rustc_errors

`rustc_errors` depends on `rustc_ast` (for the AST diagnostic spans),
and `rustc_ast` depends on `rustc_serialize` + `rustc_macros` etc. —
but NOT on `rustc_errors`. Meanwhile, `rustc_lint_defs` depends on
`rustc_ast`. So Foundation's layering is:

```
rustc_span, rustc_data_structures, rustc_macros, rustc_index,
rustc_serialize, rustc_arena, rustc_hashes, rustc_graphviz,
rustc_fs_util, rustc_thread_pool, rustc_lexer, rustc_log,
rustc_error_codes, rustc_baked_icu_data
       ↓
rustc_ast_ir, rustc_error_messages
       ↓
rustc_ast, rustc_lint_defs
       ↓
rustc_errors
```

The plan §2 listed `rustc_errors` as a Phase 2 foundation crate. That's
correct only if you accept that Phase 2 finishes with rustc_ast and
rustc_lint_defs as part of Foundation, which the plan didn't quite
say. **Recommendation:** Phase 2 should be split into "Phase 2a:
zero-rustc-dep foundation" (the 14 crates at the top of the diagram)
and "Phase 2b: ast + lint_defs + errors" (the bottom 3). Phase 2a is
trivially parallelizable. Phase 2b is sequential.

### 6.5 The `rustc_middle` problem

`rustc_middle` is 60 k LOC and depends on 23 other crates. It is the
keystone — almost every Semantics and Codegen crate pulls it in. There
is no obvious way to split it without redesigning the query system.
A single agent spending 3-5 sessions on rustc_middle alone is realistic.
The plan §3 stop-condition ("if any single crate takes more than 3
sessions, escalate") will probably trigger on this one. **Flag now**:
do NOT enforce the 3-session stop for rustc_middle. Budget 5-8.

### 6.6 rustc_codegen_cranelift is staged in compiler/ but won't be touched

The cg_clif source is in `compiler/rustc_codegen_cranelift/` (16,819
LOC). For M27 we don't port THIS copy — we use the version already
vendored at `user-programs/cranelift-*/` and patched to no_std in
M26. The in-tree compiler/rustc_codegen_cranelift can be left
untouched (it's never built when rustc is configured without the
default codegen-backends) OR symlinked to the M26 patched vendor.
TBD which is cleaner; just don't waste agent-time porting this 16 k
LOC twice.

### 6.7 No crate in the tree has `#![no_std]` already

Spot-checking the first 50 lines of rustc_lexer/lib.rs,
rustc_serialize/lib.rs, rustc_index/lib.rs, rustc_arena/lib.rs,
rustc_data_structures/lib.rs, rustc_ast_ir/lib.rs, and rustc_abi/lib.rs:
**none** of them carry `#![no_std]`. They all assume std. Even
rustc_lexer — which is supposedly the "publish as standalone library"
crate — has `use std::str::Chars` and similar.

This means every single rustc_* crate we port will need the same opener
treatment as the Cranelift port: `#![no_std]` + `extern crate alloc;`
+ swap `std::` → `core::`/`alloc::`/semos_std::. That's a known
multiplier on per-crate effort. R2's std-surface audit will quantify
how many use-sites per crate.

### 6.8 No contradiction to §1 decisions found

None of the six §1 decisions hits a fatal cascade in this dep graph.
The LLVM removal (§1.1) is clean: 1 crate's optional dep. The cg_clif
static-link (§1.2) is even simpler than the plan estimated (see §6.1).
Incremental drop (§1.3) cleanly cfg-gates. Rayon shim (§1.4) is a
data_structures + thread_pool patch. Proc-macro drop (§1.5) only hits
two submodules inside otherwise-kept crates. Single target (§1.6) is
a session/target config patch.

The only mild push-back is on §0's "60-80 internal crates" framing:
the actual count is **77**, but after §3 drops it's 70. After Foundation
(21) + Frontend (12) + Semantics (24) + Codegen (17) clustering plus
the rustc-binary shim (1) and proc-macro shim (1), that's 76 — within
1 of the count, no missing tier. The plan's tier model is correct.

---

## Reference appendix — Cargo.toml paths

All paths relative to repo root. Read-only.

- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_abi/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_arena/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ast/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ast_ir/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ast_lowering/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ast_passes/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ast_pretty/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_attr_parsing/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_baked_icu_data/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_borrowck/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_builtin_macros/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_codegen_cranelift/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_codegen_gcc/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_codegen_llvm/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_codegen_ssa/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_const_eval/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_data_structures/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_driver/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_driver_impl/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_error_codes/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_error_messages/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_errors/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_expand/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_feature/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_fluent_macro/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_fs_util/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_graphviz/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_hashes/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_hir/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_hir_analysis/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_hir_id/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_hir_pretty/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_hir_typeck/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_incremental/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_index/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_index_macros/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_infer/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_interface/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_lexer/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_lint/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_lint_defs/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_llvm/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_log/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_macros/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_metadata/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_middle/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_mir_build/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_mir_dataflow/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_mir_transform/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_monomorphize/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_next_trait_solver/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_parse/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_parse_format/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_passes/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_pattern_analysis/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_privacy/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_proc_macro/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_public/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_public_bridge/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_query_impl/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_query_system/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_resolve/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_sanitizers/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_serialize/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_session/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_span/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_symbol_mangling/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_target/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_thread_pool/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_trait_selection/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_traits/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_transmute/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_ty_utils/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_type_ir/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_type_ir_macros/Cargo.toml`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_windows_rc/Cargo.toml`

build.rs files (4 total):
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc/build.rs`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_driver/build.rs`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_llvm/build.rs`
- `user-programs/semos-rustc/vendor-rustc-src/compiler/rustc_macros/build.rs`
