# SPDX-License-Identifier: GPL-3.0-only
# tools/ref/kfxgen_hook/inject.py copies this into an export of kfxgen as
# plugin/kfxgen/convert_dump.py and calls it from three places (see there).
"""Dump what kfxgen's converter makes on its way to the chapters, layer by layer.

With ``KFXGEN_DUMP_CONVERT=<dir>`` set, ``install`` (called in ``convert_oeb_to_kfx`` once
the OEB has its opts) replaces six module-level names of ``converter`` with wrappers that
call the original and keep a deep copy of what it took or gave. Every call site looks the
name up when it calls (a bare global name), so the wrappers see every call, recursive ones
too. ``note_assembly`` is called in ``extract_chapters_from_oeb`` right before chapter
assembly, and ``write_if_asked`` at the end of ``convert_oeb_to_kfx``, which writes, under
``<dir>``:

- ``files.json``: spine href to ``NNN``, its position in calibre's spine (as ``book_dump``).
- ``resolver/NNN.json.gz`` (D1), from ``_build_style_resolver``, once per href (the first
  resolver built for it: the first pass's per-file loop's; every later one, in a second pass or
  ``_footnote_keys``, must give the same rows, or the dump fails): ``[path, resolve(e),
  compute_block_style(resolve(e))]`` for every element in document order, ``path`` being
  element-child indexes from the root (comments and processing instructions not counted),
  the last item None where ``resolve`` gives None.
- ``inline/NNN.json.gz`` (W), from ``_walk_inline``: for each call with ``is_root`` true in the
  first pass, in the order the calls start, ``[path, segments,
  normalize_runs_with_anchors(segments)]``; a second pass's in ``inline/NNN.pass1.json.gz``;
  the ``(_TABLE_SPLIT, table)`` items left out of ``segments``. A call that raises is
  ``[path, None, {"error": "<exception class>"}]``.
- ``blocks/NNN.json.gz`` (D2), from ``extract_blocks_from_html``: one record per call (one
  per pass), ``{"pass", "args", "blocks", "nav_listing_at", "tables_seen", "notes_seen",
  "trailing_ids", "end_ids", "spacing"}``. ``pass`` is 0, or 1 in the rerun with
  ``keep_nav_listings`` (a pass is counted at each ``note_assembly``). ``args`` holds
  ``native_tables``, ``keep_nav_listings`` and ``toc_targets`` (sorted, or None). ``blocks`` is the return value,
  deep-copied at return; the out-lists are as the call left them, ``tables_seen`` and
  ``notes_seen`` written as each block's place in ``blocks`` (a list of indexes and keys,
  ``[3, "blocks", 0]``, or None when the block is not there). ``spacing`` (D2a) is every
  ``_space_between`` call made inside it, in call order: ``{"depth", "measured", "blocks"}``,
  ``blocks`` the input blocks as ``[block, margins.get(block), paddings.get(block)]``, deep-copied
  at the call; ``depth`` 0 for a call not inside another. (A call made outside any
  ``extract_blocks_from_html`` is not written; kfxgen makes none.)
- ``toc.json.gz`` (T), when ``_respell_toc_hrefs`` first runs: ``{"toc_entries"`` (after
  respelling), ``"spine"`` (the hrefs ``_spine_in_order`` gave), ``"note_target_ids"`` (href
  to ``_note_target_ids(item.data)`` for each of those files), ``"note_pair_ids"`` (sorted by
  file: its own order follows string hashing), ``"link_targets"`` (``_book_link_targets``,
  sorted)``}``. The three functions are called here on the whole book, whether or not
  kfxgen called them in this conversion.
- ``assembly.json.gz`` (A): one record per pass (per ``extract_chapters_from_oeb`` that gets
  that far), ``{"spine_items_ordered", "cover_href", "chapters", "listing_without_contents"}``:
  the files deep-copied right before assembly, and the chapters deep-copied when
  ``_listing_without_contents`` is asked about them, which is right before
  ``_replace_title_page`` on both of the function's paths, and the answer.
- ``calibre_opf.xml``, ``calibre_ncx.xml``: the OPF calibre wrote (``container.opfname``) and
  the NCX its manifest names first, verbatim (no NCX file when it names none), but for the
  name calibre gives an NCX it makes from an EPUB 3 nav: Python's ``tempfile`` name, new on
  every run, written ``tmpXXXXXXXX.ncx``.

Values are written in ``model_dump``'s encoding (bytes into ``resources/``, which is removed
when empty), as gzip with no name or time in the header. Nothing here changes what kfxgen
makes: each wrapper calls the original once with the same arguments and returns what it
returned; the extra calls (``resolve`` on every element, ``compute_block_style``,
``normalize_runs_with_anchors``, the note and link functions) only read the tree. An error
in a dump is kept and raised at the end, outside kfxgen's own ``try``, so it stops the
conversion rather than changing it.

``python3 convert_dump.py units <kfxgen_tree> <out.json>`` writes the unit table of
``inline_style.py`` and ``font_table.py`` instead (no Calibre): each function's name with its
cases as ``[args, result]``.
"""

import copy
import gzip
import inspect
import json
import os
import re
import sys
from pathlib import Path

ENV = "KFXGEN_DUMP_CONVERT"

WRAPPED = (
    "_build_style_resolver",
    "_walk_inline",
    "extract_blocks_from_html",
    "_space_between",
    "_respell_toc_hrefs",
    "_listing_without_contents",
)

# name -> converter's own function, kept so the wrappers are put in only once per process.
_ORIGINALS = {}
# What one conversion has noted so far; None outside a conversion with the variable set.
_state = None


class _State:
    def __init__(self, oeb):
        self.oeb = oeb
        self.resolver = {}  # href -> D1 rows
        self.inline = {}  # href -> W records
        self.blocks = {}  # href -> D2 records
        self.toc = None
        self.assembly = []
        self.calls = []  # the extract_blocks_from_html records open now, innermost last
        self.spacing = []  # D2a records not yet taken by an extract_blocks_from_html record
        self.depth = 0  # _space_between calls open now
        self.failures = []


def _converter():
    from . import converter

    return converter


def _elements(root):
    """(path, element) for every element in document order (book_dump's)."""
    from .book_dump import _elements as elements

    return elements(root)


def _path(elem):
    """Element-child indexes from the root to ``elem``."""
    path = []
    parent = elem.getparent()
    while parent is not None:
        path.append([c for c in parent if isinstance(c.tag, str)].index(elem))
        elem, parent = parent, parent.getparent()
    return path[::-1]


def _where(root, target):
    """Indexes and keys leading from ``root`` to the object ``target`` itself, or None."""
    stack = [(root, [])]
    while stack:
        value, path = stack.pop()
        if value is target:
            return path
        if isinstance(value, dict):
            items = list(value.items())
        elif isinstance(value, list):
            items = list(enumerate(value))
        else:
            continue
        stack.extend((v, path + [k]) for k, v in reversed(items))
    return None


def _noting(fn):
    """``fn()``; an error is kept for ``write_if_asked`` instead of reaching kfxgen."""
    try:
        return fn()
    except Exception as e:  # raised again at the end, outside kfxgen's try
        _state.failures.append(e)
        return None


def _wrap_build_style_resolver(original):
    from .inline_style import compute_block_style

    def _build_style_resolver(oeb_book, item, log, *args, **kwargs):
        resolve = original(oeb_book, item, log, *args, **kwargs)
        href = getattr(item, "href", "") or ""
        data = getattr(item, "data", None)
        if _state is None or resolve is None or data is None:
            return resolve

        def note():
            rows = []
            for path, el in _elements(data):
                css = copy.deepcopy(resolve(el))
                rows.append([list(path), css, None if css is None else compute_block_style(css)])
            if href not in _state.resolver:
                _state.resolver[href] = rows
            elif rows != _state.resolver[href]:
                # A later build (a second pass, _footnote_keys) on a tree that changed.
                raise RuntimeError(f"convert_dump: {href!r}: a resolver unlike the first")

        _noting(note)
        return resolve

    return _build_style_resolver


def _wrap_walk_inline(original):
    from .inline_style import normalize_runs_with_anchors

    signature = inspect.signature(original)

    def _walk_inline(elem, *args, **kwargs):
        if _state is None:
            return original(elem, *args, **kwargs)
        bound = signature.bind(elem, *args, **kwargs)
        if not bound.arguments.get("is_root", True):
            return original(elem, *args, **kwargs)
        href = (
            _state.calls[-1]["href"] if _state.calls else bound.arguments.get("base_href") or ""
        )
        records = _state.inline.setdefault((href, len(_state.assembly)), [])
        at = len(records)
        records.append(None)  # its place: the calls inside it come after it
        path = _path(elem) if isinstance(elem.tag, str) else None
        try:
            parts = original(elem, *args, **kwargs)
        except Exception as e:
            records[at] = [path, None, {"error": type(e).__name__}]
            raise

        def note():
            split = _converter()._TABLE_SPLIT
            segments = copy.deepcopy([p for p in parts if p[0] is not split])
            records[at] = [path, segments, normalize_runs_with_anchors(segments)]

        _noting(note)
        return parts

    return _walk_inline


def _wrap_extract_blocks(original):
    signature = inspect.signature(original)

    def extract_blocks_from_html(element, *args, **kwargs):
        if _state is None:
            return original(element, *args, **kwargs)
        bound = signature.bind(element, *args, **kwargs)
        bound.apply_defaults()
        a = bound.arguments
        toc_targets = a["toc_targets"]
        call = {
            "href": a["base_href"] or "",
            "args": {
                "native_tables": a["native_tables"],
                "keep_nav_listings": a["keep_nav_listings"],
                "toc_targets": None if toc_targets is None else sorted(toc_targets),
            },
        }
        start = len(_state.spacing)
        _state.calls.append(call)
        try:
            blocks = original(element, *args, **kwargs)
        finally:
            _state.calls.pop()
            call["spacing"] = _state.spacing[start:]
            del _state.spacing[start:]

        def note():
            def places(seen):
                return None if seen is None else [_where(blocks, b) for b in seen]

            def kept(name):
                return copy.deepcopy(a[name])

            _state.blocks.setdefault(call.pop("href"), []).append(
                {
                    "pass": len(_state.assembly),
                    "args": call["args"],
                    "blocks": copy.deepcopy(blocks),
                    "nav_listing_at": kept("nav_listing_at"),
                    "tables_seen": places(a["tables_seen"]),
                    "notes_seen": places(a["notes_seen"]),
                    "trailing_ids": kept("trailing_ids"),
                    "end_ids": kept("end_ids"),
                    "spacing": call["spacing"],
                }
            )

        _noting(note)
        return blocks

    return extract_blocks_from_html


def _wrap_space_between(original):
    signature = inspect.signature(original)

    def _space_between(blocks, *args, **kwargs):
        if _state is None:
            return original(blocks, *args, **kwargs)
        bound = signature.bind(blocks, *args, **kwargs)
        bound.apply_defaults()
        a = bound.arguments

        def note():
            margins, paddings = a["margins"], a["paddings"]
            record = {
                "depth": _state.depth,
                "measured": a["measured"],
                "blocks": copy.deepcopy(
                    [
                        [b, margins.get(b), None if paddings is None else paddings.get(b)]
                        for b in blocks
                    ]
                ),
            }
            _state.spacing.append(record)

        _noting(note)
        _state.depth += 1
        try:
            return original(blocks, *args, **kwargs)
        finally:
            _state.depth -= 1

    return _space_between


def _wrap_respell_toc_hrefs(original):
    def _respell_toc_hrefs(entries, spine, *args, **kwargs):
        out = original(entries, spine, *args, **kwargs)
        if _state is None or _state.toc is not None:
            return out

        def note():
            c = _converter()
            oeb = _state.oeb
            _state.toc = {
                "toc_entries": copy.deepcopy(entries),
                "spine": [i.href for i in spine],
                "note_target_ids": {
                    i.href: c._note_target_ids(i.data)
                    for i in spine
                    if getattr(i, "data", None) is not None
                },
                # Sorted: its order follows string hashing, which changes between runs,
                # and kfxgen only looks files up in it.
                "note_pair_ids": dict(sorted(c._note_pair_ids(oeb).items())),
                "link_targets": sorted(c._book_link_targets(oeb)),
            }

        _noting(note)
        return out

    return _respell_toc_hrefs


def _wrap_listing_without_contents(original):
    def _listing_without_contents(chapters, *args, **kwargs):
        if _state is None:
            return original(chapters, *args, **kwargs)
        before = _noting(lambda: copy.deepcopy(chapters))
        answer = original(chapters, *args, **kwargs)

        def note():
            if not _state.assembly or "chapters" in _state.assembly[-1]:
                raise RuntimeError("convert_dump: chapters with no pass before them")
            _state.assembly[-1]["chapters"] = before
            _state.assembly[-1]["listing_without_contents"] = answer

        _noting(note)
        return answer

    return _listing_without_contents


_WRAPPERS = {
    "_build_style_resolver": _wrap_build_style_resolver,
    "_walk_inline": _wrap_walk_inline,
    "extract_blocks_from_html": _wrap_extract_blocks,
    "_space_between": _wrap_space_between,
    "_respell_toc_hrefs": _wrap_respell_toc_hrefs,
    "_listing_without_contents": _wrap_listing_without_contents,
}


def install(oeb_book):
    """``convert_oeb_to_kfx``'s hook: start noting, when ``KFXGEN_DUMP_CONVERT`` is set."""
    global _state
    if not os.environ.get(ENV):
        _state = None
        return
    c = _converter()
    if not _ORIGINALS:
        for name in WRAPPED:
            _ORIGINALS[name] = getattr(c, name)
            setattr(c, name, _WRAPPERS[name](_ORIGINALS[name]))
    _state = _State(oeb_book)


def note_assembly(spine_items_ordered, cover_href):
    """``extract_chapters_from_oeb``'s hook, right before assembly: a new pass."""
    if _state is None:
        return

    def note():
        _state.assembly.append(
            {
                "spine_items_ordered": copy.deepcopy(spine_items_ordered),
                "cover_href": cover_href,
            }
        )

    _noting(note)


#: The name calibre gives an NCX it makes from an EPUB 3 nav (tempfile's), new on every run.
_MADE_NCX = re.compile(rb"tmp[a-z0-9_]{8}\.ncx")


def _calibre_files(oeb):
    """(OPF bytes, NCX bytes or None): calibre's OPF and the first NCX its manifest names.
    The OPF is verbatim but for the random part of a made NCX's name (``tmpXXXXXXXX.ncx``)."""
    import posixpath
    import xml.etree.ElementTree as ET
    from urllib.parse import unquote

    container = oeb.container
    root = os.path.realpath(container.rootdir)

    def read(name):
        full = os.path.realpath(os.path.join(root, name))
        if os.path.commonpath([root, full]) != root:
            raise RuntimeError(f"convert_dump: {name!r} is outside the book's folder")
        return Path(full).read_bytes()

    opf = read(container.opfname)
    for el in ET.fromstring(opf).iter():
        if (
            isinstance(el.tag, str)
            and el.tag.rpartition("}")[2] == "item"
            and el.get("media-type") == "application/x-dtbncx+xml"
        ):
            name = posixpath.normpath(
                posixpath.join(posixpath.dirname(container.opfname), unquote(el.get("href")))
            )
            return _MADE_NCX.sub(b"tmpXXXXXXXX.ncx", opf), read(name)
    return opf, None


def _gz_json(path, value, encoder):
    text = json.dumps(encoder.encode(value), ensure_ascii=False, allow_nan=False)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(gzip.compress(text.encode("utf-8"), 9, mtime=0))


def write(directory, state):
    """Write what ``state`` noted to ``directory``."""
    from .model_dump import _Encoder

    directory = Path(directory)
    resources = directory / "resources"
    resources.mkdir(parents=True, exist_ok=True)
    encoder = _Encoder(resources)
    files = {item.href: f"{n:03d}" for n, item in enumerate(state.oeb.spine)}
    (directory / "files.json").write_text(
        json.dumps(files, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )
    # The first pass's runs in inline/NNN.json.gz, a second pass's in inline/NNN.pass1.json.gz.
    inline = {
        (href, f".pass{n}" if n else ""): records for (href, n), records in state.inline.items()
    }
    for kind, noted in (
        ("resolver", {(href, ""): v for href, v in state.resolver.items()}),
        ("inline", inline),
        ("blocks", {(href, ""): v for href, v in state.blocks.items()}),
    ):
        for (href, suffix), value in noted.items():
            if href not in files:
                raise RuntimeError(f"convert_dump: {kind} of {href!r}, not a spine file")
            _gz_json(directory / kind / f"{files[href]}{suffix}.json.gz", value, encoder)
    if state.toc is not None:
        _gz_json(directory / "toc.json.gz", state.toc, encoder)
    _gz_json(directory / "assembly.json.gz", state.assembly, encoder)
    opf, ncx = _calibre_files(state.oeb)
    (directory / "calibre_opf.xml").write_bytes(opf)
    if ncx is not None:
        (directory / "calibre_ncx.xml").write_bytes(ncx)
    if not any(resources.iterdir()):
        resources.rmdir()


def write_if_asked():
    """``convert_oeb_to_kfx``'s hook once the book is made: write, or raise what failed."""
    global _state
    state, _state = _state, None
    if state is None:
        return
    if state.failures:
        raise RuntimeError("convert_dump: a dump failed") from state.failures[0]
    write(os.environ[ENV], state)


# The unit table: inputs chosen by hand for each pure function the port has a test of its own.

LENGTHS = [
    None, "", "auto", "0", "0em", "-0em", "1em", "1.50em", "2.0em", "-1em", "+1.5em", ".5em",
    "2 em", "1.25EM", " 3pt ", "12px", "1in", "2.54cm", "1ex", "50%", "3rem", "10mm", "1e3px",
    "0.33333em", "-0.5em", "1.0000em", "100.", "7q",
]
VERTICAL_ALIGNS = [
    None, "", "super", "SUPER", " sub ", "top", "text-top", "text-bottom", "baseline",
    "middle", "bottom", "0.25em", "-0.25em", "+2px", "0", "0em", "33%", "-3px", "1.5ex",
    "-.1rem", "2 pt", "10mm",
]
COLOURS = [
    None, "", "red", "RED", "#fff", "#FFFFFF", "#ffff", "#12345678", "#ggg", "rgb(255,0,0)",
    "rgb(100%, 50%, 0%)", "rgb(300,-1,0)", "rgba(0,0,0,0.5)", "rgba(255,255,255,0)",
    "rgba(0,0,255,50%)", "transparent", "hsl(120, 100%, 25%)", "hsla(0,100%,50%,0.5)",
    "hsl(400, 150%, 50%)", "rebeccapurple", "windowtext", "inherit", "navy", "darkred",
    "yellow", "white", "black", "#767676", "#999999", "#1a1a1a", "#202020", "#333333",
]
BACKGROUNDS = [None, 0x000000, 0x333333, 0xFFFF00, 0xFFFFFF]
LETTER_SPACINGS = [
    None, "", "normal", "0.1em", "2px", "3px", "1pt", "-1px", "5%", "1mm", "0.05rem", "1in",
]
WEIGHTS = [None, "", "bold", "BOLD", "normal", "700", " 600 ", "bolder", "lighter", "100",
           "abc", 700, 650.5, "1000"]
FAMILIES = [
    ("Liberation Serif", 400, False, []),
    ("Liberation Serif", 700, True, ["liberation-serif-700i"]),
    ("Liberation Serif", 700, True, ["liberation-serif-700i", "liberation-serif-700i-1"]),
    ("'My Font', serif", 400, True, []),
    ("Ünïcödé Fäm", 400, False, []),
    ("  ", 400, False, []),
    ("Font_Name 2", 300, False, ["font-name-2-300"]),
]


def _segments(inline_style):
    """normalize_runs_with_anchors cases: lists of (text, flags) segments."""
    s = inline_style
    plain, italic, bold = frozenset(), frozenset({s.FLAG_ITALIC}), frozenset({s.FLAG_BOLD})
    link = frozenset({s.make_link_flag("b.xhtml#n1")})
    pre = frozenset({s.FLAG_PRE})
    pre_line = frozenset({s.FLAG_PRE_LINE})
    start = frozenset({s.LINE_START})
    nbsp = s.NBSP
    return [
        [],
        [("  plain   words  \n here ", plain)],
        [(nbsp, plain)],
        [(nbsp * 3, italic)],
        [(" a ", plain), (nbsp, plain), (" b", plain)],
        [("a\x1cb\x1dc\x1ed\x1ff", plain)],
        [("x y", plain), (" ", plain), (" z", plain)],
        [(" lead", plain)],
        [("a b\u0085c\u000bd", plain)],
        [("\tone\ttwo\n  \tthree", pre)],
        [("col\r\nrow  \n", pre)],
        [("  kept  ", pre), ("end", plain)],
        [("line one  \n  line two\n\n", pre_line)],
        [("\U0001d518\U0001f600 then ", plain), ("a link", link), (" after", plain)],
        [("before ", plain), ("italic ", italic), ("italic", italic), (" bold", bold)],
        [s.make_anchor_mark("start"), ("text ", plain), s.make_anchor_mark("mid"),
         ("more", italic), s.make_anchor_mark("mid"), ("  ", plain), s.make_anchor_mark("end")],
        [("verse", plain), ("", start), ("", start), ("next", plain), (s.HARD_BREAK, plain),
         ("", start), ("last", plain)],
        [("", start), ("first", plain)],
        [("a", plain), (s.HARD_BREAK, plain), ("  ", plain), (s.HARD_BREAK, plain), ("b", plain)],
        [("　ideographic　space　", plain)],
        [(" ​ zero width ", plain)],
    ]


def unit_table(inline_style, font_table):
    s, f = inline_style, font_table
    table = {
        "parse_css_length": [[[v, neg], s.parse_css_length(v, neg)] for v in LENGTHS
                             for neg in (False, True)],
        "parse_vertical_align": [[[v], s.parse_vertical_align(v)] for v in VERTICAL_ALIGNS],
        "css_rgb": [[[v], s.css_rgb(v)] for v in COLOURS],
        "css_text_colour": [[[v, bg], s.css_text_colour(v, bg)] for v in COLOURS
                            for bg in BACKGROUNDS],
        "css_link_colour": [[[v], s.css_link_colour(v)] for v in COLOURS],
        "css_letter_spacing": [[[v], s.css_letter_spacing(v)] for v in LETTER_SPACINGS],
        "normalize_runs_with_anchors": [
            [[segments], s.normalize_runs_with_anchors(segments)] for segments in _segments(s)
        ],
        "parse_weight": [[[v], f.parse_weight(v)] for v in WEIGHTS],
        "emitted_name": [],
    }
    for family, weight, italic, taken in FAMILIES:
        before = set(taken)
        name = f.emitted_name(family, weight, italic, set(taken))
        table["emitted_name"].append([[family, weight, italic, before], name])
    return table


def write_units(tree, out):
    sys.path.insert(0, str(Path(tree) / "plugin"))
    from kfxgen import font_table, inline_style
    from kfxgen.model_dump import _Encoder

    text = json.dumps(
        _Encoder(None).encode(unit_table(inline_style, font_table)),
        ensure_ascii=False,
        allow_nan=False,
        indent=1,
    )
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text + "\n", encoding="utf-8")


if __name__ == "__main__":
    if len(sys.argv) != 4 or sys.argv[1] != "units":
        sys.exit(f"usage: {sys.argv[0]} units <kfxgen_tree> <out.json>")
    write_units(sys.argv[2], sys.argv[3])
