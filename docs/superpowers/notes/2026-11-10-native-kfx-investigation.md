# Replacing Kindle Previewer with kfxgen: investigation notes

Date: 2026-11-10. Status: **design and phase 1 plan written; nothing built; nothing committed.**

## Where things stand

| Item | Path | State |
|---|---|---|
| Design (all phases) | `docs/superpowers/specs/2026-11-10-native-kfx-design.md` | Draft, waiting for your review |
| Phase 1 plan (generator) | `docs/superpowers/plans/2026-11-10-native-kfx-phase1-generator.md` | Draft, waiting for your review |
| These notes | `docs/superpowers/notes/2026-11-10-native-kfx-investigation.md` | — |

**Next action:** read the design's "Goals" and "Non-goals", then list what's wrong or missing.

## The question

Can Deckle drop Kindle Previewer and use kfxgen (`/Users/justin/GitHub/kfxgen-public`) instead?

**Why (your words):** Previewer is slow, it can't be used in a container, and it needs a
separate download.

## Measured speed (from `gutenberg/`)

| Book | Size | Previewer (s) | kfxgen via Calibre (s) |
|---|---|---|---|
| pg11 | 0.2 MB | 36.29 | 2.91 |
| pg23 | 0.4 MB | 38.87 | 1.46 |
| pg43 | 0.3 MB | 28.18 | 1.56 |
| pg55 | 0.3 MB | 41.18 | 3.19 |
| pg74 | 16.7 MB | 68.74 | 4.81 |
| pg76 | 16.0 MB | 79.78 | 7.74 |
| pg84 | 0.5 MB | 47.99 | 2.61 |
| pg86 | 26.1 MB | 81.10 | 6.54 |
| pg98 | 7.9 MB | 76.76 | 8.20 |
| **Total** | | **499.10** | **39.22** |

Sources: `gutenberg/kfx/timings.csv`, `gutenberg/kfxgen/timings.csv`.

## How Deckle uses Previewer today

Deckle makes a Kindle book in two halves:

1. **EPUB → KFX content: Previewer.** Previewer turns HTML and CSS into storylines (`$259`),
   styles (`$157`), sections, images, tables and navigation, and saves them as a KPF.
   Deckle has no code for this step: its EPUB code edits XHTML as text, with no DOM and no CSS
   handling.
2. **KPF → sideloadable KFX: Deckle (`crates/deckle-kfx`).** A Rust port of kfxlib's fix-ups.
   It adds position and location maps, stamps the ASIN and PDOC/EBOK, adds the page list, and
   writes the monolithic KFX.

kfxgen replaces half 1. Half 2's tools (Ion, symbol table, container, maps, page list,
metadata, rebuild) are generic and can be reused.

Where Previewer is wired in:
- `crates/deckle-core/src/previewer.rs`: locate (Mac and Windows paths only), run, parse logs.
- `crates/deckle-core/src/pipeline.rs`: `prepare` runs Previewer (:404); `finish_with` calls
  `kpf_to_kfx` (:791); fixed layout takes a MOBI route (:761).
- Outbox stores `book.kpf` (`queue.rs`).
- CLI gates on Previewer: `run`, `digest`, inbox, cookbook rebuilds, morgue send, reprocess,
  `kindle_upgrade`, and a run lock (`deckle-cli/src/lib.rs:2861`).
- App: Sending settings card, `PREVIEWER_REASON` gate, run blocking (`App.tsx:502-510`).
- Docker: no Previewer, so no Kindle conversion (`docs/docker.md:17-18`).

What reads Deckle's KFX afterwards (must keep working): cover thumbnail, X-Ray and Word Wise
(`text::content_runs`, which needs `$145`, `$259`, `$265`), `section_first_runs` (`$264`),
inventory, readership.

## What kfxgen is

- A Python Calibre output plugin, version 5.24.0, commit `4b81651`, GPL-3. The licenses are
  compatible with Deckle's.
- `converter.py` (9.5k lines): Calibre OEB book → chapter model.
- `native_generator.py` (8.2k lines): chapter model → KFX fragments.
- `kfxlib_minimal/` (6.3k lines): Ion and container serialization.
- Vendored math (ziamath, ziafont, latex2mathml): about 13k lines.
- **Depends on Calibre:** Stylizer (the CSS cascade, about 70 calls), Calibre's CSS flattening
  (with a `specialize_css_for_output` hook that records facts the flattening would lose), the
  OEB model, and Qt image code.
- Fixed layout is detected only, not laid out.
- Output is deterministic: ids come from SHA-256 of the metadata.
- Test assets: 16 byte-exact golden KFX files, built without Calibre via `tests/fixtures/oeb_shim.py`;
  about 98 `test_books/` directories; the 90-book Gutenberg baseline; `tools/kfxdiff.py` and
  `tools/kfxanalyze.py`.

## Options considered

| Option | What | Estimate | Outcome |
|---|---|---|---|
| A | Run `ebook-convert` with the kfxgen plugin as a subprocess; re-stamp the KFX in Rust | 3–5 days | Not chosen: users would need Calibre installed |
| B | A kfxgen entry point that doesn't need Calibre | First 2–4 weeks, **revised to 4–8 weeks** | Not chosen: it needs its own cascade and flattener, and Deckle would have to bundle Python |
| **C** | **Port kfxgen to Rust inside Deckle; the Calibre plugin is the test reference** | 6–10 weeks across 5 phases | **Chosen** |

B's estimate went up because kfxgen depends on Calibre's CSS cascade and flattening, not only
on the OEB object model.

## The chosen plan (option C)

| Phase | Delivers | Estimate |
|---|---|---|
| 1. Generator | `model.json` → KFX, byte-identical to kfxgen | 1–2 weeks |
| 2. EPUB + CSS | computed styles equal to Calibre's Stylizer | 1–2 weeks |
| 3. Converter | EPUB → chapter model, by feature | 3–5 weeks |
| 4. Integration | setting, outbox, finish path, CLI, app, Docker, docs; native is an opt-in beta | about 1 week |
| 5. Default | native becomes the default after device sign-off | days |

All estimates are inferred from code size, not measured.

**How correctness is checked:** a dump hook in kfxgen writes `generate_full_book`'s input as
`model.json`. Python and Rust both generate from it, and the outputs must match byte for byte.
Later phases compare computed styles and chapter models the same way, then run end to end on
the 9-book corpus, then you test on a Kindle.

## Calls I made that you should review

1. Fixed-layout books and MathML stay on Previewer for now.
2. Native is opt-in until phase 5. Users who have Previewer keep it as their default until
   you've tested on a device.
3. Speed target: under 2 s per text-only book, and under 39 s total for the corpus.
4. Phase 1 adds a dump hook to kfxgen-public, on a branch.
5. One new crate, `crates/deckle-kfxgen`, which reuses `deckle-kfx`. No second Ion or container
   writer.
6. Every deliberate difference from kfxgen is recorded in `crates/deckle-kfxgen/DIVERGENCES.md`.
   The pinned upstream commit is recorded in `UPSTREAM.md`.

## Risks found

- Calibre's input pipeline may change books in ways kfxgen's source doesn't show.
- kfxgen's container framing may differ from Deckle's `write_kfx`. This is phase 1, task 1.
- `rebuild` has only ever run on a book that started as a KPF.
- Image bytes won't match: Qt does the image work in kfxgen, the `image` crate in Rust.
- Calibre has two `.kfx` output plugins installed (kfxgen and "KFX Output"). The fixture script
  must confirm that kfxgen ran.
- kfxgen keeps changing. Re-pinned on 2026-11-11 from `cf6cf44` (5.23.0) to `4b81651` (5.24.0 plus #753) before any porting. Later syncs are separate tasks.

## Not yet done

- No code written, in either repo.
- Nothing committed.
- No builder-check review of the design.
- Plans for phases 2–5 come after phase 1 lands.
