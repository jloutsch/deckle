// SPDX-License-Identifier: GPL-3.0-only
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

#[test]
fn lists_768_matches_kfxgen() {
    matches_kfxgen("lists_768");
}

#[test]
fn lists_771_matches_kfxgen() {
    matches_kfxgen("lists_771");
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

/// `boxes` with its second chapter's box nested `depth` deep, built in the model directly:
/// JSON can't carry it (serde_json stops at 128 levels, 27 boxes).
fn nested_boxes(depth: usize) -> Result<(), GenerateError> {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/boxes")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 1);
    let boxed = item(entry(chapter, "blocks"), 2);
    let outer = boxed.clone();
    for _ in 1..depth {
        let inner = std::mem::replace(boxed, outer.clone());
        *entry(boxed, "blocks") = PyValue::List(vec![inner]);
    }
    generate(&model).map(|_| ())
}

/// Boxes nested past 32 levels are unsupported, before the port recurses into them: an EPUB's
/// boxes could otherwise overflow the stack (a debug build on a 2 MiB thread did at 60).
#[test]
fn boxes_nested_past_32_levels_are_unsupported() {
    nested_boxes(32).unwrap();
    for depth in [33, 1_000] {
        assert!(matches!(
            nested_boxes(depth),
            Err(GenerateError::Unsupported(
                "boxes nested more than 32 levels deep"
            ))
        ));
    }
}

/// `{key: value}` set in the dict `d`, added when absent.
fn set_key(d: &mut PyValue, key: &str, value: PyValue) {
    let PyValue::Dict(pairs) = d else {
        panic!("not a dict")
    };
    match pairs.iter_mut().find(|(k, _)| k.as_str() == Some(key)) {
        Some((_, v)) => *v = value,
        None => pairs.push((PyValue::Str(key.to_owned()), value)),
    }
}

fn no_value_no_n() -> PyValue {
    PyValue::Dict(vec![
        (PyValue::Str("value".to_owned()), PyValue::None),
        (PyValue::Str("n".to_owned()), PyValue::None),
    ])
}

/// The `$277` list entries in a generated book.
fn list_entries(model: &deckle_kfxgen::load::Model) -> usize {
    let out = generate(model).unwrap();
    format!("{:?}", out.fragments).matches("$277").count()
}

/// A box's `item: {}` is no item, as in kfxgen, where each use of it asks whether the dict
/// is empty (`if item`, `or`, `.get("item")`): the same bytes as `item: None`.
#[test]
fn an_empty_box_item_is_no_item() {
    let with = |value: PyValue| {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/lists_768")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 0);
        let boxed = entry(
            item(entry(item(entry(chapter, "blocks"), 6), "blocks"), 1),
            "box",
        );
        *entry(boxed, "item") = value;
        generate(&model).unwrap().bytes
    };
    assert!(with(PyValue::Dict(vec![])) == with(PyValue::None));
}

/// A paragraph's `list_item: {}` is an item, as in kfxgen, which asks `is not None` there:
/// its one text chunk is a `$277` with no number, the bytes of `{value: None, n: None}`.
#[test]
fn an_empty_list_item_on_one_text_chunk_is_an_item() {
    let with = |value: PyValue| {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/lists_768")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 0);
        let para = item(entry(item(entry(chapter, "blocks"), 6), "blocks"), 0);
        *entry(para, "list_item") = value;
        generate(&model).unwrap().bytes
    };
    assert!(with(PyValue::Dict(vec![])) == with(no_value_no_n()));
}

/// A paragraph cut into words and a picture with `list_item: {}` goes in a container, as in
/// kfxgen's `_mark_list_item`, but the container's `item` is the empty dict, so it is no
/// `$277`; with `{value: None, n: None}` it is one.
#[test]
fn an_empty_list_item_round_a_picture_is_no_list_entry() {
    let entries = |value: Option<PyValue>| {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/figures")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 5);
        let para = item(entry(item(entry(chapter, "blocks"), 2), "blocks"), 0);
        // The picture on a line of its own: the paragraph is cut after it.
        *entry(para, "text") = PyValue::Str("\0IMG\x01p.png\x01picture\0\n\nWords.".to_owned());
        *entry(para, "spans") = PyValue::List(vec![]);
        if let Some(value) = value {
            set_key(para, "list_item", value);
        }
        list_entries(&model)
    };
    let none = entries(None);
    assert_eq!(entries(Some(no_value_no_n())), none + 1);
    assert_eq!(entries(Some(PyValue::Dict(vec![]))), none);
}
