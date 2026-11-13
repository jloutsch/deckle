# Native KFX, phase 2 (EPUB reader and styles) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From an EPUB file alone, `deckle-kfxgen` produces the same book view (spine, manifest, contents, metadata), the same per-file element tree and the same per-element styles that kfxgen receives from Calibre 9.18, checked against dumps taken inside a real kfxgen conversion.

**Architecture:** Two new modules in `crates/deckle-kfxgen`: `epub` (package files and Calibre's view of them) and `css` (parse, cascade, Calibre's flattening, value resolution), over a small owned element tree in `dom`. Nothing writes CSS text or re-parses it: the flattening step produces each element's declared values directly. Calibre and kfxgen run only on a developer machine to make fixtures; the tests read committed dump files.

**Tech Stack:** Rust; `roxmltree` through `deckle_limits::parse_xml`; `html5ever`, `cssparser` 0.37 and `selectors` 0.38 (all already in `Cargo.lock`, added as direct dependencies; no new crate enters the lock file). Python only in `tools/ref/`.

**Spec:** `docs/superpowers/specs/2026-11-10-native-kfx-design.md`. Research behind this plan: the three reports summarized in "What the research changed" below.

## What the research changed (read before Task 1)

1. **The design's cascade oracle is wrong.** kfxgen never sees the book's CSS. Calibre's `CSSFlattener` first deletes every `style` attribute, `<style>` and stylesheet and writes one class per element; kfxgen then builds its own `Stylizer` on that result (`font_table.py:460-472`). A Stylizer run on the original XHTML gives different values. The oracle is therefore a dump of kfxgen's own Stylizer, taken inside the conversion. Task 1 corrects the design.
2. **So phase 2 reproduces what Calibre does before kfxgen runs**, as values, not as files: the element tree changes (`parse_html` cleanups, chapter-mark `<div>`s from `DetectStructure`), the first cascade on the original CSS with Calibre's user-agent sheet, the flattener's value changes, `RemoveFakeMargins`, and the class names the flattener gives.
3. **Scope limit (my call, for the owner to accept or change):** phase 2 stops at Calibre's `Style` object. kfxgen's own resolver (`converter.py:1030-1240`), its `data-kfxgen-*` marks and its spine reorder are kfxgen code and belong to phase 3. Phase 2 supplies what they consume: `get`, the computed values, the pseudo dicts, the rule list with origin, selector matching, `@font-face` rules, and the raw OPF, nav and NCX text.
4. **Size.** About 4,000 to 5,000 lines of Rust plus tests, against the design's "1 to 2 weeks". Phase 1 was about 12,000 lines.

## Global Constraints

- Pinned references: kfxgen `0bc705d`, Calibre **9.18.0** (`fixtures/kfxgen/models/TOOLS`). A dump made with another Calibre version is refused by the test harness.
- The kfxgen repository is only read. All hook code lives in `tools/ref/kfxgen_hook/` and is injected into an exported copy, as in phase 1.
- `deckle-kfxgen` must not depend on `deckle-core` (phase 4 makes `deckle-core` depend on it).
- License: the crate is `GPL-3.0-only`; every new source file carries `// SPDX-License-Identifier: GPL-3.0-only`. Code ported from Calibre (GPL v3) names its source file and function in a comment, and `crates/deckle-kfxgen/NOTICE` (new) lists Calibre's files used, including `html.css` copied verbatim.
- Every XML parse goes through `deckle_limits::parse_xml`; every archive read through `Budget`; own recursion stops at `deckle_limits::MAX_DEPTH`. `cargo test -p deckle-core --test it xml_boundary state_file_guard` must pass.
- Deterministic: no hash-map iteration order, time or randomness reaches a result. Class numbering follows spine order then document order.
- Every deliberate difference from Calibre or kfxgen gets a section in `crates/deckle-kfxgen/DIVERGENCES.md` in the existing format.
- New fixtures stay under **1.5 MB** in total (`fixtures/kfxgen` is at 7.9 MB of its 10 MB cap). Corpus dumps stay outside git.
- No README or CHANGELOG entry: nothing a user sees changes. No new network call.
- Commands: `export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"`; tests `cargo test -p deckle-kfxgen --test it <filter>`; before each commit `cargo fmt` and `cargo clippy -p deckle-kfxgen --all-targets -- -D warnings`.
- Branch `feat/native-kfx-epub-cascade`, one commit per task, messages `feat(kfxgen): …`.

## Review Focus

1. **XHTML that is not well-formed.** Calibre falls back to its HTML5 parser; the port falls back to `html5ever`. Expected: the book still loads and its text is all present; the trees may differ from Calibre's, which is recorded in `DIVERGENCES.md`. Test in Task 4.
2. **A file that is not UTF-8.** Expected: decoded by its XML declaration or BOM as Calibre does for UTF-16 and Latin-1, otherwise a typed error naming the file; never a panic. Test in Task 4.
3. **Hostile CSS.** 50,000 rules, a selector with 1,000 compound parts, `:has()` over a deep tree, an `@import` cycle. Expected: finishes in under 5 s or returns `Error::TooLarge`; no stack overflow. Tests in Tasks 5 and 7.
4. **An EPUB with no contents file, a missing cover, or a spine item absent from the manifest.** Expected: the same view Calibre gives (generated contents flag set, no cover, item skipped), not an error. Tests in Task 3.
5. **A value Calibre keeps although it is invalid** (`margin-left: foo`, `height: 10 px`, `font-size: SMALL`). Expected: kept or inherited exactly as the dump shows; the port must not "fix" it. Tests in Tasks 6 and 9.

---

### Task 1: Dumps from inside a kfxgen conversion, and the design correction

**Files:**
- Create: `tools/ref/kfxgen_hook/book_dump.py`, `tools/ref/kfxgen_hook/test_book_dump.py`, `tools/ref/make_css_quirks_epub.py`
- Modify: `tools/ref/kfxgen_hook/inject.py`, `tools/ref/kfxgen_models.sh`, `tools/ref/README.md`, `fixtures/kfxgen/README.md`, `docs/superpowers/specs/2026-11-10-native-kfx-design.md`
- Create (generated): `fixtures/kfxgen/cascade/<name>/{oeb.json, files.json, dom/NNN.xhtml, styles/NNN.json, rules/NNN.json}`, `fixtures/kfxgen/cascade/{KFXGEN_COMMIT, TOOLS}`

**Interfaces:**
- Produces, per book, when `KFXGEN_DUMP_BOOK=<dir>` is set (second injected anchor: right after `_ensure_oeb_opts` in `convert_oeb_to_kfx`, `converter.py:9508`):
  - `oeb.json`: `spine` (list of `{href, linear}`), `page_progression_direction`, `manifest` (list of `{id, href, media_type}` sorted by href), `toc` (tree of `{title, href, children}`), `auto_generated_toc`, `guide` (list of `{type, href}` sorted by type), `metadata` (`title, creator, description, language, publisher, date, cover`, each a list of strings), `raw` (`opfname`, and the sha256 of the unpacked OPF).
  - `files.json`: spine href → `NNN`.
  - `dom/NNN.xhtml`: the spine item's tree, serialized by `lxml.etree.tostring(root, encoding="unicode")` **after** the Stylizer for that item is built (building it removes `img` `width`/`height`).
  - `styles/NNN.json`: `{"table": [<style>…], "nodes": [[path, table_index]…]}` where `path` is the list of element-child indexes from the root (comments and processing instructions not counted) and `<style>` is `{"specified": cssdict(), "pseudo": _pseudo_classes, "computed": {prop: value}}` for the properties in `COMPUTED_PROPS` (below); a property whose lookup raises is written as `{"error": "<exception class>"}`. Also top-level `font_face` (list of `{prop: value}` per rule, from kfxgen's Stylizer).
  - `rules/NNN.json`: the **first-pass** Stylizer's rules, the only place the book's own CSS is visible. Third injected anchor: the start of `specialize_css_for_output` (`plugin/__init__.py:150`), which Calibre calls per file before flattening it. Content: `rules` (list of `[origin, selector_text, cssdict]` in cascade order, origin 0 for the user-agent sheet and 1 for the book) and `font_face`.
  - `COMPUTED_PROPS = text-align, font-family, font-style, white-space, list-style-type, border-collapse, border-{top,right,bottom,left}-{style,width,color}, font-size, line-height, width, height, margin-{top,right,bottom,left}, padding-{top,right,bottom,left}, text-indent, color, background-color, font-weight`.
- `kfxgen_models.sh --cascade` runs the dump for the chosen sources and writes `fixtures/kfxgen/cascade`; `--corpus` also writes `$DECKLE_KFXGEN_CORPUS_OUT/<book>/cascade`.

- [ ] **Step 1: Write `test_book_dump.py`**: with a stub OEB of one spine item, the dump writes the four outputs; two runs give identical bytes; identical style dicts share one `table` entry.
- [ ] **Step 2: Run it.** `python3 tools/ref/kfxgen_hook/test_book_dump.py`. Expected: FAIL, `book_dump` not found.
- [ ] **Step 3: Implement `book_dump.py` and the second and third anchors in `inject.py`** (stop with an error if the anchor line has moved, as the first anchor does).
- [ ] **Step 4: Write `make_css_quirks_epub.py`**: builds `fixtures/kfxgen/sources/css_quirks.epub`, one small file per quirk listed in Tasks 5 to 10 (each test names the file it reads).
- [ ] **Step 5: Add `--cascade` to `kfxgen_models.sh`**, choose the sources: `css_quirks.epub` plus at most 11 of the existing 28 that between them cover lists, tables, images, notes, vertical text, RTL and embedded fonts. Run it twice; `diff -r` of the two outputs must be empty. Check `du -sh fixtures/kfxgen/cascade` is under 1.5 MB; drop sources until it is.
- [ ] **Step 6: Correct the design.** In the spec: replace the Cascade oracle row with "kfxgen's own Stylizer, dumped inside the conversion"; replace the property list with the one in this plan's Task 9 and 10 interfaces; remove `visibility`; add the scope limit and the size from "What the research changed"; note that kfxgen also reads the raw OPF, nav and NCX.
- [ ] **Step 7: Run** `python3 tools/ref/kfxgen_hook/test_book_dump.py`. Expected: PASS.
- [ ] **Step 8: Commit** `feat(kfxgen): dump Calibre's book view and styles from a kfxgen run`.

### Task 2: The archive reader moves below `deckle-core`

**Files:**
- Create: `crates/deckle-kfx/src/epub_zip.rs` (moved from `crates/deckle-core/src/epub.rs`)
- Modify: `crates/deckle-core/src/epub.rs` (becomes `pub use deckle_kfx::epub_zip::*;`), `crates/deckle-kfx/src/lib.rs`, `crates/deckle-kfxgen/Cargo.toml` (add `deckle-limits` with `features = ["xml"]`, `roxmltree`, `html5ever`, `cssparser`, `selectors`, `zip` as the workspace has them)

**Interfaces:**
- Produces: `deckle_kfx::epub_zip::{EpubReader, check_declared_sizes, read_entry, join, percent_decode, encryption_is_drm}` with unchanged signatures; `EpubReader::budget(&mut self) -> &mut Budget` replaces crate-private field access.
- Consumes: nothing new.

- [ ] **Step 1: Move the file and its tests; re-export.** No behaviour change.
- [ ] **Step 2: Run** `cargo test -p deckle-core -p deckle-kfx` and `cargo clippy --workspace --all-targets -- -D warnings`. Expected: same results as before the move (the known environment failures in the session notes excepted).
- [ ] **Step 3: Commit** `refactor(kfx): epub archive reader moves to deckle-kfx`.

### Task 3: `epub`: the package and Calibre's view of it

**Files:**
- Create: `crates/deckle-kfxgen/src/epub/{mod.rs, package.rs, toc.rs, oeb.rs}`, `crates/deckle-kfxgen/tests/it/epub.rs`
- Modify: `crates/deckle-kfxgen/src/lib.rs`, `tests/it/main.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Package { pub opf_path: String, pub opf_text: String, pub nav_text: Option<String>, pub ncx_text: Option<String>,
      pub manifest: Vec<Item>, pub spine: Vec<SpineRef>, pub guide: Vec<GuideRef>, pub metadata: Metadata,
      pub page_progression_direction: Option<String> }
  pub struct Item { pub id: String, pub href: String, pub media_type: String, pub properties: Vec<String> }
  pub struct SpineRef { pub href: String, pub linear: bool }
  pub struct TocNode { pub title: String, pub href: String, pub children: Vec<TocNode> }
  pub struct Oeb { pub spine: Vec<SpineRef>, pub manifest: Vec<Item>, pub toc: Vec<TocNode>, pub auto_generated_toc: bool,
      pub guide: Vec<GuideRef>, pub metadata: Metadata, pub page_progression_direction: Option<String> }
  pub fn read_package(zip: &mut EpubReader) -> Result<Package, Error>;
  pub fn oeb_view(pkg: &Package, zip: &mut EpubReader) -> Result<Oeb, Error>;
  ```
  `Metadata` holds `Vec<String>` per field named in Task 1's `oeb.json`.
- `oeb_view` ports, each with its Calibre source named in a comment: hrefs relative to the OPF folder; an HTML cover or title page removed from the spine (`epub_input.py` `rationalize_cover2/3`); duplicate spine entries dropped; linked HTML missing from the spine appended as non-linear (`oeb/reader.py:339`); contents from nav else NCX, empty labels dropped, the generated-contents flag; language as Calibre writes it; dates filled to a full day.
- Not here: kfxgen's own spine reorder (`converter.py:8172-8221`); phase 3.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- `oeb.json` is Calibre's view **after** its transforms, so: hrefs are from the EPUB root (`OEBPS/one.xhtml`), not relative to the OPF; the manifest no longer holds the book's CSS, nav or NCX and does hold Calibre's `stylesheet.css` and `page_styles.css`. `oeb_view` produces that manifest; the test compares it whole.
- `raw.sha256` is `null` where Calibre wrote over the OPF; the test skips those books. `raw` names the EPUB's own OPF path.
- A year-only `dc:date` gets day 15 of the month of the run in Calibre (`calibre.utils.date.parse_only_date`). The port fills month 01, day 15; the test ignores the month for a year-only source date; `DIVERGENCES.md` records it.
- When the book has neither nav nor NCX, `oeb_view` sets `auto_generated_toc` and leaves `toc` empty; Task 8 fills it. The test compares `toc` only when the flag is false.
- References for the port rules: `package_quirks` (cover page out of the spine, guide entries, duplicate dropped, `linear="no"`, linked file appended, `en-US` → `eng`, empty NCX label dropped) and `no_toc`.

- [ ] **Step 1: Write the failing tests** in `tests/it/epub.rs`:
  - `every_cascade_fixture_oeb_matches_calibre`: for each `fixtures/kfxgen/cascade/<name>`, `oeb_view` serialized to the same JSON shape equals `oeb.json`, and `sha256(pkg.opf_text)` equals `raw.sha256`.
  - `no_contents_file_gives_generated_flag`, `spine_item_missing_from_manifest_is_skipped`, `missing_cover_is_not_an_error` (inputs built with `deckle_kfx::fixtures::stored_epub`).
  - `opf_nested_past_256_levels_is_refused` (expects `Error::Xml`).
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it epub`. Expected: FAIL to compile, `epub` missing.
- [ ] **Step 3: Implement** the four files.
- [ ] **Step 4: Run** the same command. Expected: PASS. A fixture that cannot match gets a `DIVERGENCES.md` section and is named in the test's allow-list with that section's title.
- [ ] **Step 5: Commit** `feat(kfxgen): read the package as Calibre presents it`.

### Task 4: `dom`: an owned element tree, parsed as Calibre parses it

**Files:**
- Create: `crates/deckle-kfxgen/src/dom/{mod.rs, parse.rs, clean.rs, write.rs}`, `tests/it/dom.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Dom { /* arena */ }
  #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)] pub struct NodeId(u32);
  impl Dom {
      pub fn root(&self) -> NodeId;
      pub fn tag(&self, n: NodeId) -> &str;                    // local name, lowercased
      pub fn attr(&self, n: NodeId, name: &str) -> Option<&str>;
      pub fn set_attr(&mut self, n: NodeId, name: &str, value: Option<&str>);
      pub fn parent(&self, n: NodeId) -> Option<NodeId>;
      pub fn children(&self, n: NodeId) -> &[NodeId];           // elements only
      pub fn text(&self, n: NodeId) -> &str; pub fn tail(&self, n: NodeId) -> &str;   // lxml's text/tail model
      pub fn insert_before(&mut self, at: NodeId, tag: &str) -> NodeId;
      pub fn remove(&mut self, n: NodeId); pub fn rename(&mut self, n: NodeId, tag: &str);
      pub fn path(&self, n: NodeId) -> Vec<u32>;                // the dump's path
      pub fn descendants(&self) -> impl Iterator<Item = NodeId> + '_;  // document order, a loop
  }
  pub fn parse(bytes: &[u8]) -> Result<Dom, Error>;   // parse_xml, else html5ever; then clean
  pub fn to_xhtml(dom: &Dom) -> String;                // same shape as lxml's tostring for comparison
  ```
- `clean.rs` ports `oeb/parse_utils.py:166 parse_html`: names lowercased, XHTML namespace forced, `head`/`body`/`title` ensured, MS-Office tags to `span`, empty `a[href]`/`i`/`b`/`u` without `id` or `name` removed, a `<br>` with content to `div`.
- The tree keeps namespaced attributes by their qualified name (`epub:type`), and SVG and MathML subtrees unchanged.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- `dom/NNN.xhtml` is the tree **after** flattening. Compare `<body>` subtrees only (`<head>` gains a `meta`, two `link`s and a title). Ignore the attributes `class`, `style` and `data-kfxgen-*`. Chapter-mark divs are the elements whose `styles/` entry is exactly `{display: block, page-break-after: always}`; remove those from the expected side.
- Calibre's XML parse path keeps tag and attribute names as written (`<P>`, `<DIV CLASS=…>` in `css_quirks/dom/000.xhtml`); only its HTML fallback lowercases. `Dom::tag` returns the name as Calibre keeps it; follow the dump. Matching in Task 7 is ASCII case-insensitive either way.

- [ ] **Step 1: Write the failing tests** in `tests/it/dom.rs`:
  - `parse_cleanups_match_calibre`: for `css_quirks.epub`'s `cleanups.xhtml`, the element paths and tags of `parse(bytes)` equal those of `dom/NNN.xhtml` once elements whose `style` is `display: block; page-break-after: always` (Task 8 adds them) are removed from the expected side.
  - `malformed_xhtml_loads_with_all_its_text` (Review Focus 1), `utf16_and_latin1_decode`, `undeclared_non_utf8_is_a_typed_error` (Review Focus 2).
  - `ten_thousand_nested_spans_do_not_overflow` on a 1 MiB thread: `Error::Xml` or a tree cut at `MAX_DEPTH`, never an abort.
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it dom`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Tree walks are loops over the arena, not recursion.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(kfxgen): element tree parsed and cleaned as Calibre does`.

### Task 5: `css::sheet`: stylesheets to rules

**Files:**
- Create: `crates/deckle-kfxgen/src/css/{mod.rs, sheet.rs, value.rs, media.rs}`, `crates/deckle-kfxgen/src/css/html.css` (Calibre's `resources/templates/html.css`, verbatim), `crates/deckle-kfxgen/NOTICE`, `tests/it/css_sheet.rs`
- Modify: `crates/deckle-limits/src/lib.rs` (`pub const MAX_STYLE_RULES: usize = 50_000;`), `docs/books.md` (one row in the limits table: "CSS rules in one book file: 50,000")

**Interfaces:**
- Produces:
  ```rust
  pub type Decls = Vec<(String, Decl)>;                 // property name lowercased, last declaration wins, source order kept
  pub struct Decl { pub value: String, pub important: bool }
  pub enum Origin { UserAgent, Book }
  pub struct Rule { pub origin: Origin, pub selector: String, pub decls: Decls, pub index: u32 }
  pub struct Sheet { pub rules: Vec<Rule>, pub font_face: Vec<Decls>, pub page: Vec<Decls> }
  pub fn parse_sheet(text: &str, origin: Origin, import: &mut dyn FnMut(&str) -> Option<String>) -> Result<Sheet, Error>;
  pub fn parse_style_attr(text: &str) -> Decls;
  pub fn serialize_value(tokens: &str) -> String;       // css_parser 1.1.1's output form
  pub fn media_ok(query: &str) -> bool;
  ```
- Rules ported (source in comments): `CSSPreProcessor` drops `mso-` and `panose-` declarations; `@import` is inlined before the importing sheet's rules, a cycle or a depth over 8 ends the chain; `@page` in a linked sheet is dropped; one level of `@media` is flattened and kept when `media_ok` (`screen`, `all`, `aural`, `amzn-kf8`; any width, height, aspect, colour or resolution feature fails); `@font-face` is kept only with more than one declaration; a selector list becomes one `Rule` per selector.
- `serialize_value`: property names and units lowercased; `.5em` → `0.5em`; `0pt` → `0`; keyword and colour case preserved (`RED`, `Center`); quotes normalised to double; an empty value drops the declaration; an invalid value is kept as written.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- Dump format as built: `rules/NNN.json` holds `{"ua_rules": <count>, "rules": [[origin, selector, declarations, important, key]…], "font_face": […]}` with book rules only, in cascade order; `key` is `[origin, a, b, c, d, index]`; `important` lists the `!important` property names. The user-agent rules are once in `fixtures/kfxgen/cascade/ua_rules.json` (180 rules) and must equal `parse_sheet(html.css)`.
- `book_rules_match_calibre_in_text_and_order` sorts the dump's rules by `key`'s last item (source index) before comparing with `parse_sheet`'s source order, and checks `Decl.important` against `important`.
- Follow the dump where it differs from the bullet list above: `panose-1` is kept in stylesheets, and an `@import` cycle can drop whole import chains (`css_quirks` shows which).

**Amended after the Tasks 5–6 review (these win over the interface above; Task 8 reads them):**
- `pub struct Rule { pub origin: Origin, pub selector: String, pub decls: Arc<Decls>, pub index: u32 }`: the selectors of one list share their declarations.
- `pub struct Sheet { pub rules: Vec<Rule>, pub font_face: Vec<Decls>, pub page: Vec<Decls>, pub next_index: u32 }` and `impl Sheet { pub fn normalize(&mut self, profile: &Profile) }`, which normalizes each shared block once (style rules and `@page`; `@font-face` stays as read).
- `pub fn parse_sheet(text: &str, origin: Origin, href: &str, first_index: u32, import: &mut dyn FnMut(&str) -> Option<String>) -> Result<Sheet, Error>;`: `href` is the sheet's address from the book's root (the document's for a `<style>`), and `url()`s come out book-absolute in one pass; `first_index` is the previous sheet's `next_index` (0 for a file's first sheet), so `Rule.index` runs on across a file's sheets as Calibre's does.
- Limits: `MAX_STYLE_RULES` (50,000) and `MAX_STYLE_DECLS` (150,000 units: one per declaration, a shorthand counting once per declaration it becomes, and one more per 64 bytes of a declaration, selector or stylesheet), charged as the text is read.

- [ ] **Step 1: Write the failing tests** in `tests/it/css_sheet.rs`, each with its expected strings taken from `css_quirks`'s `rules/NNN.json`:
  - `book_rules_match_calibre_in_text_and_order`: for every cascade fixture file, the port's `(origin, selector, decls)` list equals `rules/NNN.json`. The dump's declarations are already normalized (shorthands expanded), so the test is written here with `#[ignore = "needs Task 6"]` and passes after Task 6.
  - `serialize_value_cases`: the pairs listed above.
  - `media_queries`: `screen` true, `print` false, `screen and (min-width: 600px)` false, `amzn-kf8` true.
  - `import_cycle_ends`, `fifty_thousand_and_one_rules_is_too_large` (Review Focus 3).
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it css_sheet`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** on `cssparser`'s tokenizer and rule parser.
- [ ] **Step 4: Run.** Expected: PASS (one ignored).
- [ ] **Step 5: Commit** `feat(kfxgen): parse stylesheets into Calibre's rule form`.

### Task 6: `css::normalize`: shorthands and font-size keywords

**Files:**
- Create: `crates/deckle-kfxgen/src/css/{normalize.rs, font_shorthand.rs, defaults.rs}`, `tests/it/css_normalize.rs`

**Interfaces:**
- Produces: `pub fn normalize(decls: Decls, profile: &Profile) -> Decls;`, `pub struct Profile { pub fbase: f64, pub dpi: f64, pub width_pts: f64, pub height_pts: f64, pub fnames: [(&'static str, f64); 7], pub fnums: [f64; 7] }`, `pub const DEFAULT_PROFILE: Profile` (fbase 12, dpi 100, 1152 × 864, `xx-small 5, x-small 7, small 9, medium 12, large 13.5, x-large 17, xx-large 20`, fnums `5, 9, 12, 13.5, 17, 20, 22`), `pub static DEFAULTS: &[(&str, &str)]`, `pub static INHERITED: &[&str]` (both copied from `oeb/normalize_css.py` and `oeb/stylizer.py:63`).
- Expands `margin`, `padding`, `border`, `border-{top,right,bottom,left}`, `border-style`, `border-width`, `border-color`, `list-style`, `font` (port of `tinycss/fonts3.py parse_font`; unset parts take `DEFAULTS`). Does **not** expand `background`, `text-decoration`, `outline`.
- A keyword `font-size` becomes `format!("{:.1}rem", fnames[k] / fbase)`; the keyword test is case-sensitive. `-epub-x` is also written as `-webkit-x` and `x`.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- The `-epub-` copy applies only to `writing-mode` and `text-emphasis*`; other `-epub-` properties (`-epub-hyphens`) are kept as written.

- [ ] **Step 1: Write the failing tests**: `shorthands_expand_as_calibre` (table of input → expected `Decls`, from `css_quirks`'s `shorthand.xhtml` dump), `font_size_large_is_1_1rem`, `font_size_upper_case_keyword_is_left_alone`, `background_is_not_expanded`, `epub_prefix_is_copied`; un-ignore `book_rules_match_calibre_in_text_and_order`.
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it css_`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS, none ignored.
- [ ] **Step 5: Commit** `feat(kfxgen): expand shorthands and font-size keywords as Calibre does`.

### Task 7: `css::select`: selector matching with Calibre's exceptions

**Files:**
- Create: `crates/deckle-kfxgen/src/css/select/` (`mod.rs`, `tokens.rs`, `parse.rs`, `compile.rs`, `index.rs`, `eval.rs`), `tests/it/css_select.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Selector { /* parsed */ }
  pub enum Parsed { Ok { selector: Selector, pseudo: Option<&'static str>, specificity: (u32, u32, u32) }, Dropped }
  pub fn parse_selector(text: &str) -> Parsed;
  pub struct DocIndex { /* names, classes, ids, attributes, positions; work charged */ }
  impl DocIndex { pub fn new(dom: &Dom) -> DocIndex; pub fn work(&self) -> u64; }
  pub struct TooLarge;                                             // past MAX_SELECTOR_WORK
  pub fn select(sel: &Selector, dom: &Dom, index: &DocIndex) -> Result<Vec<NodeId>, TooLarge>;  // document order
  ```
- *Amended in the Task 7 review:* no `selectors` crate (each compound's tests are matched by the port itself, class and id names ASCII case-insensitively, attribute values case-sensitively, namespaces ignored) and no per-element `matches()` (it cost a pass over the document per call). Build one `DocIndex` per document and call `select` once per rule; `select` starts each compound from the smallest set of the index that holds it and charges its work to the index, giving `TooLarge` for every call once the document's total passes `deckle_limits::MAX_SELECTOR_WORK`. A selector over 64 KiB is `Dropped`.
- Calibre's exceptions (`css_selectors/select.py`, `INAPPROPRIATE_PSEUDO_CLASSES`): `hover, link, visited, active, focus, before, after, first-letter, first-line` (and the rest of that list) match every element and are reported in `pseudo`; any other unknown pseudo makes the selector `Dropped`.
- A selector with more than 256 compound parts is `Dropped`.

- [ ] **Step 1: Write the failing tests**: `selector_matches_agree_with_calibre` (for `css_quirks`'s `selectors.xhtml`: 40 selectors covering combinators, attribute forms, `:is`, `:where`, `:not`, `:has`, `:nth-child(… of S)`, `:lang`, `:empty`, `:root`, `*-of-type`, upper-case class names; the expected matched paths are read from the dump, where each selector sets `orphans: <n>` so its matches show in `specified`), `specificity_cases` (`:where` is 0, `:not(#x)` counts the id), `hover_rule_reports_pseudo_and_matches_all_p`, `marker_rule_is_dropped`, `has_over_a_deep_tree_finishes` and `thousand_part_selector_is_dropped` (Review Focus 3).
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it css_select`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS. A selector the crate and Calibre disagree on is fixed in the `Element` impl or recorded in `DIVERGENCES.md`.
- [ ] **Step 5: Commit** `feat(kfxgen): match selectors as Calibre's css_selectors does`.

### Task 8: `css::cascade`: the first Stylizer pass, and the chapter marks

**Files:**
- Create: `crates/deckle-kfxgen/src/css/cascade.rs`, `crates/deckle-kfxgen/src/dom/structure.rs`, `tests/it/css_cascade.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Cascaded { pub specified: Vec<Decls>,                 // indexed by NodeId
      pub pseudo: Vec<Vec<(&'static str, Decls)>>, pub rules: Vec<Rule>, pub font_face: Vec<Decls> }
  pub fn cascade(dom: &mut Dom, sheets: Vec<Sheet>, profile: &Profile) -> Result<Cascaded, Error>;
  pub fn mark_chapters(dom: &mut Dom);                               // DetectStructure
  ```
- `cascade` ports `Stylizer.__init__` (`oeb/stylizer.py:277-394`): sheet order `html.css`, then each `<style>` and `<link rel=stylesheet>` in document order; rules sorted by `(origin, a, b, c, index)` ascending and applied in order; a later rule overrides an `!important` property only when it is important too (so a user-agent `!important` loses to a book `!important`); `style` attributes applied last; a rule whose selector reports a pseudo goes to `pseudo` and not to `specified`; `img` `width`/`height` attributes become declarations and are removed from the tree unless CSS sets a non-auto width or height.
- `mark_chapters` ports the default chapter test of `oeb/transforms/structure.py:53`: before each `h1` or `h2` whose text matches `chapter|book|section|part` followed by a space (case-insensitive), or any element with `class="chapter"`, insert `<div style="display: block; page-break-after: always">`, skipping matches that are among the first two elements at the start of a file. It runs before `cascade`.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- The user-agent `!important` case is `frameset { float: right !important }` in `css_quirks`'s `cascade.css`: the book's value wins this first pass (see `rules/004.json`).
- Also here: `pub fn generated_toc(files: &mut [(&str, &mut Dom)]) -> Vec<TocNode>;` ports the contents Calibre builds when the book has none (`oeb/transforms/structure.py`), adding the `calibre_toc_N` ids it adds. Test `generated_contents_match_calibre` against `no_toc/oeb.json` (`Chapter One` → `OEBPS/one.xhtml#calibre_toc_2`, `Part Two` → `OEBPS/two.xhtml#calibre_toc_3`).

- [ ] **Step 1: Write the failing tests**: `important_and_order_cases` (from `css_quirks`'s `cascade.xhtml`: important beats a later rule and the `style` attribute; equal specificity takes the later; user-agent `hr` height present), `img_size_attributes_fold_into_css`, `chapter_divs_are_inserted_where_calibre_inserts_them` (after `mark_chapters`, element paths equal the dump's for every cascade fixture file), `font_face_rules_match_calibre` (against `rules/NNN.json`).
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it css_cascade`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Index rules by their rightmost tag, class or id so matching is not rules × elements.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(kfxgen): cascade the book's CSS as Calibre's first pass does`.

### Task 9: `css::flatten`: what kfxgen's Stylizer is given

**Files:**
- Create: `crates/deckle-kfxgen/src/css/{flatten.rs, fake_margins.rs}`, `tests/it/css_flatten.rs`

**Interfaces:**
- Produces:
  ```rust
  // As built (Task 9): declarations shared through `Arc`, the second cascade bounded by the
  // first's limits (so `flatten` can refuse a file), and <body>'s style set before the first
  // cascade, where Calibre sets it.
  pub struct Flat { pub specified: Vec<Arc<Decls>>, pub pseudo: Vec<Vec<(&'static str, Decls)>>, pub class: Vec<Option<Arc<str>>>, /* crate-private: ua, main */ }
  impl Flat { pub fn specified(&self, n: NodeId) -> &Decls; pub fn specified_arc(&self, n: NodeId) -> &Arc<Decls>;
              pub fn pseudo(&self, n: NodeId) -> &[(&'static str, Decls)]; pub fn class(&self, n: NodeId) -> Option<&str>; }
  #[derive(Default)] pub struct ClassNamer { /* book-wide, spine order */ }  impl ClassNamer { pub fn units(&self) -> usize; }
  pub fn prepare_body(dom: &mut Dom);                                // stylize_spine: before `cascade`, after `mark_chapters`
  pub fn flatten(dom: &mut Dom, href: &str, first: Cascaded, namer: &mut ClassNamer, profile: &Profile) -> Result<Flat, Error>;
  pub fn remove_fake_margins(namer: &mut ClassNamer, docs: &mut [(&Dom, &mut Flat)]);   // spine files, spine order
  ```
- Per file, in spine order: `dom::parse`, `mark_chapters` (and `generated_toc` over the book), `prepare_body`, the file's sheets, `cascade`, `flatten` (take `rules` and `font_face` out of the `Cascaded` first); then `remove_fake_margins` once over the spine. `flatten` also removes every `<style>` and stylesheet `<link>` (`flatten_head`), so the tree's paths are the dump's but for the two `<link>`s Calibre appends to `<head>`. `flatten` gives `Error::Cascade`/`Error::Selectors` when both cascades together pass `MAX_CASCADE_DECLS`/`MAX_SELECTOR_WORK`, and `Error::TooLarge` past `MAX_STYLE_RULES` classes in the file.
- `flatten` ports `oeb/transforms/flatcss.py` for kfxgen's settings (`disable_font_rescaling = true`, `minimum_line_height = 0`, `line_height = 0`): `html@style` moves to `body`; `body` gets `margin-top: 0; margin-bottom: 0; margin-left: 5pt; margin-right: 5pt; padding-left: 0; padding-right: 0`; `align` becomes `text-align` (on `img`: `vertical-align` or `float`); `<font>` becomes `div` or `span` with `size` as pt from `fnums`; `color` and `bgcolor` attributes become CSS; `ol@type` is deleted; `font-weight: medium` → `normal`; `in-line` → `inline`; `vertical-align: sup` → `super`; `style` and `class` attributes are removed and the new class set. Elements outside XHTML and SVG keep their attributes and get no class.
- Class names: the element's first class with digits removed, lowercased (`Note2 big` → `note`), numbered `note`, `note1`, `note2` … in order of first use across the book; no class gives `calibre`, `calibre1` …; a pseudo dict gives `pcalibre`, `pcalibre1` …; two elements with the same sorted declarations share a class.
- `Flat.specified` is what kfxgen's Stylizer reports: the element's flattened declarations cascaded again over `html.css` (book origin wins).
- `remove_fake_margins` ports `oeb/transforms/page_margin.py:49` and runs over the whole book after every file is flattened.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- The second cascade over `html.css` respects importance: the flattened declarations never carry `!important`, so a user-agent `!important` value wins there (`frameset`, `frame`, `spacer`: the dump's `specified` has `float: none` for the framesets). Otherwise the book's value wins.
- Class lists are compared as sorted sets (Calibre writes them in random order; the dump sorts them).
- References: class naming in `css_quirks`'s `flatten.xhtml` (`Note2 big` → `note`, `note3` → `note1`, `NOTE` with a style attribute → `note2`); fake margins in `fake_margins` (`all.xhtml`: removed from all 30; `half.xhtml`: kept on 15). `RemoveFakeMargins` counts per tag and depth below `<body>` over the whole book and edits the shared class.

- [ ] **Step 1: Write the failing tests**:
  - `every_cascade_fixture_specified_matches_kfxgens_stylizer`: for each fixture file, for each node, `Flat.specified` as a sorted map equals the dump's `specified`, `Flat.pseudo` equals `pseudo`, and the element's `class` attribute equals the dump's.
  - `invalid_values_are_kept` (`margin-left: foo`, `height: 10 px`; Review Focus 5), `body_margins_are_added`, `font_tag_size_plus_one_is_13_5pt`, `fake_margin_shared_by_95_percent_is_removed`.
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it css_flatten`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS for every fixture; a mismatch that stays is a `DIVERGENCES.md` section and a named allow-list entry.
- [ ] **Step 5: Commit** `feat(kfxgen): flatten styles to what kfxgen's Stylizer sees`.

### Task 10: `css::style`: Calibre's `Style` values

**Files:**
- Create: `crates/deckle-kfxgen/src/css/style.rs`, `tests/it/css_style.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum Value { Pt(f64), Text(String) }
  pub struct Styles<'a> { /* dom, flat, profile, caches */ }
  impl<'a> Styles<'a> {
      pub fn get(&self, n: NodeId, prop: &str) -> Option<&str>;             // Style.get: own declared value
      pub fn inherited(&self, n: NodeId, prop: &str) -> Result<&str, Error>; // Style._get; Err(NoDefault) where Calibre raises KeyError
      pub fn computed(&self, n: NodeId, prop: &str) -> Result<Value, Error>; // Style[prop]
      pub fn pseudo(&self, n: NodeId, name: &str) -> Option<&Decls>;
  }
  ```
- Ports `oeb/stylizer.py:518-944` and `unit_convert` (`ebooks/__init__.py:258`): unitless and `px` × 72/dpi; `in` × 72; `pt` as is; `em` × font size; `ex`/`en` × font size × 0.5; `pc` × 12; `mm` × 2.8346457; `cm` × 28.346457; `rem` × 12; `%` of the base; unparseable returned as `Text`. `font-size`: keywords from `fnames`; `smaller`/`larger` by the table, else ÷ or × 1.2; `%` and `em` of the parent; negative becomes `smaller`; unparseable inherits. `line-height`: `normal` is 1.2 × font size, a number × font size, and with no declaration the **parent's computed pt**. `width`/`height`: the attribute beats CSS, `%` of the parent, the root is `width_pts`/`height_pts`. Margins and padding `%` of the parent's width. `color` falls back to `black`. `initial` and `unset` come back as text.
- Caches are per node; parent lookups are loops, not recursion.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- No dump holds `{"error": …}`: all `COMPUTED_PROPS` have a Calibre default. Keep `Err(NoDefault)` for other properties, with a unit test that needs no dump.
- `nan` and `1e999` are read as numbers by Calibre; the dump writes them `{"number": "nan"}` and `{"number": "inf"}` (`css_quirks/styles/006.json`). `Value::Pt` carries NaN and infinity; the comparison treats NaN as equal to NaN.

- [ ] **Step 1: Write the failing tests**:
  - `every_cascade_fixture_computed_matches_kfxgens_stylizer`: for each node and each of `COMPUTED_PROPS`, `computed` equals the dump: numbers within 1e-9, text equal, `{"error": …}` ↔ `Err`.
  - Named quirks from `css_quirks`'s `values.xhtml`: `twelve_px_is_8_64pt`, `font_size_large_is_13_2pt`, `line_height_inherits_as_points`, `text_indent_percent_is_of_own_width`, `font_weight_700_computes_to_504`, `width_attribute_beats_css`.
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it css_style`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(kfxgen): resolve style values as Calibre's Style does`.

### Task 11: One entry point, the corpus, and the outcome

**Files:**
- Create: `crates/deckle-kfxgen/src/book.rs`, `tests/it/book.rs`, `tests/it/corpus_cascade.rs`, `docs/superpowers/notes/2026-11-13-native-kfx-phase2-outcome.md`
- Modify: `crates/deckle-kfxgen/src/lib.rs`, `crates/deckle-kfxgen/DIVERGENCES.md`, `fixtures/kfxgen/README.md`

**Interfaces:**
- Produces, for phase 3:
  ```rust
  pub struct Book { pub package: Package, pub oeb: Oeb, pub files: Vec<File>, pub font_face: Vec<Decls> }
  pub struct File { pub href: String, pub dom: Dom, pub flat: Flat, pub first_pass_rules: Vec<Rule> }   // rules as in rules/NNN.json
  // Task 9 as built: `flatten` takes the `Cascaded` by value and drops what it does not use,
  // so take both `Cascaded::rules` (for `first_pass_rules`) and `Cascaded::font_face` (for
  // `Book::font_face`) out of it first; run the steps in Task 9's order, one `ClassNamer` per
  // book. `flatten` can refuse a file (`css::Error::Cascade`, `Selectors`, `TooLarge`) or the
  // whole book (`css::Error::BookCascade`: past `MAX_BOOK_CASCADE_DECLS` units over all files);
  // `read_book` surfaces them as `Error::TooLarge { what }`.
  impl File { pub fn styles<'a>(&'a self, profile: &'a Profile) -> Styles<'a>; }
  pub fn read_book(path: &Path) -> Result<Book, Error>;
  pub fn manifest_bytes(path: &Path, href: &str) -> Result<Vec<u8>, Error>;
  ```
- `Error` (thiserror): `Io`, `Zip`, `Xml`, `Encoding { file }`, `TooLarge { what }`, `Drm`, `NoDefault { prop }`.

**Amended after Task 1 (the dumps are the reference; where a line above disagrees, these win):**
- `kfxgen_models.sh --corpus` runs its conversions with `TZ=UTC` for the `cascade/` dump, as `--cascade` does (script change in this task). A corpus book has its own `cascade/ua_rules.json`.

- [ ] **Step 1: Write the failing tests**: `read_book_matches_every_dump` (runs the Task 3, 8, 9 and 10 comparisons through `read_book` for every cascade fixture), `read_book_twice_is_identical`, `dump_from_another_calibre_version_is_refused` (the harness reads `cascade/TOOLS`), and in `corpus_cascade.rs` an `#[ignore]` test that does the same for `$DECKLE_KFXGEN_CORPUS/<book>/cascade` and prints the time per book.
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it book`. Expected: FAIL to compile.
- [ ] **Step 3: Implement `book.rs`.**
- [ ] **Step 4: Run** `cargo test -p deckle-kfxgen --test it`, then `tools/ref/kfxgen_models.sh --corpus` and `DECKLE_KFXGEN_CORPUS=gutenberg/models cargo test --release -p deckle-kfxgen --test it corpus -- --ignored`. Expected: PASS; each text-only corpus book under 1 s in `read_book`.
- [ ] **Step 5: Run the workspace checks**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Expected: clean, with only the known environment failures.
- [ ] **Step 6: Write the outcome note**: what matches, every `DIVERGENCES.md` section added, timings, fixture size, and what phase 3 must do first (kfxgen's resolver, the `data-kfxgen-*` marks, the spine reorder, and the "Before phase 3" list from the phase 1 note).
- [ ] **Step 7: Commit** `feat(kfxgen): read_book, checked against Calibre on fixtures and corpus`.

## Done when

- `cargo test -p deckle-kfxgen --test it` passes with every cascade fixture compared at four levels: book view, element tree, declared styles, computed values.
- The 9-book corpus passes the same comparisons, with every exception listed in `DIVERGENCES.md`.
- `fixtures/kfxgen` is under 10 MB; `cargo fmt`, clippy and the workspace tests are clean.
- The design names the corrected oracle, and the outcome note exists.

## Not in this plan

- kfxgen's resolver, marks and spine reorder (phase 3). Resolver dumps are not captured yet; phase 3's plan adds them to the same hook.
- Any call from the app, the CLI or `deckle-core` into this code (phase 4). Crediting Calibre in the app's About view belongs there too.
- MathML, fixed layout, image conversion.
- Matching Calibre on XHTML that is not well-formed (Review Focus 1).
