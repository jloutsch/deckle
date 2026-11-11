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

**Argument path** (`kfxgen_models.py args`): books written as `generate_full_book`'s chapters,
for run flags the shim can't make (it applies no CSS) on blocks with no CSS (a Calibre-path
block always has some). The books are `ARG_BOOKS` in that script.

| Fixture | What it holds |
|---|---|
| `inline_runs` | a heading's italic and not-bold words; `url:`, unfound, other-capitals and whole-file link targets; a link over a bold word beside more of it; super, sub, text-top, text-bottom, baseline, underline, line-through; ids inside, at and past a paragraph's end |
| `nested_contents` | a rebuilt contents page at 0.75 with lines at depths 1 and 2 and targets past either end; a heading-only chapter whose ids land in kfxgen's heading; a title over two blocks; a 0.75 chapter with runs and links |
| `heading_runs` | the book's heading with words after its title: its bold and italic runs and ids in kfxgen's heading, links to them |

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

The `fonts` fixture embeds Liberation Serif 2.1.5 (SIL Open Font License 1.1), taken from
calibre's bundled fonts. No fixture contains MathML.

The 9-book Gutenberg corpus is dumped the same way, outside git:
`DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub`
(28 s on an M-series Mac, 174 MB).
