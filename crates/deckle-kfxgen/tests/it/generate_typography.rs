// SPDX-License-Identifier: GPL-3.0-or-later
//! Link colours, paragraph typography and contents entries inside a chapter against kfxgen's
//! KFX (phase 1 plan, Task 12b).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

/// kfxgen's `test_books/link_greys_741/s741k.epub`: links in light and dark greys, one with a
/// black word inside, and plain links (#728, #741).
#[test]
fn link_greys_matches_kfxgen() {
    matches_kfxgen("link_greys");
}

/// Link colours past what a book reaches: found, with none of their own, not found and so
/// written as the text's colour against the page or inside a fill, a coloured run inside a
/// link found and not (#728, #732, #383).
#[test]
fn link_colour_runs_matches_kfxgen() {
    matches_kfxgen("link_colour_runs");
}

fn s(v: &str) -> PyValue {
    PyValue::Str(v.into())
}

fn pair(kind: &str, value: PyValue) -> PyValue {
    PyValue::Tuple(vec![s(kind), value])
}

/// `link_greys` with the flags of the first link run replaced.
fn with_flags(flags: Vec<PyValue>) -> Result<(), GenerateError> {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/link_greys")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 0);
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

/// A link colour kfxgen can't unpack is a model error naming the runs; two on one run are
/// unsupported: kfxgen would take the first in set order.
#[test]
fn a_link_colour_of_another_shape_is_a_model_error() {
    let colour =
        |link: PyValue, text: PyValue| pair("link-colour", PyValue::Tuple(vec![link, text]));
    for flag in [
        pair("link-colour", PyValue::Int(1)),
        colour(PyValue::Int(1), PyValue::None),
        colour(s("red"), PyValue::Int(1)),
        pair("link-colour", PyValue::Tuple(vec![PyValue::Int(1)])),
    ] {
        match with_flags(vec![flag.clone()]) {
            Err(GenerateError::Model(m)) => assert!(m.contains(".spans"), "{flag:?}: {m}"),
            other => panic!("{flag:?}: {other:?}"),
        }
    }
    assert!(matches!(
        with_flags(vec![
            colour(PyValue::Int(1), PyValue::Int(1)),
            colour(PyValue::None, PyValue::Int(1)),
        ]),
        Err(GenerateError::Unsupported("a run of two link colours"))
    ));
}
