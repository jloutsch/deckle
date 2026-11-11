#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# From kfxgen 8f55440 (research/generate_from_model.py), taking the kfxgen tree as an argument.
"""Make a KFX from a model dump: the generator alone, with no EPUB and no Calibre.

    KFXGEN_DUMP_MODEL=<dir> ebook-convert book.epub book.kfx   # writes <dir>/model.json
    python3 generate_from_model.py <kfxgen_tree> <dir> out.kfx

Deckle's Rust port of the generator is tested against what this makes.
"""

import sys
from pathlib import Path


def main(argv):
    if len(argv) != 4:
        print(f"usage: {argv[0]} <kfxgen_tree> <model_dir> <out.kfx>", file=sys.stderr)
        return 2
    sys.path.insert(0, str(Path(argv[1]) / "plugin"))
    from kfxgen import model_dump
    from kfxgen.native_generator import NativeKFXGenerator

    kwargs = model_dump.load(argv[2])
    data = NativeKFXGenerator().generate_full_book(output_path=argv[3], **kwargs)
    print(f"{argv[3]}: {len(data)} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
