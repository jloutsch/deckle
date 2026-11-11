// SPDX-License-Identifier: GPL-3.0-or-later
//! Pictures and the cover against kfxgen's KFX (phase 1 plan, Task 7).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

#[test]
fn body_images_matches_kfxgen() {
    matches_kfxgen("body_images");
}

#[test]
fn captioned_images_matches_kfxgen() {
    matches_kfxgen("captioned_images");
}

#[test]
fn with_cover_matches_kfxgen() {
    matches_kfxgen("with_cover");
}

#[test]
fn unused_manifest_image_matches_kfxgen() {
    matches_kfxgen("unused_manifest_image");
}

#[test]
fn contents_illustration_matches_kfxgen() {
    matches_kfxgen("contents_illustration");
}

#[test]
fn image_only_chapter_matches_kfxgen() {
    matches_kfxgen("image_only_chapter");
}

/// Hrefs, sizes, floats, ids and JPEGs no EPUB-made fixture reaches.
#[test]
fn image_refs_matches_kfxgen() {
    matches_kfxgen("image_refs");
}

#[test]
fn image_refs_unresolved_matches_kfxgen() {
    matches_kfxgen("image_refs_unresolved");
}

#[test]
fn cover_paths_matches_kfxgen() {
    matches_kfxgen("cover_paths");
}

#[test]
fn cover_page_matches_kfxgen() {
    matches_kfxgen("cover_page");
}

/// A cover picture with only blank blocks after it on its page stays there (#524).
#[test]
fn cover_alone_matches_kfxgen() {
    matches_kfxgen("cover_alone");
}

/// kfxgen refuses a cover that is neither a JPEG nor a PNG (:4692).
#[test]
fn a_cover_neither_jpeg_nor_png_is_a_model_error() {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/with_cover")).unwrap();
    *entry(&mut model.args, "cover_image") = PyValue::Bytes(b"GIF89a\x01\x00\x01\x00"[..].into());
    assert!(matches!(
        generate(&model),
        Err(GenerateError::Model(m)) if m.contains("cover_image")
    ));
}

/// A picture among a paragraph's words is written in them (#529), which isn't ported.
#[test]
fn pictures_among_words_are_reported_unsupported() {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/body_images")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 0);
    let paragraph = item(entry(chapter, "blocks"), 1);
    let PyValue::Str(text) = entry(paragraph, "text") else {
        panic!("not a string")
    };
    text.insert_str(0, "Words before ");
    assert!(matches!(
        generate(&model),
        Err(GenerateError::Unsupported("pictures among words"))
    ));
}
