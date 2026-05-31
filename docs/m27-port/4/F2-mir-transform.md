# F2 — rustc_mir_transform

**Date:** 2026-05-31
**Phase:** 4-codegen (codegen-tier MIR opt passes)
**Assigned crates / files:** `compiler/rustc_mir_transform/` — 95 .rs files, ~34k LOC
**Status:** IN PROGRESS

## 0. Pre-port survey

Single grep of `\bstd::` across all 95 files returns ~95 hits — extremely thin per the B1 LARGE-but-THIN pattern. The bulk substitution table coverage:

- `std::iter`, `std::mem`, `std::fmt`, `std::ops`, `std::cell`, `std::cmp`, `std::hash`, `std::slice`, `std::ops::ControlFlow`, `std::any::type_name` — all → `core::*`
- `std::borrow::Cow` → `alloc::borrow::Cow`
- `std::rc::Rc` → `alloc::rc::Rc`
- `std::collections::hash_map::Entry` (1 site in pass_manager.rs) → `rustc_data_structures::fx::StdEntry as Entry`
- `std::sync::LazyLock` (1 site in lib.rs) → cfg-conditional host/semos_std
- `std::fs::File` + `std::io` (dump_mir.rs `emit_mir`) — cfg-gate whole function; SemOS stub returns `io::Error::other()`
- `std::io::Write` in `&mut dyn std::io::Write` (1 site in dest_prop.rs) — cfg-conditional import

**hashbrown::hash_table::{Entry, HashTable}** in gvn.rs: kept as-is. This is `hash_table::Entry` (not `hash_map::Entry`), already in no_std hashbrown.

**No FatalError, no PathBuf, no scoped_thread_local, no OsString sites.** This crate is structurally a pure-CPU MIR rewriter — no FS surface except dump_mir's emit_mir + the polymorphic dyn Write extra_data closure in dest_prop.

## 1. Per-file diff summary (live, append-only)

| File | Changes | Markers |
|------|---------|---------|
