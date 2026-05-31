# M27 rustc port — canonical recipe

This is the single source of truth for "how do you port a rustc_* crate
to SemOS." Reads ~10 minutes. Hand to every Phase 2/3/4 agent at spawn.

Authority order if anything conflicts:
1. This file.
2. `docs/M27_RUSTC_PORT_PLAN.md` §1 (the nine decisions).
3. The predecessor agent's `// M27 …` markers and notes (if a
   predecessor handed you partial work).
4. `user-programs/semos-cc/PORT_LOG.md` Lessons-Learned (the original
   Cranelift port recipe — this RECIPE is its M27 successor and
   supersedes any Cranelift-specific specifics).

---

## 0. Read-before-you-write checklist (5 min)

- `docs/M27_RUSTC_PORT_PLAN.md` §1 — the nine decisions (drop LLVM,
  static cg_clif, drop incremental, drop rayon, drop proc-macros v1,
  single target, cg_clif owns ET_EXEC emission, drop i18n, FatalError
  → abort).
- `docs/m27-recon/SYNTHESIS.md` — the recon's tallies & the additions
  to §1 that were folded in.
- `docs/m27-port/EXPERIMENT_LOG.md` — the running diary. The
  Lessons-Learned tally at the bottom of each section is what to
  internalize.
- `docs/m27-port/2a/` — every prior agent's notes. Read at least one
  recent A-notes file to see the shape of the deliverable.
- `user-programs/std-shim/src/lib.rs` — what semos-std exposes today
  (it grows session-over-session as we discover gaps).

If you do nothing else, read the PLAN §1 + the experiment log's tail
section. Those two cover ~80% of the load-bearing context.

---

## 1. The recipe — apply to every crate

### 1.1 Cargo.toml
Add a `[workspace] members = []` block above `[package]`:

```toml
[workspace]
members = []

[package]
...
```

Reason: cargo's "fresh worktrees with no [workspace] inherit parent
workspace" trap. `members = []` opts out cleanly without forcing
dev-dep resolution.

**Skip:** `.cargo-checksum.json` updates. The `compiler/rustc_*`
crates are raw source, not crates.io vendor checkouts; no checksum
file exists.

### 1.2 src/lib.rs

Add in this exact order **after the leading `//!` inner doc comments,
before any items:**

```rust
#![no_std]

#[macro_use]
extern crate alloc;
```

The `#[macro_use]` is what makes `vec![]` reachable in submodules. If
the crate already declares `#![no_std]`, skip; if `extern crate alloc;`
is already there without `#[macro_use]`, add the attribute.

**Trap:** Rust inner attributes (`#![...]`) must appear before any
items. Doc comments at the file top are OK; an `extern crate` is an
item, so putting `extern crate alloc;` ahead of `#![no_std]` is a
syntax error. Order: `//!` doc comments → `#![…]` attributes →
`extern crate` items → `use` → rest.

### 1.3 All `.rs` files — std::* path substitution

Use the same table from `semos-cc/PORT_LOG.md` patch #11. In order:

```text
std::sync::Arc                 → alloc::sync::Arc
std::sync::Weak                → alloc::sync::Weak
std::collections::HashMap      → hashbrown::HashMap
std::collections::HashSet      → hashbrown::HashSet
std::collections::hash_map     → hashbrown::hash_map
std::collections::hash_set     → hashbrown::hash_set
std::borrow::Cow               → alloc::borrow::Cow
std::borrow::ToOwned           → alloc::borrow::ToOwned
std::boxed::Box                → alloc::boxed::Box
std::collections::BinaryHeap   → alloc::collections::BinaryHeap
std::collections::BTreeMap     → alloc::collections::BTreeMap
std::collections::BTreeSet     → alloc::collections::BTreeSet
std::collections::VecDeque     → alloc::collections::VecDeque
std::rc::*                     → alloc::rc::*
std::string::*                 → alloc::string::*
std::vec::*                    → alloc::vec::*
std::error::Error              → core::error::Error  (stable since 1.81)
std::*                         → core::*             (everything else)
```

Per-file: pick up `use std::…` lines first, then `std::…` in expression
positions. A Python or sed sweep is fine; eyeball each substitution
for false positives (e.g. macro emit strings — see 1.4).

### 1.4 Macros that emit `::std::*` tokens

Some crate-internal macros emit `::std::*` paths in their generated
code (declare_arena!, sometimes type-bound emits). Search for `::std`
in macro bodies and rewrite to `::core::*` / `::alloc::*`. A3 caught
this in `rustc_arena/src/lib.rs`'s declare_arena! macro.

### 1.5 Crates with FS / IO / process surface — host vs target body split

If the crate has bodies that genuinely need std (e.g.,
`rustc_fs_util`, `rustc_log`), use the `cfg(target_os = "none")`
pattern A3 introduced:

```rust
#[cfg(not(target_os = "none"))]
mod host_impl { /* original std-using body */ }

#[cfg(target_os = "none")]
mod semos_impl {
    // SemOS-target body — either uses semos_std equivalents or stubs
    // with io::Error(Unsupported) returns + // M27 markers.
}
```

This preserves the host build (useful for tooling like rustdoc) while
giving SemOS a working surface. The MARK pattern in A3's
rustc_fs_util notes is the canonical example.

### 1.6 R4 marker comments — when to leave them vs substitute directly

The recon flagged five class-blockers (R4 B1–B5). semos-std now has
shims for B1 (abort), B2 (thread_local + scoped_thread_local), B5
(OsString/OsStr); so when you encounter:

- **B1 (FatalError)** → REWRITE call site per §1.9: `raise()` →
  `process::abort()` or `process::abort_with_code(101)`;
  `catch_fatal_errors(f)` → `Ok(f())`. See A2's `fatal_error.rs`
  rewrite for the canonical shape.
- **B2 (scoped_tls)** → KEEP `scoped_thread_local!(…)` and
  `scoped_thread_local::*` imports AS IS; replace `scoped_tls` crate
  imports with `semos_std::scoped_thread_local!`. The shim exposes the
  same shape.
- **B5 (PathBuf/OsString)** → for **basic** uses (push, join, simple
  comparisons), substitute to `semos_std::path::*` and
  `semos_std::ffi::OsString` directly. For **advanced** uses (Cow<Path>,
  path.components(), path::Component::Normal, strip_prefix), LEAVE A
  MARKER `// M27 R4 B5 TODO(Phase 2b): needs semos_std::path
  extension for <api>` and don't try to substitute. A2-followup
  surfaced this in source_map.rs's FilePathMapping.

Other markers to leave (parent integrates later):
- `// M27 R3:` for hash-crate consolidation candidates where the choice
  is ABI-visible (e.g., SourceFileHashAlgorithm enum). Phase 4 owns
  the call.
- `// M27 §1.3:` for incremental-compilation paths cfg'd out per the
  decision.

### 1.7 Proc-macro crates — config only, no source touches

Proc-macros run on the HOST at build time with full std. Don't add
`#![no_std]` to them. Required treatment:

```toml
# In the crate root .cargo/config.toml — create if missing:
[build]
target = "x86_64-pc-windows-msvc"
```

And add `[workspace] members = []` to Cargo.toml. No source changes
unless something explicitly fights the proc-macro role.

A6 confirmed this pattern for `rustc_macros`, `rustc_index_macros`,
`rustc_type_ir_macros`, `rustc_fluent_macro`. Zero source LOC patched
for all four.

---

## 2. semos-std surface (current as of 2026-05-31)

Stuff you can use without leaving a marker:

| Category | API | Module path |
|----------|-----|-------------|
| Sync | `OnceLock<T>` (futex-backed), `Mutex<T>`, `Once`, `Condvar`, `RwLock<T>`, `Arc<T>` | `semos_std::sync` |
| Thread | `LocalKey<T>` + `thread_local!` macro (single-threaded variant) **plus 1.73 sugar for `LocalKey<Cell<T>>::{get,set,take,replace}` + `LocalKey<RefCell<T>>::{with_borrow,with_borrow_mut}`**, `ScopedKey<T>` + `scoped_thread_local!` macro, `spawn`, `JoinHandle<T>`, `sleep_ticks`, `sleep_ms` | `semos_std::thread` |
| Process | `exit(i32)`, `abort()`, `abort_with_code(i32)`, `Command`, `Child`, `ExitStatus` | `semos_std::process` |
| FFI | `OsString` (= `String`), `OsStr` (= `str`), `OsStrExt::to_os_string` | `semos_std::ffi` |
| Env | `args()`, `var(key)`, `var_os(key)`, `vars()`, `vars_os()`, `set_var`, `current_dir_string` | `semos_std::env` |
| Path | `Path::new`, `Path::parent`, `Path::file_name`, `Path::extension`, `Path::file_stem`, `Path::join`, `Path::canonicalize_lexical()`, `Path::components()`, `Component`, `Path::strip_prefix()`, `Path::as_os_str()`, `Cow<Path>`, `PathBuf` (push/pop/extension), basic AsRef/PartialEq | `semos_std::path` |
| FS | `File` (Read+Write+Drop), `OpenOptions`, `read`, `read_to_string`, `write`, `create_dir`, `remove_file` | `semos_std::fs` |
| IO | `Read`/`Write` traits, `Stdout` + `stdout()`, `Stderr` + `stderr()` (shared SYS_WRITE sink) | `semos_std::io` |
| Net | TcpStream, address types | `semos_std::net` |
| Collections | re-exports of alloc + hashbrown | `semos_std::collections` |
| Time | basic Duration, sleep | `semos_std::time` |

**Known gaps** (leave markers, don't try to substitute):

- `semos_std::path::Path::canonicalize` that hits the FS — only the
  lexical variant exists. Most rustc sites we've seen are lexical;
  flag the rest.
- `semos_std::io::ErrorKind` and `io::Error::other(msg)` not yet
  exposed. Subset is provided as anonymous `io::Error::other()`.
- `tracing` ecosystem (rustc_log uses it) — not vendored. Stub with
  no-op surface; restore when the tracing port lands.

---

## 3. Recipe for handing partial work to a followup

If your context budget is going to run out mid-crate:

1. **Stop at file boundaries**, not mid-file. Easier for the followup
   to pick up.
2. **Write line-precise per-file recipes in your notes**. Don't say
   "patch the rest"; say "lines 1144–1287 in source_map.rs use
   PathBuf in `FilePathMapping`; treat each site with `// M27 R4 B5
   TODO(Phase 2b)`."
3. **Estimate the remaining token cost** so the parent can pick the
   right follow-up agent (small budget for trivials, big for novel).
4. **Flag any new architectural surprise** (a new R5, a missing
   semos-std API). The followup may rediscover it independently; you
   save them the time.

A2 → A2-followup proved this is worth **~10×** token efficiency:
- A2 (novel work, full file rewrites, hit context budget): 120 t/LOC.
- A2-followup (recipe-following): **14 t/LOC** on the same crate.

The recipe-per-file in the predecessor's notes IS the load-bearing
asset.

---

## 4. Standard agent deliverable

1. **Patched sources** at the canonical main-tree path:
   `F:\Software\ArmKernel3\user-programs\semos-rustc\vendor-rustc-src\compiler\<crate>\`
2. **Notes** at `docs/m27-port/<phase>/<agent-id>-<scope>.md` — use the
   template at `docs/m27-port/HANDOFF_TEMPLATE.md`.
3. **One-paragraph completion summary** returned to the parent. Cover:
   files patched, distinct std patterns hit, recipe extensions added,
   blockers raised, token/tool-use/duration self-report.

Do **NOT**:
- Run `cargo build`. Patch-only contract.
- Modify other agents' crates. Your worktree may share main's working
  dir; isolation is by assignment only.
- Attempt `git merge`, `git checkout`, `git restore`, `git pull`. The
  sandbox denies these. Use `git show main:<path>` (read-only) +
  Write tool.

---

## 5. Token / LOC expectation per agent

Based on Phase 2a evidence (n=7):

| Class | Tokens/LOC | When |
|-------|------:|------|
| Recipe-following (predecessor docs exist) | 14 | A2-followup |
| Standard small mechanical | 30–35 | A4, A5 |
| Standard medium with one architectural decision | 80–100 | A3 |
| Novel hard crate (multiple architectural decisions) | 120 | A2 |
| Proc-macro / config-only | n/a (no source) | A6 |
| Single-crate probe | 480 | probe-rustc_hashes |

Plan for ~50 t/LOC average outside the probe; budget novel crates at
~120.

---

## 6. The lessons-learned tally (also at the bottom of EXPERIMENT_LOG)

1. Sequence recon: R1 first single, then R2/R3/R4 parallel.
2. Session token bucket is shared across parallel agents. Probe before
   N-agent waves when bucket state is unknown.
3. Worktrees branch from session-start parent state, not current main.
   Treat worktree-vs-main path resolution as agent-dependent; use
   `git show main:<path>` for reads.
4. `.cargo-checksum.json` is for vendored crates only; raw rustc-src
   has none.
5. Probe-then-fleet for quota recovery.
6. Predecessor recipes deliver ~10× followup efficiency. Always.
7. Per-agent sandbox permissions are not guaranteed. `git merge` works
   for one agent and not another. Parent owns all git plumbing.
8. Worktree CWD persists across Bash calls — prefix every git op with
   `cd /f/Software/ArmKernel3` or `git -C` explicitly.

Read the EXPERIMENT_LOG for the full incident-by-incident derivation.
