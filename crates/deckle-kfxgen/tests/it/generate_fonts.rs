// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedded fonts, monospace runs and runs of their own size against kfxgen's KFX (phase 1
//! plan, Task 10).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::{Model, load_model};
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

/// An `@font-face` mapped to text through Calibre, italic with no italic face.
#[test]
fn fonts_matches_kfxgen() {
    matches_kfxgen("fonts");
}

/// kfxgen's `test_books/font_sizes`: paragraphs, spans and cells sized in CSS.
#[test]
fn font_sizes_matches_kfxgen() {
    matches_kfxgen("font_sizes");
}

/// What those two don't reach: several faces matched exactly and by fallback, a second
/// family, monospace runs embedded and not, inside a link, sized runs at, off and back to
/// the paragraph's size, the book's heading in sized runs, faces in cells.
#[test]
fn font_faces_matches_kfxgen() {
    matches_kfxgen("font_faces");
}

fn font_faces() -> Model {
    load_model(&deckle_kfx::fixtures::kfxgen("models/font_faces")).unwrap()
}

/// The `i`th face's state in `font_faces`.
fn face(model: &mut Model, i: usize) -> &mut PyValue {
    let PyValue::Obj { state, .. } = entry(&mut model.args, "font_table") else {
        panic!("not an object")
    };
    let PyValue::Obj { state, .. } = item(entry(state, "faces"), i) else {
        panic!("not an object")
    };
    state
}

/// A face whose weight, slant, bytes or names are of another type, or whose name or location
/// is empty (kfxlib refuses an empty symbol), is a model error naming the font table.
#[test]
fn a_face_of_another_shape_is_a_model_error() {
    for (key, value) in [
        ("weight", PyValue::Str("9".into())),
        ("italic", PyValue::Int(1)),
        ("data", PyValue::Str("9".into())),
        ("css_family", PyValue::Int(9)),
        ("emitted_family", PyValue::Int(9)),
        ("emitted_family", PyValue::Str(String::new())),
        ("location", PyValue::Str(String::new())),
    ] {
        let mut model = font_faces();
        *entry(face(&mut model, 0), key) = value;
        match generate(&model) {
            Err(GenerateError::Model(m)) => assert!(m.contains("font_table"), "{key}: {m}"),
            r => panic!("{key}: {r:?}"),
        }
    }
    // A table with no faces attribute, or no table at all: none embeds nothing.
    let mut model = font_faces();
    *entry(&mut model.args, "font_table") = PyValue::Dict(Vec::new());
    assert!(matches!(generate(&model), Err(GenerateError::Model(m)) if m.contains("font_table")));
    *entry(&mut model.args, "font_table") = PyValue::None;
    assert!(generate(&model).is_ok());
}

/// `font_faces`' second chapter's second paragraph, its first run's flags set to `flags`.
fn with_flags(flags: Vec<PyValue>) -> Result<(), GenerateError> {
    let mut model = font_faces();
    let chapter = item(entry(&mut model.args, "chapters"), 1);
    let block = item(entry(chapter, "blocks"), 1);
    let PyValue::List(spans) = entry(block, "spans") else {
        panic!("not a list")
    };
    let PyValue::Tuple(span) = &mut spans[0] else {
        panic!("not a tuple")
    };
    span[2] = PyValue::Set(flags);
    generate(&model).map(|_| ())
}

fn pair(kind: &str, value: PyValue) -> PyValue {
    PyValue::Tuple(vec![PyValue::Str(kind.into()), value])
}

/// A size that isn't a number, or monospace families that aren't strings, is a model error
/// naming the runs: kfxgen raises on them.
#[test]
fn a_size_or_family_of_another_type_is_a_model_error() {
    for flag in [
        pair("size", PyValue::Str("24".into())),
        pair("mono", PyValue::Int(1)),
        pair("mono", PyValue::Tuple(vec![PyValue::Int(1)])),
    ] {
        match with_flags(vec![flag.clone()]) {
            Err(GenerateError::Model(m)) => assert!(m.contains("spans"), "{flag:?}: {m}"),
            r => panic!("{flag:?}: {r:?}"),
        }
    }
    assert!(with_flags(vec![pair("size", PyValue::Int(24))]).is_ok());
}

/// A run of two sizes or two families: kfxgen takes one in set order, which
/// `PYTHONHASHSEED` changes, so the port refuses the run.
#[test]
fn a_run_of_two_sizes_or_families_is_unsupported() {
    let mono = |f: &str| pair("mono", PyValue::Tuple(vec![PyValue::Str(f.into())]));
    for flags in [
        vec![
            pair("size", PyValue::Int(24)),
            pair("size", PyValue::Int(12)),
        ],
        vec![mono("courier"), mono("menlo")],
    ] {
        match with_flags(flags) {
            Err(GenerateError::Unsupported(f)) => {
                assert_eq!(f, "a run of two sizes or two monospace families")
            }
            r => panic!("{r:?}"),
        }
    }
}
