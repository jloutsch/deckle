# Native KFX, Phase 1: the generator — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust port of kfxgen's generator (`native_generator.py`) that turns a chapter model into a KFX, byte-identical to kfxgen's own output on every committed fixture.

**Architecture:** kfxgen gets a dump hook that writes the arguments of `generate_full_book` as `model.json` plus resource files. The new crate `deckle-kfxgen` loads that model, builds fragments, and writes them with `deckle-kfx`'s existing container writer. Each slice of the port is tested by comparing its fragments with kfxgen's KFX for the same model.

**Tech Stack:** Rust 2024 workspace (`deckle-kfx`, serde, serde_json, sha2; `indexmap` 2 as a new direct dependency, already in `Cargo.lock` transitively); Python 3.13 in `/Users/justin/GitHub/kfxgen-public` for the dump hook; Calibre 9 `ebook-convert` with the kfxgen plugin, to make fixtures.

**Spec:** `docs/superpowers/specs/2026-11-10-native-kfx-design.md` (Phase 1, Oracles, Constraints). Phases 2–5 get their own plans.

## Global Constraints

- Upstream pinned at kfxgen 5.24.0, commit `4b81651`. Port that code, not newer code.
- GPL-3.0-or-later SPDX header on every new source file, Rust and Python.
- Reuse `deckle-kfx` (Ion, `SymbolTable`, `Fragment`/`FragmentList`, `write_kfx`). No second Ion encoder or container writer.
- Deterministic: no `HashMap`/`HashSet` iteration anywhere output order can depend on it. Use `Vec`, `BTreeMap` or `IndexMap`.
- Python indexes strings by code point. Every offset, length and chunk boundary in the port counts `char`s, never bytes or UTF-16 units.
- Math is out of scope: a model containing a math span is an error, not a silent drop.
- Model JSON `schema` is `1`. A key the loader doesn't know is an error that names the key.
- Fixtures committed to Deckle stay under 10 MB total. The 9-book Gutenberg corpus stays outside git (`DECKLE_KFXGEN_CORPUS`).
- Two repos: Deckle work on branch `feat/native-kfx-generator`; kfxgen work on branch `feat/model-dump` in kfxgen-public. Never commit to `main` in either. Commit per task.
- Short comments that point to the kfxgen source they port (`native_generator.py:<line>` at `4b81651`), matching the repo's comment style.

## Review Focus

1. **Text outside the Basic Multilingual Plane, combining marks and CJK.** Python counts code points; a byte- or UTF-16-based port shifts every span and position after the first such character. `unicode_text_matches_kfxgen` in Task 6.
2. **A chapter with no text** (empty, or a single image). Expected: same sections and positions as kfxgen, no panic. `empty_chapter_matches_kfxgen` in Task 6, `image_only_chapter_matches_kfxgen` in Task 8.
3. **A model dumped by a newer kfxgen** with a key or block type the port doesn't know. Expected: load fails with the key name and the schema number, never a silent default. `unknown_model_key_is_an_error` in Task 4.
4. **Order taken from a hash set or dict leaks into output** (style dedup, resource names, symbol order). Expected: two runs produce identical bytes, and both equal kfxgen's. `generate_is_deterministic` in Task 13; byte oracles in every task.
5. **A math span in the model.** Expected: `GenerateError::Unsupported("math")`, no KFX. `math_span_is_reported_unsupported` in Task 7.

---

### Task 1: Deckle reads kfxgen's KFX

Checks the design's container-framing risk first, before anything is ported.

**Files:**
- Create: `fixtures/kfxgen/golden/{minimal,with_cover,body_images,table_cells}.kfx` (copied from kfxgen-public `tests/fixtures/golden/expected/` at `4b81651`)
- Create: `fixtures/kfxgen/README.md` (source and commit of every file)
- Modify: `crates/deckle-kfx/src/fixtures.rs` (add `kfxgen(name: &str) -> PathBuf` for `fixtures/kfxgen/`)
- Create: `crates/deckle-kfx/tests/it/kfxgen_input.rs`; Modify: `crates/deckle-kfx/tests/it/main.rs`
- Create: `crates/deckle-core/tests/it/kfxgen_cover.rs`; Modify: that crate's `tests/it/main.rs`

**Interfaces:**
- Produces: `deckle_kfx::fixtures::kfxgen(name) -> PathBuf`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn kfxgen_goldens_round_trip_byte_exact() {
    for name in ["golden/minimal.kfx", "golden/with_cover.kfx", "golden/body_images.kfx", "golden/table_cells.kfx"] {
        let bytes = std::fs::read(deckle_kfx::fixtures::kfxgen(name)).unwrap();
        let doc = read_kfx(&bytes).unwrap();
        let out = write_kfx(&doc, &WriteOptions::from_doc(&doc)).unwrap();
        assert_eq!(out, bytes, "{name}");
    }
}

#[test]
fn kfxgen_golden_has_content_runs() {
    let bytes = std::fs::read(deckle_kfx::fixtures::kfxgen("golden/minimal.kfx")).unwrap();
    let (runs, end) = deckle_kfx::text::content_runs(&bytes).unwrap();
    assert!(!runs.is_empty());
    assert!(end > 0);
}
```

In deckle-core: `kfxgen_golden_cover_extracts` asserts `thumbnail::cover_jpeg_from_kfx(&with_cover_bytes).is_some()`.

- [ ] **Step 2: Run** `cargo test -p deckle-kfx --test it kfxgen_` and `cargo test -p deckle-core --test it kfxgen_`. Expected: FAIL on the missing `fixtures::kfxgen`.
- [ ] **Step 3: Add `fixtures::kfxgen` and copy the fixtures.** If the round trip differs, find the first differing byte. A framing difference (container info fields, `kfxgen_info` trailer, entity order) is fixed by extending `WriteOptions` with the field kfxgen uses, defaulting to today's behaviour so `kpf_to_kfx_oracle` stays green. Never add a second writer. Record what you found in `fixtures/kfxgen/README.md`.
- [ ] **Step 4: Run** both test commands plus `cargo test -p deckle-kfx --test it kpf_to_kfx_oracle`. Expected: all PASS.
- [ ] **Step 5: Commit** `test(kfx): read and re-write kfxgen's KFX byte for byte`

---

### Task 2: kfxgen dumps and replays its generator input (kfxgen-public repo)

**Files (all in `/Users/justin/GitHub/kfxgen-public`):**
- Create: `plugin/kfxgen/model_dump.py`
- Modify: `plugin/kfxgen/native_generator.py` (at the top of `generate_full_book`, :4549)
- Create: `research/generate_from_model.py`
- Create: `tests/unit/test_model_dump.py`

**Interfaces:**
- Produces: env var `KFXGEN_DUMP_MODEL=<dir>`. When set, `generate_full_book` writes `<dir>/model.json` and `<dir>/resources/<sha256>` before generating. Output is unchanged.
- Produces: `model_dump.dump(dir, kwargs: dict) -> None`, `model_dump.load(dir) -> dict` (kwargs ready for `generate_full_book`).
- Produces: `python3 research/generate_from_model.py <model_dir> <out.kfx>`.
- Produces: the JSON encoding the Rust loader (Task 4) reads:

| Python value | JSON |
|---|---|
| `None`, `bool`, `int`, `str` | native (a `str` may contain `\u0000`, from image tokens) |
| `float` | JSON number from `repr()` (round-trips exactly) |
| `list` | array |
| `tuple` | `{"$tuple": [...]}` |
| `set`, `frozenset` | `{"$set": [...]}`, items sorted by their encoded JSON text |
| `dict` | `{"$dict": [[key, value], ...]}` in insertion order (keys may be non-strings) |
| `bytes` | `{"$bytes": "<sha256 hex>"}`; bytes in `resources/<sha256 hex>` |
| any other object | `{"$obj": "<module>.<Class>", "state": <encoded vars(obj)>}` |

Top level: `{"schema": 1, "kfxgen_version": "5.24.0", "kfxgen_commit": "<git rev-parse --short HEAD, or 'unknown'>", "args": <encoded kwargs except output_path>}`.

- [ ] **Step 1: Write the failing test** `test_dump_then_generate_is_byte_identical`: for every entry in `tests/fixtures/golden/inputs.py` `GOLDEN_INPUTS`, build the EPUB, convert it with `KFXGEN_DUMP_MODEL` set (as `regenerate.py` does), then `generate_full_book(**load(dir), output_path=...)`, and assert the replayed bytes equal the direct bytes. Plus `test_dump_encodes_tuple_set_bytes_and_objects` with one value of each kind and an assertion on `load(dump(x)) == x`, types included.
- [ ] **Step 2: Run** `pytest tests/unit/test_model_dump.py -v`. Expected: FAIL, no module `model_dump`.
- [ ] **Step 3: Implement** `dump`/`load` and the hook. `load` rebuilds `$obj` with `cls.__new__(cls)` plus `__dict__.update`.
- [ ] **Step 4: Run** `pytest tests/unit/test_model_dump.py -v`, then the full `pytest -m "tier1 or tier3_strict"`. Expected: PASS, and existing goldens unchanged.
- [ ] **Step 5: Commit** `feat: dump and replay generate_full_book's input (KFXGEN_DUMP_MODEL)`

---

### Task 3: The fixture set

**Files:**
- Create: `tools/ref/kfxgen_models.sh` (Deckle repo)
- Create: `fixtures/kfxgen/models/<name>/{model.json,resources/*,expected.kfx}`
- Modify: `fixtures/kfxgen/README.md` (a table: fixture name → source → features it exercises)

**Interfaces:**
- Produces: every fixture dir holds `model.json`, `resources/` and `expected.kfx` (kfxgen's output for that model, made with `research/generate_from_model.py`).
- Produces: fixture names used by later tasks: the 16 golden inputs under their golden names; `unicode_text`; `empty_chapter`; and at least one Calibre-path fixture marked in the README for each of: `boxes`, `rules_spaces`, `lists`, `notes`, `fonts`, `vertical`, `rtl`, `ruby_tcy`, `page_list`, `links`.

- [ ] **Step 1: Write the script.** It (a) runs kfxgen's golden inputs through `EpubAsOeb` with the dump set; (b) builds `unicode_text` (one chapter with `é`, `é`, `漢字`, `😀`, `𝔘` and an italic span after them) and `empty_chapter` (chapter 2 of 3 with no body text) with kfxgen's `epub_builder`; (c) runs `ebook-convert <src>.epub out.kfx` with `KFXGEN_DUMP_MODEL` on small source EPUBs from kfxgen-public `test_books/*/` for each feature in the list above; (d) writes `expected.kfx` for each by replaying the model. It must exit non-zero when `ebook-convert` produced no `model.json`: that means Calibre ran its other `.kfx` output plugin ("KFX Output", which uses Previewer), not kfxgen.
- [ ] **Step 2: Run** `tools/ref/kfxgen_models.sh`. Expected: every listed fixture dir exists; `du -sh fixtures/kfxgen` under 10 MB. Prefer smaller source books over trimming fixtures by hand. Embedded fonts in a fixture must be freely licensed; note the license in the README.
- [ ] **Step 3: Run the 9-book corpus outside git:** `DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub`. Expected: 9 model dirs.
- [ ] **Step 4: Commit** `test(kfxgen): fixture models and kfxgen's KFX for each` (fixtures and script only; not `gutenberg/`).

---

### Task 4: `deckle-kfxgen` crate, model and loader

**Files:**
- Create: `crates/deckle-kfxgen/{Cargo.toml, src/lib.rs, src/model.rs, src/pyvalue.rs, src/load.rs, UPSTREAM.md, DIVERGENCES.md}`
- Create: `crates/deckle-kfxgen/tests/it/{main.rs, load.rs, oracle.rs}`
- Modify: workspace `Cargo.toml` (`deckle-kfxgen` in `[workspace.dependencies]`)

**Interfaces:**
- Produces: `pyvalue::PyValue` enum, one variant per row of the Task 2 table (`None, Bool, Int(i64), Float(f64), Str(String), List(Vec<PyValue>), Tuple(Vec<PyValue>), Set(Vec<PyValue>), Dict(Vec<(PyValue, PyValue)>), Bytes(Arc<[u8]>), Obj { class: String, state: Box<PyValue> }`), and `PyValue::from_json(&serde_json::Value, resources: &Path) -> Result<PyValue, LoadError>`.
- Produces: `model::Model { schema: u32, kfxgen_version: String, kfxgen_commit: String, args: GenerateArgs }`, where `GenerateArgs` has one typed field per `generate_full_book` parameter. `Chapter`, `Block` (enum: `Paragraph`, `Box`, `Rule`, `Space`, `Table`), `Span { start: usize, len: usize, flags: Vec<Flag> }`, `Table`/`Row`/`Cell`, `FontTable` hold the keys the inventory lists (spec, Architecture). A `TryFrom<&PyValue>` per type rejects unknown keys.
- Produces: `load::load_model(dir: &Path) -> Result<Model, LoadError>`; `LoadError::UnknownKey { path: String, key: String, schema: u32 }`.
- Produces (test helper, `tests/it/oracle.rs`): `assert_fragments_match(actual: &FragmentList, expected_kfx: &Path, ftypes: &[&str])`. It compares fragments of those types by (ftype, fid, Ion value) and, on failure, prints both sides as Ion text for the first mismatch.

- [ ] **Step 1: Write the failing tests:** `every_fixture_model_loads` (all dirs under `fixtures/kfxgen/models`); `unknown_model_key_is_an_error` (a model with an extra paragraph key `"zzz"`, asserting `LoadError::UnknownKey { key: "zzz", schema: 1, .. }`); `pyvalue_round_trips_every_tag` (each tag from JSON and back).
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it load`. Expected: FAIL, crate missing.
- [ ] **Step 3: Implement.** `UPSTREAM.md` records `4b81651` and how fixtures are regenerated (Task 3 script). `DIVERGENCES.md` starts empty with its format: kfxgen location, what differs, why.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(kfxgen): crate, chapter model and model.json loader`

---

### Tasks 5–12: Port the generator

**Shared by Tasks 5–12** (each task's own section adds what it ports, its fixtures and the fragment types compared):

- **Files:** `crates/deckle-kfxgen/src/generate/<module>.rs`; tests in `crates/deckle-kfxgen/tests/it/generate_<module>.rs`, registered in `tests/it/main.rs`.
- **Interfaces:** `deckle_kfxgen::load::{load_model, Model}` (Task 4; `Model.args` is a `PyValue` dict read like kfxgen reads its dicts: `PyValue::get`, absent ≠ `None`). Task 5 creates the entry point `deckle_kfxgen::generate(model: &Model) -> Result<GenerateOutput, GenerateError>`, `GenerateOutput { bytes: Vec<u8>, fragments: FragmentList, warnings: Vec<String> }`, `GenerateError::{Unsupported(&'static str), Model(String) /* path + what was expected */, Container(deckle_kfx::container::ContainerError)}`, and one builder `generate::Generator` holding a `deckle_kfx` `FragmentList` and `SymbolTable`; later tasks add to it. Bytes are written with `deckle_kfx::container::write_kfx` (Task 1: kfxgen's framing round-trips through it). Test helper: `tests/it/oracle.rs` `assert_fragments_match(actual, expected_kfx, ftypes)`.
- **Steps:** (1) write `<fixture>_matches_kfxgen` per listed fixture: `assert_fragments_match(&generate(&load_model(&dir)?)?.fragments, &dir.join("expected.kfx"), &[<ftypes>])`; (2) run `cargo test -p deckle-kfxgen --test it generate_<module>` — Expected: FAIL; (3) port the listed Python from kfxgen `4b81651` (`git -C ../kfxgen-public show 4b81651:plugin/kfxgen/native_generator.py`), citing source lines in comments; a deliberate difference goes in `crates/deckle-kfxgen/DIVERGENCES.md` (create it with the first one); a byte oracle that can't pass without one is a stop-and-ask, not a test edit; (4) run the command again and `cargo test -p deckle-kfxgen --test it` — Expected: PASS, earlier tasks still green; (5) `cargo clippy -p deckle-kfxgen --all-targets -- -D warnings`, `cargo fmt`, commit `feat(kfxgen): <slice>`.

### Task 5: The `minimal` book end to end

Merged from the old Tasks 5 and 6 (ledger ruling): the container fragments name sections, resources and symbols that content generation creates, so `minimal` end to end is the first slice that can pass.

- **Module:** `skeleton`, `metadata`, `text`, `positions`, `nav`
- **Python to port** (`native_generator.py` unless noted): `generate_full_book` (:4549–5220: ids — SHA-256 container id/ASIN at :4638–4659 — symbol creation, fragment assembly); `$490` (:2859), `$258` (:2983), `$538` (:3011), `$585` (:2161), `$593`, `$270` (:3487), `$419` (:3523), `$ion_symbol_table` (:3469); `_build_chapter_content` (:5396) paragraph/text path only; `$259` (:4199), `$145` (:3545), `$157` (:3568) paragraph styles, `$260` (:4500); `_build_position_data` (:3290), `$264` (:3239), `$265` (:3261), `$550`; `build_fragment_389_toc` (:3066), `$395`. Raise `GenerateError::Unsupported("<feature>")` for any block type, span flag or argument this task doesn't port yet, so later fixtures fail loudly, not wrongly.
- **Fixtures:** `minimal`, `multi_chapter`, `long_chapter`, `unicode_text`, `empty_chapter`, `publisher_structure`
- **Fragment types compared:** every type in the expected KFX; plus `minimal_is_byte_identical`: `generate(minimal).bytes == expected.kfx`
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 6: Inline styles, links and anchors

- **Module:** `inline`
- **Python to port** (`native_generator.py` unless noted): span flags → inline `$157` runs (`build_fragment_157` :3568); link and anchor targets, `$266` (:4144, :4155); heading/title de-duplication; `GenerateError::Unsupported("math")` on a math flag
- **Fixtures:** `sub_super_marks`, `linked_toc`, `marker_offsets`, `links`; plus `math_span_is_reported_unsupported` (a `Model` from `minimal` with one span given a math flag)
- **Fragment types compared:** `$157 $259 $266 $145`
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 7: Images and the cover

- **Module:** `images`
- **Python to port** (`native_generator.py` unless noted): `$164` (:2741)/`$417` (:2801) resources, nested `$259:$271` image entries, cover, floats, `display_dims`, `resolved_image_refs`
- **Fixtures:** `body_images`, `captioned_images`, `with_cover`, `unused_manifest_image`, `contents_illustration`; plus `image_only_chapter_matches_kfxgen` (needs an `image_only_chapter` fixture: add it to `tools/ref/kfxgen_models.py` shim inputs and rerun the script)
- **Fragment types compared:** `$164 $417 $259 $157`
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 8: Tables

- **Module:** `tables`
- **Python to port** (`native_generator.py` unless noted): `_emit_table_chunks` (:5993), `_cell_style` (:7326), `build_table_style_157` (:3808), `build_cell_style_157` (:4015)
- **Fixtures:** `table_cells`, `table_layout`, `table_paragraph_cells`
- **Fragment types compared:** `$259 $157 $145`
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 9: Boxes, rules, spaces, lists and notes

- **Module:** `boxes`, `notes`
- **Python to port** (`native_generator.py` unless noted): `_make_box` (:6688), `build_box_style_157` (:3853), `build_rule_style_157` (:3915), spaces, `_space_between`; list items; notes tables (:3360) and pop-up targets
- **Fixtures:** `boxes`, `rules_spaces`, `spacers`, `lists`, `notes`
- **Fragment types compared:** `$259 $157 $145 $266`
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 10: Embedded fonts

- **Module:** `fonts`
- **Python to port** (`native_generator.py` unless noted): `$262` (:2830)/`$418` (:2820) from `FontTable`; font-family mapping in `$157`
- **Fixtures:** `fonts`
- **Fragment types compared:** `$262 $418 $157`
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 11: Page list and landmarks

- **Module:** `pages`
- **Python to port** (`native_generator.py` unless noted): `place_pages` (:1642), `page_list`, `landmarks`, `approximate_pages`
- **Fixtures:** `page_list`
- **Fragment types compared:** all
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 12: Vertical text, right to left, ruby and tate-chu-yoko

Last on purpose (ledger ruling): kfxgen's vertical-text work (#247) is still landing. **Step 0:** re-pin to kfxgen `main` (merge kfxgen `feat/model-dump` first), rerun `tools/ref/kfxgen_models.sh`, port the generator diff `git -C ../kfxgen-public diff 4b81651..<new> -- plugin/kfxgen/native_generator.py` into Tasks 5–11's modules with every earlier fixture still byte-identical, and update the pin in `crates/deckle-kfxgen/src/lib.rs`, `fixtures/kfxgen/README.md` and the spec.

- **Module:** `writing`
- **Python to port** (`native_generator.py` unless noted): `writing_mode`, `rtl`, `page_bidi`, ruby (`$756` :2342, ruby style :2328), tate-chu-yoko, emphasis marks, bidi flags, direction styles (:2321)
- **Fixtures:** `vertical`, `rtl`, `ruby_tcy`
- **Fragment types compared:** all
- **Steps:** as shared above; the module's tests are `generate_<module>`.

---

### Task 13: Whole-book equality, corpus and timing

**Files:**
- Create: `crates/deckle-kfxgen/tests/it/{whole_book.rs, corpus.rs}`
- Create: `crates/deckle-kfxgen/examples/from_model.rs`

**Interfaces:**
- Consumes: `generate`, `load_model`, `assert_fragments_match`.
- Produces: `cargo run --release -p deckle-kfxgen --example from_model -- <model_dir> <out.kfx>`, which prints `<ms> ms`.

- [ ] **Step 1: Write the failing tests:**
  - `every_fixture_is_byte_identical`: for every dir under `fixtures/kfxgen/models`, `generate(..).bytes == expected.kfx`.
  - `generate_is_deterministic`: `generate` twice on `long_chapter` gives equal bytes.
  - `corpus_is_byte_identical`: `#[ignore]` unless `DECKLE_KFXGEN_CORPUS` is set; the same check over every model dir in it.
- [ ] **Step 2: Run** `cargo test -p deckle-kfxgen --test it`. Expected: FAIL on any fixture not yet equal.
- [ ] **Step 3: Fix the remaining differences.** The fixes belong in the module that owns the fragment.
- [ ] **Step 4: Run** `cargo test -p deckle-kfxgen --test it`, then `DECKLE_KFXGEN_CORPUS=gutenberg/models cargo test -p deckle-kfxgen --test it corpus -- --ignored`. Expected: all PASS, 9/9 corpus books byte-identical.
- [ ] **Step 5: Time it:** run `from_model` in release mode on each corpus model and write the per-book ms into `fixtures/kfxgen/README.md` under "Phase 1 timings", next to kfxgen's numbers from `gutenberg/kfxgen/timings.csv`. These are the generator's numbers only. The whole-pipeline speed goal is checked in phase 3.
- [ ] **Step 6: Run** `cargo clippy -p deckle-kfxgen --all-targets -- -D warnings` and `cargo test --workspace`. Expected: clean and PASS.
- [ ] **Step 7: Commit** `feat(kfxgen): every fixture and the corpus byte-identical with kfxgen`

---

## Done when

- Every committed fixture and all 9 corpus books produce KFX byte-identical to the pinned kfxgen commit (re-pinned in Task 12).
- A model with math fails with `Unsupported("math")`.
- `DIVERGENCES.md` is empty or every entry has a reason.
- Nothing in Deckle's pipeline uses the new crate yet. That is phase 4.
