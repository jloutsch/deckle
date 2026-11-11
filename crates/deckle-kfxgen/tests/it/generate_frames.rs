// SPDX-License-Identifier: GPL-3.0-or-later
//! Frames against kfxgen's KFX: sized, floated and centred boxes, and a paragraph's floated
//! pictures in a container of their own (phase 1 plan, Task 9b).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

/// Calibre-made: figures at 40%, centred, in px and em, floated, shaded; pictures sized
/// against their box; floats opening a paragraph, with ids; a float in a sized box.
#[test]
fn frames_matches_kfxgen() {
    matches_kfxgen("frames");
}

/// What the Calibre-made book doesn't reach: a floated paragraph's every space, its heading
/// level, ids and runs over the cut, a lost float; other sizes, sides and widths of a box.
#[test]
fn frame_edges_matches_kfxgen() {
    matches_kfxgen("frame_edges");
}

/// A box's width, width in px, float and centring kfxgen can't read are model errors.
#[test]
fn a_frame_key_of_the_wrong_kind_is_a_model_error() {
    for (key, value) in [
        ("width", PyValue::Str("40%".to_owned())),
        ("width_px", PyValue::Str("300px".to_owned())),
        ("float", PyValue::Int(1)),
    ] {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/frame_edges")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 2);
        let bx = entry(item(entry(chapter, "blocks"), 0), "box");
        *entry(bx, key) = value;
        match generate(&model) {
            Err(GenerateError::Model(m)) => assert!(m.contains(key), "{key}: {m}"),
            Err(e) => panic!("{key}: {e}"),
            Ok(_) => panic!("{key}: generated"),
        }
    }
}
