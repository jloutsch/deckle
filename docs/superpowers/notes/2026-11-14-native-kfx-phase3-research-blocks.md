# Phase 3 research: kfxgen's block-level walk (converter.py at 0bc705d)

Sources: `src/converter.py` (C), `src/inline_style.py` (I) in the scratch extract. Rust paths are
under `crates/deckle-kfxgen/src/`. Labels: **[read]** = verified by reading code (or by running a
key tally over the fixture models, which I did); **[infer]** = inference, not run.

Black boxes used by the walk (other researchers): `_walk_inline` C:2490,
`normalize_runs_with_anchors` I:139, `_initial` C:2107 (drop cap), `_label_texts` C:3032,
`_is_preformatted`/`_white_space_flags` C:2013, `_make_img_token` C:1359, `_img_size_hint` C:1385,
`_img_float` C:1447, `_img_inline_extras`, `_resolve_img_src`, `_css_background_image` C:1496,
`_svg_*` C:1529-1800, `css_text_colour` I:783, `page_scale` (`page_pt`, `em_percent`; already
ported as `generate/page_scale.rs`).

---

## 1. `extract_blocks_from_html` (C:5763-7155, 1393 lines)

### Parameters [read]
`element` (spine file root), `style_resolver` (elem -> dict|None, or None), `base_href` (spine
href; without it `anchor_keys` stay empty), and out-parameters/options: `nav_listing_at` (list of
`(block index, frozenset ids)` per discarded contents listing), `tables_seen` / `notes_seen`
(first block of each table written as rows / as notes paragraphs), `native_tables` (bool),
`toc_targets` (set of fragment ids TOC entries name in this file), `link_targets` (book-wide set
of "file#id"), `keep_nav_listings`, `trailing_ids` (ids left at file end). Called once per spine
file at C:7927 with all of them set; the caller then adds `writing_mode` to every top-level block
(C:7949-7958) and later `_spine_file` (book level).

### State (closure variables) [read]
`block_tags` (C:5827-5871: XHTML-namespaced and bare names of p, div, h1-6, blockquote, pre, li,
ol, ul, dl, dt, dd, section, article, figure, table, caption, thead, tbody, tfoot, tr, hr, plus
`_BODY_BLOCK_TAGS` minus img/svg), `blocks` (output list), `notes_tables` / `native_made`
(sets of `id(table)`), `pending_ids` (anchors waiting for the next leaf block), `rule_ids`,
`pending_markers` (list markers `(marker, ordinal|None)`), `list_ordinals` (list elem ->
{li: n}), `margins` (`_ById`: block -> [top, bottom] em), `paddings` (`_ById`: block ->
[pad_top, pad_bottom, margin_inside_top, margin_inside_bottom]), `break_at` ([index]),
`leaf_cells` (set of `id(cell)`), `clear_at` ([(index, side)]).

### Order of passes [read]
1. Find `<body>` (`.//{xhtml}body`, then `.//body`, else the root) (C:5820-5824).
2. Walk: an SVG document -> `_emit_svg_blocks(body)`; else if the body has loose text (its text
   or any child's tail non-blank) -> `_walk_container(body, is_block=_is_block_at_body)`; else
   `_walk(child)` for each child (C:7068-7080).
3. Shift `nav_listing_at` indices down by the number of `space` blocks before each (bisect)
   (C:7082-7088).
4. `blocks = _space_between(blocks, margins, measured=resolver is not None, paddings)`: removes
   `space` blocks, writes `margin_above`/`margin_below`/`padding_below`/`space_above`/`tight`,
   recurses into boxes (C:7089).
5. Trailing ids: copy to `trailing_ids` (minus `rule_ids`), then append to the last non-rule
   block's `anchor_ids`, and also to a table's last row or a box's last kid (C:7097-7123).
6. `_attach_anchor_keys(blocks, base_href)` (C:7125); fallback when no blocks: `[]` for an SVG
   document, else one flat block from `body.xpath("string()")` (C:7128-7154).

### Nested functions (call graph, purpose, size) [read]
| function | lines | purpose | calls |
|---|---|---|---|
| `_walk` | 6211-6247 (37) | per element: resolve css, note page-break/clear index, `_walk_flow`, mark `front_matter`, put `break_before`/`clear` on the first non-space block from the noted index | `_walk_flow`, `_page_breaks`, `_clear_side`, `_is_front_matter` |
| `_walk_flow` | 6248-6284 (37) | non-`li`: `_walk_element` + count tables into `tables_seen`/`notes_seen`; `li`: compute ordinals once per parent, push marker, walk, drop unused marker | `_walk_element`, `_list_ordinals`, `_list_marker` |
| `_walk_element` | 6285-6531 (247) | the dispatcher: comment/PI skip; hidden -> ids pending; epub:switch -> walk default; nav listing -> record + `_discard_listing`; CSS background image -> image block; `hr` -> `_emit_rule`; native table (+notes check, TOC fallback, fostered content, captions, table block); `tr` holding a table -> `_walk_split_row`; leaf block -> paragraph dict (or carry ids / emit spacer); `img`/`svg`; container -> `_walk_container` then `_make_box` or margins | nearly all below |
| `_walk_container` | 6944-7062 (119) | children in order: block children `_walk`ed, runs of inline content flushed as one paragraph each (`_flush_inline` 6956-7007); anchors-follow-rows handling; pictures among words go inline | `_flush_inline`, `_walk`, `_walk_inline`, `_inline_picture_parts`, `_pictures_among_words`, `_anchors_follow_rows`, `_take_marker` |
| `_make_box` | 6747-6848 (102) | replace `blocks[start:]` with one `type: box` block when the container has border/fill/width-around-picture/horizontal mark | `_css_border`, `_box_background`, `_box_width`, `_float_side`, `_box_padding`, `_bidi`, `_note_box_margins` |
| `_box_share` | 6618-6698 (81) | width share a % margin is measured against (walks ancestors, skips ones the margin collapses through) | `_css_border`, `_padding_em`, `_has_content`, `parse_css_length` |
| `_emit_fostered` | 5911-5976 (66) | content of a native table outside its cells, before it, as paragraphs | `_fostered_content`, `_walk`, `_walk_inline`, `_make_img_token` |
| `_walk_split_row` | 6142-6197 (56) | rows path: a `tr` holding a table; each cell walked as body, `row: True`, header bold | `_walk_container`, `_row_align` |
| `_walk_cell` | 6094-6141 (48) | native table: walk one cell with saved/cleared pending state, space its blocks, return them | `_walk_element`, `_space_between` |
| `_note_margins` | 6699-6746 (48) | record container margin-top/-bottom on its first/last paragraph (collapse), `own_bottom_em`, `bottom_in_em`, `outer_em_ratio` | `_margin_em`, `_box_share`, `_collapse`, `_font_ratio` |
| `_carry_container_margins` | 6885-6925 (41) | add a container's side margins to its paragraphs' `margin_left/right` (as %), or to a box | `_add_margin`, `_page_share` |
| `_emit_svg_blocks` | 6024-6060 (37) | svg -> image blocks + text paragraphs | svg helpers |
| `_note_padding` | 6582-6617 (36) | record an unboxed container's padding on first/last paragraph | `_padding_em`, `_font_ratio` |
| `_note_box_margins` | 6849-6884 (36) | a box's own margins, collapsed with its first/last kid's when no border/padding | `_margin_em`, `_collapse` |
| `_emit_rule` | 6549-6581 (33) | `hr`: rule block, or a `space` block when it draws nothing | `_rule_look`, `_css_border`, `_hidden_rule_em`, `_margin_em` |
| `_discard_listing` | 6061-6093 (33) | drop a contents listing's text, keep pictures and ids | `_emit_image_block`, `_emit_svg_blocks` |
| `_emit_image_block` | 5997-6023 (27) | `<img>`/background/svg image -> block with IMG token, `picture_align` | image helpers, `compute_block_style` |
| `_take_marker` | 5977-5996 (20) | prefix pending list marker(s) to a text block | `_already_numbered`, `_prefix_marker` |
| `_inline_picture_parts` | 6926-6943 (18) | `<img>` among loose words as inline parts | image helpers |
| `_emit_space` | 6532-6548 (17) | push a `type: space` block (margin, line, height, margin_after) | |
| `_is_block_at_body` | 6198-6210 (13) | which body children count as blocks in the loose-text body path | |
| `_holds_blocks` | 5873-5883 (11) | child is in `block_tags`, or an epub:switch whose default holds one | `_epub_switch_branch` |

Module-level helpers in scope (sizes incl. docstrings) [read]: tables C:3272-3943 and
4241-4735 (~1160), boxes/rules/spacers C:3944-4240 (~300), notes/hidden/shown/marks C:4740-5055
(~315), `_attach_anchor_keys` C:5056-5130 (75), lists C:5131-5345 (~215), spacing C:5346-5760
(~415), resolver and its helpers C:211-1244 (~1030, of which ~180 are the pre-flatten marks
C:248-455 and ~110 book-level writing-mode code C:921-1032 not used per block).

---

## 2. The block dict contract

Key order is part of the contract: `PyValue::Dict` keeps pairs in order and its derived
`PartialEq` compares them in order, and the models show stable insertion orders [read: ran a key
tally over all models; e.g. paragraphs in `lists`: `text spans block_style preformatted
anchor_ids heading [heading_level] row [in_list] anchor_offsets own_bottom_em bottom_in_em
margin_above [margin_below padding_below tight space_above] anchor_keys writing_mode
_spine_file`].

### Paragraph from a leaf block (C:6439-6482) [read]
| key | type | set when |
|---|---|---|
| text | str | always (marker prefixed) |
| spans | list[(int,int,frozenset)] | always (from `_walk_inline`) |
| block_style | dict or None | `compute_block_style(css)` (+`drop_cap` for `<p>`, `align` for `tr`/display math, `bidi`, start/end remap); None without resolver/css and no bidi |
| preformatted | bool | always |
| anchor_ids | list[str] | always |
| heading | bool | always (h1-h6) |
| heading_level | int | only for h1-h6 |
| row | bool | always (`tr`) |
| in_list | True | only inside an `li` (self or ancestor) |
| label_texts | list | only when `_label_texts(elem)` is not None (black box) |
| anchor_offsets | dict str->int | always; rekeyed by `_attach_anchor_keys` |

### Paragraphs without heading/preformatted/row keys [read]
- `_flush_inline` (loose text in a container): `text spans block_style anchor_ids anchor_offsets`;
  block_style is `{"align"}`+bidi or None (C:6989-7007).
- `_emit_fostered` (C:5928-5943), svg text paragraphs (C:6051-6059): same five keys.
- `_emit_image_block` (C:6008-6022): same five, block_style None, plus `picture_align` (str) when
  the image's computed text-align maps.
- Fallback flat block (C:7146-7151): `text spans block_style anchor_ids`, no `anchor_offsets`
  until `_attach_anchor_keys` adds `{}`/keys.

### Keys added after creation [read]
| key | type | who, when |
|---|---|---|
| own_bottom_em | bool | `_note_margins` C:6716 on the last paragraph of a container, first time; boxes C:6862 |
| bottom_in_em | bool | `_note_margins` C:6744 when its margin-bottom wins; boxes always C:6861 |
| outer_em_ratio | float or None | `_note_margins` C:6735 (padded wrapper); read by `_space_between` |
| break_before | True | `_walk` C:6238 on first non-space block after a page break; boxes: always, bool C:6813 |
| front_matter | True | `_walk` C:6232 for every block of a front-matter element |
| clear | str | `_walk` C:6244 (`setdefault`); moved from first kid to box C:6809 |
| margin_above | float (em) or None-free | `_place_margin` (first block, or when a gap is placed on `b`) |
| margin_below | float (em) | `_place_margin`, and the last block of a run C:5719 |
| padding_below | float | last block of a run C:5724 |
| trailing_space | float | last block when it is not a paragraph/box C:5708 (popped by a box C:5652) |
| tight | True | `_mark_tight_runs` C:5402 |
| space_above | float | `_space_between` C:5759 (scene breaks) |
| row | True | `_walk_split_row` C:6194 (overwrites) |
| block_style.bold | True | `_walk_split_row` for bold header cells C:6188 |
| anchor_keys | list[str] | `_attach_anchor_keys` (file key prepended on block 0) |
| writing_mode, _spine_file | str, int | caller / book level, not this function |

### `type: space` (C:6537-6547), removed by `_space_between` [read]
`text "" spans [] block_style None anchor_ids [] anchor_offsets {} type margin line height
margin_after` (floats). Never reaches the model.

### `type: rule` (C:6569-6580) [read]
`text "" spans [] block_style None anchor_ids [] anchor_offsets {} type "rule" rule{...}`;
`rule`: `space_above`, `space_below` (floats; `_place_margin` rewrites `space_above`), then
`_rule_look` keys when set: `width` (float,"%"|"em"), `margin_left`/`margin_right` (float %),
`align` ("left"|"right"|"centre", only with width), `border` {side: (style, pt float or
(em,"em"), ARGB int|None)}, `height` ((float,"pt"|"em")), `fill` (ARGB int, only with height>0).

### `type: box` (C:6810-6845) [read]
`type break_before [clear] text("\n\n"-joined kid texts) spans[] block_style None font_px blocks
anchor_ids anchor_offsets box` then `bottom_in_em own_bottom_em` (C:6861-6862), spacing keys,
`anchor_keys`. `box`: `border` (dict|None), `background` (ARGB int|None), `padding` ({side: em
float or ("%",n)}|None), `font_px` (float|None), `margin_left`/`margin_right` (% float|None,
later added to by `_carry_container_margins`), `width` (%|None), `width_px`, `float`
(side|None), `centred` (bool), `bidi` (dict), `horizontal` (True, only when marked). Leaf boxes
null the kid's `margin_left/right` and drop `bidi.set_here` (C:6790-6799).

### `type: table` (C:4689-4709) and its parts [read]
Block: `type text("\n"-joined row texts) spans[] block_style None anchor_ids anchor_offsets
table`. `table`: `anchor_ids` (own), `rows`, `border`, `column_widths` (("col"|"cell",
[float|None])|None), `width` (float|None), `bidi`, `horizontal` (True when marked). Row:
`group` ("head"|"body"|"foot"), `border`, `bidi`, `group_bidi`, `group_border`, `anchor_ids`,
`cells`, then `source_order` (int); rows stably sorted head/body/foot. Text cell
(`_table_cell` C:4415-4434): `text spans anchor_ids anchor_offsets block_style header not_bold
heading colspan rowspan valign background` then `border padding width_set` (C:4586-4588).
Paragraph cell (`_paragraph_cell` C:4458-4475): same minus `heading`, plus `paragraphs` (list of
blocks from `_walk_cell`, each with `block_style` replaced by `_cell_block_style`), `anchor_ids
[]`. `_attach_anchor_keys` adds `anchor_keys` to table, rows, cells and paragraphs.

### Cross-check with the Rust generator [read]
Rust reads (via `get("…")`): blocks: `type text spans block_style heading heading_level in_list
margin_above margin_below space_above tight picture_align anchor_keys anchor_offsets
writing_mode box rule`; box: `background bidi border centred float font_px horizontal
margin_left margin_right padding width width_px float_wrapper`; rule: the `_rule_look` keys;
table/row/cell: all keys above except `anchor_ids`.

Written by the converter, **not read** by the Rust generator: `anchor_ids` (all levels),
`preformatted` (only refused when true), `row`, `front_matter`, `break_before`, `own_bottom_em`,
`bottom_in_em`, `outer_em_ratio`, `padding_below`, top-level box `font_px`, `label_texts`/`clear`
(refused when present). They still have to be produced for model equality. (`row`,
`front_matter`, `break_before`, `trailing_space` are read by Python code outside the generator:
`_mark_tight_runs`, book-level chapter split C:7500 and C:8543.)

Generator `Unsupported` that block-level output can trigger [read]:
`BLOCK_UNPORTED` (`generate/chapters.rs:22`): `preformatted`, `clear`, `label_texts`, a `table`
key on a non-table block, `float_wrapped`/`in_cell_paragraph`/`trailing` (written by kfxgen's
generator or book level, not here). Also: a picture in a list item (`chapters.rs:335`), a rule
in a box (`boxes.rs:316`), boxes nested >32, `bidi` on a rule/box block_style, an unknown
block_style key (`block_styles.rs:124`), and in tables: table block with `block_style`,
`heading`, `clear`, `trailing_space` (`tables.rs:162-173`), and **cell paragraphs with any
`block_style`, `margin_above`/`margin_below`, a box, or a picture** (`tables.rs:346-362`).
[infer] With a real resolver `_walk_cell` always gives its paragraphs `margin_above` (via
`_space_between(measured=True)`) and `_paragraph_cell.in_cell` usually a dict `block_style`, so
any paragraph cell from a real book will hit `Unsupported("block styles")` or `("block
margins")`; the models that do have `paragraphs` (`table_paragraph_cells`, `font_faces`, …) have
no source EPUB and carry `block_style: None`.

### Fixture coverage (source-backed models only) [read: tally over 28 models + grep of EPUBs]
Present: boxes (boxes, colour, dark_boxes, drop_caps, typography, figures, floats, floats_697,
frames, horizontal_blocks(_ltr) with `horizontal`, ruby, spacers, directions), rules
(rules_spaces: 35, all `_rule_look` keys), native tables (directions 3 incl. `th`, font_sizes,
horizontal_blocks(_ltr), links, ruby, spacers), lists/in_list/tight (lists, notes), space_above
(links, spacers), bidi everywhere, drop_cap (drop_caps, typography).
**Absent from every source fixture**: rows-path tables (`row: True`), notes tables, split rows,
captions, colspan/rowspan, paragraph cells, svg, math, `pre`, epub:switch, hidden elements,
page breaks, `clear`, CSS background-image pages, `keep_nav_listings`.

---

## 3. The style resolver (C:1035-1244)

`_build_style_resolver(oeb_book, item, log)` returns `resolve(elem)` or None (whole file) when
the Stylizer fails. `resolve` returns None on **any** exception for that element (C:1237). It
keeps two caches: `sizes` (elem -> font px) and `fills` (elem -> fill behind). [read]

### Resolved dict (not in the model; consumed by the walk) [read]
| key | value | how |
|---|---|---|
| font-size | float CSS px, `round(x, 3)` | `_font_size_px`: specified values up the ancestors, 16px root default, `_calibre_keyword_px` reverse-maps calibre's rewritten keywords, `_css_size_px` |
| line-height | float factor (`round(.,4)`), 1.2, or None | `_line_height_factor` C:578 |
| line-height-declared | str or None | `_specified_inherited` |
| first-letter | dict or None (`<p>` only) | `pseudo_classes(())["first-letter"]`, its font-size replaced by `round(px,3)` |
| q-css | {quotes, before, after} or None (`<q>` only) | `_q_css` C:644 |
| float, clear, own-text-align, text-indent, margin-*, text-combine-upright, vertical-align, own-direction, own-unicode-bidi, width, background-image, background-color, background, background-repeat, page-break-*, break-*, display | own declared str or None | `st.get` |
| height | own; `<hr>`: None for exactly "2px" | `_book_height` |
| text-align, font-family, font-style, white-space, list-style-type, border-collapse | computed | `style[prop]`, falling back to `.get` on exception |
| font-weight, letter-spacing, font-variant, text-transform, text-orientation, text-emphasis-style | nearest declared, as written ("inherit" skipped) | `_specified_inherited` |
| writing-mode | nearest declared, `<html>`/`<body>` only | |
| color | nearest declared; a `data-kfxgen-link-colour` mark wins | `_specified_colour` |
| text-decoration | sorted "line-through underline" union up the ancestors, or None | `_decoration_lines` |
| background-behind | 0xRRGGBB int or None | `behind`: own fill for box/cell tags, else parent's |
| padding-sides, border-widths | 4-tuples of own declared (top,right,bottom,left) | |
| border-sides | 4-tuple of (computed style, computed width, computed colour) | width is calibre's pt float, or the keyword string (`medium`) |
| (vertical-rl file) margin-*, padding-sides | replaced per slot from `data-kfxgen-v-author`/`-v-ua` marks | `_vertical_box` C:432 |

`compute_block_style(css)` (I:995-1044) returns, in order: `align` (keyword|None via
`_ALIGN_ALIASES`), `indent` ((mag str, unit sym)|None; hanging indent clamped to margin-left),
`margin_left`, `margin_right`, `font_family` (list), `line_height` (float|None), `bold`,
`italic`, `font_size` (float|None), then `small_caps text_transform letter_spacing colour
text_orientation text_emphasis` only when set (colour not when black). [read]

### Stylizer/Style API used, and Rust coverage [read Rust `css/style.rs`, `css/normalize.rs`]
| Python | used for | Rust |
|---|---|---|
| `Style.get(prop)` | most keys; `_specified_inherited`; `_decoration_lines`; `_box_background(stylizer.style(e))` in `behind` | `Styles::get` - present |
| `Style[prop]` (`_computed_value`) | text-align, font-family, font-style, white-space, list-style-type, border-collapse, border-*-style/-width/-color | `Styles::computed` - present; the port must map `Value` back to str-or-float as Python sees it and treat `Error::NoDefault` as "fall back to `.get`" |
| `Style.pseudo_classes(())` | first-letter, before/after | `Styles::pseudo` suffices: the filter is empty here, and the callers treat `{}` and missing alike |
| `stylizer.opts.input_profile.fnames/.fbase` | `_calibre_keyword_table` (f"{pt/base:.1f}rem" strings) | `Profile.fnames/fbase` exist; need the **input** profile (default) passed to the resolver - small gap |
| `stylizer.rules` (pre-flatten rule list: sort key with origin, cssdict, selector text) + `css_selectors.Select(root, ignore_inappropriate_pseudo_classes=True)` | `mark_link_state_colours`, `mark_vertical_margins` | **missing** from `Styles`; needs the pre-flatten cascade |
| `Style._get`, `_fontSize`, `lineHeight`, `width`, `fsizes`, parent style | not used by this scope | - |
The resolver never uses calibre's computed font size or line height; it recomputes from
declared values. Parent access is only `elem.getparent()` + `stylizer.style(parent)`.

---

## 4. Float, rounding and formatting hazards [read]

The model stores floats by `repr`, ints vs floats are distinct, and Dict order matters.
- `round(x, n)`: C:1073 (font-size, 3), C:594/605 (line-height, 4), C:902 (first-letter, 3),
  C:3880 (`_css_border` pt*0.625, 4), C:4067/4103 (`_rule_look`, 4), C:4504 (0.45*attr px, 4),
  C:5696 (gaps, 4). `py::round` exists (`generate/py.rs:171`).
- `round(x)` to **int, half-even**: `_spacer_em` C:4189/4191 (`own*4/3`, `factor*own`), then
  `f"{line}px"`. Rust `f64::round` is half-away; use `round_ties_even`.
- f-strings: `_add_margin` `f"{total:.4f}"` + strip zeros (C:1293); `parse_css_length`
  `f"{x:.4f}"` for in/cm/ex (I:302); `css_letter_spacing` `f"{x:.4g}"` (I:610, `py::format_g`);
  `_calibre_keyword_table` `f"{x:.1f}rem"` (C:564); `_format_ordinal` `f"{n:02d}"`.
- Magnitudes stay **strings as written** (`parse_css_length`: "1.50"->"1.5", ".5" and "+2" kept);
  hanging indent builds `"-" + margin_left[0]` (I:1021).
- Operation order differs between twins: `_unit_percent` `em * 72 / 25.4 / 12` (C:1262) vs
  `_length_percent` `72 / 25.4 * em / 12` (I:980): not equal in floating point for mm; port each
  literally. Same care for `_padding_em` (`n * k * ratio`), `_margin_em` (`sign * em[1] *
  (page_pt()/1200) * share`), `_box_share` (C:6677-6697), `_place_margin`
  (`(space + fixed) / ems + lines`), `_css_size_px` (`n * inherited / 100`, `inherited / 1.2`).
- `-0.0`: `_margin_em` multiplies by `sign = -1.0` (C:4144-4146), so "-0" gives -0.0;
  `_collapse` = `max(0.0, a, b) + min(0.0, a, b)` relies on Python's max/min returning the
  **first** of equal arguments. Rust `f64::max/min` do not promise that. `PyValue::Float`
  equality treats 0.0 == -0.0, so a Rust test would not catch a sign difference; a JSON compare
  would.
- `(x or 0) + share or None` (C:6907): int 0 + float, and 0.0 -> None.
- `collections.Counter` over rounded float gaps, ties broken by `-g`; `usual + 0.01` and `>=`
  comparisons (C:5726-5759).
- `float(width)` of calibre's computed border width; `_rule_look` compares it `== 0.72`
  (calibre's 1px*72/100) to detect the UA `hr` border (C:4076-4080).

---

## 5. lxml behaviour the Rust `Dom` must match [read Dom `dom/mod.rs`]

- **Comments/PIs are children in lxml, dropped in `Dom`** (only non-blank ones leave an
  IN_TEXT/IN_TAIL note). Visible effects: `len(elem) == 0` in `_is_empty_anchor` /
  `_is_contentless_anchor` (C:3298-3318) counts a comment child; in the loose-text body path
  `_is_block_at_body` returns True for a comment (C:6199), which **flushes the inline run**, so
  a comment splits a paragraph; `getnext()/getprevious()` in `_row_anchor_ids` stop at a comment
  (C:3399-3402); `_walk_element` skips them (C:6286). Iterating `for child in elem` and adding
  `child.tail` is otherwise equivalent to Dom's merged text. [infer] whether `itertext()` yields
  comment text in calibre's lxml version is not checked; it feeds `_text_length` (2000-char
  cell limit), `_is_spacer`, `_has_content`.
- Namespaces: `elem.tag in block_tags` and `_epub_switch_branch` compare the **qualified** tag
  (XHTML or no namespace; `{ops}switch` or literal `epub:switch`), while `_local_tag` ignores
  the namespace. `Dom::tag` is the local name, so the port must also check `Dom::namespace`.
- Attributes: Clark-notation lookups `{http://www.idpf.org/2007/ops}type` and
  `{http://www.w3.org/XML/1998/namespace}lang` beside literal `epub:type`. `Dom::attr` looks up
  by qualified name only; a book binding the ops namespace to another prefix would differ.
  Needs a lookup by (namespace URI, local name).
- Missing on `Dom`: subtree iteration from any node (`elem.iter()`, `iterdescendants()`; only
  `descendants()` from the root exists), `iterancestors`, `itertext`, `xpath("string()")`,
  `copy.deepcopy` (`_shown_tree` C:4947: copies, removes non-default switch branches, sets
  `switch.text = None` and `default.tail = None`; a filtered view would do).
- `data-kfxgen-*` marks set before calibre flattens: `-source`/`-toc`/`-front`/`-align`
  (`mark_source_classes` C:4992), `-link-colour` (C:275), `-v-author`/`-v-ua`/`-horizontal`
  (C:341). Read by `_is_nav_listing`, `_is_front_matter`, `_cell_block_style`,
  `_specified_colour`, `_vertical_box`, `_make_box`, `_table_block`. Without the root's
  `data-kfxgen-source`, `_is_nav_listing` falls back to the `class` attribute, which after
  flattening holds calibre's class names. Their caller (`specialize_css_for_output`) is not in
  the extracted sources. `_drop_vertical_marks` (C:990) removes three of them for fixed layout.
- Identity: `id(elem)` sets (`leaf_cells`, `notes_tables`, `native_made`) and `_ById` maps keyed
  by `id(block)`. Use `NodeId` for elements; blocks need a stable identity that survives being
  moved into boxes/cells (an index into an arena, or the margins stored on the block).

---

## 6. Proposed port tasks

Two intermediate dumps make the tasks testable before chapter assembly exists. Both wrap
module-level names in `converter` from Deckle's injected hook (`tools/ref/kfxgen_hook/`); the
call sites look the names up at call time [read: C:7918, C:7927, C:6120, C:7089, C:5650 call them
by bare name]:
- **D1 resolver dump**: wrap `_build_style_resolver`; for each spine file, for every element in
  document order, write `(path, resolve(e))` (and `compute_block_style` of it). `resolve` only
  fills caches, so calling it on every element changes nothing [infer]. Dedupe by href
  (`_footnote_keys` C:4766 also builds resolvers).
- **D2 blocks dump**: wrap `extract_blocks_from_html`; deep-copy its return value at return,
  plus `nav_listing_at`, `tables_seen` (as indices), `notes_seen`, `trailing_ids`, keyed by
  `base_href`. Optional **D2a**: wrap `_space_between` to record each call's input blocks with
  their `margins`/`paddings` entries inline and the call depth, so the walk can be compared
  before spacing is ported.

| # | task | Python lines (approx. count) | fixtures | test |
|---|---|---|---|---|
| 0 | Dom/marks prerequisites: subtree iter, ancestors, itertext, ns-aware attr, comment positions; the four pre-flatten marks; input profile into the resolver | C:248-455, 990-997, 4972-5013 (~250) | link_colours, link_greys, vertical, vertical_contents, ruby_tcy, horizontal_blocks(_ltr), plus any with a `toc` listing (navigation, notes, page_list, lists) | phase 2's `dom/` dumps already hold the marks (per the phase 2 note) |
| 1 | Resolver + `compute_block_style` + `css_typography` glue | C:211-246, 457-666, 881-925, 1035-1244; I:265-310, 514-614, 867-882, 938-1054 (~600) | all 28 | D1, per element |
| 2 | Walk skeleton: body selection, `_walk`, `_walk_flow` (non-li), `_walk_element` leaf and container branches, `_walk_container`/`_flush_inline`, hidden/non-rendered/switch/shown tree, nav listing discard, image blocks, bidi (`_bidi`, `_with_bidi`, `_start_end_align`, `_first_strong`), page breaks/clear/front matter, trailing ids, `_attach_anchor_keys` | C:668-875, 4740-4990, 5014-5130, 5763-5910, 5997-6023, 6061-6093, 6198-6531 (minus tables/boxes/hr), 6926-7155 (~1100) | fonts, rtl, link_colours, link_greys, navigation, notes, page_list, vertical, vertical_contents, ruby_tcy | D2a (blocks before spacing) |
| 3 | Margins and spacing: `_note_margins`, `_note_padding`, `_box_share`, `_carry_container_margins`, `_margin_em`/`_padding_em`/`_font_ratio`/`_collapse`, spacers (`_is_spacer`, `_spacer_em`, `_hidden_rule_em`, `_emit_space`), `_place_margin`, `_space_between` (minus box part), `_mark_tight_runs` | C:3893-3917, 4127-4220, 5346-5760, 6532-6548, 6582-6746, 6885-6925 (~700) | same as 2 | D2 |
| 4 | Lists: markers, ordinals, `_take_marker`, li branch of `_walk_flow`, `in_list` | C:5131-5345, 5977-5996, 6267-6284 (~240) | lists, notes | D2 |
| 5 | Rules: `_emit_rule`, `_rule_look`, `_css_border`, `_border_colour`, `_box_background`, `_background_parts` | C:3843-3892, 3980-4126, 6549-6581 (~250) | rules_spaces | D2 |
| 6 | Boxes: `_make_box`, `_note_box_margins`, `_box_width(_px)`, `_box_padding`, `_has_content`, box branch of `_space_between`, box kids in `_attach_anchor_keys` and trailing ids | C:3944-3979, 4221-4240, 5648-5682, 6747-6884 (~260) | boxes, colour, dark_boxes, drop_caps, typography, figures, floats, floats_697, frames, horizontal_blocks(_ltr), ruby | D2, then whole-model equality once book level exists |
| 7 | Native tables: `_table_is_native`, `_table_block`, cells (`_table_cell`, `_paragraph_cell`, `_cell_block_style`, `_cell_valign`, `_cell_padding`, spans, widths, columns), `_walk_cell`, `_emit_fostered`, captions, TOC fallback, anchors-follow-rows, `_table_start_ids` | C:3272-3352, 3604-3842, 3918-3943, 4241-4735, 5911-5976, 6094-6141, 6331-6387 (~1150) | directions, font_sizes, horizontal_blocks(_ltr), links, ruby, spacers | D2 |
| 8 | Rows path, notes tables, split rows, svg blocks, background-image pages, `keep_nav_listings` | C:3353-3455, 6024-6060, 6142-6197, 6398-6408, 6425-6428 (~250) | **none** | needs new fixtures (Calibre + kfxgen run, the lead's call) or unit cases from kfxgen's tests |

Order: 0 -> 1 -> 2 -> 3, then 4, 5, 6, 7 in any order (each only adds branches), 8 last. Task 2
needs the inline port (`_walk_inline`, `normalize_runs_with_anchors`, `_initial`) at least for
plain runs.

---

## 7. Things a porter would get wrong

1. Key order and key presence: leaf paragraphs always carry `preformatted`/`heading`/`row`;
   `_flush_inline`, fostered, svg-text and image blocks never do. Keys appended later land after
   `anchor_offsets` in pass order. [read]
2. Index bookkeeping across splices: `break_at`/`clear_at` hold list indices; `_make_box`
   replaces `blocks[start:]` with one block and `_walk_cell` deletes its range, so a stale index
   can make the break land on a later block. Port the index semantics, not "first block of the
   element". `nav_listing_at` is fixed up in `_walk_cell`, on TOC fallback, and for space
   blocks. [read; consequence inferred]
3. `_walk_cell` and `_walk_split_row` save and restore `pending_ids`, `pending_markers`,
   `break_at`, `clear_at`; `_walk_split_row` does not save them. [read]
4. A box keeps its kids' `break_before` and also sets its own `any(...)`; only `clear` is moved
   (popped). [read]
5. Margins live outside the blocks (`_ById`) until `_space_between`; `margins.get(b, (0, 0))`
   defaults; boxes are spaced recursively, cells separately (`measured=True` always in
   `_walk_cell` when a resolver exists). [read]
6. Python string semantics: `str.strip()`/`split()` whitespace (includes U+001C-001F and NBSP),
   `len()` in code points (`_mark_tight_runs` 60-char lines, `_prefix_marker` offsets, 2000-char
   cells), `str(None) == "None"` in `str(css.get("margin-left")).strip() == "auto"` (C:6836).
   Use `generate/py.rs` helpers. [read]
7. `_is_hidden` reads only the element's own declared `display`; hidden `li` are left out of
   ordinals (C:5226); hidden ids carry forward. [read]
8. `text-align` default: `compute_block_style` maps `start` to left, but `_start_end_align`
   remaps raw `start`/`end` only for rtl paragraphs; header cells have their own rules using the
   `data-kfxgen-align` mark (C:4306-4377). [read]
9. The resolver swallows every exception per element (None) and per file (no resolver, which
   switches `_space_between` to `measured=False` and drops spacers in cells). Deep trees can
   raise `RecursionError` in `_font_size_px`/`behind` and silently give None. [read; recursion
   limit effect inferred]
10. `_page_share` vs `_length_percent` and the other near-duplicate formulas are not
    interchangeable at the last bit (section 4). [read]
