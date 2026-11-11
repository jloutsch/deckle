# SPDX-License-Identifier: GPL-3.0-or-later
"""Put the model-dump hook into an export of kfxgen: python3 inject.py <kfxgen_tree>

Copies model_dump.py into plugin/kfxgen/ and makes generate_full_book call it first (the
call kfxgen's 55082ec had), so KFXGEN_DUMP_MODEL=<dir> dumps the generator's arguments.
Stops if the anchor line is not exactly once in generate_full_book.
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


def main(tree):
    plugin = Path(tree) / "plugin" / "kfxgen"
    path = plugin / "native_generator.py"
    text = path.read_text(encoding="utf-8")
    start = text.find("    def generate_full_book(")
    if text.count(ANCHOR) != 1 or start < 0 or text.index(ANCHOR) < start:
        sys.exit(f"inject: no single hook anchor in generate_full_book of {path}")
    if text.count("\n    def ", start + 1, text.index(ANCHOR)):
        sys.exit(f"inject: the anchor is not in generate_full_book of {path}")
    path.write_text(text.replace(ANCHOR, CALL + ANCHOR), encoding="utf-8")
    shutil.copy(Path(__file__).with_name("model_dump.py"), plugin / "model_dump.py")


if __name__ == "__main__":
    main(sys.argv[1])
