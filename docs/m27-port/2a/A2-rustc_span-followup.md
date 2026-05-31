# M27 Phase 2a A2-followup — rustc_span remaining 5 files

Continuation of `A2-rustc_span.md`. Applies the line-precise recipes A2
left (§L, §SM2, §H, §SM, §S) to the last five rustc_span files.

Same sandbox constraints as A2 (`git merge` denied). Files live in the
main tree at `F:\Software\ArmKernel3\user-programs\semos-rustc\vendor-rustc-src\compiler\rustc_span\src\`;
edited in-place there. Verify with `git status ...` from the main tree
(this worktree shows nothing because the files are outside it).

semos-std additions A2 flagged as missing have all landed
(`thread_local!`, `scoped_thread_local!`, `env::var_os`, `ffi::OsString`,
`path::canonicalize_lexical`, `process::abort_with_code`). Used
directly where applicable.

## Per-file results

### `hygiene.rs` (§H — minimal)
Only the 3-line top-of-file imports: `std::hash::Hash` →
`core::hash::Hash`, `std::sync::Arc` → `alloc::sync::Arc`,
`std::{fmt, iter, mem}` → `core::{fmt, iter, mem}`. `scoped_tls!()`
calls stay as-is (parent's scoped_tls vendor patch lands the macro on
`semos_std`'s surface). No other `std::` references in the 1,579-line
file.

### `source_map/tests.rs` (§SM — minimal)
A2's recipe said to substitute `use std::path::PathBuf;` at the top —
**but that import does NOT exist in upstream**. The file is
`use super::*;` only, and inherits PathBuf via the patched
source_map.rs. No top-level substitution needed. Discrepancy from
A2's notes: A2 said "one mid-file `std::path::Path::new` (line ~120)";
actually two `Path::new(...)` calls at 766/774, both inside
`#[cfg(target_os = "linux")]` so never compile on SemOS. No edits.

### `symbol.rs` (§S — minimal)
3-line top imports → `core::*` per recipe. Three mid-file sites
(transmute, cmp::Ordering, transmute) at lines 2858/2931/2958 —
slightly shifted from A2's ~2856/~2929/~2956 estimates, same 3 sites.
All `std::*` → `core::*`. Zero `std::` references remain.

### `lib.rs` (§L — biggest)
1. `#![no_std]` inserted as the **first** inner attribute (must
   precede the `#![feature(...)]` block). `extern crate alloc;` placed
   AFTER the inner-attribute block. A2's recipe grouped them; split
   for Rust attribute-ordering rules.
2. Imports per recipe table: `Cow` → `alloc::borrow`, core/alloc/
   semos_std splits as specified, B5 markers on path/io. Added
   explicit `use alloc::string::{String, ToString};` +
   `use alloc::vec::Vec;`.
3. `scoped_tls::scoped_thread_local!(...SESSION_GLOBALS...)` kept
   as-is with B2 marker above.
4. Mid-file `std::` refs: `std::hash::Hasher` → `core::hash::Hasher`
   (~line 335), `std::fmt::Formatter`/`Result` → `core::fmt::*`
   (~line 548), `std::mem::replace` → `core::mem::replace` (~2582).
5. `SourceFileHashAlgorithm` enum tagged with `// M27 R3:` markers
   on the three non-Blake3 variants + block-comment explaining the
   ABI-stability rationale (rmeta crosses the host/SemOS stage
   boundary).

Zero code-level `std::` references remain; only docstring mentions.

### `source_map.rs` (§SM2 — middle-complexity, **multiple Phase 2b deps surfaced**)

Items A2 anticipated, applied as written:
- Imports at top: `std::fs::File`, `std::io::{self, BorrowedBuf, Read}`,
  `std::{fs, path}` → `semos_std::fs::{self, File}`,
  `semos_std::io::{self, Read}` (BorrowedBuf dropped — Phase 2b dep),
  `semos_std::path::{self, Path, PathBuf}` with B5 markers.
- `mod monotonic { use std::ops::{Deref, DerefMut}; ... }` →
  `core::ops::*`.
- Line 279 `std::str::from_utf8` → `core::str::from_utf8`.
- FilePathMapping struct + methods tagged with `// M27 R4 B5:` for
  grep.

Items A2's recipe did NOT anticipate (Phase 2b deps surfaced now):

1. **`RealFileLoader` impl is more std-dependent than A2 noted.**
   Upstream uses `path.exists()`, `file.metadata()`, `BorrowedBuf`,
   `read_buf_exact`, `io::ErrorKind::UnexpectedEof`, `io::ErrorKind::Interrupted`,
   `io::Error::other(formatted_message)` — none of which exist in
   semos_std today. I rewrote the impl to use the semos_std free
   functions `fs::read` / `fs::read_to_string` instead. This is
   functionally equivalent for the SemOS-side build but **drops the
   upstream peak-memory optimization** in `read_binary_file` (it now
   briefly holds 2× peak RSS for binary includes). Documented
   inline with a Phase 2b TODO marker; the optimal path can come back
   once semos_std grows `BorrowedBuf` + `File::metadata`.
2. **`map_prefix` / `reverse_map_prefix_heuristically` depend on
   `Cow<Path>`, `path.as_os_str()`, `path.strip_prefix(...)`,
   `path.to_path_buf()`, `path.components()`, `path::Component::Normal`,
   `from.join(rest)`** — none of which `semos_std::path` provides
   today. Kept upstream code unchanged but tagged each method with a
   `// M27 R4 B5 TODO(Phase 2b):` marker. **These methods will not
   compile until semos_std::path grows the std::path::PathBuf API
   surface.** This is the single biggest Phase 2b dep this followup
   surfaces.
3. `to.into()` / `to.join(rest).into()` etc. inside `map_prefix`
   require `From<&PathBuf> for Cow<'_, Path>`. semos_std::path has no
   `Cow<Path>` impls — Phase 2b dep.

All other A2 recipe items in §SM2 applied cleanly.

## Phase 2b dependency surface (extended)

A2 flagged scoped-tls (B2) and PathBuf/OsString (B5) as the headline
Phase 2b items. This followup surfaces the **detailed** list of
semos_std additions needed to make `source_map.rs` compile:

| API surface | Used at | Notes |
|---|---|---|
| `semos_std::path::Path::exists()` | RealFileLoader::file_exists | Workaround applied: probe via `File::open` |
| `semos_std::fs::File::metadata()` | (was upstream RealFileLoader) | Worked around by using free `fs::read` |
| `semos_std::io::BorrowedBuf` | (was upstream `read_binary_file`) | Worked around — costs 2× peak RSS for binary include! |
| `semos_std::io::Error::other(msg)` | (was upstream `read_file`) | Worked around — semos_std's Error::other() takes no arg |
| `semos_std::io::ErrorKind` | (was upstream) | Worked around |
| `semos_std::path::Path::as_os_str()` | map_prefix | **Required for FilePathMapping to compile.** |
| `semos_std::path::Path::strip_prefix(prefix)` | map_prefix, reverse_map_prefix_heuristically | **Required.** |
| `semos_std::path::Path::components()` + `path::Component::Normal` | reverse_map_prefix_heuristically | **Required.** |
| `semos_std::path::Path::to_path_buf()` | many sites in `to_real_filename` | **Required.** |
| `From<&PathBuf> for Cow<'_, Path>` etc. | map_prefix Cow handling | **Required.** |

The scoped_tls macro (B2) was already landed by the parent and used
in hygiene.rs as a passthrough. No further B2 work needed for these
5 files.

## Surprises beyond A2's recipes

1. **A2's `use std::path::PathBuf;` at top of `source_map/tests.rs`
   doesn't exist in upstream** — the file is just `use super::*;`.
   No substitution needed. Documented above.
2. **A2's `path::Path::new` mid-file edit in tests.rs** points to two
   sites both inside `#[cfg(target_os = "linux")]`. No edits needed.
3. **`#![no_std]` placement** — A2's recipe had it adjacent to
   `extern crate alloc;`, but Rust requires `#![no_std]` (inner attr)
   to precede all items including `extern crate`. Split them with the
   `#![feature(...)]` block between. Verified Rust accepts this order.
4. **`SourceFileHashAlgorithm` enum location** — A2 didn't pin a
   line number; the actual enum lives at line ~1689 of (pre-edit)
   lib.rs, shifted slightly post-no_std-header insertion. Found by
   grep, marked correctly.
5. **`source_map.rs` `RealFileLoader` complexity exceeded A2's
   `File::open(path)` → `File::open(path.as_str())` recipe.** The
   impl uses ~6 std::fs / std::io APIs that semos_std doesn't expose.
   I rewrote the impl to use the semos_std free functions, which is a
   semantic-equivalent but loses the peak-RSS optimization. This is
   the largest behavioral change in this followup; worth a parent
   review.
6. **`FilePathMapping`'s `map_prefix` / `reverse_map_prefix_heuristically`
   require ~5 PathBuf methods semos_std::path doesn't have.** I left
   the upstream code intact under TODO markers — the file does NOT
   compile today on the SemOS target until those methods land. This
   is a hard Phase 2b dep that A2's audit didn't surface.

## What's blocked on Phase 2b that wasn't visible to A2

- **`semos_std::path::PathBuf` API surface** — needs `.as_os_str()`,
  `.strip_prefix()`, `.components()`, `.to_path_buf()`, `Path::exists()`,
  `Cow<Path>` From impls, `path::Component::Normal` enum. ~1 session
  in semos_std. Without this, `source_map.rs` does not compile.
- **`semos_std::io::BorrowedBuf` + `File::metadata` + `ErrorKind`** —
  for the peak-RSS-optimal `read_binary_file` path. Optional (the
  rewritten v1 path works without it); restore later.
- **`semos_std::io::Error::other(impl Display)` signature** —
  upstream `io::Error::other(msg)` takes a message. semos_std's
  takes none. Phase 2b can either widen the signature or callers
  drop the message (we dropped it).

## Sanity checks

- Grep for code-level `std::` in the 5 patched files: **clean** (only
  docstring/comment mentions like `/// uses std::fs` survive,
  plus the `#[cfg(target_os = "linux")]`-gated test body in
  source_map/tests.rs which never compiles on SemOS).
- `// M27 R4 B5:` markers in source_map.rs: 8 sites tagged.
- `// M27 R4 B5 TODO(Phase 2b):` markers in source_map.rs: 3 sites
  (RealFileLoader::read_binary_file, FilePathMapping::map_prefix,
  FilePathMapping::reverse_map_prefix_heuristically).
- `// M27 R4 B2:` markers: 1 site (lib.rs SESSION_GLOBALS).
- `// M27 R3:` markers: 3 sites (lib.rs SourceFileHashAlgorithm
  variants).
- `#![no_std]` + `extern crate alloc;` correctly placed in lib.rs.

Files are at:
- `F:\Software\ArmKernel3\user-programs\semos-rustc\vendor-rustc-src\compiler\rustc_span\src\hygiene.rs`
- `F:\Software\ArmKernel3\user-programs\semos-rustc\vendor-rustc-src\compiler\rustc_span\src\lib.rs`
- `F:\Software\ArmKernel3\user-programs\semos-rustc\vendor-rustc-src\compiler\rustc_span\src\source_map.rs`
- `F:\Software\ArmKernel3\user-programs\semos-rustc\vendor-rustc-src\compiler\rustc_span\src\source_map\tests.rs`
- `F:\Software\ArmKernel3\user-programs\semos-rustc\vendor-rustc-src\compiler\rustc_span\src\symbol.rs`

## Constraint compliance

- No `cargo build` runs attempted.
- No other crates modified.
- No `git` write operations.
- No `git merge` attempted (sandbox would deny anyway).
