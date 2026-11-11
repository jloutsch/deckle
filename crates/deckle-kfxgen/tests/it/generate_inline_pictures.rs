// SPDX-License-Identifier: GPL-3.0-or-later
//! Pictures among a paragraph's words, linked pictures, and paragraphs cut after their
//! picture lines and at their empty lines, against kfxgen's KFX (phase 1 plan, Task 9c).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

/// kfxgen's `test_books/floats`: floats opening a paragraph and among its words, linked
/// floats, a picture among words.
#[test]
fn floats_matches_kfxgen() {
    matches_kfxgen("floats");
}

/// kfxgen's `test_books/floats_697`: floats in mid-sentence, left and right.
#[test]
fn floats_697_matches_kfxgen() {
    matches_kfxgen("floats_697");
}

/// What the real books don't reach: every size, given height and vertical-align, pictures
/// the book doesn't hold or can't measure, linked pictures alone and among words, floats of
/// each size, and paragraphs cut after picture lines and at empty lines.
#[test]
fn inline_pictures_matches_kfxgen() {
    matches_kfxgen("inline_pictures");
}

/// A picture among words in `inline_pictures`' second paragraph, sized `size` and with the
/// extras `extra`.
fn with_picture(size: &str, extra: &str) -> Result<(), GenerateError> {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/inline_pictures")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 0);
    let block = item(entry(chapter, "blocks"), 1);
    *entry(block, "text") = PyValue::Str(format!(
        "Words \0IMG\x01OEBPS/q.png\x01q\x01{size}\x01\x01{extra}\0 end."
    ));
    generate(&model).map(|_| ())
}

/// A size or shift kfxgen can't write as a number (it raises) is a model error.
#[test]
fn a_size_past_a_float_is_a_model_error() {
    let digits = "9".repeat(400);
    for (size, extra) in [
        (format!("w={digits}px"), String::new()),
        (format!("h={digits}%"), String::new()),
        (String::new(), format!("va={digits}pt")),
    ] {
        match with_picture(&size, &extra) {
            Err(GenerateError::Model(m)) => assert!(m.contains("picture"), "{m}"),
            r => panic!("{size:.5} {extra:.5}: {r:?}"),
        }
    }
    assert!(with_picture("w=99px", "va=2pt").is_ok());
}

/// Python reads a size or shift in other digits; the port doesn't.
#[test]
fn a_size_in_other_digits_is_unsupported() {
    for (size, extra) in [("w=\u{663}em", ""), ("", "va=\u{663}pt")] {
        match with_picture(size, extra) {
            Err(GenerateError::Unsupported(f)) => {
                assert_eq!(f, "a picture size in other than ASCII digits")
            }
            r => panic!("{size} {extra}: {r:?}"),
        }
    }
}

/// One run carrying two links over a picture: kfxgen takes them in set order, which
/// `PYTHONHASHSEED` changes, so the port refuses the run. Under two links' runs kfxgen takes
/// the first run's; `inline_pictures` holds those.
#[test]
fn a_picture_in_one_run_of_two_links_is_unsupported() {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/inline_pictures")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 1);
    let block = item(entry(chapter, "blocks"), 1);
    let link = |t: &str| PyValue::Tuple(vec![PyValue::Str("link".into()), PyValue::Str(t.into())]);
    let PyValue::List(spans) = entry(block, "spans") else {
        panic!("not a list")
    };
    let PyValue::Tuple(span) = &mut spans[0] else {
        panic!("not a tuple")
    };
    span[2] = PyValue::Set(vec![link("s.xhtml#a"), link("l.xhtml#on")]);
    match generate(&model) {
        Err(GenerateError::Unsupported(f)) => assert_eq!(f, "a run in two links"),
        r => panic!("{r:?}"),
    }
}
