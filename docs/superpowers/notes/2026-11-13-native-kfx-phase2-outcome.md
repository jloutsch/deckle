# Native KFX, phase 2 (EPUB reader and styles): outcome

Date: 2026-11-13. Branch `feat/native-kfx-epub-cascade` (38 commits on `main` `0ca92a2`, not pushed). Plan: `docs/superpowers/plans/2026-11-13-native-kfx-phase2-epub-cascade.md`. Design: `docs/superpowers/specs/2026-11-10-native-kfx-design.md` (its Cascade oracle row names the corrected oracle: kfxgen's own Stylizer, dumped inside the conversion).

## Result

- `deckle_kfxgen::book::read_book(path)` reads an EPUB and gives what kfxgen's `convert_oeb_to_kfx` receives from Calibre 9.18.0: the package files, Calibre's book view (with the contents Calibre generates when the book has none), each spine file's tree after the flattener, each element's declared and pseudo-class styles as kfxgen's `Stylizer` reads them, the computed values (`File::styles`), the first-pass rules, and the book's `@font-face` rules. `manifest_bytes(path, href)` reads any other file of the book through the same limits.
- **13 cascade fixtures** (`fixtures/kfxgen/cascade/`, 49 spine files) match through `read_book` at four levels: book view (`oeb.json`), element tree and first-pass rules (`dom/`, `rules/`: 306 book rules, 3 `@font-face` rules), declared styles and classes (every element, `styles/`), computed values (34 properties of every element, over 30,000 values). One allow-list entry is used, the one Task 3 made: `css_quirks`'s `page_css1` and `page_css2` ("One page_styles.css until the CSS is read").
- **The 9-book Gutenberg corpus** (`gutenberg/models/<book>/cascade`, outside git, made by `tools/ref/kfxgen_models.sh --corpus` in the checkout the test runs from; a `gutenberg/models` dumped before this task has no `cascade/` folders and no `TZ: UTC`, and the test refuses it) matches the same four comparisons with **no mismatch and no allow-list entry**: 294 spine files, 13,278 book rules, 22,519 elements, 765,646 computed values. The corpus has no `@font-face` rule and no book without a contents file, so those two paths are checked on fixtures only.
- A changed dump fails the comparison: with one `styles/` table entry's `font-size` and one `rules/` selector altered in a copy of pg11's dump, the corpus test failed with 48 mismatches (47 elements sharing that entry, one rule).
- Tests: `deckle-kfxgen` lib 17 + it 258 passed, 13 ignored (the 12 before, plus `corpus_cascade::corpus_matches_calibre`). The six fixture-wide comparison tests of Tasks 3, 8, 9 and 10 (and the fixture loop of `img_size_attributes_fold_into_css`) became check functions that `book::read_book_matches_every_dump` and the corpus test run on `read_book`; the per-step tests on hand-made pages stay, and the test harness reads stylesheets through the library (`book::file_sheets`, `book::stylesheets`).
- The test harness refuses a dump whose `TOOLS` names another Calibre than 9.18.0 or no `TZ: UTC` (`tests/it/support.rs` `check_tools`, tested by `book::dump_from_another_calibre_version_is_refused`).

## Timings

`read_book` per corpus book, release build, M-series Mac, one run, after the fix round after the re-review (`DECKLE_KFXGEN_CORPUS=gutenberg/models cargo test --release -p deckle-kfxgen --test it corpus_cascade -- --ignored --nocapture`, the EPUBs in `gutenberg/` or `DECKLE_KFXGEN_CORPUS_EPUBS`):

| Book | Spine files | Pictures | `read_book` (ms) | Style units used |
|---|---|---|---|---|
| pg11 | 13 | 1 | 20 | 11,695 |
| pg23 | 16 | 1 | 18 | 15,066 |
| pg43 | 11 | 1 | 12 | 9,394 |
| pg55 | 26 | 1 | 24 | 21,943 |
| pg74 | 39 | 167 | 54 | 39,262 |
| pg76 | 49 | 178 | 65 | 49,743 |
| pg84 | 30 | 1 | 29 | 25,504 |
| pg86 | 48 | 227 | 68 | 43,305 |
| pg98 | 62 | 33 | 65 | 51,196 |

Every text-only book (no picture but its cover) is under the plan's 1 s; the test fails past it in a release build. "Style units" is what the book used of `MAX_BOOK_CASCADE_DECLS` (`Book::style_units`); the most a fixture book used is 7,492. The units include the first-pass rules each file keeps (counted since the final fix pass), so they are higher than in the first run of this table. `read_book` parses each spine file twice: once in `oeb_view` for its links, once for the tree. It is not needed for these times; phase 3 can share the parse if the whole pipeline needs it.

Dumping the corpus (`tools/ref/kfxgen_models.sh --corpus`, which also makes phase 1's models) took 82 s and 186 MB; phase 1's corpus test still passes on those models. The script now runs these conversions in UTC, as `--cascade` does, and writes `TZ: UTC` and the month to the corpus's `TOOLS`.

## Changes in this task beyond `book.rs`

- Stylesheets a file may use are the manifest items Calibre's manifest holds as CSS: media type lower-cased, or guessed from the extension when empty or `text/xml`, then one of `OEB_STYLES` (`epub::is_stylesheet`, the rule `oeb_view` already applied to its manifest). The test harness this replaces counted only items written exactly `text/css`. A `<style>`'s `type` must be one of `OEB_STYLES` as written, a `<link>`'s once lower-cased (`stylizer.py` `Stylizer.__init__`). Tested on hand-made books; no fixture or corpus book has such an item or type, and this was not run against Calibre. A linked stylesheet the OPF does not list is skipped, where Calibre adds and applies it (`DIVERGENCES.md`, "Calibre steps the book view leaves out").
- Stylesheets are decoded as Calibre decodes them (`OEBBook.decode`: byte-order mark, UTF-8, else the declared or default encoding), the same decoder as documents. The test harness this replaces skipped a stylesheet that was not UTF-8. `DIVERGENCES.md` "Encodings" now covers stylesheets. No fixture or corpus stylesheet is outside UTF-8.
- `book::Error` has the brief's variants plus `Package` (a package Calibre refuses, from `epub::Error::Package`). `Encoding { file }` is returned for an OPF whose bytes are not UTF-8 (an OPF that fails the zip's checksum is `Io`); documents and stylesheets are always decoded, so it names no other file.
- A file's stylesheets share one budget: `MAX_STYLE_RULES` and `MAX_STYLE_DECLS` hold for all of a file's `<style>`s, links and imports together, as `docs/books.md` states them, charged as each is read (`css::parse_sheet_in`). Before, each sheet had its own, and a file linking one large sheet many times held every copy until the cascade counted its rules. A page linking a 100,000-declaration sheet 30 times is now refused at the second link (24 MB peak, 40 ms, release).
- `Book::style_units` is added to the brief's `Book`, so the book-wide limit's headroom can be measured (table above).

No fixture or corpus comparison needed a `DIVERGENCES.md` section. The fix round after the re-review added one for the limits Calibre does not have: "Files too large to read".

## Final fix pass (whole-branch review)

The final review found paths where a small EPUB took gigabytes or minutes. Each fix has a test that failed first (`task-11-report.md`, "Final fix pass"):

- **Elements.** `MAX_DOM_NODES`, 500,000 nodes a file, stops the XML parse (`roxmltree`'s `nodes_limit`) and the HTML reading; `MAX_BOOK_DOM_NODES`, 2 million elements a book, is counted in `read_book` as each tree is built. The largest real file holds 530 elements and the largest book 5,067 (pg76). A file of 2 million empty elements (8 MB) is refused in 0.16 s at 119 MB, where it took 1.04 GB; a book of 2.25 million in 0.82 s at 595 MB. A 32 MB file is still refused only after 345 MB: the text is copied several times before it is parsed (decoding, the preprocessing passes), in proportion to the file's size, which the 256 MB entry limit bounds.
- **Rules kept per file.** The first-pass rules and `@font-face` rules each file keeps count against `MAX_BOOK_CASCADE_DECLS`. 100 files each linking a 45,000-rule sheet are refused at 690 MB (1.91 GB before).
- **Class names.** Each copy of the class names written into an element counts a unit for each 64 bytes against the file's limit: a 100 KB name on 8,000 elements is refused at 46 MB (934 MB before).
- **Repeated `<html>` tags read as HTML.** The attributes each one is compared with count against the file's HTML budget: 80,000 such tags stop in 15 ms (4.0 s before, growing with the square).
- **Folders that need quoting.** Links and `@import`s are resolved from the file's quoted href as Calibre's `Stylizer` does (`Manifest.Item.abshref`, `urlnormalize`) and looked up unquoted; a book whose folder is `My Book` lost all its CSS.
- **Links.** The manifest's stylesheets are a `HashSet`: 100,000 items and 100,000 links read in 1.1 s (a 40,000 pair took 1.09 s before, growing with the square).
- **Selector work per book.** `MAX_BOOK_SELECTOR_WORK`, 1 billion steps a book, both cascades of every file. The nine Gutenberg books take at most 21,828 steps (pg76). 40 files each just under the file limit are refused after 6.6 s (19.6 s before).
- **`<STYLE>`, `<LINK>`, a `style` of another namespace** are no longer read as stylesheets: Calibre reads `{xhtml}style`, `{svg}style` and `{xhtml}link` only.
- The book limit's check (`Error::BookCascade`) and the new selector-work check have tests in the default run.

Not done: the generated contents lower-case every title for each link of a book with no contents file (linear, at most 56 titles a link; no test could show a failure); a `color` attribute ending in `!important` stays as deferred in Task 9 (css_parser's behaviour unconfirmed). The text copies before parsing are bounded per file by the fix round below.

## Fix round after the re-review

The scoped re-review found neighbouring paths still open. Each fix has a test that failed first (`task-11-report.md`, "Fix round after re-review"). Times and peaks are from a release build of `read_book`:

- **XML files.** Every XML parse (OPF, nav, NCX, XML items) stops at `MAX_DOM_NODES` and the file is refused, named. An EPUB whose 8 MB XML item holds 2 million empty elements is refused in 0.07 s at 61 MB.
- **File size.** `MAX_CONTENT_FILE_BYTES`, 32 MiB: a spine document, the nav, the NCX or an XML item past it is refused before it is parsed, named. The largest file in the corpus and fixtures is 70 KB. A 32 MB XML item (2.63 GB before), a 32 MB NCX (633 MB before) and text files of 32 and 64 MB are refused in under 0.05 s at under 80 MB. At the limit, one paragraph of text peaks at 302 MB read as XML and 432 MB read as HTML; a file of attributes read as XML at 774 MB.
- **Elements and attributes per book.** Every document `oeb_view` reads for its links counts against `MAX_BOOK_DOM_NODES`, and attributes now count with elements there and in the held trees. 20 files of 8 MB of attributes are refused after 0.58 s at 202 MB (12 s and 1.49 GB before). A styled book just under the limit (2 million paragraphs) peaks at 1.11 GB after 2.2 s; the largest corpus book holds 7,718 elements and attributes (pg76).
- **Kept selectors.** Each 64 bytes of a kept rule's selector counts a unit against `MAX_BOOK_CASCADE_DECLS`: 100 files linking one sheet with a 4.5 MB selector are refused after 0.23 s at 381 MB (836 MB before).
- **Tests** for the HTML budget's `.min(MAX_DOM_NODES)` (fails with it removed) and for the book-limit arm of flatten's errors (`flatten_error`); the first-pass rules test checks that the message names no file.
- `docs/books.md`: a row for the 32 MB limit, the book row's 1.1 GB, attributes in the per-file row.

Not done, waiting for a ruling: a limit on the attributes of one start tag (an 80,000-attribute tag takes 30 s read as XML, 18 s as HTML; no scan before html5ever can count them faithfully without modelling its states), and a limit on the text of all a book's files together (30 files of 31 MiB of text read to 2.25 GB in 10 s).

## Fixture size

`fixtures/kfxgen` is 9.1 MB (`du -sh`), unchanged by this task, under its 10 MB cap.

## What phase 3 must do first

1. kfxgen's resolver (`converter.py:1030-1240`) on top of `Styles`: resolver dumps are not captured yet; add them to the same hook (`tools/ref/kfxgen_hook/book_dump.py`).
2. kfxgen's `data-kfxgen-*` marks on the tree (the `dom/` dumps hold them; `read_book`'s tree does not, and the tree comparison reads only names, ids and picture sizes).
3. kfxgen's spine reorder (`converter.py:8172-8221`); `Book::oeb.spine` is Calibre's order.
4. kfxgen's `Style.pseudo_classes(filter_css)` on top of `Styles::pseudo`: pop the `filter_css` properties from each pseudo-class, then drop the pseudo-classes left empty. `Styles::pseudo` gives them unfiltered.
5. The "Before phase 3" list from the phase 1 note (`2026-11-12-native-kfx-phase1-outcome.md`), not done in phase 2:
   1. Turn `BLOCK_UNPORTED` / `CHAPTER_UNPORTED` into allow-lists of the keys the port reads.
   2. Split `build_chapter_content` and the large `generate` modules; remove the `images` ↔ `chunks` mutual import.
   3. The small structure items: `build_fragment_259`'s parallel slices, `Node::Group` as a string, the `dedupe` / `dedupe_keys` duplicate, the `too_many_arguments` allowance in `inline.rs` (now line 674).

## Before merging

1. One release run of the ignored cost tests: `cargo test -p deckle-kfxgen --release -- --ignored --skip corpus` (11 tests, about 7 s of tests; nothing in the pre-push checks runs a release build, so the 5 s release bound holds only when this is run).

## Before phase 4 calls `read_book`

1. A fallback when `read_book` returns `Error::TooLarge`: a book past a limit is refused, not degraded. The per-file style limit is the one an honest book is most likely to reach (a single-file book with an inline style on each of more than about 12,000 to 25,000 elements).

Still before the final port ships (from phase 1's note): test #88 in the running desktop app.
