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
    from tests._helpers import MINIMAL_PNG, NullLog, jpeg_of
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

    def image_only_chapter(d):
        # A page of a picture alone; a heading and a picture, which kfxgen prints with no
        # heading of its own; a picture above the book's own heading.
        def page(title, body):
            return (
                '<?xml version="1.0" encoding="utf-8"?>\n'
                '<html xmlns="http://www.w3.org/1999/xhtml">'
                f"<head><title>{title}</title></head><body>{body}</body></html>"
            ).encode("utf-8")

        return (
            EpubBuilder()
            .set_metadata(title="Image Only Chapter", author="Fixture Author")
            .add_chapter("Opening", "Opening words.")
            .add_chapter("Plate", page("Plate", '<p><img src="plate.jpg" alt="a plate"/></p>'))
            .add_chapter(
                "Map", page("Map", '<h1>Map</h1><p><img src="map.png" alt="the map"/></p>')
            )
            .add_chapter(
                "Art",
                page("Art", '<p><img src="art.jpg" alt="art"/></p><h1>Art</h1><p>After it.</p>'),
            )
            .add_manifest_item(
                item_id="plate", href="plate.jpg", media_type="image/jpeg", data=jpeg_of(640, 480)
            )
            .add_manifest_item(
                item_id="map", href="map.png", media_type="image/png", data=MINIMAL_PNG
            )
            .add_manifest_item(
                item_id="art", href="art.jpg", media_type="image/jpeg", data=jpeg_of(200, 100)
            )
            .build(d, "image_only_chapter")
        )

    inputs = list(GOLDEN_INPUTS) + [
        ("unicode_text", unicode_text),
        ("empty_chapter", empty_chapter),
        ("image_only_chapter", image_only_chapter),
    ]
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


def _pic(href, alt, *more):
    """A picture's token (kfxgen `_img_tokens.py`): href, alt, then size and float."""
    return "\x00IMG\x01" + href + "\x01" + alt.replace(" ", "\x02") + "".join(
        "\x01" + m for m in more
    ) + "\x00"


def _jpeg(width, height, before=b""):
    """A JPEG's markers as far as kfxgen reads them: SOI, `before`, a SOF0 of width x height."""
    sof = b"\xff\xc0\x00\x11\x08" + height.to_bytes(2, "big") + width.to_bytes(2, "big")
    return b"\xff\xd8" + before + sof + b"\x03\x01\x22\x00\x02\x11\x01\x03\x11\x01\xff\xd9"


def _png(width, height):
    """A PNG's signature and IHDR, which hold its size."""
    size = width.to_bytes(4, "big") + height.to_bytes(4, "big")
    return b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR" + size + b"\x08\x02\x00\x00\x00"


_APP0 = b"\xff\xe0\x00\x04\x00\x00"
#: The covers of `cover_paths` and `cover_page`, each also a picture in its book's text.
_COVER = _jpeg(600, 800, _APP0)
_TALL_COVER = _png(1600, 2560)


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
    # Pictures found by exact, query, decoded, basename and absolute hrefs and not found;
    # every size hint and float; a picture block's ids, its heading level and space above;
    # ids of a picture the book doesn't hold, waiting for the next block, the chapter's
    # last entry, or its start; JPEGs whose size kfxgen reads and can't, over 1920 px, with
    # a restart marker.
    "image_refs": [
        _chapter(
            "Pictures",
            [
                _block("Pictures", heading=True, level=1),
                _block(
                    _pic("OEBPS/images/a.jpg", "first"),
                    keys=["c1.xhtml#fig", "c1.xhtml#late"],
                    offsets={"c1.xhtml#fig": 0, "c1.xhtml#late": 7},
                ),
                _block("Words between."),
                {
                    **_block(
                        " ".join(
                            [
                                _pic("OEBPS/images/a.jpg?v=1#frag", "query", "w=50%"),
                                " ",
                                _pic("../a.jpg", "up", "w=200px"),
                                _pic("OEBPS/other/a.jpg", "other", "h=50%"),
                                _pic("/abs/shown.jpg", "shown", "w=2em"),
                                _pic("OEBPS/%C3%A9.png", "accent", "h=100px"),
                                _pic("OEBPS/big.jpg", "big", "w=0%"),
                                _pic("OEBPS/eoi.jpg", "eoi", "w=12pt"),
                                _pic("OEBPS/short.jpg", "short", "w=150%"),
                                _pic("OEBPS/my pic.png", "spaced"),
                            ]
                        ),
                        heading=True,
                        level=2,
                    ),
                    "space_above": 2.0,
                },
                _block(_pic("missing/a.jpg", "lost"), keys=["c1.xhtml#lost"]),
                _block("After the lost picture."),
                _block(
                    _pic("OEBPS/images/a.jpg", "left one", "", "left")
                    + _pic("OEBPS/other/a.jpg", "right one", "w=50%", "right")
                    + _pic("OEBPS/images/a.jpg", "left again", "", "left")
                    + _pic("OEBPS/é.png", "tall left", "h=50%", "left")
                ),
                _block(
                    "\n".join(
                        _pic(f"OEBPS/{name}", name)
                        for name in ("badlen.jpg", "trunc.jpg", "nonff.jpg", "end.jpg", "tiny.png")
                    )
                ),
                _block(_pic("missing/b.jpg", "lost at the end"), keys=["c1.xhtml#end"]),
            ],
        ),
        _chapter("Ghost", [_block(_pic("nowhere.jpg", "ghost"), keys=["c2.xhtml#ghost"])]),
        _chapter(
            "Links",
            [
                _block(
                    "fig late lost end ghost",
                    [
                        (0, 3, _link("c1.xhtml#fig")),
                        (4, 4, _link("c1.xhtml#late")),
                        (9, 4, _link("c1.xhtml#lost")),
                        (14, 3, _link("c1.xhtml#end")),
                        (18, 5, _link("c2.xhtml#ghost")),
                    ],
                )
            ],
        ),
    ],
    # With unresolved hrefs (`resolved_image_refs` false) a picture is found by its name.
    "image_refs_unresolved": [
        _chapter("Loose", [_block("Loose", heading=True, level=1), _block(_pic("images/x.jpg", "x"))])
    ],
    # The cover: its picture opening the first chapter moves that block's ids to the next
    # and isn't printed twice; used again later in the book; a contents page's targets move
    # one on for the cover's page.
    "cover_paths": [
        _chapter(
            "Start",
            [
                _block(""),
                _block(_pic("cover.jpg", "cover"), keys=["c1.xhtml#cover"]),
                _block("Welcome to the book.", keys=["c1.xhtml"], offsets={"c1.xhtml": 0}),
            ],
        ),
        _chapter(
            "Gallery",
            [
                _block("Gallery", heading=True, level=1),
                _block(_pic("cover.jpg", "again")),
                _block(_pic("OEBPS/plate.png", "plate")),
            ],
        ),
        {
            "title": "Contents",
            "text": "Contents",
            "toc_level": 0,
            "toc_links": [
                {"text": "Start", "target_chapter_idx": 0, "level": 0},
                {"text": "Gallery", "target_chapter_idx": 1, "level": 0},
            ],
        },
        _chapter("Links", [_block("Back to the cover.", [(12, 5, _link("c1.xhtml#cover"))])]),
    ],
    # A first page holding only the cover's picture stays, as Kindle Previewer prints it; a
    # PNG cover over 1920 px.
    "cover_page": [
        _chapter("Cover", [_block(_pic("cover.png", "cover"))]),
        _chapter("One", [_block("One"), _block("The first chapter.")]),
    ],
    # A first page whose cover picture has only blank blocks after it: it stays, its ids
    # with it.
    "cover_alone": [
        _chapter(
            "Cover",
            [
                _block(_pic("cover.jpg", "cover"), keys=["c0.xhtml#cover"]),
                _block(""),
                _block("   "),
            ],
        ),
        _chapter(
            "One",
            [_block("One"), _block("Back to the cover.", [(12, 5, _link("c0.xhtml#cover"))])],
        ),
    ],
}

#: The image books' other arguments.
ARG_BOOK_EXTRAS = {
    "image_refs": {
        "images": {
            "OEBPS/images/a.jpg": _jpeg(300, 400, _APP0),
            "OEBPS/other/a.jpg": _jpeg(600, 100, _APP0),
            "OEBPS/shown.jpg": _jpeg(300, 400),
            "OEBPS/é.png": _png(256, 100),
            "OEBPS/my%20pic.png": _png(1024, 10),
            # Padding, a stuffed byte and a restart marker before its SOF0.
            "OEBPS/big.jpg": _jpeg(2400, 100, b"\xff\xff\xe1\x00\x04\x00\x00\xff\x00\xff\xd3"),
            "OEBPS/eoi.jpg": b"\xff\xd8\xff\xd9",
            "OEBPS/short.jpg": b"\xff\xd8\xff\xc0\x00\x11",
            "OEBPS/badlen.jpg": b"\xff\xd8\xff\xe1\x00\x01\x00\x00",
            "OEBPS/trunc.jpg": b"\xff\xd8\xff\xe1\x00",
            "OEBPS/nonff.jpg": b"\xff\xd8" + _APP0 + b"\x12\x34",
            "OEBPS/end.jpg": b"\xff\xd8" + _APP0,
            "OEBPS/tiny.png": b"\x89PNG\r\n\x1a\n\x00\x00\x00\x00",
            "OEBPS/anim.gif": b"GIF89a\x01\x00\x01\x00",
            "OEBPS/empty.jpg": b"",
            "OEBPS/unused.jpg": _jpeg(10, 10),
        },
        "display_dims": {"OEBPS/shown.jpg": (128.5, 50.0)},
        "resolved_image_refs": True,
    },
    "image_refs_unresolved": {"images": {"OEBPS/images/x.jpg": _jpeg(100, 100)}},
    "cover_paths": {
        "cover_image": _COVER,
        "images": {"cover.jpg": _COVER, "OEBPS/plate.png": _png(1024, 512)},
        "display_dims": {"cover.jpg": (300, 400)},
        "resolved_image_refs": True,
    },
    "cover_alone": {
        "cover_image": _COVER,
        "images": {"cover.jpg": _COVER},
        "resolved_image_refs": True,
    },
    "cover_page": {
        "cover_image": _TALL_COVER,
        "images": {"cover.png": _TALL_COVER},
        "resolved_image_refs": True,
    },
}


def args(kfxgen, out):
    sys.path[:0] = [str(kfxgen / "plugin"), str(kfxgen)]
    from kfxgen import model_dump
    from kfxgen.native_generator import NativeKFXGenerator

    for name, chapters in ARG_BOOKS.items():
        model = out / name
        os.environ["KFXGEN_DUMP_MODEL"] = str(model)
        kfx = NativeKFXGenerator().generate_full_book(
            title=name,
            author="Fixture Author",
            chapters=chapters,
            approximate_pages=True,
            **ARG_BOOK_EXTRAS.get(name, {}),
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
