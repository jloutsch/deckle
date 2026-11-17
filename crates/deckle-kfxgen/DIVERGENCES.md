# Where the port differs from kfxgen

Deliberate differences between `deckle-kfxgen` and kfxgen `ad38ec8`, each with why. Line
citations are at `4b81651`, an earlier pin, unless a section names another commit.

## Unicode version of Python's character classes

kfxgen reads characters through Python: `str.isspace`, `isalpha`, `isalnum`, `isdigit`,
`lower`, `unicodedata.east_asian_width`, `bidirectional` and `decimal` (what `\d`, `int()` and
`float()` read as a digit), for instance when it checks that a chapter's text
opens with its title as whole words (`_opens_with_title`). Their answers follow the Unicode
version of the Python running kfxgen.

The port answers as Unicode 16.0, the version of calibre's Python (3.14), where kfxgen runs for
users: `src/generate/py.rs` holds tables made from it, and the `regex` crate's general
categories are also 16.0. kfxgen run under another Python can class a character added to
Unicode since then differently. The shim fixtures were made with Python 3.13 (Unicode 15.1);
none of their text holds such a character.

## Superscript and subscript size overrides

kfxgen reads `KFXGEN_SUPERSCRIPT_FONT_SIZE` and `KFXGEN_SUBSCRIPT_FONT_SIZE` from the
environment for the size of a raised or lowered run (`superscript_metrics`,
`subscript_metrics`, `native_generator.py:785`, :807). The port always writes kfxgen's default,
0.75 (:691, :695), so its output depends only on the model. With either variable set, kfxgen's
book differs from the port's in those runs' `$16`.

## The page width percentages are measured against

kfxgen 0bc705d measures a percentage against Kindle Previewer's page: 384pt in a horizontal
book, 480pt in a vertical one (`page_scale.py`, #754). The width is a context variable, not an
argument: `convert` sets it around `generate_full_book`, 480pt when the book's writing mode is
vertical-rl (`converter.py:9567`), and `generate_full_book` called on its own measures against
384pt. The port sets it from the `writing_mode` argument the same way (`page_scale.rs`), so a
model `convert` made comes out the same. A vertical-rl model generated outside `page_width`,
as kfxgen's own unit tests do, is measured against 480pt by the port and 384pt by kfxgen.
Deckle's fixture tools replay every model inside `page_width`, as `convert` would.

## Block lengths deckle-kfx's decimal can't hold

A paragraph's indent and side margins come from the model as text, `(magnitude, unit)`, and
kfxgen writes the magnitude as `IonDecimal(magnitude)` (`build_fragment_157`,
`native_generator.py:3640`), any length of coefficient and exponent. deckle-kfx's Ion decimal
holds an i128 coefficient (38 digits) and an i32 exponent. A magnitude past those, such as 40
digits or `1.5e-2147483648`, is a `GenerateError::Model` naming the length, where kfxgen writes
a book. No CSS length reaches them. "inf" and "NaN" are model errors too, but there kfxgen
raises as well (`abs()` of a str when it serializes them).

## A box's width and float of another type

The converter writes a box's `width` and `width_px` as numbers or None and its `float` as
"left", "right" or None (`converter.py:6782`). kfxgen reads whatever the model holds: a truthy
`float` that isn't "left" is written as right (`build_box_style_157`,
`native_generator.py:3897`), and `width_px` is only divided by when a picture in the box has a
length for a width (`_size_in_box`, :236). The port reads `width` and `width_px` as numbers and
`float` as a string, and anything else is a `GenerateError::Model` naming the key, where kfxgen
can write a book.

## Runs past ±2^61 characters

A run comes from the model as `(start, length, flags)`, and kfxgen's ints have no bound. The
port holds them in an i64, subtracts text lengths from a run's ends (`_title_spans`,
`_trim_text`) and takes one end from another (`_direction_layers`). A run whose start or end
(`start + length`) is past ±2^61 is a `GenerateError::Model` naming the spans, where kfxgen
writes a book. A run's offsets count characters of the paragraph, so no converted book has one.

## Table cell runs that start outside the text

A run in a table cell, or in a paragraph of one, that starts before the text or after its end
is a `GenerateError::Model` naming the spans, where kfxgen writes a book. kfxgen cuts a
paragraph's runs to its text but not a cell's, and the positions map walks from the run's
start. No converted book has such a run: a run's offsets count characters of the cell.

## Units and font locations that name the book's own symbols

A length's unit and a font face's `location` come from the model as strings, and kfxgen writes
each as a symbol whatever it is. A unit that is not `$` and a number (KFX's own symbols, as the
converter writes them), a face's `location` that is not `resource/font` and a number (as
kfxgen's font table names them), and two faces of one location are each a
`GenerateError::Model`. Any other name can be one the book already has: a font located at `c0`
takes the first chapter's id, and one at a picture's is dropped with that picture.

## Run sizes and monospace families of another type

The converter writes a run's size flag as `("size", px)`, px an int or float (`css_font_size`,
`inline_style.py:513`), and its monospace flag as `("mono", families)`, a tuple of strings
(`make_mono_flag`, :496). The port reads exactly those; anything else is a
`GenerateError::Model` naming the runs. kfxgen raises on most of the rest (a string size, when
it sorts it among the paragraphs' sizes), but writes a book for some: a bool size, which Python
counts as 0 or 1, and families that aren't strings, which match no face.

## A font face of another type

`faces_from_rules` (`font_table.py`) makes each face's `css_family` a string, `weight` an int,
`italic` a bool and `data` bytes, and the converter passes the `FontTable` on unchanged. The
port reads exactly those; anything else, or an empty `emitted_family` or `location`, is a
`GenerateError::Model` naming `args.font_table`. kfxgen raises on a string weight
(`build_fragment_262`, `native_generator.py:2853`), string data (`IonBLOB`) and an empty name
(kfxlib refuses a symbol of zero length). It writes a book for an `italic` of 1, which Python
compares equal to True, and for a `css_family` that isn't a string, which no family list
names, so `FontTable.match` never picks it.

## Landmarks and page lists of another shape

The converter writes `landmarks` as `(kind, label, link key)` tuples of strings, the kind
"toc" or "start" (`_book_landmarks`, `converter.py:8905`), and `page_list` as `(label, link
key)` tuples of strings (`_book_page_list`, :8961). The port reads each as a list or tuple of
that many strings; anything else is a `GenerateError::Model` naming the argument. So is a
landmark of another kind that lands, where kfxgen raises `KeyError` (`build_fragment_389_toc`,
`native_generator.py:3198`). kfxgen raises on a key that isn't a string (`"#" not in key`) and
on an entry it can't unpack, but writes a book for a label of another type, which it writes as
that Ion type, and for an entry whose kind or label isn't a string but whose key lands nowhere.

## Directions and languages of another shape

The converter writes a `bidi` dict of `lang`, `unicode_bidi` and `direction` strings and a
`set_here` tuple of those names (`_bidi`, `converter.py:690`), and an inline element's
direction flag as `("bidi", (serial, depth, ((key, value), ...)))` of the same keys
(`make_bidi_flag`, `inline_style.py:429`). The port reads exactly those; anything else is a
`GenerateError::Model` naming the dict or the runs. So is a direction or unicode-bidi that
`_BIDI_SYMBOLS` (`native_generator.py:72`) doesn't have, even where kfxgen would not write it:
kfxgen raises `KeyError` only on one it writes, and writes a book for one `_bidi_diff` leaves
out. kfxgen also writes a book for a key it doesn't read, which it ignores, and for a value of
another type that `_bidi_diff` leaves out. A run carrying two languages alone is
`GenerateError::Unsupported`: kfxgen would write the first in set order.

## Rubies of another shape

The converter writes a ruby as `("ruby", (serial, reading))`, an int and a string
(`make_ruby_flag`, `inline_style.py:328`). The port reads exactly that; anything else is a
`GenerateError::Model` naming the runs, where kfxgen raises on most shapes it can't unpack
and writes a reading of another type as that Ion type. A run under two rubies is
`GenerateError::Unsupported`: kfxgen would take the first in set order.

## Vertical-text run flags of another shape

The converter writes a run's text-orientation and emphasis marks as `("orientation", value)`
and `("emphasis-marks", value)`, the value a string (`inline_style.py:547`, :550), and 縦中横
as `("tcy", (serial, text))`, an int and a string (`make_tcy_flag`, :378). The port reads
exactly those; anything else is a `GenerateError::Model` naming the runs, where kfxgen writes
nothing for an orientation or mark of another type but keeps the run's style apart, and raises
on most 縦中横 it can't unpack. A run carrying two of one kind is
`GenerateError::Unsupported`: kfxgen would take the first in set order.

## Link colours of another shape

The converter writes a link's colour as `("link-colour", (link, text))`, `link` an ARGB int or
None and `text` an ARGB int (`converter.py:2653`). The port reads exactly that; anything else
is a `GenerateError::Model` naming the runs, where kfxgen writes a value of another type as
that Ion type or raises measuring it against the page. A run carrying two link colours is
`GenerateError::Unsupported`: kfxgen would take the first in set order.

## Contents entries of another shape

The converter writes a chapter's `sub_entries` as dicts of a `title` string, a `key` string
or None and a `level` int (`converter.py:7705`, #363), and a contents line's `target_key` as a
string or None (:8665). The port reads exactly those; anything else is a `GenerateError::Model`
naming the entry, where kfxgen opens the chapter for a key of another type, writes a title of
another type as that Ion type, and raises on a missing key or title.

## Typography of another shape

The converter writes a block style's `small_caps`, `text_transform`, `text_orientation` and
`text_emphasis` as strings, its `letter_spacing` as a `(magnitude, unit)` pair of strings,
its `colour` as an ARGB int (`css_typography`, `inline_style.py:866`) and its `drop_cap` as an
int (`converter.py:6385`), and a run's `("small-caps" | "transform" | "spacing" | "colour", value)`
flags the same way. The port reads exactly those; anything else is a `GenerateError::Model`
naming the key or the runs, where kfxgen writes nothing for a keyword of another type but
keeps the style apart, writes a colour of another type as that Ion type, and raises on most
spacings it can't unpack. A run carrying two values of one kind is
`GenerateError::Unsupported`: kfxgen would take the first in set order. A drop cap whose
initial is so large that its line count passes 2^63 is a model error, where kfxgen writes
Python's unbounded int.

## Turned text, lists and picture places of another shape

The converter writes text set the other way from its line as `("turn", (serial, mode, text,
spans))`, an int, `"horizontal"` or `"vertical"`, a string and runs (`make_turn_flag`,
`inline_style.py:407` at ad38ec8); a list's `list` as a `type` that is one of KFX's own
symbols, a `start` int or None and an `inside` flag (`converter.py:6580`), an item's
`list_item` or `item` as `n` and `value` ints or None and an `empty` flag (:6627); and a block's
`picture_boxes` as `(left, right)` pairs, each a block style's `(magnitude, unit)` length or
None (:7400). The port reads exactly those; anything else is a `GenerateError::Model` naming the
key or the runs, where kfxgen raises on a mode it has no style for and on most shapes it can't
unpack, writes a list type of another name as a symbol (which can be one the book already has,
as for units), and writes a start or value of another type as that Ion type. A run carrying two
turned texts is `GenerateError::Unsupported`: kfxgen would take the first in set order. An item
numbered 2^63 − 1, whose next number passes an i64, is a model error, where kfxgen counts on in
Python's unbounded int.

## Contents nested more than 32 levels deep

kfxgen nests contents entries by level (`_nest_nav_entries`, `native_generator.py:2119`) and
writes them recursively; called directly under Python's default recursion limit (1,000), it
writes contents up to 162 levels deep and raises `RecursionError` at 163. The port writes them
recursively too, and a debug build on a 2 MiB thread overflowed its stack, aborting the
process, at 80 levels (72 passed). So contents nested more than 32 levels deep are
`GenerateError::Unsupported`, checked before any recursion: a book kfxgen writes at 33 to 162
levels is refused. No fixture or corpus book nests more than 3.

## Boxes nested more than 32 levels deep

kfxgen reads and writes a box's blocks recursively; called directly under Python's default
recursion limit (1,000), it writes the `boxes` fixture's box nested up to 245 deep and raises
`RecursionError` at 246. The port reads and writes them recursively too (`read_block` and
`read_box`), and a debug build on a 2 MiB thread overflowed its stack, aborting the process,
at about 60 levels (55 passed; a release build at about 250). So boxes nested more than 32
levels deep are `GenerateError::Unsupported`, checked while the model is read, before any
deeper recursion: a book kfxgen writes at 33 to 245 levels is refused. No fixture nests more
than 2, no corpus book more than 1. A model loaded from JSON can't reach the limit:
serde_json stops at 128 levels, which `load_model` reaches at 27 nested boxes.

## Three hand-made values the port reads differently

Found by running both generators on edited models (issue #13). The converter writes none of
them, and the port keeps its own behaviour:

- **A size too small to divide by.** A `font_size` of 5e-324 makes a ratio of infinity. kfxgen
  raises when it serializes it (`abs()` of a str, `ion_binary.py:728`). The port writes the
  infinite decimal as 0 and makes a book.
- **A font table whose `_by_family` disagrees with its `faces`.** kfxgen matches a family
  against `_by_family` as the model gives it. The port builds the same grouping from `faces`
  and does not read `_by_family`. The converter always derives one from the other.
- **A tate-chu-yoko run that starts before the paragraph.** Python's slice wraps a negative
  start round to the paragraph's end. The port clamps it to the start.

## The month of a year-only date

The `epub` module reads the book as Calibre 9.18 hands it to kfxgen (`oeb_view`). A `dc:date`
that gives only a year takes the 15th of the current month in Calibre: `utils/date.py`
`parse_date` fills what is missing from today's date, with the day set to 15. The port fills
January 15, so the same book gives the same date whenever it is converted. A year and month
take the 15th in both. The `cascade/` test compares such a date without its month
(`package_quirks` was dumped in November 2026).

## Dates in other forms

Calibre reads `dc:date` with `dateutil`, which accepts almost any form ("March 4, 2021",
"4/3/2021"). The port reads ISO 8601 forms only: a year, a year and month, a full date, then
an optional time and zone. A date in another form is skipped as one Calibre cannot parse is,
so the book may have no date where Calibre has one.

## Language codes

Calibre writes a book's languages as ISO 639-3 codes (`canonicalize_lang`). The port maps the
184 two-letter codes with Calibre's own table and keeps a three-letter code as it is. Calibre
keeps a three-letter code only when ISO 639-3 has it, and also reads English language names
(`english` as `eng`); the port drops a name and keeps any three letters.

## A book with no language

With no `dc:language` Calibre gives the book the language of its own interface (`get_lang`),
which depends on the computer it runs on. The port gives `en`, what Calibre gives on an English
system.

## An HTML cover with no cover image

When an EPUB 2 book's guide names an HTML cover page that opens the spine, and the book has no
cover image, Calibre renders that page to `calibre_raster_cover.jpg` and makes it the cover
(`epub_input.py` `rationalize_cover2`). The port cannot render HTML, so that book has no cover
image; the page still leaves the spine and becomes the title page, as in Calibre.

## Linked files added to the spine

Calibre adds the documents the spine links to, but does not list, at the end of the spine as
non-linear (`reader.py` `_spine_add_extra`). It collects them in a Python set, so when there are
several their order can change from one run to the next. The port adds them in the order it
finds them: spine order, then document order.

## Spine entries marked neither yes nor no

Calibre keeps a `linear` value other than yes, no, true or false as the string itself, which
kfxgen reads as true unless it is empty. The port's `linear` is a bool: false for an empty
value, true for any other.

## Which files stay in the manifest

After flattening the CSS, Calibre drops every manifest file nothing uses (`trimmanifest.py`).
Two of its inputs differ in the port:

- **Files a stylesheet names.** Calibre keeps what its flattened stylesheet names: the fonts
  of every `@font-face` rule and the images of the rules that matched an element. The port
  keeps every file named by a `url()` in the stylesheets and `<style>` elements of the
  flattened documents, so an image only an unused rule names stays.
- **Metadata values.** Calibre keeps a file whose href or id equals any metadata value
  (identifiers and timestamps among them); the port checks the seven fields it reads.

## Calibre steps the book view leaves out

These change the book only in uncommon cases and are not ported:

- Files the documents link to that the manifest does not list are added by Calibre with ids
  `added`, `added1`… (`reader.py` `_manifest_add_missing`).
  So a stylesheet a document links to that the OPF does not list is read and applied by
  Calibre's Stylizer; the port's `book::file_sheets` skips it.
- `data:` URLs become manifest files (`transforms/data_url.py`).
- With no title, Calibre takes the title sort (`calibre:title_sort` or the title's file-as) as
  the title before falling back to `Unknown`.
- Calibre reads an OEB 1 package (the `…/oeb-package/1.0` namespace) and Dublin Core 1.0 and
  0.9 elements as OPF 2 and DC 1.1 (`reader.py` `_read_opf`, `_clean_opf`). The port refuses
  an OEB 1 package and does not read those older Dublin Core elements.
- With no NCX, nav or HTML contents page, Calibre takes the contents from an OEB 1 `<tour>`
  (`reader.py` `_toc_from_tour`). The port generates them instead.
- With no guide cover, Calibre makes the largest of the Microsoft cover images the guide names
  (`other.ms-coverimage-standard` and the like) the cover (`transforms/guide.py` `Clean`). The
  port drops those references as types it does not know.

## An NCX listed in the spine as a document

With no `toc` attribute on the spine, Calibre reads the first manifest item of the NCX type
as the contents, after its reader has turned spine entries whose root is an XHTML `<html>`
into documents (`reader.py` `_spine_from_opf`, then `_find_ncx`). `read_package` finds the NCX
before that step. If the first NCX-typed item is such a spine document, Calibre reads the next
NCX-typed item and the port reads none.

## One page_styles.css until the CSS is read

Calibre writes one `page_styles*.css` per distinct set of `@page` and `@font-face` rules among
the files it flattens (`flatcss.py` `collect_global_css`). Telling those sets apart needs the
book's CSS, which the css module reads (phase 2 Tasks 5 to 7); until then `oeb_view` writes one
`page_styles.css`. The `cascade/` test allow-lists the others by id (`css_quirks` has three).
This entry goes when Task 7 makes them.

## XHTML that is not well-formed

Calibre reads a document that is not XML with `html5-parser` (`parse_utils.py` `html5_parse`),
and the port with `html5ever` (`src/dom/parse.rs`). Both follow the HTML 5 parsing algorithm, and
the book loads with all its text. On the `malformed` cascade book (tags left open, an unquoted
attribute) the two trees are the same; they can differ:

- `html5-parser` runs with `maybe_xhtml`, which reads `<p/>` as an empty paragraph;
  `html5ever` reads it as an opening tag, so what follows goes inside it.
- Names XML does not allow are made valid by replacing each bad character with `_`, which may
  not be `html5-parser`'s exact rewrite (`sanitize_names`).
- An attribute prefix (`epub:type`) gets its namespace only from a declaration on `<html>`.
- When the HTML tree is more than 100 levels deep, Calibre parses again with libxml2's HTML
  parser (`_html4_parse`). The port keeps `html5ever`'s tree up to `MAX_DEPTH` (256) and refuses
  the file past it (`dom::Error::TooDeep`), and also refuses one whose elements and attributes
  together outnumber its length in bytes plus 1,024 (`dom::Error::TooLarge`): the algorithm
  copies each open formatting element, attributes and all, into every paragraph that follows.
  Both stop work that would otherwise grow with the square of the file's size.

## Encodings

Calibre decodes a document or a stylesheet by its byte-order mark, as UTF-8, by the encoding it
declares, or else by uchardet's guess (`base.py` `OEBBook.decode`, `chardet.py`
`xml_to_unicode`, `force_encoding`). The port decodes both the same way (`dom::parse`,
`book::file_sheets`) and differs in two places:

- **No declaration.** The port has no detector and decodes a file that is not UTF-8 and
  declares nothing as windows-1252 (Python's `cp1252`), uchardet's answer for most Western text.
  Calibre can guess another encoding, or UTF-8 when uchardet has no answer.
- **Declared encodings.** UTF-8, UTF-16, Latin-1 and ASCII are decoded as Python does, under
  any name Python gives them. Any other name is looked up in the WHATWG tables (`encoding_rs`),
  which can map a name to a wider encoding than Python's codec of that name (`iso-8859-9` to
  windows-1254, `euc-kr` to windows-949, for instance) or decode a few bytes differently.
  windows-1252 keeps Python's U+FFFD for its five undefined bytes. A name neither knows, or one
  the WHATWG tables mark as unsafe (`iso-2022-kr`, `hz-gb-2312`), is decoded as UTF-8 with
  replacement characters, as Calibre does with a name Python does not know.

## Entities a file defines

Calibre replaces the entities a document's DOCTYPE defines with their text, however much text
that makes (`parse_html`). A short value referenced many times multiplies: a 100 KB file can
name 100 MB. The port refuses a file whose entities would make more than 16 MB
(`deckle_limits::MAX_ENTITY_TEXT`, `dom::Error::EntityText`).

## Files too large to read

Calibre reads every document and XML file of a book whatever its size. The port refuses the
book (`book::Error::TooLarge`) when one it reads is larger than
`deckle_limits::MAX_CONTENT_FILE_BYTES` (32 MiB), holds more than `MAX_DOM_NODES` nodes, or has
a start tag of more than `MAX_TAG_ATTRIBUTES` (256) attributes in the scan before the XML parse,
and when the documents it reads hold more than `MAX_BOOK_DOM_NODES` elements and attributes or
`MAX_BOOK_CONTENT_BYTES` (256 MiB) together. The nine Gutenberg books' largest file is 70 KB,
their largest book holds 7,718 elements and attributes and reads 989 KB, and no tag has more than
7 attributes.

Known gap: a tag the XML scan does not see is not counted when `html5ever` reads the file in the
HTML fallback, and `html5ever` compares each attribute with the ones before it. A `<p>` of
80,000 attributes after `<script><x y="</script>` (the scan reads one quoted value, `html5ever`
ends the script and reads the `<p>`) takes 12 s in a release build; 80,000 in a plain tag took
18 s before the scan counted them. `book::a_many_attribute_tag_read_as_html_is_a_known_gap`
records the cost; the outcome note's "Before phase 4" lists the options.

## Calibre steps the element tree leaves out

- Comments and processing instructions are kept as child nodes, as lxml keeps them, but both
  are one kind of node: the tree does not say which, and a processing instruction keeps its
  value, not its target. A comment made only of dashes keeps them, where Calibre's HTML 4
  fallback parser strips the dashes first.
- `HTMLPreProcessor`'s rule sets for Baen books, Book Designer and calibre's own pdftohtml
  output, and the user's search and replace, are not run.
- A document whose root is `<ncx>` is kept as parsed; Calibre parses it again as XML
  (`base.py` `_parse_xml`).
- Forcing the XHTML namespace is done on the tree; Calibre writes the tree out and parses it
  again, with fallbacks for text that does not parse (`:=`, comments, `<meta>` tags), which the
  tree never needs.

## Import cycles

When a book's stylesheets import each other in a loop, Calibre's parse of the first one it
reads recurses until Python's recursion limit, and that sheet is removed from the book
(`reader.py` `_manifest_prune_invalid`). The others then read without it. Which sheet goes first
follows the order of a Python set of manifest items, which depends on memory addresses.

The port drops each `@import` whose chain comes back to a sheet already on it, with everything
that import would have brought; the importing sheet keeps its own rules (`css/sheet.rs`). On
`css_quirks`, where `cycle_a.css` imports `cycle_b.css` and back, both give `cycle_a.css`'s
rules and lose `cycle_b.css`'s. Calibre can instead lose a sheet that links to the loop, or keep
a different half of it.

## Imports more than 8 deep

Calibre follows `@import` chains as deep as Python's recursion allows. The port follows eight
levels below the sheet it is reading and drops an import further down (`MAX_IMPORT_DEPTH`), so a
crafted chain cannot make it read the same sheets without end. Each imported sheet is read once
however often, and under however many spellings of its address (`p.css`, `./p.css`,
`a/../p.css`), it is imported; every import read or followed counts toward `MAX_STYLE_RULES`,
and each sheet's text toward `MAX_STYLE_DECLS`.

## CSS nested past 32 levels

A function inside a value (`f(g(h(…)))`) or a selector inside `:not()`, `:is()`, `:has()` and the
like nests as deep as Python's recursion allows in css_parser. The port reads 32 levels and
drops a declaration or selector list that goes deeper (`css/value.rs` `MAX_NESTING`). Only the
first level of `@media` is read, as Calibre's `StylizerRules` reads it; a deeper `@media` is
skipped without being parsed.

## Malformed media queries

Where css_parser cannot read an `@media` rule's query, it keeps the rule with no rules inside.
A few malformed queries it cuts short instead and keeps the rules: `screen and color` is read
as `screen and`, `screen and (orientation:)` as `screen and (orientation:`, and `media_ok`
accepts those. The port drops the rules of any query that is not `[only|not] type [and
(feature)]…` or `(feature) [and (feature)]…` (`css/media.rs` `media_list_valid`). A `media`
attribute is not read by css_parser, and `media_ok` treats it the same in both.

## CSS spellings css_parser reads its own way

The port follows css_parser's reading of a book's CSS (`tests/it/css_sheet.rs` lists the cases
checked against calibre). It differs on spellings books rarely use:

- A comment between selectors with spaces around it (`a /**/ b`) is kept in the selector text by
  css_parser; the port reads it as a space.
- `url( a b )`, an address with a space and no quotes, is kept by css_parser; `cssparser`
  reads it as a bad URL and the declaration is dropped.
- A number with a signed exponent (`1e+3`) is two items to css_parser (`1e +3`) and one to the
  port (`1e+3`). `1e3%` is dropped by css_parser and kept by the port.
- An escape of a surrogate code point (`\D800`) is kept by Python; the port writes U+FFFD.
- In `calc()`, css_parser's spacing rules for `*` and `/` are its own: it drops `calc(1px* 2)`,
  `calc(1px/ 2)`, `calc(1px/2/3)` and `calc(1px*2/3)` but keeps `calc(1px *2)`,
  `calc(1px/2*3)` and `calc(1px*2/3*4)`. The port keeps all of them, written with a space
  around each operator. It follows css_parser on `+` and `-` (white space needed on both
  sides), on what may stand between the operators (numbers and `var()`), and drops brackets
  and nested functions as css_parser does.
- An `@import` with a media query other than `all`, of a sheet holding `@font-face`, `@media`
  or `@page` rules, is left unresolved by css_parser's `resolveImports` when the importing sheet
  is linked, so Calibre loses the imported rules; the port imports them when the query applies.

## An escaped parenthesis in url()

css_parser misreads an unquoted address with an escaped closing parenthesis (`url(a\)b.png)`):
the declaration and the rest of the stylesheet are lost, so Calibre keeps the rules before it
and the rule holding it, empty. The port reads
the address as `a)b.png`, written `url("a\)b.png")`, and keeps the rules after it
(`an_escaped_parenthesis_in_a_url_keeps_the_sheet`).

## Selectors that stop Calibre's conversion

Calibre's selector engine (`css_selectors`) raises an exception the Stylizer does not catch for a
few selectors, and the conversion stops: a function named after one of its own dispatch
functions that takes other arguments (`p:root()`, `p:empty()`, `:class(a)`), an attribute
selector with `!=` (`[a!=b]`) or no name (`[*|=b]`), more than about 400 parts in one compound
or 300 compounds (Python's recursion limit), a `:nth-*()` number past 10^308. The port drops
the rule (`Parsed::Dropped`) and goes on. It also drops a selector of more than 256 parts in all
(type, id, class, attribute and pseudo-class selectors and pseudo-elements, those inside
`:is()`, `:not()`, `:has()` and `of` included; `css/select/mod.rs` `MAX_PARTS`), which bounds
the tests matching makes per element; Calibre applies one of 257 to about 300 parts.

## Selectors longer than 64 KiB

The port drops a selector whose text is longer than 64 KiB (`css/select/mod.rs`
`MAX_SELECTOR_BYTES`) without reading it, so a long one never holds its tokens in memory; Calibre
reads and applies it. css_parser's own limits (`MAX_STYLE_DECLS` counts a unit for each 64 bytes
of a selector) already bound how many such selectors a book file holds.

## Work on one book file's selectors

The port counts the steps of matching every selector of a book file against it (an element a
compound is tested on, times its tests; an element a combinator moves through; 64 bytes of an
attribute value compared); past `deckle_limits::MAX_SELECTOR_WORK` `css::select` gives
`css::TooLarge` for that file. Calibre has no such limit and takes as long as the selectors need
(310 s for 50,000 simple selectors on a file of 100,000 elements, measured in the Task 7
review). The limit is about 2 seconds of the port's slowest matching in a release build (a
class test, `:has()`, `:nth-child(of)`: about 20 ns a step); 2,000 rules of the kinds books use
on a file of 20,000 elements take 12% of it (`real_books_stay_far_below_the_work_limit`).

## Unknown pseudo-classes Calibre never reaches

Calibre evaluates a selector lazily: a class, id, `:is()` or `:has()` that no element passes
ends the evaluation of the parts written before it in its compound, and a `:nth-*()` argument is
read only when an element reaches it. An unknown pseudo-class or an unreadable argument raises
only when evaluation reaches it, so whether the rule is skipped depends on the document:
`:foo.zz` raises nothing where no element has class `zz`, and `:not(:foo.zz)` then matches
every element. The port checks every
part of a selector before matching and drops the rule whenever one would raise
(`probed_selectors_agree_with_calibre` lists the probed cases). Parts before a `:root` or
`:scope` in their compound are never evaluated by Calibre, and the port does not check them
either.

## The pseudo-class a rule is stored under

`stylizer.py` finds the pseudo-class a rule's declarations are stored under with a regular
expression whose alternatives come from a Python frozenset, in an order that changes from one
run to the next (string hashing is randomised). For `:focus-within`, `:focus-visible` and
`:target-within`, Calibre stores the rule under the full name or under `focus` or `target`,
depending on the run; the port always reports the full name. The port also reports the name in
lower case and folds case in ASCII only, where Calibre keeps the text's spelling and folds
case as Python's `re.I` does (`ſ` for `s`); css_parser writes pseudo-class names in lower case,
so only an attribute value or an escaped name in the selector can differ.

## Numbers in :nth-child() and the like

Calibre reads the `an+b` of `:nth-*()` with Python's `int()` and tests a position with true
division. The port holds a number as an i64 that stops at ±(2^63 − 1) (`py::py_int`), and
divides as f64, which is Python's result when both numbers are below 2^53 and can round
differently past it. Calibre's `int()` refuses more than 4300
digits, as the port does. A unicode escape naming a surrogate code point is read as U+FFFD,
where Python keeps the surrogate.

## Chapter headings in other namespaces

Calibre's default chapter expression tests `name()='h1' or name()='h2'`, the qualified name: an
element written with a prefix (`<x:h1>`) is not a heading to it. The element tree keeps no
prefixes, so `dom::mark_chapters` and `dom::generated_toc` take an `h1` or `h2` in any
namespace as a heading. Calibre's own parse writes XHTML without prefixes, so only an element of
another namespace given a prefix differs.

## Links to a bracketed host in generated contents

When a book has no contents and no chapter headings, Calibre lists its links, skipping one whose
href `urlparse` refuses. `urlparse` refuses a host in brackets (`//[...]`) that is not an IPv6
address or `v<hex>.<text>`, or that has text before the bracket or anything but a port after
it. `dom::generated_toc` refuses only unmatched brackets, so it lists such a link where Calibre
skips it. The link points outside the book either way.

## The order of rules Calibre's engine refuses

Calibre sorts every rule by css_parser's specificity, then skips at matching the ones its
selector engine refuses. A refused rule changes no style, only its place in `Cascaded::rules`.
The port takes the specificity from its own reading of the selector where it gets that far
(`li::marker`), and sorts a selector it cannot read at all as (0, 0, 0).

## Declarations one book file's cascade stores

Calibre keeps a dictionary of declarations for every element, however many. The port shares one
copy among the elements the same rules and `style` attribute reach, and counts what it stores
(`deckle_limits::MAX_CASCADE_DECLS`): past it `css::cascade` gives `css::Error::Cascade` for that
file. The flattener's second cascade, and the copies the flattener makes where an attribute
changes an element's styles, count against the same limit, carried on from the first cascade, as
its selector matching counts against `deckle_limits::MAX_SELECTOR_WORK`; past either
`css::flatten` refuses the file. At the limit the first cascade alone takes about 85 MB, and
both together up to about 120 MB (118 MB measured for 45,000 paragraphs each styled its own way,
refused in the second cascade; 25,000 pass both, in 70 MB; release build). The 49 files of the cascade fixtures use at most 394 units in the first cascade and 1,049 in both,
the files of nine Gutenberg books at most 1,495 in the first; a file of 40,000 elements each
styled its own way with 12 declarations exceeds it, and one of 9,000 paragraphs each with a
1,200-byte declaration of its own passes the first cascade but not the second. The port also counts the CSS rules of all a file's stylesheets
together against `deckle_limits::MAX_STYLE_RULES`, where `css::parse_sheet` counts each sheet's
own, and refuses a file the flattener gives more classes than that limit
(`stylesheet.css`'s rules for the file).

Calibre also holds every file's styles until the book is done, without limit. The port counts
the units of all a book's files together against `deckle_limits::MAX_BOOK_CASCADE_DECLS` (3
million, about 400 MB of styles) and refuses the book past it (`css::Error::BookCascade`): a book
of 200 files of 6,000 elements with 2,000 styles of their own each is refused at its 78th file,
in about 2 seconds. The fixture books use at most 4,196 units, 200 chapters of an ordinary novel
135,800.

## Class names from letters outside ASCII

The flattener names a class after the element's first class, made ASCII by Calibre's
`ascii_text` (its `Unihandecoder`). The port uses the deunicode crate, whose tables differ for
some letters, so a class first named by a non-ASCII class can get another name (`Ünter` is
`unter` in both; a CJK class name may be spelled differently). Which elements share a class,
and their styles, are the same.

## The order of the flattener's classes

Calibre writes `stylesheet.css` with the elements' classes sorted by `numeric_sort_key` (ICU
collation, numbers as numbers) and the pseudo-classes' after them. The port sorts names by runs
of digits as numbers and the rest by bytes, which differs from ICU for punctuation (`_`, `-`,
`.`) next to letters. The order changes styles only for an element two of the flattener's
element classes reach: one the flattener does not walk (in `<head>`, or under an element of
another namespace) whose own `class` attribute names two of them. Within a `class` attribute the
port writes the names in byte order; Calibre writes them in a Python set's order, which changes
from one run to the next (the dumps sort them).

## A negative line height

With `minimum_line_height` 0, the flattener sets `line-height: 0.0` only where an element's
line height (`Style.lineHeight`, inherited from the parent where the element sets none) is a
negative number of points and the element is not a drop cap. The port decides that from the
sign of the value as written (a number, or a length `unit_convert` reads), taking the font size
as positive and finite: where an element's font size is 0, infinite or NaN (`font-size: 0`,
`1e999pt`), Calibre leaves the line height and the port sets it.

## Values the flattener writes back

Calibre writes each class as text (`.name {key: value; …}`) and css_parser reads `stylesheet.css`
back. The port reads each class's block back with its own declaration reader, so `;` and
`!important` inside an attribute's value come back as Calibre's do. A `}` inside one (`<font
face="a}b">`) ends the rule in Calibre's sheet and changes the rules after it; the port reads it
within its own block only.

## Fake margins on elements the flattener leaves

`RemoveFakeMargins` removes a margin from a class, which every element of the book with that class
then lacks. The port works out again the styles of the elements the flattener gave that class.
An element the flattener does not walk, whose own `class` attribute happens to name a class the
flattener made and edited, keeps the margin in the port.

## Property names in style values

`css::Styles` answers a property by its CSS name only. Calibre's `Style[name]` first turns the
name into a Python attribute name (`_toDOMname`), so `fontSize` or `font-Size` also reach the
font size; kfxgen asks by CSS names.

## The nav and NCX kfxgen reads beside Calibre's OPF

kfxgen opens the nav and the NCX that Calibre's rewritten OPF names with lxml's XML parser
(`converter.py` `_container_parser`, entities left unresolved). The port reads them only when
they are UTF-8 (a file in another encoding counts as missing, as elsewhere in phase 2), refuses an
entity holding markup and a file past `deckle_limits`' node or attribute limits by treating it as
missing, as kfxgen treats a file lxml cannot parse, and expands an entity of plain text, where
lxml's `itertext` gives the reference as written (`&e;`, checked with lxml 6.0.2). An NCX item written with the misspelt
`mediatype` attribute counts as an NCX; kfxgen's XPath asks for `media-type` only. kfxgen logs
each href it rejects to its security log; the port has no such log.

## Run flags and numbers in the inline helpers

`inline_style.py`'s helpers read Python values, and the port holds them in Rust ones
(`src/convert/inline_style.rs`, `src/convert/font_table.rs`, at ad38ec8):

- A font weight past an i64 (`parse_weight`, `int()` of up to 4,300 digits) is `i64::MAX` or
  `i64::MIN`; Python keeps the whole number. Whether a run is bold comes out the same.
- A run's flags are a set kept sorted, so two are equal when their members are. Python compares
  frozensets by `==`, where `-0.0` equals `0.0` and a NaN only itself; the port compares floats
  by bits. kfxgen puts no zero and no NaN in a flag: sizes are above 0, colours are ints.
- Of two flags of one kind on a run (`flag_value`), kfxgen takes whichever its set's hash
  order gives first, which changes from run to run; the port takes the smaller.
- An anchor mark is read only with a string id; kfxgen reads any value but None, and only
  ever makes string ones (`make_anchor_mark`).
- A nested shift whose depth is a digit `isdigit()` takes and `int()` refuses (`"sub@²"`)
  is no nested shift, where kfxgen raises. kfxgen writes the depth itself, in ASCII.

Where kfxgen raises on a value from the book, the helper returns `Raised` naming the exception:
a length unit whose "i" is U+0130 or U+0131 (`KeyError`), and a colour function whose number
`float()` or `int()` refuses (`ValueError`: two points, a channel of more than 4,300 digits, a
percentage too big for a float). This is no difference by itself: kfxgen catches these higher
up, and its callers' handling is what the port's callers follow. Raised inside the style
resolver's `resolve` (`converter.py` `_build_style_resolver`, a box's or cell's fill), the
element resolves to None; raised anywhere else in a spine item's extraction, the item is left
out with a warning (`extract_chapters_from_oeb`'s `except Exception`, "Spine item parse
failed"), and the book is still made.

## The inline walk

`_walk_inline` and its helpers (`src/convert/inline.rs`, `quotes.rs`, `bidi.rs`, at ad38ec8):

- kfxgen recurses once per nested element, and past Python's recursion limit raises
  `RecursionError`, which leaves the spine item out. The port walks with a stack of its own
  and refuses an element nested more than 256 levels inside a paragraph (`Error::TooLarge`).
  `read_book` refuses a file nested that deep first, so only a tree built by hand gets there.
- A run's flags are one set object that kfxgen's segments share; the port copies them into
  each element and each segment, and charges the text in each copy (link targets, rubies'
  readings, languages) to the file's style text budget, as it charges the resolver's reads. A
  file whose flags hold long text over many runs (a 1 MiB link target around 2,000 words) is
  refused where kfxgen makes the book. So is a file whose tate-chu-yoko are nested so that their
  text is read over and over (each is read once more for every one around it).
- A CSS escape of a surrogate code point in a `<q>`'s `quotes` or `content` (`"\D800"`) is
  U+FFFD; Python's `chr` gives a lone surrogate.
- `_turned_parts` takes the `data-kfxgen-horizontal` and `-vertical` marks off every element
  inside a turned element, in the tree, for good. The port keeps that in the walk of the file
  (`Walk`), so the walk's output is the same. kfxgen's style resolver reads one of the marks
  (`_vertical_box`, a vertical element's margins and paddings) and a second pass walks the same
  tree: the port's resolver and a second pass's new `Walk` still see the marks. The inline walk
  reads no margins or paddings. The spacing steps of the block walk read the styles of block
  elements, their ancestors and those ancestors' children only, never an element inside a
  turned one, so they see no difference either; later steps are not checked.

## Pictures in text and initials

The picture tokens and the initial (`src/convert/pictures.rs`, `initial.rs`, at ad38ec8):

- An inline `<svg>` that draws shapes and nothing kfxgen takes from it (no `<image>`, no
  `<text>`, not hidden by its own style or attributes) is refused (`Error::Unsupported`, "SVG
  pictures"). kfxgen draws it with Qt as a picture of its own and names it by a hash of lxml's
  text of it (plan decision 2). An `<svg>` kfxgen leaves out (hidden, or its shapes only inside
  definitions or a nested `<svg>`) is left out here too.
- A picture source that is a `data:` URI (an `<img>`'s `src`, an SVG `<image>`'s href, a
  background picture's `url()`; `data:` in any case, nothing before it) is refused
  (`Error::Unsupported`, "pictures embedded as data: URIs") before it is copied, until
  calibre's `DataURL` step is ported. calibre's conversion runs that step before kfxgen
  (`ebooks/oeb/transforms/data_url.py`, from `plumber.py`): an `<img src="data:image/…">` it can
  decode becomes a file of the manifest, so kfxgen writes a token naming that file.
  `read_book` has no such step. An SVG `<image>` or CSS background `data:` source, and an
  `<img>` one calibre cannot decode, reach kfxgen as they are, and kfxgen writes the URI into
  the token (the generator then finds no picture by that name); the port refuses those too.
- The text a token copies out of the book (an `<img>`'s `src` and `alt`, an SVG `<image>`'s
  href) is charged to the file's style text budget. A file whose pictures' sources pass it
  (about 32 MB) is refused where kfxgen makes the book.
- kfxgen's SVG helpers and `_leading_float` recurse once per level, and past Python's
  recursion limit raise `RecursionError`, which leaves the spine item out. The port uses loops.
  `read_book` refuses a file nested deep enough for either, so only a tree built by hand gets
  there.

## The block walk

`extract_blocks_from_html`'s walk (`src/convert/blocks/`, `block_bidi.rs`, at ad38ec8):

- kfxgen recurses four calls per nested container (`_walk`, `_walk_flow`, `_walk_element`,
  `_walk_container`), and past Python's recursion limit raises `RecursionError`, which leaves
  the spine item out. The pinned `extract_blocks_from_html`, called from a script under Python
  3.14's default limit (1,000) with lxml 6.0.2 and no style resolver, raises at 249 `<div>`s
  nested in the `<body>`; inside a calibre conversion its stack starts deeper, so it raises a
  little sooner (not run). `read_book` lets a file through up to 256 levels below the root, so a
  file nested about 245 to 254 levels deep is left out by kfxgen and converted here. The port
  walks with a stack of its own and refuses an element more than 256 levels below the body
  (`Error::TooLarge`), which only a tree built by hand reaches.
- A block holding a MathML `<math>` that is not shown (hidden, or in a part the walk leaves
  out) is refused (`Error::Unsupported`, "math"), as a shown one is (plan decision 2). kfxgen
  writes the block with `label_texts`, its equations' text for matching a contents label;
  the generator refuses `label_texts` anyway.
- Each anchor key a block gets is a copy of the file's href and an id. The copies are charged to
  the file's style text budget, so a file with a very long href and very many ids is refused
  where kfxgen makes the book. The keys the per-file loop makes (`src/convert/chapters.rs`) for
  a file that shows nothing (one for each of its ids) and for the ids ending a file are charged
  too, but in whole 64-byte units, so a key under 64 bytes costs nothing: the book's node limit,
  not the budget, bounds how many there are. A file past its budget stops the conversion with
  `Error::TooLarge`, "<file>: converting this file copies or reads more than 32000000 bytes of
  its text and styles" (the budget, 500,000 units of 64 bytes, counts the text and styles the
  converter copies or reads, not only CSS). Each pass of the loop has its own budget for each
  file: what the first pass charged does not count on the second. The second pass (kfxgen's,
  printing the contents listings) also reads the listings the first left out, so a file with a
  very large listing can be refused there when the first pass was not.
- The budgets of all a book's files also count together in each pass, and in the footnote
  scan: past 2,000,000 units (`deckle_limits::MAX_BOOK_TEXT_BUDGET`, 128,000,000 bytes) the
  book is refused at the charge that passes it (`Error::TooLarge`, "converting this book copies
  or reads more than 128000000 bytes of its text and styles"), where kfxgen, which shares the
  strings the port copies, makes the book. Smallest input: 20 files, each a link to an id of
  32,000 letters round 240 words in `<i>` (each word's run copies the target); 5 such files
  convert. The nine Gutenberg books charge at most 5,409 units in a pass.
- A pass that makes more than 500,000 blocks (`deckle_limits::MAX_BOOK_BLOCKS`) is refused after
  the file that passes it (`Error::TooLarge`, "this book has more than 500000 paragraphs and
  other blocks"); kfxgen makes the book, however long it takes. Smallest input: 8 files of
  125,000 `<p>x</p>` (refused after the fifth, in 34 s, release build). The nine Gutenberg books
  make at most 3,450 blocks.
- An `<svg>`'s own ids go on each picture block it draws (`_emit_image_block`), twice a block;
  kfxgen copies them without a limit. The port charges the copies to the same budget before
  making them, so `<svg id="(1 MiB)">` with 200 `<image href="a"/>` stops the file
  (`Error::TooLarge`, "more than 32000000 bytes") where kfxgen writes 200 blocks.

## Lists

kfxgen's list code (`src/convert/blocks/lists.rs`, at ad38ec8):

- Python's int has no bound, so an `<ol start>` or `<li value>` of any length up to 4,300 digits
  is a number to kfxgen. The port reads it exactly within an i128. A typed marker past that, or
  a native list's `start`, an item's `value` or its number past an i64 (the model's integers),
  stops the file (`Error::Unsupported`, "list numbers too large"), where kfxgen writes the
  number. A number that is only read, never written (an item that spells out its own marker, a
  marker no text takes), changes nothing.
- To tell whether a list's items type their own markers, kfxgen joins each item's whole text,
  once for each list round the item. The port reads only the first characters it needs, and
  charges each node it reads to the file's style text budget, so lists nested deep whose items
  hold many elements and no text are refused where kfxgen makes the book.
- A nested list's box copies its items' texts and ids once more at each level (`_list_box`); the
  copies are charged to the same budget.

## Boxes

kfxgen's boxes (`src/convert/blocks/boxes.rs`, at ad38ec8):

- A box copies its blocks' texts and ids once more at each level of nesting (`_make_box`, as
  `_list_box`); the copies are charged to the file's style text budget. 32 bordered `<div>`s
  nested round one paragraph of 999,806 characters (32 is the deepest the generator writes) stop
  the file (`Error::TooLarge`, "more than 32000000 bytes"); 999,805 characters convert. kfxgen
  writes the 32 nested boxes, each holding the
  paragraph's whole text (about 32 MB in its model). At 64 levels the limit is 499,838
  characters, at 128 levels 249,854.
- Each block a bordered, filled, sized or turned container looks at costs one unit of the same
  budget. A container that draws no box round its blocks (a width round no picture, a contents
  entry naming a block past the first) leaves them for each such container round it to look at
  again, so a file is refused where kfxgen makes the book only past 500,000 such looks (with the
  file's other charges).

## Tables

kfxgen's native tables (`src/convert/blocks/tables.rs`, at ad38ec8):

- A table holding an `<epub:switch>` anywhere in it stops the file (`Error::Unsupported`,
  "tables holding an epub:switch") when native tables are on. kfxgen's `_shown_tree` deep-copies
  such a table (or cell) to drop the switch's cases, then asks the style resolver whether each
  row, cell and caption of the copy is hidden, and `_is_nav_listing` whether it is a contents
  listing; calibre never styled the copies, and the copy's root has no `data-kfxgen-source`, so
  what kfxgen decides there is not known here. Smallest input: `<table><tr><td><epub:switch>
  <epub:case required-namespace="…">…</epub:case><epub:default>x</epub:default></epub:switch>
  </td></tr></table>`; kfxgen writes a native table (or rows, if its copy reads as hidden).
- Each cell of a table (`<td>` or `<th>` anywhere below the `<table>`, a nested table's cells
  too) is charged three units of the file's style text budget, counted from the tree before
  anything is resolved, on top of what reading the table charges (about one unit a cell of a
  word). The file stops (`Error::TooLarge`, "more than 32000000 bytes") for 166,667 cells or more
  before any cell is read, and for about
  125,000 cells of a word each once they are read (124,990 convert and 125,010 are refused, in
  rows of 10, measured). Smallest refused input: `<table><tbody>` with 12,500 rows
  of ten `<td>cell</td>`. kfxgen converts these tables. Its `_fostered_content` tests every element
  of the table against a list of the table, its row groups and its rows, so its work grows
  with rows times cells: about 1.6 billion comparisons for 125,000 cells in rows of 10 (worked
  out from the code, not run). A table inside a table already counted is not counted again:
  kfxgen reads each nested table's whole tree again when the rows path reaches it.
- An id still waiting at the end of a cell walked as paragraphs, when the cell's last block is
  a drawn rule, is dropped. Smallest inputs: `<td><p>a</p><p>b</p><hr id="z"/></td>` and
  `<td><p>a</p><p>b</p><hr/><a id="z"></a></td>` in a native table. kfxgen's `_walk_cell`
  appends the id to that rule's `anchor_ids`, which is the one list `_EMPTY_BLOCK` shares with
  every rule and space block in the calibre process (`{**_EMPTY_BLOCK, ...}` is a shallow
  copy). From then on every rule, earlier ones too, holds `z`, and every rule in a file keyed
  afterwards gets the key `<file>#z`, in this book and later books converted by the same
  process. The port puts the id on the cell's rule, which the cell's paragraphs drop, so `z`
  names nothing and every rule keeps an empty `anchor_ids`. A link to `#z` is written without
  a link in the port (no block has the key, so the generator leaves the run unlinked); in
  kfxgen it lands on the first rule its generator finds with the key (both read in the
  generators' code, not run end to end).
- The same holds for an empty anchor after a row (`_walk_container`'s `follow`) when the row's
  first block is a space or rule block. kfxgen appends the id to the list every rule and space
  block shares. A space block comes from a row holding only a no-break space: `<table><tr
  style="display: none"><td>h</td></tr><tr><td>&#160;</td></tr><a id="x"></a><tr><td>b</td>
  </tr><a id="z"></a></table>`; the port puts `x` on that space block, which spacing takes out,
  so `x` names nothing. A rule block comes from a row holding a table whose cell opens with a
  rule: `<table><tr><td><hr/><table><tr><td>i</td></tr></table></td></tr><a id="x"></a><tr>
  <td>b</td></tr><a id="z"></a></table><hr/><p>after</p>`; the port keeps `x` on that one rule,
  and the second rule has none (in kfxgen every rule carries it).
- Each paragraph a row holding a table writes (`_walk_split_row`) costs one unit of the file's
  style text budget, and again for each such row round it, which looks at it again (to mark it
  a row, and to make it bold in a header cell). Tables nested 62 deep in cells, the innermost
  cell holding 10,000 paragraphs, stop the file (`Error::TooLarge`, "more than 32000000 bytes");
  5,000 paragraphs convert (317,759
  units). kfxgen converts both.
- A `<col span="N">` repeats its width N times (up to 1,000) in the model's column widths. Each
  repeat is charged to the file's style text budget before it is made, so a table whose `<col>`
  spans add up to about a million widths stops the file (`Error::TooLarge`, "more than 32000000
  bytes"): 2,000 `<col span="1000">` are refused
  after about 1,000 of them. kfxgen writes the two million widths.

## The book's pictures and cover

The pictures and cover handed to the generator (`src/convert/images.rs`, at ad38ec8). kfxgen
decodes, scales and writes pictures with Qt through calibre; the port uses the `image` crate.

- A picture kfxgen re-encodes (a GIF made a PNG, a CMYK JPEG made RGB, a picture over 1,920 px
  or 1 MiB) has kfxgen's format and size, not its bytes: the encoders and the scaling filter
  differ. A GIF's PNG is RGBA here, a palette PNG in kfxgen; a CMYK JPEG's colours may differ.
  A picture kept as it is keeps the EPUB's bytes in both.
- A re-encoded PNG with no alpha is the smaller of the PNG and a JPEG at quality 85 in both, but
  the sizes being compared are each encoder's, so the format chosen can differ. kfxgen also
  tries a palette PNG for a palette source, which the port does not. So whether a picture
  re-encoded for its bytes comes out smaller than it was, and is kept, can differ too. Smallest
  input: a PNG over 1,920 px with no alpha (`pictures_more`'s `wide.png` comes out a PNG in
  both).
- Which pictures decode is `image`'s answer, not Qt's: a picture neither can read is left out
  with a warning in both, but Qt fills in a JPEG cut short and the port may not, and the two
  disagree on other damaged files.
- A picture whose header states more than 100 megapixels (`deckle_limits::MAX_IMAGE_PIXELS`),
  or that needs more than `image`'s 512 MiB to decode, refuses the book (`Error::TooLarge`)
  before it is decoded. calibre lets Qt allocate up to 1,024 MB for a picture
  (`set_image_allocation_limit`), so kfxgen scales one of up to about 268 megapixels at 4
  bytes a pixel and leaves out a larger one (worked out from the code, not run). Smallest
  input: a PNG whose header says 10,001 × 10,000. A GIF counts the larger of its screen and
  its first frame, which `image` decodes at the frame's own size: a GIF of a 1 × 1 screen
  whose frame says 11,000 × 10,000 is refused too.
- A JPEG over about 48.8 megapixels refuses the book (`Error::TooLarge`) before it is decoded.
  zune-jpeg keeps 2-byte coefficient planes for up to 4 components outside `image`'s 512 MiB
  allocation limit (a progressive JPEG, or a first scan naming fewer components than the
  frame: a 200-byte progressive 10,000 × 10,000 file reached 900 MB), so 8 bytes a pixel are
  taken off that limit for every JPEG. With 3 bytes a pixel of RGB output (a CMYK JPEG is
  decoded to RGB too), 11 × pixels must stay within 536,870,912 bytes: 48,806,446 pixels at
  most. A grey JPEG (1 byte a pixel out, 9 in all) may have 59,652,323. kfxgen converts these.
  Smallest input: a baseline RGB JPEG of 7,000 × 7,000 (49 megapixels; worked out, not run).
- The pictures a book decodes may state 10 gigapixels together
  (`deckle_limits::MAX_BOOK_IMAGE_PIXELS`), counted from each header before its decode, every
  decode counted (a CMYK JPEG twice). The picture that passes it refuses the book
  (`Error::TooLarge`). kfxgen has no such limit. Smallest input: 101 pictures whose headers
  each say 10,000 × 10,000 (the 101st is refused). A book just under it (100 pictures of
  10,000 × 10,000) took 29 s in a release build for RGB JPEGs and 9.5 s for grey PNGs.
- An SVG file of the manifest, and an `<svg>` drawn only with shapes in a spine page (not in
  another `<svg>`, in a page that is not itself an SVG), refuse the book (`Error::Unsupported`,
  "SVG pictures"), even where no text shows it. kfxgen draws them with Qt.
- When several manifest pictures have "cover" in their id or href and the book names no other
  cover, the port takes the first by href. calibre's manifest is a set, so which one kfxgen
  takes varies from run to run.

## The book's fonts

`convert::font_table` (`font_table.py`) differs here:

- A WOFF2 that is tried refuses the book (`Error::Unsupported`, "WOFF2 fonts"): every `src`
  before it failed, including a damaged WOFF2 kfxgen would skip in favour of a later `url()`.
  fontTools rebuilds a WOFF2's glyph tables with its own compiler, which the port does not
  have, so it cannot tell a good WOFF2 from a damaged one. A WOFF2 over 50 MiB, or given after
  a `src` that is chosen, is skipped as in kfxgen. Smallest input:
  `src: url(bad.woff2), url(good.woff)`, where kfxgen would embed `good.woff` (read, not run:
  kfxgen's WOFF2 path needs brotli).
- The fonts a book embeds may come to 256 MiB together: a WOFF is charged the declared size of
  its tables before it is inflated (whether or not it is then embedded), a TTF/OTF its size
  before it is kept. Each file is charged once, however many rules or hrefs name it (rules
  naming one href still give one face, and hrefs finding one file by its file name give a face
  each, sharing its bytes, as in kfxgen). The font that passes it refuses the book
  (`Error::TooLarge`). kfxgen has no such limit; calibre holds every file in memory. Smallest
  input: six TTFs of 50 MiB (the sixth is refused); a 1 MB EPUB of twenty deflated ones held
  1,000 MiB as faces before. Five WOFFs of 65,535 entries each inflating 49.8 MB, the most a
  WOFF may ask for, took 3.3 s in a debug build before the sixth was refused.
- A WOFF is unpacked to the bytes fontTools 4.60.1 writes, compared on the `fonts_woff` model
  and on WOFFs made in the tests; the version calibre bundles was not read.
- When two manifest files share a file name and a `src` names neither by href, the port takes
  the last in the OPF. calibre's manifest is a set, so which one kfxgen takes varies from run
  to run.
- A font family is lower-cased by the Rust toolchain's Unicode version (17.0 for rustc 1.99),
  not Python's (16.0 for 3.14). The 28 code points Unicode 17 gave lower-case forms (U+A7CE,
  U+A7D2, U+A7D4, U+16EA0–U+16EB8) are lower-cased by Rust and left as they are by Python, so a
  family holding one gets another `css_family` and name hash than in kfxgen. Smallest input:
  `font-family: "\16EA0 x"`.
- `KFXGEN_MAX_FONT_BYTES` is not read; the cap is its default, 50 MiB. kfxgen's warnings for a
  skipped face are not reported.

## Added files of one name

kfxgen puts the files Calibre added to the spine (`_spine_in_order`) in the order of the first
contents entry naming each one's file name, then by file name, with a stable sort. Two added
files of one file name in different folders sort alike whether or not an entry names that
name, so kfxgen keeps Calibre's set order for them, which changes from run to run, and with it
which of two spellings up to letter case or percent-encoding it keeps. Names are compared
decoded, a bad sequence decoded as U+FFFD, so `%FF%FE.xhtml` and `%C0%80.xhtml` are one name
too. The port (`src/convert/toc.rs`) sorts them by href, and of two such spellings keeps the
first by href. Smallest input: a page
linking to `A/x.xhtml` and `B/x.xhtml`, neither in the OPF's spine, with or without a contents
entry naming `A/x.xhtml`; the port gives `A/x.xhtml` first every time.

## Note and link keys under a long path

`_note_pair_ids`, `_book_link_targets` and `_footnote_keys` (`src/convert/notes.rs`) make a
key holding the file's path for each id or link they keep, and read the path again for each
link they resolve. The port charges each key made, and each path read, to the file's style
text budget, as the anchor keys are; the budget counts whole 64-byte units per charge, so keys
under 64 bytes cost nothing and the book's node limit bounds the rest. Measured: 5 files of
95,000 noteref links each held about 720 bytes a link (peak 761 MB, 6.3 s in a debug build). For example, a file at a path of 2,000 characters with
40,000 note markers linking to notes that link back stops the conversion (`Error::TooLarge`, "more
than 32000000 bytes") where kfxgen makes the book; at a path of 100 characters it
converts.

## Headings a contents page repeats

When kfxgen rebuilds the book's contents page (`_rebuild_contents_page`), each heading on the
page that is also a link the page gives more than once becomes a line of its own, a copy of
that entry's line: its title and, for a place in a chapter, its key. A heading matches an
entry when their words agree after `_normalize_title`, which joins runs of white space and
strips punctuation from both ends, so a short heading can match a very long title, or a short
title with a very long key (`<file>#<id>` from the contents' own link), and these are copied
once for each such heading (kfxgen shares the strings; the port copies them). The port
(`src/convert/contents_page.rs`) counts the bytes of the titles and keys these lines copy
before it makes them, and past 33,554,432 bytes (`MAX_CONTENT_FILE_BYTES`, the size a content
file may have) stops the conversion (`Error::TooLarge`, "the contents page repeats more than
33554432 bytes of contents entry titles and keys") where kfxgen makes the book. A heading in a
real book is about as long as the title it repeats, the page's headings are in one file under
that size, and a key is a file name and an id, so only a crafted title or link reaches the
limit. Smallest inputs: a contents entry titled `"!" * 10_000_000 + "A"`, or a second entry
titled "A" in the section of the entry before it (a place) linking to an id of 10,000,000
characters, and a "Contents" page of a heading "Contents" and four headings "A", each a link: four copies, over 40,000,000
bytes, are refused; with three headings the book converts.

## Titles and keys chapters repeat

Three more steps copy a string kfxgen gives several chapters or entries
(`src/convert/assemble.rs`, `src/convert/chapters.rs`). Each counts the bytes it is about to
copy and, past 33,554,432 bytes (`MAX_CONTENT_FILE_BYTES`) in the book, stops the conversion
(`Error::TooLarge`, "the contents repeat more than 33554432 bytes of titles and keys") where
kfxgen makes the book. The two assembly copies share one count; the split and the contents
page's count above each have their own, so together they hold at most three times that.

- **Contents entries on one block.** A contents entry landing on the block of the entry before
  it gets that entry's key in the chapter's `sub_entries`; kfxgen's entries share the one
  string, the port copies it for each. Smallest input: one page, an entry naming it, an entry
  naming an id of 100,000 letters no block has (it snaps to the page's last block), and 336
  entries naming other missing ids, each snapping to the same block; with 335 the book
  converts.
- **Pages that keep a dropped contents listing.** Each contents entry that makes a page of no
  blocks keeping a listing gets the listing's keys (`_listing_keys`, `"<file>#<id>"` for each
  id). kfxgen builds these too, so the growth is kfxgen's as well (its memory grows the same
  way; the port's count stops it). Each key counts its bytes and 48 more, so many short keys
  count too. Smallest input: a page with a `<div class="toc">` of 5,000 `<a id>` and 222
  entries titled "Copyright", naming its first id and the page by turns (111 such pages); with
  220 entries the book converts.
- **A chapter cut where the writing mode changes.** Each part after the first gets the
  chapter's title (`_split_by_writing_mode`); kfxgen's parts share it. Smallest input: 36
  files alternately horizontal and vertical and two entries, the first titled with 1,000,000
  letters (34 copies); 35 files convert.

## Warnings

`convert` returns the warnings kfxgen logs (`Converted::warnings`, `src/convert/mod.rs`); no
oracle records kfxgen's log, so they were compared with the pinned Python by reading. They
differ from it here:

- A `repr` in kfxgen's text (`{entry['title']!r}`, `{href!r}`) is written as Rust's `{:?}`:
  double quotes, and Rust's escapes.
- "Spine item N parse failed (href): …" ends with the Python exception's name and what raises
  it, in the port's words (`Raised`), where kfxgen prints the exception's own message.
- When kfxgen reads the book a second time to keep its contents listings, it logs the first
  pass's warnings too, then drops what that pass made; the port keeps the second pass's only.
- Calibre's reader leaving a spine item out because it is no document (a stylesheet or a
  picture listed in the spine) is said in the port's words, not Calibre's. A spine `idref`
  naming no manifest item is dropped without a warning.
- The read-aloud narration is looked for in the OPF Calibre reads (the container's first);
  kfxgen reads every OPF the container names.
- kfxgen's `log.info` lines and the font table's messages about skipped faces are not
  returned.
