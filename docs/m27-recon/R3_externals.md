# M27 Phase 1 — R3: rustc external-deps audit

Drafted 2026-05-30. Agent R3 of the four Phase 1 recon runs (see
`docs/M27_RUSTC_PORT_PLAN.md` §0–1 and the Phase 1 spawn brief).

**Scope.** Every external (non-`rustc_*`) crate referenced by the
`[dependencies]`, `[build-dependencies]`, and `[dev-dependencies]`
sections of every `compiler/rustc_*/Cargo.toml` in
`user-programs/semos-rustc/vendor-rustc-src/`. Source: the `Cargo.toml`
files themselves, the existing Cranelift-port vendor directory
(`user-programs/semos-cc/vendor/`), and `user-programs/std-shim/`.

**Method.** Read-only. Read each crate's `Cargo.toml`. Cross-reference
the existing vendored crates by directory name + version. For wall
classifications, cite a representative file:line where the dep is
used. Where a registry copy of the external's own `Cargo.toml` was
not accessible from the sandbox, the no_std posture is inferred from
crate name + version + how rustc uses it (this is called out below).

**The §1 decisions in the plan are presumed.** Specifically, decisions
1.1 (no LLVM backend) and 1.2 (static-link cg_clif) mean every external
that *only* feeds `rustc_codegen_llvm`, `rustc_codegen_gcc`,
`rustc_baked_icu_data`, `rustc_llvm`, or `rustc_proc_macro` is marked
"DROPPED" rather than "WALL" — it leaves the dep graph with the crate.
Decision 1.3 (drop incremental) drops `rustc_incremental` and its
deps. Decision 1.4 (drop rayon) drops `rustc_thread_pool` + its
crossbeam deps. Decision 1.5 (drop proc-macros at runtime) leaves
`rustc_proc_macro` out of the runtime graph (it's still needed at
**host build time** for `rustc_macros` etc. — see §5).

---

## 1. Master external-deps table

Versions are the version-spec written in the rustc-src Cargo.toml.
"Cranelift vendor" is the version under `user-programs/semos-cc/vendor/`.
"semos-std" is whether `user-programs/std-shim/Cargo.toml` already
depends on the crate.

| crate                  | rustc ver        | Cranelift vendor | semos-std | wall class                  |
|------------------------|------------------|------------------|-----------|-----------------------------|
| **Already vendored & reusable** | | | | |
| `bumpalo`              | (transitive)     | 3.20.3           | no        | OK (no_std)                 |
| `cfg-if`               | (transitive)     | 1.0.4            | no        | OK (no_std)                 |
| `hashbrown`            | 0.16.1           | 0.15.5 + 0.17.1  | 0.15      | CHECK (version drift, see §4) |
| `indexmap`             | 2.0–2.14         | 2.14.0           | no        | OK (compatible, no_std)     |
| `libm`                 | (cg_clif dep)    | 0.2.16           | no        | OK                          |
| `log`                  | (transitive)     | 0.4.30           | no        | OK                          |
| `memchr`               | 2.7.6            | 2.8.1            | no        | OK (no_std; minor bump)     |
| `proc-macro2`          | 1                | 1.0.106          | no        | OK (host-only, see §5)      |
| `quote`                | 1                | 1.0.45           | no        | OK (host-only)              |
| `rustc-hash`           | 2.0.0            | 2.1.2            | no        | OK (no_std)                 |
| `smallvec`             | 1.8.1            | 1.15.1           | no        | OK (no_std)                 |
| `syn`                  | 2.0.9            | 2.0.117          | no        | OK (host-only)              |
| `target-lexicon`       | 0.13             | 0.13.5           | no        | OK                          |
| `unicode-ident`        | 1.0.22           | 1.0.24           | no        | OK (no_std)                 |
| `object`               | 0.37.0           | 0.36.7           | no        | CHECK (0.37 vs 0.36 — both no_std-capable with `default-features = false`) |
| **Newly required externals (small / clean)** | | | | |
| `arrayvec`             | 0.7              | —                | no        | OK (no_std documented)      |
| `bitflags`             | 2.4–2.9          | —                | no        | OK (no_std)                 |
| `derive-where`         | 1.2.7            | —                | no        | OK (proc-macro, host-only)  |
| `either`               | 1.0–1.5          | —                | no        | OK (no_std)                 |
| `itoa`                 | 1.0              | —                | no        | OK (no_std)                 |
| `pathdiff`             | 0.2.0            | —                | no        | PATCH (tiny — uses std::path) |
| `punycode`             | 0.4.0            | —                | no        | CHECK (likely OK)           |
| `rustc-demangle`       | 0.1.21           | —                | no        | OK (no_std)                 |
| `rustc-literal-escaper`| 0.0.7            | —                | no        | OK (parts of rustc itself, no_std) |
| `rustc-stable-hash`    | 0.1.0            | —                | no        | OK (no_std capable)         |
| `scoped-tls`           | 1.0              | —                | no        | OK (no_std with macro feature) |
| `shlex`                | 1.0              | —                | no        | OK (alloc-only)             |
| `thin-vec`             | 0.2.12           | —                | no        | OK (no_std)                 |
| `twox-hash`            | 1.6.3            | —                | no        | OK (no_std)                 |
| `unicode-normalization`| 0.1.25           | —                | no        | OK (no_std)                 |
| `unicode-properties`   | 0.1.4            | —                | no        | OK (no_std)                 |
| `unicode-security`     | 0.1.0            | —                | no        | OK (no_std)                 |
| `unicode-width`        | 0.2.2            | —                | no        | OK (no_std)                 |
| `synstructure`         | 0.13.0           | —                | no        | OK (proc-macro, host-only)  |
| **PATCH — clean std-feature gate, defaults to std** | | | | |
| `anstyle`              | 1.0.13           | —                | no        | PATCH (1 session — has `std` feat) |
| `derive_setters`       | 0.1.6            | —                | no        | OK (proc-macro, host-only)  |
| `fluent-bundle`        | 0.16             | —                | no        | DEEP-PATCH (see §3)         |
| `fluent-syntax`        | 0.12             | —                | no        | PATCH (lexer; 1 session)    |
| `intl-memoizer`        | 0.5.1            | —                | no        | PATCH (1 session)           |
| `unic-langid`          | 0.9.0            | —                | no        | PATCH (1 session)           |
| `getopts`              | 0.2              | —                | no        | PATCH (uses std::env::args slicing) |
| `gimli`                | 0.31 / 0.32      | —                | no        | OK (cg_clif already uses, no_std with `default-features = false`) |
| `polonius-engine`      | 0.13.0           | —                | no        | DEEP-PATCH (see §3)         |
| `regex`                | 1.4              | —                | no        | DEEP-PATCH (see §3)         |
| **PATCH — significant std contact (hashing / heavy)** | | | | |
| `blake3`               | 1.5.2            | —                | no        | PATCH (no_std + alloc OK; SIMD detect uses std) |
| `sha1`                 | 0.10.0           | —                | no        | OK (no_std)                 |
| `sha2`                 | 0.10.1           | —                | no        | OK (no_std)                 |
| `md-5`                 | 0.10.0           | —                | no        | OK (no_std)                 |
| `ar_archive_writer`    | 0.5              | —                | no        | PATCH (writes to `std::io::Write`) |
| `bstr`                 | 1.11.3           | —                | no        | PATCH (no_std with `default-features = false`) |
| `annotate-snippets`    | 0.11 / 0.12.10   | —                | no        | DEEP-PATCH (anstream + simd) |
| `anstream`             | 0.6.20           | —                | no        | DEEP-PATCH (terminal IO)    |
| `wasm-encoder`         | 0.219            | —                | no        | PATCH (alloc-only; check)   |
| `serde`                | 1.0.125          | (serde 1.0.228)  | no        | CHECK (cg_clif vendored serde already, see §4) |
| `serde_derive`         | 1.0.219          | 1.0.228          | no        | OK (host-only)              |
| `serde_json`           | 1.0.59           | —                | no        | PATCH (use `alloc` feature) |
| `serde_path_to_error`  | 0.1.17           | —                | no        | CHECK                       |
| `schemars`             | 1.0.4            | —                | no        | CHECK (likely PATCH; serde-ext) |
| `odht`                 | 0.3.1            | —                | no        | PATCH (mmap-backed hash; see §3) |
| `tracing`              | 0.1              | —                | no        | DEEP-PATCH (see §3)         |
| `tracing-core`         | 0.1.34           | —                | no        | DEEP-PATCH (see §3)         |
| `tracing-subscriber`   | 0.3.3            | —                | no        | WALL (see §2)               |
| `tracing-tree`         | 0.3.1            | —                | no        | WALL (depends on tracing-subscriber) |
| `gsgdt`                | 0.1.2            | —                | no        | CHECK (graph data, likely OK) |
| `elsa`                 | 1.11.0           | —                | no        | DEEP-PATCH (see §3)         |
| `ena`                  | 0.14.3           | —                | no        | OK (no_std + alloc)         |
| **WALL — fundamentally infeasible without §1 decisions or further mitigation** | | | | |
| `libloading`           | 0.8.0 / 0.9.0    | —                | no        | WALL (dlopen) — see §2      |
| `libc`                 | 0.2 / 0.2.50/.73 | —                | no        | WALL (POSIX syscalls) — see §2 |
| `windows` (crate)      | 0.61.0           | —                | no        | WALL (Win32 syscalls) — see §2 |
| `tikv-jemalloc-sys`    | 0.6.1            | —                | no        | DROPPED (optional `jemalloc` feature off) |
| `gccjit`               | 3.1.1            | —                | no        | DROPPED (cg_gcc not in tree per §1.1) |
| `boml`                 | 0.3.1            | —                | no        | DROPPED (cg_gcc dev-dep)    |
| `lang_tester`          | 0.8.0            | —                | no        | DROPPED (cg_gcc dev-dep)    |
| `tempfile`             | 3.2 / 3.7.1      | —                | no        | WALL → PATCH (see §2)       |
| `parking_lot`          | 0.12             | —                | no        | PATCH (uses std::sync; replace w/ spin) |
| `memmap2`              | 0.2.1            | —                | no        | WALL → STUB (see §2)        |
| `stacker`              | 0.1.17           | —                | no        | WALL → STUB (see §2)        |
| `jobserver`            | 0.1.28           | —                | no        | DROPPED (single-thread per §1.4) |
| `crossbeam-deque`      | 0.8              | —                | no        | DROPPED (rustc_thread_pool only) |
| `crossbeam-utils`      | 0.8              | —                | no        | DROPPED (rustc_thread_pool only) |
| `portable-atomic`      | 1.5.1            | —                | no        | OK (no_std; only on non-atomic-64 targets) |
| `measureme`            | 12.0.1           | —                | no        | WALL → STUB (see §2)        |
| `termize`              | 0.2              | —                | no        | WALL → STUB (ioctl TIOCGWINSZ) |
| `ctrlc`                | 3.4.4            | —                | no        | WALL → STUB (signal handler; see §2) |
| `jiff`                 | 0.2.5            | —                | no        | WALL → STUB (wall clock; see §2) |
| `find-msvc-tools`      | 0.1.2            | —                | no        | DROPPED (build-script Windows-only) |
| `cc`                   | =1.2.16          | —                | no        | DROPPED (rustc_llvm build-dep only) |
| `pulldown-cmark`       | 0.11             | —                | no        | PATCH (used only in rustc_resolve for rustdoc-like link parsing; verify no_std default-features=false posture) |
| `getrandom`            | =0.3.3           | —                | no        | DROPPED (wasi target only)  |
| `wasi`                 | =0.14.2          | —                | no        | DROPPED (wasi target only)  |
| `expect-test`          | 1.4.0            | —                | no        | DROPPED (dev-dep)           |
| `rand`                 | 0.9.0            | —                | no        | PATCH (no_std with `default-features = false`; only `rustc_session` + `rustc_incremental` consume it; incremental is dropped) |
| `rand_xoshiro`         | 0.7.0            | —                | no        | OK (no_std)                 |
| `rand_xorshift`        | 0.4              | —                | no        | DROPPED (rustc_thread_pool dev-dep only) |
| `itertools`            | 0.12             | —                | no        | PATCH (no_std with `default-features = false` — 7 rustc crates use it) |
| **External rustc-API-paired** | | | | |
| `rustc_apfloat`        | 0.2.0            | —                | no        | OK (no_std + alloc by design; this is rustc's apfloat crate published to crates.io) |
| `icu_list`             | 2.0              | —                | no        | DROPPED (rustc_baked_icu_data + rustc_error_messages — only needed for fluent locale formatting; can stub locale=en) |
| `icu_locale`           | 2.0              | —                | no        | DROPPED (same as above)     |
| `icu_provider`         | 2.0              | —                | no        | DROPPED (same)              |
| `zerovec`              | 0.11.0           | —                | no        | DROPPED (icu transitive)    |
| `thorin-dwp`           | 0.9              | —                | no        | DROPPED (DWARF packaging — not needed for first compile) |
| `measureme`            | 12.0.1           | (dup row above)  | no        | (same)                      |

**Total external crates counted (deduped, dropping pure-LLVM/GCC/wasi-target/dev-only):** 71 distinct crate names appearing in some `[dependencies]` section. After applying the §1 decisions and `dev`-dep removal, **~50–55 are in the runtime port surface**; the rest are dropped by decision.

---

## 2. WALL detail

These are the cases where "find a clean Cargo feature" doesn't work
and we have to commit to a mitigation strategy up front. Cited line
numbers are in `user-programs/semos-rustc/vendor-rustc-src/`.

### `libloading` (0.8.0 in rustc_metadata, 0.9.0 in rustc_codegen_cranelift jit feature, 0.9.0 in rustc_codegen_llvm)

**What it does in rustc.** Dynamic loading of codegen backends and
proc-macro crates. `rustc_metadata::creader::load_dylib` (see
`compiler/rustc_metadata/src/creader.rs:1394`) and the supporting
`attempt_load_dylib` at `:1367` use `libloading::Library::new` /
`libloading::os::unix::Library::open` to resolve plugin codegen
backends at runtime.

**Mitigation options:**
1. **§1.2 already chooses static-link cg_clif** — `rustc_codegen_llvm`
   leaves the tree entirely (§1.1) and `rustc_codegen_cranelift` is
   linked as a regular cargo dep. That removes the codegen-backend
   call site.
2. **proc-macros are punted** (§1.5). The crate-loader still has
   *dylib-bearing* code paths for proc-macro crates; those must be
   cfg-gated out and replaced with a single error path "proc-macros
   unsupported." Trace through `rustc_metadata/src/creader.rs` —
   probably ~50 lines of `cfg(not(feature = "no_dylib_plugins"))`.
3. **Drop `libloading` from `[dependencies]`** of `rustc_metadata`
   once those code paths are cfg'd. The dep then disappears.

This is mechanical work, not architectural — *but* it's exactly the
"plugin-load model gone" surgery the plan budgets generously in
Phase 4. Stop condition for Phase 4 explicitly calls this out.

### `libc` (used by `rustc_codegen_ssa` cfg(unix), `rustc_data_structures` cfg(unix), `rustc_driver_impl` cfg(unix), `rustc_llvm`, `rustc_session` cfg(unix), `rustc_metadata` cfg(target_os="aix"))

**What it does in rustc.** POSIX FFI: `flock`, `mmap`, `getpid`,
sigaction-y stuff, terminal size queries.

**Mitigation options.** The target is `x86_64-unknown-none` (SemOS).
None of the `cfg(unix)` / `cfg(target_os = "...")` blocks compile in
on SemOS. So `libc` falls out **by target**, not by code change —
provided nobody is using `libc` unconditionally. Quick verification
needed: grep each rustc_* crate for `extern crate libc;` or `use
libc::` outside a `cfg(unix)` / `cfg(windows)` / `cfg(target_os …)`
block. (Tentatively believed clean; the audit is a one-session check
in Phase 2 against `rustc_data_structures`.)

### `windows` (0.61.0; used in rustc_data_structures, rustc_errors, rustc_session, rustc_codegen_ssa, rustc_driver_impl)

**What it does in rustc.** Win32 file locks
(`rustc_data_structures/src/flock/windows.rs:7`), Win32 error
message lookup (`rustc_codegen_ssa/src/back/link.rs:1144` — locale
codepage lookup), Win32 library-loading-style search-path lookups
(`rustc_session/src/filesearch.rs:144`), MUTEX wait
(`rustc_errors/src/lock.rs:19`), and stackdump backtrace.

**Mitigation options.** Same as `libc` — all of these are
`cfg(windows)` blocks. On `x86_64-unknown-none` they don't compile.
Falls out by target. Verify same way as `libc`.

### `tempfile` (3.2 in rustc_data_structures + rustc_metadata, 3.7.1 in rustc_fs_util)

**What it does.** Spool a temp file for `rustc_metadata` cratefile
output, plus `rustc_codegen_ssa` linker arg files.

**Mitigation options.** SemOS has a filesystem but no `O_TMPFILE` or
randomly-named temp files. **Path A:** patch `tempfile` to call a
semos-std shim that just opens `/tmp/rustc-XXXXX` (we pick names).
**Path B:** patch every rustc usage site to call a single
`rustc_fs_util::make_temp_file` that hides the dep. (B is cleaner;
~1 session.) Marked WALL → PATCH in the table.

### `parking_lot` (0.12 in rustc_data_structures + rustc_query_system)

**What it does.** Faster Mutex / RwLock / Condvar. rustc's
`rustc_data_structures::sync` re-exports parking_lot's `Mutex`,
`MutexGuard`, `RwLock`, `RwLockReadGuard`, `Once`. The query system
uses parking_lot for its sharded-lock construction.

**Mitigation options.** With §1.4 (single-thread), every lock can be
a no-op. Easiest: replace `parking_lot::Mutex<T>` with
`core::cell::RefCell<T>` (or `spin::Mutex` if we ever go
multithreaded). semos-std already has thread::spawn but no
contention-free locks — a 50-line `parking_lot` shim crate that
re-exports `spin::Mutex` etc. is feasible. **~1 session.**

### `memmap2` (0.2.1 in rustc_data_structures, `cfg(not(target_arch = "wasm32"))`)

**What it does.** Memory-map metadata files for fast crate-loading.
Used at `compiler/rustc_data_structures/src/memmap.rs`. The semos-std
filesystem has no mmap.

**Mitigation options.** Provide a thin shim that reads the entire file
into a `Vec<u8>` and exposes the same `Mmap` / `Slice` API. Slow but
correct. ~1 session. **Mark as STUB.**

### `stacker` (0.1.17 in rustc_data_structures)

**What it does.** Detects when the recursion stack is low and grows
a new stack segment. Used in `rustc_data_structures::stack::ensure_sufficient_stack`
at `:21` to wrap recursive descents in `rustc_resolve` / `rustc_const_eval`.

**Mitigation options.** semos-std threads have a fixed stack. We
**cannot** grow them. Mitigation: replace `stacker::maybe_grow` with
a function that just calls the closure directly and trust that the
SemOS process stack is large enough (we already bumped to 4 MiB per
the risk register; deeply recursive rustc work may still blow it).
**Mark as STUB.** Long-term: increase USER_PROC_STACK_SIZE further;
much-longer-term: implement segmented stacks in semos-std.

### `measureme` (12.0.1 in rustc_data_structures, rustc_query_impl, rustc_codegen_llvm)

**What it does.** rustc's self-profiling backend — writes
`*.events`/`*.string_data` files. Used at
`compiler/rustc_data_structures/src/profiling.rs:96`.

**Mitigation options.** Self-profiling is optional in rustc; the
`rustc_data_structures::profiling::SelfProfiler` type can be feature-
gated out. Provide a no-op shim that matches the public API but
discards data. **Mark as STUB.** Roughly 100 lines of API surface
to stub; ~1 session.

### `termize` (0.2 in rustc_session + rustc_errors)

**What it does.** Query terminal width for diagnostic line-wrapping.
On Linux uses `ioctl(TIOCGWINSZ)`; on Windows uses console API.

**Mitigation options.** Return `None` (no terminal width known) →
the diagnostic emitter falls back to 80-col default. Provide a
`termize` shim crate. **~1 session.**

### `ctrlc` (3.4.4 in rustc_driver_impl, `cfg(not(target_family = "wasm"))`)

**What it does.** Install Ctrl-C handler that lets the user interrupt
a long rustc run cleanly. Used at
`compiler/rustc_driver_impl/src/lib.rs:1659`.

**Mitigation options.** SemOS has no SIGINT analogue yet. Provide
a `ctrlc` shim where `set_handler` is a no-op. ~30 lines of code.
**Mark as STUB.**

### `jiff` (0.2.5 in rustc_driver_impl, used for log filename stamps)

**What it does.** Wall-clock dates for ICE/log filenames. Used at
`compiler/rustc_driver_impl/src/lib.rs:1431` — `jiff::Zoned::now().strftime(...)`.

**Mitigation options.** SemOS does not have a wall clock and certainly
not a TZ database. **Path A:** stub `jiff` to return a fixed
`1970-01-01T00:00:00` (sorting by name still works because we append
PID). **Path B:** replace the call site with a one-shot tick counter.
A is less invasive. **~1 session.**

### `tracing-subscriber` + `tracing-tree`

**What it does.** Pretty-print spans/events. Pulls in `parking_lot`,
`smallvec`, `ansi` features, dynamic dispatch via `Layered`. Used by
`rustc_log` to attach a subscriber.

**Mitigation options.** Provide a `rustc_log` replacement that uses a
no-op tracing subscriber. `tracing` itself can be patched into a
no_std crate (it has a `std` feature). `tracing-subscriber` is much
heavier — uses `std::fs`, `std::env`, plus terminal queries. **This
is the cleanest place to truly cut.** Mark `tracing-subscriber` and
`tracing-tree` **WALL**: replace `rustc_log` with a minimal "tracing
events go to /dev/null" stub. ~150 lines.

### Summary of unmitigated WALL count

After applying §1 decisions + the STUB strategies above:

- **0 truly unmitigated walls.** Every WALL crate in the table has a
  mitigation: cfg-out (libc, windows), stub (memmap2, stacker,
  measureme, termize, ctrlc, jiff, tracing-subscriber), drop
  (jemalloc, gccjit, wasi, jobserver, crossbeam, icu, thorin-dwp,
  cc, find-msvc-tools), or patch (tempfile, parking_lot).

- This means **Phase 1's stop condition (§ "Stop condition for Phase 1")
  is met**: R3 does not surface a fourth unmitigated wall beyond
  the LLVM / libloading / rayon trio already addressed by §1.

---

## 3. PATCH / DEEP-PATCH list — work estimates

Effort is in "sessions" relative to the Cranelift port — where ~5
small-pattern crates fit in one focused session. "Big" means the
crate has its own `std::*` use that needs the bulk-substitution
recipe across many files.

| crate                | est | rationale |
|----------------------|-----|-----------|
| `bitflags`           | <0.1 | already no_std with `default-features = false`; just verify |
| `arrayvec`           | <0.1 | no_std out-of-box |
| `itertools`          | 0.5  | has `default-features = false` for no_std; verify each rustc consumer compiles |
| `either`             | <0.1 | trivial |
| `thin-vec`           | 0.2  | claims no_std; verify |
| `gimli`              | 0.5  | cg_clif already uses; the rustc-src copy is a different version but same posture |
| `serde`              | 0.5  | cg_clif vendored 1.0.228; rustc wants 1.0.125; cargo will likely accept 1.0.228 — verify lockfile compat |
| `serde_json`         | 1    | uses `alloc` feature; rustc passes string output through it; small patch |
| `serde_derive`       | 0    | host-only proc-macro; nothing to do |
| `serde_path_to_error`| 0.5  | check `default-features = false` works |
| `schemars`           | 1    | unclear no_std story; for SemOS we can drop schemars and replace its derives with hand-written `Serialize` (only `rustc_target` uses it) |
| `getopts`            | 1    | uses `std::env`; replace with `pico-args` or hand-roll a tiny CLI parser since we already have a fixed-CLI shim |
| `pathdiff`           | 0.5  | one function, std::path → use a no_std reimplementation |
| `bstr`               | 1    | feature-gate no_std; ~3 rustc consumers (rustc_codegen_ssa) |
| `ar_archive_writer`  | 2    | writes via `std::io::Write` — replace with `core2::io::Write` shim |
| `annotate-snippets`  | 2    | depends on `anstream` + `simd` features; cut simd, patch anstream |
| `anstream`           | 2    | terminal styling; replace with a "no styles" passthrough crate (cleanest cut) |
| `wasm-encoder`       | 1    | alloc-only Cargo feature exists; verify |
| `polonius-engine`    | 3    | dataflow engine using `std::collections::*` heavily; bulk substitution |
| `regex`              | 3    | `default-features = false` gives `unicode-perl`-less regex; rustc_mir_dataflow + rustc_codegen_ssa use it for trivial patterns. Could replace with hand-rolled matches and **drop the regex dep entirely** — that's actually a smaller patch than porting regex. |
| `pulldown-cmark`     | 1    | only `rustc_resolve` uses; can stub out the doc-link parsing path |
| `odht`               | 2    | mmap-backed on-disk hash; needs the memmap2 STUB underneath; patch to read whole file into Vec |
| `parking_lot`        | 1    | shim crate exporting spin::Mutex (or RefCell since single-thread) |
| `elsa`               | 2    | append-only stable-ref collections; uses `std::sync`; rewrite to use single-threaded RefCell |
| `fluent-bundle`      | 3    | heavy use of std::collections + ICU; rustc's localized error messages need a stub that just returns the English template strings |
| `fluent-syntax`      | 1    | hand-rolled lexer; mostly clean |
| `intl-memoizer`      | 1    | shim |
| `unic-langid`        | 1    | langid parsing; mostly no_std-clean with feature |
| `tracing`            | 2    | core has `std` feature; turn it off; macro layer needs care |
| `tracing-core`       | 2    | same — but rustc_log builds on it heavily |
| `blake3`             | 1    | no_std-capable but SIMD detection uses std; cfg-gate to fallback |
| `rand`               | 0.5  | no_std with `default-features = false`; only rustc_session and rustc_incremental (dropped) consume it |
| `tempfile`           | 1    | replace with rustc_fs_util::make_temp_file |

Plus the STUB crates (each ~1 session): `memmap2`, `stacker`,
`measureme`, `termize`, `ctrlc`, `jiff`, `tracing-subscriber`,
`tracing-tree` (the last two collapse into a single rustc_log shim
~1 session).

**PATCH/DEEP-PATCH crates: 24. STUB crates: 8. Rough total: ~30
sessions on externals.** That's a meaningful chunk of the plan's
30–60-session budget — externals alone could eat half.

**Top 5 by effort:** `polonius-engine` (3), `regex` (3) — but
probably cheaper to drop, `fluent-bundle` (3), `tracing` (2),
`tracing-core` (2), `elsa` (2). (Tied at 2: `annotate-snippets`,
`anstream`, `odht`, `ar_archive_writer`.)

---

## 4. Cross-reference with semos-cc/vendor (Cranelift port)

Reusable patches — already done as part of the Cranelift port. These
should drop straight in:

| crate            | cg_clif version | rustc version | reuse? |
|------------------|-----------------|---------------|--------|
| `hashbrown`      | 0.15.5 + 0.17.1 | 0.16.1        | **Partial.** Cranelift port patched both versions with `default-features = ["default-hasher"]` off / `nightly` feature handling. The rustc requirement is 0.16.1 — which we have NOT yet vendored. Two options: (a) re-pin rustc to 0.17 (probably fine, rustc only uses common APIs), (b) re-do the patch for 0.16. **(a) preferred — 0.5 sessions.** |
| `indexmap`       | 2.14.0          | 2.0–2.14      | **Full.** 2.14.0 satisfies all callers. |
| `smallvec`       | 1.15.1          | 1.8.1         | **Full.** 1.15.1 satisfies all rustc callers (it asks for `>=1.8.1`). |
| `bumpalo`        | 3.20.3          | not directly used by rustc; cg_clif transitive | n/a |
| `cfg-if`         | 1.0.4           | transitive via tracing etc. | **Full reuse.** |
| `libm`           | 0.2.16          | not in rustc — only in cg_clif | n/a |
| `log`            | 0.4.30          | only tracing-log transitive | likely reuse |
| `memchr`         | 2.8.1           | 2.7.6         | **Full reuse** (2.8.1 satisfies 2.7.6). |
| `object`         | 0.36.7          | 0.37.0/0.37.3 | **Version drift.** rustc wants 0.37.x; cg_clif uses 0.36. Need to upgrade cg_clif's vendored object to 0.37 *or* downgrade rustc — likely **upgrade** since cg_clif's own deps (`gimli`) tolerate 0.37. ~1 session. |
| `proc-macro2`    | 1.0.106         | 1             | **Full.** host-only. |
| `quote`          | 1.0.45          | 1             | **Full.** host-only. |
| `rustc-hash`     | 2.1.2           | 2.0.0         | **Full.** |
| `syn`            | 2.0.117         | 2.0.9         | **Full.** |
| `target-lexicon` | 0.13.5          | 0.13          | **Full.** |
| `unicode-ident`  | 1.0.24          | 1.0.22        | **Full.** |
| `serde` 1.0.228 + `serde_derive` 1.0.228 + `serde_core` 1.0.228 | yes | rustc wants 1.0.125/.219 | **Full reuse** (cargo will pick newest). |
| `gimli`          | (transitive) 0.31.x | 0.31/0.32  | Partial — both Cranelift port and rustc use gimli; reuse the cg_clif patch. |

**Reusable from Cranelift port: 14 crates (out of 14 in cg_clif vendor) ≈ all of them.** Net effort saved vs. starting from scratch on this group: about 1 session of porting plus the version-bump work above.

The remaining ~30 PATCH/STUB crates are net-new work on top of the Cranelift port.

---

## 5. Tools / proc-macros — host-build-time only

Several crates are **never linked into the SemOS rustc binary** because
they're proc-macros that run during `cargo build` on the host. They
still need to be available on the host build machine, but their no_std
posture is irrelevant.

| crate                  | role | concern |
|------------------------|------|---------|
| `proc-macro2`          | proc-macro AST | host-only; no concern |
| `quote`                | proc-macro emit | host-only |
| `syn`                  | proc-macro parse | host-only |
| `synstructure`         | derive helpers | host-only |
| `rustc_macros`         | rustc's own internal derives (Diagnostic, etc.) | host-only |
| `rustc_index_macros`   | newtype index! macro | host-only |
| `rustc_type_ir_macros` | TypeFoldable derives | host-only |
| `rustc_fluent_macro`   | fluent literal codegen | host-only |
| `derive-where`         | derive bound impls | host-only |
| `derive_setters`       | builder derives | host-only |
| `serde_derive`         | serde derive | host-only |

**These need a working host rustc that can build proc-macros.** That's
a regular nightly rustc — no SemOS-side work. They are big deps
(`syn` alone is several thousand LoC) but they don't affect the SemOS
build at all.

`rustc_proc_macro` is a separate beast: it's a path-deps wrapper around
`library/proc_macro/src/lib.rs`. Per §1.5 we **don't support runtime
proc-macro expansion**, so we leave `rustc_proc_macro` in the source
tree but cfg-gate all consumers to a "proc-macros unsupported" stub.
Listed in the same row as `rustc_metadata`'s plugin code (Phase 4).

---

## 6. Surprises

Some things the plan didn't anticipate, ranked roughly by impact.

1. **rustc has its own forked rayon** at `compiler/rustc_thread_pool/`
   (`rustc_thread_pool` package name, with comment "Core APIs for
   Rayon - fork for rustc"). The §1.4 decision says "patch
   rustc_data_structures and rustc_query_impl to use a single-threaded
   shim" — that's exactly the right call, because rustc_thread_pool
   has its own `crossbeam-deque` + `crossbeam-utils` deps which are
   **standalone walls** (the threadpool is the only consumer of
   them). Dropping rustc_thread_pool kills three external crates at
   once.

2. **`elsa` is not on the plan's radar but is a real porting cost.**
   `elsa` provides append-only collections with stable references —
   it's used by rustc's intern caches. Its `std::sync` posture means
   it needs patching, and it's not one of the "well-known no_std-
   compatible" crates. ~2 sessions on its own.

3. **`fluent-bundle` + `unic-langid` + `intl-memoizer` + the ICU stack
   (icu_list, icu_locale, icu_provider, zerovec)** form a cluster of
   ~7 crates that all exist to make rustc's diagnostic messages
   localizable. The plan should explicitly call out "drop
   localization, hardcode English templates" as an additional §1
   decision (call it 1.7). Otherwise these eat ~5 sessions on
   features SemOS doesn't need. The diagnostic codegen via
   `rustc_fluent_macro` happens at host-build time and bakes English
   templates into the binary — we'd need to verify that path actually
   works without runtime fluent.

4. **`measureme` is used by the LLVM backend too** — confirmed via
   `rustc_codegen_llvm/src/back/profiling.rs:5`. Cutting LLVM
   removes one of the three measureme consumers; the other two
   (`rustc_data_structures` + `rustc_query_impl`) still need the
   stub.

5. **`odht` is a serious dep we didn't see coming.** It's a
   memory-mapped open-addressing hash table that backs rustc's
   crate metadata index. Used by `rustc_hir` AND `rustc_metadata`.
   The "drop incremental" decision (§1.3) doesn't help here — `odht`
   is in the non-incremental metadata path too. The mmap-shim has
   to be solid for `odht` to work. **Important downstream
   constraint on the memmap2 STUB.**

6. **`blake3` + `sha1` + `sha2` + `md-5` are all used by `rustc_span`**
   for source-file content hashing (`compiler/rustc_span/src/lib.rs:87
   :92 :93 :1748`). Four hash crates. Reasonable mitigation is to
   pick **one** (blake3 — fastest and modern, has no_std support) and
   stub the other three to redirect to blake3 + tag the algorithm
   correctly in the metadata. Saves ~3 sessions vs. porting all four.

7. **`schemars` is used by `rustc_target` for JSON-schema generation**
   of the target spec. SemOS only ever produces one target spec
   (x86_64-unknown-none per §1.6), so we can hard-delete the schemars
   integration and emit a baked-in JSON blob. ~0.5 sessions, but
   removes a dep that otherwise would be a 1-session patch.

8. **`rustc` (the binary crate at `compiler/rustc/`) has a
   `rustc_public` + `rustc_public_bridge` dep that's there ONLY to
   ensure they end up in the sysroot for external consumers.** We
   don't need to ship a sysroot; we should drop those two deps from
   the `rustc-main` crate and not port them. The pair carries
   `scoped-tls` + `serde` deps that flow into the runtime graph
   otherwise.

9. **The host-only proc-macro crates (`syn` etc.) need a no_std-
   capable HOST toolchain** if we ever want self-hosting (the SemOS
   build of rustc compiles its own host crates). For v1 we accept
   cross-compilation only; this is a Phase 5+ concern.

10. **No surprises on `rustc_apfloat`** — it's already published to
    crates.io as a no_std + alloc crate by design (rustc upstreamed it
    explicitly). Listed as OK; expected ~0 sessions.

---

## Summary

**Total external crates in the rustc tree (deduped, all dep tables
across all `compiler/rustc_*/Cargo.toml`):** 71.

**After §1 decisions + dev-dep removal: ~50–55 in the runtime port
surface.**

**Walls (after applying mitigations): 0 fundamentally unmitigated.**
Every WALL crate in §2 has a documented stub-or-cfg-out path. The
Phase 1 stop condition is met.

**Reusable from Cranelift port: 14 crates** (every crate currently in
`user-programs/semos-cc/vendor/`). Notable version-drift items
needing 0.5–1 session each: `hashbrown` (0.15/0.17 → 0.16 spec),
`object` (0.36 → 0.37).

**Top 5 PATCH efforts (sessions):** `polonius-engine` (3) ·
`fluent-bundle` (3) · `regex` (3, but probably drop entirely) ·
`elsa` (2) · `tracing` + `tracing-core` (2 each, ~4 total).

**Suggested addition to §1 decisions:** a 1.7 — "drop localization;
hardcode English error templates." This kills 7 crates
(`fluent-bundle`, `fluent-syntax`, `intl-memoizer`, `unic-langid`,
`icu_list`, `icu_locale`, `icu_provider`, `zerovec`) at the cost of
international error messages — almost certainly the right trade for
v1.
