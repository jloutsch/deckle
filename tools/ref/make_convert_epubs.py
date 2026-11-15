# SPDX-License-Identifier: GPL-3.0-only
"""Build the small sources for the converter's dumps: python3 make_convert_epubs.py OUT_DIR

Run with kfxgen's Python (Pillow and fontTools). Writes, into OUT_DIR (fixtures/kfxgen/sources),
each a few kilobytes of hand-written markup, named for what it holds:

- text_edges.epub: text where Python and Rust differ (plan Review Focus 3): a paragraph of only
  no-break spaces, U+001C in text (in a file of its own, as XML refuses it), a tab in <pre>,
  a literal U+2028, astral characters before a link; shifts inside shifts (<sup> in <sup>,
  <sub> in a raised span), a not-bold span in bold text and in a heading, <sub>,
  line-through by <s>, <del> and CSS.
- block_edges.epub: <pre> with spaces, tabs and blank lines; hidden elements (the hidden
  attribute, display: none, a hidden span in a paragraph); an <epub:switch> with a case and a
  default; page breaks before and after; clear after a float; a comment inside a paragraph,
  and one between paragraphs.
- tables_more.epub: a table on the rows path (a hidden row, text loose in a row group); a
  notes table (two cells a row, markers in the first, every row linked from the text); a row
  holding a table (split); a caption; colspan and rowspan; cells of paragraphs.
- pictures_more.epub: a GIF, a CMYK JPEG, a 2,400 px PNG of one flat grey, an <svg> wrapping
  an <image>, and a page drawn by a CSS background picture. No SVG file, no SVG of shapes.
- fonts_woff.epub: one WOFF 1 face, Liberation Serif (from fixtures/kfxgen/sources/fonts.epub,
  licence beside it as LiberationSerif-OFL.txt) cut to the glyphs the book uses. No WOFF 2.
- package_more.epub: landmarks from the OPF guide only, an NCX pageList, a year-only date, a
  publisher, a contents page listing in a class="toc" block, and a dedication.
- listing_rerun.epub: a contents listing (class="toc") on an opening page that also holds the
  title and a byline, no chapter titled Contents and no heading-sized page with the listing,
  so kfxgen gets no Contents page and converts the book again keeping the listing
  (_listing_without_contents, the second pass with keep_nav_listings).
- marks.epub: link colours (a:link, a class's a:visited, a bare a:visited, a link's own style
  colour, a link with no href) in a horizontal file of a vertical Japanese book, and a
  vertical-rl file with a horizontal block, vertical text again inside it.

No <math> anywhere. Fixed zip dates and fonts saved without a new timestamp: the same script
with the same Pillow and fontTools makes the same bytes.
"""

import io
import struct
import sys
import zipfile
import zlib
from pathlib import Path

XHTML_NS = "http://www.w3.org/1999/xhtml"
EPUB_NS = "http://www.idpf.org/2007/ops"
CONTAINER = (
    '<?xml version="1.0"?><container version="1.0" '
    'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
    '<rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>'
    "</rootfiles></container>"
)
SOURCES = Path(__file__).resolve().parents[2] / "fixtures" / "kfxgen" / "sources"
CSS = "p { margin: 0; text-indent: 1em }\nh1 { text-align: center }\n"


def png(w, h, rgb=None, grey=None):
    """A PNG of one flat colour (or one grey, a byte a pixel)."""
    pixel = bytes([grey]) if grey is not None else bytes(rgb)
    raw = (b"\x00" + pixel * w) * h

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    colour_type = 0 if grey is not None else 2
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, colour_type, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def page(title, body, css="style.css", lang=None, body_attrs=""):
    link = f'<link rel="stylesheet" type="text/css" href="{css}"/>' if css else ""
    lang = f' xml:lang="{lang}" lang="{lang}"' if lang else ""
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        f'<html xmlns="{XHTML_NS}" xmlns:epub="{EPUB_NS}"{lang}>'
        f"<head><title>{title}</title>{link}</head>\n"
        f"<body{body_attrs}>\n{body}\n</body>\n</html>\n"
    )


def ncx(title, points, extra=""):
    """An NCX of ``points`` ((label, src), in order, all at the top level)."""
    nav = "".join(
        f'<navPoint id="n{i}" playOrder="{i}"><navLabel><text>{label}</text></navLabel>'
        f'<content src="{src}"/></navPoint>'
        for i, (label, src) in enumerate(points, 1)
    )
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/>'
        f"<docTitle><text>{title}</text></docTitle><navMap>{nav}</navMap>{extra}</ncx>\n"
    )


MEDIA = {
    "xhtml": "application/xhtml+xml",
    "css": "text/css",
    "png": "image/png",
    "gif": "image/gif",
    "jpg": "image/jpeg",
    "woff": "application/font-woff",
}


def book(out, name, title, pages, files=(), lang="en", metadata="", guide="", ncx_extra="",
         spine_attrs="", css=CSS):
    """Write ``out/name.epub``, an EPUB 2: ``pages`` ((id, label or None, xhtml), in spine
    order; a label puts it in the NCX), ``files`` ((name, bytes), in the manifest only)."""
    items = [("style.css", css)] + [(f"{pid}.xhtml", text) for pid, _, text in pages]
    items += list(files)
    manifest = "\n".join(
        ['<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>']
        + [
            f'<item id="i{n}" href="{fname}" media-type="{MEDIA[fname.rpartition(".")[2]]}"/>'
            for n, (fname, _) in enumerate(items)
        ]
    )
    spine = "".join(f'<itemref idref="i{n}"/>' for n in range(1, len(pages) + 1))
    opf = f"""<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
<dc:identifier id="id">urn:uuid:00000000-0000-4000-8000-{sum(map(ord, title)):012d}</dc:identifier>
<dc:title>{title}</dc:title><dc:creator opf:role="aut">Fixture Author</dc:creator>
<dc:language>{lang}</dc:language>{metadata}
</metadata>
<manifest>
{manifest}
</manifest>
<spine toc="ncx"{spine_attrs}>{spine}</spine>
{guide}
</package>
"""
    points = [(label, f"{pid}.xhtml") for pid, label, _ in pages if label]
    with zipfile.ZipFile(out / f"{name}.epub", "w") as zf:

        def write(fname, data, method=zipfile.ZIP_DEFLATED):
            info = zipfile.ZipInfo(fname, date_time=(2026, 11, 14, 0, 0, 0))
            info.compress_type = method
            zf.writestr(info, data)

        write("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
        write("META-INF/container.xml", CONTAINER)
        write("OEBPS/content.opf", opf)
        write("OEBPS/toc.ncx", ncx(title, points, ncx_extra))
        for fname, data in items:
            write(f"OEBPS/{fname}", data)


def text_edges(out):
    nbsp = " "
    edges = page(
        "Edges",
        "<h1>Text edges</h1>\n"
        f"<p>{nbsp}</p>\n"
        f"<p>{nbsp}{nbsp}{nbsp}</p>\n"
        "<p>A line broken by a literal line separator.</p>\n"
        "<pre>tab\tstop\n\tindented by one tab\nend</pre>\n"
        '<p>\U0001d518\U0001f600\U00020000 then <a href="#target">a link after astral '
        "characters</a>.</p>\n"
        '<p id="target">The target paragraph.</p>',
    )
    shifts = page(
        "Shifts",
        "<h1>Shifts <span class=\"light\">and weights</span></h1>\n"
        "<p>x<sup>2<sup>n</sup></sup> and H<sub>2<sub>o</sub></sub> and "
        '<span class="up">up <sub>down</sub> up</span> and <sub>low</sub> text.</p>\n'
        '<p><b>Bold <span class="light">not bold</span> bold again.</b></p>\n'
        '<p><s>struck</s>, <del>deleted</del> and <span class="through">crossed</span>.</p>',
    )
    controls = page("Controls", "<p>One\x1ctwo, split by U+001C.</p>")
    css = CSS + (
        ".light { font-weight: normal }\n.up { vertical-align: super }\n"
        ".through { text-decoration: line-through }\n"
    )
    book(
        out,
        "text_edges",
        "Text Edges",
        [("edges", "Edges", edges), ("shifts", "Shifts", shifts), ("controls", None, controls)],
        css=css,
    )


def block_edges(out):
    pre = page(
        "Preformatted",
        "<h1>Preformatted</h1>\n"
        "<pre>  two leading spaces\n\tone tab\n\n\nafter two blank lines   \n"
        "<b>bold</b> inside</pre>\n"
        "<p>Before a comment<!-- a comment inside the paragraph --> and after it.</p>\n"
        "<!-- a comment between paragraphs -->\n"
        "<p>The next paragraph.</p>",
    )
    hidden = page(
        "Hidden",
        "<h1>Hidden</h1>\n"
        "<p hidden=\"hidden\">Hidden by the attribute.</p>\n"
        '<div class="none"><p>Hidden by CSS.</p></div>\n'
        '<p>Shown, with <span class="none">a hidden span</span> inside.</p>\n'
        "<epub:switch>"
        '<epub:case required-namespace="urn:example:none"><p>A case.</p></epub:case>'
        "<epub:default><p>The default, shown.</p></epub:default>"
        "</epub:switch>\n"
        "<p>After the switch.</p>",
    )
    breaks = page(
        "Breaks",
        "<h1>Breaks</h1>\n"
        '<p class="before">A page break before this paragraph.</p>\n'
        '<p class="after">A page break after this one.</p>\n'
        '<p><span class="float">Float</span>Text beside a float.</p>\n'
        '<p class="clear">Cleared below the float.</p>',
    )
    css = CSS + (
        ".none { display: none }\n.before { page-break-before: always }\n"
        ".after { break-after: page }\n.float { float: left; font-size: 2em }\n"
        ".clear { clear: both }\n"
    )
    book(
        out,
        "block_edges",
        "Block Edges",
        [("pre", "Preformatted", pre), ("hidden", "Hidden", hidden), ("breaks", "Breaks", breaks)],
        css=css,
    )


def tables_more(out):
    tables = page(
        "Tables",
        "<h1>Tables</h1>\n"
        "<p>Notes<a href=\"#n1\">1</a>, <a href=\"#n2\">2</a> and <a href=\"#n3\">3</a>.</p>\n"
        "<table><caption>Spans and paragraphs</caption><tbody>"
        '<tr><th colspan="2">Two columns</th><th rowspan="2">Two rows</th></tr>'
        "<tr><td><p>First paragraph.</p><p>Second paragraph.</p></td><td>Plain</td></tr>"
        "</tbody></table>\n"
        "<table><tbody>"
        "<tr><td>Shown row</td><td>a</td></tr>"
        '<tr class="none"><td>Hidden row</td><td>b</td></tr>'
        "Loose text in the row group"
        "<tr><td>Last row</td><td>c</td></tr>"
        "</tbody></table>\n"
        "<table><tbody>"
        "<tr><td>Before the inner table"
        "<table><tbody><tr><td>inner</td><td>cells</td></tr></tbody></table>"
        "</td><td>Beside it</td></tr>"
        "<tr><td>Second row</td><td>d</td></tr>"
        "</tbody></table>\n"
        "<h2>Notes</h2>\n"
        "<table><tbody>"
        '<tr id="n1"><td>1.</td><td>The first note.</td></tr>'
        '<tr id="n2"><td>2.</td><td>The second note.</td></tr>'
        '<tr id="n3"><td>3.</td><td>The third note.</td></tr>'
        "</tbody></table>",
    )
    book(
        out,
        "tables_more",
        "Tables More",
        [("tables", "Tables", tables)],
        css=CSS + ".none { display: none }\ntd, th { border: 1px solid black }\n",
    )


def pictures_more(out):
    from PIL import Image

    def save(image, fmt, **kw):
        buf = io.BytesIO()
        image.save(buf, fmt, **kw)
        return buf.getvalue()

    gif = save(Image.new("P", (16, 12), 1), "GIF")
    cmyk = save(Image.new("CMYK", (24, 16), (0, 128, 255, 32)), "JPEG", quality=90)
    pictures = page(
        "Pictures",
        "<h1>Pictures</h1>\n"
        '<p><img src="small.gif" alt="A GIF"/></p>\n'
        '<p><img src="cmyk.jpg" alt="A CMYK JPEG"/></p>\n'
        '<p><img src="wide.png" alt="A wide grey PNG"/></p>',
    )
    wrapper = page(
        "Wrapper",
        '<div><svg xmlns="http://www.w3.org/2000/svg" '
        'xmlns:xlink="http://www.w3.org/1999/xlink" version="1.1" width="100%" height="100%" '
        'viewBox="0 0 30 40" preserveAspectRatio="xMidYMid meet">'
        '<image width="30" height="40" xlink:href="page.png"/></svg></div>',
    )
    background = page("Background", '<div class="bg"></div>')
    book(
        out,
        "pictures_more",
        "Pictures More",
        [
            ("pictures", "Pictures", pictures),
            ("wrapper", "Wrapper", wrapper),
            ("background", "Background", background),
        ],
        files=[
            ("small.gif", gif),
            ("cmyk.jpg", cmyk),
            ("wide.png", png(2400, 1200, grey=0x80)),
            ("page.png", png(30, 40, rgb=(200, 160, 80))),
            ("back.png", png(30, 40, rgb=(60, 120, 90))),
        ],
        css=CSS + ".bg { background-image: url(back.png); background-repeat: no-repeat; "
        "height: 100% }\n",
    )


FONT_TEXT = "Fonts WOFF: set in a face packed as WOFF one."


def fonts_woff(out):
    import logging

    from fontTools import subset
    from fontTools.ttLib import TTFont

    # It says it drops the FFTM table, which is no glyph.
    logging.getLogger("fontTools.subset").setLevel(logging.ERROR)

    with zipfile.ZipFile(SOURCES / "fonts.epub") as z:
        ttf = z.read("OEBPS/LiberationSerif-Regular.ttf")
    font = TTFont(io.BytesIO(ttf), recalcTimestamp=False)
    options = subset.Options()
    options.flavor = "woff"
    sub = subset.Subsetter(options)
    sub.populate(text=FONT_TEXT)
    sub.subset(font)
    buf = io.BytesIO()
    font.flavor = "woff"
    font.save(buf)
    chapter = page("Fonts WOFF", f'<h1>Fonts WOFF</h1>\n<p class="woff">{FONT_TEXT}</p>')
    book(
        out,
        "fonts_woff",
        "Fonts WOFF",
        [("c1", "Fonts WOFF", chapter)],
        files=[("serif.woff", buf.getvalue())],
        css=CSS + '@font-face { font-family: "Fixture WOFF"; src: url(serif.woff) }\n'
        '.woff { font-family: "Fixture WOFF", serif }\n',
    )


def package_more(out):
    contents = page(
        "Contents",
        "<h1>Contents</h1>\n"
        '<div class="toc"><p><a href="c1.xhtml">Chapter One</a></p>'
        '<p><a href="c2.xhtml">Chapter Two</a></p></div>',
    )
    dedication = page(
        "Dedication",
        '<div class="dedication" epub:type="dedication"><p>For the fixtures.</p></div>',
    )
    c1 = page(
        "Chapter One",
        '<h1>Chapter One</h1>\n<p><span id="p1"/>The first page starts here.</p>\n'
        '<p><span id="p2"/>The second page starts here.</p>',
    )
    c2 = page("Chapter Two", '<h1>Chapter Two</h1>\n<p><span id="p3"/>The third page.</p>')
    page_list = (
        "<pageList><navLabel><text>Pages</text></navLabel>"
        + "".join(
            f'<pageTarget id="pg{n}" type="normal" value="{n}" playOrder="{n + 10}">'
            f'<navLabel><text>{n}</text></navLabel><content src="{src}"/></pageTarget>'
            for n, src in ((1, "c1.xhtml#p1"), (2, "c1.xhtml#p2"), (3, "c2.xhtml#p3"))
        )
        + "</pageList>"
    )
    guide = (
        "<guide>"
        '<reference type="toc" title="Contents" href="contents.xhtml"/>'
        '<reference type="dedication" title="Dedication" href="dedication.xhtml"/>'
        '<reference type="text" title="Start" href="c1.xhtml"/>'
        "</guide>"
    )
    book(
        out,
        "package_more",
        "Package More",
        [
            ("contents", "Contents", contents),
            ("dedication", None, dedication),
            ("c1", "Chapter One", c1),
            ("c2", "Chapter Two", c2),
        ],
        metadata="<dc:date>2020</dc:date><dc:publisher>Fixture Press</dc:publisher>",
        guide=guide,
        ncx_extra=page_list,
    )


def listing_rerun(out):
    opening = page(
        "Opening",
        "<h1>Listing Rerun</h1>\n<p>By Fixture Author</p>\n"
        '<div class="toc"><p><a href="c1.xhtml">Chapter One</a></p>'
        '<p><a href="c2.xhtml">Chapter Two</a></p></div>',
    )
    c1 = page("Chapter One", "<h1>Chapter One</h1>\n<p>The first chapter.</p>")
    c2 = page("Chapter Two", "<h1>Chapter Two</h1>\n<p>The second chapter.</p>")
    book(
        out,
        "listing_rerun",
        "Listing Rerun",
        [("opening", "Opening", opening), ("c1", "Chapter One", c1), ("c2", "Chapter Two", c2)],
    )


def marks(out):
    links = page(
        "リンク",
        "<h1>リンク</h1>\n"
        '<p><a href="#a">赤いリンク</a>、<span class="blue"><a href="#b">青いリンク</a></span>、'
        '<a href="#c" style="color: #008000">緑のリンク</a>、<a>リンクではない</a>。</p>\n'
        '<p id="a">一</p>\n<p id="b">二</p>\n<p id="c">三</p>',
        lang="ja",
    )
    vertical = page(
        "縦書き",
        "<h1>縦書き</h1>\n<p>縦書きの文章です。</p>\n"
        '<div class="h"><p>Horizontal text in a vertical book.</p>'
        '<p class="v">縦書きに戻る。</p></div>\n'
        "<p>また縦書き。</p>",
        lang="ja",
        body_attrs=' class="vrl"',
    )
    css = CSS + (
        "a:link { color: #a00000 }\na:visited { color: #00a0a0 }\n"
        ".blue a:visited { color: #0000a0 }\n"
        ".vrl { writing-mode: vertical-rl; -epub-writing-mode: vertical-rl }\n"
        ".h { writing-mode: horizontal-tb; -epub-writing-mode: horizontal-tb; margin: 1em }\n"
        ".v { writing-mode: vertical-rl; -epub-writing-mode: vertical-rl }\n"
    )
    book(
        out,
        "marks",
        "Marks",
        [("links", "リンク", links), ("vertical", "縦書き", vertical)],
        lang="ja",
        spine_attrs=' page-progression-direction="rtl"',
        css=css,
    )


def main(out):
    out = Path(out)
    for build in (
        text_edges,
        block_edges,
        tables_more,
        pictures_more,
        fonts_woff,
        package_more,
        listing_rerun,
        marks,
    ):
        build(out)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} OUT_DIR")
    main(sys.argv[1])
