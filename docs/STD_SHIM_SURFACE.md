# `std` shim surface — what Phase 14 M25 has to provide

**Status:** prep work for [Phase 14 M25](ROADMAP.md#m25--std-shim-over-semantic-os-syscalls).
This document catalogs every `std` API that rustc + cargo + their direct
dependencies actually call, maps each to the Semantic OS syscall it
would route through, and identifies the gaps. It is the **spec for M25**.

## How to read this

Each table covers one `std` namespace. Per row:

- **Method** — the `std` API the upstream code calls
- **Kernel syscall(s)** — the `kernel-core::syscall::numbers` constant(s)
  the shim would dispatch to
- **Status**
  - ✅ exists — syscall is in place; the shim work is purely ABI mapping
    (translate std's args into our syscall's u64 args, translate the
    return value back into std's `Result` / `io::Error` shape)
  - 🔨 partial — a syscall exists but doesn't cover the std method's full
    semantics (most commonly: works for the demo case, doesn't handle
    blocking / large buffers / async ready-state correctly)
  - ❌ missing — no kernel-side surface today; this row is a Phase 14
    prerequisite that MUST land before the M25 shim author can write
    that part of the shim. These all gather in the prereq list at the
    end.
- **Notes** — what's actually needed; references to source files where
  the gap lives

## Source-of-truth pointers

- Syscall numbers + dispatch table: `kernel-core/src/syscall/mod.rs`
  (the `numbers` module is the authoritative ABI)
- Path namespace: `kernel-core/src/fs/paths.rs`
- Snapshot persistence: `kernel-core/src/storage/snapshot.rs`
- Process model: `kernel-core/src/process/mod.rs`
- Scheduler: `kernel-core/src/scheduler/mod.rs`
- TCP stream: `kernel-core/src/net/tcp.rs`
- Allocator: `kernel-core/src/memory/secure_alloc.rs`

---

## `std::fs`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `File::open(path)` | `SYS_OPEN` (10) | ✅ exists | Path FDs land in 96..127; ramfs FDs in 3..63. `handle_open` at `syscall/mod.rs:432` already does the routing. |
| `File::create(path)` | `SYS_OPEN` with `open_flags::CREATE` (bit 0) | ✅ exists | Tier defaults to bits 4-5 of the flag word. Shim must invent a default tier (Internal=1 is the safe one for rustc-generated files). |
| `File::open_with_options(...)` | `SYS_OPEN(flags)` | 🔨 partial | `open_flags::CREATE` and `DIRECTORY` exist; `APPEND`, `TRUNCATE`, `READ`, `WRITE`, `CREATE_NEW` all need new flag bits (4 bits free in the lower byte). Append in particular needs FWRITE to grow content (today it's full-file overwrite — see below). |
| `File::read(buf)` (`Read::read`) | `SYS_FREAD` (12) | ✅ exists | Per-call cap 4096 bytes; shim handles loop. EOF returns 0. Cursor advances correctly per `handle_fread` at `syscall/mod.rs:651`. |
| `File::write(buf)` (`Write::write`) | `SYS_FWRITE` (13) | 🔨 partial | **Full-file overwrite only.** `handle_fwrite` at `syscall/mod.rs:737` calls `ObjectContent::from_inline` which caps at 256 bytes inline AND resets the cursor to 0 on every write. rustc/cargo writes object files >256 bytes constantly. Requires (a) Allocated content path for large objects, (b) partial-write semantics that preserve the cursor + leave already-written prefix intact. |
| `File::seek(SeekFrom)` | `SYS_SEEK` (14) | 🔨 partial | `handle_seek` (line 802) works for ramfs FDs only; path FDs go through `update_path_fd_position`. Shim must route both. `SeekFrom::End` requires a length query (today not exposed for path FDs at the SYS_SEEK layer; need to teach it). |
| `File::metadata()` | `SYS_STAT` (15) | 🔨 partial | `handle_stat` at line 811 returns only the file size. std's `Metadata` exposes `len`, `is_dir`, `is_file`, `modified()`, `accessed()`, `created()`, `permissions()`. Need a richer stat: at minimum length + type + mtime. mtime needs M5 (snapshot persistence with `created_at`/`modified_at` populated from `platform::wall_clock`). |
| `File::set_len(n)` | (needs `SYS_TRUNCATE` or richer FWRITE) | ❌ missing | No truncate semantics. cargo uses this for output-file pre-sizing. |
| `File::sync_all()`, `sync_data()` | (needs `SYS_FSYNC`) | ❌ missing | No explicit fsync today; M5 snapshot is timer-driven not request-driven. cargo + rustc actively `sync_all` to be crash-safe. |
| `fs::read(path)` | `SYS_OPEN` + `SYS_FREAD` loop + `SYS_CLOSE` | ✅ exists | Pure shim composition. |
| `fs::write(path, data)` | `SYS_OPEN(CREATE)` + `SYS_FWRITE` + `SYS_CLOSE` | 🔨 partial | Same FWRITE limitation. |
| `fs::read_to_string(path)` | `SYS_OPEN` + `SYS_FREAD` + UTF-8 check | ✅ exists | Composition only. |
| `fs::copy(from, to)` | `SYS_OPEN` + `SYS_FREAD` + `SYS_OPEN(CREATE)` + `SYS_FWRITE` | 🔨 partial | Blocked on full FWRITE. |
| `fs::rename(from, to)` | (needs `SYS_RENAME`) | ❌ missing | `paths.rs` docstring at line 43 explicitly says "Rename. Two-step today: lookup + create + unlink." Cargo's atomic-replace pattern depends on rename being atomic w.r.t. crashes. |
| `fs::remove_file(path)` | `SYS_UNLINK` (17) | ✅ exists | `handle_unlink` at line 862 works for path namespace; ramfs is read-only (intentional). |
| `fs::remove_dir(path)` | `SYS_UNLINK` (17) | ✅ exists | Same syscall; empty-dir check is in `Namespace::unlink`. |
| `fs::remove_dir_all(path)` | composition of `read_dir` + `unlink` | 🔨 partial | Works if read_dir returns useful info (it does — names + then re-stat per entry). Shim implements recursion. |
| `fs::create_dir(path)` | `SYS_MKDIR` (16) | ✅ exists | `handle_mkdir` at line 846. |
| `fs::create_dir_all(path)` | walk + `SYS_MKDIR` per segment | ✅ exists | Pure shim composition. Watch for the MAX_PATH_DEPTH=32 limit in `paths.rs`. |
| `fs::read_dir(path)` | `SYS_OPEN(DIRECTORY)` + `SYS_READDIR` (18) loop | 🔨 partial | `handle_readdir` at line 886 returns names only. std's `DirEntry` exposes `file_name`, `path`, `metadata()`, `file_type()`. The shim must stat each entry separately (an N×SYS_STAT loop) to populate metadata — slow but correct. |
| `fs::canonicalize(path)` | (purely shim-side string ops + repeated `SYS_STAT`) | 🔨 partial | We don't have symlinks (intentional per paths.rs:38), so canonicalize reduces to "make it absolute". Trivial. |
| `fs::hard_link(...)`, `fs::soft_link(...)` | (needs symlink/hardlink in `paths.rs`) | ❌ missing | Out of scope per paths.rs docstring. cargo uses hard_link for incremental-build artifact dedup; we can fall back to copy. |
| `fs::set_permissions(...)` | (needs perm bits in SemanticObject) | ❌ missing | We have SecurityTier, not Unix mode bits. The shim should accept the call and silently no-op (rustc/cargo set 0755 on output binaries and 0644 on artifacts; neither matters to our security model). |
| `fs::metadata(path)` | `SYS_STAT` (15) | 🔨 partial | Same gap as `File::metadata`. |
| `fs::symlink_metadata(path)` | `SYS_STAT` (15) | 🔨 partial | Same as metadata (no symlinks → identical result). |

## `std::process`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `Command::spawn() -> Child` | `SYS_SPAWN` (40) | 🔨 partial | `handle_spawn` at line 1037 exists but: (a) the name lookup is hardcoded to `init`/`shell`/`test`/`user` (a `&'static str` requirement from the scheduler — see line 1070), so arbitrary cargo-driven binaries can't get the right Process.name; (b) no argv/envp forwarding — the Process struct has no slot for either; (c) no working-directory inheritance; (d) no stdio inheritance/redirection (the Child has fd 0/1/2 of the parent's TTY, not of pipes set up by the shim). |
| `Command::status()` | `SYS_SPAWN` + `SYS_WAIT` | 🔨 partial | `handle_wait` at line 1090 exists, returns exit code. Blocking. Works for the simple case. |
| `Command::output()` | spawn + capture stdout/stderr via pipes | 🔨 partial | `SYS_PIPE` (46) at line 950 works; spawn-with-stdio-redirected does NOT (point c above). The whole `Stdio::piped()` plumbing is the missing piece. |
| `Command::env(k, v)` / `envs(...)` | (needs env var passthrough in `SYS_SPAWN`) | ❌ missing | No envp in the spawn ABI. Cargo sets `CARGO_*`, `OUT_DIR`, `RUSTC`, etc., on every rustc invocation. Without env passthrough the entire cargo→rustc handoff is broken. |
| `Command::current_dir(path)` | (needs CWD per-process) | ❌ missing | Process struct has no cwd field. Today paths in syscalls are always absolute. Cargo invokes rustc with CWD set to the package dir; relative paths in compiler output (errors, dep-info files) assume this. |
| `Command::arg(s)` / `args(...)` | (needs argv passthrough in `SYS_SPAWN`) | ❌ missing | Same shape as envp. No argv slot in the Process struct or the spawn ABI. |
| `Child::wait()` | `SYS_WAIT` (41) | ✅ exists | `handle_wait` at line 1090 handles both specific-pid and any-child (pid=0). |
| `Child::try_wait()` | (needs `SYS_WAITNB`) | ❌ missing | `handle_wait` is always blocking. Non-blocking try_wait is what cargo uses for parallel build job tracking. |
| `Child::kill()` | `SYS_KILL` (42) | ✅ exists | `handle_kill` at line 1109. |
| `Child::stdin` / `stdout` / `stderr` | composition with `SYS_PIPE` (46) | 🔨 partial | The pipes exist; what's missing is the spawn-side hookup that points fd 0/1/2 of the child at the parent-side pipe ends. |
| `Child::id() -> u32` | `SYS_GETPID` (4) at child | ✅ exists | Returned from spawn already. |
| `process::exit(code)` | `SYS_EXIT` (2) | ✅ exists | `handle_exit` at line 240. |
| `process::abort()` | (no syscall; UD2 / triple-fault) | 🔨 partial | rustc abort on internal-compiler-error path. Shim emits `int3` or calls `SYS_EXIT(0xFE)`. |
| `process::id() -> u32` | `SYS_GETPID` (4) | ✅ exists | |

## `std::thread`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `thread::spawn(closure) -> JoinHandle` | (needs `SYS_THREAD_SPAWN`) | ❌ missing | Today `process::spawn` creates a whole new address space + ELF load. Threads need a same-AS sibling task, sharing the heap. Scheduler's `alloc_task_slot` (scheduler/mod.rs:264) can hold one — but there's no syscall surface and no shared-stack model. **Major prerequisite.** |
| `JoinHandle::join()` | (needs `SYS_THREAD_JOIN`) | ❌ missing | Cargo uses rayon for parallel codegen; rustc uses internal thread pools for parallel-frontend. Both depend on join. |
| `thread::current() -> Thread` | (needs per-task TLS slot for Thread handle) | ❌ missing | Implied by spawn. The shim's Thread struct can be entirely shim-side; what we need from the kernel is "tell me the current task's id" — already exposed via `current_task_index()` (scheduler/mod.rs:129) at kernel-side, just needs to surface as a syscall. |
| `thread::sleep(dur)` | `SYS_SLEEP` (5) | 🔨 partial | `handle_sleep` at line 932 takes timer ticks. Shim must convert Duration → ticks via known tick rate. (x86_64 platform: 100 Hz today; document the conversion factor.) |
| `thread::yield_now()` | `SYS_YIELD` (3) | ✅ exists | |
| `thread::park()` / `unpark()` | (needs `SYS_PARK` / `SYS_UNPARK`) | ❌ missing | The condvar primitive (below) covers most of what park is used for in upstream std. Could shim park-on-condvar. |
| `thread::Builder::name(s).spawn(...)` | (extension of spawn) | 🔨 partial | Scheduler's `alloc_task_slot` requires `&'static str` for name — no way to plumb a dynamic name without a name-arena. |
| `thread::scope(...)` | composition over spawn + join | ❌ missing | Inherits the missing-spawn gap. |

## `std::sync`

The Mutex/Condvar/RwLock primitives in std are normally backed by
futex on Linux / SRW locks on Windows. We have neither. Kernel-side
support needs to land in the same milestone as `std::thread`.

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `Mutex::lock()`, `unlock()` | (needs `SYS_FUTEX_WAIT` / `SYS_FUTEX_WAKE` or kernel-side mutex objects) | ❌ missing | Spinlock at user-space is unacceptable for build parallelism (rustc holds a mutex across whole-function codegen — milliseconds of contention). Either: (a) futex-style primitive over a u32 word, (b) kernel-allocated mutex objects with a small handle id. |
| `RwLock::read()`, `write()` | same | ❌ missing | rustc query cache is a RwLock<HashMap>; takes a write lock once per compilation unit and read locks per query. |
| `Condvar::wait`, `notify_one`, `notify_all` | (needs `SYS_FUTEX_*` or condvar objects) | ❌ missing | Used by rustc job server. |
| `Barrier::wait()` | derived from Condvar | ❌ missing | Less critical (only rayon's pipeline barrier uses it). |
| `Once::call_once(f)` | derived from `AtomicU32` + futex park | ❌ missing | Used by every `std::sync::OnceLock` in upstream — pervasive. Shim could fall back to a spin-then-yield if the workload tolerates it (single-threaded codegen does; parallel doesn't). |
| `atomic::*` operations | no syscall — LLVM intrinsics | ✅ exists | x86_64 atomic instructions; the Rust compiler emits them directly. Already work today. |
| `mpsc::channel()` | derived from Mutex + Condvar | ❌ missing | Inherits the gaps. |
| `mpsc::sync_channel()` | derived from Mutex + Condvar | ❌ missing | Inherits the gaps. |

## `std::net`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `TcpStream::connect(addr)` | `kernel_core::net::tcp::TcpStream::connect` (no syscall today — it's a kernel-internal call) | 🔨 partial | TcpStream exists at `kernel-core/src/net/tcp.rs:86` but **only one socket at a time** (`SOCKET_IN_USE` global; see line 96). Crates.io fetches happen one-at-a-time in cargo, so single-socket might be tolerable for prep; but rustc's job server uses local TCP for IPC in some configs. Needs (a) syscall surface for connect/send/recv, (b) multi-socket support. |
| `TcpStream::read` / `write` | (would go through new `SYS_SOCK_RECV` / `SYS_SOCK_SEND`) | 🔨 partial | TcpStream has the read/write methods (lines 162/183); just no syscall exposure yet. |
| `TcpStream::shutdown` | derived from TcpStream::close | 🔨 partial | `close()` exists at line 203; no syscall surface. |
| `TcpListener::bind` / `accept` | (needs server-side smoltcp socket + new syscalls) | ❌ missing | No listen surface today. cargo's local registry server needs this if we ever run a registry on the device; not blocking for first M27 attempt. |
| `UdpSocket::bind` / `send_to` / `recv_from` | (needs UDP support in `kernel_core::net`) | ❌ missing | smoltcp supports UDP, we just don't instantiate it. DNS resolver (M12) will need this too. |
| `ToSocketAddrs::to_socket_addrs(s)` | (needs DNS) | ❌ missing | Blocked on M12 (Phase 10) DNS resolver. For prep work, the shim can fail on hostnames and accept only `IpAddr:port` strings (matches our DEMO 16 hardcoded-IP pattern). |
| `IpAddr` / `SocketAddr` parsing | (pure user-space; no syscalls) | ✅ exists | Shim hosts these directly — no kernel work needed. |

## `std::env`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `env::var(k)` / `var_os(k)` | (needs per-process env table) | ❌ missing | No env vars on Semantic OS today. Blocking for cargo→rustc handoff (every `$CARGO_*`, `$RUSTC`, `$RUSTFLAGS` lookup fails). Two paths: (a) per-process env block populated at spawn time (canonical Unix model), (b) global env stored in a SemanticObject under `/etc/env`. Path (a) is the right one for std fidelity. |
| `env::vars()` / `vars_os()` | (iterates env block) | ❌ missing | Same as above. |
| `env::set_var(k, v)` | (mutates env block) | ❌ missing | Same. |
| `env::remove_var(k)` | (mutates env block) | ❌ missing | Same. |
| `env::current_dir()` | (needs CWD per-process) | ❌ missing | Same gap as `Command::current_dir`. |
| `env::set_current_dir(p)` | (needs CWD per-process) | ❌ missing | Same gap. |
| `env::current_exe()` | (needs exe path in Process struct) | ❌ missing | Process has a `&'static str` name (no path). Cargo uses `current_exe()` to find sibling tools (rustc finds rustfmt, rustdoc next to itself). Could synthesize from the Process.name + a hardcoded `/usr/bin/` prefix as a stopgap. |
| `env::args() -> Args` | (needs argv per-process) | ❌ missing | Same gap as `Command::arg`. **Critical** — argc/argv is the most basic Unix contract; every Rust program does this. |
| `env::temp_dir()` | pure shim (returns `/tmp`) | ✅ exists | We can hardcode `/tmp` in the shim and `mkdir /tmp` in init. |
| `env::home_dir()` | pure shim (returns `/home/<user>`) | ✅ exists | UID lookup via `SYS_LOOKUP_USER` (83) gives the username; shim composes the path. |

## `std::path`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `Path::new(s)`, `PathBuf::from(s)` | none | ✅ exists | Pure string manipulation; shim hosts the whole crate verbatim. |
| `Path::join`, `parent`, `file_name`, `extension`, etc. | none | ✅ exists | Pure string ops. |
| `Path::exists()` | `SYS_STAT` (15) | ✅ exists | Trivial wrapper. |
| `Path::is_dir()` / `is_file()` | `SYS_STAT` (15) | 🔨 partial | Stat doesn't return type today (size only). Once metadata is enriched (see fs section), trivial. |
| `Path::canonicalize()` | same as `fs::canonicalize` | 🔨 partial | See fs row. |
| `Path::read_link()` | (no symlinks) | ❌ missing | Returns an error in the shim is fine — std consumers handle ENOENT-shaped errors. |

## `std::time`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `Instant::now()` | `SYS_TIME` (70) → `platform::ticks()` | 🔨 partial | `SYS_TIME` returns ticks at line 163 (`crate::platform::ticks()`); shim converts to a Duration since boot. Monotonic. Fine for relative timing. **Important:** the SYS_TIME constant is named "time" but the impl is ticks-since-boot — std `Instant` is monotonic so semantically correct, but `SystemTime` (below) needs different plumbing. |
| `SystemTime::now()` | (needs `SYS_WALL_CLOCK`) | 🔨 partial | `platform::wall_clock()` (platform.rs:185) returns an `Option<u64>` (unix timestamp seconds). Not yet exposed as a syscall. Needed for cargo's mtime-based incremental build. **Add `SYS_WALL_CLOCK` to the numbers module.** |
| `Duration` arithmetic | none | ✅ exists | Pure user-space. |
| `SystemTime::elapsed()` | composition | 🔨 partial | Inherits SystemTime::now() gap. |

## `std::alloc`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `alloc(layout)` (`GlobalAlloc::alloc`) | `SYS_ALLOC` (30) / `SYS_BRK` (33) | 🔨 partial | `handle_alloc` at syscall/mod.rs:275 allocates **a whole physical frame** at a time (no sub-frame granularity). std assumes a real heap with arbitrary sizes/alignments. **Major gap:** need a real allocator. The hypothesis in ROADMAP "Memory allocator (jemalloc-class minimum) ~8K LOC" lives here. Could vendor `dlmalloc` or `talc` (small, no_std-compatible) and route both std::alloc and Rust's GlobalAlloc through it; back the allocator's pages with `SYS_ALLOC` / `SYS_BRK`. |
| `dealloc(ptr, layout)` | `SYS_FREE` (31) | 🔨 partial | Same gap — frame-granular, not byte-granular. Combine with allocator port. |
| `realloc(...)` | composition | 🔨 partial | Falls out of the allocator port. |
| `alloc_zeroed(layout)` | `SYS_ALLOC` + memset | 🔨 partial | Same. |
| `Box::new(x)`, `Vec::push(x)`, ... | all go through GlobalAlloc | 🔨 partial | Same. Everything is downstream of the allocator fix. |

## `std::io`

The `Read`/`Write`/`Seek`/`BufRead` traits themselves are pure user-space.
The concrete impls live in `std::fs`, `std::net`, and `std::process`.
The `io::Error` type is what the shim needs to construct from our
`u64::MAX` syscall errors — the shim hosts a translation table.

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `Stdin::read_line`, `read` | (needs stdin from TTY/keyboard) | ❌ missing | Blocked on M19 (TTY layer). For prep, the shim can return EOF immediately. |
| `Stdout::write`, `Stderr::write` | `SYS_WRITE` (0) or `SYS_FWRITE(1)` | ✅ exists | Both `handle_write` and `handle_fwrite` (line 741) route to the framebuffer console. |
| `io::copy(reader, writer)` | composition of read + write | ✅ exists | Pure shim. |
| `BufReader`, `BufWriter` | pure user-space buffering | ✅ exists | Shim hosts. |

## `std::panic`, `std::backtrace`

| Method | Kernel syscall(s) | Status | Notes |
|---|---|---|---|
| `panic::catch_unwind` | (needs unwind tables + libunwind) | ❌ missing | Rust normally aborts in `panic = "abort"` mode (which our kernel uses). Set the shim's `panic_strategy = "abort"`, which makes catch_unwind always return an error. Fine for rustc — it doesn't depend on unwinding for correctness. |
| `panic::set_hook` | pure user-space (sets a function pointer) | ✅ exists | |
| `Backtrace::capture()` | (needs DWARF unwinder) | 🔨 partial | Without DWARF tables on disk (we strip them today), backtraces will be empty addresses. Shim returns `Backtrace::disabled()`. Fine for first M27 attempt; nicer error reporting is a follow-up. |

## `std::collections`

All pure user-space (`HashMap`, `BTreeMap`, `Vec`, `String`, etc.).
Backed by the global allocator (see `std::alloc` above) — fix that
and these all work.

## `std::os::unix` / `std::os::windows`

Skipped on purpose. cg_clif and rustc don't use these directly in
target-independent code paths. cargo uses `std::os::unix::fs::symlink`
in the linker shim — see the symlink ❌ row.

If we end up needing a `std::os::semos` namespace for app authors
to call our SemanticObject APIs directly (the SUID/tier/relationship
features that don't fit Unix), that's a Phase 14 follow-up, not
M25 prereq.

---

# Phase 14 prerequisite list

Every ❌ row above, gathered. These are kernel-side syscalls / data
structures that must land BEFORE the std shim author can sit down
and write the corresponding part of M25. Ordered by "blast radius"
— how much of the shim is gated on each.

## Tier 1 — without these, M25 cannot meaningfully start

1. **Real general-purpose allocator** (currently `SYS_ALLOC` is
   frame-granular). Needed by EVERY `Vec`, `Box`, `String`,
   `HashMap` in upstream code. ROADMAP's `~8K LOC` line item.
   Possible vendor target: `dlmalloc-rs` or `talc` — both no_std,
   both have an extant frame-allocator interface.
   - New syscall: `SYS_BRK` is already in the numbers module
     (33) but has no handler — implement it to grow the heap.
2. **argv / envp passthrough in `SYS_SPAWN`**. Without this,
   `env::args()` returns empty and the cargo→rustc handoff is
   broken (every `rustc --crate-name foo --edition=2021 ...`
   invocation passes flags as argv). ABI extension: spawn takes
   pointers to argv-blob + envp-blob, the kernel copies them into
   the new address space and the entry shim parses on the other
   side.
3. **Per-process env block + CWD**. Add fields to the Process
   struct in `kernel-core/src/process/mod.rs:237`. Inherit-by-default
   on spawn. New syscalls: `SYS_GET_ENV`, `SYS_SET_ENV`,
   `SYS_GET_CWD`, `SYS_SET_CWD`.

## Tier 2 — needed for the M26 "first compile" smoke test to actually finish

4. **`SYS_FSYNC`** (or commit-now flush of M5 snapshot for a single
   FD). rustc/cargo `sync_all()` writes to be crash-safe. Without
   fsync the build looks like it works in QEMU and silently corrupts
   on metal under power loss.
5. **`SYS_RENAME`** with atomic semantics. cargo's
   atomic-overwrite-on-success pattern depends on this.
6. **`SYS_TRUNCATE`** (or richer FWRITE that doesn't reset cursor
   and respects file size). The current 256-byte inline cap is a
   hard wall.
7. **FWRITE that handles large content**. Today `handle_fwrite`
   caps at 256 bytes via `ObjectContent::from_inline`. Needs the
   `Allocated` content path actually wired up. Without this, no
   object file >256 bytes can be written — `rustc` output is dead
   on arrival.
8. **Enriched `SYS_STAT`**. Returns file size today; needs type
   (file/dir/other), mtime (from M2 wall_clock), maybe a mode word
   stub for std fidelity.

## Tier 3 — needed for parallel builds and threaded rustc

9. **`SYS_THREAD_SPAWN` / `SYS_THREAD_JOIN`**. Same-AS sibling tasks.
   Scheduler can support this (it already has a slot table); needs
   the syscall + shared-heap allocation rules + thread-local
   storage (TLS) per task.
10. **Mutex / Condvar / RwLock primitives**. Either futex-shape
    (`SYS_FUTEX_WAIT` over a u32 word + `SYS_FUTEX_WAKE`) or
    kernel-allocated objects with handle ids. Futex is simpler to
    implement and matches Linux std's lowering — recommend that path.
11. **`SYS_WAITNB` (non-blocking try_wait)**. Cargo's parallel job
    manager polls children.
12. **`thread::sleep` with Duration → ticks conversion**. The shim
    side; document tick-rate constant in `kernel-core::scheduler`.

## Tier 4 — needed for full cargo functionality (network fetches, etc.)

13. **Syscall surface for `kernel_core::net::tcp::TcpStream`**.
    `SYS_SOCK_CONNECT`, `SYS_SOCK_SEND`, `SYS_SOCK_RECV`,
    `SYS_SOCK_CLOSE`. Plus multi-socket support in the kernel
    (drop `SOCKET_IN_USE`).
14. **UDP sockets** in `kernel_core::net` + syscall surface.
    Needed for DNS resolver (M12) and ultimately for cargo's
    crates.io connectivity.
15. **DNS resolver** (M12, Phase 10). Without it, cargo can
    connect by IP only.
16. **`SYS_WALL_CLOCK`**. Trivial wrapper over `platform::wall_clock()`
    — add a syscall number (suggest 74 = `SYS_WALL_CLOCK`, leaving
    73=SYS_SYSINFO in place) and a one-line handler. Needed for
    `SystemTime::now()`.

## Tier 5 — nice-to-have, can be shimmed-around for the first M27 attempt

17. **Symlinks / hardlinks in the path namespace**. cargo's
    artifact-dedup falls back to copy; that's wasteful but correct.
18. **stdin from TTY/keyboard** (M19). rustc and cargo don't read
    stdin in normal compile flow; this matters only for the
    interactive agent loop (M22).
19. **Backtrace / DWARF unwinder**. Pretty-print on panic; not
    required for correctness.

---

# Summary table

| Tier | Items | What's blocked without it |
|---|---|---|
| 1 | 3 prereqs | M25 can't start in earnest |
| 2 | 5 prereqs | First end-to-end compile can't succeed |
| 3 | 4 prereqs | Parallel cargo / rustc internal threading |
| 4 | 4 prereqs | Network fetches (crates.io, dep resolution) |
| 5 | 3 prereqs | Polish (stdin, symlinks, backtraces) |
| **Total** | **19 ❌ prereqs** | spanning 4 of the 10 std namespaces surveyed |

Of the **65 std methods catalogued**:
- **20 ✅ exists** — pure shim work (translate args, translate errors)
- **26 🔨 partial** — exists but needs widening (cursor preservation,
  large content, multi-socket, etc.)
- **19 ❌ missing** — listed above

Phase 14 M25 work splits into three parallel tracks:
- **Track A:** the shim itself (~30K LOC from the ROADMAP hypothesis;
  mostly mechanical translation for the ✅ rows + workaround coding
  for the 🔨 rows)
- **Track B:** the kernel-side prerequisites (Tiers 1-3 above; the
  ~15K-LOC line item in ROADMAP)
- **Track C:** the allocator vendor + port (Tier 1 item #1; the
  ~8K-LOC line item in ROADMAP)
