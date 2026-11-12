// SPDX-License-Identifier: GPL-3.0-only
//! The Gutenberg corpus against kfxgen (phase 1 plan, Task 13). Its models stay outside git:
//! `DECKLE_KFXGEN_CORPUS=gutenberg/models cargo test -p deckle-kfxgen --test it corpus -- --ignored`
//! (a relative path is taken from the workspace root).
use std::path::{Path, PathBuf};

use deckle_kfxgen::generate::generate;
use deckle_kfxgen::load::load_model;

use crate::generate_skeleton::assert_matches;

/// Bytes, never `model.json`: Calibre renames pictures between runs.
#[test]
#[ignore = "needs DECKLE_KFXGEN_CORPUS, the folder of corpus models"]
fn corpus_is_byte_identical() {
    let corpus = std::env::var_os("DECKLE_KFXGEN_CORPUS").expect("set DECKLE_KFXGEN_CORPUS");
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(corpus);
    // A corpus dump that failed leaves none.
    let commit = |dir: &Path| {
        let file = dir.join("KFXGEN_COMMIT");
        std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
    };
    assert_eq!(
        commit(&corpus),
        commit(&deckle_kfx::fixtures::kfxgen("models")),
        "corpus and fixtures dumped from different kfxgen commits"
    );
    // The folder holds the books the script dumped, no fewer and no others: a book whose
    // dump failed, or one left from an earlier corpus, stops the test (issue #16).
    let mut listed: Vec<String> = std::fs::read_to_string(corpus.join("BOOKS"))
        .expect("the corpus has no BOOKS: its last dump failed, or predates the list")
        .lines()
        .map(str::to_owned)
        .collect();
    listed.sort();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let names: Vec<_> = dirs
        .iter()
        .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, listed, "the corpus folder's books, against BOOKS");
    assert!(!dirs.is_empty(), "no models in {}", corpus.display());
    for dir in dirs {
        match generate(&load_model(&dir).unwrap()) {
            Ok(out) => assert_matches(&dir, &out),
            Err(e) => panic!("{}: {e}", dir.display()),
        }
    }
}
