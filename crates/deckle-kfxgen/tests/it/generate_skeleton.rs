// SPDX-License-Identifier: GPL-3.0-or-later
//! `generate` against kfxgen's KFX for the same model (phase 1 plan, Task 5).
use std::path::{Path, PathBuf};

use deckle_kfx::container::read_kfx;
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;

use crate::load::model_dirs;
use crate::oracle::assert_fragments_match;

fn model_dir(name: &str) -> PathBuf {
    deckle_kfx::fixtures::kfxgen(&format!("models/{name}"))
}

/// Every fragment type in the KFX at `kfx`.
fn types_in(kfx: &Path) -> Vec<String> {
    let doc = read_kfx(&std::fs::read(kfx).unwrap()).unwrap();
    let mut types: Vec<String> = doc.fragments.iter().map(|f| f.ftype.0.clone()).collect();
    types.sort();
    types.dedup();
    types
}

fn assert_matches(dir: &Path, fragments: &deckle_kfx::fragment::FragmentList) {
    let expected = dir.join("expected.kfx");
    let types = types_in(&expected);
    let types: Vec<&str> = types.iter().map(String::as_str).collect();
    assert_fragments_match(fragments, &expected, &types);
}

pub(crate) fn matches_kfxgen(name: &str) {
    let dir = model_dir(name);
    let out = generate(&load_model(&dir).unwrap()).unwrap();
    assert_matches(&dir, &out.fragments);
}

#[test]
fn minimal_is_byte_identical() {
    let dir = model_dir("minimal");
    let out = generate(&load_model(&dir).unwrap()).unwrap();
    assert!(out.bytes == std::fs::read(dir.join("expected.kfx")).unwrap());
}

#[test]
fn minimal_matches_kfxgen() {
    matches_kfxgen("minimal");
}

#[test]
fn multi_chapter_matches_kfxgen() {
    matches_kfxgen("multi_chapter");
}

#[test]
fn long_chapter_matches_kfxgen() {
    matches_kfxgen("long_chapter");
}

#[test]
fn empty_chapter_matches_kfxgen() {
    matches_kfxgen("empty_chapter");
}

#[test]
fn unicode_text_matches_kfxgen() {
    matches_kfxgen("unicode_text");
}

#[test]
#[ignore = "a list item (Task 9); chapters with no heading and left out of the contents (no task names them yet)"]
fn publisher_structure_matches_kfxgen() {
    matches_kfxgen("publisher_structure");
}

/// What isn't ported yet fails loudly, never as a wrong book.
#[test]
fn every_model_matches_kfxgen_or_is_unsupported() {
    for dir in model_dirs() {
        match generate(&load_model(&dir).unwrap()) {
            Ok(out) => assert_matches(&dir, &out.fragments),
            Err(GenerateError::Unsupported(_)) => {}
            Err(e) => panic!("{}: {e}", dir.display()),
        }
    }
}
