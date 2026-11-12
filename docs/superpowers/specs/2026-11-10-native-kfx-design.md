# Native KFX conversion (a Rust port of kfxgen) — Design

Date: 2026-11-10. Status: draft for review. Supersedes nothing; Kindle Previewer stays as an
optional path.

## Why

Deckle makes a Kindle book in two halves (see `docs/formats/kfx.md`):

1. **EPUB → KFX content.** Kindle Previewer reads the HTML and CSS and writes storylines
   (`$259`), styles (`$157`), sections, images, tables and navigation into a KPF.
2. **KPF → sideloadable KFX.** `deckle-kfx` (a Rust port of kfxlib's fix-ups) adds position and
   location maps, stamps ASIN and PDOC/EBOK, adds the page list, and writes the monolithic KFX.

Half 1 is Previewer, and Previewer has three problems:

- **It is slow.** On the 9 Gutenberg books in `gutenberg/` (2026-11), Previewer took 28–81 s
  per book, 499 s total. kfxgen through Calibre took 1.5–8.2 s, 39 s total
  (`gutenberg/kfx/timings.csv`, `gutenberg/kfxgen/timings.csv`).
- **It can't run in a container.** It is Mac and Windows only. The Docker image and the Linux
  tarball can't make Kindle books (`docs/docker.md:17-18`, `:170`).
- **It is a separate download.** Users must install it before any Kindle book works; the app
  gates runs on it (`apps/desktop/src/App.tsx:502-510`, `settings/Sending.tsx:65-88`).

kfxgen (the kfxgen repository, GPL-3, same author) already does half 1 natively,
but in Python inside Calibre. This project ports it to Rust inside Deckle, so half 1 runs in the
Deckle binary with no Python, no Calibre and no Previewer.

## Goals

1. Reflowable books convert to KFX with no Previewer, on macOS, Windows, Linux and Docker.
2. Faster than kfxgen through Calibre: under 39 s total on the 9-book corpus, and under 2 s for
   each text-only book (pg11, pg23, pg43, pg55, pg84) on an M-series Mac, release build.
3. Nothing extra to download or install. The converter is part of the `deckle` binary.
4. The output matches kfxgen's, checked by oracles (below), not by eye.
5. Everything that reads Deckle's KFX keeps working on native output: cover thumbnail
   (`thumbnail::cover_jpeg_from_kfx`), X-Ray and Word Wise (`deckle_kfx::text::content_runs`),
   section reads (`section_first_runs`), ASIN/EBOK stamping, page list, the outbox.

## Non-goals

- **Fixed-layout books** (comics, fixed-layout EPUB). kfxgen only detects fixed layout
  (`converter.py:938`); it does not lay pages out. These keep the Previewer route (and the
  existing AZW3 route that already skips Previewer, `pipeline.rs:498-541`).
- **MathML in the first release.** kfxgen draws math as glyph outlines through a vendored
  ziamath/ziafont (about 13k lines of Python plus a math font). Porting that is a later,
  separate project. Until then, see "Unsupported content" below.
- Running Python or Calibre from Deckle.
- Byte equality with Previewer's output.
- Removing Previewer support.

## Anti-goals (a failure even if it works)

- A Mac or Windows user who has Previewer today gets a worse book after an update: text lost,
  a book that no longer opens, X-Ray or Word Wise gone. Native becomes the default only after
  device sign-off (phase 5).
- The port drifts from kfxgen with no record. Every deliberate difference from kfxgen is written
  down in `crates/deckle-kfxgen/DIVERGENCES.md` with a reason.
- New network calls, downloads, or telemetry.

## Constraints

- License GPL-3.0-or-later, SPDX header on every new source file. The port is a derivative of
  kfxgen; `About.tsx:122` already credits kfxgen.
- **Pinned upstream:** kfxgen 5.24.0 plus #753 to #760, commit `0bc705d`. Recorded in
  `crates/deckle-kfxgen/src/lib.rs`, `tools/ref/kfxgen_models.sh` (`KFXGEN_REF`) and
  `fixtures/kfxgen/README.md`.
- Reuse `deckle-kfx` for Ion, symbols, fragments, container read/write, metadata stamping,
  position/location maps, page list and rebuild. No second Ion encoder or container writer.
- HTML and CSS: use crates already in `Cargo.lock` (`html5ever`/`dom_query`, `cssparser`,
  `selectors`). Any new dependency (expected: a WOFF2 decoder; an SVG rasterizer such as
  `resvg`) is named and justified in the phase plan that adds it.
- Deterministic output: the same input gives the same bytes. No time, randomness or hash-map
  iteration order in the output.
- Same repo conventions as the rest of Deckle: short comments with spec references, feature
  branch per phase, commit per task.

## Architecture

```
prepare (device-independent)                              finish (per Kindle)
──────────────────────────────                            ─────────────────────────
normalized EPUB                                           unstamped book.kfx
  → epub     : OPF, spine, manifest, TOC, metadata          → read_kfx → Book::from_kfx
  → css      : computed style per element                   → metadata::apply (ASIN, EBOK…)
  → convert  : chapter model   (port of converter.py)       → add_approximate_pages
  → generate : fragments       (port of native_generator)   → rebuild → write_kfx
  → deckle-kfx Book → write_kfx → unstamped book.kfx         → thumbnail, X-Ray, Word Wise
```

- New crate `crates/deckle-kfxgen`, depending on `deckle-kfx`. Modules: `model` (the chapter
  model), `generate`, `epub`, `css`, `convert`.
- **The chapter model is the seam.** It mirrors the dict contract kfxgen's `converter.py`
  hands to `NativeKFXGenerator.generate_full_book` (`native_generator.py:4549`): chapters,
  paragraph blocks with spans and flags, tables, boxes, image tokens, the font table, landmarks
  and page list. It is serde-serializable, and its JSON form is the same one a dump hook in
  kfxgen writes. That shared JSON is what makes every phase testable against Python.

## Oracles (how correctness is checked)

| Level | Input | Python side | Rust side | Pass |
|---|---|---|---|---|
| Generator | `model.json` dumped by kfxgen | kfxgen generator → KFX | `generate` → KFX | byte-identical |
| Cascade | XHTML + CSS | Calibre `Stylizer` dump (`calibre-debug` script) | `css` | equal values for every property kfxgen reads |
| Converter | normalized EPUB | kfxgen via `ebook-convert` → `model.json` | `epub`+`css`+`convert` → model | equal, except entries in `DIVERGENCES.md` |
| End to end | 9 Gutenberg books + kfxgen `test_books` sources | kfxgen KFX | Deckle KFX | text retention ≥ kfxgen's; fragment histogram per type within listed tolerance; timing goal met |
| Device | corpus sideloaded on a Kindle | — | — | opens, TOC, links, pop-ups, cover, X-Ray, Word Wise (user signs off) |

The CSS properties the cascade must produce are the ones kfxgen reads (resolver at
`converter.py:1030-1226`): font-size, line-height, font-family, font-weight, font-style,
font-variant, text-transform, letter-spacing, text-align, text-indent, margin-*, padding-*,
border-*-{width,style,color}, border-collapse, color, background(-color, -image, -repeat),
float, clear, width, height, vertical-align, direction, unicode-bidi, writing-mode,
text-orientation, text-emphasis-style, text-combine-upright, text-decoration, white-space,
list-style-type, display, visibility, page-break-*/break-*, quotes, `::first-letter`.

It must also reproduce what kfxgen gets from Calibre's CSS flattening: Calibre's user-agent
sheet (e.g. `hr { height: 2px }`), font-size keywords → rem via the input profile, `width`/
`height`/`align` attributes folded into CSS, `-epub-writing-mode` read as `writing-mode`, and
the facts kfxgen's `specialize_css_for_output` hook records before flattening
(`data-kfxgen-*`: original toc/front-matter classes and `align`, `:link`/`:visited` colour,
user-agent vs book margins in vertical text). Where Deckle has the unflattened CSS it can read
these directly; the result must equal what kfxgen sees.

## Phases

Estimates are my judgement from the code size, not measured.

| Phase | Delivers | Size |
|---|---|---|
| 1. Generator | `model.json` → KFX, byte-identical to kfxgen on every fixture; math reported unsupported | 1–2 weeks |
| 2. EPUB + cascade | spine/manifest/TOC reader; computed styles equal to Calibre's Stylizer on fixtures | 1–2 weeks |
| 3. Converter | DOM + styles → chapter model, in feature slices (text, headings/TOC, links/notes, lists, images, tables, boxes, fonts, vertical/RTL) | 3–5 weeks |
| 4. Integration | converter setting, queue, finish path, CLI/app/Docker/docs changes; native opt-in ("built-in converter, beta") | about 1 week |
| 5. Default | native becomes the default after device sign-off; Previewer optional, for fixed layout | days |

Each phase gets its own plan in `docs/superpowers/plans/` when the previous one lands.

## Integration (phase 4)

- **Setting:** `kindle_converter = auto | native | previewer` in `RunOptions` and app settings.
  `auto`: native for reflowable books; Previewer for fixed layout when found; fixed layout
  without Previewer keeps today's error. Phase 4 ships with `previewer` as the default when
  Previewer is found and `native` otherwise; phase 5 switches the default to `auto`.
- **Queue:** new entries store an unstamped `book.kfx`. Existing `book.kpf` entries still
  finish through `kpf_to_kfx`. `is_mobi` sniffing for fixed layout stays.
- **Finish:** a new `deckle_kfx::convert::kfx_to_kfx(bytes, &ConvertOptions)` built from the
  same steps as `kpf_to_kfx` after the KPF-only ones: needs `Book::from_kfx`.
- **Page list:** an EPUB's own page-list wins. Otherwise the generator writes none and Deckle's
  `add_approximate_pages` applies `--pages` as today.
- **Warnings:** the converter's warnings (e.g. tables flattened) become run warnings prefixed
  `convert:`, in place of `previewer:` ones.
- **Gates:** CLI `run`, `digest`, inbox, cookbook rebuilds, morgue send, reprocess and
  `kindle_upgrade` stop requiring Previewer for reflowable books. The run lock that exists
  because two Previewer runs at once are unsafe (`deckle-cli/src/lib.rs:2861`) is not needed for
  native runs. The app's Sending card describes Previewer as optional, for fixed-layout books.
  Docker and Linux docs list Kindle conversion as available.
- **Normalize:** `normalize.rs` was written for Previewer. Phase 4 lists each Previewer-specific
  step and keeps, changes or skips it for native, with a test per decision.

## Unsupported content

Until math (and anything else found in phase 3) is ported, the converter returns a typed
"unsupported" result naming the feature. With `auto`: use Previewer if found; otherwise convert
natively with that element as its text/alt text and a run warning naming it.

## Keeping up with kfxgen

kfxgen is still changing (5.24.0 shipped on 2026-11-11, with #753 to #760 after it). The port
targets `0bc705d`. A sync is its own task: run `KFXGEN_REF=<new> tools/ref/kfxgen_models.sh` (it
exports that commit and injects Deckle's dump hook; kfxgen itself is only read), list
`git log 0bc705d..<new>` in kfxgen for files under `plugin/kfxgen/`, port each change, and set
the new commit in the script, the fixtures README and `crates/deckle-kfxgen/src/lib.rs`. The
model JSON carries a `schema` number; a kfxgen change to the model bumps it.

## Risks

- **Calibre's input pipeline does things not visible in kfxgen's source** (EPUB input plugin,
  splitting, cover handling). The converter oracle surfaces them; each becomes either port work
  or a `DIVERGENCES.md` entry.
- **Container framing.** `write_kfx` is byte-exact with full kfxlib; kfxgen's
  `kfxlib_minimal` may frame differently. Phase 1 task 1 checks this first.
- **`rebuild` on a Book that never was a KPF** may drop or reorder fragments. Phase 4 tests it.
- **Images.** kfxgen resizes and converts images with Calibre's Qt code; Rust's `image` crate
  won't give the same bytes. Image bytes are compared by dimensions and format, not bytes, at
  the converter and end-to-end levels.

## Left to the builder

Internal module layout and types inside `deckle-kfxgen`; which WOFF2 and SVG crates (within the
constraints); how phase 3 orders its feature slices after text; fixture sizes, as long as the
committed fixtures stay small (target under 10 MB total) and the 9-book corpus stays outside
git behind an opt-in environment variable.
