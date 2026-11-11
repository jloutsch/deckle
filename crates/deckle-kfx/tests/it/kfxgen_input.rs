// SPDX-License-Identifier: GPL-3.0-or-later
//! kfxgen's KFX goes through our reader and writer unchanged (native KFX design, Risks:
//! container framing), and the text reader X-Ray and Word Wise use can read it.
use deckle_kfx::container::{WriteOptions, read_kfx, write_kfx};

#[test]
fn kfxgen_goldens_round_trip_byte_exact() {
    for name in [
        "golden/minimal.kfx",
        "golden/with_cover.kfx",
        "golden/body_images.kfx",
        "golden/table_cells.kfx",
    ] {
        let bytes = std::fs::read(deckle_kfx::fixtures::kfxgen(name)).unwrap();
        let doc = read_kfx(&bytes).unwrap();
        let out = write_kfx(&doc, &WriteOptions::from_doc(&doc)).unwrap();
        assert_eq!(out.len(), bytes.len(), "{name}: length");
        assert!(out == bytes, "{name}: bytes differ");
    }
}

#[test]
fn kfxgen_golden_has_content_runs() {
    let bytes = std::fs::read(deckle_kfx::fixtures::kfxgen("golden/minimal.kfx")).unwrap();
    let (runs, end) = deckle_kfx::text::content_runs(&bytes).unwrap();
    assert!(!runs.is_empty());
    assert!(end > 0);
}
