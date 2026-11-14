# Native KFX, phase 2 (EPUB reader and styles): outcome

Date: 2026-11-13. Branch `feat/native-kfx-epub-cascade` (22 commits on `main` `0ca92a2`, not pushed). Plan: `docs/superpowers/plans/2026-11-13-native-kfx-phase2-epub-cascade.md`. Design: `docs/superpowers/specs/2026-11-10-native-kfx-design.md` (its Cascade oracle row names the corrected oracle: kfxgen's own Stylizer, dumped inside the conversion).

## Result

- `deckle_kfxgen::book::read_book(path)` reads an EPUB and gives what kfxgen's `convert_oeb_to_kfx` receives from Calibre 9.18.0: the package files, Calibre's book view (with the contents Calibre generates when the book has none), each spine file's tree after the flattener, each element's declared and pseudo-class styles as kfxgen's `Stylizer` reads them, the computed values (`File::styles`), the first-pass rules, and the book's `@font-face` rules. `manifest_bytes(path, href)` reads any other file of the book through the same limits.
- **13 cascade fixtures** (`fixtures/kfxgen/cascade/`, 49 spine files) match through `read_book` at four levels: book view (`oeb.json`), element tree and first-pass rules (`dom/`, `rules/`: 306 book rules, 3 `@font-face` rules), declared styles and classes (every element, `styles/`), computed values (34 properties of every element, over 30,000 values). One allow-list entry is used, the one Task 3 made: `css_quirks`'s `page_css1` and `page_css2` ("One page_styles.css until the CSS is read").
- **The 9-book Gutenberg corpus** (`gutenberg/models/<book>/cascade`, outside git) matches the same four comparisons with **no mismatch and no allow-list entry**: 294 spine files, 13,278 book rules, 22,519 elements, 765,646 computed values. The corpus has no `@font-face` rule and no book without a contents file, so those two paths are checked on fixtures only.
- A changed dump fails the comparison: with one `styles/` table entry's `font-size` and one `rules/` selector altered in a copy of pg11's dump, the corpus test failed with 48 mismatches (47 elements sharing that entry, one rule).
- Tests: `deckle-kfxgen` lib 14 + it 238 passed, 13 ignored (the 12 before, plus `corpus_cascade::corpus_matches_calibre`). The six fixture-wide comparison tests of Tasks 3, 8, 9 and 10 (and the fixture loop of `img_size_attributes_fold_into_css`) became check functions that `book::read_book_matches_every_dump` and the corpus test run on `read_book`; the per-step tests on hand-made pages stay, and the test harness reads stylesheets through the library (`book::file_sheets`, `book::stylesheets`).
- The test harness refuses a dump whose `TOOLS` names another Calibre than 9.18.0 or no `TZ: UTC` (`tests/it/support.rs` `check_tools`, tested by `book::dump_from_another_calibre_version_is_refused`).

## Timings

`read_book` per corpus book, release build, M-series Mac, one run (`DECKLE_KFXGEN_CORPUS=gutenberg/models cargo test --release -p deckle-kfxgen --test it corpus_cascade -- --ignored --nocapture`):

| Book | Spine files | Pictures | `read_book` (ms) | Style units used |
|---|---|---|---|---|
| pg11 | 13 | 1 | 44 | 4,597 |
| pg23 | 16 | 1 | 40 | 5,610 |
| pg43 | 11 | 1 | 23 | 3,498 |
| pg55 | 26 | 1 | 55 | 7,773 |
| pg74 | 39 | 167 | 107 | 17,500 |
| pg76 | 49 | 178 | 141 | 21,323 |
| pg84 | 30 | 1 | 64 | 9,184 |
| pg86 | 48 | 227 | 153 | 16,473 |
| pg98 | 62 | 33 | 143 | 18,546 |

Every text-only book (no picture but its cover) is under the plan's 1 s; the test fails past it in a release build. "Style units" is what the book used of `MAX_BOOK_CASCADE_DECLS` (`Book::style_units`); the most a fixture book used is 4,196. `read_book` parses each spine file twice: once in `oeb_view` for its links, once for the tree. It is not needed for these times; phase 3 can share the parse if the whole pipeline needs it.

Dumping the corpus (`tools/ref/kfxgen_models.sh --corpus`, which also makes phase 1's models) took 82 s and 186 MB; phase 1's corpus test still passes on those models. The script now runs these conversions in UTC, as `--cascade` does, and writes `TZ: UTC` and the month to the corpus's `TOOLS`.

## Changes in this task beyond `book.rs`

- Stylesheets a file may use are the manifest items Calibre's manifest holds as CSS: media type lower-cased, or guessed from the extension when empty or `text/xml`, then one of `OEB_STYLES` (`epub::is_stylesheet`, the rule `oeb_view` already applied to its manifest). The test harness this replaces counted only items written exactly `text/css`. Tested on a hand-made book; no fixture or corpus book has such an item, and this was not run against Calibre.
- Stylesheets are decoded as Calibre decodes them (`OEBBook.decode`: byte-order mark, UTF-8, else the declared or default encoding), the same decoder as documents. The test harness this replaces skipped a stylesheet that was not UTF-8. `DIVERGENCES.md` "Encodings" now covers stylesheets. No fixture or corpus stylesheet is outside UTF-8.
- `book::Error` has the brief's variants plus `Package` (a package Calibre refuses, from `epub::Error::Package`). `Encoding { file }` is returned for an OPF that is not UTF-8; documents and stylesheets are always decoded, so it names no other file.
- `Book::style_units` is added to the brief's `Book`, so the book-wide limit's headroom can be measured (table above).

No `DIVERGENCES.md` section is added: no fixture or corpus comparison needed one.

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

Still before the final port ships (from phase 1's note): test #88 in the running desktop app.
