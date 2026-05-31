# Per-agent handoff template (M27 Phase 2/3/4)

Copy this file's structure when writing your `docs/m27-port/<phase>/
<agent-id>-<scope>.md` notes. The template is short on purpose — the
goal is consistency, not exhaustiveness.

The single most important section is **§3 Deferred work, line-precise**.
A2 → A2-followup proved that section is worth ~10× efficiency on the
followup. Write it as if your successor will read nothing else.

---

## Template starts here

```markdown
# <Agent ID> — <crate name(s) or scope>

**Date:** 2026-MM-DD
**Phase:** 2a / 2b / 3-frontend / 3-semantics / 4-codegen / 5-integration
**Assigned crates / files:** <list>
**Status:** COMPLETE / PARTIAL / BLOCKED
**Token cost (self-report):** <tokens> / <tool_uses> / <duration>
**Source LOC patched:** <approx>

## 1. Per-file diff summary

| File | LOC | Changes | Markers added |
|------|----:|---------|---------------|
| src/<file>.rs | <loc> | <std::* substitutions / structural changes / cfg gates added> | <// M27 R4 Bx / §X / R3 sites> |
| ... | | | |

Brief prose for non-mechanical files (1-2 sentences each). Trivials
can be one row in the table.

## 2. Decisions made (architectural)

For each non-mechanical choice (cfg-out an incremental path, cfg-gate
a host-only module, replace API X with Y, etc.):

- **<decision name>**: what you did, what you considered, why you chose
  this. Cite the plan §X or recon R*.
- ...

If you didn't make any architectural choices, write "None — pure
recipe application."

## 3. Deferred work, line-precise (the load-bearing section)

If you finished everything, this section says "Nothing deferred."

Otherwise, **for each deferred file or sub-section**, write a recipe
specific enough that a followup agent can apply it without re-deriving
the analysis. Example shape (A2's hygiene.rs recipe was this
detailed):

> ### `src/hygiene.rs`
> - Lines 1-3: replace `use std::{borrow::Cow, hash::Hash, hash::Hasher};` with `use core::{hash::Hash, hash::Hasher}; use alloc::borrow::Cow;`.
> - Line 18: keep `scoped_thread_local!(HYGIENE_DATA)` as-is; semos-std
>   shim is in place.
> - Lines 1144-1287 (FilePathMapping impl): replace `use std::path::*;`
>   imports with `use semos_std::path::*;`. Mark every use of
>   `path.components()`, `Component::Normal`, `path.strip_prefix()`
>   with `// M27 R4 B5 TODO(Phase 2b)`. Don't try to substitute.
> - Otherwise: standard bulk `std::* → core::*/alloc::*` per RECIPE.

Cite the upstream file content via `git show main:<path>` line numbers
so the followup can verify the lines haven't shifted.

## 4. New API gaps discovered

If you found a missing semos-std surface that wasn't on R2's top-6
list:

- **<gap>**: where it's used (file:line), what shape it needs
  (Cow<Path>, etc.), what your interim treatment was (marker / stub /
  full deferral).

If none, "None — semos-std surface was sufficient."

## 5. Phase-routing summary

For each marker class you added:

- **`// M27 §1.x`**: dropped per a plan decision; owner = Phase 2b or
  Phase 4 integration.
- **`// M27 R4 Bx`**: needs a semos-std surface extension or kernel
  feature; owner = parent semos-std prep work.
- **`// M27 R3:`**: hash-consolidation candidate; owner = Phase 4
  (ABI-visible).
- **`// M27 TODO(Phase <n>):`**: blocked on a specific later crate.

## 6. Surprises worth flagging upward

Anything that contradicts the recon or the recipe, anything the next
wave's agents should know. Be terse.

## 7. Recipe additions

If you discovered a new pattern worth adding to
`docs/m27-port/RECIPE.md` (like A3's `cfg(target_os = "none")` host/
target split, or A6's "git show main: + Write" workaround), describe
it here. The parent will fold it into the canonical RECIPE.

```

## Template ends here

---

## Quick-reference rules

- **One file per agent**, named `<agent-id>-<short-scope>.md`.
- **Token / tool-use / duration**: self-report from your usage block;
  this feeds the running token/LOC table in EXPERIMENT_LOG.
- **Cap length** at 200-300 lines. Notes that bloat past that probably
  belong in the experiment log or as new RECIPE sections.
- **No code outside markdown fences** in notes — the canonical place
  for patched code is the source tree, not the notes.

---

## Example of a good handoff (in tree)

`docs/m27-port/2a/A2-rustc_span.md` — A2's notes are the gold standard
because A2-followup achieved 14 tokens/LOC on the basis of those
recipes. Read A2's §3 for the shape; that's why this handoff template
exists.

`docs/m27-port/2a/A3-trivials.md` — A3's notes are the gold standard
for the "three small crates with `cfg(target_os = "none")` split"
shape.

`docs/m27-port/2a/A6-proc-macros.md` — A6's notes are the gold
standard for "zero source edits, here's the config" shape.

If your work is closer in shape to one of those, mirror its structure.
