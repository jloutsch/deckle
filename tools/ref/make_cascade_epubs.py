# SPDX-License-Identifier: GPL-3.0-only
"""Build the small cascade sources besides css_quirks: python3 make_cascade_epubs.py OUT_DIR

Writes, into OUT_DIR (fixtures/kfxgen/sources):

- package_quirks.epub: an EPUB 2 whose package Calibre changes before kfxgen sees it: a cover
  image named by <meta name="cover">, an HTML cover page first in the spine (and in the guide),
  a spine item linear="no", one listed twice, an HTML file a chapter links to that the spine
  leaves out, a year-only dc:date, a region language tag (en-US), and an NCX entry with an
  empty label beside a nested one.
- no_toc.epub: an EPUB 2 with neither NCX nor nav, so Calibre makes the contents itself
  (auto_generated_toc).
- fake_margins.epub: RemoveFakeMargins (page_margin.py), which counts margins per level (tag
  and depth below <body>) over the whole book: in all.xhtml 30 <p> straight under <body> share
  `margin-left: 3em` (removed), in half.xhtml 30 <p> one level down, half of them with it
  (kept, under the 95% Calibre needs). The two sets get different classes (see
  fake_margins()).
- malformed.epub: files Calibre does not read as plain XHTML (parse_utils.py parse_html):
  unclosed.xhtml has unclosed <p>, <br> and <font> tags and an unquoted attribute (read with
  the HTML 5 parser); entities.xhtml defines entities in its DOCTYPE, one of them holding a
  character reference, and uses an HTML entity XML does not know; html4.xhtml is HTML 4 with
  upper-case tags and attributes and an HTML 4.0 DOCTYPE (names lower-cased); cp1252.xhtml is
  windows-1252, declared in its XML declaration, with curly quotes, a euro sign and an e-acute.
- comments_in_tree.epub: comments where Calibre's steps read lxml's children, which count
  them: an empty <b> holding only a comment beside one holding nothing (parse_html's
  remove_elem), a <br> holding only a comment (made a <div> when it has children), a comment
  between the head's children (its whitespace reset), a rule inside a comment in a <style>
  (the Stylizer reads a child's text), a left-floated drop cap holding a comment beside one
  without (flatcss is_drop_cap counts children), and a comment before the first child of a
  <div> whose children :first-child and :nth-child(2) pick (css_selectors counts it), and the
  same before an only element child (:only-child) and before a :first-of-type; and comments
  whose tails hold text, before and after an element.

tools/ref/kfxgen_models.sh --cascade runs it. Fixed zip dates: the same script makes the same
bytes. The dumps are the reference; nothing here says what Calibre does with them.

A year-only date is filled by Calibre from the month of the run (calibre.utils.date
parse_only_date: day 15 of the current month), so package_quirks' dumped date changes with
the month it is dumped in; cascade/TOOLS records that month.
"""

import struct
import sys
import zipfile
import zlib
from pathlib import Path

XHTML_NS = "http://www.w3.org/1999/xhtml"
CONTAINER = (
    '<?xml version="1.0"?><container version="1.0" '
    'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
    '<rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>'
    "</rootfiles></container>"
)


def png(w, h, rgb):
    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def page(title, body, css="style.css"):
    link = f'<link rel="stylesheet" type="text/css" href="{css}"/>' if css else ""
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        f'<html xmlns="{XHTML_NS}"><head><title>{title}</title>{link}</head>\n'
        f"<body>\n{body}\n</body>\n</html>\n"
    )


def opf2(title, metadata, manifest, spine, guide="", toc=True):
    return f"""<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
<dc:identifier id="id">urn:uuid:00000000-0000-4000-8000-{sum(map(ord, title)):012d}</dc:identifier>
<dc:title>{title}</dc:title><dc:creator opf:role="aut">Fixture Author</dc:creator>
{metadata}
</metadata>
<manifest>
{manifest}
</manifest>
<spine{' toc="ncx"' if toc else ''}>{spine}</spine>
{guide}
</package>
"""


def write_epub(path, files):
    with zipfile.ZipFile(path, "w") as zf:

        def write(name, data, method=zipfile.ZIP_DEFLATED):
            info = zipfile.ZipInfo(name, date_time=(2026, 11, 13, 0, 0, 0))
            info.compress_type = method
            zf.writestr(info, data)

        write("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
        write("META-INF/container.xml", CONTAINER)
        for name, data in files:
            write(f"OEBPS/{name}", data)


CSS = "p { margin: 0; text-indent: 1em }\nh1 { text-align: center }\n"


def package_quirks(out):
    chapters = {
        "c1": page(
            "Chapter One",
            '<h1>Chapter One</h1>\n<p>The first chapter, with a link to '
            '<a href="extra.xhtml">a file the spine leaves out</a>.</p>\n'
            '<h2 id="sec">A section</h2>\n<p>More text.</p>',
        ),
        "c2": page("Chapter Two", "<h1>Chapter Two</h1>\n<p>The second chapter.</p>"),
        "notes": page("Notes", '<h1>Notes</h1>\n<p id="n1">A note, outside the linear order.</p>'),
        "extra": page("Extra", "<h1>Extra</h1>\n<p>Linked from chapter one only.</p>"),
    }
    cover = page(
        "Cover",
        '<div><img src="cover.png" alt="Cover"/></div>',
        css="",
    )
    manifest = "\n".join(
        [
            '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>',
            '<item id="cover-img" href="cover.png" media-type="image/png"/>',
            '<item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/>',
            '<item id="css" href="style.css" media-type="text/css"/>',
        ]
        + [
            f'<item id="{cid}" href="{cid}.xhtml" media-type="application/xhtml+xml"/>'
            for cid in chapters
        ]
    )
    spine = (
        '<itemref idref="cover"/><itemref idref="c1"/><itemref idref="c2"/>'
        '<itemref idref="c1"/><itemref idref="notes" linear="no"/>'
    )
    guide = (
        '<guide><reference type="cover" title="Cover" href="cover.xhtml"/>'
        '<reference type="text" title="Start" href="c1.xhtml"/></guide>'
    )
    metadata = (
        "<dc:language>en-US</dc:language><dc:date>2019</dc:date>"
        '<meta name="cover" content="cover-img"/>'
    )
    ncx = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/>'
        "<docTitle><text>Package Quirks</text></docTitle><navMap>"
        '<navPoint id="n1" playOrder="1"><navLabel><text>Chapter One</text></navLabel>'
        '<content src="c1.xhtml"/>'
        '<navPoint id="n2" playOrder="2"><navLabel><text>A section</text></navLabel>'
        '<content src="c1.xhtml#sec"/></navPoint></navPoint>'
        '<navPoint id="n3" playOrder="3"><navLabel><text></text></navLabel>'
        '<content src="c2.xhtml"/></navPoint>'
        '<navPoint id="n4" playOrder="4"><navLabel><text>Notes</text></navLabel>'
        '<content src="notes.xhtml"/></navPoint>'
        "</navMap></ncx>\n"
    )
    write_epub(
        out / "package_quirks.epub",
        [
            ("content.opf", opf2("Package Quirks", metadata, manifest, spine, guide)),
            ("toc.ncx", ncx),
            ("style.css", CSS),
            ("cover.png", png(6, 9, (40, 80, 160))),
            ("cover.xhtml", cover),
        ]
        + [(f"{cid}.xhtml", text) for cid, text in chapters.items()],
    )


def no_toc(out):
    chapters = {
        "one": page("One", "<h1>Chapter One</h1>\n<p>No contents file names this.</p>"),
        "two": page("Two", "<h2>Part Two</h2>\n<p>Nor this.</p>"),
    }
    manifest = "\n".join(
        ['<item id="css" href="style.css" media-type="text/css"/>']
        + [
            f'<item id="{cid}" href="{cid}.xhtml" media-type="application/xhtml+xml"/>'
            for cid in chapters
        ]
    )
    spine = "".join(f'<itemref idref="{cid}"/>' for cid in chapters)
    write_epub(
        out / "no_toc.epub",
        [
            (
                "content.opf",
                opf2("No Contents", "<dc:language>en</dc:language>", manifest, spine, toc=False),
            ),
            ("style.css", CSS),
        ]
        + [(f"{cid}.xhtml", text) for cid, text in chapters.items()],
    )


def fake_margins(out):
    # half.xhtml's indented paragraphs have a declaration of their own: Calibre would give
    # them all.xhtml's class otherwise, and RemoveFakeMargins removes a margin from the class
    # rule, so from every element of that class.
    css = (
        "p.fm { margin-left: 3em }\n"
        "p.fh { margin-left: 3em; text-indent: 2em }\n"
        "p.plain { margin-left: 0 }\n"
    )
    all_p = "\n".join(f'<p class="fm">Paragraph {i}, indented.</p>' for i in range(1, 31))
    half_p = "\n".join(
        f'<p class="{"fh" if i % 2 else "plain"}">Paragraph {i}.</p>' for i in range(1, 31)
    )
    files = {
        "all": page("All", all_p),
        "half": page("Half", f"<div>\n{half_p}\n</div>"),
    }
    manifest = "\n".join(
        [
            '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>',
            '<item id="css" href="style.css" media-type="text/css"/>',
        ]
        + [
            f'<item id="{fid}" href="{fid}.xhtml" media-type="application/xhtml+xml"/>'
            for fid in files
        ]
    )
    spine = "".join(f'<itemref idref="{fid}"/>' for fid in files)
    ncx = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/>'
        "<docTitle><text>Fake Margins</text></docTitle><navMap>"
        + "".join(
            f'<navPoint id="n{i}" playOrder="{i}"><navLabel><text>{fid}</text></navLabel>'
            f'<content src="{fid}.xhtml"/></navPoint>'
            for i, fid in enumerate(files, 1)
        )
        + "</navMap></ncx>\n"
    )
    write_epub(
        out / "fake_margins.epub",
        [
            (
                "content.opf",
                opf2("Fake Margins", "<dc:language>en</dc:language>", manifest, spine),
            ),
            ("toc.ncx", ncx),
            ("style.css", css),
        ]
        + [(f"{fid}.xhtml", text) for fid, text in files.items()],
    )


def malformed(out):
    head = '<?xml version="1.0" encoding="utf-8"?>\n'
    files = {
        "unclosed": head
        + f'<html xmlns="{XHTML_NS}"><head><title>Unclosed</title>'
        '<link rel="stylesheet" type="text/css" href="style.css"/></head>\n<body>\n'
        "<h1>Unclosed tags</h1>\n"
        "<p class=first>An unquoted class, and a paragraph left open\n"
        "<p>A break<br>then <font size=4>a font left open\n"
        "<p>A last paragraph, closed.</p>\n"
        "</body>\n</html>\n",
        "entities": head
        + '<!DOCTYPE html [<!ENTITY author "Fixture Author"><!ENTITY sep " &#8212; ">]>\n'
        f'<html xmlns="{XHTML_NS}"><head><title>Entities</title></head>\n<body>\n'
        "<p>By &author;&sep;with&nbsp;an HTML entity and &amp; an XML one.</p>\n"
        "</body>\n</html>\n",
        "html4": '<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.0 Transitional//EN">\n'
        "<HTML><HEAD><TITLE>Upper</TITLE></HEAD>\n<BODY>\n"
        '<H2 ALIGN="center">HTML 4</H2>\n'
        '<P CLASS="Upper">Upper-case tags<BR/>and attributes.</P>\n'
        "</BODY></HTML>\n",
        "cp1252": (
            '<?xml version="1.0" encoding="windows-1252"?>\n'
            f'<html xmlns="{XHTML_NS}"><head><title>Windows</title></head>\n<body>\n'
            "<p>\u201cCurly quotes\u201d, \u20ac5 and caf\u00e9.</p>\n"
            "</body>\n</html>\n"
        ).encode("cp1252"),
    }
    manifest = "\n".join(
        [
            '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>',
            '<item id="css" href="style.css" media-type="text/css"/>',
        ]
        + [
            f'<item id="{fid}" href="{fid}.xhtml" media-type="application/xhtml+xml"/>'
            for fid in files
        ]
    )
    spine = "".join(f'<itemref idref="{fid}"/>' for fid in files)
    ncx = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/>'
        "<docTitle><text>Malformed</text></docTitle><navMap>"
        + "".join(
            f'<navPoint id="n{i}" playOrder="{i}"><navLabel><text>{fid}</text></navLabel>'
            f'<content src="{fid}.xhtml"/></navPoint>'
            for i, fid in enumerate(files, 1)
        )
        + "</navMap></ncx>\n"
    )
    write_epub(
        out / "malformed.epub",
        [
            ("content.opf", opf2("Malformed", "<dc:language>en</dc:language>", manifest, spine)),
            ("toc.ncx", ncx),
            ("style.css", CSS),
        ]
        + [(f"{fid}.xhtml", text) for fid, text in files.items()],
    )


def comments_in_tree(out):
    css = (
        "p { margin: 0 }\n"
        "span.cap { float: left; font-size: 3em; line-height: 0.5 }\n"
        "div.picks p:first-child { color: green }\n"
        "div.picks p:nth-child(2) { color: blue }\n"
        "div.alone p:only-child { color: purple }\n"
        "div.types p:first-of-type { color: olive }\n"
    )
    body = (
        '<p id="b1">Before <b><!--c--></b>after a bold holding only a comment.</p>\n'
        '<p id="b2">Before <b></b>after a bold holding nothing.</p>\n'
        '<p id="tails">a<!--c-->b<i>d</i><!--e-->f</p>\n'
        '<p id="br1">A break holding a comment<br><!--c--></br>after it.</p>\n'
        '<p class="ruled">Coloured by the rule inside the comment in the style element.</p>\n'
        '<p id="cap1"><span class="cap">T<!--c--></span>he drop cap holds a comment.</p>\n'
        '<p id="cap2"><span class="cap">T</span>he drop cap holds none.</p>\n'
        '<div class="picks"><!-- c --><p>First paragraph.</p><p>Second paragraph.</p></div>\n'
        '<div class="alone"><!-- c --><p>The only element child.</p></div>\n'
        '<div class="types"><!-- c --><p>The first paragraph of its type.</p><p>The second.</p></div>'
    )
    text = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        f'<html xmlns="{XHTML_NS}"><head><title>Comments</title>\n'
        "<!-- a comment between the head's children -->\n"
        '<link rel="stylesheet" type="text/css" href="style.css"/>\n'
        '<style type="text/css"><!-- p.ruled { color: red } --></style>\n'
        f"</head>\n<body>\n{body}\n</body>\n</html>\n"
    )
    manifest = "\n".join(
        [
            '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>',
            '<item id="css" href="style.css" media-type="text/css"/>',
            '<item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>',
        ]
    )
    ncx = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/>'
        "<docTitle><text>Comments</text></docTitle><navMap>"
        '<navPoint id="n1" playOrder="1"><navLabel><text>Comments</text></navLabel>'
        '<content src="c1.xhtml"/></navPoint></navMap></ncx>\n'
    )
    write_epub(
        out / "comments_in_tree.epub",
        [
            (
                "content.opf",
                opf2(
                    "Comments In Tree",
                    "<dc:language>en</dc:language>",
                    manifest,
                    '<itemref idref="c1"/>',
                ),
            ),
            ("toc.ncx", ncx),
            ("style.css", css),
            ("c1.xhtml", text),
        ],
    )


def main(out):
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    package_quirks(out)
    no_toc(out)
    fake_margins(out)
    malformed(out)
    comments_in_tree(out)


if __name__ == "__main__":
    main(sys.argv[1])
