# Where the port differs from kfxgen

Deliberate differences between `deckle-kfxgen` and kfxgen `4b81651`, each with why.

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
