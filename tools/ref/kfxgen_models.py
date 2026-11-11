# SPDX-License-Identifier: GPL-3.0-or-later
"""The Python half of tools/ref/kfxgen_models.sh. Run with kfxgen's Python.

    kfxgen_models.py shim <kfxgen> <out>    kfxgen's golden inputs, plus unicode_text and
                                            empty_chapter, through its Calibre-free shim
    kfxgen_models.py synth <dir>            write the synthetic EPUBs (lists, fonts, rtl)
    kfxgen_models.py args <kfxgen> <out>    books given as the generator's arguments, for
                                            run flags neither path above can make alone

Each model folder gets model.json, resources/ and expected.kfx: what kfxgen makes from that
model (native KFX design, Oracles: generator).
"""

import hashlib
import os
import sys
import tempfile
import zipfile
from pathlib import Path

CALIBRE_FONTS = Path("/Applications/calibre.app/Contents/Resources/resources/fonts/liberation")


def shim(kfxgen, out):
    sys.path[:0] = [str(kfxgen / "plugin"), str(kfxgen)]
    from kfxgen import converter, model_dump
    from kfxgen.native_generator import NativeKFXGenerator
    from tests._helpers import NullLog
    from tests.fixtures.epub_builder import EpubBuilder
    from tests.fixtures.golden.inputs import GOLDEN_INPUTS
    from tests.fixtures.oeb_shim import EpubAsOeb

    def unicode_text(d):
        body = (
            '<?xml version="1.0" encoding="utf-8"?>\n'
            '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Unicode</title></head><body>'
            "<h1>Unicode</h1>"
            "<p>Composed é, decomposed é, CJK 漢字, emoji \U0001f600, "
            "math letter \U0001d518, then <i>an italic span after them</i> and the end.</p>"
            "<p>Second paragraph \U0001f600\U0001f600 with <b>bold</b>.</p>"
            "</body></html>"
        ).encode("utf-8")
        return (
            EpubBuilder()
            .set_metadata(title="Unicode Text", author="Fixture Author")
            .add_chapter("Unicode", body)
            .add_chapter("After", "A plain chapter after it.")
            .build(d, "unicode_text")
        )

    def empty_chapter(d):
        return (
            EpubBuilder()
            .set_metadata(title="Empty Chapter", author="Fixture Author")
            .add_chapter("One", "First chapter body.")
            .add_chapter("Two", "")
            .add_chapter("Three", "Third chapter body.")
            .build(d, "empty_chapter")
        )

    inputs = list(GOLDEN_INPUTS) + [("unicode_text", unicode_text), ("empty_chapter", empty_chapter)]
    with tempfile.TemporaryDirectory() as tmp:
        for name, builder in inputs:
            work = Path(tmp) / name
            work.mkdir()
            model = out / name
            os.environ["KFXGEN_DUMP_MODEL"] = str(model)
            kfx = work / f"{name}.kfx"
            converter.convert_oeb_to_kfx(EpubAsOeb(builder(work)), str(kfx), opts=None, log=NullLog())
            del os.environ["KFXGEN_DUMP_MODEL"]
            replay = NativeKFXGenerator().generate_full_book(**model_dump.load(model))
            if replay != kfx.read_bytes():
                sys.exit(f"{name}: the replayed model makes different bytes")
            (model / "expected.kfx").write_bytes(replay)
            print(f"shim {name}: {len(replay)} bytes")


def _epub(path, title, lang, chapters, css="", rtl=False, extra=()):
    """A small EPUB 3 with an NCX: chapters are (id, title, body_xhtml_inner)."""
    items, spine, nav, ncx = [], [], [], []
    for i, (cid, ctitle, body) in enumerate(chapters, 1):
        items.append(f'<item id="{cid}" href="{cid}.xhtml" media-type="application/xhtml+xml"/>')
        spine.append(f'<itemref idref="{cid}"/>')
        nav.append(f'<li><a href="{cid}.xhtml">{ctitle}</a></li>')
        ncx.append(
            f'<navPoint id="n{i}" playOrder="{i}"><navLabel><text>{ctitle}</text></navLabel>'
            f'<content src="{cid}.xhtml"/></navPoint>'
        )
    for href, mtype, _ in extra:
        items.append(f'<item id="x{len(items)}" href="{href}" media-type="{mtype}"/>')
    direction = ' page-progression-direction="rtl"' if rtl else ""
    dir_attr = ' dir="rtl"' if rtl else ""
    opf = f"""<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="id">urn:uuid:00000000-0000-4000-8000-{int(hashlib.sha256(title.encode()).hexdigest()[:12], 16) % 10**12:012d}</dc:identifier>
<dc:title>{title}</dc:title><dc:creator>Fixture Author</dc:creator><dc:language>{lang}</dc:language>
<meta property="dcterms:modified">2026-11-11T00:00:00Z</meta>
</metadata>
<manifest>
<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
<item id="css" href="style.css" media-type="text/css"/>
{chr(10).join(items)}
</manifest>
<spine toc="ncx"{direction}>{"".join(spine)}</spine>
</package>"""
    with zipfile.ZipFile(path, "w") as zf:
        # A fixed date, so the same inputs make the same bytes.
        def write(name, data, method=zipfile.ZIP_DEFLATED):
            info = zipfile.ZipInfo(name, date_time=(2026, 11, 11, 0, 0, 0))
            info.compress_type = method
            zf.writestr(info, data)

        write("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
        write(
            "META-INF/container.xml",
            '<?xml version="1.0"?><container version="1.0" '
            'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
            '<rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>'
            "</rootfiles></container>",
        )
        write("OEBPS/content.opf", opf)
        write("OEBPS/style.css", css)
        write(
            "OEBPS/nav.xhtml",
            '<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" '
            f'xmlns:epub="http://www.idpf.org/2007/ops"{dir_attr}><head><title>Contents</title></head>'
            f'<body><nav epub:type="toc"><ol>{"".join(nav)}</ol></nav></body></html>',
        )
        write(
            "OEBPS/toc.ncx",
            '<?xml version="1.0" encoding="utf-8"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" '
            f'version="2005-1"><head/><docTitle><text>{title}</text></docTitle>'
            f'<navMap>{"".join(ncx)}</navMap></ncx>',
        )
        for cid, ctitle, body in chapters:
            write(
                f"OEBPS/{cid}.xhtml",
                '<?xml version="1.0" encoding="utf-8"?>'
                f'<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="{lang}" lang="{lang}"{dir_attr}>'
                f'<head><title>{ctitle}</title><link rel="stylesheet" href="style.css"/></head>'
                f"<body><h1>{ctitle}</h1>{body}</body></html>",
            )
        for href, _, data in extra:
            write(f"OEBPS/{href}", data)


def synth(out):
    out.mkdir(parents=True, exist_ok=True)
    _epub(
        out / "lists.epub",
        "Lists",
        "en",
        [
            (
                "c1",
                "Bullets and numbers",
                "<p>Before the lists.</p>"
                "<ul><li>Disc one</li><li>Disc two<ul><li>Nested circle</li></ul></li></ul>"
                "<ol><li>Decimal one</li><li>Decimal two</li></ol>"
                '<ol type="a" start="3"><li>Letter c</li><li>Letter d</li></ol>'
                '<ul class="bare"><li>No marker</li></ul>'
                "<p>After the lists.</p>",
            ),
            (
                "c2",
                "Definitions",
                "<dl><dt>Term</dt><dd>Its definition.</dd><dt>Other</dt><dd>Another.</dd></dl>",
            ),
        ],
        css="ul.bare { list-style: none; } ol { list-style-type: decimal; }",
    )
    font = (CALIBRE_FONTS / "LiberationSerif-Regular.ttf").read_bytes()
    _epub(
        out / "fonts.epub",
        "Fonts",
        "en",
        [
            (
                "c1",
                "An embedded font",
                '<p class="lib">Set in Liberation Serif, embedded in the book.</p>'
                '<p class="lib"><i>Italic with no italic face embedded.</i></p>'
                "<p>Body text in the reader's font.</p>",
            )
        ],
        css='@font-face { font-family: "Fixture Serif"; src: url(LiberationSerif-Regular.ttf); }\n'
        'p.lib { font-family: "Fixture Serif", serif; }',
        extra=[("LiberationSerif-Regular.ttf", "font/ttf", font)],
    )
    _epub(
        out / "rtl.epub",
        "ספר",
        "he",
        [
            (
                "c1",
                "פרק א",
                "<p>שלום עולם, with an English phrase, "
                "ועוד משפט.</p>"
                "<p>מספר 123 בתוך הטקסט.</p>",
            ),
            ("c2", "פרק ב", "<p>פסקה שנייה.</p>"),
        ],
        rtl=True,
    )
    for name in ("lists", "fonts", "rtl"):
        print(out / f"{name}.epub")


def _block(text, spans=(), keys=(), offsets=None, heading=False, level=None):
    """A paragraph block as the converter writes one, with no CSS."""
    block = {
        "text": text,
        "spans": list(spans),
        "block_style": None,
        "preformatted": False,
        "anchor_ids": [],
        "heading": heading,
        "row": False,
        "anchor_offsets": dict(offsets or {}),
        "anchor_keys": list(keys),
    }
    if level:
        block["heading_level"] = level
    return block


def _chapter(title, blocks, **extra):
    text = "\n\n".join(b["text"] for b in blocks)
    return {"title": title, "text": text, "blocks": blocks, "toc_level": 0, **extra}


def _link(target, *flags):
    return frozenset({("link", target), *flags})


#: Run flags the shim can't make (it applies no CSS), on blocks with no CSS, which a
#: Calibre-path book always has: each book given as `generate_full_book`'s chapters.
ARG_BOOKS = {
    # A book heading's italic and not-bold words; a link leaving the book, one to nothing,
    # one in other capitals, one to a whole file; a link over a bold word next to more of
    # the same link; every shift and line; ids inside, at and past a paragraph's end.
    "inline_runs": [
        _chapter(
            "Runs",
            [
                _block(
                    "Runs  with words",
                    [(0, 4, frozenset({"italic"})), (6, 4, frozenset({"not-bold"}))],
                    heading=True,
                    level=1,
                ),
                _block(
                    "See the web and gone and CASE and file here.",
                    [
                        (8, 3, _link("url:https://example.com/a")),
                        (16, 4, _link("missing.xhtml#x")),
                        (25, 4, _link("C2.XHTML#T")),
                        (34, 4, _link("c3.xhtml")),
                        (39, 2, _link("c2.xhtml#t", "bold")),
                        (41, 3, _link("c2.xhtml#t")),
                    ],
                    keys=["c1.xhtml"],
                    offsets={"c1.xhtml": 0},
                ),
                _block(
                    "Up down top bottom under strike.",
                    [
                        (0, 2, frozenset({"super", "bold"})),
                        (3, 4, frozenset({"sub", "italic"})),
                        (8, 3, frozenset({"text-top"})),
                        (12, 6, frozenset({"text-bottom", "underline"})),
                        (19, 5, frozenset({"underline"})),
                        (25, 6, frozenset({"line-through", "baseline"})),
                    ],
                ),
            ],
        ),
        _chapter(
            "Two",
            [
                _block("Two"),
                _block(
                    "Target sentence with a marker at the end of it.",
                    keys=["c2.xhtml#t", "c2.xhtml#far"],
                    offsets={"c2.xhtml#t": 47, "c2.xhtml#far": 900},
                ),
            ],
        ),
        _chapter(
            "Three",
            [
                _block("Three", keys=["c3.xhtml"], heading=True, level=2),
                _block(
                    "  Indented start, bold here.",
                    [(23, 4, frozenset({"bold"}))],
                    keys=["c3.xhtml#m"],
                    offsets={"c3.xhtml#m": 3},
                ),
            ],
        ),
    ],
    # A contents page at 0.75 with nested lines and targets past either end; a chapter whose
    # first words are its title; a heading-only chapter whose ids land in kfxgen's heading;
    # a title spelled over two blocks; a smaller chapter with runs and links back.
    "nested_contents": [
        {
            "title": "Contents",
            "text": "Contents",
            "toc_level": 0,
            "toc_links": [
                {"text": "Opening words", "target_chapter_idx": 1, "level": 0},
                {"text": "Only", "target_chapter_idx": 2, "level": 1},
                {"text": "Split", "target_chapter_idx": 3, "level": 2},
                {"text": "Last", "target_chapter_idx": -1, "level": 0},
                {"text": "Far", "target_chapter_idx": 99, "level": 0},
            ],
            "font_size": 0.75,
            "anchor_keys": ["toc.xhtml"],
        },
        _chapter(
            "Opening",
            [
                _block(
                    "Opening words go on, then a link back.",
                    [(9, 5, frozenset({"italic"})), (34, 4, _link("c4.xhtml#only"))],
                    keys=["c1.xhtml#a"],
                    offsets={"c1.xhtml#a": 20},
                )
            ],
        ),
        _chapter(
            "Only Heading",
            [
                _block(
                    "  Only Heading",
                    [(2, 4, frozenset({"bold"}))],
                    keys=["c2.xhtml#h", "c2.xhtml"],
                    offsets={"c2.xhtml#h": 9},
                    heading=True,
                    level=1,
                )
            ],
        ),
        _chapter(
            "3 Split Title",
            [
                _block("3", keys=["c3.xhtml#n"]),
                _block("Split Title", keys=["c3.xhtml#t"]),
                _block("Body after the split title.", [(0, 4, _link("c1.xhtml#a"))]),
            ],
        ),
        _chapter(
            "Smaller",
            [
                _block("Smaller"),
                _block(
                    "Text at a smaller size with italic.", [(29, 6, frozenset({"italic"}))]
                ),
                _block(
                    "Links: here.",
                    [(7, 4, _link("c3.xhtml#n")), (0, 5, _link("c2.xhtml#h"))],
                    keys=["c4.xhtml#only"],
                    offsets={"c4.xhtml#only": 3},
                ),
            ],
            font_size=0.75,
        ),
    ],
    # The book's heading is its title with words after it: its runs and ids print in
    # kfxgen's heading, and links land there.
    "heading_runs": [
        _chapter(
            "Opening",
            [
                _block(
                    "Opening \u2014 \u2026",
                    [(0, 7, frozenset({"bold"})), (8, 3, frozenset({"italic"}))],
                    keys=["c1.xhtml#a", "c1.xhtml#z"],
                    offsets={"c1.xhtml#a": 10, "c1.xhtml#z": 2},
                ),
                _block(
                    "Then a link back.",
                    [(7, 4, _link("c1.xhtml#a")), (12, 4, _link("c1.xhtml#z"))],
                ),
            ],
        )
    ],
}


def args(kfxgen, out):
    sys.path[:0] = [str(kfxgen / "plugin"), str(kfxgen)]
    from kfxgen import model_dump
    from kfxgen.native_generator import NativeKFXGenerator

    for name, chapters in ARG_BOOKS.items():
        model = out / name
        os.environ["KFXGEN_DUMP_MODEL"] = str(model)
        kfx = NativeKFXGenerator().generate_full_book(
            title=name, author="Fixture Author", chapters=chapters, approximate_pages=True
        )
        del os.environ["KFXGEN_DUMP_MODEL"]
        replay = NativeKFXGenerator().generate_full_book(**model_dump.load(model))
        if replay != kfx:
            sys.exit(f"{name}: the replayed model makes different bytes")
        (model / "expected.kfx").write_bytes(replay)
        print(f"args {name}: {len(replay)} bytes")


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "shim" and len(sys.argv) == 4:
        shim(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
    elif cmd == "synth" and len(sys.argv) == 3:
        synth(Path(sys.argv[2]).resolve())
    elif cmd == "args" and len(sys.argv) == 4:
        args(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
    else:
        sys.exit(__doc__)
