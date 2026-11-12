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

/// kfxgen's `test_books/navigation/nav-kfxgen.epub`: an NCX whose entries name places inside
/// chapters (#363), one of no title and one of no place, and a contents page's line landing on
/// one.
#[test]
fn navigation_matches_kfxgen() {
    matches_kfxgen("navigation");
}

/// Contents entries inside a chapter past what a book reaches: part-way into a paragraph, on
/// a table and a box, declared nowhere, of no key, nested, in a chapter left out of the
/// contents; a contents page's lines landing on them (#363).
#[test]
fn contents_entries_matches_kfxgen() {
    matches_kfxgen("contents_entries");
}

/// kfxgen's `test_books/typography/typography-new-source.epub`: small caps, text-transform,
/// letter spacing and colour on paragraphs, headings and runs, set back inside them, and a
/// drop cap (#239, #240).
#[test]
fn typography_matches_kfxgen() {
    matches_kfxgen("typography");
}

/// kfxgen's `test_books/colour/colour-new-source.epub`: text colours on paragraphs and runs,
/// light ones darkened against the page (#239, #383).
#[test]
fn colour_matches_kfxgen() {
    matches_kfxgen("colour");
}

/// kfxgen's `test_books/link_colours_732/s732k.epub`: grey, red and black links, grey words
/// inside and around links (#728, #732).
#[test]
fn link_colours_matches_kfxgen() {
    matches_kfxgen("link_colours");
}

/// kfxgen's `test_books/dark_box_colours_734/c734k.epub`: text colours inside light and dark
/// boxes, kept as the book gives them (#383, #734).
#[test]
fn dark_boxes_matches_kfxgen() {
    matches_kfxgen("dark_boxes");
}

/// kfxgen's `test_books/drop_caps/drop-caps-new-source.epub`: initials of several sizes and
/// lengths, coloured (#240).
#[test]
fn drop_caps_matches_kfxgen() {
    matches_kfxgen("drop_caps");
}

/// kfxgen's `test_books/figures/figures-new-2-source.epub`: figures, captions and a run in
/// small caps.
#[test]
fn figures_matches_kfxgen() {
    matches_kfxgen("figures");
}

/// Typography past what a book reaches: on the book's heading, cells (on the page, filled,
/// in a filled box), keywords Previewer doesn't write, a paragraph's orientation and
/// emphasis marks, a run's colour inside a link not found; drop caps of each size, past their
/// run, of a later run, of no size, with nothing after, counted from the end, in a language.
#[test]
fn typography_runs_matches_kfxgen() {
    matches_kfxgen("typography_runs");
}

/// A title spelled over blocks takes the first block's style but not its small caps,
/// transform, spacing and colour, which a title in one block gives it (issue #6).
#[test]
fn split_title_matches_kfxgen() {
    matches_kfxgen("split_title");
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

/// A contents entry kfxgen can't read, or a contents line's key that isn't a string, is a
/// model error naming it.
#[test]
fn a_contents_entry_of_another_shape_is_a_model_error() {
    let dir = deckle_kfx::fixtures::kfxgen("models/contents_entries");
    type Edit = fn(&mut PyValue);
    let edits: [(&str, Edit); 5] = [
        ("sub_entries[0].key", |sub| {
            let PyValue::Dict(pairs) = sub else { panic!() };
            pairs.retain(|(k, _)| k.as_str() != Some("key"));
        }),
        ("sub_entries[0].key", |sub| {
            *entry(sub, "key") = PyValue::Int(1)
        }),
        ("sub_entries[0].title", |sub| {
            *entry(sub, "title") = PyValue::None
        }),
        ("sub_entries[0].level", |sub| {
            *entry(sub, "level") = PyValue::None
        }),
        ("sub_entries", |sub| *sub = s("x")),
    ];
    for (want, edit) in edits {
        let mut model = load_model(&dir).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 1);
        let subs = entry(chapter, "sub_entries");
        if want == "sub_entries" {
            edit(subs);
        } else {
            edit(item(subs, 0));
        }
        match generate(&model) {
            Err(GenerateError::Model(m)) => assert!(m.contains(want), "{want}: {m}"),
            other => panic!("{want}: {other:?}"),
        }
    }
    let mut model = load_model(&dir).unwrap();
    let contents = item(entry(&mut model.args, "chapters"), 0);
    *entry(item(entry(contents, "toc_links"), 1), "target_key") = PyValue::Int(1);
    match generate(&model) {
        Err(GenerateError::Model(m)) => assert!(m.contains("toc_links[1].target_key"), "{m}"),
        other => panic!("{other:?}"),
    }
}

/// Typography flags kfxgen can't read are model errors naming the runs; two of one kind on a
/// run are unsupported: kfxgen would take the first in set order.
#[test]
fn typography_flags_of_another_shape_are_model_errors() {
    for flag in [
        pair("small-caps", PyValue::Int(1)),
        pair("transform", PyValue::None),
        pair("spacing", s("1em")),
        pair("spacing", PyValue::Tuple(vec![s("x"), s("$308")])),
        pair("colour", s("red")),
    ] {
        match with_flags(vec![flag.clone()]) {
            Err(GenerateError::Model(m)) => assert!(m.contains(".spans"), "{flag:?}: {m}"),
            other => panic!("{flag:?}: {other:?}"),
        }
    }
    for kind in ["small-caps", "transform"] {
        assert!(matches!(
            with_flags(vec![pair(kind, s("a")), pair(kind, s("b"))]),
            Err(GenerateError::Unsupported(
                "a run of two values of one kind of typography"
            ))
        ));
    }
    let spacing = |m: &str| pair("spacing", PyValue::Tuple(vec![s(m), s("$308")]));
    for flags in [
        vec![spacing("1"), spacing("2")],
        vec![
            pair("colour", PyValue::Int(1)),
            pair("colour", PyValue::Int(2)),
        ],
    ] {
        assert!(matches!(
            with_flags(flags),
            Err(GenerateError::Unsupported(
                "a run of two values of one kind of typography"
            ))
        ));
    }
}

/// A drop cap whose line count passes 2^63 is a model error, where kfxgen writes Python's
/// unbounded int.
#[test]
fn a_drop_cap_past_2_63_lines_is_a_model_error() {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/typography_runs")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 1);
    let block = item(entry(chapter, "blocks"), 1);
    let PyValue::List(spans) = entry(block, "spans") else {
        panic!("not a list")
    };
    let PyValue::Tuple(span) = &mut spans[0] else {
        panic!("not a tuple")
    };
    span[2] = PyValue::Set(vec![pair("size", PyValue::Float(1e300))]);
    match generate(&model) {
        Err(GenerateError::Model(m)) => assert!(m.contains("drop cap"), "{m}"),
        other => panic!("{other:?}"),
    }
}

/// A paragraph's typography or drop cap kfxgen can't read is a model error naming the key.
#[test]
fn block_typography_of_another_shape_is_a_model_error() {
    for (key, value) in [
        ("small_caps", PyValue::Int(1)),
        ("text_emphasis", PyValue::Int(1)),
        ("letter_spacing", s("1em")),
        ("colour", s("red")),
        ("drop_cap", s("1")),
    ] {
        let mut model =
            load_model(&deckle_kfx::fixtures::kfxgen("models/typography_runs")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 0);
        let block = item(entry(chapter, "blocks"), 1);
        let PyValue::Dict(style) = entry(block, "block_style") else {
            panic!("not a dict")
        };
        style.retain(|(k, _)| k.as_str() != Some(key));
        style.push((s(key), value));
        match generate(&model) {
            Err(GenerateError::Model(m)) => assert!(m.contains(key), "{key}: {m}"),
            other => panic!("{key}: {other:?}"),
        }
    }
}

/// `contents_entries` with one chapter's entries nested `depth` deep under it.
fn nested_contents(depth: usize) -> Result<(), GenerateError> {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/contents_entries")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 1);
    let PyValue::List(subs) = entry(chapter, "sub_entries") else {
        panic!("not a list")
    };
    let first = subs[0].clone();
    *subs = (1..depth)
        .map(|level| {
            let mut sub = first.clone();
            *entry(&mut sub, "level") = PyValue::Int(level as i64);
            sub
        })
        .collect();
    generate(&model).map(|_| ())
}

/// Contents nested past 32 levels are unsupported, before the port recurses into them: an
/// EPUB's contents could otherwise overflow the stack (a debug build on a 2 MiB thread did
/// at 80). kfxgen itself fails at about 163 (`RecursionError`).
#[test]
fn contents_nested_past_32_levels_are_unsupported() {
    nested_contents(32).unwrap();
    assert!(matches!(
        nested_contents(33),
        Err(GenerateError::Unsupported(
            "contents nested more than 32 levels deep"
        ))
    ));
    assert!(matches!(
        nested_contents(100_000),
        Err(GenerateError::Unsupported(
            "contents nested more than 32 levels deep"
        ))
    ));
}
