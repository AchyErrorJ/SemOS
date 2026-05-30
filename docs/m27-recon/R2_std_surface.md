# R2 — rustc std-surface audit

Phase 1 recon for M27 (rustc on SemOS). Characterizes the std::* surface
of every `rustc_*` internal crate so we can estimate porting cost.
Read-only audit; no source edits.

Method: ripgrep counts per crate for `use std::`, `std::process`,
`std::fs`, `std::thread`, `std::sync`, `std::collections`, `std::io`,
`std::path`, `std::env`, `std::os::`, `std::time`, plus `unsafe impl`
and `no_std`. Cross-checked against each crate's `Cargo.toml` feature
gating. LOC = wc -l over `src/**/*.rs` (tests included where present).

Baseline (whole `compiler/` tree, 76 rustc_* crates + rustc proxy):
`use std::` matches ~875 files, total ~4500 std import lines. Zero
crates self-declare `#![no_std]`. Zero crates have a `default = ["std"]`
feature flag — the Cranelift "flip default=[]" recipe applies to NONE
of them out of the box. The closest analog is the `nightly` feature
on 5 crates (rustc_abi/_ast_ir/_index/_type_ir/_next_trait_solver/
_pattern_analysis), but that gates `dep:rustc_data_structures` and
similar bridges, NOT the std surface itself.

That last sentence is the headline. The whole rustc tree assumes std;
nothing was designed for embedded use. Porting cost is therefore
**substitution-bounded** (sed-and-replace `std::*` → `core::*`/
`alloc::*`/`hashbrown::*`/`semos_std::*`) plus carving out a handful
of architectural blockers.

The semos-std shim today provides: `fs`, `io`, `net`, `process`, `env`,
`path`, `sync`, `thread`, `time`, `mpsc`, `collections`, plus the
standard `alloc` re-exports (`vec`, `string`, `boxed`, `format`, `rc`)
and the `core` re-exports. Anything mappable to those modules counts
as NEEDS-SHIM, not ARCHITECTURAL.

## 1. Cost classification

Four buckets, sorted ARCHITECTURAL → NEEDS-SHIM → MECHANICAL → TRIVIAL.

Bucket definitions:
- **ARCHITECTURAL**: uses `std::thread::spawn`/JoinHandle, dynamic
  loading via `libloading`, `std::process::Command` for sub-compilers,
  or `std::os::*` ungated. Removing requires architectural surgery OR
  a §1-decision drop.
- **NEEDS-SHIM**: `std::fs`/`std::path`/`std::sync::{Mutex,RwLock,Once}`/
  `std::io::{Read,Write,BufReader,stderr}` heavy. Semos-std covers the
  surface but per-call review needed.
- **MECHANICAL**: std use is `std::fmt`, `std::cmp`, `std::convert`,
  `std::mem`, `std::ops`, `std::sync::atomic`, `std::collections::{
  HashMap,HashSet}`. Bulk sed → `core::*`/`alloc::*`/`hashbrown::*`.
- **TRIVIAL**: <5 `use std::` lines AND no big subsystem use.
  Reality check: zero crates qualify because none have the
  default=["std"] flag; even tiny crates need a `#![no_std]` injection
  and one prelude block.

Counts are exact where reported, "100+" where saturating, "-" where 0.

| crate | LOC (k) | classification | top std modules (count) | notes |
|---|---|---|---|---|
| rustc_thread_pool | ~3 | ARCHITECTURAL | thread:24, sync:36, io:- | The vendored rayon fork. 100% of its purpose is `std::thread::spawn` + `JoinHandle` + scope-tree. Per §1.4 we DROP rayon entirely and write a 1-file sequential shim that exposes the same `join`/`scope`/`spawn` shapes as no-ops/inline calls. Crate as-is is unportable. |
| rustc_codegen_llvm | ~25 | ARCHITECTURAL | io:8, process:1, sync:6 | Links libllvm-c via `rustc_llvm`. §1.1 drops this entirely. We skip auditing in detail per the §1 dropdecisions. (Still tracked here in case the drop gets revisited.) |
| rustc_codegen_gcc | ~30 | ARCHITECTURAL | process:7, fs:30, env:30 | libgccjit FFI backend. Not in our scope per §1.1. The build_system/ tree alone has 30+ fs uses; gone with drop. |
| rustc_llvm | ~1 | ARCHITECTURAL | process:2, env:2, fs:1 | build.rs only. Drops with §1.1. |
| rustc_metadata | ~12 | ARCHITECTURAL | fs:13, path:5, io:6 | The plugin-load mechanism: `libloading 0.8` dep loads `librustc_codegen_*.{so,dll}` at runtime. Statically linking cg_clif per §1.2 removes this. fs/locator.rs (114-line crate-search) still needs semos-std fs + path, but tractable once libloading is gone. See deep-dive §2.1. |
| rustc_codegen_cranelift | ~50 | ARCHITECTURAL | fs:10, process:9, env:13 | Mostly build_system/ (offline build orchestrator) + driver/aot.rs that spawns `ld`. We DON'T use cg_clif's bundled driver — we use the already-vendored `cranelift-codegen` library directly from semos-cc. The `compiler/rustc_codegen_cranelift/src/` tree only gets used for the codegen integration glue, which is a few hundred LOC. Most of this crate's std surface drops with its driver. |
| rustc_codegen_ssa | ~28 | ARCHITECTURAL | back/link*:25, io:30, fs:13, process:7 | Linker driver lives here (`back/link.rs`, `back/linker.rs`, `back/command.rs`). Spawns `ld`/`lld` via `std::process::Command`. §1 doesn't drop linking, so we need to either (a) carve out link-driving to a separate semos-rustc-linker shim, or (b) emit `.o`/`.elf` directly and skip external linking. (b) matches what semos-cc does today and is preferable. See §2.2. |
| rustc_driver_impl | ~10 | ARCHITECTURAL | process:5, fs:1, env:10, thread:4 | Has signal_handler.rs (SIGABRT/SIGSEGV catch for ICE reports — irrelevant for SemOS), and spawns child compilers via Command for `--print sysroot` etc. Mostly tractable: gate signal_handler off, replace child-compiler spawn with in-process call. See §2.3. |
| rustc_interface | ~5 | ARCHITECTURAL | thread:8, sync:8, env:5, process:1 | Builds the rayon thread pool here (`util.rs`). With §1.4 (drop rayon), this drops to MECHANICAL. Keep auditing. |
| rustc_incremental | ~3 | ARCHITECTURAL | fs:5, io:24, path:5, sync:1 | The entire crate exists to persist query-cache to disk with seek-write semantics. §1.3 (drop incremental) cfg's this whole crate out. |
| rustc_session | ~12 | ARCHITECTURAL | path:23, fs:5, env:9, process:1, sync:8 | Owns sysroot/search-path resolution, target-spec loading, options parsing. `filesearch.rs` calls `std::os::unix/windows` for canonical paths. Per §1.6 (single target = x86_64-unknown-none), we hardcode sysroot + skip OS-specific canonicalization. Without that, this is ARCHITECTURAL. With §1.6, drops to NEEDS-SHIM. |
| rustc_data_structures | ~13 | ARCHITECTURAL | sync:60+, thread:2, fs:6, path:5 | The crate that holds the rayon abstractions (`sync/parallel.rs`, `sync/worker_local.rs`, `marker.rs` 20× `std::sync`, 9× `unsafe impl Send/Sync`). The flock/ subtree wants OS-specific file locks. memmap.rs wraps memmap2 → mmap. Per §1.4 we write the rayon shim HERE. Then sync.rs/sync/lock.rs/sync/freeze.rs map to `semos_std::sync`. flock.rs gets stubbed as a no-op (we don't have concurrent compiler invocations). memmap2 → vec-buffered fallback. See deep-dive §2.4. |
| rustc_proc_macro | ~5 | ARCHITECTURAL | (not audited per §1.5) | Per §1.5 we drop proc-macros initially. Crate gets cfg-disabled. |
| rustc_query_system | ~7 | ARCHITECTURAL | sync:8, io:1 | The query engine. Heavy `Arc<Mutex<...>>` and `parking_lot::RwLock`. Single-threaded mode is supposed to be supported via a Mode flag, but it's not first-class. Likely needs ~50 LOC of patches once rustc_data_structures' rayon shim is in. See §2.5. |
| rustc_query_impl | ~3 | NEEDS-SHIM | sync:2 | Generated query dispatch. Once rustc_query_system is no_std, this follows. |
| rustc_errors | ~12 | NEEDS-SHIM | io:24, path:13, sync:6, thread:2 | Diagnostic emission. emitter.rs writes to stderr (`io::Write`). json.rs serializes diagnostics. markdown/term.rs has terminfo bits. All map cleanly to semos_std::io. The translation.rs uses `std::sync::OnceLock` (×4) — reuse our local OnceLock shim from cranelift-codegen, or add to semos-std. |
| rustc_error_messages | ~3 | NEEDS-SHIM | path:30, fluent:- | Fluent-format diagnostic messages. The path:30 is mostly path-passing through diagnostic_impls trait impls. Semos-std::path covers it. |
| rustc_log | ~1 | NEEDS-SHIM | io:1, env:2 | Wraps `tracing-subscriber` for the rustc tracing log. tracing-subscriber pulls a lot of std — but is gated by a `--Z tracing` flag and stub-able. |
| rustc_fs_util | ~0.3 | NEEDS-SHIM | fs:1, path:3, os:2, time:1 | Tiny crate of fs helpers (`try_canonicalize` etc.). 8 unsafe impl. Reimplement against semos_std::fs / semos_std::path. |
| rustc_codegen_ssa.middle | ~28 | NEEDS-SHIM | (see ARCHITECTURAL) | Once linking is removed (per §2.2), what's left of ssa is back/{archive,metadata,write,lto} (still io/fs heavy but tractable) plus the MIR→IR plumbing (mir/, base.rs, common.rs — mostly MECHANICAL). |
| rustc_borrowck | ~25 | NEEDS-SHIM | io:10, path:1, sync:8 | Borrow check has graphviz/dump-mir paths that use io::Write to files. With incremental dropped (§1.3) most fs falls away. dump_mir.rs/region_infer/graphviz.rs use io::Write to stderr — fine. The polonius/legacy/facts.rs uses std::io for fact dumps — stub out. Otherwise mostly MIR-shape transforms. |
| rustc_mir_dataflow | ~10 | NEEDS-SHIM | io:5, sync:1, path:1 | framework/graphviz.rs (cfg dump) is the only meaningful io use. Otherwise MIR transforms. |
| rustc_middle | ~54 | NEEDS-SHIM | sync:6, io:9, path:11, os:3, collections:8 | The big one. ~54k LOC. Most std use is `std::collections::{HashMap,HashSet}` → `hashbrown::*` (MECHANICAL). The sync uses are `Arc<Mutex<...>>` in the tcx — semos_std::sync::{Arc, Mutex} match. The few path/io are diagnostic-emission helpers; trivial sub. on_disk_cache.rs (§1.3 drop incremental). The 30+ unsafe impl Send/Sync are for `Lift`/`Steal` newtypes — they pass through to alloc::sync::Arc. The size makes this multi-session work but the surgery is shallow. |
| rustc_resolve | ~25 | NEEDS-SHIM | path:1, sync:3, fs:0 | Mostly name resolution logic; very little std-surface. Macro path resolution touches std::path lightly. Easy. |
| rustc_hir | ~12 | MECHANICAL | path:3 | hir.rs / def.rs use std::path for diagnostic strings. Lightweight. |
| rustc_hir_analysis | ~30 | MECHANICAL | collections:1, sync:0 | Type checking. Almost no std surface. Easy. |
| rustc_hir_typeck | ~25 | MECHANICAL | collections:3, path:1 | Typeck. Similar to hir_analysis. |
| rustc_trait_selection | ~28 | MECHANICAL | path:6, io:0, collections:3 | Trait solving. Light std use. |
| rustc_const_eval | ~25 | MECHANICAL | sync:2, io:1, path:1 | MIRI-lite interpreter. Light std use, mostly cmp/fmt/mem. |
| rustc_mir_transform | ~30 | MECHANICAL | sync:1, fs:1, io:3 | MIR opt passes. dump_mir touches fs; gate or redirect to in-memory. |
| rustc_mir_build | ~18 | MECHANICAL | sync:3, io:2 | MIR construction from THIR. Easy. |
| rustc_monomorphize | ~5 | MECHANICAL | fs:2, io:2, path:1 | partitioning.rs writes a `.partitioning.txt` dump file — gate or in-memory. |
| rustc_passes | ~12 | MECHANICAL | fs:1, io:1, path:1 | Lint/HIR passes. Light std use. |
| rustc_lint | ~25 | MECHANICAL | collections:3, sync:2 | Lint engine. |
| rustc_lint_defs | ~4 | MECHANICAL | path:1, env:1, sync:3, thread:1 | Builtin lint definitions. Mostly data. The thread:1 is a docstring example. |
| rustc_infer | ~20 | MECHANICAL | collections:2, sync:1 | Type inference. |
| rustc_next_trait_solver | ~10 | MECHANICAL | (nightly feature) | Has feature gate isolating rustc-side deps. Mostly trait math, light std. |
| rustc_type_ir | ~15 | MECHANICAL | path:6, sync:1, fold:- | Has feature gate (`nightly`). Already designed for some isolation — rustc-analyzer reuses it. Easy port. |
| rustc_type_ir_macros | ~0.3 | MECHANICAL | - | Proc-macro crate; runs on host, stays std. |
| rustc_ast | ~25 | MECHANICAL | sync:5, version:- | AST nodes. Light std use. tokenstream.rs uses `Arc` heavily — semos_std::sync::Arc covers. |
| rustc_ast_pretty | ~5 | MECHANICAL | collections:2 | AST printing. |
| rustc_ast_lowering | ~15 | MECHANICAL | sync:5 | AST → HIR lowering. |
| rustc_ast_passes | ~6 | MECHANICAL | sync:- | AST validation. |
| rustc_ast_ir | ~0.5 | MECHANICAL | (nightly feature) | Shared with rustc-analyzer. Easy. |
| rustc_parse | ~25 | MECHANICAL | sync:3, fs:1, path:1, io:- | The parser. Mostly self-contained. parser/diagnostics.rs uses Command for "did you mean rustup" hint — stub out. |
| rustc_parse_format | ~3 | MECHANICAL | - | Format-string parser. |
| rustc_lexer | ~3 | MECHANICAL | use_std:1 | One use: `use std::str::Chars` in cursor.rs → `core::str::Chars`. Trivial. |
| rustc_expand | ~15 | MECHANICAL | sync:8, env:1, path:1 | Macro expansion. proc_macro.rs uses sync::Arc for dynamic libs — drops when §1.5 is applied. |
| rustc_builtin_macros | ~15 | MECHANICAL | sync:3, env:5, path:3 | Built-in macros (println!, env!, vec! etc.). env.rs reads compile-time env vars — replace with semos_std::env (or a small lookup table). |
| rustc_attr_parsing | ~6 | MECHANICAL | sync:3 | Attribute parsing. |
| rustc_attr_data_structures | (folded into rustc_hir per current tree) | - | - | Crate doesn't exist as standalone in this snapshot. |
| rustc_pattern_analysis | ~7 | MECHANICAL | sync:3, collections:1 | Exhaustiveness checker. Has feature gate (`rustc`). |
| rustc_transmute | ~3 | MECHANICAL | sync:1, collections:1 | Transmutability solver. Has feature gate. |
| rustc_target | ~15 | MECHANICAL | path:5, fs:1, collections:5 | Target specs. Per §1.6 we hardcode to one target and the path/fs uses (target-spec-from-JSON) drop. |
| rustc_macros | ~5 | MECHANICAL | sync:2, env:2 | Proc-macro impls used at rustc build time. Runs on the host, stays std. No port needed. |
| rustc_index | ~2 | MECHANICAL | std:37 | Newtyped indexed containers. Has nightly feature gate. std uses are mostly `std::ops::{Index,IndexMut}`, `std::slice`, `std::iter` — all → `core::*`. |
| rustc_index_macros | ~0.5 | MECHANICAL | - | Proc-macro crate; host-only. |
| rustc_serialize | ~3 | MECHANICAL | sync:1, path:1, io:1 | rustc's serde-equivalent. Uses io::Read/Write — semos_std::io maps. |
| rustc_hashes | ~0.2 | MECHANICAL | - | FxHash newtypes. Easy. |
| rustc_arena | ~1 | MECHANICAL | unsafe:4 | Bump allocators. Doesn't really need std. Easy port. |
| rustc_span | ~15 | MECHANICAL | sync:8, env:1, fs:4, path:0 | Source-location tracking. source_map.rs uses fs (file read) — sub to semos_std::fs. Uses scoped-tls (host thread-local) — semos_std::thread::LocalKey covers. |
| rustc_feature | ~5 | MECHANICAL | env:4, time:1, sync:1, fs:1 | Feature gate definitions. unstable.rs has a "feature acceptance date" using std::time — substitute. |
| rustc_hir_id | (tiny shim) | MECHANICAL | - | HIR id newtypes. |
| rustc_hir_pretty | ~3 | MECHANICAL | - | HIR pretty-printing. |
| rustc_baked_icu_data | (data only) | MECHANICAL | collections:1 | ICU data bake. Generated; the std use is a `BTreeMap` in the generated file. |
| rustc_graphviz | ~2 | MECHANICAL | io:11, path:3 | Graphviz emitter. io::Write everywhere → semos_std::io::Write maps. |
| rustc_borrowck.diag | (folded) | NEEDS-SHIM | path:5, io:3 | (already covered in rustc_borrowck) |
| rustc_traits | ~2 | MECHANICAL | - | Bridge layer to chalk-style query traits. |
| rustc_ty_utils | ~10 | MECHANICAL | - | tcx-attached utility queries. |
| rustc_symbol_mangling | ~5 | MECHANICAL | - | Itanium mangling. |
| rustc_privacy | ~3 | MECHANICAL | - | Visibility checker. |
| rustc_sanitizers | ~2 | MECHANICAL | - | Sanitizer build flags. Likely cfg-out for SemOS. |
| rustc_fluent_macro | ~2 | MECHANICAL | path:1, env:1, fs:1 | Host-only proc-macro that reads .ftl files. Stays std. |
| rustc_log | (above) | NEEDS-SHIM | - | already classified |
| rustc_public | ~5 | MECHANICAL | io:5 | The stable rustc API (formerly rustc_smir). Light std. |
| rustc_public_bridge | ~2 | MECHANICAL | - | Bridge to rustc_public. |
| rustc_abi | ~5 | MECHANICAL | (nightly+randomize features) | ABI/layout types. Has feature gates. |
| rustc_error_codes | (md only) | MECHANICAL | - | Error code text. No code. |
| rustc_driver | ~0.2 | MECHANICAL | - | Re-export shim of rustc_driver_impl. |
| rustc_windows_rc | (host build helper) | MECHANICAL | - | Windows resource compiler driver. Host-only. |
| rustc | ~0.1 | MECHANICAL | - | Top-level binary crate. Skinny. |

Bucket counts:
- **ARCHITECTURAL: 13** (rustc_thread_pool, rustc_codegen_llvm, rustc_codegen_gcc, rustc_llvm, rustc_metadata, rustc_codegen_cranelift, rustc_codegen_ssa, rustc_driver_impl, rustc_interface, rustc_incremental, rustc_session, rustc_data_structures, rustc_proc_macro). After applying §1 drop decisions (LLVM/gcc/llvm/proc-macro/incremental gone, rayon→shim, metadata-plugin-load gone), the residual ARCHITECTURAL set is **5**: rustc_data_structures, rustc_session, rustc_codegen_ssa (slimmed), rustc_driver_impl (slimmed), rustc_query_system.
- **NEEDS-SHIM: 8** (rustc_errors, rustc_error_messages, rustc_log, rustc_fs_util, rustc_borrowck, rustc_mir_dataflow, rustc_middle, rustc_resolve, rustc_query_impl).
- **MECHANICAL: 55** — the vast majority. Bulk-sed jobs.
- **TRIVIAL: 0** — no crate is the Cranelift "default = [\"std\"]" clean-flip pattern.

Top 5 hardest (post-§1-drops): rustc_data_structures (rayon shim
authoring), rustc_middle (sheer 54k-LOC volume + many small std touches),
rustc_codegen_ssa (must carve out the linker driver), rustc_session
(sysroot/search-path canonicalization needs targeted rewrites),
rustc_query_system (Mutex/RwLock single-threaded mode needs validation).

## 2. ARCHITECTURAL deep-dives (residual set after §1 drops)

### 2.1 rustc_metadata (post-libloading-removal)

**What it uses std for.** Two things: (a) the codegen-backend plugin
loader in `creader.rs` and around it, which calls
`libloading::Library::new` and resolves `__rustc_codegen_backend` —
this is the dynamic codegen mechanism. (b) crate-locator file
search in `locator.rs` — walks search paths, reads `.rmeta` headers,
opens `.rlib` files. Cites: `compiler/rustc_metadata/src/fs.rs:1-5`
(5× `std::fs` uses) and `compiler/rustc_metadata/src/locator.rs:1-2`
(`std::process` + 2× `std::fs` + 2× `std::path`).

**Whether §1 eliminates it.** Decision §1.2 (statically link cg_clif)
removes (a) — we delete the `libloading` dep, hard-code the codegen
backend constructor call, and rip the plugin-load fall-back. That's
~200 LOC of patches inside `creader.rs` and a Cargo.toml edit. (b)
stays — we still need to find dependency .rmeta/.rlib files. That's
exactly the semos_std::fs + semos_std::path surface.

**What else has to change.** `fs.rs` (5× fs ops) is straightforward.
`tempfile` dep needs replacement — we have no real tempdir, so write
to `/tmp/<random>` and `unlink` on drop with a tiny shim. Crate moves
from ARCHITECTURAL to NEEDS-SHIM after this surgery.

### 2.2 rustc_codegen_ssa (linker carve-out)

**What it uses std for.** The `back/` subtree is a complete linker
driver: `link.rs` (1500 LOC), `linker.rs` (gcc/msvc/wasm linker
flag generation), `command.rs` (the `Command` wrapper), `archive.rs`
(`ar` invocation), `apple.rs` (codesign), `rpath.rs`. All of these
shell out to external tools via `std::process::Command`. Cites:
`compiler/rustc_codegen_ssa/src/back/link.rs:1-8` (8× `use std::`),
`compiler/rustc_codegen_ssa/src/back/linker.rs:1-5`,
`compiler/rustc_codegen_ssa/src/back/command.rs:1-3` (3 `std::process`).

**Whether §1 eliminates it.** None of the six §1 decisions explicitly
drops linking. But the natural answer is: SemOS programs are ELFs we
build *in-process*. cg_clif emits ELF object bytes already; semos-cc
proves we can splice and emit a runnable ELF without invoking `ld`.

**What has to change.** Add a §1.7 decision: emit a fully-linked ELF
in-process and skip the external linker. Concretely:
1. Delete or stub `back/command.rs`, `back/linker.rs`,
   `back/archive.rs`, `back/apple.rs`, `back/rpath.rs`, most of
   `back/link.rs`.
2. Replace the linker invocation path in the codegen orchestrator
   with a direct call to a `semos_link::write_elf(modules, output)`
   that we own.
3. Keep `back/metadata.rs` and `back/write.rs` — those write
   per-module `.o` bytes which we *do* still need internally to
   stitch.

This is one of the big architectural surgeries. Probably 1-2 sessions.
After it, ssa drops to NEEDS-SHIM.

### 2.3 rustc_driver_impl

**What it uses std for.** Three subsystems: (a) `signal_handler.rs`
(SIGABRT/SIGSEGV → ICE report) — uses `std::os::unix::process` + raw
libc signals. (b) `lib.rs` spawns a child rustc for `--print sysroot`
and `--print target-spec`. (c) `args.rs` reads `RUSTC_BOOTSTRAP` and
similar env vars. (d) `highlighter.rs` reads stdin/stderr for the
pretty-printer fallback. Cites: `compiler/rustc_driver_impl/src/lib.rs:1-13`
(13× use std), `compiler/rustc_driver_impl/src/signal_handler.rs:1-2`.

**Whether §1 eliminates it.** Per §1.6 (single target), child-compiler
spawn for `--print target-spec` becomes a one-line constant return.
Signal handlers don't apply on SemOS (no SIGSEGV; we get
double-faults handled by the kernel). args.rs survives but trims
hugely.

**What has to change.** Cfg-gate `signal_handler.rs` to `#[cfg(unix)]`
(it's already partly gated). Replace Command-spawn for sub-prints with
direct function calls (we already have target spec in-tree). After
those edits, ~80% of the std surface evaporates and the crate becomes
NEEDS-SHIM.

### 2.4 rustc_data_structures (the rayon shim home)

**What it uses std for.** Three subsystems entangled together:
(a) `sync/parallel.rs` (223 LOC) — the rayon-or-sequential dispatch.
(b) `sync/worker_local.rs` (128 LOC) — per-thread storage for the
query system. (c) `sync/lock.rs` (187 LOC) — Mutex/RwLock newtype
that's either `parking_lot` or a single-threaded `RefCell` depending
on a feature. (d) `marker.rs` (222 LOC, 20× `std::sync`, 9×
`unsafe impl`) — `Send`/`Sync` adapter newtypes for cross-thread
tcx access. (e) `flock/{linux,unix,windows,unsupported}.rs` — file
locks for the incremental cache. (f) `memmap.rs` — wraps memmap2.
Cites: `compiler/rustc_data_structures/src/marker.rs:1` (20× use std),
`compiler/rustc_data_structures/src/sync/parallel.rs:1-2`.

**Whether §1 eliminates it.** Partly. §1.3 (drop incremental) removes
the *need* for flock and most of memmap (no on-disk cache to lock).
§1.4 (drop rayon) means parallel.rs becomes "always sequential mode"
and worker_local.rs becomes "always thread 0".

**What has to change.**
1. `sync/parallel.rs`: delete the rayon-using branch entirely. The
   sequential branch already exists (it's the test path) and just
   needs to be unconditional.
2. `sync/worker_local.rs`: replace with a `Cell<T>` (single-threaded).
3. `sync/lock.rs`: keep the `RefCell`-based fallback (single-threaded
   Mutex), drop the `parking_lot` branch.
4. `marker.rs`: the `unsafe impl Send for FromDyn<T>` etc. stay (they
   still compile under no_std).
5. `flock/*.rs`: replace all four with a no-op stub
   `Lock::new(_path) -> Ok(Self)`.
6. `memmap.rs`: replace memmap2 with a `Vec<u8>` read of the whole
   file (no incremental cache means files are small).
7. `jobserver.rs`: stub. We have no make jobserver.

Workload estimate: 1-2 sessions of focused work, mostly mechanical
substitution + the rayon-shim authoring. Crate moves to NEEDS-SHIM
afterward.

### 2.5 rustc_query_system / rustc_session post-§1

**rustc_query_system.** Today its sync uses Arc<Mutex<...>> heavily
in `dep_graph/graph.rs` (2× std::sync) and `query/job.rs` (1×).
Single-threaded mode: replace `Mutex<T>` with `RefCell<T>` (or keep
`semos_std::sync::Mutex` whose lock is uncontended in single-task
contexts — it works). The `parking_lot` dep gets cut. `caches.rs`
sync use is `OnceLock` → use the same shim we built for cranelift.

**rustc_session.** `path:23, env:9, sync:8`. The dominant use is
`PathBuf` for sysroot and search paths — semos_std::path handles
this. `filesearch.rs` has `std::os::unix::ffi::OsStrExt` (canonicalize
on the host) — replace with a `core::str::Chars`-based normalize
since we don't have symbolic links or case-folding on SemOS today.
`config.rs` parsing of `-C` flags is pure str manipulation. Per
§1.6 (single target), `target.rs` collapses to a constant. Crate
moves to NEEDS-SHIM after these edits.

## 3. Shared shim opportunities

Patterns that recur in 5+ crates and would benefit from one shared
addition to semos-std rather than per-crate hacks. In priority order:

1. **`OnceLock<T>` / `Once`** — used in rustc_errors::translation,
   rustc_session::session, rustc_codegen_cranelift, rustc_data_structures,
   rustc_middle, and at least 6 other crates (we wrote a local shim
   for cranelift-codegen; the cranelift port log calls it out
   explicitly). Adding `semos_std::sync::OnceLock<T>` (single-threaded
   AtomicBool+UnsafeCell, same as our cranelift shim) eliminates
   per-crate substitutions.

2. **`std::thread::LocalKey<T>` (thread-local storage)** — used by
   `rustc_span` (scoped-tls dep), `rustc_middle::ty::context::tls`,
   `rustc_errors`, and elsewhere. Single-task SemOS makes this a
   `Cell<T>`/`RefCell<T>` wrapper. Worth adding
   `semos_std::thread::local_key!` macro that defines a global.

3. **`std::env::var` / `var_os`** — used by rustc_session, rustc_driver_impl,
   rustc_builtin_macros (env!), rustc_log, rustc_codegen_cranelift, ~15
   crates total. semos_std::env::var should be exhaustive enough that
   we can have a const-string-table fallback when SemOS has no real env.

4. **`std::path::PathBuf::canonicalize`** — used by rustc_session,
   rustc_fs_util, rustc_metadata. Returns absolute canonical path.
   On SemOS without symbolic links, a 30-LOC implementation that
   walks `.` / `..` is enough.

5. **`std::process::abort_with_code(code: i32) -> !`** — internal name
   not real std API, but rustc has many `process::exit(101)` calls
   in ICE paths. semos_std::process::exit exists; just verify it
   takes i32 and document the rustc bug-code convention.

6. **`std::io::stderr().write_all`** — rustc_errors, rustc_graphviz,
   rustc_driver_impl. semos_std::io probably already supports stderr
   (verify); if not, ~20 LOC.

7. **`tempfile::tempdir()` / `NamedTempFile`** — rustc_metadata,
   rustc_data_structures, rustc_codegen_ssa. Tempfile crate isn't
   semos-std but we could add `semos_std::fs::tempdir()` that returns
   a `Drop`-on-rm RAII handle.

## 4. semos-std surface gaps

Concrete additions ranked by # of dependent crates touched by porting.

| Priority | semos-std API | rustc crates that need it |
|---|---|---|
| P0 | `semos_std::sync::OnceLock<T>` | 8+ crates (rustc_errors, rustc_session, rustc_middle, rustc_data_structures, rustc_codegen_ssa, ...) |
| P0 | `semos_std::thread::LocalKey<T>` + `thread_local!` macro | 5+ crates (rustc_span, rustc_middle::ty::context::tls, rustc_errors, rustc_data_structures, scoped-tls users) |
| P0 | rayon-shim (we author it inside rustc_data_structures, not semos-std, per §2.4) | rustc_data_structures, rustc_query_system, rustc_query_impl, rustc_interface |
| P1 | `semos_std::env::var{,_os}` reading from a const table | rustc_session, rustc_driver_impl, rustc_builtin_macros, rustc_log, rustc_codegen_cranelift, ~10 total |
| P1 | `semos_std::path::PathBuf::canonicalize` (no-symlink simplification) | rustc_session, rustc_fs_util, rustc_metadata |
| P1 | `semos_std::process::abort` / `exit(i32)` confirmed return type | rustc_driver_impl, rustc_session, anywhere bug! macros land |
| P2 | `semos_std::fs::canonicalize` and `semos_std::fs::metadata` shape-compatible with std::fs::Metadata | rustc_metadata, rustc_session, rustc_fs_util |
| P2 | `semos_std::io::stderr()` returning a writeable handle | rustc_errors, rustc_graphviz, rustc_driver_impl |
| P2 | `semos_std::time::SystemTime::now()` (returns a Duration-since-epoch) | rustc_feature, rustc_codegen_ssa::base (compile-time-stamp uses), rustc_data_structures::profiling |
| P3 | `semos_std::fs::File::set_len`, `seek` (for in-place rewrite) | rustc_metadata::fs (encoder rewrites), rustc_data_structures::memmap |
| P3 | tempfile equivalent: `semos_std::fs::TempDir`/`TempFile` RAII | rustc_metadata, rustc_codegen_ssa::archive |
| P3 | `semos_std::os::raw` (c_int, c_char) — for libc-via-FFI patterns | rustc_codegen_ssa::back::apple, rustc_codegen_llvm (drop), rustc_codegen_gcc (drop) |

A `semos_std::sync::OnceLock` and a `semos_std::thread::local_key!` macro
land basically everywhere; doing those FIRST (Phase 2, before Phase 3
parallel work starts) is the highest-leverage prep.

## Summary

**Bucket counts.** ARCHITECTURAL 13 → 5 after applying §1 drops (rayon,
LLVM, gcc, llvm, proc-macros, incremental, libloading-plugin-load).
NEEDS-SHIM 8. MECHANICAL 55. TRIVIAL 0 — no crate is a clean-flag flip
like the Cranelift recipe; every crate needs at least a `#![no_std]` +
prelude injection.

**Top 5 hardest (after §1).** (1) rustc_data_structures — write the
rayon shim, kill flock/memmap/jobserver, ~1-2 sessions; (2) rustc_middle
— sheer 54k-LOC volume, surgery is shallow but pervasive; (3)
rustc_codegen_ssa — needs the in-process-link carve-out (call it
decision §1.7); (4) rustc_session — sysroot/path canonicalization
rewrite; (5) rustc_query_system — single-threaded Mutex validation.

**Top 5 semos-std gaps** (priority order). (1) `sync::OnceLock<T>`
(8+ dependent crates); (2) `thread::LocalKey<T>` + `thread_local!`
macro (5+ crates including rustc_middle::tcx tls); (3) `env::var{,_os}`
reading from a const table (10+ crates); (4) `path::PathBuf::canonicalize`
(no-symlink simplification, 3 crates); (5) confirmed
`process::exit(i32)` return shape + an `abort()` ICE entry.

**Phase 1 stop condition.** R2 finds zero new unmitigated blockers
beyond what §1 already addresses (LLVM, libloading, rayon, incremental,
proc-macros, multi-target). The single new architectural surgery
discovered is in-process linking (§2.2), which is a natural
consequence of how cg_clif emits ELFs. Recommend formalizing it as
decision §1.7 before Phase 2 starts. Otherwise: proceed.
