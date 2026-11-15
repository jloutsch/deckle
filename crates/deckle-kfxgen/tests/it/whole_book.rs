// SPDX-License-Identifier: GPL-3.0-only
//! Whole books against kfxgen (phase 1 plan, Task 13).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;

use crate::generate_skeleton::assert_matches;
use crate::load::model_dirs;

/// Models made for the converter's tests (phase 3 plan, Task 2) whose books reach a generator
/// feature not ported yet (plan Decision 6): each is refused, naming the first it meets.
const NOT_PORTED: [(&str, &str); 4] = [
    ("block_edges", "preformatted text"),
    ("css_quirks", "preformatted text"),
    ("tables_more", "block styles"),
    ("text_edges", "preformatted text"),
];

/// Every other fixture is ported: `Unsupported` there is a regression, not a gap.
#[test]
fn every_fixture_is_byte_identical() {
    let dirs = model_dirs();
    assert!(!dirs.is_empty());
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let result = generate(&load_model(&dir).unwrap());
        match NOT_PORTED.iter().find(|(n, _)| *n == name) {
            Some((_, feature)) => assert!(
                matches!(result, Err(GenerateError::Unsupported(f)) if f == *feature),
                "{name}: expected Unsupported({feature:?})"
            ),
            None => match result {
                Ok(out) => assert_matches(&dir, &out),
                Err(e) => panic!("{}: {e}", dir.display()),
            },
        }
    }
}

/// No output order may hang on a hash seed: two runs in one process give the same bytes.
#[test]
fn generate_is_deterministic() {
    let model = load_model(&deckle_kfx::fixtures::kfxgen("models/long_chapter")).unwrap();
    let first = generate(&model).unwrap().bytes;
    assert!(first == generate(&model).unwrap().bytes);
}
