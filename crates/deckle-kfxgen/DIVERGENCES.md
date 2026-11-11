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
