# kfxgen fixtures

Output of [kfxgen](https://github.com/jloutsch/kfxgen) (GPL-3), the reference for the native
converter (`docs/superpowers/specs/2026-11-10-native-kfx-design.md`, Oracles). The generator
code is kfxgen commit `4b81651` (5.24.0 plus #753). `models/` were dumped with `8f55440` on
kfxgen's `feat/model-dump`, which is `4b81651` plus the dump hook only.

## golden/

Copied unchanged from kfxgen's `tests/fixtures/golden/expected/`. kfxgen builds them from
synthetic EPUBs (`tests/fixtures/golden/inputs.py`) without Calibre, through
`tests/fixtures/oeb_shim.py`, so they carry no per-element CSS.

| File | sha256 (first 16) |
|---|---|
| `minimal.kfx` | `73fbd3c30597b43e` |
| `with_cover.kfx` | `2694e310ff91d9b7` |
| `body_images.kfx` | `7353137416ae4ad0` |
| `table_cells.kfx` | `2b646ac746a8baf9` |

Regenerate: `git -C <kfxgen> show 4b81651:tests/fixtures/golden/expected/<name>.kfx > golden/<name>.kfx`.

**Found (phase 1, task 1):** `read_kfx` then `write_kfx` with `WriteOptions::from_doc`
reproduces all four byte for byte. kfxgen's container framing is the same as kfxlib's, so the
native generator can write through `deckle_kfx::container::write_kfx` with no changes to it.

## models/

One folder per fixture: `model.json` and `resources/` (the arguments of kfxgen's
`generate_full_book`, written by its `KFXGEN_DUMP_MODEL` hook) and `expected.kfx` (what kfxgen's
generator makes from them). `KFXGEN_COMMIT` names the kfxgen commit that dumped them.
Rebuild all of them with `tools/ref/kfxgen_models.sh`; two runs give the same bytes.

**Shim path** (kfxgen's Calibre-free `EpubAsOeb`, no per-element CSS): the 15 golden inputs
from kfxgen's `tests/fixtures/golden/inputs.py` under their own names (`minimal`,
`body_images`, `captioned_images`, `sub_super_marks`, `table_cells`, `table_layout`,
`table_paragraph_cells`, `contents_illustration`, `with_cover`, `multi_chapter`, `linked_toc`,
`publisher_structure`, `long_chapter`, `marker_offsets`, `unused_manifest_image`), plus:

| Fixture | What it holds |
|---|---|
| `unicode_text` | `é` composed and decomposed, `漢字`, `😀`, `𝔘`, then an italic span after them |
| `empty_chapter` | chapter 2 of 3 with no body text |
| `image_only_chapter` | a page of a picture alone; a heading and a picture, printed with no heading of kfxgen's; a picture above the book's own heading |

**Argument path** (`kfxgen_models.py args`): books written as `generate_full_book`'s chapters,
for run flags the shim can't make (it applies no CSS) on blocks with no CSS (a Calibre-path
block always has some), and for picture arguments no EPUB gives (sizes, floats, unresolved
hrefs, unreadable JPEGs). The books are `ARG_BOOKS` in that script, their other arguments
`ARG_BOOK_EXTRAS`.

| Fixture | What it holds |
|---|---|
| `inline_runs` | a heading's italic and not-bold words; `url:`, unfound, other-capitals and whole-file link targets; a link over a bold word beside more of it; super, sub, text-top, text-bottom, baseline, underline, line-through; ids inside, at and past a paragraph's end |
| `nested_contents` | a rebuilt contents page at 0.75 with lines at depths 1 and 2 and targets past either end; a heading-only chapter whose ids land in kfxgen's heading; a title over two blocks; a 0.75 chapter with runs and links |
| `heading_runs` | the book's heading with words after its title: its bold and italic runs and ids in kfxgen's heading, links to them |
| `image_refs` | pictures found by exact, query, decoded, basename and absolute hrefs, and not found; every size hint; floats; display sizes; a basename shared by two pictures; ids of a picture block, and of a picture the book doesn't hold; JPEGs whose size kfxgen reads and can't, over 1920 px, with a restart marker; a GIF, empty and unused pictures |
| `image_refs_unresolved` | an unresolved href (`resolved_image_refs` false) found by its file name |
| `cover_paths` | a cover opening the first chapter, left out there, its ids moved on; the cover's picture used later in the text; a contents page's targets after the cover's page |
| `cover_page` | a PNG cover over 1920 px whose first page holds only its picture, which stays |
| `cover_alone` | a first page whose cover picture has only blank blocks after it: the picture stays, its ids with it |
| `table_parts` | table, row, row-group and cell borders (equal sides, per side, coloured, black); widths, `<col>` and cell column widths, padding in ems and %; vertical alignment, fills, cells setting their width; head, body and foot groups; header cells, one not bold, one of paragraphs; runs in cells; empty cells, one holding an id; ids of the table, of rows with no cells, of the block alone; tables of no rows written; ids of a picture the book doesn't hold around tables |
| `table_title` | a chapter's title in its table's first row, spelled over rows in source order, over a paragraph and a row (the rest kept) or the only row (the table gone); a heading in a cell; a title-only chapter with a table's ids on its parts; a table opening with the title off its first source row; a picture, then a table of the title's words |
| `block_styles` | Calibre-shaped paragraphs: the book's heading aligned, indented, italic, spaced above and below, in its language; centred, italic, taller and closer lines; hanging and first-line indents with and without a scene break's space; a margin of None; a list item's tight line; monospace text with a bold run; a right margin; a language of its own; kfxgen's heading in the page's language from a box; a box's language, its padding in ems and % at another size; the title split over a heading and a table's row |
| `box_titles` | a chapter's title split over a box's lines (through a box in a box, a paragraph and a row, stopped by a picture), in a box in a box, alone in a box (the box drawn round kfxgen's heading), running on in a box; a book heading in a box near the start or opening the first box; a box's own ids, a box in a box, a table in a box with styled, empty and big cells; a title over rules; a rule first in its storyline; a rule among the first blocks; a noteref to a footnote in a table cell |
| `main_size_tie` | two text sizes with as many characters each: the smaller is the book's main size |
| `frame_edges` | a paragraph opening with floats (#565): tight, a scene break's space, kfxgen's own above an indented and an unindented paragraph, a margin of None, first in its chapter, a heading level, ids before, at and after the cut, at its end and with no offset, a run over the cut, a float the book doesn't hold, in a bordered box; pictures in sized boxes (#382) in pt, em, px against a box in px, an unreadable size, a width of 0, nested a box deeper; a sized box bordered, floated right, to a side that isn't left, to ""; centred with no width |
| `inline_pictures` | pictures among words (#529, #565): no size, each width and height unit, a width with a given height in % and in lengths, each vertical-align keyword and length, one the book doesn't hold with runs into it, one it can't measure, at a text size of 24px and in a 0.75 chapter, two side by side, one ending its paragraph, in a sized box, in a paragraph of 4,700 characters; a linked one among words; a linked picture alone, with ids, at a location, its link to nothing, and one an earlier paragraph's ids land on; a linked float among words, cut into entries (kfxgen #21: a repeated piece takes the first's runs); floats among words of each size, a side that isn't left or right, the space after one, a frame's float and linked float among words; paragraphs cut after a picture line (its level, indent and margins on the right part, ids at the cut) and at an empty line (verse keeping its level, a scene break's space, a no-break-space spacer, ids in the blank lines, a run over the cut, a lost picture before the cut, a tight line, in a box, after floats); a chapter of plain text cut at an empty line |
| `font_faces` | embedded faces (#15, #50): a family of a bold, an italic and, last, a regular face, matched exactly and by falling back to the regular; a family of a bold italic and a bold face, no regular; a monospace family of a regular and a 600 weight; a family found second in a paragraph's list; bold and not-bold, italic and upright runs; a header cell's bold face, an italic cell's; monospace runs (#242) embedded, not embedded, in no family and in no families at all, inside a link; runs of their own size (#241, #375): bigger, at, back to and off the paragraph's size, with a link found and not, with super, a size of 0, a paragraph wholly in one, the book's main size set by runs; the book's heading in runs of one size, of two sizes and with no CSS (QA on #720, #721); a picture's resource before the faces |

**Calibre path** (`ebook-convert` with kfxgen, CSS applied): the source is
`sources/<name>.epub`. For each, the script also checks that replaying the model gives exactly
the KFX `ebook-convert` wrote.

| Fixture | Source | Features |
|---|---|---|
| `boxes` | kfxgen `test_books/boxes/boxes-source.epub` | boxes, borders, backgrounds |
| `rules_spaces` | kfxgen `test_books/rules_722/r722-kfxgen.epub` | rules (`<hr>`, 35 of them) |
| `spacers` | kfxgen `test_books/spacers_667/S667 kfxgen.epub` | spaces, spacing between blocks |
| `lists` | synthetic (`tools/ref/kfxgen_models.py synth`) | bulleted, numbered, lettered from 3, nested, `list-style: none`, `<dl>` |
| `notes` | kfxgen `test_books/popup_notes/n445b-kfxgen.epub` | pop-up notes (`noteref`), notes lists |
| `fonts` | synthetic | an embedded font (`@font-face`) mapped to text; italic with no italic face |
| `vertical` | kfxgen `test_books/vertical_247/s746k.epub` | vertical-rl Japanese, rtl spine, ruby |
| `ruby_tcy` | kfxgen `test_books/tcy_emphasis_748/s748k.epub` | tate-chu-yoko, emphasis marks, vertical |
| `rtl` | synthetic | Hebrew, `page-progression-direction="rtl"`, `dir="rtl"`, Latin and digits inside |
| `page_list` | kfxgen `test_books/landmarks_pages/l579-main.epub` | the EPUB's page-list, landmarks |
| `links` | kfxgen `test_books/link_runs_726/s726k.epub` | links and link runs, ruby |
| `frames` | synthetic, shaped as kfxgen `test_books/figures` and `floats` less a linked picture and a picture among words | figures 40% wide, centred at 60%, 300px and 12em wide, floated, shaded; pictures with no size, in px and high in a sized box; floats opening a paragraph, with ids; a float in a sized box |
| `floats` | kfxgen `test_books/floats/floats-fix-source.epub` | floats opening a paragraph and among its words, linked floats, a picture among words |
| `floats_697` | kfxgen `test_books/floats_697/F697 kfxgen source.epub` | floats in mid-sentence, left and right (#697) |
| `font_sizes` | kfxgen `test_books/font_sizes/font-sizes-source.epub` | paragraphs, spans and cells sized in CSS (#241) |

The `fonts` fixture embeds Liberation Serif 2.1.5 (SIL Open Font License 1.1), taken from
calibre's bundled fonts. The faces of `font_faces` are TrueType fonts of one empty glyph that
`kfxgen_models.py` makes with fontTools (`_tiny_font`), holding no other font's data. No fixture
contains MathML.

The 9-book Gutenberg corpus is dumped the same way, outside git:
`DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub`
(28 s on an M-series Mac, 174 MB).
