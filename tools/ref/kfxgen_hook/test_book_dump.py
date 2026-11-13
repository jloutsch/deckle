# SPDX-License-Identifier: GPL-3.0-only
"""book_dump writes Calibre's book view and kfxgen's styles as the Rust tests read them.

Run: python3 tools/ref/kfxgen_hook/test_book_dump.py

The OEB, the Stylizer and its Style objects are stubs with the attributes book_dump reads, so
the test needs neither calibre nor kfxgen. The tree is lxml's when lxml is installed (the kfxgen
venv), else ElementTree's.
"""

import hashlib
import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

try:
    from lxml import etree

    def parse(text):
        return etree.fromstring(text.encode("utf-8"))

except ImportError:
    import xml.etree.ElementTree as etree

    def parse(text):
        builder = etree.TreeBuilder(insert_comments=True, insert_pis=True)
        parser = etree.XMLParser(target=builder)
        parser.feed(text)
        return parser.close()


def load_book_dump():
    path = Path(__file__).with_name("book_dump.py")
    spec = importlib.util.spec_from_file_location("book_dump", path)
    if spec is None or not path.exists():
        raise ImportError("book_dump not found")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


XHTML = "http://www.w3.org/1999/xhtml"
PAGE = f"""<html xmlns="{XHTML}"><head><title>One</title></head><body>
<!-- a comment is not counted in a path -->
<p class="a">first</p>
<?pi not counted either?>
<p class="a">second, styled as the first</p>
<div><img src="pic.png" width="10" height="20"/><span class="pcalibre calibre1">run</span><x:t xmlns:x="urn:x" class="b a"/></div>
</body></html>"""


class Prop:
    def __init__(self, name, value):
        self.name, self.value = name, value


class FontFaceRule:
    def __init__(self, decls):
        self.style = [Prop(k, v) for k, v in decls]


class Style:
    """Calibre's Style as book_dump reads it: cssdict(), _pseudo_classes and s[prop]."""

    def __init__(self, specified, pseudo=None):
        self._specified = specified
        self._pseudo_classes = pseudo or {}

    def cssdict(self):
        return dict(self._specified)

    def __getitem__(self, name):
        if name == "list-style-type":
            raise KeyError(name)
        if name == "font-size":
            return 12.0
        if name == "width":
            return 0.1 + 0.2
        return self._specified.get(name, "auto")


class Stylizer:
    """Building it drops img width/height, as Calibre's does."""

    def __init__(self, tree):
        self.tree = tree
        for el in tree.iter():
            if isinstance(el.tag, str) and el.tag == f"{{{XHTML}}}img":
                el.attrib.pop("width", None)
                el.attrib.pop("height", None)
        self.font_face_rules = [
            FontFaceRule([("font-family", "Q"), ("src", 'url("q.ttf")')])
        ]

    def style(self, el):
        local = el.tag.rpartition("}")[2]
        if local == "p":
            # Inserted in different orders: the same style all the same.
            if el.text == "first":
                return Style({"text-indent": "1em", "margin-top": "0"})
            return Style({"margin-top": "0", "text-indent": "1em"})
        if local == "span":
            return Style({"color": "red"}, {"hover": {"color": "blue"}})
        return Style({})


class FirstPassStylizer:
    rules = [
        ((0, 0, 0, 1, 0), [], {"display": "block"}, "p", None),
        ((1, 0, 1, 0, 3), [], {"text-indent": "1em", "margin-top": "0"}, ".a", "x.css"),
    ]
    font_face_rules = [FontFaceRule([("font-family", "Q"), ("font-weight", "bold")])]


FIRST = {"rules": [], "font_face": []}


class Obj:
    def __init__(self, **kw):
        vars(self).update(kw)


class Meta:
    def __init__(self, value):
        self.value = value

    def __str__(self):
        return self.value


class Spine(list):
    page_progression_direction = None


class Guide(dict):
    pass


CONTAINER = (
    '<?xml version="1.0"?><container version="1.0" '
    'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
    '<rootfile full-path="{}" media-type="application/oebps-package+xml"/>'
    "</rootfiles></container>"
)


def make_oeb(tmp, own_opf="OEBPS/book.opf"):
    """An OEB unpacked as calibre leaves it: the book's own OPF at ``own_opf`` and the one
    calibre writes, ``content.opf``, at the root."""
    root = Path(tmp) / "unpacked"
    (root / "META-INF").mkdir(parents=True)
    (root / "META-INF" / "container.xml").write_text(CONTAINER.format(own_opf))
    (root / own_opf).parent.mkdir(parents=True, exist_ok=True)
    (root / own_opf).write_bytes(b"<package>own</package>")
    (root / "content.opf").write_bytes(b"<package>calibre's, with a random NCX name</package>")
    item = Obj(id="c1", href="text/one.xhtml", media_type="application/xhtml+xml",
               linear=True, data=parse(PAGE))
    css = Obj(id="css", href="x.css", media_type="text/css", linear=True, data=None)
    spine = Spine([item])
    toc_child = Obj(title="One", href="text/one.xhtml", nodes=[])
    toc = Obj(title=None, href=None, nodes=[toc_child])
    guide = Guide(toc=Obj(type="toc", href="text/one.xhtml"),
                  cover=Obj(type="cover", href="text/one.xhtml"))
    metadata = Obj(title=[Meta("Book")], creator=[Meta("A"), Meta("B")], description=[],
                   language=[Meta("en")], publisher=[], date=[Meta("2026-01-01")],
                   cover=[Meta("css")])
    oeb = Obj(spine=spine, manifest=[item, css], toc=toc, auto_generated_toc=False,
              guide=guide, metadata=metadata,
              container=Obj(rootdir=str(root), opfname="content.opf"))
    return oeb, item


def run_dump(book_dump, tmp, name):
    out = Path(tmp) / name
    oeb, item = make_oeb(Path(tmp) / f"{name}-book")
    first_pass = {}
    with mock.patch.dict(os.environ, {"KFXGEN_DUMP_BOOK": str(out)}):
        book_dump.note_first_pass(item, FirstPassStylizer(), first_pass)
    book_dump.dump(out, oeb, lambda it: Stylizer(it.data), first_pass)
    return out


def tree_bytes(directory):
    return {
        p.relative_to(directory).as_posix(): p.read_bytes()
        for p in sorted(directory.rglob("*")) if p.is_file()
    }


class BookDumpTest(unittest.TestCase):
    def setUp(self):
        self.book_dump = load_book_dump()
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def test_writes_the_four_outputs(self):
        out = run_dump(self.book_dump, self.tmp.name, "a")
        self.assertEqual(
            sorted(tree_bytes(out)),
            ["dom/000.xhtml", "files.json", "oeb.json", "rules/000.json", "styles/000.json"],
        )
        self.assertEqual(json.loads((out / "files.json").read_text()), {"text/one.xhtml": "000"})

        oeb = json.loads((out / "oeb.json").read_text())
        self.assertEqual(oeb["spine"], [{"href": "text/one.xhtml", "linear": True}])
        self.assertEqual([m["href"] for m in oeb["manifest"]], ["text/one.xhtml", "x.css"])
        self.assertEqual(oeb["toc"], [{"title": "One", "href": "text/one.xhtml", "children": []}])
        self.assertEqual([g["type"] for g in oeb["guide"]], ["cover", "toc"])
        self.assertEqual(oeb["metadata"]["creator"], ["A", "B"])
        self.assertEqual(oeb["metadata"]["description"], [])
        self.assertIsNone(oeb["page_progression_direction"])
        self.assertIs(oeb["auto_generated_toc"], False)
        self.assertEqual(
            oeb["raw"],
            {"opfname": "OEBPS/book.opf",
             "sha256": hashlib.sha256(b"<package>own</package>").hexdigest()},
        )

        rules = json.loads((out / "rules/000.json").read_text())
        self.assertEqual(
            rules["rules"],
            [[0, "p", {"display": "block"}], [1, ".a", {"text-indent": "1em", "margin-top": "0"}]],
        )
        self.assertEqual(rules["font_face"], [{"font-family": "Q", "font-weight": "bold"}])

    def test_an_own_opf_calibre_wrote_over_has_no_hash(self):
        out = Path(self.tmp.name) / "d"
        oeb, item = make_oeb(Path(self.tmp.name) / "d-book", own_opf="content.opf")
        self.book_dump.dump(out, oeb, lambda it: Stylizer(it.data), {item.href: FIRST})
        raw = json.loads((out / "oeb.json").read_text())["raw"]
        self.assertEqual(raw, {"opfname": "content.opf", "sha256": None})

    def test_the_tree_is_written_after_the_stylizer_is_built(self):
        out = run_dump(self.book_dump, self.tmp.name, "a")
        dom = (out / "dom/000.xhtml").read_text()
        self.assertIn('src="pic.png"', dom)  # the img is there
        self.assertNotIn('width="10"', dom)
        self.assertNotIn('height="20"', dom)

    def test_paths_count_elements_only_and_equal_styles_share_an_entry(self):
        out = run_dump(self.book_dump, self.tmp.name, "a")
        styles = json.loads((out / "styles/000.json").read_text())
        nodes = {tuple(path): index for path, index in styles["nodes"]}
        # html, head, title, body, p, p, div, img, span, x:t: comments and the PI are not
        # counted.
        self.assertEqual(
            sorted(nodes),
            sorted([(), (0,), (0, 0), (1,), (1, 0), (1, 1), (1, 2), (1, 2, 0), (1, 2, 1),
                    (1, 2, 2)]),
        )
        self.assertEqual(nodes[(1, 0)], nodes[(1, 1)])
        self.assertEqual(nodes[()], nodes[(1, 2)])
        self.assertEqual(len(styles["table"]), 3)
        p = styles["table"][nodes[(1, 0)]]
        self.assertEqual(p["specified"], {"margin-top": "0", "text-indent": "1em"})
        self.assertEqual(p["pseudo"], {})
        self.assertEqual(p["computed"]["list-style-type"], {"error": "KeyError"})
        self.assertEqual(p["computed"]["width"], 0.1 + 0.2)
        self.assertEqual(p["computed"]["font-size"], 12.0)
        self.assertEqual(set(p["computed"]), set(self.book_dump.COMPUTED_PROPS))
        span = styles["table"][nodes[(1, 2, 1)]]
        self.assertEqual(span["pseudo"], {"hover": {"color": "blue"}})
        self.assertEqual(styles["font_face"], [{"font-family": "Q", "src": 'url("q.ttf")'}])

    def test_flattened_class_lists_are_written_sorted(self):
        # calibre joins an element's classes from a set, in hash order: sorted, two runs of
        # calibre give one dump. Only XHTML and SVG elements get calibre's classes.
        out = Path(self.tmp.name) / "e"
        oeb, item = make_oeb(Path(self.tmp.name) / "e-book")
        self.book_dump.dump(out, oeb, lambda it: Stylizer(it.data), {item.href: FIRST})
        dom = (out / "dom/000.xhtml").read_text()
        self.assertIn('class="calibre1 pcalibre"', dom)
        self.assertIn('class="b a"', dom)
        # kfxgen's own tree is left as calibre made it.
        span = [el for el in item.data.iter() if (el.get("class") or "").startswith("pcalibre")]
        self.assertEqual(len(span), 1)

    def test_two_runs_give_identical_bytes(self):
        a = run_dump(self.book_dump, self.tmp.name, "a")
        b = run_dump(self.book_dump, self.tmp.name, "b")
        self.assertEqual(tree_bytes(a), tree_bytes(b))

    def test_nothing_is_noted_or_written_without_the_variable(self):
        first_pass = {}
        with mock.patch.dict(os.environ, {}, clear=True):
            self.book_dump.note_first_pass(Obj(href="x"), FirstPassStylizer(), first_pass)
            self.book_dump.dump_if_asked(object())
        self.assertEqual(first_pass, {})

    def test_a_spine_file_calibre_never_flattened_stops_the_dump(self):
        out = Path(self.tmp.name) / "c"
        oeb, _ = make_oeb(Path(self.tmp.name) / "c-book")
        with self.assertRaisesRegex(RuntimeError, "text/one.xhtml"):
            self.book_dump.dump(out, oeb, lambda it: Stylizer(it.data), {})
        self.assertFalse((out / "oeb.json").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
