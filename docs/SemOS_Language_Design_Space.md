# SemOS Language Design Space
### A functional language for SemOS, authored by agents, reviewed by humans

Name: **Patina** — the finish oxidation leaves on metal: a Rust pun
(the implementation language) and a word designers already own (finish,
age, material honesty). A little pretentious, as intended.

---

## 1. Context and Purpose

SemOS is a designer's operating system built on an enhanced folder system:
folders are native typed objects, files carry metadata, views render from
that metadata, and contents are editable in place.

SemOS needs a language because the OS's core interactions — views, queries,
transformations, batch operations, rendering pipelines — should be
*scriptable and composable*, not hardcoded.

The twist: **the primary author of this language is an AI agent, not a
human.** The human designer reads, reviews, and approves. This single fact
drives most of the design.

---

## 2. What a Designer Wants Their OS to Do

These are the capabilities the language must express. Everything else is
secondary.

1. **Views as live queries** — a folder is not a static container; it is a
   query over files. "All drawings tagged `clt`, latest revision, laid out
   as a grid." The view updates when files change.
2. **Transformation pipelines** — rename 40 renders, convert every `.step`
   to a preview, export sheets to PDF. Data flows through a pipeline:
   select → transform → output.
3. **Rewindable everything** — designers iterate. Files and folders behave
   as immutable values, so history and undo are structural, not bolted on.
4. **Metadata as first-class data** — tags, relations ("this sheet
   references that model"), provenance, revision lineage.
5. **Batch operations with a plan step** — an agent proposes a plan
   (renames, moves, conversions); the human reviews the plan; the OS
   executes it. Plan/apply separation is a core idiom.
6. **Spatial arrangements** — views aren't only lists: grids, boards,
   canvases, timelines. A render layout is just another function output.
7. **Delegation** — the language is the interface through which agents act
   on the OS. Scripts are units of work an agent can be assigned.

---

## 3. Agent-First Design Principles

This is the research core. Human language design optimizes for terseness,
familiarity, and forgiveness. Agent-first design inverts the priorities:

### 3.1 Verbosity is free; ambiguity is expensive
Agents don't get tired typing. Every shortcut, every piece of sugar, every
"three ways to do it" is a decision point where an agent can hallucinate.
- Exactly one obvious way to do each thing.
- Explicit block terminators (`end`) — no significant whitespace.
- Explicit types at every definition boundary.
- No implicit conversions, no operator overloading, no globals.

### 3.2 The spec replaces training data
No model was trained on Patina. The language must be small enough that the
**complete spec fits in an agent's context window** alongside the task.
Smallness is the distribution strategy. Target: a spec of a few thousand
words that fully defines the language.

### 3.3 Compiler errors are a conversation with a machine
The consumer of an error message is the agent that will fix the code.
Errors must be:
- Structured (machine-readable, e.g. JSON mode alongside human text)
- Precisely located (file, line, column, span)
- Actionable (expected vs. found, plus a suggested fix where possible)

```
error[E0412]: type mismatch
  at: rename-renders.ptna, line 12, column 9
  expected: List File
  found:    List Folder
  hint: did you mean to call `files recursive` first?
```

### 3.4 Optimize for review, not authorship
The human's job is reading and approving agent output. So:
- Pipelines read top-to-bottom, mirroring data flow.
- A canonical formatter is built in from day one — one true layout, so
  diffs show only real changes.
- Names are encouraged to be long and descriptive; the agent types them,
  the human skims them.

### 3.5 Deterministic and sandboxable
Pure core, no hidden state, no ambient randomness or clock access — time
and entropy come in as explicit inputs. An agent can run a script and trust
the result. The OS can cache any pure result forever (memoization for
free).

### 3.6 The environment is introspectable
Agents hallucinate APIs. Defense: the runtime answers questions.
- `(doc files)` → machine-readable docs for `files`
- `(schema Folder)` → the exact shape of the Folder type
- `(search "rename")` → matching functions with signatures

The runtime *is* the documentation. The spec tells agents to query before
guessing.

---

## 4. Language Shape

### 4.1 Paradigm
Pure, functional, Miranda-inspired. Algebraic data types, pattern matching,
first-class functions. No mutation, no null, no exceptions-as-control-flow
(errors are values: `Result`).

### 4.2 Totality (Dhall-inspired)
The core language is **total**: all functions terminate. Recursion must be
structural (provably shrinking). Why: agent-written scripts run unattended
inside an OS. A script that can't hang is a safety feature, not a
limitation. General recursion, if ever needed, is an explicit effect with
a fuel budget.

### 4.3 Evaluation
- **v0: eager (strict).** Easy to implement, easy to reason about.
- **Later: lazy streams** for file-watching, folder history, and large
  collections — the Miranda soul, added deliberately rather than by
  default.

### 4.4 Types
- **v0: explicit annotations on every definition.** Agents don't mind
  writing them; they act as machine-checked documentation for reviewers.
- **v1: Hindley-Milner inference** within definition bodies; annotations
  still required at boundaries.
- Algebraic data types; `Maybe` and `Result` in the prelude.

### 4.5 Effects are explicit
Pure core, effect shell. Anything that touches the world — reading files,
writing, rendering, watching, calling out to tools — is typed as an
effect:

```
def apply-plan : Plan -> IO (Result Report)
```

Effects compose through explicit `do` blocks; pure code can never perform
them. The type signature tells the reviewer exactly what a script is
allowed to do.

### 4.6 Capability security
Agent scripts do not get ambient authority over the file system. They
receive **capabilities** as arguments: a read-only handle on one folder, a
write handle on an output directory. What a script can touch is visible in
its signature — crucial both for an OS and for running untrusted agent
code.

**Now specified, tier-aware** (Patina_v0_Spec §8.1–§8.7): capabilities are
indexed by SemOS's `SecurityTier` (`Cap Read Secret`, `Cap Write Public`),
data read through them is `Classified t a` and propagates its tier through
pure code, sinks admit only data at or below their tier (`to-llm` is
`Public`-only), and the sole downward flow is a console-gated, logged
`declassify`. The payoff: a `main` with no `Declassify` parameter is a
*static non-exfiltration proof*, welding the language type system to the
kernel's `current_task_max_tier()` fence instead of leaving an opaque gap
between them.

### 4.7 Modules: content-addressed (the Unison idea) — research fork
Definitions identified by the hash of their content; names are metadata
over the codebase database, not the identity of code.
- No dependency/version conflicts, ever.
- Renaming is free and doesn't break callers.
- Agents can share and query a codebase DB directly — arguably the most
  agent-native module system that exists.
- Cost: bigger build. Fork decision: file-based modules in v0,
  content-addressed store as a later experiment.

### 4.8 The pipeline idiom
`|>` is the primary composition operator. Scripts read as data flow:

```
def recent-work (folder : Folder) : Render
    folder
      |> files recursive
      |> where (modified-within (days 7))
      |> sort-by modified Descending
      |> take 20
      |> render grid
end
```

---

## 5. The SemOS Object Model

The language's standard library is the OS's data model. Core types:

```
type File     = { path, name, kind, metadata, revision, modified, ... }
type Folder   = { entries, metadata, view }        -- a typed native object
type Metadata = Map Key Value                       -- tags, relations, provenance
type Revision = { id, parent, timestamp, author }
type Render   = Grid ... | Board ... | List ... | Canvas ...
type Plan     = List Operation                      -- proposed, reviewable
type Operation = Rename ... | Move ... | Convert ... | Tag ...
```

The plan/apply idiom, used constantly by agents:

```
def organize-renders (folder : Folder) : Plan
    folder
      |> files
      |> where (kind-is "render")
      |> map propose-rename
      |> plan
end

-- reviewed by the human, then:
def execute : Plan -> IO (Result Report)
```

---

## 6. Grammar (v0 — decided)

Locked decisions: **no infix operators except `|>`** (no precedence
table exists); **kebab-case** value identifiers; UpperCamel type names;
explicit `end` terminators; parameters declared in the `def` header;
application by juxtaposition, parens for grouping only.

### 6.1 Lexical rules

- Comments: `--` to end of line.
- Value identifiers: `lower (lower|digit)* ("-" (lower|digit)+)*` —
  hyphens are always *inside* a name; names never start or end with
  one.
- Type names: UpperCamel. Type variables: lowercase.
- **Negative literals:** a `-` immediately followed by a digit is a
  number literal (`-5`). Since identifiers can never begin with `-`,
  this is unambiguous. A `-` followed by a letter is a lexer error.
- Text: `"..."` with escapes `\"` `\\` `\n` `\t`. Interpolation is
  restricted to names and field paths — `"sheet {sheet.name}"` — and
  interpolated values must be `Text` (call `to-text` explicitly; no
  implicit conversion, principle 3.1).
- Keywords (the complete list): `def end type match let do true false`.
- Trailing commas are allowed in multi-line records and lists
  (diff-friendly).

### 6.2 Grammar (complete v0 EBNF)

```
program    := (def | type-def)*

def        := "def" ident param* ":" type NEWLINE block
param      := "(" ident ":" type ")"
block      := ("let" ident "=" expr NEWLINE)* expr "end"

type-def   := "type" tname tvar* "=" NEWLINE ("|" ctor type*)+
            | "type" tname tvar* "=" record-type

expr       := app ("|>" app)*
app        := atom+
atom       := literal
            | ident ("." ident)*
            | "(" expr ")"
            | lambda
            | match-expr
            | do-block
            | record
            | list

lambda     := "\" ident+ "->" expr
match-expr := "match" expr NEWLINE ("|" pattern "->" expr NEWLINE)+ "end"
do-block   := "do" NEWLINE (stmt NEWLINE)+ "end"
stmt       := "let" ident "=" expr | expr
record     := "{" (ident "=" expr ("," ident "=" expr)* ","?)? "}"
list       := "[" (expr ("," expr)* ","?)? "]"

pattern    := "_" | literal | ident | ctor pattern*
            | "[" "]" | "[" pattern "," "..." ident "]"
            | "{" (ident ("," ident)*)? "}"

type       := atype ("->" atype)*
atype      := btype+
btype      := tname | tvar | "(" type ")" | record-type
record-type:= "{" (ident ":" type ("," ident ":" type)* ","?)? "}"

literal    := int | float | text | "true" | "false"
```

### 6.3 Desugaring — rich syntax, poor AST

- `x |> f a b` ≡ `f a b x` — the piped value becomes the LAST argument
- `def f (a : A) (b : B) : C ... end` ≡ `f : A -> B -> C`
- `\x y -> e` ≡ `\x -> \y -> e`

The AST has no pipe node and no def-header node. `|>` is surface
syntax only; the core language is application, lambda, let, match.

### 6.4 Static rules (v0)

- **No self-reference:** a `def` may not mention its own name.
  Repetition happens through prelude combinators (`map`, `fold`,
  `where`) — the simplest possible totality check. Structural
  recursion returns when the checker can prove shrinking (v1+).
- `match` must be exhaustive; the typechecker lists the missing cases.
- Every `def` is fully annotated; no inference in v0.
- Precedence, in full: field access (part of an atom), then
  application, then `|>`. There is nothing else.

### 6.5 The one true layout (formatter canon)

- 4-space indent; body begins on the line after the `def` header.
- Pipelines with 2+ stages: one stage per line, `|>` aligned.
- `match` branches one per line, `|` aligned; `end` on its own line.
- Records and lists: one line if ≤ 80 columns, else one item per line
  with trailing comma.
- One blank line between top-level definitions.

The formatter is part of the spec; agents are required to emit this
layout (principle 3.4).

---

## 7. Open Research Questions

1. **Laziness** — Miranda's soul. Add as opt-in streams (file watching,
   history traversal) rather than default evaluation?
2. **Type inference scope** — when is Hindley-Milner worth it? (v1)
3. **Effect model** — simple `IO` tag (v0) vs. effect rows/algebraic
   effects (later, research-grade)?
4. **Totality checking** — structural recursion only in v0; how painful is
   that in practice for OS scripting?
5. **Content-addressed codebase** — file modules (v0) vs. Unison-style
   hash-addressed store (the big agent-native bet)?
6. **Host boundary** — how much of Rust is exposed, and through what FFI?
   (JSON values across the boundary keeps it simple.)
7. **Concurrency** — file events, parallel pipelines. Explicit async or
   pure dataflow?
8. **Units as types** — phantom-typed `Quantity` vs. full F#-style
   dimension analysis? How much checking is worth the complexity?
9. **Conversion graph** — how does path-finding handle lossy hops,
   cost, and converter versioning?
10. **Store schema** — what is the *minimal* schema (blobs, metadata,
    relations, event log) that supports views without WinFS-ing
    ourselves?

---

## 8. Prior Art to Steal From

| Source | What to take |
|---|---|
| **Miranda** | Purity, algebraic types, the aesthetic of mathematical cleanliness |
| **Dhall** | Totality as a feature; "can't hang, can't crash" scripting |
| **Nix** | Pure functions describing a system; laziness where it pays |
| **jq** | The pipeline idiom as the whole language; transform-as-query |
| **Unison** | Content-addressed code; codebase as queryable database |
| **Gleam** | A small, friendly, Rust-implemented functional language to read end-to-end |
| **Roc** | Platform/app split — SemOS is the platform, scripts are apps |
| **F# units of measure** | Compile-time dimensional analysis for physical quantities |
| **Pandoc** | The conversion-graph model: many formats, path-find between them |
| **BeOS (BeFS)** | Live queries that appear as folders — shipped, 1990s, proof it works |
| **Plan 9** | "Everything is a file" composes at OS scale |
| **WinFS** | Cautionary tale: relational-filesystem ambition, shipped nothing — keep the store dumb |

---

## 9. v0 Scope (the weekend interpreter)

Written in Rust, tree-walking, no cleverness:

- [ ] Values: int, float, text, bool, list, record, ADT
- [ ] `def` with mandatory type annotation, explicit `end`
- [ ] Pattern matching via `match`
- [ ] Pipeline operator `|>`
- [ ] Pure prelude: map, where/filter, sort-by, take, fold
- [ ] `IO` effect tag (no inference, just tagged)
- [ ] Canonical formatter (AST → text, one true layout)
- [ ] Structured JSON error mode alongside human-readable errors
- [ ] Introspection builtins: `(doc x)`, `(schema T)`, `(search text)`

Explicitly NOT in v0: type inference, laziness, modules beyond single
files, concurrency, FFI beyond JSON.

---

## 10. OS Modules

Three modules extend Patina from a scripting language into the OS's
nervous system. Each is a *library with a schema*, not a language
change — the small-core principle survives.

### 10.1 Quantities and Units (temp calculations)

Sensor data, battery cutoff voltages, buck-converter heat, panel
dimensions — the OS constantly touches physical quantities, and untyped
numbers are how unit bugs happen. Patina makes units part of the type:

```
read-sensor : Sensor -> IO (Quantity Temperature)
lvd-cutoff  : Quantity Voltage -> Quantity Voltage -> Bool
panel-fits  : Quantity Length -> Quantity Length -> Bool
```

Key design point — **temperature is the canonical trap**: a
*temperature* (20 °C) and a *temperature difference* (20 °C warmer) are
different things. Adding two temperatures is meaningless; adding a
delta to a temperature is fine. So the type system distinguishes them:

```
Temperature        -- affine: a point on the scale
TemperatureDelta   -- linear: a difference

add  : Temperature -> TemperatureDelta -> Temperature
diff : Temperature -> Temperature -> TemperatureDelta
-- add : Temperature -> Temperature -> ???   (type error, correctly)
```

Conversions (°C ↔ °F ↔ K, mm ↔ inch) are explicit total functions. No
implicit conversion anywhere — principle 3.1 applies to physics.
Units-in-types turns the compiler into a physics checker and deletes
the Mars Climate Orbiter bug class, which matters when agents write
the math.

### 10.2 Type Translations

Two senses, both first-class:

**Representation translation** — file-format conversion as typed,
composable functions:

```
step-to-mesh    : File Step -> IO (Result (File Mesh))
mesh-to-preview : File Mesh -> IO (Result (File Preview))
```

Registered converters form a **conversion graph**: the OS path-finds
from the type you have to the type you need (STEP → mesh → preview).
Every translation records provenance in metadata ("this preview came
from v7.4 of the model"), feeding the relation graph in 10.3. Pure
translators cache forever (principle 3.5); effectful ones (shelling
out to FreeCAD) are explicit `IO`.

**Schema migration** — when a type evolves (Folder v1 → v2), the
migration is an explicit function stored *with the type*. Old data is
never silently rewritten; it is viewed through the migration. Types
have lineage, same as files have revisions. Agents write migrations;
humans review them like any other plan.

### 10.3 The Database IS the Filesystem

The enhanced folder system already *is* a database wearing a
filesystem's clothes:

| Filesystem concept | Database concept |
|---|---|
| Folder | Table / collection |
| File | Row / record |
| Metadata | Columns |
| View | Live query |
| Tags / relations | Indexes / foreign keys |
| Revision history | Event log / time travel |

So invert the usual architecture. Tradition bolts a database onto
files; SemOS makes the **content-addressed store the source of truth
and the filesystem a rendered view of it**. A folder can be
*materialized* (real entries) or *virtual* (a live query result — a
smart folder). Same interface, two implementations.

Precedent, good and cautionary:

- **BeOS (BeFS)** shipped live queries that appeared as folders in the
  1990s — the direct ancestor of SemOS views. Proof it works.
- **Plan 9** showed "everything is a file" composes beautifully at OS
  scale.
- **WinFS** (Microsoft's relational filesystem) is the cautionary
  tale: schema-everything ambition, shipped nothing. Lesson: keep the
  store dumb (content-addressed blobs + metadata + relations), put the
  smarts in views.

Consistency model stays boring on purpose: single writer, immutable
revisions, folder state as a fold over an event log. Time-travel
queries ("the project folder as of Tuesday") fall out of immutability
for free — capability 2.3 (rewindable everything) implemented once,
at the store layer.

For agents this is the natural interface: structured queries in,
structured rows out — no parsing directory listings, no scraping.
Introspection (3.6) extends to the store: `(schema Folder)`,
`(relations file)` are runtime calls.

### 10.4 Where's the Query Language? (There Isn't One)

SemOS does **not** get a separate database/query language. The SQL
world maintains two languages — one for data, one for logic — plus a
permanent glue layer between them (ORMs, query builders, mappers).
That glue exists because the database speaks one value model
(relations, rows, NULL) and the program speaks another (types,
functions, records). A "database language" as a separate thing is a
symptom of that mismatch.

SemOS avoids it structurally: the store holds Patina values, so one
type system runs from disk to screen.

- A **query** is a pipeline: `map` / `where` / `sort-by` / `fold` over
  lists of records *is* the query language (the LINQ insight — queries
  as library calls, not syntax; also jq, also Datomic's
  queries-as-data).
- A **schema** is Patina type definitions. No DDL.
- A **migration** is a Patina function (10.2).
- A **view** is a function `Folder -> Render`.

The one place a database-language-like thing legitimately exists is
beneath the surface: **pushdown**. A restricted subset of Patina —
pure, first-order predicates over record fields — can be recognized by
the store and executed against indexes instead of pulling every file
into the interpreter. Syntactically one language; semantically, some
functions are store-executable. The optimizer is invisible: same code,
faster. That is the right kind of database language — not one anyone
writes, but one the store understands.

Roadmap consequence: this adds *nothing* to the v0 language. All
database work lives in the store layer and the standard library.

---

*This document is the exploration space, not the spec. The spec (v0
grammar + semantics, small enough to fit in an agent's context) is the
next deliverable.*
