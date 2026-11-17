# Phase 3 research: inline runs, images, fonts (kfxgen 0bc705d)

Sources read: `p3/src/{converter.py, inline_style.py, font_table.py, image_optimize.py}` at the pin;
Rust `crates/deckle-kfxgen/src/generate/{inline.rs, images.rs, typography.rs, fonts.rs, py.rs}`,
`book.rs`, `DIVERGENCES.md`; dumped models under `fixtures/kfxgen/models/`; fontTools 4.60.1 from
`/opt/homebrew/lib/python3.14/site-packages` (not Calibre's bundled copy).

Legend: **[read]** = verified by reading code or dumps; **[ran]** = checked by running plain Python
(no Calibre, no kfxgen); **[mem]** = from memory or inference, not verified here.

`_img_tokens.py` and `math_draw.py` were not in the extracted set. The token format below comes
from `_make_img_token` (converter.py:1359) and the Rust regex (`images.rs:28`).

---

## 1. `_walk_inline` and what lands in the model

### Signature and return  [read]
`_walk_inline(elem, flags=frozenset(), style_resolver=None, is_root=True, base_href=None,
split_tables=False, parent_emphasis=None)` (converter.py:2490). It returns a **list** (not a
generator) of `(segment, flags)` pairs:

- `segment` is a `str` (it can be `""`), or the sentinel `_TABLE_SPLIT` (an `object()`, :2149)
  paired with a `<table>` element when `split_tables=True` (:2815).
- `flags` is a `frozenset` whose members are strings or 2-tuples `(kind, value)`.

Callers pass the list straight to `normalize_runs_with_anchors` (inline_style.py:139): leaf
blocks at :6411, cells at :4402, and the container paths at :5921, :6966 and :7051 (block
walk's scope).

### Segment kinds the walk emits  [read]
| Segment | Where | Flags |
|---|---|---|
| text, tail | :2812, :2896 | `cur`, the element's accumulated flags (a tail carries the **parent's** `cur`, #59) |
| anchor mark `("", {("anchor-mark", id)})` | `make_anchor_mark`, inline_style.py:113 | id from `id=` or `<a name=>` (:3272) |
| `<br>` → `(" ", flags - ws)` | :2526 | |
| block-drawn span → `("", {"line-start"})` before and after | :2721, :2919 | |
| cell edge `(" ", ∅)` before and after `td`/`th` (not root) | :2729, :2917 | |
| block child in an inline walk `(" ", ∅)` around it | :2856–2876 | |
| `<q>` marks `(open, cur)` / `(close, cur)` | :2804, :2897 | |
| `<img>` → token, flags = **only the link flags** of `cur` | :2821–2831 | |
| `<svg>` with `<image>`s → token per image, size `h=100%`, flags ∅ | :2848 | |
| `<svg>` `<text>` → `(" ", ∅)`, marks, `(text + " ", ∅)` | :2851 | |
| `<svg>` shapes only → token with href `kfxgen-inline-svg/<sha1[:16]>.svg` | :2835 | |
| tcy → `("￼", common_flags | {("tcy", (serial, text))})` | :2760–2803 | |
| `<math>` → `_math_parts` | :2755 | |
| hidden element → only anchor marks of its subtree | :2519 | |
| comment / PI → `[]` (its tail is kept by the caller) | :2512 | |

### Flags  [read]
Strings: `italic`, `bold`, `not-bold`, `not-italic`, `super`, `sub`, `text-top`, `text-bottom`,
`underline`, `line-through`, `noteref`; nested shifts `"sub@2"`, `"super@3="`, `"baseline@2"`
(`_nest_shift`, :97; `=` = keeps the parent's size). `ws-pre`, `ws-pre-line`, `line-start` and
the anchor marks are consumed by `normalize_runs_with_anchors` and never reach the model.

Tuples `(kind, value)`:
- `("link", target)`: `"<file>#<frag>"`, `"<file>"`, or `"url:" + href` for http/https/mailto
  (:1947, :1955). The file part is `unquote(posixpath.normpath(join(dirname(base), href)))`, the
  fragment is unquoted, and case is kept (`links` model holds `C2.XHTML#T`). An inner `<a>` drops
  the outer link, its `noteref` and its link colour (:2572–2575).
- `("ruby", (serial, reading))`, `("tcy", (serial, text))`, `("math", (serial, drawing, alt,
  source))`, `("bidi", (serial, depth, (("direction", v), ("lang", v), ("unicode_bidi", v))))`
  with the keys sorted (inline_style.py:432). The serials come from **process-global**
  `itertools.count()`s (inline_style.py:326, :349, :376; converter.py:814).
- `("lang", "fr")`; `("mono", ("family", ...))`; `("size", float_px)`.
- Typography: `("small-caps", "small-caps"|"normal")`, `("transform", kw|"none")`,
  `("spacing", (mag_str, unit_sym))` (`("0","$308")` for normal), `("colour", argb_int)`,
  `("orientation", kw|"mixed")`, `("emphasis-marks", str|"none")` (:2659–2698).
- `("link-colour", (argb_or_None, argb_text))` (:2693).
- `("picture", ...)` is defined (inline_style.py:401) but **never produced** at the pin (grep:
  no caller). Rust refuses it as `Unsupported("images")` (inline.rs:173).

### `normalize_runs_with_anchors` contract  [read]
Input segments → `(text, spans, anchors)` (inline_style.py:139–262):
- It iterates by **code point**. Offsets in `spans` and `anchors` are code-point indices.
- Collapsible whitespace is ASCII `" \t\n\r\f"` only (:82). Not `\v`, not NBSP.
- Leading whitespace is dropped (`prev_space=True`); trailing `" "`/`"\n"` are stripped.
- ` ` (any literal one in the book's text too, not only `<br>`) and, in pre/pre-line, `\n`
  become `"\n"`. A break before any text is dropped, and spaces before a break are dropped.
  `line-start` adds a `"\n"` with **empty** flags unless the last char is already `"\n"`; a
  `<br>` straight after it adds nothing (`line_started`).
- In pre: spaces kept, tabs expand to the next multiple of 8 from the line start, `\r` dropped.
  Afterwards a kept space at a line start, after an NBSP, or beside another kept space becomes
  NBSP (:235–243).
- A collapsed space carries **its own segment's** flags (:220).
- `spans` = maximal runs of equal, non-empty flag sets: `[(start, length, frozenset)]`.
- `anchors` = `{id: offset}`: the first declaration wins, clamped to `len(text)`.

The block dict then holds `text`, `spans` (list of 3-tuples), and the anchors as
`anchor_offsets`/`anchor_keys` (keyed by `<file>#<id>` in the block walk; seen in the models).

### Image token  [read]
`"\x00IMG\x01" + href + "\x01" + alt + ["\x01" size ["\x01" float ["\x01" extra]]] + "\x00"`,
trailing empty fields dropped (:1359–1377). `\x00`, `\x01`, `\x02` are stripped from href and alt;
alt spaces become `\x02`. Size: `w=<raw>` or `h=<raw>` from computed width, else height, else the
attributes, matching `^\d+(?:\.\d+)?(?:%|px|em|pt)?$` (:1382–1403). Float: `left`/`right` from
the img's own float, or from a floated wrapper whose only child is the img (then the size becomes
the wrapper's width when it was None or `w=100%`) (:1447). Extra: `h=<raw>;va=<kw|length>`
(:1418). The href is `_resolve_img_src`: resolved against the file, or the raw `src` when that
fails (:1873). Normalization can rewrite characters inside a token: a tab or newline in alt
collapses to `" "`, and in pre text tabs expand.

### Cross-check with the Rust generator  [read]
`inline.rs::read_spans` (:197) and `flag_of` (:134) accept exactly the kinds above, with three
exceptions:
- any `@` flag → `Unsupported("a shift inside another")` (:149);
- `("math", ...)` → `Unsupported("math")` (:215);
- `("picture", ...)` → `Unsupported("images")`.

It also refuses two of one kind on one run (two sizes, two languages, ...). The converter can
legitimately produce two `("link", t)` flags only through odd markup: `_walk_inline` removes the
outer link. `link_target` takes `min` (#759).

`size` values in Calibre-path models are always floats (`24.0`, `13.333`). They come from the
resolver's `round(px, 3)` (:1073, :902). The port must emit `PyValue::Float`, not `Int`.

Calibre-path models exercise these flag kinds (my scan of 28 models) [ran]: bidi (directions);
bold; colour (colour, dark_boxes, link_colours, typography); emphasis-marks (ruby_tcy); italic;
lang (directions); link (many); link-colour (link_colours, link_greys); mono (links); noteref
(notes); orientation (ruby, vertical); ruby (links, ruby, vertical); size (6 fixtures);
small-caps, spacing and transform (typography, figures); super (links); tcy (ruby_tcy,
horizontal_blocks*); underline (links). **None** has `not-bold`/`not-italic`, `sub`,
`text-top`/`text-bottom`, `line-through` or a nested shift. Those exist only in the
argument-path `inline_runs`, which the converter cannot be tested against. Serials in every model
run 0, 1, 2… in reading order [ran].

### Keys the walk reads from the style resolver  [read]
`font-weight`, `font-style`, `font-family`, `font-size` (float px), `font-variant`,
`text-transform`, `letter-spacing`, `color`, `background-behind` (0xRRGGBB or None),
`text-orientation`, `text-emphasis-style`, `text-decoration` (`_decoration_lines`: the union up
the ancestors, :909), `vertical-align`, `display`, `white-space`, `own-direction`,
`own-unicode-bidi`, `q-css`, `first-letter`, `text-combine-upright`, `width`, `height`, `float`,
`clear`, `background-image`, `background-repeat`. The resolver (`_build_style_resolver`,
:1035–1250) is not in my scope, but phase 3 needs it **before** the inline walk. The walk checks
`"text-decoration" in own_css` and `prop in own_css` (:2585, :2628): with Calibre the key is
always present, even when None. The Rust resolver must keep "present with None" apart from
"absent".

---

## 2. Text-handling hazards

1. **Code points, not bytes** [read]. Every offset (spans, anchors, `_first_letter_length`,
   tab stops, `text[2:]`) is in code points. Build `Vec<char>` or track char counts. `py::len`,
   `py::take` and `py::skip` already exist (py.rs:126–136).
2. **Two whitespace sets** [read][ran]. `normalize_runs` collapses ASCII only. Many other places
   use Python's `str.split()`/`strip()`, which strip `isspace` chars: NBSP, U+1680, U+2000–200A,
   U+2028/9, U+202F, U+205F, U+3000, U+0085 and **U+001C–001F** (checked: `'\x1c'.isspace()` and
   `'\xa0'.isspace()` are True). Those places: tcy text (:2761), ruby reading (:2939), drop-cap
   letters (:2141), svg `<text>` (:1597), math tokens and alt (:3001), `css_small_caps`,
   `css_text_emphasis`, `text-decoration.split()`, and the block test `text.strip()` (an NBSP-only
   paragraph is a spacer). Rust's `char::is_whitespace` lacks U+001C–001F, so use `py::is_space`,
   `py::strip` and `py::without_spaces`. `strip(_COLLAPSING)` (:2275) is ASCII only.
3. **`unicodedata`** [read]. `bidirectional(ch)` in `_first_strong` (:769) only needs the classes
   L, R and AL. `category(ch)[0]` in `_first_letter_length` (:2046) needs P, Z, L and N; the
   `regex` crate's `\p{P}`, `\p{Z}`, `\p{L}`, `\p{N}` are Unicode 16 and match DIVERGENCES.md
   "Unicode version". No crate in `Cargo.lock` gives the bidi class directly except
   `icu_properties 2.3.0`, which is present but transitive (its Unicode version is unverified).
   Recommendation: a generated L/R/AL range table in `py.rs`, made from Python 3.14's
   `unicodedata` (Unicode 16) as the existing tables are. No NFC/NFD normalization happens
   anywhere in my scope, so `unicode-normalization` is not needed.
4. **Regex semantics** [ran]. Python `\d` matches any Nd digit (`'١٢'` matches `^\d+$`), and so
   does Rust's Unicode `\d`. But Python `$` also matches before a final `\n`
   (`re.match(r'^\d+$','12\n')` is True), and Rust `$` does not. Most inputs are stripped first
   (:1400, :1425, :1464), so the difference is contained; port each `$` knowingly. The Rust
   generator already refuses non-ASCII digits in sizes (`frames.rs:72`, `images.rs:585`), so
   `w=١٠٠px` gets through the converter and then fails in the generator. Python `\s` includes
   U+001C–001F and Rust's does not.
5. **Number parsing** [ran]. `float()` accepts `1_0`, surrounding whitespace, `inf`/`nan` and
   non-ASCII digits. `int()` accepts `' +7_00 '` (`parse_weight`, font_table.py:233). Use
   `css::style::py_float` (style.rs:530) and write a matching `py_int`.
6. **Lowercasing** [mem]. `str.lower()` and Rust `to_lowercase` both apply the full Unicode
   mapping. They agree on everything here except edge cases such as final sigma, which Rust
   handles and Python does not. Only CSS keywords and `lang` are lowercased.
7. **Sentinels in book text** [read]. A literal ` ` in the text is a line break. A literal
   `￼` reads as an object mark in `_last_shown` (:2196). A literal `\x00`/`\x01` outside a
   token can make `IMG_TOKEN_RE` match text the book wrote; Python has the same behaviour, so
   copy it.
8. **`unquote`** [mem]. `urllib.parse.unquote` decodes UTF-8 with `errors="replace"`. `images.rs`
   has an `unquote`; reuse it rather than `percent-encoding`.
9. **Global state** [read]. The serial counters, `_Q_DOCUMENT` (:2462, a cache keyed by the
   document root's identity) and `_svg_picture_href`'s hash. To match a dump, the serials must
   advance exactly as Python's do. A second walk of the same element (title probing, label
   texts) would use up serials. Check the block walk's callers for re-walks.
10. **`lxml.itertext()`** [mem, unverified]. Used for tcy, ruby, svg and drop caps. Whether it
    yields comment and PI text must match Phase 2's DOM API. I could not run lxml here (not
    installed).

---

## 3. Floats and colours written into the model  [read]

| Value | Code | Python formatting |
|---|---|---|
| run size | resolver :1073, first-letter :902 | `round(px, 3)`, a float (`py::round` exists) |
| letter-spacing px | inline_style.py:608 | `f"{float(mag) * 0.45:.4g}"` → str (`py::format_g`) |
| CSS length mag | inline_style.py:291–295 | in/cm/ex: `f"{v*factor:.4f}"`, then `rstrip("0").rstrip(".")` on any mag with a `.` |
| colour, text | `css_text_colour` :783 | ARGB int; 30-step float bisection; `int()` truncates when darkening, `math.ceil(c*hi - 1e-9)` when lightening; `** 2.4` |
| colour, blend | `css_rgb` :720–724 | `round(c*alpha + 255*(1-alpha))`: **banker's rounding** (`round(2.5)==2`, checked) |
| colour, % channel | :687 | `min(int(float(x[:-1]) * 256 // 100), 255)`: float floor division |
| colour, hsl | :696 | `colorsys.hls_to_rgb`, then `round(c*255)` (banker's) |
| link colour | :829 | `0xFF000000 \| rgb`, not darkened |
| svg size hint | :1671 | `f"{v*k:g}em"` |
| svg picture size | :1788–1792 | `f"{w:g}"`, `viewBox "0 0 {w:g} {h:g}"`; these bytes feed the sha1 href |
| display_dims | :9239, :9299 | an SVG file: Qt's `(int, int)`; an inline svg: `(float, float)` = own/scale |

`typography.rs::on_page` (:213) already ports the darkening branch against the white page. The
converter also needs the box branch: lighten, `darken` chosen by contrast, and the `_scaled`
254-channel check (inline_style.py:753–816). Keep the operation order: `lin()`'s `** 2.4` is
libm `pow` in both languages.

---

## 4. Images

### Pipeline  [read]
`convert_oeb_to_kfx` (:9526–9545):
1. `extract_cover_image` (:9368). Method 1: `metadata.cover[0]` as a manifest id, then as an href.
   Method 2: guide types `cover` / `other.ms-coverimage-standard`. Method 3: the first manifest
   image with "cover" in its id or href. Each candidate must be bytes, **over 100 bytes**, with
   JPEG (`FF D8 FF`) or PNG (`89 PNG`) magic. A GIF cover is no cover.
2. `extract_images_from_oeb` (:9192): every manifest item whose media type contains `"image"`:
   - `image/svg+xml` → `svg_to_png` (Qt `QSvgRenderer` and calibre `rasterize_svg`) at scale 2,
     or 1 for an SVG shown in a table cell (`_pictures_in_table_cells`). The box is
     `min(scale*max(own), 1920)` and `display_dims[href] = own`.
   - JPEG/PNG magic → bytes kept as they are.
   - GIF87a/89a → `gif_to_png` (calibre `scale_image` at its own size, `as_png=True`).
   - anything else → dropped with a warning.
   - Then `_inline_svg_pictures`: every shapes-only `<svg>` in spine pages (not nested in another
     svg, not a page that is itself an SVG), keyed `kfxgen-inline-svg/<sha1(lxml tostring)[:16]>.svg`,
     drawn at 4× (`_INLINE_SVG_SCALE`), with `display_dims` = own/4.
3. `cmyk_to_rgb` (image_optimize.py:104): a JPEG whose SOF has 4 components → Qt `RGB32` →
   JPEG at quality `KFXGEN_IMAGE_QUALITY` (default 85).
4. `_check_and_optimize`: with `kfxgen_embed_original_images`, only `decodes()` filtering.
   Otherwise `optimize_images`:
   - size from GIF/PNG/JPEG headers (any SOF in `_SOF_MARKERS`);
   - unchanged when `max(w,h) <= KFXGEN_IMAGE_MAX_DIM` (1920, range 16–20000) **and**
     `len <= KFXGEN_IMAGE_MAX_BYTES` (1 MiB, range 1 KiB–1 GiB), as long as it decodes;
   - otherwise `scale_image` into a box of `(1920,1920)` when over the dimension cap, else its own
     size (re-encode only), as PNG or JPEG matching the source, quality 85;
   - a scaled PNG without alpha or tRNS (IHDR colour type 0 or 2): the smallest of RGB PNG,
     palette PNG (256 colours, undithered, when the source was a palette) and **JPEG**;
   - re-encoded only for bytes → the original is kept when the result is not smaller;
   - undecodable → dropped (cover → None).
5. Model: `cover_image` = bytes or None, `images` = `{manifest href: bytes}`, `display_dims` =
   `{href: (w, h)}`, `resolved_image_refs=True`.

### What Rust can match exactly
- **Keys** [read]: manifest hrefs as Calibre writes them (e.g. `p.png` or `OEBPS/p.png`) plus the
  inline-svg keys.
- **Which images are kept** [read]: decided by magic bytes, media type and `> 100` bytes for the
  cover. `decodes()` (Qt) cannot be matched exactly: Qt fills in a truncated JPEG, and
  zune-jpeg's tolerance differs [mem]. Treat a decode failure as "dropped" and record the
  difference in DIVERGENCES.
- **Order** [read]: does not matter. The generator sorts `images` by href (images.rs:345).
  Calibre's manifest iterates a set, so the dict order changes between runs [mem; the README
  saw pg74's order change between dumps]. Compare `images` as a map.
- **Cover sharing** [read]: the generator finds the cover's copy in `images` by **byte
  equality** (images.rs:351). Any re-encode must give the cover and its manifest copy identical
  bytes. A deterministic encoder run on identical input does that; keep it that way.
- **Dimensions**: pass-through is exact. A resized image gets exact dimensions if calibre's
  `fit_image` is ported (floor of the scaled side [mem, unverified]). SVG dimensions come from
  Qt's `defaultSize` and `QSize.scale(KeepAspectRatio)`, which can be computed but only with
  Qt's unit rules (pt read as px, no em; converter.py:1693).
- **Format**: JPEG stays JPEG. GIF and SVG become PNG. A **PNG over the cap can become a JPEG**
  depending on which encoder output is smallest, so the format itself can differ from Python's.
- **Bytes**: exact only for pass-through. Every Calibre-path fixture is pass-through [ran]: all
  `images` and the one cover (`page_list`) equal the EPUB's files byte for byte. No fixture has
  an SVG, GIF, CMYK or oversize picture, or `display_dims` entries.

### Dependencies  [read Cargo.lock / Cargo.toml]
Present: `image 0.25.10` (workspace: `png`, `jpeg`; `gif` is enabled by deckle-comic and
deckle-digest), `png`, `gif 0.14.2`, `zune-jpeg 0.5.15`, `color_quant`, `flate2 1.1.10`, `brotli
9.0.0`, `ttf-parser 0.25.1`, `sha1 0.11` (workspace), `icu_properties 2.3.0`. Absent: `resvg`,
`usvg`, `tiny-skia`, `woff2`.

**Smallest set:** `image` (workspace features, plus `gif` if GIF→PNG is ported) and `sha1`.
Nothing new.
- SVG (manifest SVG files and shapes-only inline `<svg>`): typed `Unsupported("SVG pictures")`
  in phase 3. resvg would draw differently from Qt anyway; Qt ignores much of SVG [mem]. That
  also avoids reproducing lxml's `tostring` bytes for the sha1 href. `<svg><image href=…>`
  wrappers and `<svg><text>` need no rasterizing and can be ported.
- CMYK JPEG: port with `image` (zune-jpeg converts CMYK/YCCK to RGB [mem, unverified]) and the
  `JpegEncoder` at quality 85. Dimensions and format match.
- Oversize/overweight: port with `image` resize (filter choice is free: bytes are not compared)
  and `fit_image` dimensions. Skip the palette step and record the possible PNG/JPEG choice
  difference in DIVERGENCES. Making it `Unsupported` instead would refuse every book with a
  >1920 px or >1 MiB picture, which is common.
- GIF → PNG: port. It is cheap: first frame at the logical-screen size.
- Env vars `KFXGEN_IMAGE_MAX_DIM`/`_MAX_BYTES`/`_QUALITY` and `KFXGEN_MAX_FONT_BYTES`: take the
  defaults as constants. Deckle has no reason to read kfxgen's env vars (lead's call).

---

## 5. Fonts (font_table.py, `_font_table_for` :9473)

### End to end  [read]
1. `build_font_table`: the `@font-face` rules of every spine item's Stylizer, in spine order,
   **with repeats** (`Book.font_face` holds the same; book.rs:44–47). `kfxgen_disable_font_embedding`
   → `FontTable([])`.
2. `build_manifest_lookup`: href → bytes for every manifest item whose data is bytes, plus a
   basename fallback where the **last** item of a basename wins (dict overwrite, :447).
3. `faces_from_rules` (:300):
   - family = `normalize_family` (strip, strip quotes, strip, lower); a rule without one is
     skipped;
   - weight = `parse_weight` (`bold` 700, `normal` 400, `int()`, else 400); italic =
     `italic`/`oblique`;
   - srcs = every `url(...)` in `src` (`_URL_RE`), or the bare value, whose bytes are TTF/OTF
     (`00 01 00 00`, `OTTO`, `true`, `ttcf`) or WOFF/WOFF2;
   - a `(family, weight, italic)` key that some rule gives as TTF/OTF skips the rules that give
     it only as WOFF (`sfnt_faces`, computed over **all** rules first);
   - TTF/OTF srcs first, in the author's order (a stable sort on `is_woff`);
   - per src: WOFF unpacked once per href (cached, errors cached too); a size over
     `MAX_FONT_BYTES` (50 MiB) skipped; `_check_sfnt` (table directory within the file, required
     `cmap head hhea hmtx maxp` and one of `glyf`/`CFF `/`CFF2`; `ttcf` not checked); the first
     that passes is chosen;
   - a chosen href already used → skip the rule (dedupe by href, **after** choosing);
   - `Face(css_family, weight, italic, data, emitted_family, location="resource/font{idx}")`,
     idx counting kept faces only.
4. `emitted_name`: `_slug(family)` (`[^a-z0-9]+`→`-`, stripped; a non-ASCII name gets
   `-` + `sha1(utf8)[:8]`, or `font-<hash>` when nothing is left) + `-{weight}` + `i` when
   italic, deduped with `-1`, `-2`…
5. `FontTable.match` (the generator's job, already ported in fonts.rs).

### In the model  [read]
`{"$obj": "kfxgen.font_table.FontTable", "state": {"faces": [Face...], "_by_family": {family:
[Face...]}}}` (fonts, font_faces). Each Face's state is `css_family, weight, italic, data
($bytes), emitted_family, location`, in that order. `fonts.rs` reads `faces` (:110) and rebuilds
`_by_family` itself (:167). To match a dumped model exactly, the converter must also produce
`_by_family` (insertion order = first appearance).

### WOFF / WOFF2
- **WOFF1 can be done with flate2 alone and byte-identical** [read in fontTools 4.60.1; Calibre's
  bundled version unverified]. `TTFont(...).save(out, reorderTables=None)` (font_table.py:212)
  writes with `SFNTWriter`:
  - tables are copied raw (none is decompiled: `recalcBBoxes=False`, `recalcTimestamp=False`,
    lazy load);
  - **write order** = `sortedTagList`: sorted, `DSIG` moved last, then `TTFTableOrder` (`head
    hhea maxp OS/2 hmtx LTSH VDMX hdmx cmap fpgm prep cvt  loca glyf kern name post gasp PCLT`),
    or `OTFTableOrder` (`head hhea maxp OS/2 name cmap post CFF `) when `CFF ` is present, the
    rest after. Each table first writes its `dependencies` recursively (`head`→maxp, loca,
    CFF , CFF2; `hhea`→hmtx, glyf, CFF , CFF2; `maxp`→glyf; `loca`→glyf; `glyf`→fvar;
    `OS/2`→head; `name`→ltag; `vhea`→vmtx…; ttFont.py:699, tables/*.py);
  - data padded to 4 bytes; the directory sorted by tag; the usual searchRange fields; per-table
    checksums with `head`'s checkSumAdjustment zeroed; the master checksum written into `head`
    (sfnt.py:272–370).
  - Implementing the WOFF1 checks (:83–116) plus this writer is about 200 lines of Rust.
- **WOFF2 needs a table-transform implementation**. glyf/loca (and possibly hmtx) are stored
  transformed, and fontTools rebuilds and **recompiles** them with its own glyf compiler, so
  matching bytes means porting that compiler. No woff2 crate is in the lock. Recommend
  `Unsupported("WOFF2 font")` when a WOFF2 src would be chosen. Not "skip": kfxgen embeds it
  when Calibre has brotli and fontTools [mem]. Plain TTF/OTF is unaffected.
- TTF/OTF bytes are embedded as they are, so they are byte-identical by construction (`fonts`
  fixture: the face's sha256 equals the EPUB's `LiberationSerif-Regular.ttf` [ran]).
- `ttf-parser` is not needed: `_check_sfnt` is plain big-endian reads.

---

## 6. Proposed port tasks (ordered by dependency)

Python line counts are approximate and include comments.

**T1. inline_style primitives** (no DOM). inline_style.py:17–994 (~900 lines, minus the colour
table's formatting): `normalize_runs_with_anchors`, `parse_css_length`, `parse_vertical_align`,
`_parse_font_family`, `is_monospace`, `css_*` typography, `_rgb_alpha`/`css_rgb`/
`css_text_colour`/`css_link_colour`/`is_transparent`, plus font_table's `parse_weight`/
`parse_style`. Tests: unit tables. The cheapest oracle is a table generated by running the pinned
`inline_style.py`, which is pure Python with no Calibre (needs the lead's OK, since it runs
kfxgen code). Otherwise hand-written cases from the doc comments (P734 values etc.). It reuses
and extends `typography.rs` luminance.

**T2. Inline walk core.** converter.py `_walk_inline` :2490–2922 (430), shifts :97–136, links
:1841–1990 (150), white space and emphasis :1994–2040, link colour helpers :2152–2280, block
display :2282–2298, quotes :2306–2488 (180), ruby :2924–2953, anchors :3272–3291, `_is_hidden`/
`_epub_switch_branch` :4908–4945, inline bidi :752–814. About 1,150 lines. `<math>` →
`Unsupported("math")` at :2755. Shapes-only `<svg>` → `Unsupported("SVG pictures")`. Depends on
T1 and on the resolver dict (block-walk researcher). Fixtures: links, link_colours, link_greys,
typography, colour, dark_boxes, directions, ruby, ruby_tcy, vertical, font_sizes, notes, rtl,
figures. **Intermediate dump:** a 4th `inject.py` hook that wraps `converter._walk_inline` and,
for `is_root=True` calls, records `(base_href, elem.getroottree().getpath(elem), walk output
less the `_TABLE_SPLIT` elements, normalize_runs_with_anchors(output))`. Recursive calls go
through the module global, so the wrapper sees them, and filtering on `is_root` keeps the dump to
blocks. This tests T2 before the block walk exists. It could be split T2a (emphasis, white space,
links, anchors, shifts, cells, quotes) and T2b (typography, colour, link colour, mono, size,
bidi, lang, ruby, tcy) with the same dump.

**T3. Initials and drop caps.** :2041–2145 (~105). Needs T2 and the resolver's `first-letter`.
Fixtures: drop_caps, typography. Tested at block level (`block_style.drop_cap`, the first
span), or by adding the post-`_initial` spans to the T2 dump.

**T4. Picture tokens in text.** :1296–1611 (~315): `_make_img_token`, size hint, inline extras,
`_img_float`, `_resolve_img_src`, `_css_background_image`, `_svg_image_refs`,
`_svg_text_paragraphs`, `_svg_draws_shapes_only` (detection only), `_page_shows_something`.
Needs T2. Fixtures: figures, floats, floats_697, frames, horizontal_blocks, horizontal_blocks_ltr
(tokens in `text`), page_list (cover). Tested through the T2 dump (tokens are in the text).

**T5. Image set and cover.** :9192–9452 (~260) + image_optimize.py (400). It needs only Book's
manifest, guide and metadata, so it can run **in parallel with T1–T4**. Pass-through, magic
filtering, GIF→PNG, CMYK and oversize re-encoding with `image`; SVG → Unsupported. Test: the
`images` keys and bytes (exact for pass-through), `cover_image`, `display_dims` against figures,
floats, floats_697, frames, horizontal_blocks*, page_list. Oversize, GIF and CMYK have **no
Calibre-path fixture**: they need new sources through `kfxgen_models.sh` (each compared by
dimensions and format), or unit tests only.

**T6. Font table.** font_table.py (506) + :9473. Needs Book.font_face and manifest bytes.
**Parallel** with the rest. WOFF1→sfnt byte-exact with flate2; WOFF2 → Unsupported. Test: a
`FontTable` PyValue equal to the `fonts` model (faces **and** `_by_family`). `font_faces` is
argument-path, so use it only as a shape reference. WOFF1 has no fixture: add a synthetic EPUB
whose WOFF1 is made with fontTools, and check the unpacked bytes against `font.save`, run outside
Calibre (lead's call).

**T7. Block-level direction helpers** (they may belong to the block walk). `_bidi`,
`_with_bidi`, `_start_end_align`, `_starts_a_language`, `_pictures_among_words` :691–880 (~150).
Fixtures: directions, rtl, vertical. Needs the L/R/AL table from T2.

**T8 (later, optional). Math text fallback** for the spec's `auto` mode ("its text/alt text"):
`_math_reading`/`_math_row`/`_math_elem` :3014–3270 (~255) give kfxgen's own undrawn text.
`alttext`, else whitespace-joined token text, is the minimal stand-in.

Order: T1 → T2 → {T3, T4, T7}; T5 and T6 are independent and can start first.

---

## 7. Surprises and likely porting mistakes

1. **Nested shifts are common, and the generator refuses them.** `<sup><span
   style="vertical-align:super">1</span></sup>` (publisher footnote markup) gives `"super@2"`,
   and a `vertical-align: baseline` span inside `<sup>` gives `"baseline@2"`. `inline.rs:149`
   returns `Unsupported("a shift inside another")`, and no Calibre fixture exercises it. Expect
   corpus books to hit it once the converter exists. The generator side of #536/#638 is
   outstanding work.
2. **A tail carries the parent's `cur`**, and an `<img>` token carries only the link flags
   (:2830). Bold or italic around a picture is not on its token.
3. **`text-decoration` is the ancestors' union** (`_decoration_lines`), applied at the root
   too. An underlined paragraph becomes one underline span over its whole text, not a block
   style.
4. **NBSP**: kept by `normalize_runs`, stripped by `.split()`/`.strip()` elsewhere. Using one
   Rust whitespace predicate everywhere would get one side wrong.
5. **`size` must be a float** and **colours ints**. `display_dims` mixes int pairs (SVG file) and
   float pairs (inline svg).
6. **Serials are process-global and walk-order dependent.** A port that numbers per chapter or
   per block will pass KFX tests (only relative order matters to the generator) but fail
   model-equality tests. Re-walks consume numbers.
7. **`images` order is meaningless** (Calibre's set and the generator's sort). Do not spend time
   reproducing it. **Cover sharing is by byte equality**, so cover and manifest copy must be
   re-encoded identically.
8. **Image format can flip PNG→JPEG** for an oversized opaque PNG (smallest-of rule), and
   "keep the original if not smaller" depends on encoder sizes. Both differ in Rust by design.
9. **The font-lookup basename fallback is last-wins**, and the face dedupe is by *chosen href*
   after the TTF-before-WOFF sort. `sfnt_faces` is computed over all rules before the loop.
10. **WOFF1 bytes are reproducible** (fontTools copies tables raw, in its own order); WOFF2's are
    not without porting fontTools' glyf compiler.
11. **Math has a stray-text branch that never calls `draw_math`** (:2996), and kfxgen also falls
    back to text when `draw_math` returns None. Since Rust cannot tell whether `draw_math` would
    succeed, raise Unsupported for every `<math>`, including one inside `<epub:default>`
    (:2532). `_label_texts` (:3032) also reads math for contents labels (block walk's scope).
12. **`_svg_picture_href` hashes `lxml.etree.tostring` output** (namespace declarations,
    attribute order, empty-element form). Even with resvg, matching the token href needs
    lxml-exact serialization: another reason for Unsupported.
13. **`_q_marks` counts quote depth across the whole document** in `iterwalk` order, not per
    paragraph, and reads `lang` only, never `xml:lang` (:668). The bidi language reads both.
14. **The `<pre>` leading newline** is removed only from the `<pre>`'s own `.text` (`\r\n` or
    `\n`), not from a child's.
15. **`_draws_as_block` uses `css` only for non-root elements** (`css` stays None at the root),
    and excludes cell, body-block and table tags (shared constants with the block walk).
