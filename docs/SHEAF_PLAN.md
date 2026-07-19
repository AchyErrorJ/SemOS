# Sheaf — Bundle Filesystem
## Design & Implementation Plan (coding-agent handoff)

**Status:** v0.1 design, ready for Phase 0 implementation
**Author:** Jeremie (AchyErrorJ) · **Date:** 2026-07-17 (agent-profile addendum 2026-07-19)
**Spec note:** `.agent` files are optional **text leaves** with `role = "agent"` (§14),
not a third leaf kind and not executable authority.
**Context:** Designed for SemOS (semantic-object OS, SUID-addressed namespace) but
Phase 0 is a standalone userland prototype so the model can be validated before
kernel integration.

---

## 1. Thesis

The file is dead; long live the bundle. In Sheaf, **the folder is the native
unit of content**. Every "file" is a bundle: a folder containing its payload
plus everything needed to describe, render, and verify it — a manifest, style
facets, previews, provenance. Leaves are terminal and come in exactly two
structural kinds: **text leaves** (.md / .toml / .css / .agent — agent- and
human-editable UTF-8) and **blob leaves** (raw bytes with a TOML sidecar).
`.agent` is not a new structural leaf kind; it is an optional text leaf with
`role = "agent"` that declares how an agent may interact with the bundle (§14).
Nothing recurses below a leaf.

Sharing is **export**, and export is a one-way *projection* of the bundle:
PDF is a frozen visual projection, `.sheaf` (tar/zip) is a lossless transport
projection, flat `.md` is a lossy text projection. Every exported artifact
carries a **SUID provenance stamp** back to its source bundle.

This mirrors the LegibleStudios rule that already works: the structured
document is the contract; everything else is a rendering of it.

---

## 2. Terminology (use these exact words)

| Term | Meaning |
|---|---|
| **Sheaf** | The filesystem as a whole. |
| **Bundle** | A folder that presents as a single file. Identified by the presence of `bundle.toml`. |
| **Facet** | A named member of a bundle (a leaf file or a sub-bundle). |
| **Text leaf** | Terminal facet: `.md`, `.toml`, `.css`, `.agent`. UTF-8, diffable, LLM-fluent. |
| **Blob leaf** | Terminal facet: arbitrary bytes (png, elf, ucode…). MUST have a `<name>.toml` sidecar. |
| **Agent-profile leaf** | Optional text leaf with extension `.agent` and `role = "agent"`. Declares requested tools, readable/writable facets, tier ceiling, and confirmation policy for an agent operating on this bundle. |
| **SUID** | 128-bit semantic unique ID (two u64s, `high:low`), minted by the system at bundle creation. Globally unique, never reused. |
| **Default facet** | The facet a bundle renders when opened normally (declared in manifest). |
| **Export / projection** | One-way rendering of a bundle to a shareable artifact. |
| **Edit Contents** | The UI verb that opens a bundle *as a folder* (right-click). |

---

## 3. Bundle anatomy

A bundle is a directory containing **exactly one** `bundle.toml` at its root.
Detection rule (the "bundle bit"): *a directory containing `bundle.toml`
presents as a file; all other directories present as folders.*

Example — a photo:

```
beach-photo/                 ← presents as a single file
├── bundle.toml              ← manifest (REQUIRED, exactly one per bundle root)
├── image.png                ← blob leaf (payload)
├── image.toml               ← sidecar for the blob (REQUIRED for every blob)
├── note.md                  ← text leaf (caption, user notes)
└── preview.png              ← facet: thumbnail (+ preview.toml sidecar)
```

Example — a document:

```
quarterly-report/
├── bundle.toml
├── content.md               ← default facet
├── style.css                ← render facet
├── assets/                  ← a nested bundle (has its own bundle.toml)
│   ├── bundle.toml
│   └── chart.png (+ chart.toml)
└── provenance.toml          ← export lineage (see §7)
```

Example — a document with bundle-bound agents:

```
quarterly-report/
├── bundle.toml
├── content.md               ← default facet
├── style.css                ← render facet
├── editor.agent             ← agent leaf: "help maintain this bundle"
├── reviewer.agent           ← agent leaf: "review before export"
└── provenance.toml
```

Rules:

- Bundles MAY contain sub-bundles. Sub-bundles present as files inside their
  parent. Depth is unbounded but cycles are impossible (tree structure).
- Leaves are terminal. A `.md` or `.agent` file never contains a
  `bundle.toml`; there is no recursion below any leaf. `.agent` remains a text
  leaf; its special behavior comes from `role = "agent"`, not from a third leaf
  kind.
- Every blob leaf MUST have a sibling sidecar `<blobname>.toml`. A blob
  without a sidecar is a spec violation (`sheaf lint` fails).
- Every `.agent` text leaf MUST be UTF-8 and MUST parse as an agent declaration
  (§14). Agent-profile leaves need no sidecar; their hash + tier live in
  `bundle.toml` like any other text facet.
- `bundle.toml` is itself a text leaf but is NOT a facet — it is the bundle's
  identity, not its content.

---

## 4. Manifest schema (`bundle.toml`)

```toml
schema = 1                        # u32, this spec version. REQUIRED.
suid = "8f3a2c1d9e4b5a67:01cf2e8890ab4d5e"  # minted at creation, immutable. REQUIRED.
kind = "document"                 # free-form short string: document, photo, app, data… REQUIRED.
title = "Quarterly Report"        # human name (the folder name is cosmetic; this is canonical-ish)
created = 2026-07-17T14:03:22Z    # RFC 3339 UTC, set once by the system
modified = 2026-07-17T15:41:09Z   # updated by the system on any facet write

default_facet = "content.md"      # path relative to bundle root. REQUIRED.

tier = 1                          # 0=Public 1=Internal 2=Sensitive 3=Secret
                                  # ceiling for all facets; kernel-held in SemOS,
                                  # advisory in userland prototype

[facets."content.md"]
leaf = "text"                     # text | blob
role = "payload"                  # payload | render | preview | meta | data | agent
tier = 1                          # per-facet tier, MUST be <= bundle tier
mime = "text/markdown"
sha256 = "…"                      # REQUIRED for blob facets; recommended for text facets

[facets."style.css"]
leaf = "text"
role = "render"
tier = 0
mime = "text/css"
sha256 = "…"

[facets."image.png"]
leaf = "blob"
role = "payload"
tier = 2
sha256 = "…"                      # REQUIRED for blob facets

[facets."reviewer.agent"]
leaf = "text"
role = "agent"
tier = 1
mime = "application/sheaf-agent+toml"
sha256 = "…"                      # of the .agent declaration bytes
```

Blob sidecar (`image.toml`) — deliberately smaller than a manifest:

```toml
schema = 1
mime = "image/png"
sha256 = "…"          # of the blob bytes
bytes = 184220
title = "Beach, golden hour"
```

Agent-profile text leaf (`reviewer.agent`) — see §14 for the full schema:

```toml
schema = 1
name = "reviewer"
purpose = "Review content.md before export and suggest fixes."
model = "default-local-or-approved-remote"
max_tier = 1
tools = ["read_facet", "write_facet", "comment", "export"]
inputs = ["content.md", "style.css"]
outputs = ["review.md"]
```

Invariants an agent MUST maintain:

- `suid` never changes after minting, even across rename/move/copy-in-place.
  A *copy* is a new bundle: new SUID, with `derived_from` set (see §7).
- `facets.*.tier <= tier` (bundle ceiling). In SemOS, the kernel enforces
  this; in userland, `sheaf lint` enforces it.
- `default_facet` must resolve to an existing facet with role `payload`.
- `sha256` fields must match reality after any write.
- `.agent` facets MUST have `leaf = "text"` and `role = "agent"`. An agent
  profile's declared `max_tier` MUST be `<= facets.<agent>.tier <= bundle
  tier`. An agent profile can request tools, but it does not receive them until
  the runtime grants them under the caller/vouch policy.

---

## 5. UI model — "Edit Contents"

The interaction contract (applies to the SemOS desktop/sem-sh and any host UI):

| Gesture | On a bundle | On a plain folder |
|---|---|---|
| Single click | Select | Select |
| Double click / Enter | **Open** = render `default_facet` with its render facets | Open as folder |
| Right-click | Context menu | Context menu |

Bundle context menu (in this order):

1. **Open** (same as double-click)
2. **Edit Contents** — open the bundle *as a folder*: shows facets, allows
   add/remove/rename facet, edit text leaves, including `.agent` profiles, in place. This is
   the only way "inside" from the GUI.
3. **Export as…** → PDF / `.sheaf` / flat `.md` (see §6)
4. **Get Info** — manifest view: SUID, kind, tier, facet list, provenance chain
5. **Run Agent…** — choose an `.agent` facet to run against this bundle. The UI
   must show requested tools/tier before launch.
6. **Reveal in Folder** — show the bundle as a directory in its parent

Terminal verbs (and the Phase 0 CLI) mirror this exactly so agents and humans
share one mental model:

```
sheaf open <path>            # render default facet
sheaf edit <path>            # list facets / open as folder
sheaf export <path> --to pdf|sheaf|md [--out …]
sheaf info <path>            # manifest + provenance
sheaf agent <path> <name>     # run <name>.agent after showing/confirming grants
```

Design note: never show a bundle's guts by default in listings. If the user
sees facets in `ls`, the illusion is broken and tools will start depending on
it. `sheaf edit` is the door; keep it the only door (aside from explicit
debug flags).

---

## 6. Export (projections)

| Format | What it is | Lossy? | Use for |
|---|---|---|---|
| `.sheaf` (tar.gz of the bundle dir) | Lossless transport projection | No | Sharing with another Sheaf system; backup; git |
| PDF | Frozen visual projection of default facet + render facets | Yes (interactivity, other facets) | Humans, permits, print |
| flat `.md` | Text projection: default facet text + `> exported from` header | Yes (blobs become placeholder links) | Email, chat, LLM context |

Rules:

- Export is **one-way**. Importing a PDF back into Sheaf creates a *new*
  bundle wrapping the PDF as a blob — it never reconstructs the source bundle.
  State this in the UI so nobody expects round-tripping.
- `.sheaf` import is the exception: it restores the bundle byte-for-byte,
  **but mints a new SUID** and records `derived_from` (a copy, not the same
  object — otherwise two live objects would share one identity).
- Export of a bundle containing facets above the caller's tier (SemOS) either
  omits those facets (with a manifest note) or fails, per a `--on-tier-skip`
  flag. Never silently include them.

---

## 7. Provenance via SUID

Every export stamps its origin; every copy records its parentage.

**`provenance.toml`** (facet, maintained by the system, append-only):

```toml
schema = 1
suid = "8f3a…:01cf…"
derived_from = "b21e…:77aa…"        # SUID of source bundle if this is a copy/import
[[exports]]
at = 2026-07-17T16:02:11Z
format = "pdf"
sha256 = "…"                        # of the exported artifact
by = "user:jeremie"                 # or "agent:claude" — agents MUST identify as agents
```

**Inside exported artifacts:**

- PDF: document metadata `Subject` = `sheaf://8f3a2c1d9e4b5a67:01cf2e8890ab4d5e`
  plus `X-Sheaf-Export: 2026-07-17T16:02:11Z` in the info dict.
- `.sheaf`: provenance.toml travels inside (it is a facet).
- flat `.md`: first line is a comment header:
  `<!-- sheaf://8f3a…:01cf… · exported 2026-07-17T16:02:11Z · from "Quarterly Report" -->`

`sheaf verify <artifact>` resolves the stamp: confirms hash match against a
live bundle if available, prints the chain `artifact ← bundle ← parent`.

This makes export a *feature of the object model*, not an escape hatch:
every artifact in the wild is traceable home.

---

## 8. Security model (SemOS integration — Phase 1+)

- **Tier-per-facet** is the headline upgrade over per-object redaction: a
  manifest can be Public while its payload is Secret. The LLM/agent view of a
  bundle = the set of facets at or below the caller's tier, with text facets
  (including `.agent` profiles) passed through the existing redactor. Blobs are
  never redacted — they are included or excluded wholesale.
- **`.agent` profiles are data, not authority.** A `.agent` text leaf declares desired
  behavior and requested tools; it is not executable by itself and cannot grant
  itself clearance. Runtime authority is still derived from the human caller,
  SemOS task tier, vouch grants, and the agent profile's own `max_tier` ceiling.
  Effective tier = `min(caller_tier, bundle_tier, facet_tier, agent.max_tier)`.
- **The manifest cannot elevate itself.** `tier` fields are writable only by a
  caller whose clearance covers the change (same rule as today's semantic
  objects). Kernel-held, not bundle-held, in SemOS; the in-bundle copy is a
  cached projection.
- **Render facets are untrusted input.** `style.css` (or any future script
  facet) executes in the renderer's privilege context. Treat a bundle from an
  agent (tier 0) as hostile: sanitize CSS, no network loads from render
  facets, no facet may reference outside its bundle root.
- **Torn writes:** a bundle save touches multiple facets. Commit atomically —
  in SemOS, one snapshot-ring transaction per save; in userland, write to
  `<facet>.tmp` + rename, then update manifest last. A bundle whose manifest
  is older than its facets is dirty; `sheaf lint` reports it.
- Agents identify as agents in provenance (`by = "agent:…"`). Non-negotiable —
  this is the audit trail for the vouch model.
- Agent profiles MUST write provenance for material edits (`by =
  "agent:<bundle-suid>/<agent-name>"`) and MUST NOT silently modify facets
  above their effective tier.

---

## 9. Implementation phases

### Phase 0 — userland prototype (validate the model) · target: 1–2 weeks
Rust crate `sheaf` + CLI `sheaf`, pure std + serde/toml/sha2/tar/flate2.

- [ ] `sheaf-core` lib: Bundle type, manifest/sidecar (de)serialization, SUID
      minting (random 128-bit), lint/verify
- [ ] CLI verbs: `new`, `open`, `edit`, `add <facet>`, `rm <facet>`,
      `export --to sheaf|md`, `import`, `info`, `lint`, `verify`,
      `agent <bundle> <agent-name>`
      (PDF export stubbed behind a `pdf` feature; plain-text rendering OK)
- [ ] Agent-profile leaves: parse/lint `.agent`, display requested grants, run a
      stubbed/local "dry-run" executor that can read/write declared facets
      through the same atomic commit path (real LLM backend can come later)
- [ ] Torn-write safety: tmp+rename writes, dirty-bundle detection
- [ ] Provenance: append on copy/export, stamp headers in `.md` exports
- [ ] 90%+ of acceptance tests in §10 green

**Exit criteria:** round-trip a photo bundle and a nested document bundle;
corrupt a facet mid-write and prove `lint` catches it; export → verify chain.

### Phase 1 — SemOS namespace integration
- [ ] Registry: object content becomes *facet set* instead of single buffer
      (backward-compat: a legacy object = bundle with one anonymous payload)
- [ ] Facet addressing: `path/to/thing::facet` in the path namespace;
      `SYS_SEM_READ_FACET` alongside today's whole-object reads
- [ ] Tier-per-facet enforcement in the syscall layer (reuse the existing
      `current_task_max_tier()` gating pattern — but note the known
      pointer-validation hardening needed in that layer first)
- [ ] LLM context builder: facet-filtered views replace whole-object redaction
- [ ] Snapshot ring: multi-facet atomic commit

### Phase 2 — UI
- [ ] sem-sh: `open` / `edit` / `export` / `info` builtins
- [ ] Desktop (ls-app-style head): bundle icon treatment, double-click render,
      right-click menu per §5, facet editor view
- [ ] A renderer for `content.md + style.css` (tiny-skia path already exists)

### Phase 3 — ecosystem
- [ ] `sheaf://` SUID resolution over the network (object sync)
- [ ] git-native VCS story (bundles of text leaves diff cleanly — document it)
- [ ] Importers: `.docx`, `.pages`, Obsidian vault → bundle

Do NOT start Phase 1 before Phase 0 exit criteria pass. The kernel work is the
expensive place to discover the model is wrong.

---

## 10. Acceptance tests (Phase 0)

1. `sheaf new doc.md` → folder with valid manifest, new SUID, `content.md`
   default facet. `sheaf lint` passes.
2. Bundle renamed/moved → SUID unchanged, lint passes.
3. `sheaf export --to sheaf` → import elsewhere → new SUID, `derived_from`
   set, facet hashes match original.
4. Blob facet with sidecar deleted → lint FAILS with "blob without sidecar".
5. Facet tier above bundle ceiling → lint FAILS.
6. Kill the process between facet write and manifest update → next `lint`
   reports dirty bundle; `sheaf repair` re-syncs manifest.
7. PDF/md export contains the `sheaf://` stamp; `sheaf verify` prints the chain.
8. A 3-level nested bundle exports and re-imports with structure intact.
9. A folder *without* `bundle.toml` is never treated as a bundle by any verb.
10. `sheaf edit` on a text leaf opens the bytes as-is (no hidden transform).
11. `.agent` facet with `leaf != "text"` or `role != "agent"` → lint FAILS.
12. `.agent` facet with `max_tier > facet tier` or requested output facet
    above effective tier → lint/run FAILS.
13. `sheaf agent bundle reviewer` prints requested tools/tier before run and
    records `by = "agent:<suid>/reviewer"` provenance for any edit.

---

## 11. Open questions (human decisions needed — agents, do not guess)

1. **Bundle extension?** Bundles-as-folders need no extension, but host-OS
   interop is nicer with one (`.sheafdir`? macOS-style `.sheaf` on dirs?).
   Leaning: no extension inside Sheaf; `.sheaf` only for exported archives.
2. **Duplicate SUID detection on import** — if a `.sheaf` is imported twice,
   both copies share `derived_from`. Fine, or do we keep a local SUID registry
   to warn "you already have this object"?
3. **Facet size caps** — SemOS objects have `MAX_FILE_CONTENT` today. Per-facet
   cap, per-bundle cap, or streaming blobs (out of manifest, into a block
   store) for big media?
4. **Does `style.css` get a sibling `render.md` for LLM consumption** (a
   text description of how the thing should look), or is CSS enough for agents?
5. **Name of the archive magic bytes** — plain `.tar.gz` or a custom header so
   `file` can say "Sheaf bundle archive"?
6. **Agent runtime backend** — local-only first, approved remote LLM, or both
   behind per-run confirmation? Default for Phase 0: parse/lint + dry-run/local
   executor, no network unless explicitly configured.

---

## 12. Non-goals (v1)

- No POSIX emulation layer. Legacy apps get exports, not mounts.
- No in-place editing of blobs (images edited outside, re-added as facets).
- No cross-bundle hard links in v1; reference by SUID in `provenance.toml`.
- No executable arbitrary scripts in v1. `.agent` is declarative data; execution
  happens only through the Sheaf/SemOS agent runtime and its tool grant system.

---

## 13. Addendum — mixed-world semantics (2026-07-17)

Bundles and non-bundles coexist permanently; Sheaf does not convert the world.
The `bundle.toml` detection rule is the whole boundary: directory with a
manifest → presents as a file; without → plain folder; loose plain files are
unchanged. Programs fall into three classes:

1. **Sheaf-native** — uses the sheaf API; full facet/manifest/provenance access.
2. **Unaware byte-readers** (`cat`, `grep`, compilers): reading a bundle path
   returns the **default facet's bytes** as a transparent projection. Zero
   changes needed for most tools.
3. **Unaware traversers** (`find`, backup, git): directory enumeration reports
   a bundle as a **file, not a directory** (the macOS package-bit lie).
   Traversing inside requires an explicit `--contents` opt-in — the CLI twin
   of right-click → Edit Contents.

**The load-bearing invariant:** a tool that doesn't know about bundles must
never be able to accidentally tear one. Writes from unaware programs to a
bundle path replace the default facet only (manifest `modified` bump +
provenance entry); they cannot touch other facets. Multi-facet writes are
restricted to opted-in tools, which must follow the atomic-commit rule (§8).

Import story: existing trees stay loose files/folders until `sheaf pack`
converts them. Legacy Linux-ABI user programs (SemOS) land in classes 2–3
automatically.

Phase 0 additions: implement the read-projection and the readdir lie in the
CLI/library (a FUSE mount is the natural demo vehicle), plus acceptance test
14: `find`-style traversal of a tree containing bundles visits each bundle
exactly once, as a file; test 15: an unaware write to a bundle path updates
only the default facet and the manifest stamp.

---

## 14. Agent-profile text leaves (`.agent`)

An **agent-profile leaf** is an optional terminal **text** facet with extension
`.agent`, `leaf = "text"`, and `role = "agent"`. It is a UTF-8 TOML document
that declares a bundle-bound helper profile: what an agent may read/write,
which tools it requests, what tier ceiling applies, and when human confirmation
is required. It is intentionally *not* a program: it cannot run outside the
Sheaf/SemOS agent runtime, cannot grant itself tools, and cannot raise its own
clearance.

Use cases:

- `editor.agent`: maintain style/content consistency inside the bundle.
- `reviewer.agent`: review default facet before export.
- `publisher.agent`: produce export projections and update provenance.
- `ingest.agent`: normalize imported loose files into bundle facets.

Minimal schema:

```toml
schema = 1
name = "reviewer"                         # stable, unique within bundle
purpose = "Review content.md before export and suggest fixes."
model = "default-local-or-approved-remote" # policy label, not raw authority
max_tier = 1                              # MUST be <= facet tier

tools = [
  "read_facet",
  "write_facet",
  "comment",
  "export",
]

inputs = ["content.md", "style.css"]       # relative facet paths or globs
outputs = ["review.md"]                    # facets this agent may create/write
```

Optional fields:

```toml
system = "You are a careful editor. Preserve the author's voice."
temperature = 0.2
requires_human_confirm = true              # default true for write/export tools
network = false                            # default false; true still needs runtime grant
```

Rules:

- `name` MUST match the filename stem (`reviewer.agent` → `name = "reviewer"`).
- `max_tier <= facets."<name>.agent".tier <= bundle tier`.
- Effective tier at runtime is `min(caller_tier, bundle_tier, facet_tier,
  max_tier)`.
- `tools` are requested capabilities, not granted capabilities. The runtime
  must show requested tools and effective tier before launch, and SemOS must
  enforce tool grants in the syscall/runtime layer.
- `inputs` and `outputs` are bundle-relative. No `..`, absolute paths, network
  paths, or references outside the bundle root.
- Writes must use the same atomic commit path as human edits. Every material
  write appends provenance with `by = "agent:<bundle-suid>/<agent-name>"`.
- Agent-profile leaves are ordinary text leaves for diff/redaction/export, but
  role-specific for execution. Flat `.md` export should list agent profiles as
  metadata, not inline their full prompts unless `--include-agents` is
  explicitly requested.

Lint failures:

- invalid TOML/UTF-8;
- filename stem != `name`;
- missing `leaf = "text"` or `role = "agent"` in `bundle.toml`;
- `max_tier` above facet/bundle tier;
- requested output facet above effective tier;
- input/output path escapes the bundle root;
- unknown tool name unless `--allow-unknown-tools` is passed for forward
  compatibility.

Phase 0 can implement `.agent` as parse/lint + dry-run/local execution only.
Remote LLM execution is a later backend choice and must be opt-in.

*Hand this file to a coding agent whole. It contains the model, the schemas,
the UI contract, the phases, and the tests. Where it says "open question,"
stop and ask the human.*
