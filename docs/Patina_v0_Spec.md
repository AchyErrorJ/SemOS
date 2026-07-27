# Patina v0 — Language Specification

This document **fully defines** Patina v0. It is written to be pasted,
whole, into an AI agent's context before the agent reads or writes
Patina code. If something is not defined here, it does not exist in
v0 — do not invent it.

Patina is the scripting language of SemOS: pure, functional, total,
eager, explicitly typed, authored primarily by AI agents and reviewed
by humans. Files use the extension `.ptna`, UTF-8 encoded. (Not
`.pat` — that extension belongs to CAD hatch-pattern files.)

---

## 1. Design invariants

- **One way to do each thing.** No sugar with two spellings, no
  optional forms.
- **Explicit over implicit.** No implicit conversions, no inference,
  no ambient state.
- **Total.** Every program terminates (see §7.5).
- **Pure core, effect shell.** Effects exist only inside `IO` (§8).
- **Errors are structured data** (§10), because their primary reader
  is an agent that will fix the code.

---

## 2. Lexical rules

### 2.1 Comments
`--` begins a comment, ending at the newline. No block comments.

### 2.2 Identifiers
- **Value identifiers** are kebab-case:
  `lower (lower|digit)* ("-" (lower|digit)+)*`
  Hyphens are always *inside* a name; names never start or end with
  one. Examples: `sort-by`, `has-tag`, `x`, `v2-sheet`.
- **Type names and constructors** are UpperCamel: `List`, `Ok`,
  `File`.
- **Type variables** are lowercase words: `a`, `b`, `result`.

### 2.3 Keywords (complete list)
`def` `end` `type` `match` `let` `do` `true` `false`

Keywords cannot be used as identifiers.

### 2.4 Literals
- **Int:** `digit+`, optionally preceded by `-`. A `-` immediately
  followed by a digit is part of a number literal. Since identifiers
  can never begin with `-`, this is unambiguous; a `-` followed by a
  letter is a **lexer error**.
- **Float:** `digit+ "." digit+`, optional leading `-`. No scientific
  notation.
- **Text:** `"..."` with escapes `\"` `\\` `\n` `\t`. Interpolation:
  `{name}` or `{name.field}`. Only names and field paths may be
  interpolated — never arbitrary expressions — and interpolated values
  must have type `Text` (use `to-text` explicitly).
- **Bool:** `true`, `false`.

### 2.5 Punctuation (complete list)
`(` `)` `[` `]` `{` `}` `,` `:` `=` `.` `|` `\` `->` `|>`

### 2.6 Whitespace
Whitespace is insignificant except as a token separator. Blocks are
delimited by keywords (`end`), never by indentation.

---

## 3. Grammar (complete EBNF)

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

Precedence, in full: field access (inside an atom), then application
(juxtaposition, left-associative), then `|>`. **There is nothing
else.** No arithmetic operators exist; arithmetic is function
application (§9).

---

## 4. Desugaring

Sugar is surface syntax that the parser rewrites into the core before
typechecking. The evaluator never sees it.

| Surface | Core |
|---|---|
| `x |> f a b` | `f a b x` (piped value = LAST argument) |
| `def f (a : A) (b : B) : C ... end` | `f : A -> B -> C` |
| `\x y -> e` | `\x -> \y -> e` |
| `"...{x}..."` | `text-concat ["...", x, "..."]` |
| record pattern `{name, rev}` | binds `name` and `rev` from the record |

The core AST has exactly four expression forms: **application,
lambda, let, match** (plus literals, names, records, lists, do).

---

## 5. Types

### 5.1 Built-in types
`Int`, `Float`, `Text`, `Bool`, `Unit` (single value: `unit`),
`List a`, `Maybe a`, `Result a`, function types `A -> B`, record
types `{field : T, ...}`. Host-defined built-in types for effects and
security (§8): `Path`, the capability `Cap mode tier`, the classified
value `Classified tier a`, the access modes `Read` / `Write`, the four
tier tags `Public` / `Internal` / `Sensitive` / `Secret`, and the
`Declassify` capability.

```
type Maybe a  = | Some a
                | None

type Result a = | Ok a
                | Err Text
```

### 5.2 User types
`type-def` declares an ADT (`type Order = | Ascending | Descending`)
or a record alias (`type Point = {x : Float, y : Float}`). Type
parameters follow the name: `type Pair a b = | Pair a b`.

### 5.3 IO
`IO a` is the type of *descriptions of effects*. An `IO a` value does
nothing by itself; only the host performs it (§8). There is no way
to extract an `a` from an `IO a` except inside a `do` block.

### 5.4 Rules
- Every `def` is fully annotated. No type inference in v0.
- No subtyping. No null. No implicit conversion of any kind.
- No operator overloading and no user-defined overloading. The only
  polymorphic names are the builtins marked ◊ (the prelude in §9 and the
  classification builtins in §8.3).
- No subtyping — including the tier lattice (§8.1). Tiers never coerce
  implicitly; every tier change is an explicit `raise` or `declassify`.

---

## 6. Static rules

The typechecker rejects a program violating any of these, with a
structured error (§10):

1. **No self-reference.** A `def` may not mention its own name,
   directly or mutually. Repetition happens through prelude
   combinators (`map`, `fold`, `where`).
2. **Exhaustive match.** Every `match` must cover all constructors of
   the scrutinee's type (a wildcard `_` or a binding pattern covers
   the rest). The error lists the missing cases.
3. **Scope.** Names are resolved lexically. Every name must be bound
   (parameter, `let`, pattern, or prelude). Later `let` bindings may
   shadow earlier ones.
4. **Annotation agreement.** The body's type must equal the declared
   type of the `def`.
5. **`let` exists only at `def` block level.** Match branches and
   lambda bodies are single expressions. The idiom: extract a named
   helper `def` rather than nesting bindings. Small named defs are
   more reviewable — this is deliberate.

---

## 7. Evaluation

### 7.1 Values
A value is: a literal, a **closure** (lambda + captured environment),
a constructor applied to values, a record of values, a list of
values, or an opaque `IO` description.

### 7.2 Environments
An environment is an immutable mapping from names to values.
Definitions, parameters, `let` bindings, and pattern bindings extend
the environment; nothing ever mutates it.

### 7.3 Eager (call-by-value)
- In `f x`: evaluate `f`, then `x`, then apply. Left to right.
- Applying a closure: bind parameters to argument values in the
  closure's captured environment, evaluate the body.
- `let x = e`: evaluate `e` to a value, bind `x`, continue.
- `match s`: evaluate `s`; try branches **top to bottom**; take the
  first whose pattern matches; bind its variables; evaluate its
  expression. Exhaustiveness (§6.2) guarantees one always matches.
- Records and lists evaluate their fields/elements left to right.

### 7.4 do blocks
Inside `do`, statements are performed in order. A statement of type
`IO a` is *performed*; `let x = action` binds the produced `a`;
pure statements are evaluated and discarded. The final statement
must have type `IO a`; the block then has type `IO a`.

### 7.5 Termination
v0 has no self-reference (§6.1) and every prelude combinator is
structurally decreasing on its list argument. Therefore every Patina
v0 program terminates. This is a guarantee, not a hope.

---

## 8. Effects and the host

- A **program** is a single `.pat` file: a set of `def`s. There is
  no `import` in v0.
- The host runs a program by calling the `def` named `main`. `main`
  may declare parameters; the host supplies them (capabilities,
  arguments). Applying `main` to those arguments must yield a value
  of type `IO a`, which the host then performs. Example:
  `main (cap : CapR) (path : Text) : IO Unit`.
- The pure core **cannot perform effects**. `IO` values are built by
  host builtins and threaded through `do`; the interpreter treats
  them as opaque.
- **Capabilities are tier-indexed.** The host passes opaque capability
  values as arguments to `main`; Patina cannot forge them and has no
  ambient authority. Every capability carries an **access mode** and a
  **tier** (§8.1): `Cap Read Sensitive`, `Cap Write Public`. Both are
  visible in every signature that uses the capability, so a reviewer
  reads a script's authority — and its blast radius — straight off its
  type.
- Host builtins provided in v0:

```
print       : Text -> IO Unit
read-text   : Cap Read t  -> Path -> IO (Result (Classified t Text))
write-text  : Cap Write t -> Path -> Classified s Text -> IO (Result Unit)   -- requires s ⊑ t
to-llm      : Classified Public Text -> IO (Result Text)                     -- Public only
```

`Path`, `Cap`, `Classified`, the access modes `Read` / `Write`, and the
four tier tags are host-defined built-in type names. SemOS library
functions — folders, views, plans, converters — build on these and are
specified separately; they are not part of the language.

### 8.1 Tiers

There are exactly four tiers, a closed set, ordered least- to
most-restricted:

```
Public  ⊑  Internal  ⊑  Sensitive  ⊑  Secret
```

They are **type-level tags**: they appear only as type arguments
(`Cap Read Secret`, `Classified Public Text`); there are no *values* of
these types, and no way to construct, compare, or pattern-match one at
run time. `⊑` reads "may flow to". Data flows **up** the lattice for
free (§8.3, `raise`); it flows **down** only through `declassify`
(§8.5). The four tags are SemOS's `SecurityTier` (§8.7).

### 8.2 Classified data

`Classified t a` is a value of type `a` carrying a tier watermark `t`.
It is produced by reading through a capability (§8, `read-text`) and by
`public` (§8.3). **The `a` cannot be extracted in pure code** — a
classified value leaves only through a sink whose tier admits it (§8.4)
or through `declassify` (§8.5). That is the whole containment: high-tier
bytes cannot reach a low-tier destination without a visible, gated call.

### 8.3 Propagation (all explicit — no implicit coercion)

Names marked ◊ are polymorphic builtins.

```
public          : a -> Classified Public a                              ◊
raise           : Classified s a -> Classified t a                      ◊   -- valid iff s ⊑ t
map-classified  : (a -> b) -> Classified t a -> Classified t b          ◊
map2-classified : (a -> b -> c)
                  -> Classified t a -> Classified t b -> Classified t c ◊
```

- Plain values enter classification at `Public` via `public`.
- `raise` moves a watermark **up** the lattice only; a downward `raise`
  is a type error (E0431) — use `declassify`.
- Pure computation rides under the watermark: `map-classified` preserves
  the tier, so any transformation of `Secret` data is still `Secret`.
- `map2-classified` combines two values **at the same tier**. To combine
  different tiers, `raise` the lower one to match first — the explicit
  join. Mismatched tiers are E0432; there is no automatic join.

### 8.4 Sinks

A sink is an effect that consumes classified data. Each sink declares
the tier it admits; the typechecker rejects data above it.

- `write-text : Cap Write t -> … -> Classified s Text -> …` admits `s`
  only when `s ⊑ t`. A `Cap Write Public` writes only `Public` data; a
  `Cap Write Secret` writes anything. Persisting `Secret` data therefore
  requires a `Secret`-cleared destination.
- `to-llm : Classified Public Text -> …` admits **only** `Public` —
  `Secret ⇒ no LLM`, enforced statically, matching the tier's meaning.
- Any external or networked effect a SemOS library adds must declare its
  admitted tier the same way.

Sending data above a sink's tier is E0430.

### 8.5 Declassification (the one downward door)

```
declassify : Declassify -> Classified s a -> Classified Public a        ◊
```

`declassify` is the only way data moves down the lattice. It requires a
`Declassify` capability, which the host issues **only under human
authorization at the console** — the same gate as `vouch` / `pair`
(§8.7). Every call is recorded in the provenance / audit log with the
source tier and call site. An agent cannot obtain `Declassify` on its
own, and the human sees every declassification.

Consequently: **if a script's `main` has no `Declassify` parameter, the
typechecker guarantees no data read at a higher tier reaches a
lower-tier sink.** The proof is the signature — nothing else need be
audited to know a script cannot exfiltrate.

### 8.6 Typing rules

- `⊑` is a fixed relation over the closed four-element set (§8.1),
  consulted by the typing rules of `raise`, `write-text`, `to-llm`, and
  `declassify`. It is **not subtyping**: values never coerce implicitly,
  every tier change is an explicit call, and §5.4 (no subtyping) still
  holds. The relation is total and decided by table lookup.
- Tiers, like all types in v0, are written explicitly at every `def` and
  `main` boundary. There is no tier inference.

Example — a reviewer can read the fence off the header:

```
-- reads Secret, writes only to a Public sink, no Declassify granted:
-- the typechecker rejects any path from the Secret data to `out`.
def redact-report (src : Cap Read Secret) (out : Cap Write Public) : IO (Result Unit)
    ...
end
```

### 8.7 Host correspondence (SemOS)

- The four tiers are SemOS's `SecurityTier`: `Public 0`, `Internal 1`,
  `Sensitive 2`, `Secret 3`.
- The host issues a task only capabilities within its
  `current_task_max_tier()`. A tier-0 (auto-sandboxed) agent gets
  `Public` capabilities: it can reach `to-llm`, but can never obtain a
  `Cap Read Secret` in the first place.
- `read-text` additionally fails at run time (E0501) if the object's
  stored tier exceeds the capability's tier — the static watermark is a
  conservative over-approximation, the runtime check is exact.
- `Declassify` issuance and every `declassify` call route through the
  interactive-console authority and the audit log — the imperative twin
  of the plan/apply review idiom, and the same console gate that guards
  `SYS_VOUCH` / `SYS_PAIR`.

---

## 9. Prelude (v0, complete)

Names marked ◊ are polymorphic builtins — the only polymorphic names
in v0.

### Lists
```
map      : (a -> b) -> List a -> List b
where    : (a -> Bool) -> List a -> List a      -- keep matching
fold     : (b -> a -> b) -> b -> List a -> b
take     : Int -> List a -> List a
drop     : Int -> List a -> List a
length   : List a -> Int
reverse  : List a -> List a
concat   : List a -> List a -> List a
sort-by  : (a -> b) -> Order -> List a -> List a   -- b: Int|Float|Text
```
`type Order = | Ascending | Descending`

### Arithmetic (Int)
```
add  : Int -> Int -> Int
sub  : Int -> Int -> Int
mult : Int -> Int -> Int
div  : Int -> Int -> Result Int    -- Err on division by zero
mod  : Int -> Int -> Result Int    -- Err on modulus by zero
```

### Arithmetic (Float)
```
fadd  : Float -> Float -> Float
fsub  : Float -> Float -> Float
fmult : Float -> Float -> Float
fdiv  : Float -> Float -> Result Float
```

### Comparison ◊ (defined on Int, Float, Text, Bool only)
```
eq   : a -> a -> Bool
lt   : a -> a -> Bool
lte  : a -> a -> Bool
gt   : a -> a -> Bool
gte  : a -> a -> Bool
```

### Bool
```
and  : Bool -> Bool -> Bool
or   : Bool -> Bool -> Bool
not  : Bool -> Bool
```

### Text
```
to-text     : a -> Text        -- ◊ defined on Int, Float, Text, Bool
text-concat : List Text -> Text
```

### Maybe / Result
```
maybe : b -> (a -> b) -> Maybe a -> b
```

### IO
```
map-io   : (a -> IO b) -> List a -> IO (List b)
for-each : (a -> IO Unit) -> List a -> IO Unit
```

Anything not listed here does not exist in v0.

---

## 10. Error contract

Errors have two forms — a human form and a JSON form — carrying
identical information. The JSON form is the contract; agents consume
it to fix code.

### 10.1 Human form
```
error[E0412]: type mismatch
  at: rename-renders.ptna, line 12, column 9
  expected: List File
  found:    List Folder
  hint: did you mean to call `files recursive` first?
```

### 10.2 JSON form
```json
{
  "code": "E0412",
  "kind": "type-mismatch",
  "phase": "typecheck",
  "file": "rename-renders.ptna",
  "span": {"line": 12, "col": 9, "end-line": 12, "end-col": 18},
  "expected": "List File",
  "found": "List Folder",
  "hint": "did you mean to call `files recursive` first?"
}
```

Fields `expected`, `found`, `hint` are optional; all others are
mandatory. `phase` is one of `lex | parse | static | typecheck |
eval`.

### 10.3 Codes (v0)
| Code | Phase | Meaning |
|---|---|---|
| E0101 | lex | unexpected character |
| E0102 | lex | unterminated text literal |
| E0103 | lex | `-` followed by a letter |
| E0201 | parse | expected `end` |
| E0202 | parse | unexpected token |
| E0301 | static | duplicate definition |
| E0302 | static | self-reference |
| E0401 | typecheck | unbound name |
| E0412 | typecheck | type mismatch |
| E0420 | typecheck | non-exhaustive match |
| E0430 | typecheck | classified data sent to a lower-tier sink (§8.4) |
| E0431 | typecheck | illegal downward `raise` — use `declassify` (§8.3) |
| E0432 | typecheck | tier mismatch in a same-tier combiner — `raise` to align (§8.3) |
| E0501 | eval | host effect failed (incl. read refused / `declassify` denied, §8.7) |

---

## 11. Canonical layout (mandatory)

Agents MUST emit exactly this layout; the formatter defines it and
can normalize any valid program to it.

- 4-space indent; body begins on the line after the `def` header.
- Pipelines with 2+ stages: one stage per line, `|>` aligned.
- `match` branches one per line, `|` aligned; `end` on its own line.
- Records and lists: one line if ≤ 80 columns, else one item per
  line with a trailing comma.
- One blank line between top-level definitions.

---

## 12. Explicitly not in v0

Type inference (including tier inference) · recursion of any kind ·
laziness · imports/modules beyond one file · concurrency · operator
overloading · user-defined infix · exceptions · mutation · general
string interpolation · FFI beyond host builtins · field-accessor
functions (write `(\f -> f.modified)`) · plain type aliases (use `Text`
and friends directly).

Tiered-capability refinements deferred past v0: finer-grained
`declassify` (v0 declassifies only to `Public`) · implicit label joins
(v0 requires an explicit `raise` to combine tiers) · per-object static
tiers (v0 watermarks a read at the *capability's* tier, not the stored
object's — the runtime check narrows it) · declassification policies
richer than a single console-issued `Declassify` capability.

These are roadmap items (see the design space document), not gaps to
work around. If a task seems to require one, the task should be
expressed with the prelude — or escalated to the human.

---

*v0.0.2 — tiered capabilities (§8.1–§8.7): capabilities and sinks are
tier-indexed, classification propagates through pure code, `to-llm` is
`Public`-only, and the sole downward flow is a console-gated, logged
`declassify`. A `main` without a `Declassify` parameter is a static
non-exfiltration proof. Reflects SemOS's `SecurityTier` and the
`current_task_max_tier()` fence. New error codes E0430–E0432.*

*v0.0.1 — patches from the five-script field test: §6.5 let-idiom
documented, §7.4 do-block typing tightened, §8 main signature with
host-supplied arguments, §12 deferrals recorded (field accessors,
type aliases).*
