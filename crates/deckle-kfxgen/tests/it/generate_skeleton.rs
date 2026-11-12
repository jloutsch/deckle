// SPDX-License-Identifier: GPL-3.0-only
//! `generate` against kfxgen's KFX for the same model (phase 1 plan, Task 5).
use std::path::{Path, PathBuf};

use deckle_kfx::container::read_kfx;
use deckle_kfxgen::generate::generate;
use deckle_kfxgen::load::load_model;

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

/// Fragments first (they name the first difference), then the whole file:
/// the bytes also catch symbol order and framing the fragment list can't.
pub(crate) fn assert_matches(dir: &Path, out: &deckle_kfxgen::generate::GenerateOutput) {
    let expected = dir.join("expected.kfx");
    let types = types_in(&expected);
    let types: Vec<&str> = types.iter().map(String::as_str).collect();
    assert_fragments_match(&out.fragments, &expected, &types);
    let want = std::fs::read(&expected).unwrap();
    if out.bytes != want {
        let at = out.bytes.iter().zip(&want).position(|(a, b)| a != b);
        panic!(
            "{}: bytes differ, first at offset {:?} (got {} bytes, want {})",
            dir.display(),
            at.unwrap_or(out.bytes.len().min(want.len())),
            out.bytes.len(),
            want.len()
        );
    }
}

pub(crate) fn matches_kfxgen(name: &str) {
    let dir = model_dir(name);
    let out = generate(&load_model(&dir).unwrap()).unwrap();
    assert_matches(&dir, &out);
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
fn publisher_structure_matches_kfxgen() {
    matches_kfxgen("publisher_structure");
}

/// A vertical book (#754): percentages against Previewer's 480pt page in frames, pictures
/// among words, floats, padding, cell sides and contents levels.
#[test]
fn vertical_page_matches_kfxgen() {
    matches_kfxgen("vertical_page");
}
