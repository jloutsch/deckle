# kfxgen book-level converter: map for the phase 3 port

Pinned sources: `p3/src/converter.py` (all line numbers below are this file unless named).
`plugin/__init__.py` is not in `p3/src`; I read its `specialize_css_for_output`, `convert`,
`_apply_prefs` and option list with `git show 0bc705d:plugin/__init__.py` in kfxgen-public
(read-only, nothing written). `_img_tokens.py`, `page_scale.py`, `math_draw.py` are also not in
`p3/src`; the Rust side already has `page_scale.rs` (384/480 pt) and image-token handling.

Legend: **[V]** verified by reading code or data; **[I]** inference, not checked.

---

## 1. Call graph, in execution order (down to `extract_blocks_from_html`)

Sizes are lines from the `def` to the next top-level definition (comments included).

```
plugin convert (plugin/__init__.py:178)               _apply_prefs (124): per-user prefs OR'd into opts
  convert_oeb_to_kfx 9493 (135)
    _ensure_oeb_opts 9454 (19)                       oeb_book.opts = opts
    extract_metadata 7189 (57)                       title/author(s)/description/language/publisher/date
      _kfx_language 7174 (15) -> _iso639_1 7165      calibre lang_as_iso639_1 (3 -> 2 letters)
      _opf_raw_dates 7249 (14) -> _container_parser 8128   dc:date text of calibre's OPF
    extract_cover_image 9368                         [images researcher] -> (cover_image, cover_href)
    _pictures_in_table_cells 3456, extract_images_from_oeb 9192, cmyk_to_rgb, _check_and_optimize  [images]
    _fixed_layout 966 (24)                           >half of spine files carry <meta viewport width=N>
    rtl = oeb.spine.page_progression_direction == "rtl"   (9585)
    _book_writing_mode 951 (15)                      vertical-rl iff rtl and language base "ja", unless fixed layout
    with page_width(pt):                             global page width, 480 vertical / 384 otherwise
      extract_chapters_from_oeb 7830 (267)
        _book_link_targets 3483 (21)                 only if native_tables: every in-book <a href> key
        _note_pair_ids 3522 (82)                     only if oeb.auto_generated_toc
        _extract_toc_with_hrefs 9139 (53)            flatten oeb.toc to [{title, href, level}]
          _with_ncx_empty_labels 9081 (58)           put back NCX entries with blank labels
        _spine_in_order 8172 (52)                    calibre spine, added files re-sorted, case dups dropped
        _respell_toc_hrefs 8153 (19)                 TOC href -> spine spelling up to case/%-encoding
        toc_targets (7868-7876)                      fragments per file basename
        per spine file (7890-8004), try/except per file:
          _is_calibre_inline_toc 7363                skip calibre's generated inline TOC
          _drop_vertical_marks 990                   fixed layout only: remove v-author/v-ua/horizontal marks
          _build_style_resolver 1035                 [resolver; prerequisite #1]
          _note_target_ids 4790 (73) | note_pairs[file]
          extract_blocks_from_html 5763              BLACK BOX (out-params nav_listing_at, tables_seen,
                                                     notes_seen, trailing_ids)
          _file_writing_mode 933 -> b["writing_mode"] on every block
          _svg_text_paragraphs 1570                  log only
          empty file -> orphan_keys (+ _own_anchor_ids); next shown file -> _prepend_keys 1929
          trailing ids -> _drop_keys 1906, moved to orphan_keys
        last block takes leftover orphan keys (8018-8026); none shown -> raise ValueError (8028)
        if toc_entries: _assemble_chapters_by_coordinate 7517 (313)
            _anchor_block_index 7346, _normalize_href 7307 (34), _is_unsafe_href 7266 (41),
            _href_fragment 7341, _entry_key 7445, _key_doc 1898 -> _resolve_doc_path 1844,
            _section_heads 7464 (53) -> _rewritten_page 7457, _normalize_title 8338,
            _page_shows_something 1310, _leading_chapter_title 7383, _is_file_name_label 7412,
            _retitle_from_heading 7429
          -> _listing_without_contents 8097 -> _nav_listing_contents_chapter 8401
             -> if true: _with_listings_kept 8111 = extract_chapters_from_oeb(keep_nav_listings=True)  RECURSION
          -> _replace_title_page 8601 (98)
               _nav_listing_contents_chapter, page_bidi (inline_style.py:443),
               _split_text_above_listing 8464, _split_files_after_contents 8494,
               _split_front_matter_below_listing 8527, _count_text_blocks 8559,
               _rebuild_contents_page 8714 (192) -> _keep_illustrations 8582, _drop_keys,
                   _block_keys 8577, _dedupe_keep_order 4863, _is_link_flag 2152
          -> _warn_flattened_tables 8276 (59)        log only
        else / no chapters: fallback, one chapter per file (8053-8094), same two tails
    _warn_media 8243 (33) -> _has_media_overlays 8224   log only
    _split_by_writing_mode 999 (36)                  cut chapters where block writing_mode changes
    _font_table_for 9473                             [fonts researcher]
    with page_width(pt): generate_full_book(...)
      _footnote_keys 4744 (37) -> _build_style_resolver, _is_hidden 4908
      _book_landmarks_for 9039 -> _book_landmarks 8992 (47) -> _book_files 8929, _nav_links 8982, _respelled 8961
      _book_page_list 9048 (33) -> _book_files, _nav_links, _page_label 8916, _respelled
```

Before all of this, per spine file, Calibre's `CSSFlattener` calls the plugin's
`specialize_css_for_output(log, opts, item, stylizer)` with the **first-pass** Stylizer, just
before it flattens that file (plugin/__init__.py:150-176) [V]. It calls, each in its own
try/except: `mark_source_classes` 4992 (22), `mark_link_state_colours` 275 (55),
`mark_vertical_margins` 341 (88). They set `data-kfxgen-source` (root), `-toc`, `-front`,
`-align`, `-link-colour`, `-v-author`, `-v-ua`, `-horizontal`.

## 2. What the book-level code reads, and whether Deckle has it

| kfxgen reads | where | Deckle today |
|---|---|---|
| `metadata.title/creator/description/language/publisher/date` (lists, `[0]` taken; creator all) | 7208-7234 | **Have**: `Oeb.metadata` (`epub/mod.rs:89`), `Vec<String>`; language is Calibre's 3-letter code (cascade `oeb.json`: `eng`, `heb`, `jpn`) [V] |
| calibre `lang_as_iso639_1` (3 -> 2 letters) | 7165 | **Missing**. Deckle has the 2 -> 3 table (DIVERGENCES "Language codes"); invert it [I: covers the same 184 codes] |
| `container.opfname` parsed: `//metadata/*[local-name()='date']` text, all descendants' text | 7249 | **Missing as such**. `Package.metadata.date` is the book's own OPF, not Calibre's rewrite. Same text unless Calibre changed the date element [I] |
| `oeb.spine` items: `.href`, `.linear`, `.data` | everywhere | **Have**: `Oeb.spine` (`SpineRef{href, linear}`), `Book.files[i].dom` (same order) [V] |
| `oeb.spine.page_progression_direction` | 9585 | **Have**: `Oeb.page_progression_direction` [V] |
| `oeb.toc` nodes (`title`, `href`, children; top level read as level 0) | 9139 | **Have**: `Oeb.toc` (`TocNode`), filled by `read_book` with `generated_toc` when auto-generated (`book.rs:202-209`). The `Oeb.toc` doc comment saying "empty when auto_generated_toc" is stale for `Book` [V] |
| `oeb.auto_generated_toc` | 7848 | **Have**: `Oeb.auto_generated_toc` [V] |
| `container.rootdir` + `_container_parser` (raw file reads, inside the folder, no entities) | 8128 | **No equivalent API**: `Book` keeps no archive handle; `book::manifest_bytes(path, href)` (`book.rs:251`) reads from the EPUB path. `Package.opf_text`, `nav_text`, `ncx_text` hold the book's own files [V] |
| Calibre's OPF manifest (`id`, `href`) + spine `itemref/@idref` -> basenames "listed" | 8172 | **Partly**: `Package.manifest`/`spine` are the book's own OPF. Calibre's rewrite drops the HTML cover page and duplicate entries from its spine (spec `2026-11-10-native-kfx-design.md:127-134`); basenames only are compared, so folder rewriting doesn't matter [V for the code; the rewrite's content I] |
| Calibre's OPF nav item (`properties` contains `nav`), path from OPF folder | 8929 | **Have**: `PackageFiles.nav_path` and `Package.nav_text` (pub(crate)). kfxgen's key base is the nav's path relative to Calibre's OPF (the root), i.e. root-relative, as Deckle's hrefs are [V] |
| Calibre's OPF first item of type `application/x-dtbncx+xml` | 8949, 9097 | **Partly**: `Package.ncx_text` is the book's own (spine `toc` item, else first NCX item). For an EPUB 3 book with a nav Calibre adds an NCX made from the nav under a random name (spec :128); kfxgen then reads that one. **Missing**: what that generated NCX holds (pageList? blank labels?) [I: neither; needs a probe] |
| Calibre's OPF `guide/reference` (`type`, `title`, `href`), hrefs root-relative | 9019 | **Have, crate-private**: `Package.opf.guide` (`OpfRef{kind,title,href}`); public `GuideRef` and `Oeb.guide` have no title. Book's own hrefs are OPF-relative: join with the OPF folder [V] |
| `META-INF/container.xml` every rootfile, each OPF's `item[@media-overlay]` | 8224 | **Missing** (Package reads one OPF). Log only |
| kfxgen resolver: body `writing-mode`; `display` for `_is_hidden` | 933, 4908 | Base **have** (`Styles`); kfxgen's resolver is phase 3 prerequisite #1. `_is_non_rendered` is the blocks researcher's |
| Pre-flatten classes and `align=` (`mark_source_classes`) | 4992 | **Missing after read_book**: `File.dom` is post-flatten (classes renamed, `align` folded) [V `book.rs:58-60`] |
| First-pass Stylizer `rules`: `(origin…, …, cssdict, selector)`; `rule[0][0]` origin | 290, 386 | **Have**: `File.first_pass_rules`. But their selectors only match the pre-flatten tree [V] |
| First-pass `stylizer.style(e).get("writing-mode")` for body/html and every element | 371, 404 | **Missing after read_book**: the first cascade is moved into `flatten` (`book.rs:218-225`) [V] |
| calibre `css_selectors.Select(root, ignore_inappropriate_pseudo_classes=True)` | 314, 380 | Phase 2 has selector matching; whether it ignores `:link`/`:visited`/`:hover` as calibre's does is **unchecked** [I] |
| `opts.kfxgen_disable_native_tables`, `_disable_font_embedding`, `_open_at_cover`, `_disable_approximate_pages` (+ `_embed_original_images`, images) | 9580, 9476, 9044, 9621 | **Missing**: an options struct, all default false [V defaults: plugin options `recommended_value=False`] |
| `opts.output_profile` (Stylizer) | font_table.py:471 | **Have**: `DEFAULT_PROFILE` |
| `cover_href` | 9533 | images researcher |
| `page_bidi(blocks)` | 8621 | inline researcher (`inline_style.py:443`) |

**Headline gaps** (in order of how much they block):
1. The `data-kfxgen-*` marks need the pre-flatten tree and the first-pass styles, which only exist
   inside `read_book` between `cascade` and `flatten` (`book.rs:216-225`). They must be computed
   there (a phase 2 change), not from `Book`.
2. A model of Calibre's rewritten OPF (`content.opf`): dates, spine "listed" set, guide with
   titles, nav path, and which NCX (incl. the one generated from a nav). In 3 of 5 cascade
   fixtures checked (`navigation`, `vertical`, `notes`) the book's OPF *is* root `content.opf`
   and Calibre overwrote it (`oeb.json` `raw.sha256: null`) [V].
3. kfxgen's spine reorder (`_spine_in_order`) on top of `Oeb.spine`.
4. 3 -> 2 letter language table; options struct; raw-file reads from the archive.

## 3. What this layer produces

### Arguments of `generate_full_book` (9599-9631) [V]

| arg | value |
|---|---|
| `title` | `metadata.title[0]`, default `"Untitled"` |
| `author` | `", ".join(creator)`, default `"Unknown"` |
| `authors` | `[str(c) for c in creator]`, **None** (key absent) with no creator |
| `description` | `description[0]` or None (raw HTML string, e.g. `<p>A test… <b>HTML</b>…</p>`) |
| `language` | `_kfx_language(language[0])`, default `"en"`; `und` -> `en`; 3 letters -> 2 where known; rest kept |
| `publisher` | `publisher[0]`, default `"kfxgen"` |
| `issue_date` | only if Calibre has a date: first raw OPF `date` text, stripped, `[:10]` if it starts `YYYY-MM-DD`, else None; if the OPF can't be read, Calibre's `date[0][:10]` |
| `rtl` | spine ppd == `"rtl"` |
| `writing_mode` | `"horizontal-tb"` if fixed layout, else `_book_writing_mode(language, rtl)` |
| `footnote_keys` | `set` of `"<file>#<id>"` for every id on/in an `epub:type~=footnote` element not hidden (self or ancestor) |
| `landmarks` | list of tuples `(kind, label, key)`, kind `toc`/`start`; nav `landmarks` first, else guide (`toc`, `text`); only the last `toc` kept; all `start`s kept; `start` removed if `kfxgen_open_at_cover`; keys respelled to spine case |
| `page_list` | list of tuples `(label, key)`; nav `page-list`, else NCX `pageList`; labels cleaned by `_page_label`; **empty list if any label isn't all digits or roman letters** |
| `approximate_pages` | `opts.kfxgen_disable_approximate_pages is not True` -> True by default |
| `resolved_image_refs` | always True |
| `chapters` | below |
| `cover_image`, `images`, `display_dims`, `font_table` | other layers; `asin` is never passed (None) |

Cross-check against the 74 models [V]: all 20 argument names present in every model; all 28
source-built models have `approximate_pages: true`, `publisher: "kfxgen"`, `authors` set.
`footnote_keys` is non-empty only in `notes` (`{"$set": ["a.xhtml#an1", "d.xhtml#dn1",
"e.xhtml#en1", "notes.xhtml#bn1"]}`); `landmarks` and `page_list` non-empty only in `page_list`
(`[("toc","Contents","contents.xhtml"), ("start","Start","c1.xhtml")]`, `[("i","front.xhtml#pg_i"), …]`);
`issue_date`/`description` only in `navigation`; vertical in `horizontal_blocks`, `ruby_tcy`,
`vertical`, `vertical_contents`; rtl in those plus `rtl`.

### Chapter dict keys

Set by this layer (the key order is Python insertion order and is kept in `model.json` as
`$dict` pairs):

| key | set by | notes |
|---|---|---|
| `title`, `text`, `blocks` | `_mk` 7701; fallback 8073; splits | `text` = `"\n\n".join` of block texts |
| `toc_level` | assembly 7778; splits copy it | absent on fallback chapters, head chapter, tail orphans |
| `sub_entries` | 7792 | `[{title, key, level}]`; key None = section's own block |
| `_omit_title_heading` | 7429, 7650-7660, 7812, 8079, 8509, 8540, 8683, `_split_by_writing_mode` | |
| `_omit_from_toc` | 7669, 7809, splits, `_split_by_writing_mode` | |
| `_from_spine` | 7442, 8076 | not read by the Rust generator |
| `_had_nav_listing`, `_nav_listing_at`, `_listing_keys` | 7695-7698, 7770, 8081 | `_listing_keys` popped by the rebuild; the other two survive when no rebuild happens (1 model) |
| `page_bidi` | `_replace_title_page` 8621/8632/8661 | from `inline_style.page_bidi` |
| `anchor_keys`, `heading_level`, `toc_links`, `preserved_images` | `_rebuild_contents_page` 8783-8906 | `blocks` popped; `toc_links` items `{text, target_chapter_idx, level[, target_key][, heading_level, block_style]}` |
| `font_size` | 8665, 8693, 8696 | `SMALL_FONT_SIZE = 0.75` for rebuilt contents and `SMALL_TEXT_CHAPTERS` titles |

Block keys this layer sets: `writing_mode` (7957), `_spine_file` (7540), `anchor_keys` /
`anchor_offsets` (orphans, trailing, TOC snaps, rebuild). In the 28 source-built models every one
of 1026 blocks carries `_spine_file` and `writing_mode` [V].

Block keys this layer reads from `extract_blocks_from_html`: `text`, `anchor_ids`,
`anchor_keys`, `anchor_offsets`, `heading`, `heading_level`, `break_before`, `front_matter`,
`type` (`box` -> `blocks`; `table` -> `table.rows[].cells[].paragraphs`), `spans` (flags,
`_is_link_flag`), `block_style`.

Rust generator (`generate/chapters.rs:32-56`) reads chapter `title, text, blocks, toc_level,
font_size, heading_level, anchor_keys, toc_links, sub_entries, preserved_images, _is_cover,
_omit_title_heading, _omit_from_toc, writing_mode, page_bidi`. Not read: `_from_spine`,
`_had_nav_listing`, `_nav_listing_at`, block `_spine_file`; the phase 1 allow-list task must list
them. Chapter-level `writing_mode` and `_is_cover` are set by the generator itself
(`native_generator.py:4937`), not by this layer [V].

Model tally over the 28 source-built models: `toc_links` in `directions`, `navigation`,
`page_list`, `vertical_contents`; `sub_entries` and `_omit_title_heading` only `navigation`;
`_omit_from_toc` `navigation`, `page_list`. No source-built model has `_from_spine`,
`preserved_images` or a surviving `_had_nav_listing` [V].

## 4. Nondeterminism and environment dependence

- **Spine reorder ties** [V code, I effect]: added files sort by `(TOC index, basename)`; two
  added files with the same basename in different folders and no TOC entry tie, and Python's
  stable sort keeps Calibre's set order, which varies per run. Same for which case-variant
  survives `folded` dedupe. Otherwise the reorder makes Calibre's random order deterministic, so
  DIVERGENCES "Linked files added to the spine" stops mattering for the chapter model once
  `_spine_in_order` is ported.
- **Dates** [V]: a year-only raw date gives `issue_date = None`, so Calibre's "15th of this
  month" never reaches the model, provided Calibre has some date.
- **No language** [V DIVERGENCES]: Calibre uses its interface language; Deckle gives `en`.
- **Options from the environment** [V]: `_apply_prefs` ORs Calibre's per-user plugin prefs into
  `opts`; plugin also recommends `disable_font_rescaling=True`, `minimum_line_height=0`
  (phase 2 already models these, `css/flatten.rs:6`).
- **Global state** [V]: `page_width(pt)` context manager around extraction and generation.
- **Sets** [V]: in this layer every set is used for membership, `min`, or iterated `sorted`
  (`listing_entry` 7614), so hash order does not leak. `footnote_keys` is a set handed to the
  generator; the dump sorts it. Whether the generator iterates it is the generator's question.
- **Dict order** [V]: `model.json` keeps dict insertion order (`model_dump.py:66`), so a
  byte-equal model needs keys emitted in Python's order (e.g. the rebuilt contents chapter:
  `title, text, toc_level, page_bidi, anchor_keys, heading_level, toc_links, font_size`).
- **Time, randomness**: none in this layer [V]. (UUIDs only in the Previewer fallback.)
- **Swallowed errors** [V]: `extract_metadata` keeps partial metadata on any exception;
  `_extract_toc_with_hrefs` keeps entries appended before an exception; each spine file is
  skipped on exception (7983); `_build_style_resolver` returns None on failure, which turns
  `breaks_known` off (every TOC entry its own section) and leaves blocks without
  `writing_mode`. In a normal Calibre run the resolver always exists [I].
- **Log-only** [V]: `_warn_media`, `_warn_flattened_tables`, the SVG-text warning, the
  dropped/snapped/out-of-order TOC messages. Phase 4 wants these as `convert:` warnings, so
  collect them, but they don't affect the model.

## 5. Proposed port tasks

Fixtures with both `sources/<name>.epub` and `models/<name>` (28): boxes, colour, dark_boxes,
directions, drop_caps, figures, floats, floats_697, font_sizes, fonts, frames,
horizontal_blocks, horizontal_blocks_ltr, link_colours, link_greys, links, lists, navigation,
notes, page_list, rtl, ruby, ruby_tcy, rules_spaces, spacers, typography, vertical,
vertical_contents. Source only: css_quirks, fake_margins, malformed, no_toc, package_quirks.
Cascade dumps: css_quirks, directions, fake_margins, fonts, frames, lists, malformed,
navigation, no_toc, notes, package_quirks, rtl, vertical.

**T1. Calibre's OPF view and the book arguments** (about 400-600 Rust lines [I]).
`CalibreOpf` (dates, listed spine basenames, guide with titles, nav path, NCX choice), the
3 -> 2 language table, `ConvertOptions`, then `metadata`, `rtl`, `fixed_layout`, `writing_mode`,
`landmarks`, `page_list`, `approximate_pages`, plus the href helpers (`_is_unsafe_href`,
`_normalize_href`, `_resolve_doc_path`, `_key_doc`, `_resolve_link_target`, `_page_label`).
`footnote_keys` here too once the resolver's `display` exists. Test: compare the non-chapter
arguments with every source-built `model.json` (no new dump). Coverage is thin: no fixture has
a publisher, a year-only date, guide-only landmarks, an NCX pageList, or a non-numeric page
label. Add a model for `package_quirks` (year-only date, `en-US`, NCX blank label, linked file)
and a small guide/NCX-pageList source; or dump the arguments from the cascade run.

**T2. The `data-kfxgen-*` marks inside `read_book`** (about 250 lines [I]). Between `cascade`
and `flatten`, per file, in kfxgen's order, each failure isolated. Test: the cascade `dom/`
dumps already hold `data-kfxgen-source` (49), `-align` (6), `-v-author`/`-v-ua` (69) [V]. No
cascade dump holds `-toc`, `-front`, `-link-colour` or `-horizontal` [V]: add `link_colours`,
`link_greys`, `horizontal_blocks` and a `class="toc"`/dedication source to `--cascade`, and
extend the tree comparison to these attributes. Independent of T1.

**T3. TOC entries, spine order, note ids** (about 400 lines [I]). `_extract_toc_with_hrefs` +
`_with_ncx_empty_labels`, `_spine_in_order`, `_respell_toc_hrefs`, `toc_targets`,
`_note_target_ids`, `_note_pair_ids`, `_book_link_targets`. Needs T1's `CalibreOpf`. Test: a new
`toc.json` from `book_dump.py` (the functions are pure; call them in the hook):
`{toc_entries (after respell), spine hrefs in kfxgen order, note_ids per file, note_pairs,
link_targets sorted}`. Exercised by all cascade fixtures; `package_quirks` (blank NCX label,
linked file, `linear=no`), `no_toc` (auto TOC, note pairs), `notes`.

**T4. Per-file loop and chapter assembly** (about 700 lines [I]). The loop's book-level parts
(inline-TOC skip, `_drop_vertical_marks`, block `writing_mode`, orphan and trailing keys, last
block fallback, `ValueError`), `_assemble_chapters_by_coordinate`, `_section_heads`, the
no-TOC fallback, `_split_by_writing_mode`, the `keep_nav_listings` second pass. Test before
blocks exist: dump, per pass, `spine_items_ordered` as built (deep copy before assembly mutates
it) plus `cover_href`, and the chapters right before `_replace_title_page`. Feed Python's blocks
to the Rust assembly. Fixtures: all 28; `navigation` (sub_entries, same block twins),
`page_list`, `vertical`/`horizontal_blocks` (writing-mode split), `notes` (TOC entries on notes).

**T5. Contents and title-page rewrite** (about 500 lines [I]). `_listing_without_contents`,
`_replace_title_page`, `_nav_listing_contents_chapter`, the three splits,
`_rebuild_contents_page`. Needs `page_bidi` (inline) and blocks' `block_style`/`spans` shapes.
Test: same T4 dump in, final chapters vs `model.json`. Fixtures: `directions`, `navigation`,
`page_list`, `vertical_contents`. The listing cases (`_had_nav_listing`, splits, rerun) have
no source-built model; only the out-of-git Gutenberg corpus (pg2701, pg2160, pg3011, pg64317,
pg100, pg5197 are cited in comments) exercises them.

Order: T1 and T2 in parallel; T3 after T1; T4 after T3; T5 after T4. Warnings collected in
T4/T5 as a list.

## 6. Surprises and likely porter mistakes

1. **Calibre's OPF, not the book's.** `_opf_raw_dates`, `_spine_in_order`, `_book_files`,
   `_with_ncx_empty_labels` parse `container.opfname`, the OPF Calibre wrote. When the book's
   own OPF is root `content.opf`, Calibre overwrote it, so even the "raw" date is Calibre's
   serialization [V file overwrite; I content]. `_has_media_overlays` alone reads the book's own
   OPFs via `container.xml`.
2. **Marks cannot be computed from `Book`.** `File.dom` has renamed classes and no `align`; the
   first-pass rules' selectors would match the renamed classes. Do it before `flatten`.
3. **Basename matching.** TOC entries find their spine file by basename (`_normalize_href`,
   `spine_order.index`), first match wins; the reorder compares basenames. Matching on full
   paths would be "more correct" and wrong.
4. **The recursion.** `_with_listings_kept` reruns the whole extraction on trees pass 1 already
   touched (`_drop_vertical_marks`, and whatever `extract_blocks_from_html` mutates). Make the
   Rust extraction pure so pass 2 equals a fresh run, and check that kfxgen's mutations are
   idempotent.
5. **Shared block dicts.** Chapters slice the same block objects; `_drop_keys` over all
   chapters and `_assemble`'s `anchor_keys` writes mutate in place; `_split_by_writing_mode`
   copies shallowly. Identity tests (`is`, `id(b)`) appear in `_rebuild_contents_page` and the
   table warning; `chapters.index(ch)` (8626, 8655-8659) uses dict equality, not identity.
6. **`_spine_file` and the other private keys reach the generator** and the model JSON; dropping
   them breaks a byte-equal model compare.
7. **TOC entry level**: top-level `oeb.toc` nodes are level 0; a node with an empty title is
   skipped but its children keep level+1 (9161-9171).
8. **`issue_date`** needs both: Calibre has a date *and* the raw first date is a full date.
9. **`page_list` is all or nothing**: one bad label drops the whole list (9076).
10. **Landmarks keep only the last `toc`** but every `start` (9033-9035).
11. **Percent-encoding**: TOC fragments arrive encoded (`fn%3a1`), ids raw; keys use decoded
    file names (`_key_doc`) but `_respelled`/`_respell_toc_hrefs` compare differently
    (`unquote` then lower case). Port each comparison as written.
12. **`_is_unsafe_href`** decodes up to 16 times and rejects growth past 4x; it also logs to a
    security logger. Port the bounds exactly; they decide which hrefs become `""`.
