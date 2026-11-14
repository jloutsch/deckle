# Where the port differs from kfxgen

Deliberate differences between `deckle-kfxgen` and kfxgen `0bc705d`, each with why. Line
citations are at `4b81651`, the previous pin, unless a section names another commit.

## Unicode version of Python's character classes

kfxgen reads characters through Python: `str.isspace`, `isalpha`, `isalnum`, `isdigit`,
`lower` and `unicodedata.east_asian_width`, for instance when it checks that a chapter's text
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

Calibre decodes a document by its byte-order mark, as UTF-8, by the encoding it declares, or
else by uchardet's guess (`base.py` `OEBBook.decode`, `chardet.py` `xml_to_unicode`,
`force_encoding`). The port differs in two places:

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

## Calibre steps the element tree leaves out

- Comments and processing instructions are dropped and the text around them joined. Calibre
  keeps them in the tree; no path or style counts them. The two rules that read one are told
  what they need: the lower-casing check on the root's last child, whether that child was a
  comment; `DetectStructure`'s `at_start` (`dom::mark_chapters`), where one whose text is not
  blank stood (in an element's text or tail). Where a later clean-up step moves text from one
  element to another, that record stays with the element; and a comment made only of dashes
  counts as text, where Calibre's HTML 4 fallback parser strips the dashes first.
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
division. The port holds a number as an i128 that stops at ±2^125, refuses non-ASCII decimal
digits (Python reads `٣` as 3), and divides as f64, which is Python's result when both numbers
are below 2^53 and can round differently past it. Calibre's `int()` refuses more than 4300
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
`1e999pt`), Calibre leaves the line height and the port sets it. The port reads numbers as
Python's `float()` does but for `_` between digits and digits outside ASCII, which it does not
read; and it counts an element's children without the comments lxml counts when it tests a drop
cap.

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

## Numbers and names in style values

`css::Styles` reads a number in a style value as Python's `float()` does, but for `_` between
digits and digits outside ASCII, which it does not read (the same reader as "A negative line
height"): where Calibre reads `1_0` as 10 pixels, the port gives the text back. And it answers a
property by its CSS name only. Calibre's `Style[name]` first turns the name into a Python
attribute name (`_toDOMname`), so `fontSize` or `font-Size` also reach the font size; kfxgen asks
by CSS names.
