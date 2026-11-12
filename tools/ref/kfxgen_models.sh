#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Rebuild fixtures/kfxgen/models: kfxgen's generator input (model.json, resources/) and what
# kfxgen makes from it (expected.kfx), the reference for crates/deckle-kfxgen (native KFX
# design, Oracles).
#
#   tools/ref/kfxgen_models.sh                                   # the committed fixtures
#   tools/ref/kfxgen_models.sh --refresh-sources                 # ... from kfxgen's test_books
#   DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models \
#     tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub       # a corpus, outside git
#
# Needs a kfxgen repo (KFXGEN, default ../kfxgen-public; only read) with its .venv, and calibre.
# KFXGEN_REF is the kfxgen commit to take (default 0bc705d, the one the port targets); it must
# be 0bc705d or later, since the replay imports kfxgen's page_scale. It is exported with git
# archive, and the dump hook (tools/ref/kfxgen_hook) is put into the export.
# Nothing is written in the kfxgen repo. Calibre runs with its own config folder holding only
# that export, so your installed plugins (and the Previewer-based "KFX Output") are neither
# used nor changed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KFXGEN="$(cd "${KFXGEN:-$ROOT/../kfxgen-public}" && pwd)"
# The commit the port targets, and the oldest the replay works with.
KFXGEN_MIN=0bc705d3ec9489417db5112ef8b40e57da938bed
KFXGEN_REF="${KFXGEN_REF:-$KFXGEN_MIN}"
PY="${KFXGEN_PYTHON:-$KFXGEN/.venv/bin/python}"
EBOOK_CONVERT="${EBOOK_CONVERT:-ebook-convert}"
CALIBRE_CUSTOMIZE="${CALIBRE_CUSTOMIZE:-calibre-customize}"
OUT="$ROOT/fixtures/kfxgen/models"
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
trap 'rm -rf "$WORK" "$BUILD"' EXIT

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

# calibre_model NAME EPUB DIR: dump NAME through ebook-convert into DIR/NAME, replay it, and
# check the replay is ebook-convert's own output.
calibre_model() {
  local name="$1" epub="$2" dir="$3/$1"
  rm -rf "$dir"
  if ! KFXGEN_DUMP_MODEL="$dir" "$EBOOK_CONVERT" "$epub" "$WORK/$name.kfx" >"$WORK/$name.log" 2>&1; then
    tail -20 "$WORK/$name.log" >&2
    echo "$name: ebook-convert failed" >&2
    exit 1
  fi
  [ -f "$dir/model.json" ] || {
    echo "$name: no model.json, so calibre did not run kfxgen" >&2
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
    calibre_model "$(basename "$epub" .epub)" "$epub" "$DECKLE_KFXGEN_CORPUS_OUT"
  done
  for epub in "$@"; do basename "$epub" .epub; done >"$DECKLE_KFXGEN_CORPUS_OUT/BOOKS"
  record_tools "$DECKLE_KFXGEN_CORPUS_OUT"
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
"$PY" "$ROOT/tools/ref/kfxgen_models.py" shim "$TREE" "$BUILD"
"$PY" "$ROOT/tools/ref/kfxgen_models.py" args "$TREE" "$BUILD"

# Calibre-path fixtures, one per feature (fixtures/kfxgen/README.md). Their sources are the
# committed sources/*.epub. kfxgen's test_books/ is untracked in places, so no commit pins
# it: --refresh-sources copies from the kfxgen working tree, and says which sources changed.
"$PY" "$ROOT/tools/ref/kfxgen_models.py" synth "$SOURCES" >/dev/null
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
LIST
for epub in "$SOURCES"/*.epub; do
  calibre_model "$(basename "$epub" .epub)" "$epub" "$BUILD"
done
record_tools "$BUILD"
rm -rf "$OUT"
mv "$BUILD" "$OUT"
du -sh "$ROOT/fixtures/kfxgen"
