# Native KFX, phase 1 (the generator): outcome

Date: 2026-11-12. Branch `feat/native-kfx-generator` (61 commits on `main` `d28aa6d`, not pushed). Plan: `docs/superpowers/plans/2026-11-10-native-kfx-phase1-generator.md`. Issue: jloutsch/deckle#2.

## Result

- `crates/deckle-kfxgen` turns kfxgen's generator input (`model.json`) into a KFX **byte-identical to kfxgen's**, pinned to kfxgen `0bc705d`.
- **70 fixtures** (`fixtures/kfxgen/models/`) and the **9-book Gutenberg corpus** (`gutenberg/models/`, outside git) all match byte for byte; `Unsupported` fails the suite. Tests: lib 1 + it 104 (+1 corpus test, run with `DECKLE_KFXGEN_CORPUS=gutenberg/models`).
- Generator time: 15–112 ms per corpus book, 511 ms for all 9 (release). kfxgen through Calibre: 1.46–8.20 s per book (whole `ebook-convert` runs, not the same work). Kindle Previewer: 28–81 s.
- Fixtures regenerate with `tools/ref/kfxgen_models.sh`: it exports a kfxgen commit with `git archive` and injects Deckle's dump hook (`tools/ref/kfxgen_hook/`). kfxgen itself is only read. Tool versions are recorded in `fixtures/kfxgen/models/TOOLS`.
- Deliberate differences from kfxgen: `crates/deckle-kfxgen/DIVERGENCES.md` (all need input the converter never writes, or absurd values; plus nesting limits of 32 levels for contents and boxes).
- Still `Unsupported` (no corpus book needs them): block-styled paragraphs in table cells, pictures in table cells and list items, a table cell cut at an empty line, preformatted text, clearing floats, MathML.

## How the plan changed

Tasks added on the way, each because a review or a corpus run found work no task owned: 7b (split `text.rs`, byte-exact tests), 9b (frames), 9c (pictures among words, empty-line cuts), 12b (link colours, typography, contents entries), 12c (dump hook moved into Deckle, at the user's choice), 12d (re-pin to `0bc705d`). Tasks 5 and 6 were merged. Paragraph block styles (Task 9) and sized/monospace runs (Task 10) were added to existing tasks.

## Before phase 3 (from the final review)

1. Revisit the Task 4 ruling: turn `BLOCK_UNPORTED` / `CHAPTER_UNPORTED` into allow-lists of the keys the port reads, so keys Deckle's own converter writes can't be silently ignored.
2. Split `build_chapter_content` (~650 lines) and the large modules (`images.rs`, `inline.rs`, `writing.rs`, `boxes.rs`, 735–953 lines); remove the `images` ↔ `chunks` mutual import.
3. Small structure items: `build_fragment_259`'s 8 parallel slices, `Node::Group` as a string, the `dedupe` / `dedupe_keys` duplicate, `too_many_arguments` at `inline.rs:628`.

Smaller items that can wait are listed in the final review's triage; none affects output on any fixture or corpus book.

## Before the final port ships

- Test #88 in the running desktop app with a reader attached. The fix (`16eb377`, an awaited
  `platform.confirm`) passed the build and unit tests but was never run live: press Remove on a
  book on a Kobo, PocketBook or reMarkable and choose Cancel (the book must stay), then OK; send
  a puzzle book that is already on the device and answer "Replace it?" both ways.

## Found in kfxgen

- jloutsch/kfxgen#759 (filed 2026-11-11): output depended on `PYTHONHASHSEED` when one run carried two links. Fixed upstream in #760; the port follows the fix.
