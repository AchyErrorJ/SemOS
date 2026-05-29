# MarlOS typesetter — Path A (HTML-out only) port plan

Date: 2026-05-29
Target: a SemOS Ring-3 user program that reads a `book.toml` + its
markdown files from the SemOS FS and writes a single self-contained
HTML file to the SemOS FS. No PDF, no EPUB, no font embedding, no
subprocess (pandoc), no headless Chromium.

Source under analysis: `F:\Software\ArmKernel3\docs\marlos-typesetter-source\`
(copied from MarlOS `src-tauri/src/typesetter/` so the agent sandbox can
read it).

Scope cut from the original MarlOS module:

| MarlOS module      | Path A action                              |
| ------------------ | ------------------------------------------ |
| `mod.rs`           | port (slimmed re-export list)              |
| `book_config.rs`   | port + tiny TOML reader replacement        |
| `citations.rs`     | port + regex replacement                   |
| `structure.rs`     | rewrite (kill scraper/html5ever + regex)   |
| `pandoc.rs`        | DROP — replaced by in-process pulldown-cmark |
| `epub_export.rs`   | DROP for Path A                            |
| `pdf_export.rs`    | DROP for Path A                            |

The new Ring-3 binary `user-programs/marlos-typeset/` is the deliverable.
Output is one HTML file: front-matter (title, copyright, dedication, ToC)
+ body sections + back-matter (notes, list of figures, acknowledgements).
A separate stylesheet `.css` file ships next to the HTML; the writer
opens both in whatever browser they have once we have one. Inline `<style>`
is also acceptable for V1 since there's no SemOS browser yet.

---

## Section 1 — Per-module verdict

### 1.1 `book_config.rs` — book.toml loader/saver

**Role:** Parse `book.toml` into a `BookConfig` struct; serialize a
default starter. Owns the trim/typography/export settings that drive the
CSS template downstream.

**Crate deps (from `use` lines in book_config.rs):**

- `std::path::{Path, PathBuf}` (`book_config.rs:28`)
- `serde::{Deserialize, Serialize}` (`book_config.rs:30`)
- `thiserror::Error` derived on the error enum (`book_config.rs:32`)
- `toml` (referenced as `toml::from_str` at `book_config.rs:274`)
- `std::fs::read_to_string` / `std::fs::write` (`book_config.rs:273, 438, 490`)

**Std uses (file:line):**

- `std::path::Path` / `PathBuf` — every signature uses them
  (`book_config.rs:28, 244, 271, 305, 445`)
- `std::fs::read_to_string` (`book_config.rs:273`) — semos-std `fs::read_to_string` is a drop-in (`std-shim/src/fs.rs:170`)
- `std::fs::write` (`book_config.rs:438, 490`) — semos-std `fs::write` drop-in (`fs.rs:178`)
- `path.is_file()` / `path.is_dir()` / `path.exists()`
  (`book_config.rs:245, 259, 264, 293, 450`) — **NOT in semos-std `path` today**
  (lexical-only; see `std-shim/src/path.rs:7-12` doc comment). Need
  either: (a) add `fs::metadata` to semos-std, or (b) try-open and treat
  `Err` as "no such file". Recommendation: **(a)**. Add a kernel
  `SYS_STAT` (already partly in `syscall/mod.rs`; check for an `exists`
  helper) and a `semos_std::fs::metadata(&str)` returning an
  `io::Result<Metadata>` with `is_file()` / `is_dir()`. Tracked as the
  one semos-std addition for stage 2.
- `path.extension().and_then(|s| s.to_str())` (`book_config.rs:247`) —
  semos-std `Path::extension(&self) -> Option<&str>` (`path.rs:71`) is a
  drop-in (already returns `&str`, not `&OsStr`).
- `PathBuf::display()` (`book_config.rs:297`) — semos-std `PathBuf` has
  `as_str()` (`path.rs:134`); rewrite `format!("{}", x.display())` →
  `x.as_str()`.
- `path.parent()` (`book_config.rs:255, 277, 446`) — semos-std
  `Path::parent` is a drop-in (`path.rs:40`).
- `path.join(...)` (`book_config.rs:256, 260, 292, 305, 449`) — semos-std
  `Path::join` is a drop-in (`path.rs:93`).
- `path.file_name() / file_stem()` (`book_config.rs:454, 458`) —
  drop-ins (`path.rs:57, 82`).

**Serde + thiserror:**

- `serde` itself works on no_std with `default-features = false`; the
  derive macros work unchanged. The kernel-x86_64 tree already imports
  serde (`Cargo.toml` of compiler crate uses it) but **not in Ring 3
  yet** — first user program to do so. The vendored serde 1.0.228 is
  already present at `F:\Software\ArmKernel3\compiler\vendor\serde-1.0.228`
  and `serde_derive-1.0.228`. Re-use the vendor dir or add as a normal
  `[dependencies]` entry; both should resolve to the same crate.
- `thiserror` is currently `2.x` in the MarlOS Cargo.toml. The no_std
  variant of thiserror 1.x (cached: `thiserror-1.0.69`) supports
  no_std. **Verdict:** drop thiserror; hand-write the small enums with
  `core::fmt::Display + Debug`. The typesetter only has ~5 error enums
  and they all have a thin Display anyway.

**Toml parser:**

- The crate `toml` (the high-level wrapper, top of the chain) **needs
  std** (depends on `toml_edit`, which uses `std::collections::HashMap`,
  `std::io`, etc.). Even with `default-features = false` the `de`
  module assumes std. Adding it to a no_std user program is going to
  pull in a lot.
- The MarlOS `book.toml` schema is small and flat:
  `[book]`, `[trim]`, `[typography]`, `[export]`, plus a top-level
  `files = [...]` array. Values are strings, ints, floats, bools, and
  inline tables for `margins_in`. **No nested arrays of tables, no
  array literals with multi-type values.**
- Recommendation: **hand-write a 200-300 LOC TOML subset** in
  `user-programs/marlos-typeset/src/toml_lite.rs` that parses exactly
  the subset MarlOS writes via `to_toml_string` (`book_config.rs:314`).
  Plus an even simpler emitter — MarlOS already hand-formats output;
  reuse that code unchanged once we have alloc strings.
- Alternative: **port `toml_edit` 0.22** with `default-features = false`
  and patch out `std::collections::HashMap` → `hashbrown::HashMap` (the
  semos-std flavor). Estimate: 1-2 sessions of patching upstream code;
  brittle. The hand-roll is faster, smaller, and matches our own writer
  output 1:1.

**Verdict:** `port clean` with three local changes:

1. Add `semos_std::fs::metadata` (~1 hour) for `is_file()`/`is_dir()` —
   or skip it and rely on "try open + treat MAX as not-present".
2. Replace `toml::from_str` with `toml_lite::from_str`.
3. Drop `thiserror`, hand-write `Display + Debug` on `BookConfigError`.

**Session estimate:** 1.0 session (TOML lite ~0.5 + porting itself ~0.3
+ wiring serde derive on no_std ~0.2).

---

### 1.2 `citations.rs` — citation transform + Unicode super/subscript + figure-class hoist

**Role:** Reads markdown text, replaces `[CITE: ...]` markers with
`<sup>` back-links, builds the `# Notes` back-matter section, warns on
`[VERIFY:]` markers. Plus two post-pandoc passes that work on rendered
HTML (`normalize_unicode_scripts`, `tag_math_anchors`, `hoist_figure_classes`).

**Crate deps:**

- `std::path::{Path, PathBuf}` (`citations.rs:13`)
- `std::sync::OnceLock` (`citations.rs:14`)
- `regex::Regex` (`citations.rs:16`)
- `std::env::temp_dir` (`citations.rs:56`)
- `std::fs::create_dir_all` / `read_to_string` / `write` / `remove_file`
  (`citations.rs:50, 57, 59, 68`)
- `uuid::Uuid` (`citations.rs:58`)
- `thiserror::Error` (`citations.rs:20`)

**Std uses (file:line):**

- `std::sync::OnceLock` — used as static lazy regex cache (lines 110-116,
  425, 576-577, 592-594, 611, 737). **Substitution:** semos-std lacks
  `OnceLock` (the `sync` module wraps a `Once` for one-time init —
  check `std-shim/src/sync.rs` to confirm). Two paths:
  - (a) add a thin `OnceLock<T>` to `semos_std::sync` (~30 LOC, a
    spinlock-guarded `Option<T>`).
  - (b) Eliminate the regex caching entirely — see "regex" below — and
    OnceLock becomes unnecessary.

  Recommendation: (b). With the regex replacement we don't have an
  expensive type to cache anymore; just call the parser inline.
- `std::env::temp_dir()` (`citations.rs:56`) — semos-std `env` has no
  `temp_dir`. We can use a fixed `/tmp/marlos-typeset/` path (the kernel
  FS has no temp namespace concept; just create the dir at startup).
  Or skip temp files entirely by passing the transformed markdown string
  in memory to the next stage (Path A doesn't need a pandoc subprocess
  anyway). **Recommendation:** skip temp files; `prepare_book_markdown`
  returns the `String` directly instead of a temp `PathBuf`. The
  function only writes a file because pandoc reads from disk —
  unnecessary for in-process pulldown-cmark.
- `std::fs::create_dir_all` (`citations.rs:57`) — semos-std has
  `fs::create_dir` (`fs.rs:184`); no `create_dir_all`. Trivial to write
  (split on `/`, create each prefix). Or moot if we skip temp files.

**Regex crate substitution:**

The regex uses are:

- `CITE_RE`: `r"(?s)\s?\[CITE:\s*(.+?)\]"` (`citations.rs:123`) — lazy
  group, dot-matches-newline.
- `VERIFY_RE`: `r"\[VERIFY:"` (`citations.rs:127`) — literal substring.
- `ANY_HEADING_RE`: `r"^(#{1,6})\s+(.+?)\s*$"` (`citations.rs:133`) —
  per-line, anchored.
- `CHAPTER_RE`: `r"(?i)^Chapter\s+(\d+|[ivxlcdm]+)\b"` (`citations.rs:138`).
- `INTERLUDE_RE`: `r"(?i)^Interlude\s+(\d+|[ivxlcdm]+)\b"` (`citations.rs:144`).
- `APPENDIX_RE`: `r"(?i)^Appendix\b"` (`citations.rs:149`).
- `APPENDIX_INSERT_RE`: `r"(?mi)^#\s+Appendix\b"` (`citations.rs:427`) —
  multiline mode used over the full output buffer.
- `RE` (math-anchor): `r#"<blockquote>(\s*<p><strong>Math Anchor)"#`
  (`citations.rs:578`) — substring with one capture.
- `FIG_RE` (figure-class hoist): `r#"(?s)(<figure\b)([^>]*)(>\s*<img\b)([^>]*?)(/?>)"#`
  (`citations.rs:596`).
- `CLASS_RE`: `r#"class="([^"]*)""#` (`citations.rs:597`).
- `STYLE_RE`: `r#"style="([^"]*)""#` (`citations.rs:598`).

All eleven patterns are **tiny + non-backtracking-pathological** —
classic anchored prefixes, simple alternations, one-character lookahead.
None use backreferences or look-around. They will work on `regex-lite`
unchanged (`regex-lite` parses the same syntax minus a few Unicode
classes and `(?P<name>...)` group syntax — but the captures here are all
unnamed, except `INTERLUDE_RE` and `CHAPTER_RE` in citations.rs use
unnamed groups, while `structure.rs` uses `(?P<num>...)` named groups
which is a portability concern handled separately below).

Substitution options:

- **`regex` (default) + `regex-syntax` + `regex-automata`:** the
  family is documented as no_std-compatible with `default-features =
  false` plus `features = ["std"] = []`. In practice the cached
  `regex-1.12.2` in this machine's registry advertises a `std` feature
  that gates `Error: std::error::Error`, and `unicode` features that
  pull in `regex-syntax` + `regex-automata` with their own no_std
  stories. Builds at opt-0 are SLOW (regex compiles `regex-automata`
  which is large). **Compile-time risk: high** at opt-0.
- **`regex-lite`** (separate crate, same author): pure no_std-with-alloc,
  ~6× smaller, no DFA codegen, no perf features. Supports the
  PCRE-subset syntax used here. **Recommendation: vendor regex-lite.**
  Use it everywhere `regex` is currently used.
- **Hand-roll byte-level scanners.** Pattern-by-pattern this works
  because the patterns are simple; but with 11 of them across two files
  it's not the cheapest path. Worth it for the 3-4 hottest patterns if
  regex-lite turns out flaky.

**Verdict:** `needs shim` — three substitutions:

1. `regex::Regex` → `regex_lite::Regex` (drop-in API for these patterns).
2. `OnceLock<Regex>` → call `Regex::new` inline (regex-lite compile is
   fast; the optimization isn't worth a OnceLock dependency).
3. `std::fs::*` + `std::env::temp_dir` → in-memory string (skip temp
   files entirely; `prepare_book_markdown` returns `(String,
   CitationTransformResult)`, not `(PathBuf, …)`).

**Session estimate:** 1.0 session (regex-lite vendoring ~0.3, the
substitutions + tests ~0.7).

---

### 1.3 `structure.rs` — section recognition + HTML enrichment + ToC + figure list + drop-cap wrapping

**Role:** Operates on pandoc's `<section class="level1">` HTML, classifies
each section as front/chapter/interlude/back matter, injects
`data-section-*` attributes, builds the generated front-matter (title,
copyright, dedication) + back-matter (acks, list of figures), wraps the
first-paragraph drop cap and lead-in span. This is the *single biggest
change* in the port.

**Crate deps:**

- `std::sync::OnceLock` (`structure.rs:21`)
- `chrono::Datelike` (`structure.rs:23`) — used only at line 516 for
  current year of the copyright page.
- `regex::Regex` (`structure.rs:24`)
- `scraper::{Html, Selector}` (`structure.rs:25`)
- `serde::{Deserialize, Serialize}` (`structure.rs:26`)
- `std::collections::HashMap` (`structure.rs:396`)
- `std::cell::RefCell` (`structure.rs:607`)

**Std uses (file:line):**

- `std::collections::HashMap` (`structure.rs:396`) — semos-std
  `collections::HashMap` (`collections.rs:13`) drops in.
- `std::cell::RefCell` (`structure.rs:607`) — `core::cell::RefCell` drops in.
- `chrono::Utc::now().year()` (`structure.rs:516`) — see Section 5.

**scraper / html5ever — the hard part:**

`structure.rs:247-273` `extract_top_level_sections` uses scraper's CSS
selector engine to:

```rust
let section_sel = Selector::parse(r#"section.level1"#).expect("selector");
let h1_sel = Selector::parse("h1").expect("h1 selector");
for section in doc.select(&section_sel) {
    let id = section.value().attr("id").unwrap_or("").to_string();
    let classes = section.value().attr("class").unwrap_or("").to_string();
    let data_header = section.value().attr("data-header")...;
    let h1 = section.select(&h1_sel).next()...
        .map(|h| h.text().collect::<Vec<_>>().join(""));
    ...
}
```

That is the **entire** scraper / html5ever surface used by the
typesetter. One pattern: "find every top-level `<section class*=level1>`,
read its `id`, `class`, `data-header` attrs, and the text content of
its first `<h1>` child."

**Decision: do NOT port scraper / html5ever.** Reasons:

- scraper + html5ever + markup5ever + tendril is ~30 KLOC; porting any
  of them to no_std + alloc is a multi-session effort each. tendril
  uses thread-locals; markup5ever has been historically std-only;
  html5ever pulls in mac and other procmacro-heavy deps. Not happening
  in a reasonable session count.
- The actual need is a 50-line linear scan.

**Replacement:** since Path A is generating the HTML ourselves from
pulldown-cmark in the same process, **we never need to parse pandoc-shaped
HTML at all**. Instead of "pandoc → HTML → re-parse → classify", do
**"pulldown-cmark events → classify directly during streaming → emit
HTML with attributes already present"**.

Concretely, replace the entire pipeline:

```
markdown → pandoc → HTML → scraper(section/h1) → BookStructure → enrich_html
```

with:

```
markdown → pulldown-cmark Parser → in-process event walker
                                      → BookStructure (built during walk)
                                      → emit_html (writes data-section-* up front)
```

The event walker needs to detect:

- `Event::Start(Tag::Heading { level: H1, .. })` — opens a new top-level
  section. Buffer the inner text until `Event::End(TagEnd::Heading(H1))`.
- The H1 text (just the plain text, joined) classifies the section via
  the same regexes (chapter / interlude / appendix prefix → section
  kind + number).
- Heading attributes (pulldown-cmark supports `{#id .class key=value}`
  attribute lists if the `heading_attributes` extension is enabled) carry
  the optional `data-header` running-header override.
- Slug generation for section IDs (the per-chapter ID format used by
  citations.rs and structure.rs differs slightly; pick one — the
  `derive_chapter_id` flavor from `citations.rs:199` is the canonical
  one).

Then emit HTML by walking events again (or buffer events the first time
and emit on the second pass): wrap each section in
`<section id="..." class="level1" data-section-type="..." data-section-number="..." data-section-title="...">`
and close at the next H1 boundary. pulldown-cmark's HTML writer
(`pulldown_cmark::html::push_html`) can be wrapped: we feed it events
section-by-section, inserting our `<section>` open/close tags between
sections.

**`enrich_html` (`structure.rs:394-427`) goes away** because the section
attributes are written at emit time, not patched in afterwards.

**`build_list_of_figures` (`structure.rs:606-655`) becomes a separate
post-pass** on the emitted HTML string. The regex it uses
(`structure.rs:614`) is doable in `regex-lite`, OR we can move
figure-numbering into the same event walker (count figures during the
walk; emit the `<figcaption>` with the number already in place). The
event-walker approach is the natural fit and removes another regex.

**`wrap_chapter_openers` + `wrap_opener_text` (`structure.rs:732-846`)
also go into the event walker** — emit `<span class="drop-cap">` and
`<span class="lead-in">` around the first text run of the first
paragraph of each chapter section, instead of regex-patching them in
afterwards. The current regex
`r"(?s)(<section [^>]*data-section-type=\"chapter\"[^>]*>.*?<p[^>]*>)([^<]+)"` is
a hack precisely because there's no good HTML AST; with the event stream
this is trivial.

**`build_toc` (`structure.rs:677-721`)** has zero HTML parsing — it walks
the already-built `BookStructure`. Ports as-is.

**`build_generated_front_matter` (`structure.rs:479-561`)** and
**`build_generated_back_matter` (`structure.rs:567-592`)** are pure
string assembly from `BookMeta`. Port as-is; the only blocker is
`chrono::Utc::now().year()` (Section 5).

**Verdict:** `replace` — `extract_top_level_sections`, `enrich_html`,
`wrap_chapter_openers`, `wrap_opener_text`, `build_list_of_figures` are
replaced by an event walker over pulldown-cmark events. `build_toc`,
`build_generated_front_matter`, `build_generated_back_matter`,
`classify_sections`, `derive_chapter_id`, `parse_roman` / `to_roman` /
`normalize_h1` port as-is.

**Session estimate:** 2.0 sessions (the event walker is the biggest
single piece of new code in the whole port — ~400 LOC and the
classification regression tests need to pass against synthetic
manuscripts).

---

### 1.4 `pandoc.rs` — pandoc subprocess wrapper

**Role:** Spawn pandoc as a subprocess, feed markdown via stdin or a
file path, read HTML from stdout. Handles `--mathml`/`--katex`,
`--section-divs`, `--id-prefix`.

**Crate deps:**

- `std::path::{Path, PathBuf}` (`pandoc.rs:23`)
- `std::process::Stdio` (`pandoc.rs:24`)
- `tokio::io::AsyncWriteExt` (`pandoc.rs:27`)
- `tokio::process::Command` (`pandoc.rs:28`)
- `serde::{Deserialize, Serialize}` (`pandoc.rs:26`)
- `std::env::var` / `var_os` / `split_paths` (`pandoc.rs:88, 311, 317`)
- `thiserror::Error` (`pandoc.rs:30`)

**Verdict: drop for Path A.** Replaced by pulldown-cmark in-process.

But: read carefully which pandoc features are load-bearing so the
pulldown-cmark replacement matches. From `pandoc.rs:118-125`:

```
markdown+smart+footnotes+pipe_tables+link_attributes
+lists_without_preceding_blankline-yaml_metadata_block
```

Mapping to pulldown-cmark options:

- `smart` (smart quotes, em-dashes, ellipses): pulldown-cmark has
  `Options::ENABLE_SMART_PUNCTUATION`. **Available.**
- `footnotes`: `Options::ENABLE_FOOTNOTES`. **Available.** Note:
  Pandoc footnotes (`[^id]` + `[^id]: text`) parse identically.
- `pipe_tables`: `Options::ENABLE_TABLES`. **Available.**
- `link_attributes`: pandoc lets the writer add `{.class width=4in}` to
  inline image/link syntax. pulldown-cmark has
  `Options::ENABLE_HEADING_ATTRIBUTES` for headings but the
  per-image/per-link attribute extension was added as part of the
  `Options::ENABLE_DEFINITION_LIST` / link-attributes scope in recent
  versions. **Status: needs investigation** — confirm pulldown-cmark
  0.13+ has the inline-attribute extension we need; if not, we run
  these attributes through a separate post-pass that pattern-matches
  `![alt](src){...}` ourselves (cheap).
- `lists_without_preceding_blankline`: pulldown-cmark is more permissive
  about list starts than CommonMark, but the strict "list starts without
  a blank line after a paragraph" case may render as continuation text.
  **Test required.** If broken, document the requirement that
  manuscripts add the blank line, or pre-process markdown to insert one.
- `-yaml_metadata_block`: pulldown-cmark does NOT interpret `---` as a
  YAML metadata block (it's a thematic break by default). **Free
  drop-in.** The minus-sign in pandoc was disabling a behavior we don't
  have anyway.
- `--section-divs`: this is the load-bearing one. pulldown-cmark emits
  flat `<h1>` / `<p>` / etc., NOT `<section>` wrappers. We synthesize
  the sections in the event walker (Section 1.3) — that's the whole
  point of doing classification at parse time.
- `--id-prefix=ch`: pandoc auto-generates heading IDs with this prefix.
  pulldown-cmark with `Options::ENABLE_HEADING_ATTRIBUTES` lets the
  writer supply `# Title {#id}` explicitly; auto-generated IDs are not
  built in. The event walker generates IDs (slugify the heading text,
  prepend `ch`/`interlude`/etc.) the same way citations.rs already
  does — see `derive_chapter_id` in `citations.rs:199`.
- `--mathml` / `--katex`: pulldown-cmark has `Options::ENABLE_MATH`
  (since 0.10), which emits `<span class="math inline">` and
  `<div class="math display">` for `$...$` / `$$...$$`. **Available
  for the on-screen tag-passing case.** The actual math rendering
  (KaTeX or MathML conversion) is out of scope for Path A — we emit
  the raw `$...$` content in a `<span class="math">` and the writer
  can paste a KaTeX CDN script tag into the head template if they
  want it rendered. Path A note: explicitly DO NOT try to bundle a
  math renderer.

**Session estimate:** N/A (drop, replaced by Stage 1).

---

### 1.5 `epub_export.rs` + `pdf_export.rs`

**Role:** EPUB via pandoc; PDF via headless Chromium DevTools Protocol.

**Path A: drop both.** Read once for shared type names that other
modules import — `epub_export::*` and `pdf_export::*` are re-exported
through `mod.rs:23-30`. With epub + pdf cut, `mod.rs` becomes:

```
pub mod book_config;
pub mod citations;
pub mod html_emit;     // new — replaces pandoc.rs + structure.rs HTML side
pub mod structure;     // pure classification + ToC + front-matter generators

pub use book_config::{BookConfig, BookConfigError, BookMeta};
pub use citations::{
    transform_citations, normalize_unicode_scripts, tag_math_anchors,
    hoist_figure_classes, prepare_book_markdown, CitationError,
    CitationTransformResult,
};
pub use structure::{
    analyze, BookSection, BookStructure, SectionKind, StructuredHtml,
    build_generated_front_matter, build_generated_back_matter,
    build_list_of_figures, build_toc,
};
pub use html_emit::{emit_html, HtmlEmitOptions};
```

No shared types lost. The `BookConfig` doesn't reference epub/pdf
internals; `BookStructure` doesn't either.

**Session estimate:** 0.0 (just delete).

---

### 1.6 `mod.rs` — re-export module

Trivial: prune the `pub use` lines to drop pdf/epub. **Session
estimate: 0.0.**

---

## Section 2 — The pandoc kill

**Problem:** `pandoc.rs:99-300` is built around
`tokio::process::Command::new("pandoc").arg(...).output().await`. Pandoc
is a Haskell binary; we will never run it on SemOS. The `tokio` async
runtime is also out (Section 6).

**Replacement: `pulldown-cmark` in-process.**

**Crate-level posture check (training-knowledge, registry source dirs
are not readable from this sandbox so direct Cargo.toml inspection is
unavailable — flag for human verification):**

- `pulldown-cmark` ≥0.10 has `default-features = false` no_std support
  per its README; the default feature set is `["html", "simd"]`. Path
  A must specify `default-features = false, features = ["html"]` (or
  rebuild html.rs ourselves on top of the event stream). The `simd`
  feature pulls in `bytecount` with SIMD intrinsics — drop it.
- `pulldown-cmark` depends on `unicode-script` (small) for smart-punct
  classification, `memchr` (already vendored — `compiler/vendor/memchr-2.8.1`),
  and `bitflags` (small).
- The HTML writer (`pulldown_cmark::html::push_html`) writes into a
  `&mut String` — works under alloc.

**Verify before committing the port:** read
`C:\Users\jerro\.cargo\registry\src\index.crates.io-*\pulldown-cmark-*\Cargo.toml`
manually on the dev machine and confirm `[features]` lists `default`,
`html`, `simd`, and that `[dependencies]` has no `default-features = true`
deps that would forcibly pull in std. This plan assumes that's clean —
if it's not, fall back to `pulldown-cmark-escape` + a hand-written
HTML writer.

**Concrete pandoc-option mapping table (referenced by the event walker):**

| Pandoc option                       | pulldown-cmark / our code                                  |
| ----------------------------------- | ---------------------------------------------------------- |
| `markdown+smart`                    | `Options::ENABLE_SMART_PUNCTUATION`                        |
| `+footnotes`                        | `Options::ENABLE_FOOTNOTES`                                |
| `+pipe_tables`                      | `Options::ENABLE_TABLES`                                   |
| `+link_attributes`                  | **needs investigation** — pulldown-cmark 0.13+ may have it; otherwise post-pass |
| `+lists_without_preceding_blankline`| pulldown-cmark behaves like CommonMark; **test with the manuscript** |
| `-yaml_metadata_block`              | **N/A** — pulldown-cmark doesn't parse YAML blocks         |
| `--section-divs`                    | event walker emits `<section>` wrappers around H1 ranges   |
| `--id-prefix=ch`                    | event walker generates IDs in `derive_chapter_id` style    |
| `--mathml`                          | not supported in Path A (drop math rendering)              |
| `--katex`                           | emit raw `$...$` in `<span class="math">`; writer attaches KaTeX externally |
| `--standalone`                      | event walker wraps body in `<!DOCTYPE html><html><head>...` template |

**Concrete pandoc call sites that disappear:**

- `pandoc.rs:99` `convert_file` — gone.
- `pandoc.rs:186` `convert_str` — gone.
- `pandoc.rs:261` `version` — gone.
- `pandoc.rs:276` `probe` — gone (the UI status check is also gone since
  there's no UI in Path A).
- `pandoc.rs:310` `which_on_path` — gone.

**`prepare_book_markdown` (`citations.rs:40-62`) now becomes:**

```rust
pub fn prepare_book_markdown(config: &BookConfig)
    -> Result<(String, CitationTransformResult), CitationError>
{
    let mut combined = String::new();
    for (i, file) in config.resolved_files().into_iter().enumerate() {
        if i > 0 { combined.push_str("\n\n"); }
        combined.push_str(&semos_std::fs::read_to_string(file.as_str())?);
    }
    let result = transform_citations(&combined);
    Ok((result.transformed.clone(), result))
}
```

I.e. it returns the transformed `String` directly. No temp file. No
uuid (the `uuid` crate is out — it depends on `getrandom`, which needs
OS entropy via syscall; cheap to add later but unnecessary for Path A).

---

## Section 3 — The HTML parse problem

**Detailed selector/DOM-op enumeration from `structure.rs`:**

| Op                                                      | File:line                                                                      |
| ------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `Selector::parse("section.level1")`                     | `structure.rs:249`                                                             |
| `Selector::parse("h1")`                                 | `structure.rs:250`                                                             |
| `for section in doc.select(&section_sel)`               | `structure.rs:253`                                                             |
| `section.value().attr("id")`                            | `structure.rs:254`                                                             |
| `section.value().attr("class")`                         | `structure.rs:255`                                                             |
| `section.value().attr("data-header")`                   | `structure.rs:257`                                                             |
| `section.select(&h1_sel).next()`                        | `structure.rs:262-263`                                                         |
| `h.text().collect::<Vec<_>>().join("")`                 | `structure.rs:264`                                                             |
| Regex over the section tag for attribute injection      | `structure.rs:157-170` (`section_open_re`)                                     |
| Regex for figure caption numbering                      | `structure.rs:611-617`                                                         |
| Regex for chapter-opener `<p>` detection                | `structure.rs:737-748` (`CAP_RE`)                                              |

**Walker design (concrete, executable):**

```rust
// Pseudocode for the event walker. Two passes: pass 1 builds the
// BookStructure, pass 2 emits HTML.

struct WalkState {
    sections: Vec<BookSection>,
    current_h1_buf: Option<String>,           // accumulate H1 text
    current_h1_attrs: Option<HeadingAttrs>,   // {#id .class header="..."}
    awaiting_first_p_of_chapter: bool,
    first_p_first_text_run_consumed: bool,
    figure_count: u32,
    inside_figure: bool,
    inside_figcaption: bool,
    output: String,
}

fn pass1_collect_structure(md: &str) -> BookStructure { /* events → headings */ }
fn pass2_emit(md: &str, structure: &BookStructure) -> String {
    // Re-parse events; when we hit H1: close prior <section>, open new <section>
    // with data-section-* attrs from structure[order_index].
    // When we hit the first text of the first <p> after an H1 in a chapter
    // section: wrap drop-cap + lead-in.
    // When we hit a figure with caption: write `<figure id="...">`, increment
    // figure_count, prepend "Fig. N." in the figcaption.
    // Everything else passes through pulldown_cmark::html::push_html on
    // sub-event slices.
}
```

Two-pass is simpler than maintaining all that state in one pass; the
markdown is in memory anyway, and parse cost is small relative to the
overall pipeline. If the heap budget is tight on a 100-page manuscript
(Section 7), switch to one-pass with a smarter classifier that runs
on H1 only.

**Things the walker must detect:**

1. `Event::Start(Tag::Heading { level: H1, id, classes, attrs })` —
   open a new section. The pulldown-cmark `Tag::Heading` variant in
   modern versions carries `id: Option<CowStr>` and `classes:
   Vec<CowStr>` when `ENABLE_HEADING_ATTRIBUTES` is on, plus
   `attrs: Vec<(CowStr, Option<CowStr>)>` for `{header="..."}`-style
   attributes.
2. `Event::End(TagEnd::Heading(H1))` — H1 text is complete; classify.
3. `Event::Start(Tag::Heading { level: H2|H3|… })` — pass through;
   subsection. Inside the citations.rs pre-pass already, but
   structurally a subsection within the current top-level section.
4. `Event::Start(Tag::Paragraph)` immediately after an H1 in a Chapter
   section — flag "next text event is the chapter opener". On that
   text event, split on the first alphabetic char, wrap with
   `<span class="drop-cap">` + `<span class="lead-in">` (re-use
   `wrap_opener_text` logic from `structure.rs:764-846`, which is pure
   string manipulation already).
5. `Event::Start(Tag::Image { .. })` inside a paragraph that is being
   rendered as a figure — pandoc's `implicit_figures` extension turned
   any standalone image paragraph into `<figure><img/></figure>`.
   pulldown-cmark doesn't have a direct equivalent; we approximate by:
   on `Tag::Paragraph` start, peek ahead to see if the only events
   before `Tag::Paragraph` end are `Tag::Image` + optional whitespace.
   If yes, emit `<figure>` instead of `<p>`, write the image, and write
   `<figcaption>` from the image's alt text. Number it: `Fig. N.`
   prefix.
6. Track `figure_count`. After the whole document, walk `sections` for
   figures and build the `<nav class="toc lof">` list.

**Patterns the walker must NOT touch but must pass through:**

- Inline `<sup>` (citation back-refs already emitted by
  `transform_citations` in citations.rs).
- Inline `<a>` (links from `[text](url)`).
- `<blockquote>` content — `tag_math_anchors` post-pass adds
  `class="math-anchor"` where applicable.
- All other text events.

---

## Section 4 — `regex` on alloc-only

**Question:** does `regex` work with `default-features = false` under no_std + alloc?

**Answer (from training, registry Cargo.toml inspection blocked by
sandbox; verify before vendoring):** the regex crate's README has
historically claimed no_std + alloc support via
`default-features = false` plus no opt-in features that need std. In
practice, the `Error` type implements `std::error::Error` only when
the `std` feature is on; everything else (compilation + matching) works
on alloc. The compile-time cost is the issue, not the runtime.

**Risk:** at opt-level=0, `regex` 1.12 compiles ~200K LOC worth of
generated DFA code. Could easily be a multi-minute build. `regex-syntax`
and `regex-automata` are not small.

**Recommendation: `regex-lite`.** Same author (BurntSushi),
purpose-built for the situation where you just want PCRE-subset
matching without the full DFA engine:

- Pure no_std + alloc (advertised; verify via the crate's README on the
  dev machine).
- ~10× smaller compile time at opt-0.
- API is `regex_lite::Regex::new` + `.captures` + `.captures_iter` +
  `.replace_all` — same shape as `regex`, drop-in for the patterns used
  here.
- No SIMD, no Unicode property classes (the typesetter only uses
  `\s`, `\d`, `[a-z]`-style classes, and word boundaries `\b` — all
  ASCII-handled by regex-lite).

**Caveat:** `regex-lite` does NOT support named captures `(?P<num>...)`.
`structure.rs:108-115` and `structure.rs:124-131` use named captures
for `num` and `title`. **Rewrite** those patterns to unnamed groups
and use `caps.get(1)` / `caps.get(2)` index access. (citations.rs uses
unnamed groups already.) Quick mechanical fix.

**Verdict:** `regex-lite` is the choice. Estimated session impact:
included in citations.rs (1.0) and structure.rs (2.0) above.

---

## Section 5 — `chrono` on alloc-only

**Use site:** `structure.rs:516` — `chrono::Utc::now().year()` to
populate the default copyright year.

**Crate posture:** chrono with `default-features = false` advertises
no_std support, BUT the `clock` feature (which gives you `Utc::now()`)
requires the `std` feature internally because it goes through `SystemTime`
or `libc::clock_gettime`. On a `target = x86_64-unknown-none` build
with no libc, chrono can't get the time.

**Alternative crates:**

- `time` crate has similar limitation; `time::OffsetDateTime::now_utc()`
  requires the `std` feature.

**Recommendation: don't port a calendar crate; use the kernel's RTC.**

- The SemOS kernel reads the RTC at boot (mentioned in memory:
  "RTC century byte already handled" — see `docs/ROADMAP.md` M10
  watchdog entry).
- Expose it via a new `SYS_RTC_TIME` syscall returning `(year, month,
  day, hour, minute, second)` as a packed u64, or just the year.
- semos-std `time` module adds `time::current_year() -> u32` (~30 LOC).

For Path A copyright, we only need the year. If even that is too much
plumbing for the first cut, **require `book.copyright_year` to be
non-empty** and treat the empty case as an error (the current code
silently falls back to chrono — we'd surface "set copyright_year in
book.toml"). The writer is publishing a book; supplying a year is
reasonable.

**Verdict:** drop chrono entirely. Either:

- (a) Add `SYS_RTC_TIME` + `semos_std::time::current_year()` (~0.5
  session including the syscall plumbing).
- (b) Make `book.copyright_year` required (~5 minutes).

Recommend (b) for Path A; (a) is a future-MarlOS-as-a-citizen
improvement worth doing eventually.

---

## Section 6 — Tokio elimination

**Use sites (file:line):**

- `pandoc.rs:27` `use tokio::io::AsyncWriteExt;`
- `pandoc.rs:28` `use tokio::process::Command;`
- `pandoc.rs:99` `pub async fn convert_file(...)` — async, await
  internally.
- `pandoc.rs:164` `let output = cmd.output().await...`
- `pandoc.rs:186` `pub async fn convert_str(...)`
- `pandoc.rs:234-240` `child = cmd.spawn()...; stdin.write_all(...).await; child.wait_with_output().await`
- `pandoc.rs:262` `Command::new(pandoc).arg("--version").output().await`
- `epub_export.rs:15` `use tokio::process::Command;`
- `epub_export.rs:???` (further async pandoc calls)

**Verdict:** tokio is **only** used to await pandoc subprocesses.
With pandoc gone (Section 2) and EPUB gone (Path A), tokio drops out
entirely. The replacement code is fully synchronous: pulldown-cmark
parses synchronously, string manipulation is synchronous, semos-std
FS I/O is synchronous via syscalls.

No further substitution required. Just don't add a tokio dependency to
the new `marlos-typeset` Cargo.toml.

---

## Section 7 — Heap budget

**Kernel heap:** 16 MiB (`kernel-core/src/memory/heap.rs:45`,
`HEAP_SIZE = 16 * 1024 * 1024`). A user program's allocations come
out of its own per-process arena, which is sized differently — but a
SemOS user program is also constrained by the kernel's per-file
`MAX_FILE_CONTENT` cap of 2 MiB
(`kernel-core/src/semantic/object.rs:26`).

**100-page book back-of-envelope:**

- 100 pages × ~300 words/page × ~6 chars/word = ~180,000 chars =
  **~180 KiB markdown source.**
- Citation transform output: ~180 KiB body + ~10 KiB notes section =
  **~200 KiB intermediate markdown.**
- pulldown-cmark parse: events are short-lived borrows from the source
  string; the in-flight event buffer is negligible (single-event-at-a-time
  iterator) — call it **<100 KiB** for any reasonable book.
- pulldown-cmark HTML output: pandoc-equivalent HTML is roughly 1.5-2×
  the markdown source size = **~300-360 KiB HTML.**
- Section enrichment + drop-cap wrapping: in-place rewrites or
  string-builder concatenation — **~360 KiB peak** for the
  enriched HTML (we keep the unwrapped version in memory only
  transiently).
- Front-matter + back-matter + ToC: trivial, **<20 KiB**.
- BookStructure (Vec<BookSection>): ~40 sections × ~500 B/section =
  **~20 KiB**.

**Working set estimate at peak:** ~800 KiB-1 MiB inside the program.
**Kernel heap allocations** (file content for SYS_FREAD): the FS keeps
the content blob in kernel-heap-backed `Allocated` objects. Each file
mapped into the kernel namespace is up to 2 MiB.

**Conclusion:** comfortably fits. A 100-page book is **NOT** the
constraint. The constraints to watch:

1. **Single combined markdown >2 MiB → FS write fails.** If
   `prepare_book_markdown` writes the transformed markdown back to the
   FS (it doesn't have to in Path A, but if we did persist it),
   `SYS_FWRITE` enforces `MAX_FILE_CONTENT = 2 MiB`. Fix: don't persist
   the intermediate; keep it in memory.
2. **Output HTML >2 MiB.** For a 100-page book HTML is ~350 KiB —
   nowhere near the cap. A 500-page book (1.5M chars markdown) lands at
   ~3 MiB HTML — over the cap. Mitigation if we hit it: split the
   output into one file per chapter (`book/ch01.html`, ...) plus an
   `index.html` ToC. That's also better UX (deep-linkable chapters).
3. **Stack budget.** Per-task stack is 128 KiB
   (memory note `project_semantic_os_layout_sensitivity`). pulldown-cmark
   uses a small fixed stack; the event walker is iterative. Should be
   safe. Avoid recursive descent in any hand-written code (use explicit
   stacks).

**Stream-per-chapter recommendation:** not needed at the 100-page
target. **Defer until** the writer hits a book larger than ~300 pages
or until the per-chapter HTML output approach is wanted for browsing
ergonomics anyway. Mention this in the Path A README so the writer
knows the failure mode if they pour a 1500-page MS in.

---

## Section 8 — Staging (boot-validated milestones)

Each stage is a separate DEMO with its own assert harness, integrated
and validated by the parent agent before the next stage starts.
Sub-agent work is fine for the staged code; the boot/QEMU validation is
always the parent's job (sub-agents can't run QEMU — memory
`feedback_agents_cannot_run_qemu`).

### Stage A — pulldown-cmark hello-world on SemOS

**Goal:** prove pulldown-cmark builds and runs in a Ring-3 program
linked against semos-std at opt-level=0.

**Deliverable:** `user-programs/pulldown-cmark-smoke/`:

- Cargo.toml with `pulldown-cmark = { version = "0.13", default-features = false, features = ["html"] }`.
- Vendor pulldown-cmark + its deps (bitflags, unicode-script, memchr —
  memchr is already in `compiler/vendor/`; check if it satisfies
  pulldown-cmark's version requirement).
- `src/main.rs` reads a fixed string (`# Hello\n\nWorld with *emphasis*.`),
  parses it, writes HTML via `pulldown_cmark::html::push_html` to a
  `String`, prints with `println!`.
- DEMO 73: SYS_SPAWN it, capture stdout, assert it contains
  `<h1>Hello</h1>` and `<em>emphasis</em>`.

**Risk:** pulldown-cmark might have a hidden std dependency. If the
build fails, fall back to vendoring + patching out the std use sites.
Expect 1 session of "make it compile" — this is the first complex
no_std crate the project will vendor that isn't on the recommended-vendor
list.

**Exit criteria:** DEMO 73 PASS, suite stays clean (no regression).

**Session estimate:** 1.5 sessions (1.0 vendoring + patching, 0.5
DEMO 73).

### Stage B — book_config loader

**Goal:** read a `book.toml` from the SemOS FS into a `BookConfig`.

**Deliverable:**

- `user-programs/marlos-typeset/` crate (one new program; pulldown-cmark
  comes along as a dep via Stage A's vendored copy).
- `book_config.rs` + `toml_lite.rs` ported.
- Pre-install a small `/book/book.toml` + one `/book/manuscript.md` in
  the kernel ramfs (M27 D.2's persistence path makes this cheap).
- `src/main.rs` opens `/book/book.toml`, parses, prints
  `Loaded book: "{title}" by {author}, {N} files` to stdout.
- DEMO 74: SYS_SPAWN, assert exit code 0 and stdout contains the
  expected title.

**Add (if not already there):** `semos_std::fs::metadata` (Section 1.1).

**Exit criteria:** DEMO 74 PASS, suite clean.

**Session estimate:** 1.5 sessions (1.0 port + toml_lite, 0.5 DEMO 74).

### Stage C — markdown → HTML with structure analysis

**Goal:** the core of the typesetter — citation transform + event
walker that classifies sections and emits enriched HTML.

**Deliverable:**

- `citations.rs` ported (with regex-lite + no temp files).
- `structure.rs` rewritten as event walker.
- `html_emit.rs` (new) — the event walker + pulldown-cmark glue.
- `src/main.rs` reads `/book/book.toml`, processes citations, walks the
  combined markdown, emits one `/book/out.html` file.
- DEMO 75: SYS_SPAWN, after exit read `/book/out.html`, assert it
  contains `<section data-section-type="chapter" data-section-number="1">`
  and `<sup class="note-ref">` (citation back-ref).

**Exit criteria:** DEMO 75 PASS. The unit tests in `citations.rs`
(`tests` module) and `structure.rs` `tests` module become Ring-3
integration tests inside this program — run them all at startup and
return non-zero on first failure.

**Session estimate:** 2.5 sessions (this is the bulk of the work —
regex-lite vendor 0.3, citations port 0.7, event walker 1.5).

### Stage D — front-matter, ToC, back-matter, drop caps

**Goal:** the polished output — generated title page, copyright,
dedication, ToC, list of figures, drop-cap + lead-in spans on chapter
openers.

**Deliverable:**

- `build_generated_front_matter` / `build_generated_back_matter` /
  `build_toc` / `build_list_of_figures` ported (mostly as-is from
  `structure.rs`).
- Drop-cap wrapping moved into the event walker.
- `wrap_opener_text` (`structure.rs:764`) ported as a pure helper
  invoked from the walker.
- DEMO 76: SYS_SPAWN, read `/book/out.html`, assert:
  - `<section ... data-front-page="title">` with the book title.
  - `<section ... data-front-page="toc">` with at least one
    `<a class="toc-entry toc-chapter" ...>` entry.
  - `<span class="drop-cap">` appears before chapter 1's first text.
  - `# Notes` heading is present (citations test) and
    `<section ... data-back-page="acknowledgements">` if the book.toml
    sets them.

**Exit criteria:** DEMO 76 PASS. Plus a manual "browse the output in
Chrome on the dev machine" sanity check.

**Session estimate:** 1.0 session.

**Stage E (optional, post-Path-A):** add a tiny stylesheet emit. The
existing CSS in `pdf_export.rs:build_export_css` and
`epub_export.rs:build_epub_css` is rich; Path A wants a minimal-but-pretty
"screen reader" stylesheet for the writer to skim. ~0.5 session if
desired; not strictly required for Path A success ("HTML out").

---

### Stage total

| Stage | Description                              | Sessions |
| ----- | ---------------------------------------- | -------- |
| A     | pulldown-cmark smoke                     | 1.5      |
| B     | book_config loader                       | 1.5      |
| C     | citations + event walker + HTML emit     | 2.5      |
| D     | front-matter / ToC / drop caps           | 1.0      |
| ----- | ---------------------------------------- | -----    |
| Total | Path A end-to-end                        | **6.5**  |

Add **0.5 contingency** for the first-no_std-vendor surprises (Stage A
historically over-runs in this project; semos-std `metadata` add might
also bleed into Stage B).

**Path A total: ~7 sessions.**

---

## Section 9 — Open questions for human

1. **pulldown-cmark version pin.** Latest stable is 0.13 (as of late
   2025); 0.13 has `Options::ENABLE_HEADING_ATTRIBUTES` and
   `Options::ENABLE_MATH`. Confirm the `link_attributes` extension
   coverage (image attribute lists `{.fig-bleed width=4in}`) before
   the manuscript test. If 0.13 doesn't support image attributes, do we
   (a) post-pass them, (b) require the writer to inline a CSS class via
   `<figure class="...">` directly, or (c) hold for an upstream PR. My
   bias: (a) — the post-pass is a ~50-line regex-lite job.

2. **Math rendering.** Path A drops the math renderer. The MarlOS
   pipeline supports both KaTeX and MathML — `[CITE: ...]` notes in
   the physics manuscript will contain math markup. Do we:
   - (a) emit `$...$` literally inside `<span class="math">` and let
     the writer attach KaTeX via a `<script>` tag in a hand-written
     wrapper template,
   - (b) port a tiny MathML emitter (regex translation of common LaTeX
     to MathML — ad-hoc, but a few-hundred-line job since the manuscript
     uses a constrained subset), or
   - (c) defer math entirely until Path B (when fonts are in play).

   My bias: (a). Cheapest. Writer can paste a KaTeX CDN tag manually
   before the next build cycle adds it to the template.

3. **Should `marlos-typeset` be one Ring-3 program or a shell builtin
   (like `agent` and `edit`)?** Shell builtins live kernel-side and
   bypass the syscall layer; a builtin would let us skip the
   `SYS_SPAWN` overhead and the `MAX_FILE_CONTENT` constraint on
   output (kernel-side code allocates from the kernel heap directly).
   The downside is kernel layout sensitivity (`project_semos_layout_sensitivity`).
   My bias: **Ring-3 program**, until we've validated it on a real
   manuscript. Lower blast radius, and the Phase 14 self-hosting story
   has us moving everything to Ring 3 anyway.

4. **TOML reader: lite or upstream?** The hand-rolled `toml_lite`
   ships maintenance debt. Plan ships it; if upstream `toml_edit`
   turns out to be no_std-portable with `default-features = false`
   plus our hashbrown substitution, prefer that. The "verify by reading
   Cargo.toml on disk" step in Stage B will determine this — the
   sandbox blocked me from doing it ahead of time.

5. **Where on the SemOS FS do book sources live?** Suggestion: `/book/`
   for the active project. Multi-book support is post-Path-A. The
   shell `cd /book && typeset` workflow is clean.

6. **Drop-cap font dependency.** `pdf_export.rs` embeds EB Garamond.
   Path A uses the browser's system font — drop caps will render in
   whatever the browser falls back to. The writer is on Windows, has
   EB Garamond installed system-wide; the local browser will use it via
   `font-family: "EB Garamond", Georgia, serif`. Confirm this is OK
   for V1, otherwise we need a font-embedding stage (Path B).

7. **Citation tests as runtime asserts in Ring 3.** The existing
   `#[cfg(test)] mod tests` blocks in `citations.rs` and `structure.rs`
   use stdlib's test harness which we don't have. Plan: wrap them in
   plain `fn` and call from `main.rs` at startup, returning a non-zero
   exit code on first failure. Lower fidelity than `#[test]` but works
   without a test runner. Acceptable for the porting milestone.

---

## Appendix A — semos-std additions required

Single point of truth for what needs adding to `user-programs/std-shim/`
before the marlos-typeset port can land:

| Addition                                  | Where                | Size  | Required by | Notes |
| ----------------------------------------- | -------------------- | ----- | ----------- | ----- |
| `fs::metadata(&str) -> io::Result<Metadata>` with `is_file()`/`is_dir()` | `std-shim/src/fs.rs` | ~40 LOC + kernel-side SYS_STAT helper | book_config.rs `Path::is_file/is_dir` ports | Could be skipped via try-open if SYS_STAT isn't already exposed |
| `fs::create_dir_all(&str)` | `std-shim/src/fs.rs` | ~30 LOC | citations.rs temp dir (only if we keep temp files; recommended to skip) | Optional |
| Optional: `sync::OnceLock<T>` | `std-shim/src/sync.rs` | ~50 LOC | Only if we keep regex-cached statics (recommended to skip) | Optional |
| Optional: `time::current_year() -> u32` via new SYS_RTC_TIME | `std-shim/src/time.rs` + kernel syscall | ~80 LOC | structure.rs `chrono::Utc::now().year()` (alternative: require book.copyright_year) | Optional |

None of these are blocking — each has a workaround. Confirm before
Stage B which subset we want.

---

## Appendix B — Concrete Cargo.toml skeleton for marlos-typeset

```toml
[package]
name = "marlos-typeset"
version = "0.1.0"
edition = "2021"

[workspace]                   # opt out of any parent workspace

[[bin]]
name = "marlos-typeset"
path = "src/main.rs"

[dependencies]
semos-std = { path = "../std-shim" }
pulldown-cmark = { version = "0.13", default-features = false, features = ["html"] }
regex-lite = { version = "0.1", default-features = false }   # verify default-features list on dev box
serde = { version = "1", default-features = false, features = ["derive", "alloc"] }
# NO toml — hand-write
# NO chrono — drop or use kernel RTC
# NO thiserror — hand-write Display
# NO tokio
# NO scraper / html5ever
# NO uuid

[profile.release]
panic = "abort"
opt-level = 0          # #54 — mandatory
lto = true
strip = true
codegen-units = 1
```

Plus the standard `.cargo/config.toml` (target = x86_64-unknown-none,
build-std core+compiler_builtins) + `build.rs` + `link.ld` copied from
`sync-demo/` (memory note `feedback_new_user_program_nonpie`).

---

## Appendix C — Files that drop from the port

For grep-ability, the modules NOT carried into the marlos-typeset crate:

- `pandoc.rs` — entire file.
- `epub_export.rs` — entire file.
- `pdf_export.rs` — entire file (and all the embedded EB Garamond
  font-face plumbing).
- `mod.rs` `pub use epub_export::*` / `pub use pdf_export::*` lines —
  replaced by the slimmer re-export shown in Section 1.5.

Kept verbatim (modulo std → semos_std + regex → regex-lite + named
captures → unnamed):

- `citations.rs`: `transform_citations`, `transform_line`,
  `derive_chapter_id`, `slugify`, `parse_num_to_arabic`,
  `normalize_unicode_scripts`, `tag_math_anchors`, `hoist_figure_classes`.
- `structure.rs`: `parse_roman`, `to_roman`, `parse_number`,
  `normalize_h1`, `classify_sections`, `build_generated_front_matter`,
  `build_generated_back_matter`, `build_toc`, `escape_html`,
  `wrap_opener_text`.
- `book_config.rs`: `BookConfig`, `BookMeta`, `TrimMargins`, `TrimConfig`,
  `TypographyConfig`, `ExportConfig`, `to_toml_string`, `init_from_markdown`,
  `toml_string_literal`, `fmt_float`.

Roughly **70% of the typesetter logic ports unchanged**. The 30% rewrite
is concentrated in the HTML emission path (event walker replaces
scraper + the post-pandoc regexes).
