# SPDX-License-Identifier: GPL-3.0-only
"""convert_dump's wrappers leave kfxgen's conversion as it was.

Run with kfxgen's Python (lxml, Pillow), from anywhere:

    KFXGEN=<kfxgen repo> <kfxgen>/.venv/bin/python tools/ref/kfxgen_hook/test_convert_dump.py

The test exports kfxgen (``KFXGEN``, default ``../kfxgen-public`` beside this repository;
commit ``KFXGEN_REF``, default ad38ec8; only read, with ``git archive``), puts the hooks in with
``inject.py``, and converts every ``fixtures/kfxgen/sources/*.epub`` through kfxgen's
Calibre-free ``EpubAsOeb``, given the unpacked book as its container folder: once with
converter's own functions, once with each wrapper alone, and once with all of them. Each run's
``model.json`` and KFX must be the bytes of the first; two runs with all of them must write the
same dumps. Without Calibre there is no Stylizer, so ``_build_style_resolver`` gives None and
the D1 dump is empty here: ``tools/ref/kfxgen_models.sh --convert`` checks the Calibre path,
D1 included, against the committed models.
"""

import hashlib
import itertools
import os
import subprocess
import sys
import tempfile
import types
import unittest
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SOURCES = ROOT / "fixtures" / "kfxgen" / "sources"


def export_kfxgen(tmp):
    repo = Path(os.environ.get("KFXGEN", ROOT.parent / "kfxgen-public"))
    ref = os.environ.get("KFXGEN_REF", "ad38ec8f192126f8cf2b9b61814c6dabe3d12c52")
    tree = Path(tmp) / "kfxgen"
    tree.mkdir()
    archive = subprocess.run(
        ["git", "-C", str(repo), "archive", ref], check=True, capture_output=True
    ).stdout
    subprocess.run(["tar", "-x", "-C", str(tree)], input=archive, check=True)
    subprocess.run([sys.executable, str(HERE / "inject.py"), str(tree)], check=True)
    return tree


def unpack(epub, folder):
    """The book unpacked into ``folder``, and its OPF's path there."""
    with zipfile.ZipFile(epub) as z:
        z.extractall(folder)
        container = ET.fromstring(z.read("META-INF/container.xml"))
    rootfile = next(e for e in container.iter() if e.tag.endswith("rootfile"))
    return rootfile.get("full-path")


def tree_digest(directory):
    """{relative path: sha256} of every file under ``directory``."""
    return {
        str(p.relative_to(directory)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(Path(directory).rglob("*"))
        if p.is_file()
    }


class ConvertDumpTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        tree = export_kfxgen(cls.tmp.name)
        sys.path[:0] = [str(tree / "plugin"), str(tree)]
        from kfxgen import converter, convert_dump
        from tests._helpers import NullLog
        from tests.fixtures.oeb_shim import EpubAsOeb

        cls.converter, cls.dump, cls.NullLog, cls.EpubAsOeb = (
            converter,
            convert_dump,
            NullLog,
            EpubAsOeb,
        )
        cls.originals = {n: getattr(converter, n) for n in convert_dump.WRAPPED}
        os.environ[convert_dump.ENV] = cls.tmp.name
        convert_dump.install(None)  # puts the wrappers in, once for the process
        del os.environ[convert_dump.ENV]
        cls.wrappers = {n: getattr(converter, n) for n in convert_dump.WRAPPED}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def use(self, names):
        """converter's names: the wrappers of ``names``, its own functions for the rest."""
        for n in self.dump.WRAPPED:
            setattr(self.converter, n, (self.wrappers if n in names else self.originals)[n])

    def convert(self, epub, work, dump_dir=None):
        """(model.json bytes, KFX bytes) of one conversion in ``work``, dumping to
        ``dump_dir`` when given."""
        work.mkdir()
        book = work / "book"
        opf = unpack(epub, book)
        oeb = self.EpubAsOeb(epub)
        oeb.container = types.SimpleNamespace(rootdir=str(book), opfname=opf)
        env = {"KFXGEN_DUMP_MODEL": str(work / "model")}
        if dump_dir is not None:
            env[self.dump.ENV] = str(dump_dir)
        # Run serials (ruby, tcy, bidi...) count from 0 in each calibre conversion, a process
        # of its own; here every conversion shares one process.
        for module in list(sys.modules.values()):
            if (getattr(module, "__name__", "") or "").startswith("kfxgen."):
                for name, value in list(vars(module).items()):
                    if isinstance(value, itertools.count):
                        setattr(module, name, itertools.count())
        os.environ.update(env)
        try:
            self.converter.convert_oeb_to_kfx(
                oeb, str(work / "out.kfx"), opts=None, log=self.NullLog()
            )
        finally:
            for k in env:
                del os.environ[k]
        return (work / "model" / "model.json").read_bytes(), (work / "out.kfx").read_bytes()

    def test_every_wrapper_leaves_the_model_and_the_kfx_as_they_were(self):
        sources = sorted(SOURCES.glob("*.epub"))
        self.assertTrue(sources)
        converted = 0
        for epub in sources:
            with self.subTest(epub.stem), tempfile.TemporaryDirectory() as tmp:
                tmp = Path(tmp)
                self.use(())
                try:
                    plain = self.convert(epub, tmp / "plain")
                except Exception as e:  # the shim reads less than calibre: not every book
                    print(f"{epub.stem}: kfxgen's shim cannot convert it ({e!r})")
                    continue
                converted += 1
                for name in self.dump.WRAPPED:
                    self.use((name,))
                    got = self.convert(epub, tmp / name, tmp / f"{name}.dump")
                    self.assertEqual(got[0], plain[0], f"{name}: model.json changed")
                    self.assertEqual(got[1], plain[1], f"{name}: the KFX changed")
                self.use(self.dump.WRAPPED)
                runs = []
                for n in (1, 2):
                    got = self.convert(epub, tmp / f"all{n}", tmp / f"all{n}.dump")
                    self.assertEqual(got, plain, "all the wrappers changed the conversion")
                    runs.append(tree_digest(tmp / f"all{n}.dump"))
                self.assertEqual(runs[0], runs[1], "two runs wrote other dumps")
                for part in ("files.json", "assembly.json.gz", "calibre_opf.xml"):
                    self.assertIn(part, runs[0])
                self.assertTrue(any(p.startswith("blocks/") for p in runs[0]))
                self.assertTrue(any(p.startswith("inline/") for p in runs[0]))
        self.use(self.dump.WRAPPED)
        print(f"{converted} of {len(sources)} sources converted through the shim")
        self.assertGreater(converted, len(sources) // 2)

    def test_nothing_is_written_without_the_variable(self):
        epub = SOURCES / "lists.epub"
        self.use(self.dump.WRAPPED)
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            self.convert(epub, tmp / "run")
            self.assertEqual(sorted(p.name for p in tmp.iterdir()), ["run"])
            self.assertIsNone(self.dump._state)


if __name__ == "__main__":
    unittest.main()
