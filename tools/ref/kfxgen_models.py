# SPDX-License-Identifier: GPL-3.0-or-later
"""The Python half of tools/ref/kfxgen_models.sh. Run with kfxgen's Python.

    kfxgen_models.py shim <kfxgen> <out>    kfxgen's golden inputs, plus unicode_text and
                                            empty_chapter, through its Calibre-free shim
    kfxgen_models.py synth <dir>            write the synthetic EPUBs (lists, fonts, rtl, frames)
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
import zlib
from pathlib import Path

CALIBRE_FONTS = Path("/Applications/calibre.app/Contents/Resources/resources/fonts/liberation")


def _pin_commit(model_dump):
    # model_dump asks git for the commit; an exported kfxgen has no .git, so the shell script
    # passes the commit it was exported from (KFXGEN_COMMIT).
    if os.environ.get("KFXGEN_COMMIT"):
        model_dump._commit = lambda: os.environ["KFXGEN_COMMIT"]


def shim(kfxgen, out):
    sys.path[:0] = [str(kfxgen / "plugin"), str(kfxgen)]
    from kfxgen import converter, model_dump
    from kfxgen.native_generator import NativeKFXGenerator
    from tests._helpers import MINIMAL_PNG, NullLog, jpeg_of
    from tests.fixtures.epub_builder import EpubBuilder
    from tests.fixtures.golden.inputs import GOLDEN_INPUTS
    from tests.fixtures.oeb_shim import EpubAsOeb

    _pin_commit(model_dump)

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


def _epub(path, title, lang, chapters, css="", rtl=False, extra=(), bare=(), vertical=()):
    """A small EPUB 3 with an NCX: chapters are (id, title, body_xhtml_inner), each opening
    with its title as an <h1> unless its id is in `bare`, written vertical-rl if its id is in
    `vertical`."""
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
                f"<body{' style=\"writing-mode: vertical-rl\"' if cid in vertical else ''}>"
                f"{'' if cid in bare else f'<h1>{ctitle}</h1>'}{body}</body></html>",
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
    # Direction and language inside a left-to-right book (#246, #605, #710, #714).
    _epub(
        out / "directions.epub",
        "Directions",
        "en",
        [
            (
                "c0",
                "Contents",
                '<ul><li><a href="c1.xhtml">Runs</a></li><li><a href="c2.xhtml">Paragraphs'
                '</a></li><li><a href="c3.xhtml">Tables and boxes</a></li></ul>',
            ),
            (
                "c1",
                "Runs",
                '<p>Plain words, then <span dir="rtl">שלום עולם</span> in a run, and '
                "<bdi>עברית</bdi> isolated.</p>"
                '<p><b>Bold <span dir="rtl">מודגש</span> around</b> it.</p>'
                '<p>A <span lang="fr">mot français</span> and <span lang="de">ein '
                "<i>deutsches</i> Wort</span>.</p>"
                '<p><a href="c2.xhtml#t">A link <span dir="rtl">קישור</span> holding</a> a '
                "direction.</p>"
                '<p><a href="c2.xhtml#t"><span dir="rtl">רק כיוון</span></a> a link of a '
                "direction alone.</p>"
                '<p><bdo dir="rtl">override</bdo> and <span dir="ltr">nested <span dir="rtl">'
                'פנימי <span lang="he">x</span></span> out</span>.</p>'
                '<p><span class="emb">embedded</span>, <span dir="auto">אוטומטי</span>, '
                '<span dir="rtl" class="big">גדול</span>, <i lang="fr">en italique</i>.</p>'
                '<p><span lang="fr"><span dir="rtl">מילה</span></span>, <span dir="rtl"><b>א'
                "</b> ב</span>.</p>"
                '<p><span lang="fr" class="big">Tout en grand.</span></p>',
            ),
            (
                "c2",
                "Paragraphs",
                '<p dir="rtl">פסקה מימין לשמאל with <span dir="ltr">left to right</span>.</p>'
                '<p dir="rtl" lang="he" class="start">יישור להתחלה.</p>'
                '<p lang="fr">Un paragraphe en français.</p>'
                '<p class="plain">A plaintext paragraph.</p>'
                '<p id="t">The target.</p>',
            ),
            (
                "c3",
                "Tables and boxes",
                '<table dir="rtl" lang="he"><thead lang="en"><tr><th>Head</th><th>Two</th></tr>'
                '</thead><tbody><tr dir="ltr"><td>cell</td><td lang="fr">cellule</td></tr>'
                "<tr><td>שורה</td><td>two</td></tr></tbody></table>"
                '<table><tr><td dir="rtl">תא</td><td>plain</td></tr></table>'
                '<div class="box" lang="fr"><p>Dans une boîte.</p><p lang="en">English in '
                'it.</p></div><div class="box" dir="rtl"><p>בתיבה.</p></div>',
            ),
            ("c4", "A table alone", '<table dir="rtl"><tr><td>טבלה</td></tr></table>'),
            ("c5", "Vertical", "<p>An English chapter set vertical-rl.</p>"),
        ],
        bare=("c4",),
        vertical=("c5",),
        css="span.emb { direction: rtl; unicode-bidi: embed; }\n"
        "span.big { font-size: 1.5em; }\n"
        "p.start { text-align: start; }\n"
        "p.plain { unicode-bidi: plaintext; }\n"
        "div.box { border: 1px solid black; padding: 0.5em; }",
    )
    # Ruby in a horizontal book (#236): readings in paragraphs and headings.
    _epub(
        out / "ruby.epub",
        "ルビ",
        "ja",
        [
            (
                "c1",
                "漢字かんじ",
                '<p class="t"><ruby><b>漢</b>字<rt>かんじ</rt></ruby></p>'
                "<p>前<ruby>漢<rt>かん</rt>字<rt>じ</rt></ruby>後。</p>"
                "<p><ruby><b>漢</b>字<rt>かんじ</rt></ruby>、二つの run の上に。</p>"
                "<p><b><ruby>強<rt>つよ</rt></ruby></b>、太字の上に。</p>"
                '<p><a href="c2.xhtml#t"><ruby>漢<rt>かん</rt></ruby>字へ</a>、リンクの中に。</p>'
                "<p><ruby>同<rt>おな</rt></ruby><ruby>同<rt>おな</rt></ruby>、同じ読み。</p>"
                "<p><ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp></ruby>、括弧の読み。</p>",
            ),
            (
                "c2",
                "第(だい)二(に)の章",
                '<p class="t"><ruby>第<rt>だい</rt></ruby><ruby>二<rt>に</rt></ruby>の章</p>'
                '<p id="t">ここが<ruby>行<rt>い</rt></ruby>き<ruby>先<rt>さき</rt></ruby>。</p>',
            ),
            (
                "c3",
                "三（さん）",
                '<p class="t"><ruby>三<rt>さん</rt></ruby></p><p>本文。</p>',
            ),
            (
                "c4",
                "四の章",
                "<h1><ruby>四<rt>よん</rt></ruby>の章</h1><p>読みの無い題。</p>",
            ),
            (
                "c5",
                "縦書きの章",
                '<p>縦に<ruby>書<rt>か</rt></ruby>く。<span class="up">ABC</span>は立てる。</p>'
                '<p class="m">余白のある段落。</p>'
                '<div class="box"><p>枠の中。</p></div>'
                "<table><tr><td>表</td><td>二</td></tr></table>",
            ),
        ],
        css="p.t { font-size: 1.5em; font-weight: bold; }\n"
        "p.m { margin: 1em 2em 0.5em 3em; }\n"
        "span.up { text-orientation: upright; }\n"
        "div.box { border: 1px solid black; padding: 1em 0.5em 0.25em 2em; }",
        bare=("c1", "c2", "c3", "c4"),
        vertical=("c5",),
    )
    # A vertical book's rebuilt contents page, a chapter of no writing mode (#247).
    _epub(
        out / "vertical_contents.epub",
        "縦の目次",
        "ja",
        [
            (
                "c0",
                "Contents",
                '<ul><li><a href="c1.xhtml">第一章</a></li><li><a href="c2.xhtml">第二章</a>'
                "</li></ul>",
            ),
            ("c1", "第一章", "<p>縦に書いた本文。</p>"),
            ("c2", "第二章", "<p>二つ目の章。</p>"),
        ],
        rtl=True,
        vertical=("c0", "c1", "c2"),
    )
    # Shaped as kfxgen's test_books/figures (#382) and floats (#243, #565), less what isn't
    # ported (a linked picture, a picture among words).
    after = "<p>The boats went out past the harbour wall, and the quay watched them go.</p>"
    _epub(
        out / "frames.epub",
        "Frames",
        "en",
        [
            (
                "c1",
                "Figure at 40%",
                '<div class="w40"><img src="p.png" alt="picture"/>'
                "<p>Figure 1. The harbour at low water.</p></div>" + after,
            ),
            (
                "c2",
                "Centred figures",
                '<div class="fig" style="width:60%"><img src="p.png" alt="picture"/></div>'
                '<div class="figcenter" style="width: 300px;">'
                '<img src="p.png" alt="picture" style="width: 300px;"/>'
                '<p class="caption">Figure 2. 300px in 300px.</p></div>'
                '<div class="figcenter" style="width: 12em;">'
                '<img src="p.png" alt="picture" style="width: 64px;"/></div>' + after,
            ),
            (
                "c3",
                "Floated figure",
                '<div class="f40"><img src="p.png" alt="picture"/><p>Figure 3. Beside.</p></div>'
                + after,
            ),
            (
                "c4",
                "Sizes in a box",
                '<div class="half"><img src="p.png" alt="no size"/></div>'
                '<div class="half"><img src="p.png" alt="px" style="width: 128px;"/></div>'
                '<div class="half"><img src="p.png" alt="high" style="height: 100px;"/></div>'
                '<div class="shaded"><img src="p.png" alt="shaded"/></div>' + after,
            ),
            (
                "c5",
                "Floats first",
                '<p><img class="fl" src="p.png" alt="left"/>Words beside the picture.</p>'
                '<p><img class="fl" src="p.png" alt="left"/> <img class="fr" src="p.png" '
                'alt="right"/> Words <i>between</i> both, '
                '<span id="mid">an id</span> in them.</p>'
                '<p id="whole"><img class="fr" src="p.png" alt="right"/>Its own id.</p>'
                + after
                + '<p><a href="c5.xhtml#mid">To the id</a>, <a href="c5.xhtml#whole">to the '
                "paragraph</a>.</p>",
            ),
            (
                "c6",
                "Float in a box",
                '<div class="half"><p><img class="fl" src="p.png" alt="left" '
                'style="width: 64px;"/>Words in a sized box.</p></div>' + after,
            ),
        ],
        css="div.w40 { width: 40%; } div.w40 img { width: 100%; }\n"
        "div.fig { display: block; margin: 0 auto; text-align: center; }"
        " div.fig img { width: 100%; }\n"
        "div.f40 { float: left; width: 40%; margin-right: 1em; } div.f40 img { width: 100%; }\n"
        ".figcenter { margin: auto; text-align: center; }\n"
        ".caption { font-size: smaller; text-align: center; }\n"
        "div.half { width: 50%; }\n"
        "div.shaded { width: 40%; background-color: #eeeeee; }\n"
        "img.fl { float: left; width: 30%; } img.fr { float: right; width: 25%; }",
        extra=[("p.png", "image/png", _png_image(600, 400))],
    )
    for name in ("lists", "fonts", "rtl", "directions", "ruby", "vertical_contents", "frames"):
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
    text = "\n\n".join(b["text"] for b in blocks if "text" in b)
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


def _png_image(width, height):
    """A whole PNG of one grey, which Calibre reads as a picture."""

    def chunk(kind, data):
        body = kind + data
        return len(data).to_bytes(4, "big") + body + zlib.crc32(body).to_bytes(4, "big")

    ihdr = width.to_bytes(4, "big") + height.to_bytes(4, "big") + b"\x08\x02\x00\x00\x00"
    rows = (b"\x00" + b"\x99" * 3 * width) * height
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


def _cell(text, spans=(), keys=(), **extra):
    """A table cell as the converter writes one, with no CSS."""
    cell = {
        "text": text,
        "spans": list(spans),
        "anchor_ids": [],
        "anchor_offsets": dict.fromkeys(keys, 0),
        "block_style": None,
        "header": False,
        "not_bold": False,
        "heading": False,
        "colspan": 1,
        "rowspan": 1,
        "valign": None,
        "background": None,
        "border": None,
        "padding": None,
        "width_set": False,
        "anchor_keys": list(keys),
    }
    cell.update(extra)
    return cell


def _row(cells, group="body", order=0, keys=(), **extra):
    """A table row: its group, its place in the source and its ids."""
    row = {
        "group": group,
        "border": None,
        "bidi": None,
        "group_bidi": None,
        "group_border": None,
        "anchor_ids": [],
        "cells": cells,
        "source_order": order,
        "anchor_keys": list(keys),
        "anchor_offsets": {},
    }
    row.update(extra)
    return row


def _table(rows, keys=(), stray=(), block_keys=None, **extra):
    """A table block, its text and ids gathered from its rows as the converter does; `stray`
    ids are the block's alone. `block_keys` replaces the block's ids."""
    table = {
        "anchor_ids": [],
        "rows": rows,
        "border": None,
        "column_widths": None,
        "width": None,
        "bidi": {},
        "anchor_keys": list(keys),
        "anchor_offsets": {},
    }
    table.update(extra)
    every = list(keys)
    for r in rows:
        every += r["anchor_keys"]
        for c in r["cells"]:
            every += c["anchor_keys"]
            for p in c.get("paragraphs") or ():
                every += p["anchor_keys"]
    every = list(dict.fromkeys(every + list(stray))) if block_keys is None else block_keys
    lines = (" ".join(c["text"] for c in r["cells"] if c["text"]) for r in rows)
    return {
        "type": "table",
        "text": "\n".join(line for line in lines if line),
        "spans": [],
        "block_style": None,
        "anchor_ids": [],
        "anchor_offsets": dict.fromkeys(every, 0),
        "table": table,
        "anchor_keys": every,
    }


def _style(px=16.0, line=1.2, bold=False, italic=False, family=("serif",), lang=None, **more):
    """A block style as the Calibre path writes one: every key set, None where the CSS has
    none."""
    style = {
        "align": None,
        "indent": None,
        "margin_left": None,
        "margin_right": None,
        "font_family": list(family),
        "line_height": line,
        "bold": bold,
        "italic": italic,
        "font_size": px,
    }
    if lang:
        style["bidi"] = {"lang": lang}
    style.update(more)
    return style


def _para(
    text,
    spans=(),
    keys=(),
    above=1.0,
    below=None,
    heading=False,
    level=None,
    scene=None,
    tight=False,
    **style,
):
    """A Calibre-path paragraph: a block style and the book's margins in ems. `above` is
    left out for "absent", as the converter leaves it out with no margin of its own; `scene`
    is a scene break's space above (#230); `tight` a list item's line (#701)."""
    block = _block(text, spans, keys, dict.fromkeys(keys, 0), heading, level)
    if scene is not None:
        block["space_above"] = scene
    if tight:
        block.update(in_list=True, tight=True)
    block["block_style"] = _style(**style)
    block["writing_mode"] = "horizontal-tb"
    if above != "absent":
        block["margin_above"] = above
    if below is not None:
        block["margin_below"] = below
    return block


def _box(blocks, keys=(), above=1.0, below=0.0, **look):
    """A box block (#238): a solid border and a padding unless `look` says otherwise."""
    box = {
        "border": {s: ("solid", 0.45, None) for s in ("top", "right", "bottom", "left")},
        "background": None,
        "padding": {s: 0.5 for s in ("top", "right", "bottom", "left")},
        "font_px": 16.0,
        "margin_left": None,
        "margin_right": None,
        "width": None,
        "width_px": None,
        "float": None,
        "centred": False,
        "bidi": {},
    }
    box.update(look)
    every = list(keys)
    for b in blocks:
        every += [k for k in b.get("anchor_keys") or () if k not in every]
    return {
        "type": "box",
        "box": box,
        "blocks": blocks,
        "text": "\n\n".join(b["text"] for b in blocks),
        "anchor_ids": [],
        "anchor_keys": every,
        "anchor_offsets": dict.fromkeys(every, 0),
        "margin_above": above,
        "margin_below": below,
        "writing_mode": "horizontal-tb",
    }


def _rule(**look):
    return {
        "type": "rule",
        "rule": {"space_above": 0.5, "space_below": 0.5, **look},
        "anchor_ids": [],
        "anchor_keys": [],
        "anchor_offsets": {},
        "writing_mode": "horizontal-tb",
    }


def _frame_edges():
    """A floated paragraph's space (tight, a scene break's, kfxgen's own above an indented
    paragraph and an unindented one, a margin of None, first in its chapter), its heading
    level, its ids (before, at and after the cut, at its end, with no offset) and a run over
    the cut, a float the book doesn't hold; pictures in sized boxes in pt, em, px against a
    box in px whose % width disagrees, and a size kfxgen can't read; a box in a sized box; a
    sized box bordered; floated right and to a side that isn't left; centred with no width; a
    width of 0 and a float of ""."""
    left = _pic("OEBPS/p.png", "left", "w=30%", "left")
    ids = left + "Ids in both parts."
    plain = {"border": None, "padding": None}

    def sized(hint, **look):
        return _box([_para(_pic("OEBPS/p.png", "p", hint), above=0.0)], **{**plain, **look})

    return [
        _chapter(
            "Floated",
            [
                _para("Floated", heading=True, level=1, above=0.67),
                {**_para(left + "A tight line."), "tight": True},
                _para(left + "A scene break above.", above="absent", scene=2.0),
                _para(left + "Indented, no margin.", above="absent", indent=("1.5", "$308")),
                _para(left + "Unindented, no margin.", above="absent"),
                _para(left + "A margin of None.", above=None, below=0.5),
                _para(left + "A floated heading.", heading=True, level=2),
                {
                    **_para(ids, [(0, len(left) + 3, frozenset({"bold"}))]),
                    "anchor_keys": [
                        "f.xhtml#head",
                        "f.xhtml#cut",
                        "f.xhtml#mid",
                        "f.xhtml#end",
                        "f.xhtml#none",
                    ],
                    "anchor_offsets": {
                        "f.xhtml#head": 1,
                        "f.xhtml#cut": len(left),
                        "f.xhtml#mid": len(left) + 4,
                        "f.xhtml#end": len(ids),
                    },
                },
                _para(
                    _pic("missing.png", "lost", "w=30%", "left") + "After a lost float.",
                    keys=["f.xhtml#lost"],
                ),
                _para("The lost float's id lands here."),
                _box([_para(left + "Floated in a bordered box.", above=0.0)]),
                _para(
                    "To head, cut, mid, end, none, lost.",
                    [
                        (3, 4, _link("f.xhtml#head")),
                        (9, 3, _link("f.xhtml#cut")),
                        (14, 3, _link("f.xhtml#mid")),
                        (19, 3, _link("f.xhtml#end")),
                        (24, 4, _link("f.xhtml#none")),
                        (30, 4, _link("f.xhtml#lost")),
                    ],
                ),
            ],
        ),
        _chapter(
            "First",
            [_para(left + "First in its chapter, after a scene break.", above="absent", scene=2.0)],
        ),
        _chapter(
            "Sized",
            [
                sized("w=12pt", width=50.0),
                sized("w=2em", width=25.0, width_px=100.0),
                _box([_para(_pic("OEBPS/q.png", "q", "w=50vw"), above=0.0)], **plain, width=50.0),
                _box(
                    [_box([_para(_pic("OEBPS/p.png", "inner", "w=100px"), above=0.0)], **plain)],
                    **{**plain, "width": 50.0},
                ),
                sized("w=60%", width=40.0, border={"bottom": ("solid", 0.45, None)}),
                sized("w=40%", width=30.0, float="right"),
                sized("w=20%", width=30.0, float="up"),
                sized("w=10%", width=30.0, float=""),
                sized("w=128px", width=0.0),
                _box([_para("Centred with no width.", above=0.0)], centred=True),
            ],
        ),
    ]


def _inline_pictures():
    """Pictures among words (#529, #565) past what the Calibre-path `floats` and `floats_697`
    reach: every size, given height and vertical-align; a picture the book doesn't hold or
    can't measure; runs and ids moved round them; a linked picture alone, at a location and
    with ids, and a linked float among words, cut into entries; floats of each size, a side
    that isn't left or right; a paragraph cut after its picture lines and at its empty lines,
    with spacers, ids in its blank lines and a run over the cut; pictures under two links'
    runs; a chapter opening on a scene break, one at 0.75 and one of plain text."""
    p, q = "OEBPS/p.png", "OEBPS/q.png"
    pic = _pic

    def words(*parts):
        return " ".join(parts)

    lost = pic("missing.png", "lost")
    lost_words = words(lost, "Lost", lost, "in words.")
    long_words = " ".join(["Filler words that run on."] * 60)
    long_text = words(long_words, pic(q, "q"), long_words, pic(p, "p", "w=2em"), long_words)
    linked = pic(p, "linked", "w=50%")
    linked_float = pic(p, "lf", "w=30%", "right")
    # kfxgen #21: the second "Same words" takes the first's place, runs and ids.
    same = words("Same words", linked_float, "Same words")
    at_cut = "Words above.\n" + pic(q, "q") + "\nWords below."
    blanks = "One line.\n" + "    " + "\nTwo lines.\nThree.\n\n\nFour."
    lost_cut = lost + " Bold words.\n\nNext."
    return [
        _chapter(
            "Sizes",
            [
                _para("Sizes", heading=True, level=1),
                _para(words("A picture", pic(q, "q"), "among words, and again", pic(q, "q"))),
                _para(
                    words(
                        "Widths",
                        *(pic(p, "p", s) for s in ("w=50%", "w=2em", "w=24px", "w=18pt")),
                        "end.",
                    )
                ),
                _para(
                    words(
                        "Heights",
                        *(pic(p, "p", s) for s in ("h=50%", "h=2em", "h=30", "h=12pt")),
                        "end.",
                    ),
                    px=24.0,
                ),
                _para(
                    words(
                        "Given heights",
                        *(
                            pic(p, "p", s, "", h)
                            for s, h in (
                                ("w=2em", "h=50%"),
                                ("w=50%", "h=50%"),
                                ("w=24px", "h=50%"),
                                ("w=18pt", "h=50%"),
                                ("w=24px", "h=2em"),
                                ("w=24px", "h=30"),
                            )
                        ),
                        "end.",
                    )
                ),
                _para(
                    words(
                        "Aligned",
                        *(
                            pic(q, "q", "", "", f"va={va}")
                            for va in (
                                "middle",
                                "top",
                                "bottom",
                                "text-top",
                                "text-bottom",
                                "baseline",
                                "super",
                                "sub",
                                "-2px",
                                "0",
                                "3",
                                "1.5em",
                                ".5rem",
                                "10%",
                                "2pt",
                                "bogus",
                            )
                        ),
                        "end.",
                    )
                ),
                _para(
                    lost_words,
                    [
                        (len(lost) + 1, 4, frozenset({"bold"})),
                        (len(lost) + 3, len(lost) + 2, frozenset({"italic"})),
                        (len(lost) + 6, 5, frozenset({"underline"})),
                    ],
                    keys=["s.xhtml#lost"],
                ),
                {
                    **_para(
                        words(pic("OEBPS/eoi.jpg", "unmeasured"), "Unmeasured, then linked",
                              pic(q, "q"), "and", pic(q, "q"), pic(q, "q") + "."),
                        [(len(pic("OEBPS/eoi.jpg", "unmeasured")) + 25, len(pic(q, "q")),
                          _link("s.xhtml#lost"))],
                        heading=True,
                        level=2,
                    ),
                    "anchor_keys": ["s.xhtml#a", "s.xhtml#b", "s.xhtml#c", "s.xhtml#d"],
                    "anchor_offsets": {"s.xhtml#a": 0, "s.xhtml#b": 30, "s.xhtml#c": 2000},
                },
                _para(
                    words(lost, "Words after a lost picture", pic(q, "q"), "end."),
                    [(len(lost) + 1, 5, frozenset({"bold"}))],
                ),
                _para(words("Ends on a picture", pic(q, "q"))),
                # Its size against the main text's rounds to 0 (`_rem`).
                _para(words("Tiny", pic(q, "q"), pic(p, "p", "w=24px")), px=0.0001),
                _box(
                    [_para(words("In a sized box", pic(q, "q", "w=50%"), "end."), above=0.0)],
                    border=None,
                    padding=None,
                    width=50.0,
                ),
                _para(long_text),
                _para(
                    "To lost, b, d, nowhere.",
                    [
                        (3, 4, _link("s.xhtml#lost")),
                        (9, 1, _link("s.xhtml#b")),
                        (12, 1, _link("s.xhtml#d")),
                        (15, 7, _link("l.xhtml#pending")),
                    ],
                ),
            ],
        ),
        _chapter(
            "Linked",
            [
                _para("Linked", heading=True, level=1),
                {
                    **_para(linked, [(0, len(linked), _link("s.xhtml#a"))]),
                    "anchor_keys": ["l.xhtml#start", "l.xhtml#on", "l.xhtml#bare"],
                    "anchor_offsets": {"l.xhtml#start": 0, "l.xhtml#on": 3},
                },
                _para(linked, [(0, len(linked), _link("nowhere.xhtml#x"))]),
                _para(linked_float, [(0, len(linked_float), _link("s.xhtml#a"))]),
                {
                    **_para(
                        same,
                        [
                            (11, len(linked_float), _link("s.xhtml#a")),
                            (len(same) - 10, 4, frozenset({"bold"})),
                        ],
                        level=3,
                    ),
                    "anchor_keys": ["l.xhtml#first", "l.xhtml#second"],
                    "anchor_offsets": {"l.xhtml#first": 2, "l.xhtml#second": len(same) - 5},
                },
                {
                    **_para(pic(q, "q") + "\n" + pic(q, "q")),
                    "anchor_keys": ["l.xhtml#lines"],
                    "anchor_offsets": {"l.xhtml#lines": len(pic(q, "q")) + 1},
                },
                _para(lost, keys=["l.xhtml#pending"]),
                _para(linked, [(0, len(linked), _link("l.xhtml#on"))]),
                _para(
                    "To on, first, second, bare, lines.",
                    [
                        (3, 2, _link("l.xhtml#on")),
                        (7, 5, _link("l.xhtml#first")),
                        (14, 6, _link("l.xhtml#second")),
                        (22, 4, _link("l.xhtml#bare")),
                        (28, 5, _link("l.xhtml#lines")),
                    ],
                ),
            ],
        ),
        _chapter(
            "At a location",
            [
                _para("x" * 109),
                _para(linked, [(0, len(linked), _link("s.xhtml#a"))]),
                _para(words("Two", pic(q, "q") + pic(q, "q"), "together.")),
            ],
            _omit_title_heading=True,
        ),
        _chapter(
            "Lines",
            [
                _para("Lines", heading=True, level=1),
                {
                    **_para(
                        at_cut,
                        [(6, 10, frozenset({"bold"}))],
                        level=3,
                        below=0.5,
                        indent=("1.5", "$308"),
                    ),
                    "anchor_keys": ["n.xhtml#a", "n.xhtml#b", "n.xhtml#c", "n.xhtml#d"],
                    "anchor_offsets": {
                        "n.xhtml#a": 12,
                        "n.xhtml#b": 13,
                        "n.xhtml#c": 14 + len(pic(q, "q")),
                        "n.xhtml#d": -1,
                    },
                },
                _para("Words.\n" + pic(q, "q")),
                {
                    **_para("Words.\n" + pic(q, "q") + "\n \n"),
                    "anchor_keys": ["n.xhtml#k"],
                    "anchor_offsets": {"n.xhtml#k": 8 + len(pic(q, "q"))},
                },
                _para(" \n\nWords after a blank start.\nAnd more."),
                _para("Words.\n\n \u00a0\nNext."),
                {
                    **_para("Verse one,\nverse two.\n\n\nVerse three.", level=2, scene=2.0,
                            above="absent"),
                    "anchor_keys": ["n.xhtml#e", "n.xhtml#f", "n.xhtml#g", "n.xhtml#h"],
                    "anchor_offsets": {
                        "n.xhtml#e": 22,
                        "n.xhtml#f": 23,
                        "n.xhtml#g": 24,
                        "n.xhtml#h": 99,
                    },
                },
                {
                    **_para(blanks, [(4, 12, frozenset({"italic"}))]),
                    "anchor_keys": ["n.xhtml#i", "n.xhtml#j", "n.xhtml#l"],
                    "anchor_offsets": {"n.xhtml#i": 12, "n.xhtml#j": 30, "n.xhtml#l": 10},
                },
                {
                    **_para("Words.\n" + pic(q, "q") + "\n\nMore words."),
                    "anchor_keys": ["n.xhtml#m"],
                    "anchor_offsets": {"n.xhtml#m": 8 + len(pic(q, "q"))},
                },
                _para("Words.\n\n" + pic(q, "q") + " among more words."),
                _para(lost_cut, [(len(lost) + 1, 4, frozenset({"bold"}))]),
                {**_para("A tight line.\n\nAnother."), "tight": True},
                _box([_para("In a box.\n\nCut.", above=0.0)]),
                _para(pic(p, "left", "w=30%", "left") + "Floated words.\n\nMore words."),
                _para(
                    "To a, b, c, d, e, f, g, h, i, j, k, l, m.",
                    [(3 + 3 * i, 1, _link(f"n.xhtml#{c}")) for i, c in enumerate("abcdefghijklm")],
                ),
            ],
        ),
        _chapter(
            "Floats",
            [
                _para("Floats", heading=True, level=1),
                _para(
                    words(
                        "Floats",
                        *(
                            pic(p, "f", s, side)
                            for s, side in (
                                ("w=2em", "left"),
                                ("w=24px", "right"),
                                ("w=12pt", "left"),
                                ("h=50%", "right"),
                                ("h=2em", "left"),
                                ("h=30", "right"),
                                ("h=12pt", "left"),
                                ("", "right"),
                                ("w=30%", "up"),
                            )
                        ),
                        "end.",
                    )
                ),
                _para(words("Spaces after", pic(p, "f", "w=20%", "left") + "   dropped.")),
                _para(words("Its own side", pic(p, "f", "w=20%", "left", "fl=right"), "wins.")),
                _para(
                    "Ends at the float" + pic(p, "f", "w=10%", "left") + " after.",
                    [(12, 5, _link("s.xhtml#a"))],
                ),
                _para(
                    pic(p, "lead", "w=20%", "left")
                    + words("Then", linked_float, "linked", pic(p, "f", "w=10%", "right"), "end."),
                    [(len(pic(p, "lead", "w=20%", "left")) + 5, len(linked_float),
                      _link("s.xhtml#a"))],
                ),
            ],
        ),
        _chapter(
            "Two links",
            [
                # A picture under two links' runs: kfxgen takes the first run's (:1826).
                _para(linked, [(0, len(linked), _link("s.xhtml#a")),
                               (0, len(linked), _link("l.xhtml#on"))]),
                _para(linked, [(0, len(linked), _link("l.xhtml#on")),
                               (0, len(linked), _link("s.xhtml#a"))]),
                _para(
                    words("Words", linked_float, "more."),
                    [(6, len(linked_float), _link("s.xhtml#a")),
                     (0, 6 + len(linked_float), _link("l.xhtml#on"))],
                ),
                _para(
                    words("Words", pic(q, "q"), "more."),
                    [(6, len(pic(q, "q")), _link("s.xhtml#a")),
                     (6, len(pic(q, "q")), _link("l.xhtml#on"))],
                ),
            ],
        ),
        _chapter(
            "Scene first",
            [_para("Opens after a scene break.", above="absent", scene=2.0)],
        ),
        _chapter(
            "Small",
            [
                _block("Small", heading=True, level=1),
                _block(words("Small", pic(q, "q"), pic(p, "p", "w=24px"), pic(p, "p", "h=2em"))),
                _block(linked, [(0, len(linked), _link("s.xhtml#a"))]),
            ],
            font_size=0.75,
        ),
        {
            "title": "Plain",
            "text": "Plain\n\nOne line.\n \nAfter a blank line.",
            "blocks": None,
            "toc_level": 0,
        },
    ]


def _on(text, words, *flags):
    """A run over the first `words` in `text`."""
    return (text.index(words), len(words), frozenset(flags))


def _mono(*families):
    return ("mono", tuple(families))


def _size(px):
    return ("size", px)


def _font_faces():
    """Embedded faces matched to paragraphs, runs and cells (#15, #50), monospace runs
    (#242) and runs of their own size (#241, #375) past what the Calibre-path `fonts` and
    `font_sizes` reach."""
    faces = "Regular, bold, italic, both and none."
    light = "A bold paragraph with a light word."
    upright = "An italic paragraph with an upright word."
    second = "Sans from its second family, bold and italic."
    code = "Code here, bold code, courier, fira, other, empty."
    linked = "See the linked code sample here, and there."
    sizes = "Bigger, main, smaller and sup words."
    twenty = "At twenty: back to main, same, linked same, lost same, up."
    serif = ("book serif", "serif")
    return [
        _chapter(
            "Faces",
            [
                _para("Faces", heading=True, level=1, px=24.0, bold=True, family=serif),
                _para(
                    faces,
                    [
                        _on(faces, "bold", "bold"),
                        _on(faces, "italic", "italic"),
                        _on(faces, "both", "bold", "italic"),
                    ],
                    family=serif,
                ),
                _para(light, [_on(light, "light", "not-bold")], bold=True, family=serif),
                _para(
                    upright, [_on(upright, "upright", "not-italic")], italic=True, family=serif
                ),
                _para(
                    second,
                    [_on(second, "bold and italic", "bold", "italic")],
                    family=("missing family", "book sans", "book serif"),
                ),
                _para("Monospace the book doesn't embed.", family=("Courier New", "monospace")),
                _para(
                    code,
                    [
                        _on(code, "Code", _mono("book mono", "monospace")),
                        _on(code, "bold code", "bold", _mono("book mono")),
                        _on(code, "courier", _mono("courier")),
                        _on(code, "fira", _mono("fira code", "x serif")),
                        _on(code, "other", _mono("x serif", "y sans")),
                        _on(code, "empty", _mono()),
                    ],
                    family=serif,
                ),
                # A link over monospace words, one italic: the run inside takes the link's
                # family, not the paragraph's (#715). A link set in no families: no style,
                # and an italic word in it keeps the paragraph's family.
                _para(
                    linked,
                    [
                        _on(linked, "linked", ("link", "c2.xhtml"), _mono("book mono")),
                        _on(linked, " code", ("link", "c2.xhtml"), _mono("book mono")),
                        _on(
                            linked,
                            " sample",
                            ("link", "c2.xhtml"),
                            _mono("book mono"),
                            "italic",
                        ),
                        (linked.index("there"), 3, frozenset({("link", "c1.xhtml"), _mono()})),
                        _on(linked, "re.", ("link", "c1.xhtml"), _mono(), "italic"),
                    ],
                    family=serif,
                ),
                _para(_pic("OEBPS/p.png", "a picture")),
            ],
        ),
        _chapter(
            "Sizes",
            [
                _para("Sizes", keys=["c2.xhtml"], heading=True, level=1, px=24.0, bold=True),
                _para(
                    sizes,
                    [
                        _on(sizes, "Bigger", _size(24.0)),
                        _on(sizes, "main", _size(16)),
                        _on(sizes, "smaller", _size(12.0), "bold"),
                        _on(sizes, "sup", "super"),
                    ],
                ),
                _para(
                    twenty,
                    [
                        _on(twenty, "back to main", _size(16.0)),
                        _on(twenty, "same", _size(20.0)),
                        _on(twenty, "linked same", _size(20.0), ("link", "c1.xhtml")),
                        _on(twenty, "lost same", _size(20.0), ("link", "missing.xhtml")),
                        _on(twenty, "up", _size(12.0), "super"),
                    ],
                    keys=["c1.xhtml"],
                    px=20.0,
                ),
                _para("Wholly in one sized run.", [(0, 24, frozenset({_size(24.0)}))], px=18.0),
                # A size of 0 counts no characters: the paragraph is at its own (#375).
                _para("A run of size 0.", [(0, 16, frozenset({_size(0)}))], px=20.0),
                # The book's main size is in runs: its paragraph's own is 20px (#375 review).
                _para(
                    "The main size is in runs. " * 80,
                    [(0, 26 * 80, frozenset({_size(16.0)}))],
                    px=20.0,
                ),
            ],
        ),
        # The book's heading in runs of one size prints at it, the space between unsized
        # (QA on #720); in runs of two sizes at its own (#721).
        _chapter(
            "One Size",
            [
                _para(
                    "One Size",
                    [(0, 3, frozenset({_size(40.0)})), (4, 4, frozenset({_size(40.0)}))],
                    heading=True,
                    level=1,
                    px=32.0,
                    bold=True,
                ),
                _para("Body."),
            ],
        ),
        _chapter(
            "Two Sizes",
            [
                _para(
                    "Two Sizes",
                    [(0, 3, frozenset({_size(40.0)})), (4, 5, frozenset({_size(20.0)}))],
                    heading=True,
                    level=1,
                    px=32.0,
                    bold=True,
                ),
                _para("Body."),
            ],
        ),
        # A heading of no CSS in one size: a book style of only that size. In two: none, so
        # kfxgen's heading keeps its size and its runs none (#241).
        _chapter(
            "No Style",
            [_block("No Style", [(0, 8, frozenset({_size(24.0)}))], heading=True), _para("Body.")],
        ),
        _chapter(
            "Unsized Heading",
            [
                _block(
                    "Unsized Heading",
                    [(0, 7, frozenset({_size(24.0), "bold"})), (8, 7, frozenset({_size(30.0)}))],
                    heading=True,
                ),
                _para("Body."),
            ],
        ),
        _chapter(
            "Cells",
            [
                _para("Cells", heading=True, level=1, px=24.0, bold=True),
                _table(
                    [
                        _row(
                            [
                                _cell("Head", header=True, block_style=_style(family=serif)),
                                _cell(
                                    "Leaning",
                                    block_style=_style(italic=True, family=serif),
                                ),
                                _cell(
                                    "Small and big",
                                    [(10, 3, frozenset({_size(24.0), _mono("book mono")}))],
                                    block_style=_style(family=serif),
                                ),
                                _cell(
                                    "Head lines",
                                    header=True,
                                    block_style=_style(family=serif),
                                    paragraphs=[
                                        _block("Head"),
                                        _block("lines", [(0, 5, frozenset({"italic"}))]),
                                    ],
                                ),
                            ]
                        )
                    ]
                ),
            ],
        ),
    ]


def _tiny_font(family, style):
    """A TrueType font of one empty glyph, made here: a face's bytes only travel through the
    generator, so the faces of `font_faces` need no glyphs."""
    from io import BytesIO

    from fontTools.fontBuilder import FontBuilder
    from fontTools.pens.ttGlyphPen import TTGlyphPen

    fb = FontBuilder(1000, isTTF=True)
    fb.font.recalcTimestamp = False
    fb.setupGlyphOrder([".notdef"])
    fb.setupCharacterMap({})
    fb.setupGlyf({".notdef": TTGlyphPen(None).glyph()})
    fb.setupHorizontalMetrics({".notdef": (500, 0)})
    fb.setupHorizontalHeader(ascent=800, descent=-200)
    fb.setupNameTable({"familyName": family, "styleName": style})
    fb.setupOS2()
    fb.setupPost()
    fb.font["head"].created = fb.font["head"].modified = 0
    out = BytesIO()
    fb.save(out)
    return out.getvalue()


def _font_faces_extras():
    """`font_faces`'s faces, as `faces_from_rules` makes them: a family of a bold, an italic
    and, last, a regular face; one of a bold italic and a bold, no regular; a monospace one of
    a regular and a 600 weight (the bold threshold)."""
    from kfxgen.font_table import Face, FontTable, emitted_name

    taken = set()
    faces = []
    for family, weight, italic, style in [
        ("book serif", 700, False, "Bold"),
        ("book serif", 400, True, "Italic"),
        ("book serif", 400, False, "Regular"),
        ("book sans", 700, True, "Bold Italic"),
        ("book sans", 700, False, "Bold"),
        ("book mono", 400, False, "Regular"),
        ("book mono", 600, False, "Semibold"),
    ]:
        faces.append(
            Face(
                css_family=family,
                weight=weight,
                italic=italic,
                data=_tiny_font(family.title(), style),
                emitted_family=emitted_name(family, weight, italic, taken),
                location=f"resource/font{len(faces)}",
            )
        )
    return {
        "font_table": FontTable(faces),
        "images": {"OEBPS/p.png": _png(600, 400)},
        "resolved_image_refs": True,
    }


def _link_colour_runs():
    """Link colours (#728, #732): written as the link's own when its target is found, as the
    text's when not, measured against the page (#383) unless a fill is open around it."""
    grey, grey_text = 0xFF999999, 0xFF767676
    yellow, white, navy = 0xFFFFFF00, 0xFFFFFFFF, 0xFF000080

    def lc(link, text):
        return ("link-colour", (link, text))

    def gone(text, words, colour, *more):
        return _on(text, words, ("link", "gone.xhtml#x"), lc(colour, colour), *more)

    def inside(text, target, colour):
        """A link over all of `text`, its last word in the link's colour."""
        last = text.rindex(" ") + 1
        return _block(
            text,
            [
                (0, last, _link(target)),
                (last, len(text) - last, _link(target, lc(colour, colour))),
            ],
        )

    return [
        _chapter(
            "Found",
            [
                _block("Found", heading=True, level=1),
                _block("Grey link here.", [_on("Grey link here.", "link", ("link", "lc.xhtml#t"), lc(grey, grey_text))]),
                _block("Black link here.", [_on("Black link here.", "link", ("link", "lc.xhtml#t"), lc(None, 0xFF000000))]),
                inside("Plain then grey", "lc.xhtml#t", grey),
                _block("The target.", keys=["lc.xhtml#t"]),
            ],
        ),
        _chapter(
            "Missing",
            [
                _block("Missing", heading=True, level=1),
                _block("Yellow gone here.", [gone("Yellow gone here.", "gone", yellow)]),
                _block("White gone here.", [_on("White gone here.", "gone", ("link", "gone.xhtml#x"), lc(None, white))]),
                _block("French gone here.", [_on("French gone here.", "gone", ("link", "gone.xhtml#x"), lc(None, white), ("lang", "fr"))]),
                _block("Navy gone here.", [gone("Navy gone here.", "gone", navy)]),
                _block("Bold gone here.", [gone("Bold gone here.", "gone", yellow, "bold")]),
                _block("Again gone here.", [gone("Again gone here.", "gone", yellow)]),
                inside("Plain then yellow", "gone.xhtml#x", yellow),
            ],
        ),
        _chapter(
            "Filled",
            [
                _block("Filled", heading=True, level=1),
                _box([_block("Boxed gone.", [gone("Boxed gone.", "gone", yellow)])], background=0xFFDDDDDD),
                _box([_block("Clear gone.", [gone("Clear gone.", "gone", yellow)])], background=0),
                _table(
                    [
                        _row(
                            [
                                _cell("Cell gone.", [gone("Cell gone.", "gone", yellow)], background=0xFF333333),
                                _cell(
                                    "Para gone.",
                                    background=0xFF333333,
                                    paragraphs=[_block("Para gone.", [gone("Para gone.", "gone", yellow)])],
                                ),
                                _cell("Bare gone.", [gone("Bare gone.", "gone", yellow)]),
                            ]
                        )
                    ]
                ),
                _block("After gone.", [gone("After gone.", "gone", yellow)]),
            ],
        ),
    ]


_APP0 =b"\xff\xe0\x00\x04\x00\x00"
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
    # Table parts: borders, widths, padding, alignment and fills; head, body and foot groups;
    # header cells, a header cell of paragraphs, runs in cells; empty cells, one holding an
    # id; ids of the table, of rows with no cells, of the block alone; a table of no rows
    # written; ids of a picture the book doesn't hold before and after a table.
    "table_parts": [
        _chapter(
            "Parts",
            [
                _block("Some parts."),
                _block(_pic("missing.png", "gone"), keys=["p.xhtml#pic"]),
                _table(
                    [
                        _row(
                            [
                                _cell("Name", header=True),
                                _cell("Kind", header=True, not_bold=True),
                                _cell("Note", [(0, 4, frozenset({"italic"}))], header=True),
                            ],
                            group="head",
                            group_border={
                                "top": ("solid", 1.0, None),
                                "bottom": ("double", 2.5, 0xFF000080),
                            },
                        ),
                        _row([], keys=["p.xhtml#skip"], order=1),
                        _row(
                            [
                                _cell(
                                    "Bold and link",
                                    [(0, 4, frozenset({"bold"})), (9, 4, _link("p.xhtml#ta"))],
                                    valign="top",
                                    background=0xFFEEEEEE,
                                    width_set=True,
                                    padding={"top": 0.5, "right": 0.5, "bottom": 0.0, "left": ("%", 5.0)},
                                    border={
                                        "top": ("solid", 1.0, 0xFFFF0000),
                                        "bottom": ("dotted", 0.5, None),
                                        "left": ("none", 1.0, None),
                                        "right": ("solid", 0, None),
                                    },
                                ),
                                _cell("", [(0, 1, frozenset({"bold"}))], keys=["p.xhtml#blank"]),
                                _cell("\xa0"),
                            ],
                            order=2,
                            border={s: ("solid", 0.75, None) for s in ("top", "right", "bottom", "left")},
                        ),
                        _row(
                            [
                                _cell("Sum", valign="bottom"),
                                _cell("x", valign="baseline"),
                                _cell("Plain"),
                                _cell(
                                    "Total all",
                                    header=True,
                                    paragraphs=[
                                        _block("Total"),
                                        _block("all", [(0, 3, frozenset({"italic"}))]),
                                    ],
                                ),
                            ],
                            group="foot",
                            order=3,
                        ),
                        _row([], group="foot", keys=["p.xhtml#end"], order=4),
                    ],
                    keys=["p.xhtml#ta"],
                    stray=["p.xhtml#stray"],
                    width=100.0,
                    border={s: ("solid", 1.0, None) for s in ("top", "right", "bottom", "left")},
                    column_widths=("col", [25.0, None, 50.0, None]),
                ),
                # A plain cell as in the table above: one style at either width.
                _table([_row([_cell("Plain"), _cell("Again")])], width=40.0),
                _block(_pic("missing.png", "gone"), keys=["p.xhtml#last"]),
            ],
        ),
        _chapter(
            "Columns",
            [
                _block(_pic("missing.png", "gone")),
                _table(
                    [
                        _row(
                            [
                                _cell(
                                    "Padded",
                                    padding={
                                        "top": 0.5,
                                        "right": 0.5,
                                        "bottom": 0.0,
                                        "left": ("%", 5.0),
                                    },
                                ),
                                _cell("Wide", colspan=2),
                                # Tiny negative sides: Python's quantize gives -0 and -0.001.
                                _cell(
                                    "Negative",
                                    padding={"right": -0.00005, "left": -0.00016},
                                ),
                                _cell(
                                    "Per cent",
                                    padding={
                                        "top": ("%", 10.0),
                                        "right": 0.0375,
                                        "bottom": 0.0375,
                                        "left": 0.0375,
                                    },
                                ),
                            ]
                        )
                    ],
                    width=50.0,
                    border={
                        s: ("dashed", 0.75, 0xFF336699) for s in ("top", "right", "bottom", "left")
                    },
                    column_widths=("cell", [30.0, 30.0, None, 40.0, None]),
                ),
            ],
            font_size=0.75,
        ),
        # Only a table of no rows written: the chapter's entry is a no-break space. Its border
        # differs by side, two of them in widths written with an exponent.
        _chapter(
            "",
            [
                _table(
                    [_row([], keys=["bare.xhtml#row"])],
                    keys=["bare.xhtml#t"],
                    stray=["bare.xhtml#stray"],
                    border={
                        "bottom": ("solid", 1.0, None),
                        "left": ("solid", 0.00005, None),
                        "right": ("solid", 2500000.0, None),
                    },
                )
            ],
            text="Bare",
            anchor_keys=["bare.xhtml"],
        ),
        # A picture's ids pass over a table of no rows written to the paragraph after it.
        _chapter(
            "Skipped",
            [
                _block(_pic("missing.png", "gone"), keys=["s.xhtml#pic"]),
                _table([_row([])]),
                _block("After."),
            ],
        ),
        _chapter(
            "Links",
            [
                _block(
                    "To t, skip, end, stray, blank, pic, last; bare t, row, stray, file; s.",
                    [
                        (3, 1, _link("p.xhtml#ta")),
                        (6, 4, _link("p.xhtml#skip")),
                        (12, 3, _link("p.xhtml#end")),
                        (17, 5, _link("p.xhtml#stray")),
                        (24, 5, _link("p.xhtml#blank")),
                        (31, 3, _link("p.xhtml#pic")),
                        (36, 4, _link("p.xhtml#last")),
                        (47, 1, _link("bare.xhtml#t")),
                        (50, 3, _link("bare.xhtml#row")),
                        (55, 5, _link("bare.xhtml#stray")),
                        (62, 4, _link("bare.xhtml")),
                        (68, 1, _link("s.xhtml#pic")),
                    ],
                )
            ],
        ),
    ],
    # A chapter's title in its table: in the first row, spelled over rows in source order;
    # over a paragraph and a row, the rest of the table kept; over a paragraph and the only
    # row, the table gone; a heading in a cell; a title-only chapter with an empty table.
    "table_title": [
        _chapter(
            "Census Table",
            [_table([_row([_cell("Census Table of 1801")]), _row([_cell("8,893")], order=1)])],
            anchor_keys=["t1.xhtml"],
        ),
        _chapter(
            "Over Rows",
            [
                _table(
                    [
                        _row([_cell("Over")], group="head"),
                        _row([_cell("Data")], order=2),
                        _row([_cell("Rows")], group="foot", order=1),
                    ]
                )
            ],
        ),
        _chapter(
            "Split Title",
            [
                _block("Split", keys=["t3.xhtml"]),
                _table(
                    [
                        _row([_cell("Title", keys=["t3.xhtml#c0"])], keys=["t3.xhtml#r0"]),
                        _row([_cell("Rest of it")], order=1),
                        _row([_cell("More")], order=2),
                    ]
                ),
            ],
        ),
        _chapter(
            "Gone Table",
            [
                _block("Gone", keys=["t4.xhtml"]),
                _table([_row([_cell("Table", keys=["t4.xhtml#c"])])]),
                _block("After the gone table."),
            ],
        ),
        _chapter(
            "Cell Head",
            [_block("Before the cell heading."), _table([_row([_cell("Heading here", heading=True)])])],
        ),
        # The table's ids only on its parts.
        _chapter(
            "Only",
            [
                _block("Only"),
                _table(
                    [_row([_cell("", keys=["t6.xhtml#c"])], keys=["t6.xhtml#r"])],
                    keys=["t6.xhtml#t"],
                    block_keys=[],
                ),
            ],
        ),
        # A table whose text opens with the title, its first row in the source not.
        _chapter(
            "Runs In",
            [_table([_row([_cell("Runs In here")], group="head", order=1), _row([_cell("Other")])])],
        ),
        # A picture, then a table holding the title's words.
        _chapter(
            "Pictured",
            [_block(_pic("missing.png", "gone")), _table([_row([_cell("Pictured")])])],
        ),
        _chapter(
            "Links",
            [
                _block(
                    "To one, r0, c0, gone, cell, row, t.",
                    [
                        (3, 3, _link("t1.xhtml")),
                        (8, 2, _link("t3.xhtml#r0")),
                        (12, 2, _link("t3.xhtml#c0")),
                        (16, 4, _link("t4.xhtml#c")),
                        (22, 4, _link("t6.xhtml#c")),
                        (28, 3, _link("t6.xhtml#r")),
                        (33, 1, _link("t6.xhtml#t")),
                    ],
                )
            ],
        ),
    ],
    # Paragraph and heading styles no Calibre-made fixture has: the book's heading aligned,
    # indented, italic and spaced below; centred, italic, taller lines, a hanging indent, a
    # scene break with no margin of the book's, monospace text, a language of its own.
    "block_styles": [
        _chapter(
            "Styled",
            [
                _para(
                    "Styled",
                    keys=["s1.xhtml"],
                    above=0.67,
                    below=0.5,
                    heading=True,
                    level=1,
                    px=32.0,
                    bold=True,
                    italic=True,
                    align="center",
                    margin_left=("5", "$314"),
                    margin_right=("5", "$314"),
                    lang="en",
                ),
                _para(
                    "Centred, italic and tall.",
                    [(9, 6, frozenset({"bold"}))],
                    below=0.5,
                    italic=True,
                    align="center",
                    line=1.8,
                ),
                _para("Lines set closer than Previewer writes.", above="absent", line=0.5),
                _para("A hanging indent.", above="absent", indent=("-1.5", "$308")),
                _para("An indent.", above="absent", scene=1.0, indent=("1.5", "$308")),
                _para("A tight list item.", above=1.0, tight=True),
                _para("A small scene break.", above="absent", scene=1.0),
                _para("A wide scene break.", above="absent", scene=2.5),
                _para(
                    "Mono with bold.",
                    [(10, 4, frozenset({"bold"}))],
                    family=("Courier New", "monospace"),
                ),
                _para("A margin of None.", above=None),
                _para("Right margin.", margin_right=("2", "$308")),
                _para("En français.", lang="fr"),
                _para("The main text, plain, to make 16 px and 1.2 the book's own."),
                _para("More of the main text, so that it stays the most."),
            ],
        ),
        # The title split over a heading and a table's row: the heading's style prints.
        _chapter(
            "Two Line",
            [
                _para("Two", above=0.5, bold=True, px=24.0, align="left"),
                _table([_row([_cell("Line")]), _row([_cell("Kept")], order=1)]),
                _para("Body."),
            ],
        ),
        # kfxgen's heading takes the page's language, here a box's; the box's own, and a
        # paragraph's that is the box's and one that isn't. Its padding in ems and %, at a
        # size not the main text's.
        _chapter(
            "Untitled page",
            [
                _box(
                    [_para("Boxed German.", lang="de"), _para("Boxed English.", lang="en")],
                    above=1.0,
                    below=0.5,
                    font_px=20.0,
                    padding={"top": 0.5, "right": ("%", 10.0), "bottom": 0.5, "left": 1.0},
                    margin_left=6.25,
                    bidi={"lang": "de"},
                ),
                _para("Après la boîte.", lang="fr"),
            ],
        ),
    ],
    # Two text sizes with as many characters each: the main size is the smaller (`_most`).
    "main_size_tie": [
        _chapter("Tie", [_para("Ten chars.", px=20.0), _para("Ten chars!", px=16.0)]),
    ],
    # The chapter's title in and around boxes: split over a box's lines, in a box in a box,
    # a box holding only the title, the title running on in a box, a book heading in a box
    # near the start; a box's own ids, a box in a box and a table in one, which sets no
    # language inside the box's; the title over rules; a rule as a storyline's first entry.
    "box_titles": [
        _chapter(
            "Split Title",
            [
                _box(
                    [_para("Split", keys=["b1.xhtml#a"]), _para("Title"), _para("Rest.")],
                    keys=["b1.xhtml"],
                ),
            ],
        ),
        _chapter(
            "Nested",
            [
                _box(
                    [
                        _box([_para("Nested", keys=["b2.xhtml#n"]), _para("Inner rest.")]),
                        _para("Outer rest."),
                    ]
                ),
            ],
        ),
        _chapter(
            "Only title",
            [
                _box([_para("Only title", below=0.8)], keys=["b3.xhtml"]),
                _para("After the box."),
            ],
        ),
        _chapter("Remainder", [_box([_para("Remainder."), _para("Second.")])]),
        # Split over a box's lines: one line in a box in a box, written otherwise than the
        # title; a paragraph and a table's row; a run a picture stops in a box in a box.
        _chapter(
            "Split-Title",
            [
                {
                    **_box(
                        [
                            {
                                **_box([_para("Split Title!", keys=["b5.xhtml#s"])]),
                                "anchor_keys": [],
                                "anchor_offsets": {},
                            },
                            _para("Rest."),
                        ]
                    ),
                    "anchor_keys": [],
                    "anchor_offsets": {},
                }
            ],
        ),
        _chapter(
            "Split Row",
            [
                _box(
                    [
                        _para("Split"),
                        _table([_row([_cell("Row")]), _row([_cell("Kept row")], order=1)]),
                        _para("After."),
                    ]
                )
            ],
        ),
        _chapter(
            "Split Pic",
            [
                _box([_box([_para("Split"), _para(_pic("missing.png", "x"))]), _para("More.")]),
                _para("Pic"),
            ],
        ),
        _chapter("Pictured box", [_box([_para(_pic("missing.png", "y")), _para("Words.")])]),
        # A box opening with the book's heading of the title: no heading near the start.
        _chapter("Boxed Title", [_box([_para("Boxed Title", heading=True), _para("Text.")])]),
        # A rule among the first blocks: no label over them, the split cut eats it.
        _chapter("Ruled Two", [_para("Ruled"), _rule(), _para("Two"), _para("Body.")]),
        _chapter(
            "Near",
            [
                _box(
                    [
                        _para("A boxed heading", heading=True, bold=True),
                        _box([_para("Box in a box.")], background=4292730333, padding=None),
                        _table(
                            [
                                _row(
                                    [
                                        _cell(
                                            "Cell note.",
                                            keys=["s7.xhtml#cellnote"],
                                            block_style=_style(
                                                italic=True,
                                                family=("monospace",),
                                                lang="fr",
                                                align="right",
                                            ),
                                        ),
                                        _cell("", block_style=_style(px=24.0)),
                                        _cell(
                                            "",
                                            block_style=_style(px=24.0),
                                            paragraphs=[
                                                {
                                                    "text": "In a big cell.",
                                                    "spans": [],
                                                    "anchor_ids": [],
                                                    "anchor_keys": [],
                                                    "anchor_offsets": {},
                                                    "heading": False,
                                                }
                                            ],
                                        ),
                                    ]
                                )
                            ]
                        ),
                    ],
                    keys=["s7.xhtml#own"],
                    bidi={"lang": "fr"},
                ),
            ],
            anchor_keys=["s7.xhtml"],
        ),
        _chapter(
            "Later heading",
            [_para("Intro."), _box([_para("Inside heading", heading=True)])],
        ),
        _chapter("Ruled", [_rule(), _para("Ruled", heading=True, bold=True, px=32.0), _rule()]),
        _chapter(
            "First rule",
            [_box([_para("")]), _rule(width=(50.0, "%")), _para("After the rule.")],
            _omit_title_heading=True,
        ),
        _chapter(
            "Links",
            [
                _para(
                    "To own, split, nested, only, note, line.",
                    [
                        (28, 4, _link("s7.xhtml#cellnote", "noteref")),
                        (34, 4, _link("b5.xhtml#s")),
                        (3, 3, _link("s7.xhtml#own")),
                        (8, 5, _link("b1.xhtml#a")),
                        (15, 6, _link("b2.xhtml#n")),
                        (23, 4, _link("b3.xhtml")),
                    ],
                )
            ],
        ),
    ],
    # Frames (#382, #565) past what the Calibre-path `frames` reaches.
    "frame_edges": _frame_edges(),
    # Pictures among words, linked pictures and paragraphs cut at their picture and empty
    # lines, past what the Calibre-path `floats` and `floats_697` reach.
    "inline_pictures": _inline_pictures(),
    "font_faces": _font_faces(),
    # An EPUB page list with no landmarks or cover; landmarks with no page list or cover.
    # Their targets are in ARG_BOOK_EXTRAS.
    "page_list_alone": [
        _chapter(
            "One",
            [
                _block("Opening words.", keys=["c1.xhtml"]),
                _block(
                    "The first page starts here and the second there.",
                    keys=["c1.xhtml#p1", "c1.xhtml#p2"],
                    offsets={"c1.xhtml#p1": 0, "c1.xhtml#p2": 31},
                ),
            ],
        ),
        _chapter(
            "Two",
            [
                _block("Two words.", keys=["c2.xhtml#top", "c2.xhtml#p3"], offsets={"c2.xhtml#p3": 4}),
                _block("Again.", keys=["c2.xhtml#p3", "c3.xhtml"], offsets={"c2.xhtml#p3": 2, "c3.xhtml": 3}),
            ],
        ),
    ],
    "landmarks_alone": [
        _chapter("Front", [_block("Front words.", keys=["f.xhtml"])]),
        _chapter(
            "Parts",
            [_block("Parts listed here.", keys=["toc.xhtml#list"], offsets={"toc.xhtml#list": 9})],
        ),
        _chapter(
            "Body",
            [
                _block("Body starts.", keys=["b.xhtml#s"]),
                _block("More body.", keys=["b2.xhtml"], offsets={"b2.xhtml": 5}),
            ],
        ),
    ],
    # 縦中横 whose texts fill kfxgen's content_math fragment (#247, :2380): 4,096 and 4,095
    # bytes, 8,191 in all, so the next text, of one byte, still joins them; at 8,192 the one
    # after starts content_math2.
    "tcy_notes": [
        _chapter(
            "縦中横",
            [
                _block(
                    "一\ufffc二\ufffc三\ufffc四\ufffc五",
                    [
                        (1, 1, frozenset({("tcy", (0, "1" * 4096))})),
                        (3, 1, frozenset({("tcy", (1, "2" * 4095))})),
                        (5, 1, frozenset({("tcy", (2, "1"))})),
                        (7, 1, frozenset({("tcy", (3, "34"))})),
                    ],
                )
            ],
        )
    ],
    # Link colours (#728, #732) past what the Calibre-path `link_greys` reaches: a link's
    # colour found and not, none for it, a run inside a link not found, on the page, in a
    # filled box and cell and on a box filled 0.
    "link_colour_runs": _link_colour_runs(),
}

#: The image books' other arguments.
ARG_BOOK_EXTRAS = {
    "tcy_notes": {"language": "ja", "rtl": True, "writing_mode": "vertical-rl"},
    "inline_pictures": {
        "images": {
            "OEBPS/p.png": _png(600, 400),
            "OEBPS/q.png": _png(256, 100),
            "OEBPS/eoi.jpg": b"\xff\xd8\xff\xd9",
        },
        "resolved_image_refs": True,
    },
    # Made after kfxgen is importable.
    "font_faces": _font_faces_extras,
    # Pages at ids with and without offsets, a whole file by its first id and by an id of
    # its own, an id declared twice (the first counts), and ones that land nowhere: another
    # file, an id in other capitals.
    "page_list_alone": {
        "page_list": [
            ("i", "c1.xhtml#p1"),
            ("ii", "c1.xhtml#p2"),
            ("x", "gone.xhtml#p"),
            ("iii", "C1.XHTML#P2"),
            ("3", "c2.xhtml#p3"),
            ("4", "c2.xhtml"),
            ("5", "c1.xhtml"),
            ("6", "c3.xhtml"),
            ("y", "gone.xhtml"),
        ],
    },
    # Landmarks that land nowhere, of a kind kfxgen doesn't know, at a file, at an id with an
    # offset, a second start after the first that lands (#609); a page list that lands
    # nowhere, with no approximate pages.
    "landmarks_alone": {
        "landmarks": [
            ("start", "Gone", "gone.xhtml#x"),
            ("other", "Skip", "gone.xhtml"),
            ("toc", "Contents", "toc.xhtml"),
            ("toc", "Missing", "toc.xhtml#nothere"),
            ("start", "Start", "b.xhtml#s"),
            ("toc", "List", "toc.xhtml#list"),
            ("start", "Again", "f.xhtml"),
            ("toc", "Case", "TOC.XHTML#LIST"),
            ("toc", "More", "b2.xhtml"),
        ],
        "page_list": [("1", "gone.xhtml#p")],
        "approximate_pages": False,
    },
    "box_titles": {"footnote_keys": {"s7.xhtml#cellnote"}},
    "frame_edges": {
        "images": {"OEBPS/p.png": _png(600, 400), "OEBPS/q.png": _png(256, 100)},
        "resolved_image_refs": True,
    },
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

    _pin_commit(model_dump)
    for name, chapters in ARG_BOOKS.items():
        model = out / name
        extras = ARG_BOOK_EXTRAS.get(name, {})
        if callable(extras):
            extras = extras()
        os.environ["KFXGEN_DUMP_MODEL"] = str(model)
        kfx = NativeKFXGenerator().generate_full_book(
            title=name,
            author="Fixture Author",
            chapters=chapters,
            **{"approximate_pages": True, **extras},
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
