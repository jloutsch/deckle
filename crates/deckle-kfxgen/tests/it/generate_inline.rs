// SPDX-License-Identifier: GPL-3.0-or-later
//! Inline runs, links and anchors against kfxgen's KFX (phase 1 plan, Task 6).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_skeleton::matches_kfxgen;

#[test]
fn sub_super_marks_matches_kfxgen() {
    matches_kfxgen("sub_super_marks");
}

#[test]
fn linked_toc_matches_kfxgen() {
    matches_kfxgen("linked_toc");
}

#[test]
fn marker_offsets_matches_kfxgen() {
    matches_kfxgen("marker_offsets");
}

/// Run flags, link targets and ids no shim or Calibre-path fixture reaches.
#[test]
fn inline_runs_matches_kfxgen() {
    matches_kfxgen("inline_runs");
}

#[test]
fn nested_contents_matches_kfxgen() {
    matches_kfxgen("nested_contents");
}

#[test]
fn heading_runs_matches_kfxgen() {
    matches_kfxgen("heading_runs");
}

#[test]
#[ignore = "a table, block styles and margins, sized and monospace runs, ruby: Tasks 8 to 12"]
fn links_matches_kfxgen() {
    matches_kfxgen("links");
}

/// The value at `key` of a Python dict.
pub(crate) fn entry<'a>(dict: &'a mut PyValue, key: &str) -> &'a mut PyValue {
    let PyValue::Dict(pairs) = dict else {
        panic!("not a dict")
    };
    let (_, v) = pairs
        .iter_mut()
        .find(|(k, _)| k.as_str() == Some(key))
        .unwrap();
    v
}

pub(crate) fn item(list: &mut PyValue, i: usize) -> &mut PyValue {
    let PyValue::List(items) = list else {
        panic!("not a list")
    };
    &mut items[i]
}

/// Math is out of scope: a span holding an equation is an error, not a silent drop.
#[test]
fn math_span_is_reported_unsupported() {
    let dir = deckle_kfx::fixtures::kfxgen("models/minimal");
    let mut model = load_model(&dir).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 0);
    let paragraph = item(entry(chapter, "blocks"), 1);
    // `make_math_flag`: ("math", (serial, drawing, alt, source)).
    let math = PyValue::Tuple(vec![
        PyValue::Str("math".into()),
        PyValue::Tuple(vec![
            PyValue::Int(0),
            PyValue::None,
            PyValue::Str("x".into()),
            PyValue::Str("<math/>".into()),
        ]),
    ]);
    *entry(paragraph, "spans") = PyValue::List(vec![PyValue::Tuple(vec![
        PyValue::Int(0),
        PyValue::Int(5),
        PyValue::Set(vec![math]),
    ])]);
    assert!(matches!(
        generate(&model),
        Err(GenerateError::Unsupported("math"))
    ));
}

/// A chapter's heading level picks a `$798` unit, of which there are six: another is an
/// error in the model, not a panic.
#[test]
fn chapter_heading_level_out_of_range_is_a_model_error() {
    let dir = deckle_kfx::fixtures::kfxgen("models/minimal");
    let mut model = load_model(&dir).unwrap();
    let PyValue::Dict(chapter) = item(entry(&mut model.args, "chapters"), 0) else {
        panic!("not a dict")
    };
    chapter.push((PyValue::Str("heading_level".into()), PyValue::Int(7)));
    // With no blocks, no book heading's level replaces the chapter's.
    chapter.retain(|(k, _)| k.as_str() != Some("blocks"));
    assert!(matches!(
        generate(&model),
        Err(GenerateError::Model(m)) if m.contains("heading_level")
    ));
}
