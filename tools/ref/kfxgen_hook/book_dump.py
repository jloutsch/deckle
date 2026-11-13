# SPDX-License-Identifier: GPL-3.0-only
# tools/ref/kfxgen_hook/inject.py copies this into an export of kfxgen as
# plugin/kfxgen/book_dump.py and calls it from two places (see there).
"""Dump what Calibre hands kfxgen: the book view, each spine file's tree, and its styles.

With ``KFXGEN_DUMP_BOOK=<dir>`` set:

- ``note_first_pass`` runs at the start of the plugin's ``specialize_css_for_output``, which
  Calibre calls on each file before it flattens the CSS. It keeps the first-pass Stylizer's
  rules: the only place the book's own CSS is still visible.
- ``dump_if_asked`` runs in ``convert_oeb_to_kfx`` once the OEB has its opts, before kfxgen
  reads anything. For each spine file it builds kfxgen's own Stylizer (``build_stylizer``) and
  writes, under ``<dir>``:

  - ``files.json``: spine href to ``NNN``, the file's spine position.
  - ``dom/NNN.xhtml``: the tree, written after the Stylizer is built (building it removes
    ``img`` ``width`` and ``height``), several classes of one element sorted (``_tostring``).
  - ``styles/NNN.json``: ``table``, the distinct styles, each ``{"specified": cssdict(),
    "pseudo": _pseudo_classes, "computed": {prop: s[prop]}}`` for ``COMPUTED_PROPS`` (a lookup
    that raises is ``{"error": "<exception class>"}``); ``nodes``, ``[path, table_index]`` for
    every element in document order, ``path`` being element-child indexes from the root
    (comments and processing instructions not counted); ``font_face``, the Stylizer's
    ``@font-face`` rules as ``{prop: value}``.
  - ``rules/NNN.json``: the first-pass ``rules``, ``[origin, selector_text, cssdict]`` in
    cascade order (origin 0 for Calibre's user-agent sheet, 1 for the book), and its
    ``font_face``.
  - ``oeb.json``, last, so its presence means the dump is whole: ``spine``,
    ``page_progression_direction``, ``manifest`` (sorted by href), ``toc``,
    ``auto_generated_toc``, ``guide`` (sorted by type), ``metadata`` and ``raw`` (the
    EPUB's own OPF: its path in the EPUB and its sha256; see ``_raw_opf``).

Floats are written by ``repr``, which reads back exactly. Nothing here changes what kfxgen
makes: it only builds one more Stylizer per file, as kfxgen itself does several times.
"""

import copy
import hashlib
import json
import os
from pathlib import Path

ENV = "KFXGEN_DUMP_BOOK"

COMPUTED_PROPS = (
    "text-align",
    "font-family",
    "font-style",
    "white-space",
    "list-style-type",
    "border-collapse",
    *(
        f"border-{side}-{part}"
        for side in ("top", "right", "bottom", "left")
        for part in ("style", "width", "color")
    ),
    "font-size",
    "line-height",
    "width",
    "height",
    *(f"margin-{side}" for side in ("top", "right", "bottom", "left")),
    *(f"padding-{side}" for side in ("top", "right", "bottom", "left")),
    "text-indent",
    "color",
    "background-color",
    "font-weight",
)

# href -> the first pass's rules, kept between the two hooks of one conversion.
_FIRST_PASS = {}


def _font_faces(stylizer):
    return [
        {prop.name: prop.value for prop in rule.style}
        for rule in stylizer.font_face_rules
    ]


def note_first_pass(item, stylizer, first_pass=None):
    """The plugin's ``specialize_css_for_output`` hook: keep this file's first-pass rules."""
    if not os.environ.get(ENV):
        return
    # A rule is (sort key, selector parts, cssdict, selector text, sheet href); the key's
    # first item is 0 for the user-agent sheet, 1 for the book's.
    (_FIRST_PASS if first_pass is None else first_pass)[item.href] = {
        "rules": [[rule[0][0], rule[3], dict(rule[2])] for rule in stylizer.rules],
        "font_face": _font_faces(stylizer),
    }


def _json(value, **kw):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, **kw)


def _write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text.encode("utf-8"))


def _list_file(fields, sort_keys):
    """A JSON object of ``fields`` (name -> value), a list value one item per line."""
    parts = []
    for name, value in fields:
        if isinstance(value, list) and value:
            body = ",\n".join(f"  {_json(x, sort_keys=sort_keys)}" for x in value)
            parts.append(f" {_json(name)}: [\n{body}\n ]")
        else:
            parts.append(f" {_json(name)}: {_json(value, sort_keys=sort_keys)}")
    return "{\n" + ",\n".join(parts) + "\n}\n"


FLATTENED_NS = ("{http://www.w3.org/1999/xhtml}", "{http://www.w3.org/2000/svg}")


def _tostring(root):
    """The tree as text, an element's classes sorted where calibre gave it several.

    CSSFlattener writes an element with pseudo-class styles as ``' '.join(a_set)``, so the
    order follows Python's string hashing and changes between runs (``calibre1 pcalibre``
    or ``pcalibre calibre1``); calibre's frozen Python ignores PYTHONHASHSEED. Only XHTML
    and SVG elements get calibre's classes, so only theirs are sorted, on a copy: kfxgen's
    tree is left as calibre made it."""
    root = copy.deepcopy(root)
    for el in root.iter():
        if not isinstance(el.tag, str) or not el.tag.startswith(FLATTENED_NS):
            continue
        classes = (el.get("class") or "").split()
        if len(classes) > 1:
            el.set("class", " ".join(sorted(classes)))
    # lxml inside calibre. The test's stub tree is ElementTree's when lxml isn't installed.
    if type(root).__module__.startswith("lxml"):
        from lxml import etree
    else:
        import xml.etree.ElementTree as etree
    return etree.tostring(root, encoding="unicode")


def _elements(root):
    """(path, element) for every element in document order. A loop, not recursion: a deep
    tree would pass Python's recursion limit."""
    stack = [((), root)]
    while stack:
        path, el = stack.pop()
        yield path, el
        children = [c for c in el if isinstance(c.tag, str)]
        stack.extend(
            (path + (i,), child) for i, child in reversed(list(enumerate(children)))
        )


def _computed(style, prop):
    try:
        return style[prop]
    except Exception as e:  # what Calibre raises is the answer: KeyError, ValueError...
        return {"error": type(e).__name__}


def _styles(root, stylizer):
    table, index, nodes = [], {}, []
    for path, el in _elements(root):
        s = stylizer.style(el)
        entry = {
            "specified": dict(s.cssdict()),
            "pseudo": {k: dict(v) for k, v in s._pseudo_classes.items()},
            "computed": {prop: _computed(s, prop) for prop in COMPUTED_PROPS},
        }
        key = _json(entry, sort_keys=True)
        if key not in index:
            index[key] = len(table)
            table.append(entry)
        nodes.append([list(path), index[key]])
    return _list_file(
        [("font_face", _font_faces(stylizer)), ("nodes", nodes), ("table", table)],
        sort_keys=True,
    )


def _toc(node):
    return [
        {"title": child.title, "href": child.href, "children": _toc(child)}
        for child in node.nodes
    ]


OPF_MIME = "application/oebps-package+xml"


def _raw_opf(rootdir, calibre_opfname):
    """The EPUB's own OPF, as unpacked: its path from META-INF/container.xml (the first
    rootfile of the OPF type that exists, as EPUBInput.find_opf takes it) and its sha256.

    Not ``container.opfname``: that is the OPF calibre writes after reading the book
    (``content.opf`` at the root, hrefs from the root, and a manifest item for an NCX made
    from the nav under a random temporary name), so it differs on every run. When the
    book's own OPF is that same file, calibre wrote over it, and sha256 is None."""
    import xml.etree.ElementTree as ET

    root = Path(rootdir)
    try:
        container = ET.parse(root / "META-INF" / "container.xml").getroot()
    except (OSError, ET.ParseError):
        return {"opfname": None, "sha256": None}
    for el in container.iter():
        if not isinstance(el.tag, str) or el.tag.rpartition("}")[2] != "rootfile":
            continue
        if el.get("media-type") != OPF_MIME or not el.get("full-path"):
            continue
        name = el.get("full-path")
        path = root.joinpath(*name.split("/"))
        if path.is_file():
            if path.resolve() == (root / calibre_opfname).resolve():
                return {"opfname": name, "sha256": None}
            return {"opfname": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
    return {"opfname": None, "sha256": None}


def _oeb_view(oeb):
    container = oeb.container
    return {
        "spine": [{"href": item.href, "linear": item.linear} for item in oeb.spine],
        "page_progression_direction": oeb.spine.page_progression_direction,
        "manifest": sorted(
            (
                {"id": item.id, "href": item.href, "media_type": item.media_type}
                for item in oeb.manifest
            ),
            key=lambda m: (m["href"], m["id"]),
        ),
        "toc": _toc(oeb.toc),
        "auto_generated_toc": oeb.auto_generated_toc,
        "guide": sorted(
            ({"type": ref.type, "href": ref.href} for ref in oeb.guide.values()),
            key=lambda g: (g["type"], g["href"]),
        ),
        "metadata": {
            name: [str(x) for x in getattr(oeb.metadata, name)]
            for name in (
                "title",
                "creator",
                "description",
                "language",
                "publisher",
                "date",
                "cover",
            )
        },
        "raw": _raw_opf(container.rootdir, container.opfname),
    }


def dump(directory, oeb, stylizer_for, first_pass):
    """Write the dump of ``oeb`` to ``directory``; ``stylizer_for(item)`` builds kfxgen's
    Stylizer, ``first_pass`` is what ``note_first_pass`` kept."""
    directory = Path(directory)
    files = {}
    for n, item in enumerate(oeb.spine):
        if item.href not in first_pass:
            raise RuntimeError(
                f"book_dump: calibre never flattened spine file {item.href!r}"
            )
        name = f"{n:03d}"
        files[item.href] = name
        stylizer = stylizer_for(item)  # first: it removes img width and height
        _write(directory / "dom" / f"{name}.xhtml", _tostring(item.data))
        _write(directory / "styles" / f"{name}.json", _styles(item.data, stylizer))
        first = first_pass[item.href]
        _write(
            directory / "rules" / f"{name}.json",
            # Declarations in Calibre's order, which the port keeps too.
            _list_file(
                [("font_face", first["font_face"]), ("rules", first["rules"])],
                sort_keys=False,
            ),
        )
    _write(directory / "files.json", _json(files, indent=1) + "\n")
    _write(directory / "oeb.json", _json(_oeb_view(oeb), indent=1) + "\n")


def dump_if_asked(oeb_book):
    """The converter's hook: dump when ``KFXGEN_DUMP_BOOK`` is set."""
    directory = os.environ.get(ENV)
    if not directory:
        return
    from .font_table import build_stylizer

    dump(directory, oeb_book, lambda item: build_stylizer(oeb_book, item), _FIRST_PASS)
