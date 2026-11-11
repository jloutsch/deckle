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

/// A paragraph whose floats aren't all at its start goes on to kfxgen's inline pictures
/// (`_inline_pictures`, #565), not ported: a float after words, after an unfloated picture,
/// a linked float, and a token whose side is empty.
#[test]
fn a_float_among_words_is_unsupported() {
    let pic = |more: &str| format!("\0IMG\x01OEBPS/p.png\x01p{more}\0");
    let left = pic("\x01w=30%\x01left");
    for text in [
        format!("Words {left} more words."),
        format!("{left}{} words.", pic("\x01w=30%")),
        format!("{} words.", pic("\x01w=30%\x01")),
        format!("{left}{left} words."),
    ] {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/frame_edges")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 0);
        let block = item(entry(chapter, "blocks"), 1);
        *entry(block, "text") = PyValue::Str(text.clone());
        if text.starts_with(&left) && text[left.len()..].starts_with(&left) {
            // Both floats open it; the second is linked.
            *entry(block, "spans") = PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(left.chars().count() as i64),
                PyValue::Int(1),
                PyValue::Set(vec![PyValue::Tuple(vec![
                    PyValue::Str("link".to_owned()),
                    PyValue::Str("f.xhtml#head".to_owned()),
                ])]),
            ])]);
        }
        match generate(&model) {
            Err(GenerateError::Unsupported(f)) => assert_eq!(f, "pictures among words", "{text:?}"),
            Err(e) => panic!("{text:?}: {e}"),
            Ok(_) => panic!("{text:?}: generated"),
        }
    }
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

/// The words after a paragraph's floats, cut at an empty line, are several entries in kfxgen
/// (`_cut_after_picture_lines` :535, #602), not ported; one line break keeps one entry.
#[test]
fn floated_words_cut_at_an_empty_line_are_unsupported() {
    let left = "\0IMG\x01OEBPS/p.png\x01left\x01w=30%\x01left\0";
    for (words, cut) in [
        ("Words.\n\nMore words.", true),
        ("Words.\nMore words.", false),
    ] {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/frame_edges")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 0);
        let block = item(entry(chapter, "blocks"), 4);
        *entry(block, "text") = PyValue::Str(format!("{left}{words}"));
        match generate(&model) {
            Err(GenerateError::Unsupported(f)) if cut => {
                assert_eq!(f, "a paragraph cut at an empty line")
            }
            Ok(_) if !cut => {}
            Err(e) => panic!("{words:?}: {e}"),
            Ok(_) => panic!("{words:?}: generated"),
        }
    }
}
