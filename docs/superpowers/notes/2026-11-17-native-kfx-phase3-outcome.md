# Native KFX, phase 3 (the converter): outcome

Date: 2026-11-17. Branch `feat/native-kfx-converter` (not pushed). Plan:
`docs/superpowers/plans/2026-11-14-native-kfx-phase3-converter.md`. Design:
`docs/superpowers/specs/2026-11-10-native-kfx-design.md`. The port follows kfxgen at commit
`ad38ec8` (5.24.0 plus #753 to #778).

## Result

- `deckle_kfxgen::convert::convert(path, opts)` reads an EPUB (`book::read_book`, phase 2) and
  runs every ported stage of kfxgen's `convert_oeb_to_kfx`: the metadata (with the landmarks
  and page list, which kfxgen works out last, in its call to the generator; neither can fail or
  log, so the order does not show), the cover and pictures, the chapters (with kfxgen's second
  pass when dropping the contents listings leaves the book no contents page), the title page and
  contents page, the split by writing mode, the fonts and the footnotes. It returns `Converted`: the
  `Model` (`generate_full_book`'s arguments, the input the phase 1 generator takes) and the
  warnings kfxgen logs on the way.
- It does not write a KFX: the caller passes the model to `generate::generate`. It does not run
  Calibre, Kindle Previewer or Python.
- **Nothing user-facing calls it yet.** The app still converts EPUB to KFX through Kindle
  Previewer. Phase 4 wires `convert` and `generate` into the app.
- **The 55 source fixtures:** the model equals kfxgen's for every one (the whole argument tree,
  by `model_diff`: key order, `None` against absent, int against float, floats bit for bit).
  Pictures kfxgen re-encodes are compared by format, size and JPEG components, not bytes
  (`DIVERGENCES.md`, "The book's pictures and cover"); their order is not compared, since
  Calibre lists a book's pictures in another order from run to run. The generator makes
  kfxgen's `expected.kfx` byte for byte from 50 of the models as they are, and from
  `pictures_more`'s with kfxgen's three re-encoded pictures put in; it refuses the other four
  by name, as before (table below).
- **The nine Gutenberg books:** the model equals kfxgen's recorded model for all nine, with no
  difference, and the KFX is byte-identical for all nine (pg84 and pg98 with kfxgen's
  re-encoded cover put in: each cover is over 1,920 px on its long side, 2,726 and 2,366 px, so
  both scale it, and the port's encoder makes other bytes).
- Warnings: kfxgen's log-only warnings are collected into `Converted::warnings` (list below).
- `docs/books.md`'s size limits table lists the phase 3 limits a book's owner could meet (six
  "EPUB to KFX" rows), as phase 2 listed its own before anything user-facing called them.

## Timings

Release build, M-series Mac, one run each, not a quiet machine: the one-minute load average was
22 when the run of all ignored tests started and when it ended (other processes). Command:
`DECKLE_KFXGEN_CORPUS=gutenberg/models DECKLE_KFXGEN_CORPUS_EPUBS=<folder of the EPUBs> cargo
test --release -p deckle-kfxgen -- --ignored --nocapture --test-threads=1`.

| Book | Pictures | Chapters | `convert` (ms) | `generate` (ms) | Together (ms) |
|---|---|---|---|---|---|
| pg11 | 1 | 17 | 87 | 14 | 101 |
| pg23 | 1 | 19 | 52 | 9 | 61 |
| pg43 | 1 | 14 | 47 | 8 | 55 |
| pg55 | 1 | 30 | 116 | 15 | 131 |
| pg74 | 167 | 41 | 406 | 51 | 457 |
| pg76 | 178 | 49 | 523 | 57 | 580 |
| pg84 | 1 | 32 | 143 | 33 | 176 |
| pg86 | 227 | 52 | 547 | 67 | 614 |
| pg98 | 33 | 68 | 420 | 105 | 525 |
| all nine | | | | | 2,701 |

`convert` includes `read_book` (12 to 71 ms of it, `corpus_cascade` in the same run). The
generator time of pg84 and pg98 covers two runs: the test generates again with kfxgen's cover
put in. The test's limits, which it checks in a release build: each book with no picture but
its cover under 2 s (the slowest, pg84, took 0.18 s), all nine under 39 s (2.7 s). A run at load 5,
before the warnings were added, gave 2.9 s for all nine.

The ignored cost tests in the same run (all 16 passed):

| Test | Time |
|---|---|
| `chapters_cost_in_a_release_build`: one file of 20,000 paragraphs | loop 1.17 s (929 MB allocated in all, not peak), assembly 0.19 ms |
| `a_many_attribute_tag_read_as_html_is_a_known_gap` (20,000 attributes) | 0.57 s |
| `a_class_rule_matching_everything_many_times_stops` | 1.26 s |
| `a_rule_matching_everything_many_times_stops` | 0.56 s |
| `a_hundred_thousand_large_style_attributes_stop` | 0.13 s |
| `fifty_thousand_rules_on_a_hundred_thousand_elements` | 0.06 s |
| `a_book_of_many_styled_files_cost` / `_is_refused` | 1.32 s / 1.28 s |
| `fifty_thousand_distinct_elements_cost` | 0.21 s |
| `fifty_thousand_realistic_selectors_finish` | 0.10 s |
| `worst_case_selectors_are_refused` | 0.86 s |
| `selector_work_rates` | 8.8 ns a step |
| the three corpus tests and `styles_just_under_the_limit_peak_memory` | pass (36.5 s for all 16 tests) |

## Refusals the fixtures and corpus reach

`convert` refuses by name (`convert::Error::Unsupported`) and the generator refuses a model by
name (`GenerateError::Unsupported`). Counted over the 55 source fixtures and the nine corpus
books, each book counted at the first refusal it meets (the generator stops there). No source
or corpus book reaches a `convert::Error::Unsupported`. The refusals are tested on books made in
the tests.

| Refusal | Kind | Fixtures | Corpus |
|---|---|---|---|
| "preformatted text" | generator | 3 (`block_edges`, `css_quirks`, `text_edges`) | 0 |
| "block styles" (in table cells) | generator | 1 (`tables_more`) | 0 |
| the other 40 `GenerateError::Unsupported` names (42 in all, counted in the source: boxes nested past 32 levels, floated pictures, a rule in a box, math, a run in two languages, pictures in a table cell, …) | generator | 0 | 0 |
| "math" | convert | 0 | 0 |
| "SVG pictures" | convert | 0 | 0 |
| "pictures embedded as data: URIs" | convert | 0 | 0 |
| "list numbers too large" | convert | 0 | 0 |
| "tables holding an epub:switch" | convert | 0 | 0 |
| "WOFF2 fonts" | convert | 0 | 0 |

`convert::Error::TooLarge` limits (none reached by a fixture or a corpus book):

| Limit | Value | Smallest refused input |
|---|---|---|
| A file's text budget: the text and styles the converter copies or reads in one spine file | 500,000 units of 64 bytes (32,000,000 bytes), new for each file on each pass | 2,000 `<col span="1000">` in one table; `<table>` of 12,500 rows of ten one-word cells; 32 bordered `<div>`s nested round a paragraph of 999,806 characters |
| The book's text budget: the files' budgets of one pass (and of the footnote scan) together | 2,000,000 units (128,000,000 bytes, `MAX_BOOK_TEXT_BUDGET`); the corpus charges at most 5,409 (pg76) | 20 files each a link to an id of 32,000 letters round 240 words in `<i>` (5 convert); refused at a 112 MB peak, and 100 such files at 139 MB (2.7 GB before) |
| Blocks a pass makes | 500,000 (`MAX_BOOK_BLOCKS`), checked after each file; the corpus makes at most 3,450 (pg98) | 8 files of 125,000 `<p>x</p>`: refused after the fifth, 34 s and 1.7 GB peak (55 s and 4.05 GB converted before) |
| Repeated titles and keys: the contents page's headings; in assembly, an entry's key copied onto the entries on its block and a dropped listing's keys onto each page that keeps it; a chapter's title copied onto each part the writing-mode split makes | 33,554,432 bytes (`MAX_CONTENT_FILE_BYTES`) each, the three steps counting apart | an entry titled `"!" * 10_000_000 + "A"` and a "Contents" page of four heading links "A"; 336 entries snapping onto a block named by an id of 100,000 letters; 111 pages keeping a listing of 5,000 ids; 36 files alternately vertical under a title of 1,000,000 letters |
| One picture | 100 megapixels (`MAX_IMAGE_PIXELS`) | a PNG whose header says 10,001 × 10,000 |
| One picture's decode memory | `image`'s 512 MiB, less 8 bytes a pixel for a JPEG (about 48.8 megapixels for RGB) | a baseline RGB JPEG of 7,000 × 7,000 (worked out, not run) |
| A book's pictures together | 10 gigapixels (`MAX_BOOK_IMAGE_PIXELS`) | 101 pictures whose headers say 10,000 × 10,000 |
| A book's fonts together | 256 MiB | six TTFs of 50 MiB |
| Nesting inside a paragraph, and below the body | 256 levels (`MAX_DEPTH`) | only a tree built by hand: `read_book` refuses a file nested that deep first |
| Text the converter keeps from the nav and NCX (`CalibreOpf`) | 64 MiB (`Error::Book(TooLarge)`) | a nav whose links are nested so each is read once per level; a real nav is under the 32 MB file limit |

## Where the built-in converter refuses a book kfxgen converts

Each row is one follow-up candidate. "Refused" means `convert` returns an error naming the
feature or the limit; kfxgen (through Calibre) makes the book. Details are in
`crates/deckle-kfxgen/DIVERGENCES.md` under the section named.

| What is refused | Smallest input | Why |
|---|---|---|
| List numbers of 2^63 or more (`<ol start>`, `<li value>`, a native list's numbers) and typed markers past 2^127 ("list numbers too large") | `<ol start="9223372036854775808"><li>x</li></ol>`, or `start="9223372036854775807"` with two items (ran) | The model holds 64-bit ints; Python's are unbounded. Lists. |
| Boxes nested deep round very long text | 32 bordered `<div>`s round one paragraph of 999,806 characters (64 levels: 499,838; 128 levels: 249,854) | Each level copies its blocks' text; the copies are charged to the file's text budget. Boxes. |
| Containers that draw no box, looked at past the budget | a file needing more than 500,000 looks at the blocks of containers that draw no box round them (with the file's other charges) | Each look costs a unit; a running count would remove this refusal (deferred minor, Task 15). Boxes. |
| Deep lists whose items hold many elements and no text | lists nested deep, items of many empty elements (no measured smallest) | Each node read to tell typed markers is charged. Lists. |
| Tables holding an `<epub:switch>` ("tables holding an epub:switch"), native tables on | `<table><tr><td><epub:switch><epub:case required-namespace="…">…</epub:case><epub:default>x</epub:default></epub:switch></td></tr></table>` | kfxgen deep-copies the table without the switch's cases and asks the resolver about elements Calibre never styled; what it decides is not known. Tables. |
| Tables of about 125,000 cells or more | `<table><tbody>` of 12,500 rows of ten `<td>cell</td>` (166,667 cells refused before any is read) | Each cell is charged three units up front, plus what reading it costs. kfxgen's own work grows with rows × cells. Tables. |
| Split rows of deeply nested tables with very large cells | tables nested 62 deep in cells, the innermost cell holding 10,000 paragraphs (5,000 convert) | Each paragraph a row holding a table writes costs a unit, again for each such row round it. Tables. |
| `<col span>` adding up to about a million column widths | 2,000 `<col span="1000">` | Each repeated width is charged before it is made. Tables. |
| Pictures above 100 megapixels | a PNG whose header says 10,001 × 10,000 | Bounds decode memory; calibre lets Qt allocate up to 1,024 MB (about 268 megapixels). Pictures. |
| JPEGs above about 48.8 megapixels (59.7 for grey) | a baseline RGB JPEG of 7,000 × 7,000 (worked out, not run) | zune-jpeg keeps coefficient planes outside `image`'s allocation limit; 8 bytes a pixel are taken off it. Pictures. |
| More than 10 gigapixels of pictures in a book | 101 pictures of 10,000 × 10,000 | Bounds decode time (100 such pictures took 29 s in a release build). Pictures. |
| Pictures embedded as `data:` URIs ("pictures embedded as data: URIs") in an `<img>` | `<img src="data:image/png;base64,…"/>` (ran) | calibre's `DataURL` step, which turns them into files before kfxgen, is not ported; it needs a recorded fixture. Pictures in text. |
| `data:` sources calibre does not turn into files: an SVG `<image>`'s href, a CSS background's `url()`, an `<img>` source calibre cannot decode (same refusal) | `<svg><image href="data:image/png;base64,…"/></svg>` | kfxgen writes the URI into the picture token and the generator finds no picture by that name, so the picture is lost but the book is made; the port refuses before copying the URI. Pictures in text. |
| Contents nested more than 32 levels deep (generator, "contents nested more than 32 levels deep") | contents entries nested 33 levels deep (kfxgen writes up to 162) | The generator writes contents recursively; a debug build overflowed a 2 MiB thread at 80 levels. Contents nested more than 32 levels deep. |
| Boxes nested more than 32 levels deep (generator, "boxes nested more than 32 levels deep") | 33 bordered `<div>`s nested (kfxgen writes up to 245) | The generator reads and writes boxes recursively; a debug build overflowed a 2 MiB thread at about 60 levels. Boxes nested more than 32 levels deep. |
| SVG pictures: an SVG file in the manifest, or an inline `<svg>` drawn only with shapes ("SVG pictures") | `<svg><rect width="1" height="1"/></svg>` in a page (ran) | kfxgen draws them with Qt; no SVG renderer is ported (follow-up jloutsch/deckle#96, plan decision 2). Pictures. |
| MathML ("math") | `<p>a <math><mi>x</mi></math></p>`, shown or not (ran) | Plan decision 2; the generator does not write equations either. The block walk. |
| WOFF2 fonts ("WOFF2 fonts"), when one is tried | `@font-face { src: url(f.woff2) }` used by the text | fontTools rebuilds WOFF2 glyph tables with its own compiler; not ported (follow-up jloutsch/deckle#97). Fonts. |
| More than 256 MiB of fonts in a book | six TTFs of 50 MiB | Bounds memory from a small EPUB of deflated fonts. Fonts. |
| Note and link keys under a very long path | a file at a path of 2,000 characters with 40,000 note markers linking to notes that link back (100 characters converts) | Each key holds the path; the copies are charged to the file's budget. Note and link keys. |
| More than 32 MB of titles and keys repeated on a contents page | an entry titled `"!" * 10_000_000 + "A"` and four heading links "A" on a "Contents" page | kfxgen shares the strings; the port copies them, counted first. Headings a contents page repeats. |
| More than 32 MB of keys repeated by assembly, or of titles by the writing-mode split | 336 entries snapping onto the block of an entry whose id has 100,000 letters; 111 pages keeping a listing of 5,000 ids; 36 alternately vertical files under a title of 1,000,000 letters | kfxgen shares the key and the title (it builds the listing keys too); the port copies them, counted first. Titles and keys chapters repeat. |
| More than 128,000,000 bytes copied or read by a pass over all the book's files | 20 files each a link to an id of 32,000 letters round 240 italic words | Each file is under its own budget; the copies add up over the files. The block walk. |
| More than 500,000 blocks in a pass | 8 files of 125,000 `<p>x</p>` | Bounds memory (about 4.7 KB a block) and time (about 55 µs a block). The block walk. |
| Long text in a run's flags over many runs | a 1 MiB link target round 2,000 words | Flags are copied into each element and segment, charged. The inline walk. |
| Tate-chu-yoko nested so their text is read over and over | nested tate-chu-yoko (no measured smallest) | Each is read once more for every one round it, charged. The inline walk. |
| Many pictures drawn by one `<svg>` with a long id | `<svg id="(1 MiB)">` with 200 `<image href="a"/>` | The id goes on each picture block twice, charged. The block walk. |
| Picture sources and alt texts over about 32 MB in a file | many `<img>` with long `src`/`alt` (no measured smallest) | Each token's copied text is charged. Pictures in text. |
| A very long inherited style value over many elements | a 9.6 MB inherited value on many elements (no measured smallest) | Bytes the resolver copies per element are charged (Task 8 ruling). Not in `DIVERGENCES.md` beyond the budget's description in "The block walk". |

The phase 2 limits on reading the book (`docs/books.md`'s EPUB to KFX rows, and the phase 2
note's "Before phase 4") apply before all of these.

## Before phase 4 calls `convert`

From this phase's ledger (`GATE` lines):

1. **Resolver cost.** `Resolver::resolve` costs about 33 µs per element in a release build (100,000
   visible `<a id>` elements: 3.34 s, linear); a file at the 500,000-element limit would take
   about 17 s. At book scale: 4 files of 240,000 `<a id>` that show nothing, then a page of
   text (960,000 elements), took 33.1 s for one pass (799 MB peak; 40 files of 24,000 the same,
   review, release); the book's 2 million elements and attributes would take 35 to 70 s a pass,
   twice that when the second pass runs. The time of a very large book is not bounded by any
   limit: phase 4 needs a way to cancel a conversion. Each paragraph or cell is fully resolved
   4 to 5 times from different memos (81% of a large table's time). Candidate fix: a per-node
   cache of resolve's values (estimated 3 to 4 times faster). Open.
2. **`Resolver::charge` rounds each charge down to whole 64-byte units,** so many small charges
   are free: charges of 64 bytes or more are bounded by the file's budget and, since the final
   fix pass, by the book's (`MAX_BOOK_TEXT_BUDGET`, per pass); charges under 64 bytes only by
   the book's node limit (761 MB measured for note keys). Open; decide with the memory limits
   below.
3. **The XML parser's stack.** `roxmltree` recurses once per element level; in a release build
   253 levels took 160 to 176 KiB of stack (a 2 MiB thread has about 11.6 times that); a debug
   build overflows a 2 MiB thread at 135 to 253 levels. Open: which thread the app runs
   `read_book` and `convert` on, and its stack size.
4. **`fit_image` was read at Calibre v8.12, not 9.18** (the Calibre the fixtures were made with).
   Compare it at 9.18 before relying on scaled picture sizes. Open.
5. **fontTools version.** A WOFF is unpacked as fontTools 4.60.1 writes it; which fontTools
   Calibre 9.18 bundles was not read. Open.
6. **Unicode versions.** Font families are lower-cased by Rust's Unicode tables (17.0) and
   Python's are 16.0; 28 code points differ (`DIVERGENCES.md`, "The book's fonts", and "Unicode
   version of Python's character classes"). Open; decide whether it matters.
7. **Peak memory with vertical marks** at `MAX_BOOK_DOM_NODES` (Task 5): the marks are bounded per
   element but not charged; re-measure. Open.
8. **Phase 2 `string_value` calls in `epub/package.rs`** are not budgeted (nested elements could
   store depth × size), per the Task 6 re-review. Unverified. Open.
9. **Budget margin** (Task 8). Measured with a temporary print (removed): the largest charge
   of any file's text budget is 1,014 units of 500,000 in the corpus (pg76's
   `76-h-2.htm.html`, 0.2%) and 298 in the fixtures (`css_quirks`'s `selectors.xhtml`).
   The budget counts whole 64-byte units per charge, so it understates the work; the cost is
   gate 1. No real book here comes near the limit; a 20,000-paragraph file is still estimated,
   not measured, against it.
10. **JPEG decode memory outside this crate** (Task 18): not checked in this phase; the owner
    has the note. Open.
11. **The model is built by copying every chapter** (`Chapter::to_py` while the chapters are
    alive): 8 files of 20,000 paragraphs peak at 854 MB through `convert` against 522 MB for the
    chapters alone (review, ran). A consuming conversion would move the values, about halving
    the peak of a book near `MAX_BOOK_BLOCKS` (about 2.4 GB and 27 s a pass at that limit,
    extrapolated from 4.7 KB and 55 µs a block). Joins the phase 2 item "the limits add up".
    Open.
12. **Warnings have no size cap.** A contents entry's title is copied, escaped, into the
    "dropped: not in spine" warning: a 20 MB NCX gave 70 MB of warnings (review, ran), up to
    about 224 MB at `CalibreOpf`'s 64 MiB (worked out). Cutting the title in the warning would
    bound it. Open.
13. **Warnings are lost with an error.** `convert` returns only the error, so a book whose only
    shown file kfxgen raises on gives `NoContent` ("the book shows nothing") without the "Spine
    item N parse failed" line kfxgen logs first. Phase 4 should return the warnings with the
    error or put the reason in it. Open.
14. Closed in this phase: the SVG-root spine page (Calibre's reader drops it, as `read_book`
    does; Task 21), the 929 MB for a 20,000-paragraph file (total allocation, not peak: 137 MB
    peak; Task 21), the 20,000-paragraph walk time (1.04 s in a release build in Task 21, 1.17 s at load 22
    here); a page's file name copied once for each table written as rows (fix round 1 keeps
    the file's index); and, in phase 2's `read_package`, the item's href copied once for each
    spine entry naming it (fix round 1 drops the unread `Package::spine` field: 5,000 entries
    of a 20,000-character name allocated 102 MB in `read_package` and now take under 20 MB
    through `oeb_view`); and, in the final fix pass, what the final review found bounded per
    item but multiplied across the book: the files' text budgets added up (now
    `MAX_BOOK_TEXT_BUDGET`), keys and titles copied per entry, page or part (now counted against
    `MAX_CONTENT_FILE_BYTES`), and the book's blocks (now `MAX_BOOK_BLOCKS`), each refused
    before the memory is taken (the block limit after the file that passes it); `walk_body`'s
    unreachable SVG-page branch deleted.

The phase 2 note's seven items ("Before phase 4 calls `read_book`") still stand; phase 3 changed
none of them and did not re-measure them:

1. A fallback when `read_book` returns `Error::TooLarge` (a book past a limit is refused, not
   degraded); now also for `convert`'s own `TooLarge` and `Unsupported`.
2. The many-attribute tag read as HTML (known gap).
3. A stylesheet parsed again for every file that links it, with no book-wide count of sheet bytes.
4. Item 2's worst case is larger than its measured figure (extrapolated about 10 hours).
5. The limits add up: decide the peak memory the app accepts and lower the limits to it.
6. The OPF has no size check of its own.
7. The attribute scan can refuse a file Calibre reads.

Still before the final port ships (from phase 1's note, carried by phase 2's): test #88 in the
running desktop app.

## Warnings

`Converted::warnings` holds, in the order kfxgen logs them, with kfxgen's words (a Python
`repr` written as Rust's `{:?}`):

- A spine item Calibre leaves out of its spine as no document (a stylesheet or picture listed
  in the spine). Calibre's reader logs this; the words are the port's, not Calibre's.
- The pictures kfxgen leaves out or keeps unconverted (Task 18).
- "Spine item N parse failed (href): …": a file kfxgen raises on. The text after the colon names
  the Python exception and what raises it, in the port's words; kfxgen prints the exception's own
  message.
- Words drawn in SVG pictures, written after the picture.
- A contents entry naming no shown file ("dropped: not in spine"), and one naming an id no block
  has ("snapping to block N").
- Tables written as rows of text, counted among those still in a chapter.
- The read-aloud narration and audio and video clips left out.

Only the last pass's warnings are kept: kfxgen logs the first pass's too, then drops what that
pass made. Not produced: kfxgen's `log.info` lines; the font table's skipped-face messages (log
only in kfxgen, and not in the brief's list); a narration named only in a second OPF of the
container (kfxgen reads every OPF the container names, the port the first); a spine `idref`
naming no manifest item (the port's reader drops it without a warning).

## Deferred minors

From the ledger, none of them a difference a fixture or the corpus shows:

- Task 1: `boxes.rs` kid `list_item {}` of a numbered box item not counted from the box item as
  Python's `or` does; `number_turns`/`number_tcy` serial lookup is linear per serial; `up()`
  skips the 29-digit carry case.
- Task 2: `convert_dump.py` docstring clause order for `inline/`; no hook test that a differing
  later resolver fails the dump.
- Task 4: `dom/structure.rs` `at_start` does `ancestors.contains` per node; comments before or
  after the root element are not kept; `Dom::index_in_parent` is reached only by tests, and
  `Dom::ancestors` and `Dom::qualified` had no caller but tests then.
- Task 7: `compile.rs` keeps its own `py_space` beside `dom::py_space`.
- Task 6: generated-NCX nesting is unit-tested only; `CalibreOpf::new` reads the nav only when
  `nav_path` is set; the OPF 3 empty-date case is unrecorded.
- Task 8: `TextBudget::new` duplicates `Default`; `TextBudget::charged` is public for a test;
  `Styles`/`Resolver` are not `Send`.
- Task 9: `Walk::link_work()` public for a test; `begin_turned` rescans a subtree holding a
  picture, ruby or math at each nested marked element (depth × nodes, as kfxgen); a table-cell
  expectation checked by reading the Python, not running it.
- Task 11 and 12: about 8 resolves per leaf `<p>` and 3 per container end; resolve once and pass
  down (joins gate 1); nested front-matter marking and the `would_box` contents scan cost depth ×
  blocks.
- Task 12: `deckle-limits/src/lib.rs` "the 49 cascade fixture files at most 1,049" not re-checked
  after the `fake_margins` re-record.
- Task 13: the `li`-without-parent branch in `lists.rs` is unreachable but kept.
- Task 14: `rules.rs` `float_or_none` `Bool` arm unreachable; `BorderSide.kind`/`.colour` could
  be private; `magnitude()`'s `unwrap_or(0.0)` cannot trigger; `BORDER_SIDES` duplicates
  `resolver.rs` `SIDES`; `emit_rule` resolves the element a third time, as the pin does.
- Task 15: a running count would answer `make_box`'s two `any()` tests in O(1) and remove the
  "500,000 looks" refusal; three closures repeat `parse_css_length(text_of(css.get(key)))`; the
  box `bidi.set_here` value has no dump or Python confirmation; `Block::remove`'s unreachable
  `Field::Blocks` arm.
- Task 18: the cover-sharing test proves equal bytes, not one encoding; the cover is cloned (up
  to 256 MB) instead of moved; the warning order differs from kfxgen's phases. Re-review: the
  GIF unit test uses only a 2-colour table; the JPEG test does not pin the multiplier between 3
  and 7; worst-case GIF decode memory is about 637 MB (worked out, not run).
- Task 19 (M5): unreached code in `font_table.rs` (the bare-`src` branch of `src_urls`, the
  `MissingFile` arm of `Fonts::bytes`, an `unwrap_or_default`).
- Task 21 re-review: R1, the book's-last-block tail write has no cost test; R2, subtracting
  note ids from the contents targets is pinned by no test; R3, a wrong `DIVERGENCES.md`
  sentence about the second pass's budget (corrected in this task's fix round 1).
- Task 22 (M7): `split_by_writing_mode` splits only blocks held as `Field::Blocks`.
- Task 23 review: M1, M2 and M3 (gates 11 to 13); M6, the picture comparison reads no pixels,
  so a picture both re-encode to the right size and format with wrong pixels passes (a mean
  per-channel difference after decoding both would catch it).

## Code no test reaches (deletion candidates)

- `SpineItem::from_py`, `Block::from_py`, `Chapter::from_py`: only tests call them (they feed
  kfxgen's recorded values to assembly and the title-page step).
- `drop_keys`' box arms and `prepend_keys`' table arm (Task 21), `count_text_blocks`' box-in-cell
  arm and `listing_at`'s negative arm (Task 22).
- The unreached arms in `font_table.rs` (Task 19, above).

## What was not checked

- Books beyond the 55 fixtures and nine corpus books. The corpus is English prose; pg74, pg76
  and pg86 have pictures, boxes and native tables, pg98 pictures and boxes. No corpus model
  has an embedded font, a vertical page, a footnote key, a page list or a table written as
  rows; those paths are checked on fixtures only.
- The warnings against kfxgen's log: no recorded log; each was compared with the pinned Python
  by reading.
- Calibre's own wording for the spine items its reader leaves out.
- Timings on a quiet machine.
- The SVG-words check in a crafted file: 240 nested `<svg>`s round a `<text>` of 100,000
  `<tspan>`s took 1.48 s for the walk and the check together (review, release; depth × nodes,
  as kfxgen).
- Peak memory of `convert` on the corpus.
