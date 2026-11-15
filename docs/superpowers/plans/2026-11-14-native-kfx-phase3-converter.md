# Native KFX, phase 3 (the converter) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `deckle_kfxgen::convert::convert(epub)` returns the arguments kfxgen's `convert_oeb_to_kfx` hands its generator (the chapter model), equal to kfxgen's own dump for every source fixture and the 9 Gutenberg books, so that `convert` then `generate` makes a Kindle book from an EPUB with no Python, Calibre or Previewer.

**Architecture:** A new module `convert` in `crates/deckle-kfxgen`, a port of kfxgen's `converter.py`, `inline_style.py`, `font_table.py` and `image_optimize.py` (about 11,600 Python lines). It reads phase 2's `Book` (tree and styles as Calibre gives them) and writes the `PyValue` tree phase 1's `generate` reads. Each layer is tested against a dump taken inside a real kfxgen conversion before the layers above it exist: resolver per element, inline runs per paragraph, blocks per file, contents entries, chapters.

**Tech Stack:** Rust. No crate new to `Cargo.lock`: `image` (workspace; this crate adds the `gif` feature), `sha1`, `flate2`, `regex`. Python only in `tools/ref/`.

**Spec:** `docs/superpowers/specs/2026-11-10-native-kfx-design.md`. Research behind this plan, to read before the task that names it: `docs/superpowers/notes/2026-11-14-native-kfx-phase3-research-{book,blocks,inline}.md` (called R-book, R-blocks, R-inline below). Their line numbers are at kfxgen `0bc705d`; after Task 1 find functions by name.

## Decisions (all six accepted by the owner on 2026-11-14)

1. **The pin moves first (Task 1).** kfxgen has 10 fixes after `0bc705d` (to `ad38ec8`): about 630 changed lines in the converter and 1,000 in the generator. Task 1 ports the generator changes and regenerates the fixtures, so the converter is ported once, from the newer text. **Accepted by the owner on 2026-11-14**, who had just finished kfxgen's last major open item (`ad38ec8` is kfxgen's newest commit). The pin then stays at `ad38ec8` until phase 3 ends; newer kfxgen commits wait for a sync after it.
2. **Refused, not ported:** drawing SVG pictures (an SVG file in the manifest, an inline `<svg>` of shapes only), WOFF2 fonts, MathML. Each gives `Error::Unsupported` naming the feature and the file. Reasons: R-inline §4 and §5 (Qt draws SVG its own way and the picture's name is a hash of lxml's text; fontTools recompiles WOFF2 glyphs, so bytes cannot match without porting it). Phase 4 decides the fallback for such a book. Follow-ups: jloutsch/deckle#96 (SVG), #97 (WOFF2).
3. **Blocks are ordered maps, not typed structs.** The model is compared with Python's key order (R-blocks §2), and `generate` reads `PyValue`. A typed block would be a second contract to keep equal.
4. **Fixture cap raised from 10 MB to 12 MB**, with the new dumps stored gzip-compressed. `fixtures/kfxgen` was at 9.1 MB before Task 1 and 11.5 MB after it (86 models), so Task 2 also stores each `model.json` compressed (6.2 MB of JSON becomes about 0.2 MB).
5. **Deferred from phase 1's "Before phase 3" list:** splitting `build_chapter_content` and the large `generate` modules, and the small structure items. The converter does not touch them. The allow-list item is done (Task 3).
6. **Generator features still unsupported after this phase** (a real book reaching one gets `GenerateError::Unsupported`): a shift inside another (`<sup>` in a superscript span), block styles and margins on paragraphs in table cells, pictures in table cells and list items, preformatted text, `clear`. Task 23 counts how many fixture and corpus books reach each; porting them is a separate plan. Follow-ups: jloutsch/deckle#98 to #102.

## Global Constraints

- Pinned references: kfxgen `ad38ec8` once Task 1 lands (`0bc705d` before), Calibre **9.18.0**. A dump made with another Calibre is refused by the test harness (`support::check_tools`).
- The kfxgen repository is only read. Hook code lives in `tools/ref/kfxgen_hook/` and is injected into an exported copy.
- Calibre runs only through `tools/ref/kfxgen_models.sh`, only on `fixtures/kfxgen/sources/*.epub` and the nine `gutenberg/pg*.epub`. No agent reads, lists or runs anything over the owner's Calibre library or any other book collection.
- `deckle-kfxgen` must not depend on `deckle-core`.
- Licence: `// SPDX-License-Identifier: GPL-3.0-only` on every new source file. Code ported from Calibre or fontTools names its source file and function in a comment and is listed in `crates/deckle-kfxgen/NOTICE`.
- The model is the contract: dict keys in Python's insertion order, a key present with `None` kept apart from a key absent, `int` apart from `float`, floats equal bit for bit (so `-0.0` is not `0.0`). Python's rounding, formatting, whitespace and code-point rules through `generate/py.rs` (R-blocks §4, R-inline §2); near-duplicate formulas are each ported as written.
- Deterministic: no hash-map iteration order, time or randomness reaches a result. Run serials (ruby, tcy, bidi) count in walk order across the whole book.
- Limits: every XML parse through `deckle_limits`; own recursion stops at `deckle_limits::MAX_DEPTH` with `Error::TooLarge`; no step's time grows with the square of a file's size without a counted budget. Each new limit gets a row in `docs/books.md`.
- Every deliberate difference from kfxgen gets a section in `crates/deckle-kfxgen/DIVERGENCES.md`. Where Python swallows an exception and continues (R-book §4), the port returns the error, and that is one such section.
- Smallest complete change: no option, abstraction or helper a task's tests do not need. Reviewers list code no test reaches as deletion candidates.
- New committed fixtures and dumps: `fixtures/kfxgen` stays under **12 MB**. Corpus dumps stay outside git.
- No README or CHANGELOG entry: nothing calls `convert` yet, so nothing a user sees changes. No new network call.
- Commands: `export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"`; tests `cargo test -p deckle-kfxgen --test it <filter>`; before each commit `cargo fmt` and `cargo clippy -p deckle-kfxgen --all-targets -- -D warnings`.
- Branch `feat/native-kfx-converter` from `main`, one commit per task, messages `feat(kfxgen): …`, plain subjects that say what now works.

## Review Focus

1. **Markup nested very deep** (20,000 nested `<span>` or `<div>`). Expected: `Error::TooLarge` in under 5 s, no stack overflow. kfxgen's resolver gives `None` on Python's recursion limit; that difference is recorded. Tests in Tasks 8, 9 and 11.
2. **Sizes that multiply.** A table of 200,000 cells; a 30 MB paragraph; 50,000 siblings each asking its ancestors for font size, decoration and fill; a 100-megapixel picture. Expected: finishes under the cost limit (`support::COST_LIMIT`) or `Error::TooLarge`; pictures decode under `deckle-limits`' pixel limit. Tests in Tasks 8, 16 and 18.
3. **Text where Python and Rust differ.** A paragraph of only NBSP; U+001C in text; a tab in `<pre>`; a literal U+2028; astral characters before a link. Expected: the model kfxgen dumps for the `text_edges` fixture. Tests in Tasks 7 and 9.
4. **Hostile hrefs.** `%252e%252e/` encoded 16 times, `../../etc/passwd`, `file:` and absolute paths as picture, font or link targets. Expected: what `_is_unsafe_href` gives (an empty target), and nothing outside the archive is ever opened. Tests in Tasks 6 and 18.
5. **A book that shows nothing**, or whose every file fails. Expected: `Error::NoContent`, never a panic or an empty book. Test in Task 21.

---

## Stage A: groundwork

### Task 1: Move the pin to kfxgen `ad38ec8`

**Files:** Modify `tools/ref/kfxgen_models.sh` (`KFXGEN_MIN`), `crates/deckle-kfxgen/src/lib.rs`, `fixtures/kfxgen/README.md`, `crates/deckle-kfxgen/src/generate/*`, `DIVERGENCES.md`; regenerate `fixtures/kfxgen/{models,cascade}`; the design's Constraints line.

**Interfaces:** Produces every fixture model, `expected.kfx` and cascade dump at `ad38ec8`. `generate` is byte-identical on them again.

- [ ] **Step 1:** Regenerate fixtures and the corpus at the new commit (`KFXGEN_REF=ad38ec8 tools/ref/kfxgen_models.sh`, then `--cascade`, then `--corpus`). Record which of the 70 models and 13 cascade dumps changed.
- [ ] **Step 2:** Run `cargo test -p deckle-kfxgen`. Expected: generator tests fail on the changed models; cascade tests pass unchanged (the 10 commits do not touch `plugin/__init__.py`). A cascade failure stops the task for a ruling.
- [ ] **Step 3:** Port the generator changes commit by commit, oldest first (`git log --reverse 0bc705d..ad38ec8 -- plugin/kfxgen/native_generator.py`), one commit each, until every fixture and the corpus are byte-identical. Add kfxgen's own new test books for these fixes (`lists_768`, `lists_771`, `block_picture_769`, `block_picture_770`, `chapter_starts_773`, `chapter_starts_777`, `lists_horizontal_items_764`, `horizontal_blocks_749`) as sources where an existing fixture does not cover the change.
- [ ] **Step 4:** `DECKLE_KFXGEN_CORPUS=gutenberg/models cargo test -p deckle-kfxgen --release -- --ignored corpus`. Expected: PASS, 0 mismatches.
- [ ] **Step 5:** Commit(s).

### Task 2: Dumps and the comparison harness

**Files:** Create `tools/ref/kfxgen_hook/convert_dump.py`, `tools/ref/kfxgen_hook/test_convert_dump.py`, `tools/ref/make_convert_epubs.py`, `crates/deckle-kfxgen/tests/it/convert_support.rs`. Modify `tools/ref/kfxgen_hook/inject.py`, `tools/ref/kfxgen_models.sh` (`--convert`), `tools/ref/README.md`, `fixtures/kfxgen/README.md`.

**Interfaces:** Produces, per source fixture, under `fixtures/kfxgen/convert/<name>/` (gzip JSON, the encoding of `model_dump.py`), when `KFXGEN_DUMP_CONVERT=<dir>` is set. All are taken by wrapping module-level names of `converter` (the call sites look them up at call time):
- `resolver/NNN.json.gz` (**D1**): for each spine file, every element in document order as `[path, resolve(e), compute_block_style(resolve(e))]`. Wraps `_build_style_resolver`; one dump per href.
- `inline/NNN.json.gz` (**W**): for each `_walk_inline` call with `is_root=True`: `[path, segments (table-split elements left out), normalize_runs_with_anchors(segments)]`, in call order.
- `blocks/NNN.json.gz` (**D2**): `extract_blocks_from_html`'s return value deep-copied at return, with `nav_listing_at`, `tables_seen` and `notes_seen` as indexes, `trailing_ids`; and **D2a**, each `_space_between` call's input blocks with their `margins` and `paddings` entries inline and the call depth.
- `toc.json.gz` (**T**): `_extract_toc_with_hrefs` after respelling, the spine hrefs after `_spine_in_order`, `_note_target_ids` per file, `_note_pair_ids`, `_book_link_targets` sorted.
- `assembly.json.gz` (**A**): per pass, `spine_items_ordered` deep-copied before assembly, `cover_href`, and the chapters right before `_replace_title_page`.
- `calibre_opf.xml`, `calibre_ncx.xml`: the OPF Calibre wrote (`container.opfname`) and the NCX it names, verbatim.
- `units/inline_style.json` (one file, not per book): a table of inputs and outputs of the pure functions of `inline_style.py` and `font_table.py` (`parse_css_length`, `parse_vertical_align`, `css_rgb`, `css_text_colour`, `css_link_colour`, `css_letter_spacing`, `normalize_runs_with_anchors`, `parse_weight`, `emitted_name`), made by importing the exported kfxgen in plain Python, no Calibre.
- A model (`models/<name>`) for each source that has none: `css_quirks`, `fake_margins`, `malformed`, `no_toc`, `package_quirks`.
- New sources from `make_convert_epubs.py`, each a few kilobytes, each named for what it holds: `text_edges` (Review Focus 3, nested shifts, `not-bold`, `sub`, `line-through`), `block_edges` (`pre`, hidden elements, `epub:switch`, page breaks, `clear`, a comment splitting a paragraph), `tables_more` (rows path, notes table, split row, caption, colspan and rowspan, paragraph cells), `pictures_more` (a GIF, a CMYK JPEG, a 2,400 px PNG, an `<svg><image>` wrapper, a CSS background page), `fonts_woff` (one WOFF1 face), `package_more` (guide-only landmarks, NCX `pageList`, a year-only date, a publisher, a `class="toc"` listing, a dedication), `marks` (link colours, a horizontal block in vertical text).
- Every `fixtures/kfxgen/models/<name>/model.json` becomes `model.json.gz` (the script writes it so; `load` reads either, with `flate2`, already in `Cargo.lock`); corpus models outside git stay plain.
- `convert_support.rs`: `pub fn model_diff(ours: &PyValue, dump: &PyValue) -> Vec<String>` (paths of every difference; dict order, key presence, int against float and float bits all count; `$bytes` by hash), and `pub fn dump(name: &str, file: &str) -> PyValue`.

- [ ] **Step 1:** Write `test_convert_dump.py` (each wrapper leaves the conversion's `model.json` byte-identical to a run without it) and `convert_support::model_diff_sees_order_presence_and_float_bits`. Run: FAIL.
- [ ] **Step 2:** Write the hook, the injector anchor, `--convert`, the sources and the harness.
- [ ] **Step 3:** Run `tools/ref/kfxgen_models.sh --convert`; both tests PASS; `du -sh fixtures/kfxgen` under 12 MB. If the dumps still do not fit, keep D1 for 10 named fixtures only and say which in the README.
- [ ] **Step 4:** Commit.

### Task 3: The generator names every key it reads

**Files:** Modify `crates/deckle-kfxgen/src/generate/chapters.rs` (and the readers it calls); test `tests/it/generate_skeleton.rs`.

**Interfaces:** `BLOCK_UNPORTED` and `CHAPTER_UNPORTED` become allow-lists: a chapter, block, box, rule, table, row or cell key outside the keys the port reads or knowingly ignores (`_spine_file`, `_from_spine`, `_had_nav_listing`, `_nav_listing_at`, `anchor_ids`, `row`, `front_matter`, `break_before`, `own_bottom_em`, `bottom_in_em`, `outer_em_ratio`, `padding_below`; R-book §3, R-blocks §2) is `GenerateError::Unsupported` naming the key.

- [ ] **Step 1:** Test `an_unknown_block_key_is_refused_by_name` (a `minimal` model with `"surprise": 1` on a block gives `Unsupported` containing `surprise`). Run: FAIL.
- [ ] **Step 2:** Implement; all 70+ fixtures and the corpus still pass.
- [ ] **Step 3:** Commit.

## Stage B: what phase 2 must also supply

### Task 4: The tree as lxml shows it

**Files:** Modify `crates/deckle-kfxgen/src/dom/mod.rs`, `dom/parse.rs`; test `tests/it/dom.rs`.

**Interfaces (R-blocks §5):** Produces on `Dom`: `subtree(n) -> impl Iterator<Item = NodeId>` (the node, then its descendants in document order), `ancestors(n)`, `itertext(n) -> String`, `attr_ns(n, namespace_uri, local) -> Option<&str>`, `qualified(n) -> (Option<&str>, &str)`, and comments and processing instructions kept as child nodes of kind `Comment` that `children`, `prev_sibling` and `next_sibling` return and `descendants` of elements skips, so `children(n).len()` equals lxml's `len(elem)`.

- [ ] **Step 1:** Tests: `a_comment_is_a_child_as_in_lxml` (`<a id="x"><!--c--></a>` has one child; its text is unchanged), `attr_by_namespace_ignores_the_prefix` (`xmlns:e="http://www.idpf.org/2007/ops" e:type="footnote"` is found), `itertext_matches_the_dump` (for every cascade `dom/` file, the concatenated text equals the dump's). Run: FAIL.
- [ ] **Step 2:** Implement. Every phase 2 test still passes (the cascade comparison reads elements only).
- [ ] **Step 3:** Commit.

### Task 5: kfxgen's marks, made before the styles are flattened

**Files:** Create `crates/deckle-kfxgen/src/convert/marks.rs`. Modify `book.rs` (between `cascade` and `flatten`), `tests/it/book.rs` (the tree comparison also reads `data-kfxgen-*`), `tools/ref/kfxgen_models.sh` (`--cascade` also takes `link_colours`, `link_greys`, `horizontal_blocks`, `marks`, `package_more`).

**Interfaces:** `File::dom` carries `data-kfxgen-source` (root), `-toc`, `-front`, `-align`, `-link-colour`, `-v-author`, `-v-ua`, `-horizontal`, set in the plugin's order (`mark_source_classes`, `mark_link_state_colours`, `mark_vertical_margins`; `plugin/__init__.py` `specialize_css_for_output`), from the first cascade's rules and the tree before flattening.

- [ ] **Step 1:** Extend `read_book_matches_every_dump` to compare the eight attributes on every element. Run: FAIL on every fixture.
- [ ] **Step 2:** Port the three functions (`converter.py` 248-455, 4972-5013). Selector matching ignores `:link`, `:visited` and `:hover` the way Calibre's `Select(…, ignore_inappropriate_pseudo_classes=True)` does; check it against the `link_colours` dump.
- [ ] **Step 3:** Run the fixture comparison and the corpus: PASS, 0 mismatches.
- [ ] **Step 4:** Commit.

### Task 6: Calibre's package view and the book's arguments

**Files:** Create `convert/mod.rs`, `convert/options.rs`, `convert/package.rs`, `convert/href.rs`, `convert/args.rs`; test `tests/it/convert_args.rs`.

**Interfaces (R-book §2, §3):**
- `pub struct ConvertOptions { pub disable_native_tables: bool, pub disable_font_embedding: bool, pub open_at_cover: bool, pub disable_approximate_pages: bool, pub embed_original_images: bool }`, `Default` all `false`.
- `pub(crate) struct CalibreOpf { dates: Vec<String>, listed: HashSet<String>, guide: Vec<(String, String, String)>, nav_path: Option<String>, ncx: Option<Ncx> }`, built from `Book`: what kfxgen reads in the OPF Calibre rewrote (dates as Calibre serializes them, spine basenames without the cover page and duplicates, guide hrefs from the root, the NCX Calibre makes from a nav). Checked against `calibre_opf.xml` and `calibre_ncx.xml`.
- `pub(crate) fn book_args(book: &Book, opf: &CalibreOpf, opts: &ConvertOptions) -> Vec<(String, PyValue)>`: `title, author, authors, description, language, publisher, issue_date, rtl, writing_mode, landmarks, page_list, approximate_pages, resolved_image_refs`, as R-book §3's table.
- `href::{is_unsafe, normalize, resolve_doc_path, key_doc, resolve_link_target, external_target}`: ports of `_is_unsafe_href`, `_normalize_href`, `_resolve_doc_path`, `_key_doc`, `_resolve_link_target`, `_external_link_target`.

- [ ] **Step 1:** Tests: `book_arguments_match_every_model` (those thirteen arguments equal each source-built model's, by `model_diff`), `calibre_opf_facts_match_what_calibre_wrote` (dates, listed basenames, guide, NCX page list and labels read from the two dumped files equal `CalibreOpf`), `unsafe_hrefs_give_an_empty_target` (Review Focus 4's inputs). Run: FAIL.
- [ ] **Step 2:** Implement, with the 3-to-2 letter language table as the inverse of phase 2's.
- [ ] **Step 3:** PASS. Commit.

## Stage C: styles and text runs

### Task 7: The pure helpers of `inline_style.py`

**Files:** Create `convert/inline_style.rs`; modify `generate/py.rs` (`py_int`, and a generated table of the bidi classes L, R, AL from Unicode 16, made as the existing tables were); test `tests/it/convert_inline_style.rs`.

**Interfaces (R-inline §1, §3):** `normalize_runs(segments: &[Segment]) -> (String, Vec<Span>, Vec<(String, usize)>)` with `Segment { text: String, flags: Flags }`, `Flags` an ordered set of `Flag` (a string or a `(kind, PyValue)` pair) that writes itself as the model's `frozenset`; `parse_css_length`, `parse_vertical_align`, `css_rgb`, `css_text_colour`, `css_link_colour`, `css_typography`, `css_letter_spacing`, `is_monospace`, `page_bidi`, the flag makers.

- [ ] **Step 1:** Test `every_row_of_the_unit_table_matches` (each input of `units/inline_style.json` gives its recorded output). Run: FAIL.
- [ ] **Step 2:** Port `inline_style.py` 17-994. Offsets are code points; collapsible whitespace is ASCII `" \t\n\r\f"` only; colour blends round half to even.
- [ ] **Step 3:** PASS. Commit.

### Task 8: kfxgen's style resolver

**Files:** Create `convert/resolver.rs`, `convert/block_style.rs`; test `tests/it/convert_resolver.rs`.

**Interfaces (R-blocks §3):** `Resolver::new(file: &File, profile: &Profile) -> Resolver`; `resolve(&self, n: NodeId) -> Option<&Resolved>`; `Resolved::get(&self, key) -> Option<&PyValue>` keeps a key present with `None` apart from absent; `Resolved::to_py()`; `compute_block_style(css: &Resolved) -> PyValue`. Font size, decoration and fill are memoized per element, so a file costs one pass.

- [ ] **Step 1:** Tests: `resolver_matches_the_dump_for_every_element` (D1, both columns, every fixture that has one), `a_deep_tree_resolves_in_one_pass` (20,000 nested spans: under `COST_LIMIT` or `TooLarge`), `siblings_do_not_rewalk_their_ancestors` (50,000 siblings 200 deep). Run: FAIL.
- [ ] **Step 2:** Port `converter.py` 211-246, 457-666, 881-925, 1035-1244 and `inline_style.py` 995-1054.
- [ ] **Step 3:** PASS. Commit.

### Task 9: Text runs

**Files:** Create `convert/inline.rs`, `convert/quotes.rs`, `convert/bidi.rs`; test `tests/it/convert_inline.rs`.

**Interfaces (R-inline §1, §6 T2):** `Walk<'a> { book, file, resolver, serials: &mut Serials, base_href }`; `Walk::inline(&mut self, n: NodeId, split_tables: bool) -> Result<Vec<Part>, Error>` with `Part::Run(Segment)` or `Part::Table(NodeId)`; `Serials` counts ruby, tcy and bidi across the book. `<math>` gives `Error::Unsupported { feature: "math", file }`; an `<svg>` of shapes only gives `feature: "SVG pictures"`.

- [ ] **Step 1:** Tests: `runs_match_the_dump_for_every_paragraph` (W: segments and normalized result, in call order, every fixture), `math_is_refused_by_name`, `a_span_nested_20000_deep_is_refused_not_overflowed`. Run: FAIL.
- [ ] **Step 2:** Port `_walk_inline` and its helpers: shifts (97-136), links (1841-1990), white space and emphasis (1994-2040), link colours (2152-2280), `_draws_as_block`, quotes (2306-2488), ruby, tcy, anchors (3272-3291), `_is_hidden`, `_epub_switch_branch`, inline bidi (752-814).
- [ ] **Step 3:** PASS. Commit.

### Task 10: Drop caps and pictures in text

**Files:** Create `convert/initial.rs`, `convert/pictures.rs`; extend `tests/it/convert_inline.rs`.

**Interfaces:** `initial(…)` (port of `_initial`, `_with_initial`, `_first_letter_length`, `_leading_float`); `img_token(href, alt, size, float, extra) -> String` and the size, float and extras helpers (1296-1500); `css_background_image`; `svg_image_refs`, `svg_text_paragraphs`, `svg_draws_shapes_only`; `resolve_img_src`.

- [ ] **Step 1:** The W comparison now covers `drop_caps`, `typography`, `figures`, `floats`, `floats_697`, `frames`, `horizontal_blocks`, `pictures_more` (they were skipped in Task 9 by name). Run: FAIL on those.
- [ ] **Step 2:** Implement. PASS with no fixture skipped.
- [ ] **Step 3:** Commit.

## Stage D: blocks

Each task of this stage extends `convert/blocks/` and one test, `tests/it/convert_blocks.rs`: `blocks_match_the_dump` compares, for each fixture in the task's list, every spine file's blocks with D2 (Task 11: with D2a, the blocks before spacing). A fixture joins the list in the task that makes it pass and never leaves it. Read R-blocks §1, §2 and §7 first.

### Task 11: The walk

**Files:** Create `convert/blocks/{mod.rs, walk.rs, shown.rs, anchors.rs}`, `convert/block_bidi.rs`.

**Interfaces:** `pub(crate) fn extract_blocks(w: &mut Walk, opts: &ExtractOptions) -> Result<FileBlocks, Error>`; `ExtractOptions { native_tables: bool, toc_targets: &HashSet<String>, link_targets: &HashSet<String>, keep_nav_listings: bool }`; `FileBlocks { blocks: Vec<Block>, nav_listing_at, tables_seen, notes_seen, trailing_ids }`; `Block` is an ordered map with a stable id, so margins can be kept beside the blocks as Python's `_ById` does. The function changes nothing in `Book`, so a second pass equals a fresh run.

- [ ] **Step 1:** List: `fonts`, `rtl`, `link_colours`, `link_greys`, `navigation`, `notes`, `page_list`, `vertical`, `vertical_contents`, `ruby_tcy`, `block_edges`, against D2a. Add `a_div_nested_20000_deep_is_refused_not_overflowed`. Run: FAIL.
- [ ] **Step 2:** Port body selection, `_walk`, `_walk_flow` (not `li`), the leaf and container branches of `_walk_element`, `_walk_container` and `_flush_inline`, hidden and non-rendered elements, `epub:switch`, `_shown_tree`, the contents-listing discard, picture blocks, `_bidi`, `_with_bidi`, `_start_end_align`, `_first_strong`, page breaks, `clear`, front matter, trailing ids, `_attach_anchor_keys`. List indexes in `break_at`, `clear_at` and `nav_listing_at` keep Python's meaning across splices (R-blocks §7.2).
- [ ] **Step 3:** PASS. Commit.

### Task 12: Margins and spacing

- [ ] **Step 1:** The same list, now against D2. Run: FAIL.
- [ ] **Step 2:** Port `_note_margins`, `_note_padding`, `_box_share`, `_carry_container_margins`, `_margin_em`, `_padding_em`, `_font_ratio`, `_collapse` (the first of equal arguments wins, so `-0.0` survives), spacers, `_emit_space`, `_place_margin`, `_space_between` (not its box part), `_mark_tight_runs` into `convert/blocks/spacing.rs`.
- [ ] **Step 3:** PASS. Commit.

### Task 13: Lists

- [ ] **Step 1:** Add `lists` and the list fixtures Task 1 added. Run: FAIL.
- [ ] **Step 2:** Port the list code as it stands at the pin (markers and ordinals at `0bc705d`; kfxgen #768 rewrote it as list entries) into `convert/blocks/lists.rs`.
- [ ] **Step 3:** PASS. Commit.

### Task 14: Rules

- [ ] **Step 1:** Add `rules_spaces`. Run: FAIL.
- [ ] **Step 2:** Port `_emit_rule`, `_rule_look`, `_css_border`, `_border_colour`, `_box_background`, `_background_parts` into `convert/blocks/rules.rs`.
- [ ] **Step 3:** PASS. Commit.

### Task 15: Boxes

- [ ] **Step 1:** Add `boxes`, `colour`, `dark_boxes`, `drop_caps`, `typography`, `figures`, `floats`, `floats_697`, `frames`, `marks`. Run: FAIL.
- [ ] **Step 2:** Port `_make_box`, `_note_box_margins`, `_box_width`, `_box_width_px`, `_box_padding`, `_has_content`, the box part of `_space_between`, and box kids in anchors and trailing ids into `convert/blocks/boxes.rs`.
- [ ] **Step 3:** PASS. Commit.

### Task 16: Tables

- [ ] **Step 1:** Add `directions`, `font_sizes`, `horizontal_blocks`, `horizontal_blocks_ltr`, `links`, `ruby`, `spacers`. Add `a_table_of_200000_cells_stays_under_the_cost_limit`. Run: FAIL.
- [ ] **Step 2:** Port `_table_is_native`, `_table_block`, the cell functions, widths and columns, `_walk_cell` (it saves and restores the pending ids, markers and break indexes), `_emit_fostered`, captions, the contents fallback, anchors that follow rows, `_table_start_ids` into `convert/blocks/tables.rs`.
- [ ] **Step 3:** PASS. Commit.

### Task 17: The remaining block paths

- [ ] **Step 1:** Add `tables_more`, `pictures_more`, and every remaining source fixture, so the list is all of them. Run: FAIL.
- [ ] **Step 2:** Port the rows path, notes tables (`_is_notes_table`, `_anchors_follow_rows`), `_walk_split_row`, `_emit_svg_blocks` for `<svg><image>` and `<svg><text>`, background-image pages, `keep_nav_listings`.
- [ ] **Step 3:** PASS on every source fixture. Commit.

## Stage E: the book

### Task 18: Pictures and the cover

**Files:** Create `convert/images.rs`; test `tests/it/convert_images.rs`. Modify `Cargo.toml` of the crate (`image` with `gif`, `sha1`).

**Interfaces (R-inline §4):** `pub(crate) fn images(book: &Book, path: &Path, opts: &ConvertOptions) -> Result<Images, Error>`; `Images { cover: Option<Vec<u8>>, images: BTreeMap<String, Vec<u8>>, display_dims: Vec<(String, PyValue)>, warnings: Vec<String> }`. Pass-through for JPEG and PNG within 1,920 px and 1 MiB; GIF to PNG; a CMYK JPEG to RGB at quality 85; an oversize picture resized into 1,920 px by Calibre's `fit_image` arithmetic; an SVG file gives `Unsupported { feature: "SVG pictures" }`. The cover and its copy in `images` are encoded once, so their bytes are equal. kfxgen's environment variables are not read; the defaults are constants.

- [ ] **Step 1:** Tests: `picture_sets_match_every_model` (keys equal; bytes equal where the model's bytes are the EPUB's; otherwise format and dimensions equal, with the PNG-or-JPEG choice for an oversize opaque PNG allowed to differ and listed in `DIVERGENCES.md`), `the_cover_shares_bytes_with_its_copy`, `a_picture_outside_the_archive_is_never_opened`, `a_100_megapixel_picture_is_refused_before_decoding`. Run: FAIL.
- [ ] **Step 2:** Port `extract_cover_image`, `extract_images_from_oeb`, `_pictures_in_table_cells` and `image_optimize.py`.
- [ ] **Step 3:** PASS. Commit.

### Task 19: The font table

**Files:** Create `convert/fonts.rs`; test `tests/it/convert_fonts.rs`.

**Interfaces (R-inline §5):** `pub(crate) fn font_table(book: &Book, path: &Path, opts: &ConvertOptions) -> Result<PyValue, Error>`: the `FontTable` object as the model holds it (`faces`, then `_by_family`). WOFF1 is unpacked with `flate2` and written as fontTools' `SFNTWriter` writes it (table order, padding, checksums; ported from `fontTools/ttLib/sfnt.py`, named in `NOTICE`). A chosen WOFF2 face gives `Unsupported { feature: "WOFF2 fonts" }`.

- [ ] **Step 1:** Tests: `font_tables_match_every_model` (`fonts`, `fonts_woff`, and every other source fixture's empty table, by `model_diff`, bytes by hash), `woff2_is_refused_by_name`, `a_font_past_50_mib_is_skipped`. Run: FAIL.
- [ ] **Step 2:** Port `font_table.py`.
- [ ] **Step 3:** PASS. If the WOFF1 bytes differ from the dump because Calibre's fontTools writes differently from 4.60.1, stop for a ruling. Commit.

### Task 20: Contents entries, file order and notes

**Files:** Create `convert/toc.rs`, `convert/notes.rs`; test `tests/it/convert_toc.rs`.

**Interfaces (R-book §5 T3):** `toc_entries(book, opf) -> Vec<TocEntry { title, href, level }>`; `spine_in_order(book, opf, &entries) -> Vec<usize>` (indexes into `Book::files`); `note_target_ids`, `note_pair_ids`, `book_link_targets`, `footnote_keys`. Contents entries find their file by basename, the first match wins. Where kfxgen's order depends on Calibre's set order (two added files of one basename and no entry), the port orders by href, a `DIVERGENCES.md` section.

- [ ] **Step 1:** Tests: `contents_entries_and_file_order_match_the_dump` (T, every fixture), `footnote_keys_match_every_model`. Run: FAIL.
- [ ] **Step 2:** Port `_extract_toc_with_hrefs`, `_with_ncx_empty_labels`, `_spine_in_order`, `_respell_toc_hrefs`, `_note_target_ids`, `_note_pair_ids`, `_book_link_targets`, `_footnote_keys`.
- [ ] **Step 3:** PASS. Commit.

### Task 21: Chapters

**Files:** Create `convert/chapters.rs`, `convert/assemble.rs`; test `tests/it/convert_chapters.rs`.

**Interfaces (R-book §5 T4):** `pub(crate) fn assemble(items: Vec<SpineBlocks>, entries: &[TocEntry], …) -> Result<Vec<Chapter>, Error>`, `Chapter` an ordered map; the per-file loop (the generated inline contents skipped, vertical marks dropped for fixed layout, `writing_mode` and `_spine_file` on every block, orphan and trailing keys); the fallback of one chapter per file; `split_by_writing_mode`; the second pass with `keep_nav_listings`. A book with no shown block gives `Error::NoContent`.

- [ ] **Step 1:** Tests: `assembly_matches_the_dump` (A's `spine_items_ordered` read from Python in, chapters before the title-page step out, every fixture), `the_per_file_loop_feeds_assembly_what_python_fed_it` (our own blocks equal A's input), `a_book_that_shows_nothing_is_an_error`. Run: FAIL.
- [ ] **Step 2:** Port `extract_chapters_from_oeb`, `_assemble_chapters_by_coordinate`, `_section_heads` and their helpers (7341-7830), `_split_by_writing_mode`, `_fixed_layout`, `_drop_vertical_marks`.
- [ ] **Step 3:** PASS. Commit.

### Task 22: The contents page and the title page

**Files:** Create `convert/contents_page.rs`; extend `tests/it/convert_chapters.rs`.

- [ ] **Step 1:** Test `final_chapters_match_every_model` (A's chapters in, the model's `chapters` out; `directions`, `navigation`, `page_list`, `vertical_contents`, `package_more`, then all). Run: FAIL.
- [ ] **Step 2:** Port `_listing_without_contents`, `_replace_title_page`, `_nav_listing_contents_chapter`, the three splits, `_rebuild_contents_page`, `_keep_illustrations` (8338-8910). `chapters.index(ch)` compares by value, the rebuild by identity (R-book §6.5).
- [ ] **Step 3:** PASS. Commit.

## Stage F: the whole converter

### Task 23: `convert`, the corpus, and the outcome

**Files:** Modify `convert/mod.rs`; create `tests/it/convert_book.rs`, `tests/it/corpus_convert.rs`, `docs/superpowers/notes/2026-11-XX-native-kfx-phase3-outcome.md`; modify `DIVERGENCES.md`, `docs/books.md`, `fixtures/kfxgen/README.md`.

**Interfaces:**
- `pub fn convert(path: &Path, opts: &ConvertOptions) -> Result<Converted, Error>`; `pub struct Converted { pub model: Model, pub warnings: Vec<String> }` (kfxgen's log-only warnings: flattened tables, media overlays, text in SVG, dropped contents entries).
- `pub enum Error { Book(book::Error), Unsupported { feature: &'static str, file: String }, NoContent, TooLarge { what: &'static str } }`.

- [ ] **Step 1:** Tests: `convert_matches_every_model` (the whole argument tree by `model_diff`; pictures by Task 18's rule), `convert_then_generate_matches_expected_kfx` (byte-identical for every fixture whose pictures pass through), `corpus_convert::corpus_matches_kfxgen` (ignored; the nine books: model equal, KFX byte-identical, each text-only book under 2 s and all nine under 39 s in a release build). Run: FAIL.
- [ ] **Step 2:** Wire the stages in `convert_oeb_to_kfx`'s order. Fix what the corpus shows; a difference that stays is a `DIVERGENCES.md` section or a stop for a ruling.
- [ ] **Step 3:** Count, over fixtures and corpus, the books that reach each `GenerateError::Unsupported` and each `convert::Error::Unsupported`; write the table into the outcome note with timings, the list of deferred items, and what phase 4 must do first (the seven gates of the phase 2 note still stand).
- [ ] **Step 4:** `cargo test -p deckle-kfxgen`, the release run of the ignored cost tests, `cargo clippy --workspace`. Commit.

---

## Order and size

Tasks run in number order. 18 and 19 depend only on Task 6 and may move earlier if a later task is blocked. About 11,600 Python lines to port plus about 1,000 generator lines in Task 1; phase 1 was about 12,000 Rust lines in 61 commits. Estimate: 30 to 45 hours of agent time across 23 tasks with a review after each, most of it in Tasks 1, 9, 11, 16 and 21.
