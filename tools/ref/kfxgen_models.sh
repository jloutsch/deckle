#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Rebuild fixtures/kfxgen/models: kfxgen's generator input (model.json, resources/) and what
# kfxgen makes from it (expected.kfx), the reference for crates/deckle-kfxgen (native KFX
# design, Oracles).
#
#   tools/ref/kfxgen_models.sh                                   # the committed fixtures
#   DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models \
#     tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub       # a corpus, outside git
#
# Needs kfxgen with model_dump.py (KFXGEN, default ../kfxgen-public), its .venv, and calibre.
# For an exported kfxgen (no .git), set KFXGEN_COMMIT to the commit it was exported from.
# Calibre runs with its own config folder holding only this kfxgen, so your installed plugins
# (and the Previewer-based "KFX Output") are neither used nor changed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KFXGEN="$(cd "${KFXGEN:-$ROOT/../kfxgen-public}" && pwd)"
PY="${KFXGEN_PYTHON:-$KFXGEN/.venv/bin/python}"
EBOOK_CONVERT="${EBOOK_CONVERT:-ebook-convert}"
CALIBRE_CUSTOMIZE="${CALIBRE_CUSTOMIZE:-calibre-customize}"
OUT="$ROOT/fixtures/kfxgen/models"
SOURCES="$ROOT/fixtures/kfxgen/sources"

[ -f "$KFXGEN/plugin/kfxgen/model_dump.py" ] || {
  echo "kfxgen at $KFXGEN has no plugin/kfxgen/model_dump.py (it is on kfxgen's feat/model-dump)" >&2
  exit 1
}
# KFXGEN_COMMIT names the commit when KFXGEN is an export with no .git (git archive).
KFXGEN_COMMIT="${KFXGEN_COMMIT:-$(git -C "$KFXGEN" rev-parse --short HEAD)}"
export KFXGEN_COMMIT
WORK="$(mktemp -d -t deckle-kfxgen-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

export CALIBRE_CONFIG_DIRECTORY="$WORK/calibre"
mkdir -p "$WORK/plugin"
cp -R "$KFXGEN/plugin/." "$WORK/plugin/"
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
  "$PY" "$KFXGEN/research/generate_from_model.py" "$dir" "$dir/expected.kfx" >/dev/null
  cmp -s "$dir/expected.kfx" "$WORK/$name.kfx" || {
    echo "$name: the replayed model makes different bytes from ebook-convert" >&2
    exit 1
  }
  echo "calibre $name: $(wc -c <"$dir/expected.kfx" | tr -d ' ') bytes"
}

if [ "${1:-}" = "--corpus" ]; then
  shift
  : "${DECKLE_KFXGEN_CORPUS_OUT:?set DECKLE_KFXGEN_CORPUS_OUT to the folder for the models}"
  mkdir -p "$DECKLE_KFXGEN_CORPUS_OUT"
  for epub in "$@"; do
    calibre_model "$(basename "$epub" .epub)" "$epub" "$DECKLE_KFXGEN_CORPUS_OUT"
  done
  echo "$KFXGEN_COMMIT" >"$DECKLE_KFXGEN_CORPUS_OUT/KFXGEN_COMMIT"
  exit 0
fi

rm -rf "$OUT"
mkdir -p "$OUT" "$SOURCES"
"$PY" "$ROOT/tools/ref/kfxgen_models.py" shim "$KFXGEN" "$OUT"
"$PY" "$ROOT/tools/ref/kfxgen_models.py" args "$KFXGEN" "$OUT"

# Calibre-path fixtures, one per feature (fixtures/kfxgen/README.md). Sources from kfxgen's
# test_books/ are copied in when that folder has them; otherwise the committed copy is used.
"$PY" "$ROOT/tools/ref/kfxgen_models.py" synth "$SOURCES" >/dev/null
while IFS='|' read -r name src; do
  [ -f "$KFXGEN/test_books/$src" ] && cp "$KFXGEN/test_books/$src" "$SOURCES/$name.epub"
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
floats|floats/floats-fix-source.epub
floats_697|floats_697/F697 kfxgen source.epub
font_sizes|font_sizes/font-sizes-source.epub
LIST
for epub in "$SOURCES"/*.epub; do
  calibre_model "$(basename "$epub" .epub)" "$epub" "$OUT"
done
echo "$KFXGEN_COMMIT" >"$OUT/KFXGEN_COMMIT"
du -sh "$ROOT/fixtures/kfxgen"
