#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Rebuild fixtures/kfxgen/models: kfxgen's generator input (model.json, resources/) and what
# kfxgen makes from it (expected.kfx), the reference for crates/deckle-kfxgen (native KFX
# design, Oracles). With --cascade, rebuild fixtures/kfxgen/cascade instead: what Calibre
# hands kfxgen (book view, trees, styles, first-pass rules; tools/ref/kfxgen_hook/book_dump.py).
# With --convert, rebuild fixtures/kfxgen/convert: what kfxgen's converter makes on its way to
# the chapters, per source (tools/ref/kfxgen_hook/convert_dump.py), from runs that must give
# the committed models byte for byte; run it after the models.
#
#   tools/ref/kfxgen_models.sh                                   # the committed fixtures
#   tools/ref/kfxgen_models.sh --refresh-sources                 # ... from kfxgen's test_books
#   tools/ref/kfxgen_models.sh --cascade                         # the committed cascade dumps
#   tools/ref/kfxgen_models.sh --convert                         # the committed converter dumps
#   DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models \
#     tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub       # a corpus, outside git;
#                                                                # each book's cascade dump too
#   KFXGEN_ONLY=comments_in_tree tools/ref/kfxgen_models.sh     # one committed source only:
#                                                                # with no flag, --cascade or
#                                                                # --convert, only its folder is
#                                                                # made and replaced
#
# KFXGEN_ONLY makes no source and leaves every other folder as it is. Its run must have the
# kfxgen commit and tools (and, for --cascade, the month and user-agent rules) of the folder it
# goes into, or nothing is replaced.
#
# Needs a kfxgen repo (KFXGEN, default ../kfxgen-public; only read) with its .venv, and calibre.
# KFXGEN_REF is the kfxgen commit to take (default ad38ec8, the one the port targets); it must
# be ad38ec8 or later, since the port follows that commit. It is exported with git
# archive, and the dump hook (tools/ref/kfxgen_hook) is put into the export.
# Nothing is written in the kfxgen repo. Calibre runs with its own config folder holding only
# that export, so your installed plugins (and the Previewer-based "KFX Output") are neither
# used nor changed.
set -euo pipefail

ONLY="${KFXGEN_ONLY:-}"
# A source name only: it becomes a folder name under the fixture folders, which are replaced.
[[ "$ONLY" =~ ^[A-Za-z0-9_]*$ ]] || { echo "KFXGEN_ONLY must be a source name: '$ONLY'" >&2; exit 2; }
[ -z "$ONLY" ] || [ "${1:-}" != --corpus ] || { echo "--corpus does not take KFXGEN_ONLY" >&2; exit 2; }
[ -z "$ONLY" ] || [ "${1:-}" != --refresh-sources ] || { echo "--refresh-sources does not take KFXGEN_ONLY" >&2; exit 2; }

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KFXGEN="$(cd "${KFXGEN:-$ROOT/../kfxgen-public}" && pwd)"
# The commit the port targets, and the oldest the script takes.
KFXGEN_MIN=ad38ec8f192126f8cf2b9b61814c6dabe3d12c52
KFXGEN_REF="${KFXGEN_REF:-$KFXGEN_MIN}"
PY="${KFXGEN_PYTHON:-$KFXGEN/.venv/bin/python}"
EBOOK_CONVERT="${EBOOK_CONVERT:-ebook-convert}"
CALIBRE_CUSTOMIZE="${CALIBRE_CUSTOMIZE:-calibre-customize}"
OUT="$ROOT/fixtures/kfxgen/models"
CASCADE="$ROOT/fixtures/kfxgen/cascade"
CONVERT="$ROOT/fixtures/kfxgen/convert"
SOURCES="$ROOT/fixtures/kfxgen/sources"
HOOK="$ROOT/tools/ref/kfxgen_hook"

# One commit, whatever KFXGEN_REF names, and one that holds KFXGEN_MIN: a branch can be older.
KFXGEN_FULL="$(git -C "$KFXGEN" rev-parse --verify --end-of-options "$KFXGEN_REF^{commit}")"
git -C "$KFXGEN" merge-base --is-ancestor "$KFXGEN_MIN" "$KFXGEN_FULL" || {
  echo "KFXGEN_REF $KFXGEN_REF ($KFXGEN_FULL) does not contain kfxgen $KFXGEN_MIN" >&2
  exit 1
}
KFXGEN_COMMIT="$(git -C "$KFXGEN" rev-parse --short=7 "$KFXGEN_FULL")"
export KFXGEN_COMMIT
WORK="$(mktemp -d -t deckle-kfxgen-XXXXXX)"
# The models are built beside OUT and take its place only when every one is made: a run
# that fails leaves the committed fixtures as they were.
BUILD="$OUT.new"
CASCADE_BUILD="$CASCADE.new"
CONVERT_BUILD="$CONVERT.new"
trap 'rm -rf "$WORK" "$BUILD" "$CASCADE_BUILD" "$CONVERT_BUILD"' EXIT

# The export, with the hook.
TREE="$WORK/kfxgen"
mkdir -p "$TREE"
git -C "$KFXGEN" archive "$KFXGEN_FULL" | tar -x -C "$TREE"
"$PY" "$HOOK/inject.py" "$TREE"

export CALIBRE_CONFIG_DIRECTORY="$WORK/calibre"
mkdir -p "$WORK/plugin"
cp -R "$TREE/plugin/." "$WORK/plugin/"
find "$WORK/plugin" \( -name __pycache__ -o -name '*.pyc' -o -name .DS_Store \) -prune -exec rm -rf {} +
(cd "$WORK/plugin" && zip -rq "$WORK/kfxgen.zip" .)
"$CALIBRE_CUSTOMIZE" -a "$WORK/kfxgen.zip" >/dev/null

# calibre_model NAME EPUB DIR [BOOK_DIR]: dump NAME through ebook-convert into DIR/NAME,
# replay it, and check the replay is ebook-convert's own output. With BOOK_DIR, the same run
# also writes the cascade dump there.
calibre_model() {
  local name="$1" epub="$2" dir="$3/$1" book="${4:-}"
  rm -rf "$dir"
  # A run that also dumps the cascade is in UTC, as --cascade's (calibre_book) are: calibre
  # turns a date with no time into midnight local time, written in UTC.
  local zone=()
  [ -z "$book" ] || zone=(TZ=UTC)
  if ! env ${zone[@]+"${zone[@]}"} KFXGEN_DUMP_MODEL="$dir" KFXGEN_DUMP_BOOK="$book" \
    "$EBOOK_CONVERT" "$epub" "$WORK/$name.kfx" >"$WORK/$name.log" 2>&1; then
    tail -20 "$WORK/$name.log" >&2
    echo "$name: ebook-convert failed" >&2
    exit 1
  fi
  [ -f "$dir/model.json" ] || {
    echo "$name: no model.json, so calibre did not run kfxgen" >&2
    exit 1
  }
  [ -z "$book" ] || [ -f "$book/oeb.json" ] || {
    echo "$name: no cascade oeb.json, so the book dump did not finish" >&2
    exit 1
  }
  "$PY" "$HOOK/generate_from_model.py" "$TREE" "$dir" "$dir/expected.kfx" >/dev/null
  cmp -s "$dir/expected.kfx" "$WORK/$name.kfx" || {
    echo "$name: the replayed model makes different bytes from ebook-convert" >&2
    exit 1
  }
  echo "calibre $name: $(wc -c <"$dir/expected.kfx" | tr -d ' ') bytes"
}

# record_tools DIR: the kfxgen commit and the tools that made DIR. Calibre's CSS handling
# shapes a Calibre-path model.json, so another Calibre can make other fixtures.
record_tools() {
  echo "$KFXGEN_COMMIT" >"$1/KFXGEN_COMMIT"
  {
    echo "calibre: $("$EBOOK_CONVERT" --version | head -1)"
    echo "kfxgen venv python: $("$PY" -c 'import platform; print(platform.python_version())')"
    echo "kfxgen venv lxml: $("$PY" -c 'import lxml.etree; print(lxml.etree.__version__)')"
  } >"$1/TOOLS"
}

# replace_only BUILD DEST: with KFXGEN_ONLY, DEST/$ONLY replaced by BUILD/$ONLY, once BUILD's
# KFXGEN_COMMIT and TOOLS (written by record_tools) are DEST's.
replace_only() {
  local f
  for f in KFXGEN_COMMIT TOOLS; do
    cmp -s "$1/$f" "$2/$f" || {
      echo "$ONLY: this run's $f is not the one in $2:" >&2
      diff "$1/$f" "$2/$f" >&2 || true
      exit 1
    }
  done
  rm -rf "${2:?}/$ONLY"
  mv "$1/$ONLY" "$2/$ONLY"
  du -sh "$2/$ONLY" "$ROOT/fixtures/kfxgen"
}

# record_cascade_run DIR: the time zone and month of a run that dumped cascades, after
# record_tools. A year-only dc:date takes day 15 of this month
# (calibre.utils.date.parse_only_date).
record_cascade_run() {
  {
    echo "TZ: UTC"
    echo "month of the run: $(TZ=UTC date +%Y-%m)"
  } >>"$1/TOOLS"
}

# gzip_models DIR: each DIR/*/model.json stored compressed, with no name or time in the header,
# so two runs give the same bytes; deckle_kfxgen::load reads either form. Corpus models
# (--corpus) stay plain.
gzip_models() {
  "$PY" -I -c '
import gzip, sys
from pathlib import Path
for p in sorted(Path(sys.argv[1]).glob("*/model.json")):
    p.with_name("model.json.gz").write_bytes(gzip.compress(p.read_bytes(), 9, mtime=0))
    p.unlink()
' "$1"
}

if [ "${1:-}" = "--corpus" ]; then
  shift
  : "${DECKLE_KFXGEN_CORPUS_OUT:?set DECKLE_KFXGEN_CORPUS_OUT to the folder for the models}"
  mkdir -p "$DECKLE_KFXGEN_CORPUS_OUT"
  # Checked before any model is touched: an empty name's folder is the whole corpus.
  for epub in "$@"; do
    [ -f "$epub" ] && [ -n "$(basename "$epub" .epub)" ] || {
      echo "--corpus: not an EPUB file: '$epub'" >&2
      exit 1
    }
  done
  # A run that fails leaves no BOOKS and no KFXGEN_COMMIT, so the corpus test stops on what
  # it left; BOOKS lists the books of a run that made them all.
  rm -f "$DECKLE_KFXGEN_CORPUS_OUT"/{BOOKS,KFXGEN_COMMIT,TOOLS}
  for epub in "$@"; do
    name="$(basename "$epub" .epub)"
    calibre_model "$name" "$epub" "$DECKLE_KFXGEN_CORPUS_OUT" "$DECKLE_KFXGEN_CORPUS_OUT/$name/cascade"
  done
  for epub in "$@"; do basename "$epub" .epub; done >"$DECKLE_KFXGEN_CORPUS_OUT/BOOKS"
  record_tools "$DECKLE_KFXGEN_CORPUS_OUT"
  record_cascade_run "$DECKLE_KFXGEN_CORPUS_OUT"
  exit 0
fi

# calibre_book NAME EPUB DIR: the cascade dump of NAME into DIR/NAME. Nothing else is kept.
# In UTC: calibre turns a date with no time into midnight local time, written in UTC.
calibre_book() {
  local name="$1" epub="$2" dir="$3/$1"
  rm -rf "$dir"
  if ! TZ=UTC KFXGEN_DUMP_BOOK="$dir" "$EBOOK_CONVERT" "$epub" "$WORK/$name.kfx" >"$WORK/$name.log" 2>&1; then
    tail -20 "$WORK/$name.log" >&2
    echo "$name: ebook-convert failed" >&2
    exit 1
  fi
  [ -f "$dir/oeb.json" ] || {
    echo "$name: no oeb.json, so calibre did not run kfxgen or the dump did not finish" >&2
    exit 1
  }
  echo "cascade $name: $(find "$dir" -type f | wc -l | tr -d ' ') files"
}

# The cascade fixtures (fixtures/kfxgen/README.md, cascade/): the sources built here, and
# committed sources that between them hold lists, tables, pictures, notes, vertical and
# right-to-left text, an embedded font, and every data-kfxgen-* mark kfxgen's plugin makes
# before the flattener (link colours, horizontal blocks, toc and front matter classes).
# Kept under 1.5 MB.
CASCADE_BUILT="css_quirks package_quirks no_toc fake_margins malformed comments_in_tree"
CASCADE_BUILT="$CASCADE_BUILT refused_selector"
CASCADE_SOURCES="$CASCADE_BUILT lists notes vertical rtl fonts directions frames navigation"
CASCADE_SOURCES="$CASCADE_SOURCES link_colours link_greys horizontal_blocks marks package_more"

if [ "${1:-}" = "--cascade" ] && [ -n "$ONLY" ]; then
  case " $CASCADE_SOURCES " in
    *" $ONLY "*) ;;
    *) echo "$ONLY is not in CASCADE_SOURCES" >&2; exit 1 ;;
  esac
  rm -rf "$CASCADE_BUILD"
  mkdir -p "$CASCADE_BUILD"
  calibre_book "$ONLY" "$SOURCES/$ONLY.epub" "$CASCADE_BUILD"
  cmp -s "$CASCADE/ua_rules.json" "$CASCADE_BUILD/$ONLY/ua_rules.json" || {
    echo "$ONLY: other user-agent rules than cascade/ua_rules.json" >&2
    exit 1
  }
  rm "$CASCADE_BUILD/$ONLY/ua_rules.json"
  record_tools "$CASCADE_BUILD"
  record_cascade_run "$CASCADE_BUILD"
  replace_only "$CASCADE_BUILD" "$CASCADE"
  exit 0
fi

if [ "${1:-}" = "--cascade" ]; then
  rm -rf "$CASCADE_BUILD"
  mkdir -p "$CASCADE_BUILD"
  "$PY" -I "$ROOT/tools/ref/make_css_quirks_epub.py" "$SOURCES/css_quirks.epub"
  "$PY" -I "$ROOT/tools/ref/make_cascade_epubs.py" "$SOURCES"
  for name in $CASCADE_SOURCES; do
    calibre_book "$name" "$SOURCES/$name.epub" "$CASCADE_BUILD"
  done
  # Calibre's user-agent rules, once: every book's must be the same.
  first="${CASCADE_SOURCES%% *}"
  for name in $CASCADE_SOURCES; do
    cmp -s "$CASCADE_BUILD/$first/ua_rules.json" "$CASCADE_BUILD/$name/ua_rules.json" || {
      echo "$name: other user-agent rules than $first" >&2
      exit 1
    }
  done
  mv "$CASCADE_BUILD/$first/ua_rules.json" "$CASCADE_BUILD/ua_rules.json"
  for name in $CASCADE_SOURCES; do rm -f "$CASCADE_BUILD/$name/ua_rules.json"; done
  record_tools "$CASCADE_BUILD"
  record_cascade_run "$CASCADE_BUILD"
  rm -rf "$CASCADE"
  mv "$CASCADE_BUILD" "$CASCADE"
  du -sh "$CASCADE"
  exit 0
fi

# The converter dumps (fixtures/kfxgen/README.md, convert/): every source, dumped in a run
# whose model.json and KFX must be the committed model's and expected.kfx, so the dumps
# change nothing; and the unit table of inline_style.py and font_table.py.
if [ "${1:-}" = "--convert" ]; then
  rm -rf "$CONVERT_BUILD"
  mkdir -p "$CONVERT_BUILD/units"
  "$PY" -I "$HOOK/convert_dump.py" units "$TREE" "$CONVERT_BUILD/units/inline_style.json"
  epubs=("$SOURCES"/*.epub)
  [ -z "$ONLY" ] || epubs=("$SOURCES/$ONLY.epub")
  for epub in "${epubs[@]}"; do
    name="$(basename "$epub" .epub)"
    model="$OUT/$name"
    [ -f "$model/model.json.gz" ] || {
      echo "$name: no models/$name: run tools/ref/kfxgen_models.sh first" >&2
      exit 1
    }
    if ! KFXGEN_DUMP_MODEL="$WORK/$name.model" KFXGEN_DUMP_CONVERT="$CONVERT_BUILD/$name" \
      "$EBOOK_CONVERT" "$epub" "$WORK/$name.kfx" >"$WORK/$name.log" 2>&1; then
      tail -20 "$WORK/$name.log" >&2
      echo "$name: ebook-convert failed" >&2
      exit 1
    fi
    [ -f "$CONVERT_BUILD/$name/assembly.json.gz" ] || {
      echo "$name: no assembly.json.gz, so the dump did not finish" >&2
      exit 1
    }
    # Byte for byte, but for the order of the images argument: calibre lists the manifest's
    # pictures in another order from run to run, with or without the dumps.
    "$PY" -I -c '
import gzip, json, sys
ours = gzip.decompress(open(sys.argv[1], "rb").read())
dumped = open(sys.argv[2], "rb").read()
if ours == dumped:
    sys.exit(0)
def images_sorted(text):
    doc = json.loads(text, parse_int=lambda s: ("int", s), parse_float=lambda s: ("float", s))
    for pair in doc["args"]["$dict"]:
        if pair[0] == "images":
            pair[1]["$dict"].sort(key=lambda p: p[0])
    return doc
same = images_sorted(ours) == images_sorted(dumped)
if same:
    print(f"{sys.argv[3]}: the same model.json but for the order of its images")
sys.exit(not same)
' "$model/model.json.gz" "$WORK/$name.model/model.json" "$name" || {
      echo "$name: the run with the dumps made another model.json than models/$name" >&2
      exit 1
    }
    cmp -s "$model/expected.kfx" "$WORK/$name.kfx" || {
      echo "$name: the run with the dumps made another KFX than models/$name" >&2
      exit 1
    }
    echo "convert $name: $(du -sk "$CONVERT_BUILD/$name" | cut -f1) KB"
  done
  record_tools "$CONVERT_BUILD"
  if [ -n "$ONLY" ]; then
    cmp -s "$CONVERT_BUILD/units/inline_style.json" "$CONVERT/units/inline_style.json" || {
      echo "$ONLY: this run's units/inline_style.json is not the committed one" >&2
      exit 1
    }
    replace_only "$CONVERT_BUILD" "$CONVERT"
    exit 0
  fi
  rm -rf "$CONVERT"
  mv "$CONVERT_BUILD" "$CONVERT"
  du -sh "$CONVERT" "$ROOT/fixtures/kfxgen"
  exit 0
fi

REFRESH=
case "${1:-}" in
  "") ;;
  --refresh-sources) REFRESH=1 ;;
  *) echo "unknown argument: $1" >&2; exit 2 ;;
esac

rm -rf "$BUILD"
mkdir -p "$BUILD" "$SOURCES"

if [ -n "$ONLY" ]; then
  [ -f "$SOURCES/$ONLY.epub" ] || { echo "$ONLY: no sources/$ONLY.epub" >&2; exit 1; }
  calibre_model "$ONLY" "$SOURCES/$ONLY.epub" "$BUILD"
  record_tools "$BUILD"
  gzip_models "$BUILD"
  replace_only "$BUILD" "$OUT"
  exit 0
fi

"$PY" "$ROOT/tools/ref/kfxgen_models.py" shim "$TREE" "$BUILD"
"$PY" "$ROOT/tools/ref/kfxgen_models.py" args "$TREE" "$BUILD"

# Calibre-path fixtures, one per feature (fixtures/kfxgen/README.md). Their sources are the
# committed sources/*.epub. kfxgen's test_books/ is untracked in places, so no commit pins
# it: --refresh-sources copies from the kfxgen working tree, and says which sources changed.
"$PY" "$ROOT/tools/ref/kfxgen_models.py" synth "$SOURCES" >/dev/null
"$PY" -I "$ROOT/tools/ref/make_convert_epubs.py" "$SOURCES"
while IFS='|' read -r name src; do
  if [ -n "$REFRESH" ] && [ -f "$KFXGEN/test_books/$src" ]; then
    cmp -s "$KFXGEN/test_books/$src" "$SOURCES/$name.epub" || {
      cp "$KFXGEN/test_books/$src" "$SOURCES/$name.epub"
      echo "source $name: refreshed from test_books/$src"
    }
  fi
  [ -f "$SOURCES/$name.epub" ] || { echo "$name: no source (test_books/$src)" >&2; exit 1; }
done <<'LIST'
boxes|boxes/boxes-source.epub
rules_spaces|rules_722/r722-kfxgen.epub
spacers|spacers_667/S667 kfxgen.epub
notes|popup_notes/n445b-kfxgen.epub
vertical|vertical_247/s746k.epub
ruby_tcy|tcy_emphasis_748/s748k.epub
page_list|landmarks_pages/l579-main.epub
links|link_runs_726/s726k.epub
link_greys|link_greys_741/s741k.epub
navigation|navigation/nav-kfxgen.epub
typography|typography/typography-new-source.epub
colour|colour/colour-new-source.epub
link_colours|link_colours_732/s732k.epub
dark_boxes|dark_box_colours_734/c734k.epub
drop_caps|drop_caps/drop-caps-new-source.epub
figures|figures/figures-new-2-source.epub
floats|floats/floats-fix-source.epub
floats_697|floats_697/F697 kfxgen source.epub
font_sizes|font_sizes/font-sizes-source.epub
horizontal_blocks_749|horizontal_blocks_749/s749k.epub
vertical_pictures_761|vertical_pictures_761/s761k.epub
vertical_in_horizontal_763|vertical_in_horizontal_763/s763k.epub
lists_horizontal_items_764|lists_horizontal_items_764/s764k.epub
turned_words_765|turned_words_765/s765k.epub
pictures_side_by_side_766|pictures_side_by_side_766/s766k.epub
lists_768|lists_768/s768m.epub
block_picture_769|block_picture_769/s769k.epub
block_picture_770|block_picture_770/s770k.epub
lists_771|lists_771/s771k.epub
chapter_starts_773|chapter_starts_773/t773k.epub
chapter_starts_777|chapter_starts_777/t777k.epub
LIST
for epub in "$SOURCES"/*.epub; do
  calibre_model "$(basename "$epub" .epub)" "$epub" "$BUILD"
done
record_tools "$BUILD"
gzip_models "$BUILD"
rm -rf "$OUT"
mv "$BUILD" "$OUT"
du -sh "$ROOT/fixtures/kfxgen"
