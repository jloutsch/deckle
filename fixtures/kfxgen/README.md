# kfxgen fixtures

Output of [kfxgen](https://github.com/jloutsch/kfxgen) (GPL-3), the reference for the native
converter (`docs/superpowers/specs/2026-11-10-native-kfx-design.md`, Oracles). Every file is
from kfxgen commit `4b81651` (5.24.0 plus #753).

## golden/

Copied unchanged from kfxgen's `tests/fixtures/golden/expected/`. kfxgen builds them from
synthetic EPUBs (`tests/fixtures/golden/inputs.py`) without Calibre, through
`tests/fixtures/oeb_shim.py`, so they carry no per-element CSS.

| File | sha256 (first 16) |
|---|---|
| `minimal.kfx` | `73fbd3c30597b43e` |
| `with_cover.kfx` | `2694e310ff91d9b7` |
| `body_images.kfx` | `7353137416ae4ad0` |
| `table_cells.kfx` | `2b646ac746a8baf9` |

Regenerate: `git -C <kfxgen> show 4b81651:tests/fixtures/golden/expected/<name>.kfx > golden/<name>.kfx`.

**Found (phase 1, task 1):** `read_kfx` then `write_kfx` with `WriteOptions::from_doc`
reproduces all four byte for byte. kfxgen's container framing is the same as kfxlib's, so the
native generator can write through `deckle_kfx::container::write_kfx` with no changes to it.
