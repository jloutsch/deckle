# SPDX-License-Identifier: GPL-3.0-only
# From kfxgen 8f55440 (plugin/kfxgen/model_dump.py). tools/ref/kfxgen_models.sh copies it into
# an export of kfxgen as plugin/kfxgen/model_dump.py and calls it from generate_full_book.
# The changes: the commit comes from $KFXGEN_COMMIT (the export has no .git), and loading
# takes a model.json as data: it builds only kfxgen's classes and reads only the resources
# their hashes name.
"""Dump the generator's input to a folder, and load it back.

With ``KFXGEN_DUMP_MODEL=<dir>`` set, ``NativeKFXGenerator.generate_full_book``
writes ``<dir>/model.json`` and ``<dir>/resources/<sha256>`` before it
generates. Deckle's Rust port of the generator reads the same folder and must
produce the same KFX from it, so the encoding keeps every Python type the
generator can tell apart:

- ``None``, ``bool``, ``int``, ``str`` and ``float`` are plain JSON (a float is
  written by ``repr``, which reads back exactly; NaN and infinity are refused).
- A list is an array; a tuple ``{"$tuple": [...]}``; a set ``{"$set": [...]}``
  and a frozenset ``{"$frozenset": [...]}``, their items sorted by their JSON.
- A dict is ``{"$dict": [[key, value], ...]}`` in insertion order, so keys may
  be tuples.
- Bytes are ``{"$bytes": "<sha256>"}``, kept once in ``resources/``.
- Any other object is ``{"$obj": "<module>.<qualname>", "state": vars(obj)}``.

A subclass of any of these (a namedtuple, a defaultdict) is refused rather
than written as its base type, which would load back as something else.
"""

import hashlib
import importlib
import json
import os
import re
import types
from pathlib import Path

SCHEMA = 1
# This package as imported: "kfxgen", or "calibre_plugins.kfxgen.kfxgen" inside calibre.
# Classes from it are written as kfxgen.<module>.<class>, so a dump loads anywhere.
_PACKAGE = __package__


def _commit():
    return os.environ.get("KFXGEN_COMMIT", "unknown")


class _Encoder:
    def __init__(self, resources):
        self.resources = resources

    def encode(self, v):
        t = type(v)
        if v is None or t in (bool, int, str):
            return v
        if t is float:
            return v
        if t is list:
            return [self.encode(x) for x in v]
        if t is tuple:
            return {"$tuple": [self.encode(x) for x in v]}
        if t in (set, frozenset):
            items = sorted(
                (self.encode(x) for x in v),
                key=lambda e: json.dumps(e, ensure_ascii=False, allow_nan=False),
            )
            return {"$set" if t is set else "$frozenset": items}
        if t is dict:
            return {"$dict": [[self.encode(k), self.encode(x)] for k, x in v.items()]}
        if t is bytes:
            digest = hashlib.sha256(v).hexdigest()
            path = self.resources / digest
            if not path.exists():
                from .native_generator import _safe_write_bytes

                _safe_write_bytes(str(path), v)
            return {"$bytes": digest}
        if isinstance(
            v, (bool, int, float, str, list, tuple, set, frozenset, dict, bytes)
        ):
            raise TypeError(
                f"model_dump: a {t.__module__}.{t.__qualname__} would load back as its base type"
            )
        if callable(v) or isinstance(v, types.ModuleType):
            raise TypeError(
                f"model_dump: {t.__module__}.{t.__qualname__} is code, not data"
            )
        if not hasattr(v, "__dict__"):
            raise TypeError(
                f"model_dump: {t.__module__}.{t.__qualname__} has no __dict__"
            )
        module = t.__module__
        if module == _PACKAGE or module.startswith(_PACKAGE + "."):
            module = "kfxgen" + module[len(_PACKAGE) :]
        return {
            "$obj": f"{module}.{t.__qualname__}",
            "state": self.encode(vars(v)),
        }


def dump(directory, kwargs):
    """Write ``kwargs`` (the generator's arguments) to ``directory``."""
    from . import __version__

    directory = Path(directory)
    resources = directory / "resources"
    resources.mkdir(parents=True, exist_ok=True)
    doc = {
        "schema": SCHEMA,
        "kfxgen_version": __version__,
        "kfxgen_commit": _commit(),
        "args": _Encoder(resources).encode(kwargs),
    }
    text = json.dumps(doc, ensure_ascii=False, allow_nan=False, indent=1)
    from .native_generator import _safe_write_bytes

    _safe_write_bytes(str(directory / "model.json"), text.encode("utf-8"))


def _resolve(qualified):
    # Importing runs a module's code: only kfxgen's own, which the dump writes as kfxgen.*.
    if not isinstance(qualified, str) or not qualified.startswith("kfxgen."):
        raise ValueError(f"model_dump: {qualified!r} is not a kfxgen class")
    module, _, qualname = qualified.rpartition(".")
    # A nested class: walk back until the prefix imports.
    parts = []
    while module:
        try:
            obj = importlib.import_module(module)
            break
        except ImportError:
            module, _, outer = module.rpartition(".")
            parts.insert(0, outer)
    else:
        raise ImportError(f"model_dump: cannot import {qualified}")
    for name in parts + qualname.split("."):
        obj = getattr(obj, name)
    # Not a name a kfxgen module only imports (its Path, its tempfile).
    home = getattr(obj, "__module__", None) or ""
    if not isinstance(obj, type) or not any(
        home == p or home.startswith(p + ".") for p in ("kfxgen", _PACKAGE)
    ):
        raise ValueError(f"model_dump: {qualified!r} is not a kfxgen class")
    return obj


def _decode(e, resources):
    if isinstance(e, list):
        return [_decode(x, resources) for x in e]
    if not isinstance(e, dict):
        return e
    (tag, body), *rest = e.items()
    if tag == "$tuple":
        return tuple(_decode(x, resources) for x in body)
    if tag == "$set":
        return {_decode(x, resources) for x in body}
    if tag == "$frozenset":
        return frozenset(_decode(x, resources) for x in body)
    if tag == "$dict":
        return {_decode(k, resources): _decode(v, resources) for k, v in body}
    if tag == "$bytes":
        # A name is the sha256 of its bytes: no other path is read, and no other bytes.
        if not isinstance(body, str) or not re.fullmatch(r"[0-9a-f]{64}", body):
            raise ValueError(f"model_dump: resource {body!r} is not a sha256")
        data = (resources / body).read_bytes()
        if hashlib.sha256(data).hexdigest() != body:
            raise ValueError(f"model_dump: resource {body} is not the bytes it names")
        return data
    if tag == "$obj":
        cls = _resolve(body)
        obj = cls.__new__(cls)
        obj.__dict__.update(_decode(e["state"], resources))
        return obj
    raise ValueError(f"model_dump: unknown tag {tag!r}")


def load(directory):
    """Read a dump back as the generator's keyword arguments."""
    directory = Path(directory)
    doc = json.loads((directory / "model.json").read_text(encoding="utf-8"))
    if doc.get("schema") != SCHEMA:
        raise ValueError(f"model_dump: schema {doc.get('schema')!r}, expected {SCHEMA}")
    return _decode(doc["args"], directory / "resources")


def dump_if_asked(kwargs):
    """The generator's hook: dump when ``KFXGEN_DUMP_MODEL`` is set."""
    directory = os.environ.get("KFXGEN_DUMP_MODEL")
    if directory:
        dump(directory, kwargs)
