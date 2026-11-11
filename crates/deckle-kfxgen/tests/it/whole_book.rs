// SPDX-License-Identifier: GPL-3.0-or-later
//! Whole books against kfxgen (phase 1 plan, Task 13).
use deckle_kfxgen::generate::generate;
use deckle_kfxgen::load::load_model;

use crate::generate_skeleton::assert_matches;
use crate::load::model_dirs;

/// Every fixture is ported: `Unsupported` here is a regression, not a gap.
#[test]
fn every_fixture_is_byte_identical() {
    let dirs = model_dirs();
    assert!(!dirs.is_empty());
    for dir in dirs {
        match generate(&load_model(&dir).unwrap()) {
            Ok(out) => assert_matches(&dir, &out),
            Err(e) => panic!("{}: {e}", dir.display()),
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
