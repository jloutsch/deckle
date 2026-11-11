// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing direction, language, ruby, tate-chu-yoko and vertical text against kfxgen's KFX
//! (phase 1 plan, Task 12).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::{Model, load_model};
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

/// A Hebrew book whose spine turns right to left (#246): its page direction and
/// `he-reflow-language`, its paragraphs' direction.
#[test]
fn rtl_matches_kfxgen() {
    matches_kfxgen("rtl");
}

/// Direction and language inside a left-to-right book: runs of their own direction, alone,
/// nested, in bold and in links (#710, #715); languages alone (#710); paragraphs, tables,
/// rows, groups, cells and boxes of their own (#246, #605); a rebuilt contents page's
/// language (#714).
#[test]
fn directions_matches_kfxgen() {
    matches_kfxgen("directions");
}

fn directions() -> Model {
    load_model(&deckle_kfx::fixtures::kfxgen("models/directions")).unwrap()
}

fn s(v: &str) -> PyValue {
    PyValue::Str(v.into())
}

fn pair(kind: &str, value: PyValue) -> PyValue {
    PyValue::Tuple(vec![s(kind), value])
}

/// The directions model with the right-to-left paragraph's `bidi` edited.
fn with_bidi(edit: impl FnOnce(&mut PyValue)) -> Result<(), GenerateError> {
    let mut model = directions();
    let chapter = item(entry(&mut model.args, "chapters"), 2);
    let block = item(entry(chapter, "blocks"), 1);
    edit(entry(entry(block, "block_style"), "bidi"));
    generate(&model).map(|_| ())
}

/// A direction or unicode-bidi `_BIDI_SYMBOLS` doesn't have makes kfxgen raise `KeyError`
/// writing it; a `bidi` of another shape makes it raise too. Model errors naming the dict.
#[test]
fn a_direction_kfxgen_cannot_write_is_a_model_error() {
    let edits: [fn(&mut PyValue); 5] = [
        |b| *entry(b, "direction") = s("sideways"),
        |b| *entry(b, "unicode_bidi") = PyValue::Int(1),
        |b| *entry(b, "set_here") = PyValue::Int(1),
        |b| *b = PyValue::List(Vec::new()),
        |b| {
            let PyValue::Dict(pairs) = b else { panic!() };
            pairs.push((s("dir"), s("rtl")));
        },
    ];
    for edit in edits {
        match with_bidi(edit) {
            Err(GenerateError::Model(m)) => assert!(m.contains(".bidi"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
    // A language is written as it is.
    with_bidi(|b| *entry(b, "lang") = s("x-anything")).unwrap();
}

/// The directions model with the first run of its first paragraph given `flags`.
fn with_flags(flags: Vec<PyValue>) -> Result<(), GenerateError> {
    let mut model = directions();
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

/// A direction flag kfxgen can't unpack, or a language that isn't a string, is a model error
/// naming the runs. Two languages on one run is unsupported: kfxgen would take the first in
/// set order.
#[test]
fn a_direction_flag_of_another_shape_is_a_model_error() {
    let keys = |k: &str, v: &str| PyValue::Tuple(vec![PyValue::Tuple(vec![s(k), s(v)])]);
    for flag in [
        pair("bidi", PyValue::Int(1)),
        pair(
            "bidi",
            PyValue::Tuple(vec![s("0"), PyValue::Int(1), keys("lang", "he")]),
        ),
        pair(
            "bidi",
            PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(1), PyValue::Int(2)]),
        ),
        pair(
            "bidi",
            PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(1), keys("dir", "rtl")]),
        ),
        pair("lang", PyValue::Int(1)),
    ] {
        match with_flags(vec![flag.clone()]) {
            Err(GenerateError::Model(m)) => assert!(m.contains(".spans"), "{flag:?}: {m}"),
            other => panic!("{flag:?}: {other:?}"),
        }
    }
    assert!(matches!(
        with_flags(vec![pair("lang", s("fr")), pair("lang", s("de"))]),
        Err(GenerateError::Unsupported("a run in two languages"))
    ));
}
