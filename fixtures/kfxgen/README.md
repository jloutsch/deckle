# kfxgen fixtures

Output of [kfxgen](https://github.com/jloutsch/kfxgen) (GPL-3), the reference for the native
converter (`docs/superpowers/specs/2026-11-10-native-kfx-design.md`, Oracles). The generator
code is kfxgen commit `ad38ec8` (5.24.0 plus #753 to #778). `models/` were dumped from that
commit with Deckle's own hook (`tools/ref/kfxgen_hook/`); nothing in the kfxgen repo is changed.

## golden/

Copied unchanged from kfxgen's `tests/fixtures/golden/expected/`. kfxgen builds them from
synthetic EPUBs (`tests/fixtures/golden/inputs.py`) without Calibre, through
`tests/fixtures/oeb_shim.py`, so they carry no per-element CSS.

| File | sha256 (first 16) |
|---|---|
| `minimal.kfx` | `73fbd3c30597b43e` |
| `with_cover.kfx` | `2694e310ff91d9b7` |
| `body_images.kfx` | `6cd2d5426d20092e` |
| `table_cells.kfx` | `2b646ac746a8baf9` |

Regenerate: `git -C <kfxgen> show ad38ec8:tests/fixtures/golden/expected/<name>.kfx > golden/<name>.kfx`
(`body_images.kfx` changed at `f605dd6`, #769; the others are as at `4b81651`).

**Found (phase 1, task 1):** `read_kfx` then `write_kfx` with `WriteOptions::from_doc`
reproduces all four byte for byte. kfxgen's container framing is the same as kfxlib's, so the
native generator can write through `deckle_kfx::container::write_kfx` with no changes to it.

## models/

One folder per fixture: `model.json.gz` and `resources/` (the arguments of kfxgen's
`generate_full_book`, written by Deckle's `KFXGEN_DUMP_MODEL` hook as `model.json`, which the
script stores gzip-compressed with no name or time in the header) and `expected.kfx` (what
kfxgen's generator makes from them). `deckle_kfxgen::load` reads either form; corpus models
stay plain `model.json`. `KFXGEN_COMMIT` names the kfxgen commit they came from, and
`TOOLS` the Calibre (`ebook-convert --version`) and the kfxgen venv's Python and lxml that made
them (calibre 9.18.0, Python 3.13.18, lxml 6.0.2). Regenerating expects the same Calibre: its CSS
handling shapes a Calibre-path fixture's `model.json`, so another version can change those, and
possibly their `expected.kfx`. Rebuild all of them with `tools/ref/kfxgen_models.sh`, or one
committed source with `KFXGEN_ONLY=<name>` (also with `--cascade` and `--convert`: only that
source's folder is made and replaced, and only when the run's kfxgen commit and tools are the
folder's); two runs
give the same `expected.kfx`, and the same `model.json` but for the order of the `images`
argument of a book with several pictures, which calibre lists in another order from run to run
(`block_picture_769`, `block_picture_770`, `pictures_side_by_side_766` and
`vertical_pictures_761` on 2026-11-14; the committed models were kept). The script
needs kfxgen `ad38ec8` or later (the commit the port follows), and stops on a
`KFXGEN_REF` that doesn't contain it. It exports commit
`KFXGEN_REF` (default `ad38ec8`) of the kfxgen repo `KFXGEN` (default `../kfxgen-public`, only
read) with `git archive`, puts the hook into the export
(`tools/ref/kfxgen_hook/inject.py` adds a call at the top of `generate_full_book`, and the two
calls `cascade/` needs, and stops if an anchor line has moved), and builds the Calibre plugin from that export. kfxgen's `convert`
runs the generator inside `page_width`, 480pt for a vertical-rl book (#754); that is not an
argument, so the dump doesn't hold it, and every replay
(`tools/ref/kfxgen_hook/generate_from_model.py`'s `generate`) sets it from `writing_mode` the same
way.

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
| `cover_in_box` | the cover's picture in a box in a box opening the first page, a caption after it: the picture left out, its ids and the inner box's on the caption; a link to the picture's id |
| `cover_box_alone` | the cover's picture alone in its box: the box left out, its ids and the picture's (which the box doesn't name) on the next block |
| `cover_mode` | the cover's picture opening the first page with a vertical writing mode the rest of the page doesn't carry (#247): the picture left out, the page horizontal |
| `table_parts` | table, row, row-group and cell borders (equal sides, per side, coloured, black); widths, `<col>` and cell column widths, padding in ems and %; vertical alignment, fills, cells setting their width; head, body and foot groups; header cells, one not bold, one of paragraphs; runs in cells; empty cells, one holding an id; ids of the table, of rows with no cells, of the block alone; tables of no rows written; ids of a picture the book doesn't hold around tables |
| `table_title` | a chapter's title in its table's first row, spelled over rows in source order, over a paragraph and a row (the rest kept) or the only row (the table gone); a heading in a cell; a title-only chapter with a table's ids on its parts; a table opening with the title off its first source row; a picture, then a table of the title's words |
| `block_styles` | Calibre-shaped paragraphs: the book's heading aligned, indented, italic, spaced above and below, in its language; centred, italic, taller and closer lines; hanging and first-line indents with and without a scene break's space; a margin of None; a list item's tight line; monospace text with a bold run; a right margin; a language of its own; kfxgen's heading in the page's language from a box; a box's language, its padding in ems and % at another size; the title split over a heading and a table's row |
| `box_titles` | a chapter's title split over a box's lines (through a box in a box, a paragraph and a row, stopped by a picture), in a box in a box, alone in a box (the box drawn round kfxgen's heading), running on in a box; a book heading in a box near the start or opening the first box; a box's own ids, a box in a box, a table in a box with styled, empty and big cells; a title over rules; a rule first in its storyline; a rule among the first blocks; a noteref to a footnote in a table cell |
| `main_size_tie` | two text sizes with as many characters each: the smaller is the book's main size |
| `frame_edges` | a paragraph opening with floats (#565): tight, a scene break's space, kfxgen's own above an indented and an unindented paragraph, a margin of None, first in its chapter, a heading level, ids before, at and after the cut, at its end and with no offset, a run over the cut, a float the book doesn't hold, in a bordered box; pictures in sized boxes (#382) in pt, em, px against a box in px, an unreadable size, a width of 0, nested a box deeper; a sized box bordered, floated right, to a side that isn't left, to ""; centred with no width |
| `inline_pictures` | pictures among words (#529, #565): no size, each width and height unit, a width with a given height in % and in lengths, each vertical-align keyword and length, one the book doesn't hold with runs into it, one it can't measure, at a text size of 24px and in a 0.75 chapter, two side by side, one ending its paragraph, in a sized box, in a paragraph of 4,700 characters; a linked one among words; a linked picture alone, with ids, at a location, its link to nothing, and one an earlier paragraph's ids land on; a linked float among words, cut into entries (kfxgen #21: a repeated piece takes the first's runs); floats among words of each size, a side that isn't left or right, the space after one, a frame's float and linked float among words; paragraphs cut after a picture line (its level, indent and margins on the right part, ids at the cut) and at an empty line (verse keeping its level, a scene break's space, a no-break-space spacer, ids in the blank lines, a run over the cut, a lost picture before the cut, a tight line, in a box, after floats); a chapter of plain text cut at an empty line |
| `two_links` | runs carrying two links, which kfxgen takes in target order since 0bc705d (#759; the same bytes under `PYTHONHASHSEED` 0 to 3): a picture and words under one, both landing; a picture alone under one; the smaller landing nowhere; a target only the larger names; a link colour with the other landing nowhere; a run of two links then one of the larger; noterefs whose smaller target is a footnote and isn't; an empty target with another, and alone, over words and a picture alone |
| `font_faces` | embedded faces (#15, #50): a family of a bold, an italic and, last, a regular face, matched exactly and by falling back to the regular; a family of a bold italic and a bold face, no regular; a monospace family of a regular and a 600 weight; a family found second in a paragraph's list; bold and not-bold, italic and upright runs; a header cell's bold face, an italic cell's; monospace runs (#242) embedded, not embedded, in no family and in no families at all, inside a link; runs of their own size (#241, #375): bigger, at, back to and off the paragraph's size, with a link found and not, with super, a size of 0, a paragraph wholly in one, the book's main size set by runs; the book's heading in runs of one size, of two sizes and with no CSS (QA on #720, #721); a picture's resource before the faces |
| `page_list_alone` | an EPUB page list (#578) with no landmarks or cover: pages at ids with and without offsets, at a whole file by its first id and by an id of its own, at an id declared twice; pages at another file and at an id in other capitals, which land nowhere |
| `landmarks_alone` | contents and start landmarks (#579) with no page list or cover: at a file, at an id with an offset, a second start after the first that lands (#609); landmarks that land nowhere, one of a kind kfxgen doesn't know; a page list that lands nowhere, with no approximate pages |
| `tcy_notes` | 縦中横 in a vertical Japanese book whose texts fill kfxgen's `content_math` fragment (#247): 4,096 and 4,095 bytes, a third of one byte that still joins them at 8,191, a fourth that starts `content_math2` at 8,192 |
| `vertical_page` | a vertical book (#754): `frame_edges`, `inline_pictures`, `table_parts`, `box_titles` and `nested_contents` together as horizontal-tb chapters in a vertical-rl book, so their frames, pictures among words, floats, cell and box padding, cell sides and contents levels are measured against Previewer's 480pt page; and a horizontal box holding a picture in a horizontal-tb chapter, where the picture isn't a plain one (#247) |
| `link_colour_runs` | link colours (#728, #732): a link's own colour with its target found, none of its own, its target not found so written as the text's colour against the page (#383) or inside a filled box or cell and on a box filled 0, in a language of its own, bold, twice; a coloured run inside a link found and not; an empty target, coloured and around a bold run, with an id `#top` whose file is empty |
| `contents_entries` | contents entries inside a chapter (#363): part-way into a paragraph, on a table and on a box (their first entry's start), declared nowhere, of no key; levels nested and left out; in a chapter left out of the contents; a rebuilt contents page's lines landing on them and on a place declared nowhere |
| `typography_runs` | typography (#239, #240, #247): small caps, case, spacing and colour on the book's heading, paragraphs (two colours only the bisection's last steps darken right), runs and cells (on the page, filled, filled 0, in a filled box); keywords Previewer doesn't write; a paragraph's text-orientation and emphasis marks; a run's colour inside a link not found; drop caps of each size, past their run, of a later run, of no size, with nothing after, counted from the end, in a language, in a cell |
| `split_title` | a title spelled over two paragraphs, and over two with a rule between (#64): kfxgen's heading takes the first block's style (#490) but not its small caps, transform, spacing and colour (#239); a title in one block, which gives them |

**Calibre path** (`ebook-convert` with kfxgen, CSS applied): the source is
`sources/<name>.epub`, each one. For each, the script also checks that replaying the model gives exactly
the KFX `ebook-convert` wrote. The script builds from the committed sources. kfxgen's
`test_books` is untracked in places, so no commit pins it: `kfxgen_models.sh --refresh-sources`
copies the sources again from the kfxgen working tree and names each one that changed. The
`fonts` source takes Liberation Serif from Calibre's fonts folder (`CALIBRE_FONTS`, by default
the macOS app's).

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
| `directions` | synthetic | direction and language in an English book (#246, #605, #710, #714): `dir`, `<bdi>`, `<bdo>`, `dir="auto"`, CSS `direction` and `unicode-bidi` on runs, nested, in bold, in and around links, under a language; `lang` alone on runs, over italic, over a whole sized paragraph; paragraphs right to left, `text-align: start`, `plaintext`; tables, row groups, rows and cells of their own direction and language; boxes of their own; a rebuilt contents page; a chapter of a table alone under kfxgen's heading; a chapter set vertical-rl in an English book |
| `ruby` | synthetic | ruby in a horizontal Japanese book (#236): over one character, two, two runs, a bold run, the start of a link; the same reading twice; `<rp>`; chapters opening with a paragraph whose contents label runs the readings in bare, in parentheses and in full-width ones (two rubies, one over two runs); a heading whose label leaves them out; a chapter set vertical-rl (#247): margins, a box's padding, a table, an upright run |
| `vertical_contents` | synthetic | a vertical-rl Japanese book (rtl spine) whose contents page kfxgen rebuilds, a chapter of no writing mode of its own (#247) |
| `horizontal_blocks` | synthetic, shaped as kfxgen 0bc705d's `test_calibre_vertical_writing.py` and `test_calibre_words_around_pictures.py` | horizontal blocks in a vertical Japanese book (#247): a horizontal paragraph, boxes plain, bordered, padded and filled, a table; pictures in them (#755) plain, aligned left, centre, right and justified, sized by a width in % and a height in px, linked, floated, beside a paragraph and linked beside one; words around a picture in a centred block (#757); margins and padding against the 480pt page (#754) |
| `horizontal_blocks_ltr` | synthetic, the same files | the same on an ltr spine: the files are vertical, the book isn't, so a picture in a horizontal block is a block picture aligned as its block (#755), on the 384pt page |
| `page_list` | kfxgen `test_books/landmarks_pages/l579-main.epub` | the EPUB's page-list, landmarks |
| `links` | kfxgen `test_books/link_runs_726/s726k.epub` | links and link runs, ruby |
| `frames` | synthetic, shaped as kfxgen `test_books/figures` and `floats` less a linked picture and a picture among words | figures 40% wide, centred at 60%, 300px and 12em wide, floated, shaded; pictures with no size, in px and high in a sized box; floats opening a paragraph, with ids; a float in a sized box |
| `floats` | kfxgen `test_books/floats/floats-fix-source.epub` | floats opening a paragraph and among its words, linked floats, a picture among words |
| `floats_697` | kfxgen `test_books/floats_697/F697 kfxgen source.epub` | floats in mid-sentence, left and right (#697) |
| `font_sizes` | kfxgen `test_books/font_sizes/font-sizes-source.epub` | paragraphs, spans and cells sized in CSS (#241) |
| `link_greys` | kfxgen `test_books/link_greys_741/s741k.epub` | links in light and dark greys, one around a black word, plain links (#728, #741) |
| `navigation` | kfxgen `test_books/navigation/nav-kfxgen.epub` | an NCX naming places inside chapters (#363): of no title, of no place, a chapter's start again; a contents page's line landing on one |
| `typography` | kfxgen `test_books/typography/typography-new-source.epub` | small caps, text-transform, letter spacing and colour on paragraphs, headings and runs, set back inside them; a drop cap (#239, #240) |
| `colour` | kfxgen `test_books/colour/colour-new-source.epub` | text colours on paragraphs and runs, light ones darkened against the page (#239, #383) |
| `link_colours` | kfxgen `test_books/link_colours_732/s732k.epub` | grey, red and black links, grey words inside and around links (#728, #732) |
| `dark_boxes` | kfxgen `test_books/dark_box_colours_734/c734k.epub` | text colours in light and dark boxes (#383, #734) |
| `drop_caps` | kfxgen `test_books/drop_caps/drop-caps-new-source.epub` | drop caps of several sizes and lengths (#240) |
| `figures` | kfxgen `test_books/figures/figures-new-2-source.epub` | figures and captions, a run in small caps |
| `horizontal_blocks_749` | kfxgen `test_books/horizontal_blocks_749/s749k.epub` | horizontal blocks in a vertical Japanese book: plain, bordered and padded boxes (#749) |
| `vertical_pictures_761` | kfxgen `test_books/vertical_pictures_761/s761k.epub` | pictures directly in vertical-rl text, sized by a height or a width, linked, aligned (#761) |
| `vertical_in_horizontal_763` | kfxgen `test_books/vertical_in_horizontal_763/s763k.epub` | vertical-rl blocks inside horizontal ones in a vertical book, their margins and padding (#763) |
| `lists_horizontal_items_764` | kfxgen `test_books/lists_horizontal_items_764/s764k.epub` | horizontal items in vertical lists, the list's margins on them, padded wrappers (#764) |
| `turned_words_765` | kfxgen `test_books/turned_words_765/s765k.epub` | words set the other way from their line: horizontal in vertical text, vertical in a horizontal block, with runs and links (#765) |
| `pictures_side_by_side_766` | kfxgen `test_books/pictures_side_by_side_766/s766k.epub` | pictures side by side on one line, on lines of their own, linked, sized in em (#766) |
| `lists_768` | kfxgen `test_books/lists_768/s768m.epub` | lists as list entries: marker styles, starts, values, inside markers, unmarked lists (#768) |
| `block_picture_769` | kfxgen `test_books/block_picture_769/s769k.epub` | block pictures placed by their block: alignment, indent, side margins, linked (#769) |
| `block_picture_770` | kfxgen `test_books/block_picture_770/s770k.epub` | block pictures in nested % and em margins, own margins, `display: block` with auto margins (#769) |
| `lists_771` | kfxgen `test_books/lists_771/s771k.epub` | more lists: unmarked indents, empty items, start 0, armenian, items of several blocks (#768) |
| `chapter_starts_773` | kfxgen `test_books/chapter_starts_773/t773k.epub` | contents entries naming an id at a file's very end (#772) |
| `chapter_starts_777` | kfxgen `test_books/chapter_starts_777/t777k.epub` | a closing rule's id and the heading above a contents target as chapter starts (#774, #776) |
| `css_quirks` | synthetic (`tools/ref/make_css_quirks_epub.py`) | Calibre's CSS quirks, one file per group (see `cascade/`) |
| `package_quirks` | synthetic (`tools/ref/make_cascade_epubs.py`) | a package Calibre changes: cover by `<meta>`, a spine item twice and one `linear="no"`, a year-only date, `en-US`, an empty NCX label |
| `no_toc` | synthetic, the same script | neither NCX nor nav: Calibre makes the contents |
| `fake_margins` | synthetic, the same script | margins Calibre's RemoveFakeMargins removes and keeps |
| `malformed` | synthetic, the same script | files Calibre reads with its HTML parser, entities, HTML 4, windows-1252 |
| `comments_in_tree` | synthetic, the same script | comments where Calibre's steps count lxml's children: an empty `<b>` and a `<br>` holding one, one between the head's children, a rule inside one in `<style>`, a drop cap holding one, one before the children `:first-child` and `:nth-child(2)` pick, one beside an only element child (`:only-child`) and one before a `:first-of-type`, comments whose tails hold text |
| `text_edges` | synthetic (`tools/ref/make_convert_epubs.py`) | paragraphs of only no-break spaces, U+001C in text (Calibre drops it), a tab in `<pre>`, a literal U+2028, astral characters before a link; shifts in shifts, a not-bold span in bold and in a heading, `<sub>`, line-through by `<s>`, `<del>` and CSS |
| `block_edges` | synthetic, the same script | `<pre>` with spaces, tabs and blank lines; hidden elements; an `<epub:switch>`; page breaks before and after; `clear` after a float; comments in and between paragraphs |
| `tables_more` | synthetic, the same script | a rows-path table (a hidden row, loose text), a notes table, a row holding a table, a caption, colspan and rowspan, cells of paragraphs |
| `pictures_more` | synthetic, the same script | a GIF, a CMYK JPEG, a 2,400 px PNG, an `<svg>` around an `<image>`, a page drawn by a CSS background picture |
| `fonts_woff` | synthetic, the same script | one WOFF 1 face (Liberation Serif cut to the book's glyphs) |
| `package_more` | synthetic, the same script | landmarks from the OPF guide only, an NCX `pageList`, a year-only date, a publisher, a `class="toc"` listing, a dedication |
| `listing_rerun` | synthetic, the same script | a `class="toc"` listing on an opening page holding the title and a byline, and no Contents chapter: kfxgen converts the book a second time keeping the listing (`keep_nav_listings`) |
| `marks` | synthetic, the same script | link colours by `a:link`, a class's `a:visited`, a bare `a:visited` and a link's own style, in a vertical Japanese book; a horizontal block in a vertical-rl file, vertical text inside it |

The generator refuses four of these models, as it refuses any book needing a feature not ported
yet (phase 3 plan, Decision 6): `block_edges`, `css_quirks` and `text_edges` (preformatted text)
and `tables_more` (block styles in table cells). `tests/it/whole_book.rs` checks that each is
refused naming that feature.

The sources from kfxgen's `test_books` were written by kfxgen's author for its tests: made-up
text and test pictures, no other book's content. kfxgen does not publish all of them; the copies
here are offered under the GNU General Public License, version 3, as kfxgen is.

The `fonts` fixture embeds Liberation Serif 2.1.5 (SIL Open Font License 1.1), taken from
calibre's bundled fonts, in `sources/fonts.epub` and `models/fonts/resources/`. Its licence is
`sources/LiberationSerif-OFL.txt`. The faces of `font_faces` are TrueType fonts of one empty glyph that
`kfxgen_models.py` makes with fontTools (`_tiny_font`), holding no other font's data. The WOFF
face of `fonts_woff` is the same Liberation Serif, cut to the glyphs that book uses, under the same
licence file. No fixture contains MathML.

The 9-book Gutenberg corpus is dumped the same way, outside git (`gutenberg/` is in
`.gitignore`). Its books are Project Gutenberg's EPUB 2 with images ("EPUB (older E-readers)"),
saved as `gutenberg/pg<id>.epub` from `https://www.gutenberg.org/ebooks/<id>.epub.images`
(which serves `https://www.gutenberg.org/cache/epub/<id>/pg<id>-images.epub`):

| File | Title | sha256 of the copy used |
|---|---|---|
| pg11.epub | Alice's Adventures in Wonderland | `51e684a97bc63fb1007e433d6eee6d07508cec4c234080281a1483868f4696ae` |
| pg23.epub | Narrative of the Life of Frederick Douglass, an American Slave | `4ff2ebe464657f9e98a25056e0beb92fea2e70ee54b958baa16e04ea0910d7b3` |
| pg43.epub | The strange case of Dr. Jekyll and Mr. Hyde | `2fb4cc132937360fedf1548e815188a5ac27cf5b383f523bc181e0234e1eb5c8` |
| pg55.epub | The Wonderful Wizard of Oz | `b397de9e62c1f12bf597f5775d11bd9f1bfb8517b605f6df203290972bcfa61e` |
| pg74.epub | The Adventures of Tom Sawyer, Complete | `f7ec56a0768b0a202a98aa4bf8ff1fc54c738ac36c0d6c2ea853154474c1b12d` |
| pg76.epub | Adventures of Huckleberry Finn | `ba2554e8bb1b33a3c6b8813abf20a306d76036b38be128622efe112fd62c139a` |
| pg84.epub | Frankenstein; or, the modern prometheus | `8959a5fdc05e48c83574427a9ffe81b5303cffe5457d4279736f236ffea5a157` |
| pg86.epub | A Connecticut Yankee in King Arthur's Court | `c48f96b05cc2753556b9885d55d256556bc97bb80482b37d41f136c4e150dc82` |
| pg98.epub | A Tale of Two Cities | `75fdf6049eca262f24ec95f9fbeae8aef97438fd1e545436c7072fc23f4b38a7` |

Project Gutenberg rebuilds these files from time to time (the copies used say they were converted
on 2026-08-01), so a fresh download can differ from them: on 2026-11-11 `pg11-images.epub` was
187,950 bytes, not 187,952. A different copy still makes a valid corpus, since the dump and the
test both start from whatever is in `gutenberg/`; it just isn't the same books byte for byte.
To dump them:
`DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub`
(82 s on an M-series Mac on 2026-11-13, 186 MB; 28 s and 174 MB before each book also got its
`cascade/` dump, which the same run writes to `<book>/cascade`), which writes `KFXGEN_COMMIT`,
`TOOLS` and `BOOKS` (the books it dumped) there too, once every book is made. Its conversions run
with `TZ=UTC`, as `--cascade`'s do, and `TOOLS` says so with the month of the run; each
`<book>/cascade` holds its own `ua_rules.json`. The corpus test stops unless the folder holds
exactly the books in `BOOKS`: a dump that failed leaves no list, and a book left from an earlier
corpus isn't in it (delete its folder). Two dumps
give the same `expected.kfx` for every book, but not always the same `model.json`: in two dumps of
pg74 the `images` argument held the same pictures in another order (pg76, pg86 and pg98's
`model.json` also changed on a redump, their `expected.kfx` not). The port makes the same bytes
from either order. `crates/deckle-kfxgen` makes every corpus book byte for
byte: `DECKLE_KFXGEN_CORPUS=gutenberg/models cargo test -p deckle-kfxgen --test it corpus -- --ignored`.
The same command also runs `corpus_cascade`, which reads each book with `book::read_book` and
compares it with its `cascade/` dump as `book::read_book_matches_every_dump` compares the
fixtures below; it reads the EPUBs from `DECKLE_KFXGEN_CORPUS_EPUBS`, else from the corpus
folder's parent (`gutenberg/`), and refuses a corpus whose `TOOLS` is not Calibre 9.18.0 in UTC.

## cascade/

What Calibre hands kfxgen, dumped inside a real conversion by `tools/ref/kfxgen_hook/book_dump.py`
(`KFXGEN_DUMP_BOOK`), the reference for phase 2's `epub` and `css` modules. kfxgen never sees the
book's CSS: Calibre flattens it to one class per element first, and kfxgen builds its own
`Stylizer` on the result. Rebuild with `tools/ref/kfxgen_models.sh --cascade` (under 40 s on an
M-series Mac); two runs give the same bytes. `KFXGEN_COMMIT` and `TOOLS` are as in `models/`
(calibre 9.18.0); `TOOLS` also says the dump ran with `TZ=UTC` (Calibre turns a date with no
time into local midnight) and in which month (a year-only `dc:date` takes day 15 of the month
of the run, so `package_quirks`'s date changes with it). The test harness refuses a dump whose
`TOOLS` names another Calibre or no `TZ: UTC` (`tests/it/support.rs` `check_tools`).
`tests/it/book.rs` `read_book_matches_every_dump` reads each source with `book::read_book` and
compares the result with its dump at four levels: book view (`oeb.json`), element tree and
first-pass rules (`dom/`, `rules/`), declared styles and classes (`styles/` `specified`,
`pseudo`), computed values (`styles/` `computed`).

`ua_rules.json` holds Calibre's user-agent rules (its `html.css`), the same for every file of
every book (the dump and the script both check), and one folder per source,
`sources/<name>.epub`:

| File | What it holds |
|---|---|
| `oeb.json` | the book view kfxgen is given: `spine` (`href`, `linear`), `page_progression_direction`, `manifest` (`id`, `href`, `media_type`, sorted by href), `toc` (`title`, `href`, `children`), `auto_generated_toc`, `guide` (`type`, `href`, sorted by type), `metadata` (`title`, `creator`, `description`, `language`, `publisher`, `date`, `cover`, each a list), `raw` |
| `files.json` | spine href → `NNN`, its place in the spine |
| `dom/NNN.xhtml` | the file's tree as kfxgen gets it (flattened, chapter marks in, `data-kfxgen-*` marks in), written after kfxgen's Stylizer is built |
| `styles/NNN.json` | `table`: distinct styles, each `specified` (`cssdict()`), `pseudo` (the pseudo-class dicts) and `computed` (`Style[prop]` for 34 properties; `{"error": "<exception>"}` if it raises, `{"number": "nan"}`, `"inf"` or `"-inf"` for what JSON can't hold); `nodes`: `[path, table index]` per element in document order, the path being element-child indexes from the root; `font_face` |
| `rules/NNN.json` | Calibre's first-pass Stylizer, before flattening: `rules`, the book's rules as `[origin, selector, declarations, important, key]` in cascade order (`important` the sorted names of its `!important` properties, `key` Calibre's sort key `[origin, a, b, c, d, index]`); `ua_rules`, how many user-agent rules come before them (all of `ua_rules.json`, same form); `font_face` |

The manifest is Calibre's after its transforms: the book's stylesheets are gone, Calibre's
`stylesheet.css` and `page_styles*.css` are in. Two things in a dump are not as kfxgen has them:

- `raw` is the EPUB's own OPF (`opfname`, its path in the EPUB, and `sha256`). The OPF kfxgen
  reads is the one Calibre writes, `content.opf` at the unpacked root, which names an NCX made
  from the nav under a random file name, so it differs on every run. When the book's own OPF is
  that root `content.opf` (`notes`, `vertical`, `navigation`), Calibre writes over it and
  `sha256` is `null`.
- In `dom/`, an element's classes are sorted when Calibre gave it several (a class and
  `pcalibre` classes). Calibre joins them from a Python set, so their order changes from run to
  run, and its Python ignores `PYTHONHASHSEED`. kfxgen's own tree is not changed.

| Source | Spine files | Size | Why |
|---|---|---|---|
| `css_quirks` | 7 | 274 KB | made by `tools/ref/make_css_quirks_epub.py`: one file per group of quirks the phase 2 tests name (`cleanups`, `sheet`, `shorthand`, `selectors`, `cascade`, `flatten`, `values`) |
| `package_quirks` | 4 | 33 KB | made by `tools/ref/make_cascade_epubs.py`: an HTML cover page, a `linear="no"` file, a duplicate spine entry, a linked file the spine leaves out, a year-only date, `en-US`, an NCX entry with no label |
| `no_toc` | 2 | 17 KB | the same script: no NCX and no nav, so Calibre makes the contents |
| `fake_margins` | 2 | 20 KB | the same script: a margin `RemoveFakeMargins` removes, and one it keeps |
| `malformed` | 4 | 29 KB | the same script: files Calibre does not read as plain XHTML: tags left open and an unquoted attribute (its HTML parser), DOCTYPE entities, HTML 4 in upper case, windows-1252 |
| `comments_in_tree` | 1 | 14 KB | the same script: comments where Calibre's steps count or read lxml's children (`parse_html`'s empty-element removal, `<br>` and head whitespace, the Stylizer's `<style>` text, a drop cap, `:first-child` and `:nth-child`) |
| `lists` | 2 | 28 KB | lists |
| `notes` | 6 | 62 KB | pop-up notes (`noteref`), notes lists |
| `vertical` | 3 | 51 KB | vertical-rl Japanese, rtl spine, ruby |
| `rtl` | 2 | 18 KB | Hebrew, `page-progression-direction="rtl"` |
| `fonts` | 1 | 13 KB | an embedded font (`@font-face`) |
| `directions` | 6 | 68 KB | tables, direction and language |
| `frames` | 6 | 78 KB | figures, pictures, floats |
| `navigation` | 4 | 37 KB | an NCX naming places inside chapters |

761 KB in all, with `ua_rules.json` (19 KB).

## convert/

What kfxgen's converter makes on its way to the chapters, dumped inside a real conversion of each
source by `tools/ref/kfxgen_hook/convert_dump.py` (`KFXGEN_DUMP_CONVERT`), the reference for phase
3's `convert` module, layer by layer. Rebuild with `tools/ref/kfxgen_models.sh --convert`, after
the models (under 30 s on an M-series Mac); two runs give the same bytes. Each run also dumps its
model, which must be the committed `models/<name>/model.json.gz` byte for byte (but for the order
of the `images` argument, which calibre changes from run to run anyway) and make its
`expected.kfx`: the dumps change nothing. `KFXGEN_COMMIT` and `TOOLS` are as in `models/`.

Per source, `convert/<name>/` holds gzip JSON in `model_dump.py`'s encoding (`convert_dump.py`'s
docstring has every key):

| File | What |
|---|---|
| `files.json` | spine href to `NNN`, its place in calibre's spine |
| `resolver/NNN.json.gz` | kfxgen's style resolver on every element of the file, and `compute_block_style` of it |
| `inline/NNN.json.gz` | each paragraph's runs (`_walk_inline` from a block) and `normalize_runs_with_anchors` of them, first pass; a second pass's in `inline/NNN.pass1.json.gz` |
| `blocks/NNN.json.gz` | `extract_blocks_from_html`'s blocks and out-lists, and the input of each `_space_between` call with its margins: one record per pass, its `pass` 0 or 1 |
| `toc.json.gz` | contents entries, spine order, note ids, link targets |
| `assembly.json.gz` | per pass, the files before chapter assembly and the chapters after it |
| `calibre_opf.xml`, `calibre_ncx.xml` | the OPF calibre wrote and the NCX it names, verbatim but for the random name of an NCX calibre makes from a nav (`tmpXXXXXXXX.ncx`) |

`convert/units/inline_style.json` is a table of inputs and outputs of the pure functions of
kfxgen's `inline_style.py` and `font_table.py`, made by importing them, without Calibre.
`crates/deckle-kfxgen/tests/it/convert_support.rs` reads the dumps (`dump`) and compares a value
with one (`model_diff`). Only `listing_rerun` makes a second pass (`keep_nav_listings`): its `assembly.json.gz` holds two
passes, its `blocks/` two records, and it has `inline/NNN.pass1.json.gz`; every other book one.
`resolver/` is the first resolver built for each file; the dump stops if any later one (the second
pass's, `_footnote_keys`') gives other values, so it holds for every pass.

## Phase 1 timings

`cargo run --release -p deckle-kfxgen --example from_model -- gutenberg/models/<book> out.kfx`
on an M-series Mac, median of three runs. It times `generate` alone: the model is already
loaded, and nothing reads the EPUB or applies CSS. kfxgen's column is a whole `ebook-convert`
run from `gutenberg/kfxgen/timings.csv` (Calibre start-up, EPUB parsing, CSS and kfxgen's
generator together), so the two columns don't measure the same work. The whole-pipeline speed
goal is checked in phase 3.

| Book | EPUB size | `generate` (ms) | kfxgen through Calibre (s) |
|---|---|---|---|
| pg11 | 0.2 MB | 23.2 | 2.91 |
| pg23 | 0.4 MB | 16.4 | 1.46 |
| pg43 | 0.3 MB | 15.0 | 1.56 |
| pg55 | 0.3 MB | 26.5 | 3.19 |
| pg74 | 16.7 MB | 92.3 | 4.81 |
| pg76 | 16.0 MB | 101.0 | 7.74 |
| pg84 | 0.5 MB | 30.3 | 2.61 |
| pg86 | 26.1 MB | 112.4 | 6.54 |
| pg98 | 7.9 MB | 94.0 | 8.20 |
| total of the rows | | 511.1 | 39.02 |

Whole run (from `gutenberg/kfxgen/timings.csv`, its `TOTAL` line): 39.22 s. That is 0.20 s more
than the rows; the script that wrote the file isn't in the repo, so what the difference is (most
likely the time between books) isn't recorded.
