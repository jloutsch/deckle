// SPDX-License-Identifier: GPL-3.0-or-later
//! Block styles, boxes, rules, spaces, lists and pop-up notes against kfxgen's KFX (phase 1
//! plan, Task 9).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

#[test]
fn boxes_matches_kfxgen() {
    matches_kfxgen("boxes");
}

#[test]
fn rules_spaces_matches_kfxgen() {
    matches_kfxgen("rules_spaces");
}

#[test]
fn spacers_matches_kfxgen() {
    matches_kfxgen("spacers");
}

#[test]
fn lists_matches_kfxgen() {
    matches_kfxgen("lists");
}

#[test]
fn notes_matches_kfxgen() {
    matches_kfxgen("notes");
}

/// Paragraph and heading styles the Calibre-made fixtures don't reach: alignment, italic,
/// line heights, indents, scene breaks, monospace, a language of its own, a box's language
/// and padding at another size.
#[test]
fn block_styles_matches_kfxgen() {
    matches_kfxgen("block_styles");
}

/// A chapter's title in and around boxes, boxes in boxes, a table in a box, a box's ids, a
/// title over rules, a rule first in its storyline, a note in a table cell.
#[test]
fn box_titles_matches_kfxgen() {
    matches_kfxgen("box_titles");
}

/// Two text sizes with as many characters each: the smaller is the main size.
#[test]
fn main_size_tie_matches_kfxgen() {
    matches_kfxgen("main_size_tie");
}

/// A length's magnitude goes into an Ion decimal of 38 digits and a 32-bit exponent. kfxgen
/// raises on "inf" and "NaN" (`abs()` of a str when it writes them) and writes longer
/// coefficients and exponents, which deckle-kfx's decimal can't hold: each is a model error
/// naming the length, never a 0 or a panic (DIVERGENCES.md). "1e400" fits and is written.
#[test]
fn a_length_kfxgen_cannot_hold_is_a_model_error() {
    let forty = "1".repeat(40);
    let cases = [
        ("inf", false),
        ("NaN", false),
        (forty.as_str(), false),
        ("1.5e-2147483648", false),
        ("1e400", true),
        ("2.5", true),
    ];
    for key in ["margin_left", "indent"] {
        for (magnitude, ok) in cases {
            let mut model =
                load_model(&deckle_kfx::fixtures::kfxgen("models/block_styles")).unwrap();
            let chapter = item(entry(&mut model.args, "chapters"), 0);
            let style = entry(item(entry(chapter, "blocks"), 1), "block_style");
            *entry(style, key) = PyValue::Tuple(vec![
                PyValue::Str(magnitude.to_owned()),
                PyValue::Str("$314".to_owned()),
            ]);
            match generate(&model) {
                Ok(_) => assert!(ok, "{key} {magnitude}"),
                Err(GenerateError::Model(m)) => {
                    assert!(!ok && m.contains(key), "{key} {magnitude}: {m}")
                }
                Err(e) => panic!("{key} {magnitude}: {e}"),
            }
        }
    }
}
