# SPDX-License-Identifier: GPL-3.0-only
"""Build fixtures/kfxgen/sources/css_quirks.epub: python3 make_css_quirks_epub.py OUT.epub

One small file per group of Calibre CSS quirks that the native KFX port's tests name (plan
2026-11-13, tasks 4 to 10). tools/ref/kfxgen_models.sh --cascade builds it and dumps it in a
kfxgen conversion; the tests read their expected values from that dump, so nothing here says
what Calibre does, only what to ask it.

- cleanups.xhtml (task 4): parse_html's clean-ups: no head or title, upper-case and Office
  tags, empty links and inline tags, a <br> with content, an element in no namespace.
- sheet.xhtml (task 5): @import (a chain, and a cycle), @page, @media, mso- and panose-
  declarations, value spellings, a selector list, @font-face of one and of two declarations.
- shorthand.xhtml (task 6): margin, padding, border, list-style and font shorthands,
  font-size keywords, shorthands Calibre leaves alone, -epub- properties.
- selectors.xhtml (task 7): one selector per region; selector n sets `orphans: n` and, but
  for :root, starts with `#sN`, so each element is matched by one selector at most and the
  dump's `specified` shows which.
- cascade.xhtml (task 8): !important against later rules and the style attribute, equal
  specificity, sheet order across <link> and <style>, img width and height attributes,
  @font-face, a chapter heading DetectStructure marks.
- flatten.xhtml (task 9): what CSSFlattener changes: html@style, align, <font>, color and
  bgcolor, ol@type, keyword fixes, class names, invalid values, an element outside XHTML.
- values.xhtml (task 10): what Style computes: units, font-size keywords and relative sizes,
  line-height, widths, percentages, colours, initial and unset. It opens with a heading
  DetectStructure leaves alone (the first element of its file).

The zip is written with fixed dates, so the same script makes the same bytes.
"""

import struct
import sys
import zipfile
import zlib

XHTML_NS = "http://www.w3.org/1999/xhtml"


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


def page(title, body, head="", html_attrs="", css=True, name=""):
    link = f'<link rel="stylesheet" type="text/css" href="{name}.css"/>' if css else ""
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        f'<html xmlns="{XHTML_NS}" xml:lang="en" lang="en"{html_attrs}>\n'
        f"<head><title>{title}</title>{link}{head}</head>\n"
        f"<body>\n{body}\n</body>\n</html>\n"
    )


# --- cleanups (task 4) -----------------------------------------------------------------

CLEANUPS = f"""<?xml version="1.0" encoding="utf-8"?>
<html xmlns="{XHTML_NS}" xmlns:o="urn:schemas-microsoft-com:office:office">
<body>
<P>An upper-case paragraph.</P>
<p>An Office tag: <o:p>inside o:p</o:p>.</p>
<p>Empty, removed: <a href="#nowhere"></a><i></i><b/><u></u>.</p>
<p>Empty, kept: <a href="#k1" id="k1"></a><b id="k2"></b><i name="k3"></i><a name="k4"></a>.</p>
<p>A break with content: <br>inside the br</br> after it.</p>
<p>A plain break:<br/>after it.</p>
<p>An element in no namespace: <span xmlns="">no namespace</span>.</p>
<DIV CLASS="Upper">An upper-case div and attribute.</DIV>
</body>
</html>
"""

# --- sheet (task 5) --------------------------------------------------------------------

SHEET_CSS = """@import url("imported.css");
@page { margin: 1em }
@media print { p.pr { orphans: 2 } }
@media screen { p.sc { orphans: 3 } }
@media amzn-kf8 { p.kf { orphans: 4 } }
@media screen and (min-width: 600px) { p.mw { orphans: 5 } }
@media all and (orientation: portrait) { p.or { orphans: 6 } }
@media aural { p.au { orphans: 7 } }
p.mso { mso-margin-top-alt: 1em; panose-1: 2 2 6 3; color: red }
p.v1 { margin-left: .5em; margin-right: 0pt; color: RED; text-align: Center }
p.v2 { font-family: 'Times New Roman', serif; width: 10PX; height: 1EM; widows: 2 }
p.v3 { color: ; orphans: 8 }
p.v4 { margin-left: foo; height: 10 px }
p.v5 { margin-top: -.25EM; text-indent: +1.50em; line-height: 1.20 }
p.v6 { color: #ABC; background-color: rgb(10, 20, 30); quotes: "\\201C" '\\201D' }
p.v7 { margin-left: 1em; margin-left: 2em }
h4, h5.x { orphans: 9 }
@font-face { font-family: "One" }
@font-face { font-family: "Two"; font-weight: bold }
"""

IMPORTED_CSS = """@import "deep.css";
p.imp { orphans: 10 }
"""

DEEP_CSS = """p.deep { orphans: 11 }
"""

# A cycle, linked on its own: in calibre 9.18 a cycle loses the whole chain, so it would
# hide the imports above.
CYCLE_A_CSS = """@import "cycle_b.css";
p.ca { orphans: 13 }
"""

CYCLE_B_CSS = """@import "cycle_a.css";
p.cb { orphans: 14 }
"""

SHEET = page(
    "Sheet",
    """<p class="pr">print</p>
<p class="sc">screen</p>
<p class="kf">amzn-kf8</p>
<p class="mw">screen, min-width</p>
<p class="or">orientation</p>
<p class="au">aural</p>
<p class="mso">mso and panose</p>
<p class="v1">values 1</p>
<p class="v2">values 2</p>
<p class="v3">values 3</p>
<p class="v4">values 4</p>
<p class="v5">values 5</p>
<p class="v6">values 6</p>
<p class="v7">values 7</p>
<h4>h4</h4>
<h5 class="x">h5.x</h5>
<p class="imp">imported</p>
<p class="deep">imported twice over</p>
<p class="st">from the style element</p>
<p class="ca">import cycle, first sheet</p>
<p class="cb">import cycle, second sheet</p>""",
    head="<style>@page { margin-top: 2em } p.st { orphans: 12 }</style>"
    '<link rel="stylesheet" type="text/css" href="cycle_a.css"/>',
    name="sheet",
)

# --- shorthand (task 6) ----------------------------------------------------------------

SHORTHANDS = [
    ("m1", "margin: 1em"),
    ("m2", "margin: 1em 2em"),
    ("m3", "margin: 1em 2em 3em"),
    ("m4", "margin: 1em 2em 3em 4em"),
    ("m5", "margin: auto"),
    ("p4", "padding: 1px 2px 3px 4px"),
    ("p2", "padding: 0 5%"),
    ("b1", "border: 2px solid red"),
    ("b2", "border-top: thin dashed"),
    ("b3", "border-style: solid dotted"),
    ("b4", "border-width: 1px 2px 3px"),
    ("b5", "border-color: red green blue yellow"),
    ("b6", "border: none"),
    ("b7", "border-left: 3px double"),
    ("ls1", "list-style: square inside"),
    ("ls2", "list-style: none"),
    ("ls3", "list-style: upper-roman outside url(pic.png)"),
    ("f1", "font: italic bold 12px/30px Georgia, serif"),
    ("f2", 'font: 1.2em "Times New Roman"'),
    ("f3", "font: small-caps 80% sans-serif"),
    ("f4", "font: bold large/1.5 serif"),
    ("f5", "font: 12pt"),
    ("fs1", "font-size: large"),
    ("fs2", "font-size: LARGE"),
    ("fs3", "font-size: smaller"),
    ("fs4", "font-size: xx-small"),
    ("fs5", "font-size: normal"),
    ("fs6", "font-size: smallest"),
    ("fs7", "font-size: x-large"),
    ("bg", "background: #eee url(pic.png) no-repeat"),
    ("td", "text-decoration: underline overline"),
    ("ol", "outline: 1px solid red"),
    ("ep", "-epub-hyphens: auto; -epub-writing-mode: horizontal-tb"),
    ("wk", "-webkit-hyphens: manual"),
    ("imp", "margin: 1em !important"),
    ("mix", "margin: 1em; margin-left: 2em"),
]

SHORTHAND_CSS = "".join(f"p.{cls} {{ {decl} }}\n" for cls, decl in SHORTHANDS)
SHORTHAND = page(
    "Shorthands",
    "\n".join(f'<p class="{cls}">{decl}</p>' for cls, decl in SHORTHANDS),
    name="shorthand",
)

# --- selectors (task 7) ----------------------------------------------------------------

# (selector after "#sN ", the region's inner XHTML). Region 1 is :root, unscoped.
SELECTORS = [
    (None, "<p>:root matches html alone</p>"),
    ("> *", "<p>child</p><div><p>grandchild</p></div>"),
    ("div p", "<p>not in a div</p><div><section><p>deep in a div</p></section></div>"),
    ("div > p", "<div><p>child of div</p><section><p>grandchild</p></section></div>"),
    ("h3 + p", "<h3>h</h3><p>next</p><p>after next</p>"),
    ("h3 ~ p", "<p>before</p><h3>h</h3><div>d</div><p>later</p><p>later still</p>"),
    ("[title]", '<p title="">empty title</p><p>no title</p>'),
    ("[title=x]", '<p title="x">x</p><p title="X">X</p><p title="x y">x y</p>'),
    ("[title~=b]", '<p title="a b c">a b c</p><p title="ab">ab</p>'),
    ("[lang|=en]", '<p lang="en">en</p><p lang="en-GB">en-GB</p><p lang="eng">eng</p>'),
    ("[title^=ab]", '<p title="abc">abc</p><p title="cab">cab</p>'),
    ("[title$=yz]", '<p title="xyz">xyz</p><p title="yzx">yzx</p>'),
    ("[title*=mid]", '<p title="amidst">amidst</p><p title="mi d">mi d</p>'),
    ('[title="X" i]', '<p title="x">x</p><p title="X">X</p>'),
    ('[data-x="Value"]', '<p data-x="Value">Value</p><p data-x="value">value</p>'),
    (":is(p, li)", "<p>p</p><ul><li>li</li></ul><div>div</div>"),
    (":where(.w)", '<p class="w">w</p><p>none</p>'),
    ("p:not(.n)", '<p class="n">n</p><p>not n</p>'),
    ("p:not(#nid)", '<p id="nid">nid</p><p>other</p>'),
    ("div:has(> em)", "<div><em>em child</em></div><div><p><em>em grandchild</em></p></div>"),
    (
        "li:nth-child(2 of .k)",
        '<ul><li class="k">k1</li><li>plain</li><li class="k">k2</li><li class="k">k3</li></ul>',
    ),
    ("p:lang(fr)", '<div lang="fr"><p>fr by parent</p></div><p lang="fr-CA">fr-CA</p><p>en</p>'),
    (":empty", "<p></p><p> </p><p><!-- c --></p><p>text</p>"),
    ("p:first-of-type", "<h3>h</h3><p>first p</p><p>second p</p>"),
    ("p:last-of-type", "<p>first p</p><p>last p</p><h3>h</h3>"),
    ("p:nth-of-type(2)", "<p>1</p><h3>h</h3><p>2</p><p>3</p>"),
    ("span:only-of-type", "<p><span>only</span><em>e</em></p><p><span>a</span><span>b</span></p>"),
    ("li:nth-child(odd)", "<ul><li>1</li><li>2</li><li>3</li><li>4</li></ul>"),
    ("li:nth-last-child(2)", "<ul><li>1</li><li>2</li><li>3</li></ul>"),
    ("li:first-child", "<ul><li>1</li><li>2</li></ul>"),
    ("li:last-child", "<ul><li>1</li><li>2</li></ul>"),
    ("em:only-child", "<p><em>only</em></p><p><em>a</em><em>b</em></p>"),
    (".BigText", '<p class="bigtext">bigtext</p><p class="BigText">BigText</p><p class="BIGTEXT">BIGTEXT</p>'),
    # Its own id in other capitals: the selector says #S34.
    ("P", "<p>upper-case type selector</p>"),
    ("p:hover", "<p>hover</p>"),
    ("p::first-letter", "<p>first-letter</p>"),
    ("li::marker", "<ul><li>marker</li></ul>"),
    ("p:nth-child(3n+1)", "<p>1</p><p>2</p><p>3</p><p>4</p>"),
    ("p.a.b", '<p class="a b">a b</p><p class="a">a</p><p class="b a c">b a c</p>'),
    ("a:link", '<p><a href="#s40">link</a><a name="n40">anchor</a></p>'),
    ("p:first-line", "<p>first-line</p>"),
]


def selector_text(n, rest):
    if rest is None:
        return ":root"
    region = "#S34" if n == 34 else f"#s{n}"
    return f"{region} {rest}"


SELECTORS_CSS = "".join(
    f"{selector_text(n, rest)} {{ orphans: {n} }}\n" for n, (rest, _) in enumerate(SELECTORS, 1)
)
SELECTORS_PAGE = page(
    "Selectors",
    "\n".join(f'<div id="s{n}">{body}</div>' for n, (_, body) in enumerate(SELECTORS, 1)),
    name="selectors",
)

# --- cascade (task 8) ------------------------------------------------------------------

CASCADE_CSS = """p.i1 { color: red !important }
p.i1 { color: blue }
p.i2 { color: green !important }
p.e1 { color: olive }
p.e1 { color: navy }
#id1 { color: teal }
p.cls { color: maroon }
p.order { color: red }
p.uai { float: left }
img.styled { width: 50% }
@font-face { font-family: "Quirk"; font-weight: bold; font-style: italic }
@font-face { font-family: "Solo" }
"""

CASCADE2_CSS = "p.order { color: blue }\n"

CASCADE = page(
    "Cascade",
    """<p>The opening paragraph.</p>
<p>Another paragraph.</p>
<p class="i1">important, then a later rule</p>
<p class="i2" style="color: purple">important, then the style attribute</p>
<p class="e1">equal specificity, later wins</p>
<p class="e1" style="color: purple">the style attribute last</p>
<p class="cls" id="id1">an id beats a class</p>
<p class="order">a link, a style element, a second link</p>
<hr/>
<hr size="1"/>
<img src="pic.png" alt="" width="30" height="40"/>
<img src="pic.png" alt="" width="50%"/>
<img class="styled" src="pic.png" alt="" width="30" height="40"/>
<img src="pic.png" alt="" style="height: 2em" width="30" height="40"/>
<h2>Chapter Two</h2>
<p>After the chapter heading.</p>
<p class="chapter">A paragraph classed chapter.</p>
<h2>Interlude</h2>
<h1>Part  Three</h1>
<frameset/>""",
    head='<style>p.order { color: green } p.uai { float: right !important }</style>'
    '<link rel="stylesheet" type="text/css" href="cascade2.css"/>',
    name="cascade",
)

# --- flatten (task 9) ------------------------------------------------------------------

FLATTEN_CSS = """p.fw { font-weight: medium }
span.il { display: in-line }
span.sup { vertical-align: sup }
p.bad { margin-left: foo; height: 10 px }
p.ps:hover { color: red }
"""

FLATTEN = page(
    "Flatten",
    """<p align="center">align centre</p>
<p align="justify">align justify</p>
<p><img src="pic.png" alt="" align="left"/><img src="pic.png" alt="" align="middle"/></p>
<p><font size="+1">font +1</font> <font size="2" color="red" face="serif">font 2</font>
<font size="7">font 7</font> <font size="-2">font -2</font></p>
<font size="5"><p>a font around a block</p></font>
<table border="1" bgcolor="#eeeeee"><tr><td bgcolor="yellow" align="right">cell</td></tr></table>
<p color="blue">a color attribute on p</p>
<ol type="a"><li>a</li><li>b</li></ol>
<p class="fw">font-weight: medium</p>
<p><span class="il">in-line</span> <span class="sup">sup</span></p>
<p class="Note2 big">Note2 big</p>
<p class="note3">note3</p>
<p class="NOTE">NOTE</p>
<p class="note3">note3 again</p>
<p class="Note2 big" style="color: red">Note2 big, styled</p>
<p>no class</p>
<p class="bad">invalid values</p>
<p class="ps">a pseudo class</p>
<x:thing xmlns:x="urn:example:other" class="kept" style="color: red">outside XHTML</x:thing>""",
    html_attrs=' style="color: #333"',
    name="flatten",
)

# --- values (task 10) ------------------------------------------------------------------

VALUES = [
    ("u-px", "font-size: 12px"),
    ("u-in", "margin-left: 1in"),
    ("u-pt", "margin-left: 7pt"),
    ("u-em", "margin-left: 2em; font-size: 20px"),
    ("u-ex", "margin-left: 2ex"),
    ("u-en", "margin-left: 2en"),
    ("u-pc", "margin-left: 1pc"),
    ("u-mm", "margin-left: 10mm"),
    ("u-cm", "margin-left: 1cm"),
    ("u-rem", "margin-left: 1.5rem"),
    ("u-pct", "margin-left: 10%"),
    ("u-none", "margin-left: 12"),
    ("u-text", "margin-left: foo"),
    ("fs-large", "font-size: large"),
    ("fs-smaller", "font-size: smaller"),
    ("fs-larger", "font-size: larger"),
    ("fs-neg", "font-size: -1em"),
    ("fs-pct", "font-size: 150%"),
    ("fs-bad", "font-size: bogus"),
    ("fs-xs", "font-size: x-small"),
    ("lh-normal", "line-height: normal"),
    ("lh-num", "line-height: 2"),
    ("lh-px", "line-height: 24px"),
    ("lh-pct", "line-height: 150%"),
    ("ti-pct", "text-indent: 10%; width: 200px"),
    ("fw-700", "font-weight: 700"),
    ("fw-bold", "font-weight: bold"),
    ("w-pct", "width: 50%"),
    ("h-pct", "height: 50%"),
    ("mp-pct", "margin-top: 10%; padding-left: 5%"),
    ("col-initial", "color: initial"),
    ("col-unset", "color: unset"),
    ("col-inherit", "color: inherit"),
    ("bg", "background-color: #ffeedd"),
    ("ws", "white-space: pre"),
    ("ta", "text-align: justify"),
    ("bd", "border-top: 2px solid #123456; border-left-width: thick"),
    ("ff", "font-family: 'Liberation Serif', serif; font-style: oblique"),
]

VALUES_CSS = (
    "".join(f"p.{cls} {{ {decl} }}\n" for cls, decl in VALUES)
    + """div.lh-parent { font-size: 20px; line-height: 1.5 }
div.lh-parent p { font-size: 10px }
div.w-parent { width: 300px }
div.w-parent p { width: 50%; margin-left: 10% }
table.w-attr { width: 100px; border-collapse: collapse }
li.lst { list-style-type: lower-greek }
"""
)

VALUES_PAGE = page(
    "Values",
    "<h1>Part One</h1>\n"
    + "\n".join(f'<p class="{cls}">{decl}</p>' for cls, decl in VALUES)
    + """
<div class="lh-parent"><p>line-height inherited as points</p></div>
<div class="w-parent"><p>half of 300px, margin 10% of it</p></div>
<table class="w-attr" width="300"><tr><td>width attribute and CSS</td></tr></table>
<img src="pic.png" alt="" width="64" style="width: 32px"/>
<ul><li class="lst">greek</li><li>default</li></ul>
<p>no style at all</p>""",
    name="values",
)

# --- the package -----------------------------------------------------------------------

FILES = [
    ("cleanups", "Clean-ups", CLEANUPS),
    ("sheet", "Sheet", SHEET),
    ("shorthand", "Shorthands", SHORTHAND),
    ("selectors", "Selectors", SELECTORS_PAGE),
    ("cascade", "Cascade", CASCADE),
    ("flatten", "Flatten", FLATTEN),
    ("values", "Values", VALUES_PAGE),
]

STYLES = {
    "sheet.css": SHEET_CSS,
    "imported.css": IMPORTED_CSS,
    "deep.css": DEEP_CSS,
    "cycle_a.css": CYCLE_A_CSS,
    "cycle_b.css": CYCLE_B_CSS,
    "shorthand.css": SHORTHAND_CSS,
    "selectors.css": SELECTORS_CSS,
    "cascade.css": CASCADE_CSS,
    "cascade2.css": CASCADE2_CSS,
    "flatten.css": FLATTEN_CSS,
    "values.css": VALUES_CSS,
}


def opf():
    items = [
        '<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>',
        '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>',
        '<item id="pic" href="pic.png" media-type="image/png"/>',
    ]
    items += [
        f'<item id="css-{name[:-4]}" href="{name}" media-type="text/css"/>' for name in STYLES
    ]
    items += [
        f'<item id="{fid}" href="{fid}.xhtml" media-type="application/xhtml+xml"/>'
        for fid, _, _ in FILES
    ]
    spine = "".join(f'<itemref idref="{fid}"/>' for fid, _, _ in FILES)
    return f"""<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="id">urn:uuid:00000000-0000-4000-8000-000000000c55</dc:identifier>
<dc:title>CSS Quirks</dc:title><dc:creator>Fixture Author</dc:creator>
<dc:language>en</dc:language><dc:publisher>Deckle fixtures</dc:publisher>
<dc:date>2026-03</dc:date><dc:description>Calibre CSS quirks, one file each.</dc:description>
<meta property="dcterms:modified">2026-11-13T00:00:00Z</meta>
</metadata>
<manifest>
{chr(10).join(items)}
</manifest>
<spine toc="ncx">{spine}</spine>
<guide><reference type="text" title="Start" href="cleanups.xhtml"/></guide>
</package>
"""


def nav():
    lis = "".join(f'<li><a href="{fid}.xhtml">{title}</a></li>' for fid, title, _ in FILES)
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        f'<html xmlns="{XHTML_NS}" xmlns:epub="http://www.idpf.org/2007/ops">'
        "<head><title>Contents</title></head>"
        f'<body><nav epub:type="toc"><ol>{lis}</ol></nav></body></html>\n'
    )


def ncx():
    points = "".join(
        f'<navPoint id="n{i}" playOrder="{i}"><navLabel><text>{title}</text></navLabel>'
        f'<content src="{fid}.xhtml"/></navPoint>'
        for i, (fid, title, _) in enumerate(FILES, 1)
    )
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/>'
        f"<docTitle><text>CSS Quirks</text></docTitle><navMap>{points}</navMap></ncx>\n"
    )


def main(out):
    with zipfile.ZipFile(out, "w") as zf:

        def write(name, data, method=zipfile.ZIP_DEFLATED):
            info = zipfile.ZipInfo(name, date_time=(2026, 11, 13, 0, 0, 0))
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
        write("OEBPS/content.opf", opf())
        write("OEBPS/nav.xhtml", nav())
        write("OEBPS/toc.ncx", ncx())
        for name, css in STYLES.items():
            write(f"OEBPS/{name}", css)
        for fid, _, text in FILES:
            write(f"OEBPS/{fid}.xhtml", text)
        write("OEBPS/pic.png", png(4, 4, (200, 60, 60)))


if __name__ == "__main__":
    main(sys.argv[1])
