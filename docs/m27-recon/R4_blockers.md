# M27 Phase 1 — R4: rustc fundamental-block audit

Drafted 2026-05-30. Agent R4 of the four Phase 1 recon runs (see
`docs/M27_RUSTC_PORT_PLAN.md` §0, §1, §4). Companion to R1 (dep graph),
R2 (std-surface), R3 (externals).

**Scope.** Anything in the rustc tree at
`user-programs/semos-rustc/vendor-rustc-src/compiler/` that rises above
"this needs a patch" to "this is an architectural assumption the port
strategy doesn't yet address." The six decisions in plan §1 already
cover LLVM removal (1.1), dlopen plugin model for codegen backends
(1.2), incremental compilation (1.3), rayon parallel queries (1.4),
proc-macros (1.5), and single target (1.6). This audit looks for the
**fourth** wall — anything else of equivalent magnitude.

**Method.** Read-only. Tracked through the query system, arena
allocators, type-checking infrastructure, metadata format, diagnostics,
sysroot search, and the linker invocation path. Cited at least one
file:line per finding. Compared the rustc code against what the SemOS
std-shim at `user-programs/std-shim/src/` actually exposes — that
delta is the real porting cost.

The stop-condition test (plan §2): if there are more than **3
unaddressed blockers**, Phase 2 doesn't start; we re-strategize. The
final count + verdict is in §5.

---

## 1. Confirmed blockers

These are the architectural patterns that the six §1 decisions do
**not** address and that need an additional plan-level call before
Phase 2 starts.

### B1. Panic-as-control-flow (FatalError + catch_unwind)

**Description.** rustc terminates the current compilation on a fatal
diagnostic by panicking with a sentinel value (`FatalErrorMarker`) and
catching that panic at the top of the compilation pass. Every
`emit_fatal()`, every parse error that can't be recovered from, every
`abort_if_errors()` call ultimately routes through this. It is **not**
just for ICEs — it is the primary error-propagation path inside
rustc's query engine and diagnostic emission. The rustc codebase
treats `catch_fatal_errors(|| ...)` exactly the way most Rust code
treats `?` on a `Result`.

**File:line.**

- `compiler/rustc_span/src/fatal_error.rs:16` —
  `std::panic::resume_unwind(Box::new(FatalErrorMarker))`.
- `compiler/rustc_span/src/fatal_error.rs:34` — `catch_fatal_errors`
  wraps `panic::catch_unwind(panic::AssertUnwindSafe(f))`.
- `compiler/rustc_query_system/src/dep_graph/mod.rs:70` — query forcing
  uses `panic::catch_unwind` to scope a panic; on `Err(value)` it
  checks `value.is::<rustc_errors::FatalErrorMarker>()` and re-raises.
- `compiler/rustc_data_structures/src/sync/parallel.rs:23` — the
  `ParallelGuard::run` core primitive is built on `catch_unwind` +
  `resume_unwind`, used even in serial mode (see `serial_join` at
  line 50).
- 68 occurrences of `FatalError` / `catch_fatal_errors` / `.raise()`
  across 20 files in `compiler/`.

**Why §1 decisions don't address it.** §1.4 drops rayon but the
`ParallelGuard` primitive still runs in single-threaded mode and still
calls `catch_unwind` on every join (`compiler/rustc_data_structures/
src/sync/parallel.rs:51-57`). The query system's `dep_graph` uses
`catch_unwind` independently of any threading decision (`compiler/
rustc_query_system/src/dep_graph/mod.rs:70`). And nothing in §1 says
anything about FatalError itself.

The blocker is that **semos-std panics abort**. `user-programs/
std-shim/src/rt.rs:28-39` is the panic handler — it prints a message
and calls `process::exit(101)`. There is no
`std::panic::catch_unwind` because there is no unwinding machinery:
no `eh_personality` lang item, no `_Unwind_Resume`, no .eh_frame
processing in the kernel or in semos-std. The x86_64-unknown-none
target spec at `compiler/rustc_target/src/spec/targets/
x86_64_unknown_none.rs:27` sets `panic_strategy: PanicStrategy::Abort`
which means the rustc binary compiled for SemOS will have
`catch_unwind` lowered to "just run the closure, no catch" and any
`FatalError::raise` will *terminate the rustc process* instead of
unwinding to the catch site.

That means user-program compilation errors → semos-rustc exits. Not
"emits a diagnostic and returns nonzero" — actually crashes mid-pass.
Recoverable parsing failures, type errors, all of it.

**Mitigation options.**

- **A. Replace FatalError with Result threading** (rewrite). Convert
  every `emit_fatal()` / `FatalError::raise()` to a `Result` return
  and rewrite the dozens of catch sites. This is a fork of huge depth
  — it touches diagnostics, the query engine, MIR build, codegen
  orchestration. Estimate: **multiple sessions per crate × ~15
  crates** = a sustained subproject in its own right. "fork rustc and
  rip out."
- **B. Implement libunwind / DWARF .eh_frame walking on SemOS**
  (kernel feature). Make `panic = "unwind"` actually work for SemOS
  ELF binaries. This means: (a) cg_clif must emit .eh_frame (it
  already can — see `compiler/rustc_codegen_cranelift/src/debuginfo/
  unwind.rs`), (b) semos-std needs an `eh_personality` lang item +
  `_Unwind_Resume` implementation backed by a small DWARF unwinder
  (gimli/libunwind-rs port), (c) the SemOS panic-handler model must
  invoke that unwinder instead of `process::exit(101)`. Estimate:
  **3-5 sessions** for a minimal unwinder, but high uncertainty —
  Rust personality routines are not simple, and SEH/itanium-eh
  semantics with Rust's panic-payload Box have subtle interactions
  with the allocator. The plan §1 didn't mention this and the §6
  ("what to do today") doesn't budget for it.
- **C. Single-shot mode: any FatalError = process exit**. Accept that
  the user-program compile fails the entire semos-rustc invocation,
  not just one error → diagnostic. Each compile is one-shot anyway,
  so the diagnostic gets emitted before the panic and the user sees
  an error. Estimate: **0 sessions** if we keep panic=abort. **But**
  this only works if the panic happens after the diagnostic prints —
  the diagnostic flush has to be guaranteed before abort. Need to
  audit every `emit_fatal()` site to verify (the diagnostic context
  may buffer). Probably feasible but with surprises.

**Stop-condition flag.** Option C is acceptable for v1 if we accept
"compilation error = full restart"; Options A and B are both serious
work. Recommend pre-committing to **C** for hello-world DEMO 80 and
deferring A/B to a later milestone. Document the limitation in
ROADMAP. **Not a kill condition** for M27 — but is a fourth decision
point that needs to be made before Phase 2.

### B2. Thread-local storage for SESSION_GLOBALS and TyCtxt (TLV)

**Description.** rustc threads two pieces of state through every
single pass implicitly: `SESSION_GLOBALS` (interner, source map,
hygiene context — held in `rustc_span`) and the current `TyCtxt`'s
`ImplicitCtxt` (held in `rustc_middle`). Both are stored in
thread-local storage and accessed via free functions like
`rustc_span::with_session_globals(|sg| ...)` and
`rustc_middle::ty::tls::with(|tcx| ...)`. They are accessed *constantly*
— span interning, symbol interning, type queries, every diagnostic.
Even in single-threaded mode they are accessed via TLS.

**File:line.**

- `compiler/rustc_span/src/lib.rs:185` —
  `scoped_tls::scoped_thread_local!(static SESSION_GLOBALS:
  SessionGlobals);`
- `compiler/rustc_span/src/lib.rs:170-175` — `with_session_globals`
  body: `SESSION_GLOBALS.with(f)`.
- `compiler/rustc_thread_pool/src/tlv.rs:7` — `thread_local!(pub
  static TLV: Cell<*const ()> = const { Cell::new(ptr::null()) });`
- `compiler/rustc_middle/src/ty/context/tls.rs:39` — `use
  rustc_thread_pool::tlv::TLV;` followed by `TLV.with(|tlv| ...)`
  patterns at lines 57, 71.
- `compiler/rustc_data_structures/src/sync/worker_local.rs:41-45`,
  52-59 — `thread_local!` for `REGISTRY` and `THREAD_DATA`.
- 17 files in `compiler/` use `thread_local!` macros directly, plus
  every consumer of `scoped_tls` (5 files).

**Why §1 decisions don't address it.** §1.4 (drop rayon) eliminates
the *parallel* query case, but the single-threaded mode still uses
TLS for SESSION_GLOBALS and the implicit `TyCtxt`. The plan's
"single-threaded shim" replaces rayon's work-stealing with sequential
execution, but doesn't address that scoped-tls itself is unavailable.

The blocker: **semos-std has no `thread_local!` macro and no
`scoped_tls` equivalent.** The shim has Mutex/RwLock/Condvar/Once
backed by SYS_FUTEX (`user-programs/std-shim/src/sync.rs`) but no
`#[thread_local]` lang-item support, and no userland glue for
`__tls_get_addr` or static-TLS template loading. The kernel doesn't
set up an FS/GS-based TLS area for processes. semos-std's `thread`
module spawns threads via SYS_THREAD_SPAWN with a raw entry function
+ a single `u64` arg — no per-thread storage is allocated.

For a *single-threaded* rustc, the workaround is straightforward:
replace `thread_local!` with `static MUT` / `static
once_cell::Lazy`. The TLS is artificial in single-threaded mode. But
this is a **mechanical rewrite of every TLS site** in 17 files —
nontrivial but tractable — *if* combined with §1.4's single-threaded
decision.

**Mitigation options.**

- **A. Single-threaded TLS shim** (multi-session, mechanical). Define
  a `semos_tls` crate that exposes `scoped_thread_local!` and
  `thread_local!` macros that lower to `static Cell<…>` (no actual
  per-thread storage; one thread). Wire `scoped-tls` and the `tlv.rs`
  shim through it. Estimate: **1-2 sessions** of the broader Phase 2
  port effort.
- **B. Real TLS in the kernel + libc-style support in semos-std**
  (multi-session, kernel work). Make SYS_THREAD_SPAWN allocate a TLS
  area, teach semos-std to use `#[thread_local]` properly via
  `FS_BASE` MSR. Required *only* if we want parallel rustc. Estimate:
  **3-4 kernel sessions plus libc-side glue**. **Out of scope** given
  §1.4.

**Stop-condition flag.** Option A is acceptable and fits the §1
decisions. **Not a kill condition** but is a foundation-phase work
item not currently called out in the plan's Phase 2 list. Add a line
to the Phase 2 spec.

### B3. Stacker / dynamic stack growth (`ensure_sufficient_stack`)

**Description.** rustc uses the `stacker` crate to grow the stack on
demand at recursive call sites. The recursion depths on real Rust
code (deeply nested expressions, trait obligations, monomorphization)
exceed any fixed user stack. The crate works by checking if there's
less than `RED_ZONE` (100 KiB) free on the current stack; if so, it
allocates a new stack with `mmap`, jumps to it via inline assembly,
runs the closure, then jumps back. Hot paths in the parser, AST
lowering, trait selection, MIR build call it on every recursive
descent.

**File:line.**

- `compiler/rustc_data_structures/src/stack.rs:21` —
  `stacker::maybe_grow(RED_ZONE, STACK_PER_RECURSION, f)`.
- `compiler/rustc_data_structures/src/stack.rs:4` — `RED_ZONE: usize
  = 100 * 1024` (100 KiB).
- `compiler/rustc_data_structures/src/stack.rs:9` —
  `STACK_PER_RECURSION: usize = 1024 * 1024` (1 MiB per growth).
- 54 occurrences of `ensure_sufficient_stack` across 20 files,
  including:
  - `compiler/rustc_trait_selection/src/traits/select/mod.rs:6` (6×
    in that file alone).
  - `compiler/rustc_trait_selection/src/solve/normalize.rs:5`.
  - `compiler/rustc_const_eval/src/const_eval/valtrees.rs:2`.
  - `compiler/rustc_parse/src/parser/expr.rs:3`.
  - `compiler/rustc_ast_lowering/src/expr.rs:2`, `pat.rs:2`.

**Why §1 decisions don't address it.** None of the decisions touch
stack management; the plan §4 risk register mentions stack sizes
but only at the kernel/process level (4 MiB max stack).

The blocker: stacker uses `mmap` + arch-specific assembly to switch
to a new stack mid-call. semos-std exposes `SYS_MMAP_ANON`, so the
allocation is possible, but the assembly bits depend on stacker
having a working `psm` (Portable Stack Manipulator) for SemOS's
target. `psm` ships hand-written assembly for x86_64-linux,
x86_64-windows, etc. It does have a generic-fiber fallback but the
fallback uses `getcontext`/`setcontext` which SemOS doesn't have, or
the no_std `psm::on_stack`/`psm::replace_stack` primitives which
**do** work standalone since they're pure inline assembly — but
they need to be enabled by feature flag in psm's Cargo.toml.

**Mitigation options.**

- **A. Vendor psm + stacker, port psm's x86_64 backend to SemOS**
  (single session). psm's x86_64 assembly is the same on Linux and
  SemOS — just save/restore rsp + rbp. The Cargo.toml gating may need
  an `unknown-none` arm. Then make stacker call into psm's on_stack
  primitive with a `SYS_MMAP_ANON`-allocated stack. Estimate:
  **1-2 sessions** during Phase 2.
- **B. Stub `ensure_sufficient_stack` to be a no-op + bump user stack
  to ~64 MiB**. Risk: real code overflows. But for hello-world this
  works. Estimate: **0 sessions code; budget kernel-side stack bump**.
- **C. Stub + rewrite the deep-recursion paths to be iterative**.
  Most calls are convenience — the parser, trait selection, MIR
  build all *could* be done iteratively. But that's an upstream
  rustc-quality change. "fork rustc and rip out." **Not viable.**

**Stop-condition flag.** Option A is doable but takes a session.
Option B is the v1 plan. **Not a kill condition.** Add to the Phase
2 spec: "if stack-overflow shows up in DEMO 80, fall through to
Option A."

### B4. The vendored rayon fork — `rustc_thread_pool` is heavier than expected

**Description.** §1.4 says "single-threaded rayon shim." That works
for `rustc_data_structures::sync::parallel` (the 50-line file at
`compiler/rustc_data_structures/src/sync/parallel.rs`), but
`rustc_thread_pool` is **not** just a wrapper crate — it is a full
**vendored fork** of the rayon-core crate, with its own work-stealing
scheduler, registry, sleep machinery, latches, scope/spawn
primitives, and thread-local TLV (see B2). It is referenced from
both `rustc_data_structures` (for `parallel::join`) **and** from
`rustc_middle` (for the deadlock handler in `compiler/rustc_interface/
src/util.rs:224-263` — the cycle-handler thread spawn).

The relevant point: making it "single-threaded" means **either**
porting the whole work-stealing scheduler to use `std::thread::spawn`
(which on SemOS is OK but the deadlock-handler thread spawn is
actually USED in the query system), **or** stubbing the entire crate
to a no-op shim where `join` becomes sequential `(a(), b())` and
`scope` becomes immediate execution.

**File:line.**

- `compiler/rustc_thread_pool/Cargo.toml:8` — `"Core APIs for Rayon
  - fork for rustc"`.
- `compiler/rustc_thread_pool/src/registry.rs` — work-stealing
  registry (whole file, ~hundreds of lines).
- `compiler/rustc_data_structures/src/sync/parallel.rs:97-113`
  — `rustc_thread_pool::spawn` and `rustc_thread_pool::scope`
  called from the not-yet-thread-aware sync module.
- `compiler/rustc_interface/src/util.rs:219-264` — the
  `ThreadPoolBuilder` + `deadlock_handler` setup. The deadlock
  handler **spawns a thread to break query cycles** —
  that is, the query system *can* deadlock from cycles and the
  handler thread is the recovery path.

**Why §1 decisions don't address it.** §1.4 just says "single-
threaded `rayon` shim that runs everything sequentially." It does
not say: who owns the shim? what does `scope` do? what does the
deadlock-handler thread do in single-threaded mode? In a truly
single-threaded compiler, **query cycles always deadlock fatally**
— there is no other thread to break them. We have to either:

- Detect and bail on query cycles synchronously (do not enter the
  cycle in the first place; emit a normal diagnostic).
- Accept that some test cases (cyclic trait bounds, recursive types
  with associated types) will hang.

**Mitigation options.**

- **A. Replace `rustc_thread_pool` with a 50-line stub crate** —
  `spawn` runs the closure inline; `join(a, b)` runs sequentially;
  `scope` becomes immediate; the `Registry`/`ThreadPool` types are
  zero-sized newtypes that hold no state. Patch the deadlock handler
  to be a no-op (cycle = synchronous bug! emission). Estimate:
  **1-2 sessions** including patching `rustc_interface/util.rs`'s
  scaffolding around the deadlock-handler thread spawn.
- **B. Carry the vendored fork as-is and patch it to no_std + alloc**
  (using `std::thread::spawn` from semos-std). Estimate: **multi-
  session port** of the fork. **Not worth it** for v1.

**Stop-condition flag.** Option A is the correct read of §1.4 but
the plan's Phase 2 list doesn't explicitly include it. Add to Phase
2 spec.

### B5. PathBuf / OsStr type-shape

**Description.** rustc uses `std::path::PathBuf` and `std::ffi::OsStr`
absolutely everywhere — argument parsing, sysroot search, file
encoder paths, crate-name resolution, target spec loading. These
types live in `std`, not `core`/`alloc`. semos-std's path module
(`user-programs/std-shim/src/path.rs`) is a **str-based** Path —
zero-cost wrapper over `&str`. There is **no OsString or OsStr** in
semos-std. Every `path: &Path` parameter / `PathBuf` field needs to
be rewritten.

**File:line.**

- `compiler/rustc_session/src/config.rs:1-50` — 9 occurrences of
  `OsString` / `OsStr` in the config struct.
- `compiler/rustc_session/src/filesearch.rs:1-6` — `PathBuf`, 6
  occurrences of OsString-shaped APIs.
- `compiler/rustc_metadata/src/locator.rs` — full file uses `PathBuf`.
- `compiler/rustc_codegen_ssa/src/back/link.rs:1576-1620` —
  `exec_linker` uses Path/Command APIs.

**Why §1 decisions don't address it.** The plan assumes the std
surface will be brought up incrementally during Phase 3 (R2's
remit), but R2's audit is per-crate-surface and PathBuf is so
pervasive that it crosses every crate boundary.

semos-std *could* re-export `core::ffi::OsStr` if such a thing
existed — it doesn't; `OsStr` is std-only because it must wrap
platform-specific encodings (UTF-8 on Unix, WTF-8 on Windows). On
SemOS the path encoding is just UTF-8 so a newtype around `str` is
fine.

**Mitigation options.**

- **A. semos-std grows an `OsString`/`OsStr` newtype** that's just
  a wrapper for `String`/`str` on SemOS. Add `path::PathBuf`/`Path`
  that wrap `OsString`/`OsStr` instead of `String`/`str`. Add
  `std::ffi::OsString`/`OsStr` re-exports. Estimate: **1 session**.
- **B. Patch every rustc use of `OsString`/`OsStr` to `String`/`str`**.
  Estimate: **multi-session mechanical refactor**.

**Stop-condition flag.** Option A is the right move. This is more
of a "this needs to happen before Phase 2 starts" than a true
blocker. Already implied by R2's audit. **Not a kill condition.**

---

## 2. False alarms

Patterns I investigated and decided are NOT structural blockers.

### F1. `bumpalo`-style arenas needing huge contiguous virtual space

The rustc arena (`rustc_arena/src/lib.rs:53-99`) grows in chunks
starting at 4 KiB and doubling up to a 2 MiB max. Each chunk is a
plain `Box::new_uninit_slice(capacity)` allocation
(line 57) — backed by the global allocator. **No `mmap` is used by
rustc_arena directly.** It works on top of semos-std's
SYS_HEAP_ALLOC-backed allocator with no changes. The 100s-of-MiB
comment in the source refers to *total* arena footprint across many
chunks, not a single contiguous block. False alarm — `rustc_arena` is
fine on no_std + alloc.

### F2. memmap2 for crate metadata loading

`compiler/rustc_data_structures/src/memmap.rs` is a thin wrapper
around `memmap2::Mmap` with an existing `cfg(any(miri, target_arch
= "wasm32"))` fallback (line 9, 32-42) that reads the file into a
`Vec<u8>` instead of mmap. Adding `cfg(any(miri, target_arch =
"wasm32", target_os = "semos"))` (or whatever cfg we pick) lets the
Vec fallback run on SemOS. Two-line patch. False alarm.

### F3. `rustc_metadata`'s libloading usage

The `libloading::Library` calls at `compiler/rustc_metadata/src/
creader.rs:1367-1430` are only used for **proc-macro DLL loading**
— see the surrounding `process_path_extern` for proc-macro crates.
This is covered by §1.5 (drop proc-macros). The codegen-backend
dlopen path is separate (`compiler/rustc_interface/src/util.rs:302`
— `load_backend_from_dylib`) and is covered by §1.2 (static-link
cg_clif). The plumbing model has a clean `#[cfg(feature = "llvm")]`
branch at `util.rs:341` we can mirror for cg_clif. False alarm.

### F4. Diagnostics emitter ANSI / Unicode / IsTerminal

`compiler/rustc_errors/src/emitter.rs` uses `anstream::AutoStream` +
`anstyle` for color, `unicode-width` for column metrics,
`std::io::IsTerminal` for the tty check. None of this is
load-bearing on SemOS — we can hard-code "not a tty, no colors,
ASCII only" and stub `IsTerminal`. anstream/anstyle are no_std-able
with `default-features = false`. unicode-width is pure no_std.
Diagnostics will look uglier on the SemOS console but work. False
alarm — this is a cleanup, not a blocker.

### F5. Jobserver client

`compiler/rustc_data_structures/src/jobserver.rs:11-54` —
`Client::from_env_ext(true)` returns `NoEnvVar` when there's no
jobserver env var (which is the SemOS case), and the code at lines
24-32 falls through to `default_client()` which creates a local
32-token jobserver. This works without inheriting from Cargo. False
alarm — even in single-threaded mode the jobserver protocol is
self-contained.

### F6. Backtrace / std::error::Report

`compiler/rustc_errors/src/lib.rs:24` imports
`std::backtrace::Backtrace` and `std::error::Report`. Both are
std-only. **But** they're only used in ICE handling and verbose error
display — the rustc_errors emitters work fine without them. Stub to
empty types. Once-off patch. False alarm.

### F7. Encoded metadata format / .rmeta file layout

`compiler/rustc_metadata/src/rmeta/encoder.rs:2420-2540` writes a
plain little-endian byte stream to a `File`. Header is fixed-string
+ position. No platform-specific layout. Works on any
read+write+seek-capable file. semos-std exposes File via OpenOptions
but is missing `Seek` — see B7 (added below as a normal porting
task, not a blocker). False alarm at the file-format level. The
**code** uses `seek` (`encoder.rs:2563, 2567`) so semos-std needs to
gain a `Seek` impl for File — but this is small. (Not a blocker.)

### F8. Tracing macros

`tracing = "0.1"` is used everywhere. Default features include `std`
but the crate is `default-features = false` no_std-compatible. The
`tracing-subscriber` crate used by `rustc_log` is std-heavy but only
the `init_logger` glue uses it — stub `rustc_log::init_logger` to
`Ok(())` and the `debug!()`/`info!()` macros throughout the compiler
become inert. False alarm — tracing is widespread but easily stubbed.

### F9. Encoded crate metadata using libc / unix-specific types

`compiler/rustc_metadata/Cargo.toml:37-40` has a `target_os = "aix"`
arm for libc; otherwise just `rustc_*` crates + `bitflags`,
`libloading` (F3), `odht`, `tempfile`, `tracing`. The format itself
is platform-neutral. False alarm.

---

## 3. Linker question

**How rustc produces an executable today.** `rustc_codegen_ssa::back::
link::link_natively` is the entry point. It calls into
`rustc_codegen_ssa::back::link::linker_with_args` to build a
`Command` struct, then `exec_linker` (`back/link.rs:1576-1644`) does
`cmd.command().spawn()` with `Stdio::piped()` and waits for the
external linker process (gcc/ld/lld/link.exe). The linker binary path
is found via `find_msvc_tools` or target-spec `linker_name`. The
plugin point — `rustc_codegen_ssa::back::linker::get_linker` (`back/
linker.rs:49-110`) — returns a `Box<dyn Linker + 'a>` whose `cmd`
field is a `super::command::Command` (a thin wrapper around
`std::process::Command`).

There is **no library/in-process linker path** in rustc proper. The
`get_linker` plugin model is for picking a *flavor* (gnu/lld/msvc/
darwin) but every flavor terminates in `Command::new(linker)
.spawn()`. We cannot use this path on SemOS.

**The cg_clif escape.** `compiler/rustc_codegen_cranelift/src/driver/
aot.rs:140-200` (`produce_final_output_artifacts`) emits a `.o`
object file and hands it back to rustc_codegen_ssa for linking. The
cranelift backend itself never calls a linker. It uses
`cranelift_object::ObjectModule` + `object` crate to write a single
relocatable ELF. **In our setup, what we want is to skip the
codegen_ssa link step entirely and let cg_clif emit a full ET_EXEC.**

This already works for `semos-cc`: `user-programs/semos-cc/src/
main.rs:175-230` builds an ET_EXEC from object bytes + a shim
template. The path to make this work for semos-rustc:

1. After `produce_final_output_artifacts` writes the `.o` files,
   intercept before `rustc_codegen_ssa::back::link::link_binary` is
   called. The interception point is `rustc_interface/src/passes.rs`'s
   `start_codegen` → driver → `link_binary` call chain.
2. Substitute a custom `link_binary_semos` that:
   - Reads each `.o` from disk (cg_clif already wrote them).
   - Concatenates the `.text`/`.data`/`.bss` sections.
   - Wraps in an ET_EXEC ELF with entry at SemOS's canonical
     0x400078 (same as semos-cc).
   - Writes the result to the user's `-o` output path.
3. Skip the entire `rustc_codegen_ssa::back::linker` chain.

The cleanest interception: **add a `CodegenBackend::link_executable`
trait method** (or just add a `#[cfg(target_os = "semos")]` branch in
the driver) that lets cg_clif own the final link step. This was
**not** in §1.2 (which only said "statically link cg_clif as a
codegen plugin") but should be added as a corollary decision.

**Open question for Phase 4:** rustc's driver expects to dispatch
multiple `.o` → linker; cg_clif emits one `.o` per CGU. With
incremental disabled and the single-CGU mode (`-Ccodegen-units=1`),
we always get exactly one `.o`. Good — that's the case our
semos-cc-style ET_EXEC emitter handles. Document the single-CGU
requirement.

---

## 4. Sysroot question

**Where rustc decides what sysroot to use.** Three independent
entry points all converge in `compiler/rustc_session/src/
filesearch.rs:189-256` (`default_sysroot`).

The discovery algorithm:

1. **`from_env_args_next`** (lines 233-252): if `argv[0]` is a
   symlink, follow it, pop `bin/rustc`, look for `lib/rustlib/$target`
   directory; if it exists, that's the sysroot.
2. **`default_from_rustc_driver_dll`** (lines 190-227): use
   `current_dll_path()` which calls `dladdr` on Unix (line 77),
   `GetModuleHandleExW` on Windows (line 150), to find the path of
   the `rustc_driver.dll`/`.so`. Walks up to find the sysroot.

The Linux/Windows paths both depend on dynamic-linking machinery
that SemOS does not have. The first path (argv[0] symlink) is
fakable but requires `fs::read_link` (`filesearch.rs:240`) which
semos-std does not implement.

**What rustc expects to find at the sysroot.** Looking at
`make_target_lib_path` (line 47) and the callers in
`rustc_metadata/src/locator.rs`:

- `$sysroot/lib/rustlib/$target/lib/` containing:
  - `libcore-<hash>.rlib`
  - `liballoc-<hash>.rlib`
  - `libstd-<hash>.rlib` (or our `libsemos_std-<hash>.rlib`)
  - All upstream sysroot crates (panic_abort, alloc, core, std,
    proc_macro, compiler_builtins, etc.).
- `$sysroot/lib/rustlib/$target/bin/` for self-contained linker (we
  skip this — see §3).

**Surface to fake out for SemOS.**

1. Replace `default_sysroot()` body entirely with a constant path:
   `PathBuf::from("/semos/rustc-sysroot")` (or a const set at build
   time via `option_env!`). This bypasses dladdr/GetModuleHandleExW.
   Single function rewrite.
2. Build a real rustlib tree at that path on the SemOS image
   containing:
   - `core.rlib`, `alloc.rlib`, `compiler_builtins.rlib` — already
     produced by the host nightly `build-std`. Copy these into the
     image filesystem.
   - `semos_std.rlib` — our `user-programs/std-shim` output.
   - A skeleton `panic_abort.rlib` (or a stub).
3. semos-std must expose `fs::read_link` (or rustc's `argv[0]` path
   must skip the `read_link` check on SemOS) — add a stub that
   returns `Err` so the argv[0] code falls through to
   `default_from_rustc_driver_dll` which we've stubbed to return our
   constant path.

**Phase-2 contact surface (concrete files to patch):**

- `compiler/rustc_session/src/filesearch.rs:189-256` — replace
  `default_sysroot()` body.
- `compiler/rustc_session/src/filesearch.rs:60-186` — delete the
  per-OS `current_dll_path` implementations or `#[cfg(target_os =
  "semos")]` them to `Err("not supported")`.
- `compiler/rustc_session/src/config.rs` — the `Sysroot` struct,
  passed through `Session` to everything. No code changes; just
  becomes a constant.
- Build-time: `option_env!("SEMOS_SYSROOT")` baked into the
  semos-rustc binary at host-side build time.

**Not blocking** for the v1 plan; clean fake-out path. The kernel
needs to expose the sysroot via the normal SemOS FS (we already have
`SYS_OPEN` / `SYS_FREAD` / etc.), so no kernel work — just image
provisioning.

---

## 5. Stop condition assessment

**Confirmed blockers count: 5.**

- B1 — Panic-as-control-flow (FatalError + catch_unwind).
- B2 — Thread-local storage (SESSION_GLOBALS + TyCtxt TLV).
- B3 — Stacker / dynamic stack growth.
- B4 — Vendored rayon fork (`rustc_thread_pool`) is heavier than a
  shim.
- B5 — PathBuf / OsStr type-shape gap in semos-std.

**Plan §2 stop condition.** "If R4 identifies more than 3 'no clean
mitigation' blockers, the project's strategy needs rethinking."

**Verdict: PROCEED to Phase 2 with caveats. Not the strict
stop-condition trigger, but close.**

Reasoning:

- **B5 has a clean mitigation** (Option A: extend semos-std with
  `OsString`/`OsStr`/`PathBuf` wrappers). Single-session, clearly
  scoped, no fundamental obstacle. Don't count toward stop.
- **B2 has a clean mitigation** (Option A: single-threaded TLS shim
  via static-cell macro). Multi-session but mechanical. Don't count
  toward stop.
- **B3 has a clean mitigation** (Option A: vendor + port psm's
  x86_64 backend). Single-session, well-bounded. Or fall back to
  Option B (no-op + big stack) for v1. Don't count toward stop.
- **B4 has a clean mitigation** (Option A: 50-line stub of
  rustc_thread_pool). Don't count toward stop.
- **B1 is the one true unresolved blocker.** Option C (accept that
  any FatalError exits the process) is the v1 plan. It works for
  hello-world but means semos-rustc cannot report >1 error per
  invocation, and complex multi-pass scenarios may surface FatalError
  inside loops that we'd want to recover from. Option A (rewrite
  FatalError to Result) is a multi-session subproject. Option B
  (implement unwinding on SemOS) is a kernel feature outside M27's
  scope.

**Net: 1 unaddressed blocker (B1).** Strictly below the "more than 3"
threshold.

**However**, B1 is qualitatively the kind of blocker that can sink
the project later. If during Phase 5 we discover that the
v1-acceptable Option C breaks because diagnostics aren't flushed
before the abort, or because some recoverable error path needs to
unwind, we'll be looking at a 5-10 session detour to either rewrite
FatalError or implement an unwinder. **Recommend** the project plan
add a §1.7 decision point before Phase 2 starts:

> §1.7 (proposed) — FatalError handling on SemOS. v1 uses panic=abort
> + acceptance that any FatalError terminates the rustc process. If
> this proves intolerable in Phase 5, implement a SemOS
> stack-unwinder (gimli + a minimal eh_personality in semos-std,
> estimated 3-5 sessions).

And add lines to the Phase 2 spec covering B2-B5 mitigation as
foundation work.

With those amendments, Phase 2 can start.

**Biggest single blocker.** B1 — Panic-as-control-flow.

---

## Notes for Phase 2 onboarding

- The single-shot semos-std OsString/PathBuf addition (B5 Option A)
  should land **before** any rustc crate gets touched. Otherwise
  every per-crate port will discover the same hole independently.
- The `rustc_thread_pool` shim crate (B4 Option A) and the
  `scoped-tls` / `thread_local!` no-op macros (B2 Option A) can be
  prepared as standalone vendor crates **before** Phase 2 even
  starts. They are pure-mechanical and unblock everything below
  rustc_data_structures.
- The stacker question (B3) should be resolved during Phase 2 at the
  `rustc_data_structures` port; whichever route (port psm or
  no-op-stub) is chosen, the choice ripples through 20 callers.
- §1.7 (the FatalError decision) should be made by the orchestrator
  *now*, before Phase 2 spawns. The wrong choice here doesn't surface
  until DEMO 80 in Phase 5.
- §3's linker-bypass corollary should be made an explicit §1.8 — let
  cg_clif own the final ET_EXEC emission, skip
  `rustc_codegen_ssa::back::link::link_binary` entirely on SemOS.
