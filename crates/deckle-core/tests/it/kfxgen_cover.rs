// SPDX-License-Identifier: GPL-3.0-or-later
//! The thumbnail step finds the cover in kfxgen's KFX (native KFX design, Goals 5).

#[test]
fn kfxgen_golden_cover_extracts() {
    let bytes = std::fs::read(deckle_kfx::fixtures::kfxgen("golden/with_cover.kfx")).unwrap();
    assert!(deckle_core::thumbnail::cover_jpeg_from_kfx(&bytes).is_some());
}
