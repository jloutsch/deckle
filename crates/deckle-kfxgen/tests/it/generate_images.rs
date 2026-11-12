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

/// The cover's picture in a box in a box, a caption after it: left out, its ids on the
/// caption (issue #5).
#[test]
fn cover_in_box_matches_kfxgen() {
    matches_kfxgen("cover_in_box");
}

/// The cover's picture alone in its box: the box left out, its ids and the picture's on the
/// next block (issue #5).
#[test]
fn cover_box_alone_matches_kfxgen() {
    matches_kfxgen("cover_box_alone");
}

/// The cover's picture is vertical, the rest of its page isn't: the page's writing mode is
/// read once the picture is out (issue #8).
#[test]
fn cover_mode_matches_kfxgen() {
    matches_kfxgen("cover_mode");
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

/// A contents line at a level or to a chapter at an i64's edge is no panic (issue #12).
#[test]
fn a_contents_line_at_an_i64_edge_does_not_panic() {
    for (key, value) in [("level", i64::MIN), ("target_chapter_idx", i64::MAX)] {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/cover_paths")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 2);
        let link = item(entry(chapter, "toc_links"), 0);
        *entry(link, key) = PyValue::Int(value);
        let result = generate(&model);
        if key == "level" {
            assert!(
                matches!(result, Err(GenerateError::Unsupported(_))),
                "{key}"
            );
        }
    }
}
