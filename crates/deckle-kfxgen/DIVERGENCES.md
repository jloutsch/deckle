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
