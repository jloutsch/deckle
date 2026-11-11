// SPDX-License-Identifier: GPL-3.0-or-later
//! The book's page list and landmarks against kfxgen's KFX (phase 1 plan, Task 11).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::{Model, load_model};
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

/// kfxgen's `test_books/landmarks_pages`: an EPUB page list, contents and start landmarks, a
/// cover.
#[test]
fn page_list_matches_kfxgen() {
    matches_kfxgen("page_list");
}

/// A page list with no landmarks or cover: ids with and without offsets, whole files, ids
/// that land nowhere.
#[test]
fn page_list_alone_matches_kfxgen() {
    matches_kfxgen("page_list_alone");
}

/// Landmarks with no page list or cover: ones that land nowhere, a second start, a page list
/// that lands nowhere with no approximate pages.
#[test]
fn landmarks_alone_matches_kfxgen() {
    matches_kfxgen("landmarks_alone");
}

fn landmarks_alone() -> Model {
    load_model(&deckle_kfx::fixtures::kfxgen("models/landmarks_alone")).unwrap()
}

fn s(v: &str) -> PyValue {
    PyValue::Str(v.into())
}

fn tuple(items: &[PyValue]) -> PyValue {
    PyValue::Tuple(items.to_vec())
}

/// Landmarks or a page list kfxgen can't unpack, or whose labels and targets aren't strings,
/// are model errors naming the argument. So is a landmark of a kind kfxgen doesn't know that
/// lands: kfxgen raises `KeyError` on it.
#[test]
fn landmarks_or_pages_of_another_shape_are_model_errors() {
    for (key, value) in [
        ("landmarks", s("toc")),
        (
            "landmarks",
            PyValue::List(vec![tuple(&[s("toc"), s("f.xhtml")])]),
        ),
        (
            "landmarks",
            PyValue::List(vec![tuple(&[s("toc"), PyValue::Int(1), s("f.xhtml")])]),
        ),
        (
            "landmarks",
            PyValue::List(vec![tuple(&[s("toc"), s("Front"), PyValue::Int(1)])]),
        ),
        (
            "landmarks",
            PyValue::List(vec![tuple(&[s("other"), s("Front"), s("f.xhtml")])]),
        ),
        ("page_list", s("1")),
        (
            "page_list",
            PyValue::List(vec![tuple(&[s("1"), s("f.xhtml"), s("x")])]),
        ),
        (
            "page_list",
            PyValue::List(vec![tuple(&[PyValue::Int(1), s("f.xhtml")])]),
        ),
        (
            "page_list",
            PyValue::List(vec![tuple(&[s("1"), PyValue::None])]),
        ),
    ] {
        let mut model = landmarks_alone();
        *entry(&mut model.args, key) = value.clone();
        match generate(&model) {
            Err(GenerateError::Model(m)) => assert!(m.contains(key), "{value:?}: {m}"),
            r => panic!("{value:?}: {r:?}"),
        }
    }
    // None, or a list that lands nowhere, is no landmarks.
    let mut model = landmarks_alone();
    *entry(&mut model.args, "landmarks") = PyValue::None;
    assert!(generate(&model).is_ok());
    *item(entry(&mut model.args, "page_list"), 0) = tuple(&[s("1"), s("gone.xhtml")]);
    assert!(generate(&model).is_ok());
}
