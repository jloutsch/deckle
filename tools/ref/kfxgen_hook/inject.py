# SPDX-License-Identifier: GPL-3.0-only
"""Put the dump hooks into an export of kfxgen: python3 inject.py <kfxgen_tree>

Copies model_dump.py and book_dump.py into plugin/kfxgen/ and adds three calls:

- at the top of generate_full_book (the call kfxgen's 55082ec had), so KFXGEN_DUMP_MODEL=<dir>
  dumps the generator's arguments;
- right after _ensure_oeb_opts in convert_oeb_to_kfx, so KFXGEN_DUMP_BOOK=<dir> dumps the book
  view, trees and styles kfxgen receives;
- at the start of the plugin's specialize_css_for_output, which calibre calls on each file
  before it flattens the CSS, so the same dump holds the first-pass rules.

Stops if an anchor line is not exactly once in its function.
"""

import shutil
import sys
from pathlib import Path

ANCHOR = "        # Once per book, before anything content-dependent: a retired override\n"
CALL = '''        # First, while locals() holds only the arguments: Deckle's Rust port is
        # tested against what this method makes from the same dump.
        _model_args = {
            k: v for k, v in locals().items() if k not in ("self", "output_path")
        }
        from .model_dump import dump_if_asked

        dump_if_asked(_model_args)

'''

BOOK_ANCHOR = "    _ensure_oeb_opts(oeb_book, opts)\n"
BOOK_CALL = """
    # Deckle: calibre's book view, trees and styles as kfxgen receives them.
    from .book_dump import dump_if_asked as _deckle_dump_book

    _deckle_dump_book(oeb_book)
"""

FIRST_PASS_ANCHOR = (
    "        try:\n"
    "            from .kfxgen.converter import mark_source_classes\n"
)
FIRST_PASS_CALL = """        # Deckle: the first-pass rules, before kfxgen marks anything.
        from .kfxgen.book_dump import note_first_pass

        note_first_pass(item, stylizer)
"""


def insert(path, function, anchor, call, after):
    """Put ``call`` before (or ``after``) the one ``anchor`` line inside ``function``."""
    text = path.read_text(encoding="utf-8")
    start = text.find(function)
    if text.count(anchor) != 1 or start < 0 or text.index(anchor) < start:
        sys.exit(f"inject: no single hook anchor in {function.strip()} of {path}")
    at = text.index(anchor)
    indent = function[: len(function) - len(function.lstrip())]
    if text.count(f"\n{indent}def ", start + 1, at):
        sys.exit(f"inject: the anchor is not in {function.strip()} of {path}")
    new = anchor + call if after else call + anchor
    path.write_text(text.replace(anchor, new), encoding="utf-8")


def main(tree):
    plugin = Path(tree) / "plugin"
    insert(
        plugin / "kfxgen" / "native_generator.py",
        "    def generate_full_book(",
        ANCHOR,
        CALL,
        after=False,
    )
    insert(
        plugin / "kfxgen" / "converter.py",
        "def convert_oeb_to_kfx(",
        BOOK_ANCHOR,
        BOOK_CALL,
        after=True,
    )
    insert(
        plugin / "__init__.py",
        "    def specialize_css_for_output(",
        FIRST_PASS_ANCHOR,
        FIRST_PASS_CALL,
        after=False,
    )
    here = Path(__file__).parent
    shutil.copy(here / "model_dump.py", plugin / "kfxgen" / "model_dump.py")
    shutil.copy(here / "book_dump.py", plugin / "kfxgen" / "book_dump.py")


if __name__ == "__main__":
    main(sys.argv[1])
