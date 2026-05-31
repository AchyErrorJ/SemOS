# B4 — rustc_data_structures sync submodule (parking_lot collapse)

**Date:** 2026-05-30
**Phase:** 2b
**Assigned crates / files:**
- `compiler/rustc_data_structures/src/sync.rs`
- `compiler/rustc_data_structures/src/sync/lock.rs`
- `compiler/rustc_data_structures/src/sync/freeze.rs`

**Status:** COMPLETE
**Token cost (self-report):** ~24k tokens / ~15 tool uses / ~1 session
**Source LOC patched:** ~95 lines added / gated (host-only fences + SemOS
shims). No upstream code deleted.

## 1. Per-file diff summary

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| `sync.rs` | ~70 added | Gated `pub use parking_lot::{Mapped*RwLockReadGuard, Mapped*RwLockWriteGuard, RwLockReadGuard, RwLockWriteGuard}` to `cfg(not(target_os = "none"))`; SemOS branch re-exports `core::cell::{Ref as ReadGuard, Ref as MappedReadGuard, RefMut as WriteGuard, RefMut as MappedWriteGuard}`. Split `pub struct RwLock<T>(parking_lot::RwLock<T>)` host vs SemOS — SemOS variant wraps `core::cell::RefCell<T>` and routes `read/write/try_write/borrow*/into_inner/get_mut` to `RefCell::borrow/borrow_mut/try_borrow/try_borrow_mut/into_inner/get_mut`. | `// M27 R4 B1 (parking_lot collapse)` at top of guard re-exports and again at top of split `RwLock<T>` |
| `sync/lock.rs` | ~30 added | Gated `use parking_lot::RawMutex` and `use parking_lot::lock_api::RawMutex as _` to host. Added `#[cfg(target_os = "none")] mod parking_lot_shim` exposing a unit `RawMutex` struct with const `INIT`, `try_lock` → `true`, `lock` → `()`, `unsafe unlock` → `()`. Patched `Lock::new` to always pick `Mode::NoSync` on `target_os = "none"`; host branch unchanged. Split `use crate::sync::{DynSend, DynSync, mode}` so `mode` is host-only (only used by host `Lock::new`). | `// M27 R4 B1 (parking_lot collapse)` at parking_lot import gate, in `parking_lot_shim`, and in `Lock::new` SemOS branch |
| `sync/freeze.rs` | 0 | No edits required. `FreezeLock` consumes `RwLock<()>` + `ReadGuard<'_, ()>` + `WriteGuard<'_, ()>` exclusively via re-exports from `crate::sync`, so the gating in `sync.rs` flows through transparently. Verified `Ref::map` / `RefMut::map` signatures match the (non-existent) callers' expectations. | none |

## 2. Decisions made (architectural)

- **Approach (b) — collapse Mode::Sync to Mode::NoSync on SemOS.** Per
  A1's explicit recommendation in
  `docs/m27-port/2a/A1-rustc_data_structures.md` "Pending work →
  parking_lot detachment". Approach (a) (vendor parking_lot's no_std
  RawMutex) would have required either pulling in parking_lot's
  thread-id semantics from std::thread or vendoring crossbeam-utils'
  no_std mode, both significantly larger surface area. Approach (b)
  is consistent with plan §1.4 (single-threaded rustc) and matches
  what A1 already did inside `sync/parallel.rs` (shim
  `parking_lot::Mutex` → `RefCell<T>` on SemOS).

- **`Mode` enum retained, not removed.** The `Mode::Sync` discriminant
  is preserved so the `Lock`'s `mode` field, `lock_assume(mode: Mode)`,
  and `LockGuard.mode` all keep the same public-ABI shape across host
  and SemOS. The `Mode::Sync` arm is statically unreachable on SemOS
  because `Lock::new` only ever returns `Mode::NoSync` there; the
  `match mode { Mode::Sync => ... }` arms in `try_lock`, `lock_assume`,
  and `LockGuard::drop` are dead code on SemOS but still type-check
  against the `parking_lot_shim::RawMutex` unit stub.

- **`parking_lot_shim` exposes a unit type with no-op methods rather
  than uninhabited.** A `!` / `Infallible` substitute would be tighter
  but would require touching the `match mode {}` arms (need
  `unsafe { core::hint::unreachable_unchecked() }` plumbing). The unit
  shim is a strictly smaller diff and the methods are unreachable in
  practice — pure type-system glue.

- **SemOS RwLock<T> backed by `core::cell::RefCell<T>` (not the
  imported `RefCell` from semos_std).** `core::cell` is the canonical
  no_std location and `RefCell::borrow`/`borrow_mut`/`try_borrow`/
  `try_borrow_mut`/`into_inner`/`get_mut` cover the whole public
  surface used by `RwLock<T>`. No semos_std dependency added; this
  keeps the gating fully local to `target_os = "none"` and avoids
  pulling semos_std into this module's namespace just for a lock.

- **ReadGuard / WriteGuard alias to `Ref` / `RefMut` (not a newtype).**
  `parking_lot::RwLockReadGuard` and `parking_lot::MappedRwLockReadGuard`
  are distinct upstream types because parking_lot's map operation has
  to track the un-mapping lifetime separately. `core::cell::Ref`
  doesn't need this — `Ref::map` returns another `Ref`. So aliasing
  both `ReadGuard` and `MappedReadGuard` to the same `Ref` keeps the
  callers (notably `steal.rs:35`/`steal.rs:48`'s
  `ReadGuard::map(borrow, ...)` / `WriteGuard::map(borrow, ...)`) ABI-
  identical without inventing a Mapped* newtype.

## 3. Deferred work, line-precise

Nothing deferred for the three assigned files. The parking_lot
detachment item from A1's notes is fully cleared for `sync.rs`,
`sync/lock.rs`, and `sync/freeze.rs`.

**Note for parent integration**: the rest of A1's pending-work list
(rustc-stable-hash, tracing, elsa, ena Cargo.toml flips) is not in
scope for this agent and remains pending as A1 documented.

**Possible follow-up Send/Sync auto-trait gap**: on the host
`parking_lot::RwLock<T>` is `Send + Sync`. On SemOS `RefCell<T>` is
`Send` (when `T: Send`) but `!Sync`. If a caller later proves it
needs `RwLock<T>: Sync` (e.g. to put `Steal<T>` in a `&'static`),
the fix is an `unsafe impl<T: Send> Sync for RwLock<T> {}` under
`#[cfg(target_os = "none")]` — safe because the SemOS rustc is
single-threaded per plan §1.4 so there is no actual concurrent
access. Not added preemptively; flagging in case a downstream agent
hits an obvious "needs Sync" error.

## 4. New API gaps discovered

None — the patches are entirely self-contained inside this submodule
and do not surface any new semos-std requirements.

## 5. Phase-routing summary

- **`// M27 R4 B1`** (parking_lot collapse): owner = this agent;
  resolved.

No new R3 / §1.x / Phase 4 markers introduced.

## 6. Surprises worth flagging upward

- `freeze.rs` needed zero edits. The R4 B1 item in A1's notes
  ("sync/freeze.rs hasn't been gated yet — still references
  parking_lot::RwLock") was technically a false positive: `freeze.rs`
  never directly references `parking_lot::*` — it pulls `RwLock`,
  `ReadGuard`, `WriteGuard` from `crate::sync`, and the parking_lot
  reference is in `sync.rs`. Once `sync.rs`'s `RwLock<T>` is split
  and the guard aliases re-exported, `freeze.rs` rides for free. A1's
  diagnostic was correct in intent (the chain leads back to
  parking_lot) but the patch sites are all in `sync.rs`.

- `Lock::new` uses `unlikely(mode::might_be_dyn_thread_safe())` on the
  host. `unlikely` itself remains imported (it is also used in
  `Lock::lock_assume`'s `if unlikely(self.mode_union.no_sync.replace
  (LOCKED) == LOCKED)`). Only the `mode` module import is gated to
  host-only; `unlikely` is not.

## 7. Recipe additions

The parking_lot collapse pattern is worth folding into RECIPE.md as a
sibling to A3's `cfg(target_os = "none")` host/target split:

> ### 1.5b Synchronization primitives — parking_lot collapse
>
> When a rustc crate uses `parking_lot::{Mutex, RwLock, RawMutex}` for
> the parallel-compilation path, the SemOS target (single-threaded per
> plan §1.4) does not need real synchronization. Collapse the parking_lot
> path to `core::cell::{RefCell, Ref, RefMut}`:
>
> - Gate `pub use parking_lot::{*Guard*}` re-exports to
>   `cfg(not(target_os = "none"))`. Add a matching SemOS re-export from
>   `core::cell::{Ref as ReadGuard, Ref as MappedReadGuard,
>   RefMut as WriteGuard, RefMut as MappedWriteGuard}`.
> - Split `pub struct RwLock<T>(parking_lot::RwLock<T>)` host vs SemOS
>   with the SemOS variant wrapping `core::cell::RefCell<T>` and
>   routing methods to `borrow/borrow_mut/try_borrow/try_borrow_mut/
>   into_inner/get_mut`.
> - For `Lock<T>` types that already have a `Mode::Sync` / `Mode::NoSync`
>   discriminant (e.g. `rustc_data_structures::sync::lock`), gate the
>   `parking_lot::RawMutex` import to host-only and provide a
>   `parking_lot_shim::RawMutex` unit stub with the matching method
>   signatures (`INIT`, `try_lock`, `lock`, `unsafe unlock`). Patch
>   `Lock::new` to always select `Mode::NoSync` on SemOS; the
>   `Mode::Sync` arm becomes statically unreachable.
> - Callers that do `ReadGuard::map(borrow, |x| ...)` work unchanged
>   because `Ref::map` has the same signature.
>
> A1's notes recommended this approach (alternative: vendor parking_lot's
> no_std mode); B4 implemented it for `rustc_data_structures`.

Plus the verification observation: when one module re-exports another's
parking_lot-tied types (as `freeze.rs` does), the gate needs to live in
the *defining* module only. Once `sync.rs` is patched, all downstream
re-exporters get the right SemOS aliases for free.
