# SPDX-License-Identifier: GPL-3.0-only
"""Print the Unicode tables of crates/deckle-kfxgen/src/generate/py.rs.

Run with the Python whose Unicode version the port follows (3.14, Unicode 16.0, as calibre's):

    python3.14 -I tools/ref/py_unicode_tables.py

The output is the block of tables in py.rs, from `SPACE` to `BIDI_STRONG`, formatted as
rustfmt leaves it; paste it over that block. Nothing else is read or written.
"""

import sys
import unicodedata

UNICODE = "16.0.0"
WIDTH = 100


def packed(items):
    """Items four spaces in, as many to a line as fit in WIDTH."""
    lines, line = [], "   "
    for item in items:
        if len(line) + 1 + len(item) > WIDTH:
            lines.append(line)
            line = "   "
        line += " " + item
    lines.append(line)
    return lines


def runs(value_of):
    """(first, last, value) for each run of consecutive code points of one non-None value."""
    out = []
    for c in range(sys.maxunicode + 1):
        v = value_of(chr(c))
        if out and out[-1][2] == v and out[-1][1] == c - 1:
            out[-1][1] = c
        else:
            out.append([c, c, v])
    return [r for r in out if r[2] is not None]


def main():
    if unicodedata.unidata_version != UNICODE:
        sys.exit(f"Unicode {unicodedata.unidata_version}, not {UNICODE}")
    out = []

    space = [c for c in range(sys.maxunicode + 1) if chr(c).isspace()]
    out.append("/// `str.isspace()`: every character it is true for.")
    out.append(f"const SPACE: [char; {len(space)}] = [")
    out += packed(f"'\\u{{{c:X}}}'," for c in space)
    out += ["];", ""]

    digit = runs(lambda ch: True if ch.isdigit() and unicodedata.category(ch) != "Nd" else None)
    out.append("/// `str.isdigit()` beyond category Nd: superscripts, circled digits and the like.")
    out.append(f"const DIGIT_NOT_ND: [(u32, u32); {len(digit)}] = [")
    out += [f"    (0x{a:X}, 0x{b:X})," for a, b, _ in digit]
    out += ["];", ""]

    wide = runs(lambda ch: True if unicodedata.east_asian_width(ch) in ("W", "F") else None)
    out.append('/// `unicodedata.east_asian_width(c) in ("W", "F")`.')
    out.append("#[rustfmt::skip]")
    out.append(f"const EAST_ASIAN_WIDE: [(u32, u32); {len(wide)}] = [")
    out += packed(f"(0x{a:X}, 0x{b:X})," for a, b, _ in wide)
    out += ["];", ""]

    zeros = [c for c in range(sys.maxunicode + 1) if unicodedata.decimal(chr(c), None) == 0]
    out.append("/// The zero of each run of ten decimal digits (`unicodedata.decimal(c) == 0`).")
    out.append("#[rustfmt::skip]")
    out.append(f"const DECIMAL_ZEROS: [u32; {len(zeros)}] = [")
    out += packed(f"0x{c:X}," for c in zeros)
    out += ["];", ""]

    names = {"L": "L", "R": "R", "AL": "Al"}
    strong = runs(lambda ch: names.get(unicodedata.bidirectional(ch)))
    out.append("/// `unicodedata.bidirectional(c)` where it is L, R or AL: (first, last, class).")
    out.append("#[rustfmt::skip]")
    out.append("#[allow(dead_code)] // Task 9's `_first_strong` reads it.")
    out.append(f"const BIDI_STRONG: [(u32, u32, Strong); {len(strong)}] = [")
    out += packed(f"(0x{a:X}, 0x{b:X}, {k})," for a, b, k in strong)
    out.append("];")
    print("\n".join(out))


if __name__ == "__main__":
    main()
