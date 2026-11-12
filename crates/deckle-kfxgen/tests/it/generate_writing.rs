// SPDX-License-Identifier: GPL-3.0-only
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

/// Ruby in a horizontal Japanese book (#236): readings over one character, two, two runs, a
/// bold run, the start of a link, the same reading twice, in parentheses; headings whose
/// contents label runs the readings in, bare, in parentheses and in full-width ones, and one
/// whose label leaves them out; readings in several chapters.
#[test]
fn ruby_matches_kfxgen() {
    matches_kfxgen("ruby");
}

/// kfxgen's `test_books/vertical_247/s746k.epub`: a vertical-rl Japanese book on a
/// right-to-left spine (#247), its margins moved for vertical text, upright and sideways
/// runs, a ruby, a link, and a horizontal colophon page.
#[test]
fn vertical_matches_kfxgen() {
    matches_kfxgen("vertical");
}

/// A vertical-rl book whose contents page kfxgen rebuilds: a chapter of no writing mode of
/// its own takes the book's (#247).
#[test]
fn vertical_contents_matches_kfxgen() {
    matches_kfxgen("vertical_contents");
}

/// kfxgen's `test_books/tcy_emphasis_748/s748k.epub`: tate-chu-yoko, bold and in bold, and
/// emphasis marks of four kinds, one the book's own, in a vertical-rl book (#247).
#[test]
fn ruby_tcy_matches_kfxgen() {
    matches_kfxgen("ruby_tcy");
}

/// 縦中横 whose texts fill a `content_math` fragment: 8,191 bytes keep the next text in it,
/// 8,192 start the next fragment (`_math_note`, :2380).
#[test]
fn tcy_notes_matches_kfxgen() {
    matches_kfxgen("tcy_notes");
}

/// Horizontal blocks in a vertical book (#247): boxes, bordered and padded, a table, and
/// pictures in them (#755), plain, aligned, sized, linked, floated and beside a paragraph;
/// words around a picture (#757); percentages on Previewer's 480pt page (#754).
#[test]
fn horizontal_blocks_matches_kfxgen() {
    matches_kfxgen("horizontal_blocks");
}

/// The same files on an ltr spine: the book isn't vertical, its files are, so a picture in
/// a horizontal block is a block one, aligned as its block (#755), on a 384pt page.
#[test]
fn horizontal_blocks_ltr_matches_kfxgen() {
    matches_kfxgen("horizontal_blocks_ltr");
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

/// A ruby kfxgen can't unpack is a model error naming the runs; two on one run are
/// unsupported: kfxgen would take the first in set order.
#[test]
fn a_ruby_of_another_shape_is_a_model_error() {
    for flag in [
        pair("ruby", s("かん")),
        pair("ruby", PyValue::Tuple(vec![s("0"), s("かん")])),
        pair(
            "ruby",
            PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(1)]),
        ),
    ] {
        match with_flags(vec![flag.clone()]) {
            Err(GenerateError::Model(m)) => assert!(m.contains(".spans"), "{flag:?}: {m}"),
            other => panic!("{flag:?}: {other:?}"),
        }
    }
    let ruby = |n: i64| pair("ruby", PyValue::Tuple(vec![PyValue::Int(n), s("よみ")]));
    assert!(matches!(
        with_flags(vec![ruby(0), ruby(1)]),
        Err(GenerateError::Unsupported("a run under two rubies"))
    ));
}

/// 縦中横, a text-orientation or emphasis marks kfxgen can't unpack are model errors naming
/// the runs; two of one kind on a run are unsupported: kfxgen would take the first in set
/// order.
#[test]
fn vertical_run_flags_of_another_shape_are_model_errors() {
    for flag in [
        pair("tcy", s("12")),
        pair(
            "tcy",
            PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(12)]),
        ),
        pair("orientation", PyValue::Int(1)),
        pair("emphasis-marks", PyValue::None),
    ] {
        match with_flags(vec![flag.clone()]) {
            Err(GenerateError::Model(m)) => assert!(m.contains(".spans"), "{flag:?}: {m}"),
            other => panic!("{flag:?}: {other:?}"),
        }
    }
    let tcy = |n: i64| pair("tcy", PyValue::Tuple(vec![PyValue::Int(n), s("12")]));
    for flags in [
        vec![tcy(0), tcy(1)],
        vec![
            pair("orientation", s("upright")),
            pair("orientation", s("mixed")),
        ],
        vec![
            pair("emphasis-marks", s("filled")),
            pair("emphasis-marks", s("open")),
        ],
    ] {
        assert!(
            matches!(
                with_flags(flags.clone()),
                Err(GenerateError::Unsupported(_))
            ),
            "{flags:?}"
        );
    }
}
