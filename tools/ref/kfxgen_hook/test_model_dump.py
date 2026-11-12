# SPDX-License-Identifier: GPL-3.0-or-later
# From kfxgen 8f55440 (tests/unit/test_model_dump.py): its encoding tests, run against a stub
# kfxgen package so they need no kfxgen tree. The test that replays kfxgen's golden inputs
# stays out (it needs kfxgen's tests package); regenerating the fixtures proves it, since
# kfxgen_models.sh checks every replay against ebook-convert's own bytes.
"""model_dump keeps every type the generator can tell apart.  Run: python3 -m pytest <this file>"""

import collections
import importlib
import json
import shutil
import sys
import types
from pathlib import Path

import pytest


@pytest.fixture
def model_dump(tmp_path, monkeypatch):
    pkg = tmp_path / "pkg" / "kfxgen"
    pkg.mkdir(parents=True)
    (pkg / "__init__.py").write_text('__version__ = "test"\n')
    (pkg / "font_table.py").write_text("class FontTable:\n    pass\n")
    (pkg / "native_generator.py").write_text(
        "def _safe_write_bytes(path, data):\n    open(path, 'wb').write(data)\n"
    )
    shutil.copy(Path(__file__).with_name("model_dump.py"), pkg / "model_dump.py")
    monkeypatch.syspath_prepend(str(tmp_path / "pkg"))
    for name in [m for m in sys.modules if m == "kfxgen" or m.startswith("kfxgen.")]:
        monkeypatch.delitem(sys.modules, name)
    return importlib.import_module("kfxgen.model_dump")


def test_dump_encodes_tuple_set_bytes_and_objects(model_dump, tmp_path):
    obj = importlib.import_module("kfxgen.font_table").FontTable()
    vars(obj).update(name="Serif", faces={("normal", 400): b"\x00\x01font"})
    value = {
        "none": None,
        "flags": [True, False, 0, 1],
        "float": 0.1 + 0.2,
        "text": "a\x00IMG\x01x.png\x00 é 😀",
        "pair": (1, "two"),
        "set": frozenset({"italic", "bold"}),
        "plain_set": {3, 1, 2},
        ("tuple", "key"): [(1, 2), (3, 4)],
        "bytes": b"\xff\xd8\xff\xe0jpeg",
        "obj": obj,
    }
    model_dump.dump(tmp_path, value)
    back = model_dump.load(tmp_path)

    assert back.keys() == value.keys()
    for key in value:
        if key == "obj":
            continue
        assert back[key] == value[key], key
        assert type(back[key]) is type(value[key]), key
    assert [type(x) for x in back["flags"]] == [bool, bool, int, int]
    assert type(back[("tuple", "key")][0]) is tuple
    assert type(back["obj"]) is type(obj)
    assert vars(back["obj"]) == vars(obj)


def test_dump_refuses_code(model_dump, tmp_path):
    with pytest.raises(TypeError, match="code, not data"):
        model_dump.dump(tmp_path, {"f": len})


def test_dump_refuses_a_subclass_it_would_load_as_its_base(model_dump, tmp_path):
    with pytest.raises(TypeError, match="base type"):
        model_dump.dump(tmp_path, {"d": collections.defaultdict(list)})


def test_dump_writes_bytes_once_by_hash(model_dump, tmp_path):
    blob = b"same picture"
    model_dump.dump(tmp_path, {"a": blob, "b": blob})
    assert len(list((tmp_path / "resources").iterdir())) == 1


def test_dump_records_the_commit_from_the_environment(model_dump, tmp_path, monkeypatch):
    monkeypatch.setenv("KFXGEN_COMMIT", "abc1234")
    model_dump.dump(tmp_path, {})
    assert json.loads((tmp_path / "model.json").read_text())["kfxgen_commit"] == "abc1234"


def test_dump_names_kfxgen_classes_as_kfxgen_inside_calibre(model_dump, tmp_path, monkeypatch):
    # In calibre the package is calibre_plugins.kfxgen.kfxgen; a dump must load outside it.
    monkeypatch.setattr(model_dump, "_PACKAGE", "calibre_plugins.kfxgen.kfxgen")
    cls = type(
        "FontTable", (), {"__module__": "calibre_plugins.kfxgen.kfxgen.font_table"}
    )
    model_dump.dump(tmp_path, {"font_table": cls()})
    doc = json.loads((tmp_path / "model.json").read_text())
    assert doc["args"]["$dict"][0][1]["$obj"] == "kfxgen.font_table.FontTable"


def _model(tmp_path, args):
    (tmp_path / "resources").mkdir(exist_ok=True)
    (tmp_path / "model.json").write_text(json.dumps({"schema": 1, "args": args}))


# A model.json from somewhere else is data: loading it imports and builds only kfxgen's
# classes, and reads only the resources its hashes name (Deckle issue #14).
@pytest.mark.parametrize(
    "name",
    [
        "types.SimpleNamespace",
        "this.x",
        "kfxgenx.font_table.FontTable",
        # A class a kfxgen module only imports, and a function of kfxgen's.
        "kfxgen.model_dump.Path",
        "kfxgen.model_dump.load",
    ],
)
def test_load_refuses_a_class_outside_kfxgen(model_dump, tmp_path, name, monkeypatch):
    monkeypatch.delitem(sys.modules, "this", raising=False)
    _model(tmp_path, {"$obj": name, "state": {"$dict": []}})
    with pytest.raises(ValueError, match="not a kfxgen class"):
        model_dump.load(tmp_path)
    assert "this" not in sys.modules


def test_load_refuses_a_resource_that_is_not_a_hash(model_dump, tmp_path):
    (tmp_path / "secret.txt").write_text("secret")
    for name in ["../secret.txt", str(tmp_path / "secret.txt"), "abc"]:
        _model(tmp_path, {"$bytes": name})
        with pytest.raises(ValueError, match="not a sha256"):
            model_dump.load(tmp_path)


def test_load_refuses_a_resource_that_is_not_its_hash(model_dump, tmp_path):
    name = "0" * 64
    _model(tmp_path, {"$bytes": name})
    (tmp_path / "resources" / name).write_bytes(b"other bytes")
    with pytest.raises(ValueError, match="is not the bytes"):
        model_dump.load(tmp_path)
