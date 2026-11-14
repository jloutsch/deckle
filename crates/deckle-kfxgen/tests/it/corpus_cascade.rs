// SPDX-License-Identifier: GPL-3.0-only
//! `read_book` on the Gutenberg corpus against Calibre (phase 2 plan, Task 11): the four
//! comparisons of `book::read_book_matches_every_dump` on each corpus book's cascade dump.
//! The dumps stay outside git:
//!
//! ```text
//! DECKLE_KFXGEN_CORPUS_OUT=gutenberg/models tools/ref/kfxgen_models.sh --corpus gutenberg/*.epub
//! DECKLE_KFXGEN_CORPUS=gutenberg/models \
//!   cargo test --release -p deckle-kfxgen --test it corpus_cascade -- --ignored --nocapture
//! ```
//!
//! A relative path is taken from the workspace root. The EPUBs are looked for in
//! `DECKLE_KFXGEN_CORPUS_EPUBS`, else in the corpus folder's parent (`gutenberg/`).
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use deckle_kfxgen::book::read_book;

use crate::support::{Dump, Tally, allowed, check_book, check_tools};

/// How long `read_book` may take on a book with no picture but its cover, in a release build.
const TEXT_BOOK_LIMIT: Duration = Duration::from_secs(1);

fn from_root(p: std::ffi::OsString) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(p)
}

#[test]
#[ignore = "needs DECKLE_KFXGEN_CORPUS, the folder of corpus dumps"]
fn corpus_matches_calibre() {
    let corpus =
        from_root(std::env::var_os("DECKLE_KFXGEN_CORPUS").expect("set DECKLE_KFXGEN_CORPUS"));
    let epubs = std::env::var_os("DECKLE_KFXGEN_CORPUS_EPUBS")
        .map(from_root)
        .unwrap_or_else(|| corpus.join(".."));
    check_tools(&corpus).unwrap();
    // BOOKS lists the books of a run that made them all (`--corpus`).
    let books: Vec<String> = std::fs::read_to_string(corpus.join("BOOKS"))
        .expect("the corpus has no BOOKS: its last dump failed")
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(!books.is_empty(), "no books in {}", corpus.display());
    let mut tally = Tally::default();
    let mut slow = Vec::new();
    for name in &books {
        let path = epubs.join(format!("{name}.epub"));
        let start = Instant::now();
        let book = read_book(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let took = start.elapsed();
        let pictures = book
            .oeb
            .manifest
            .iter()
            .filter(|i| i.media_type.starts_with("image/"))
            .count();
        eprintln!(
            "{name}: read_book {took:?}, {} files, {pictures} pictures, {} style units",
            book.files.len(),
            book.style_units
        );
        if pictures <= 1 && !cfg!(debug_assertions) && took >= TEXT_BOOK_LIMIT {
            slow.push(format!("{name}: {took:?}"));
        }
        let before = tally.failures.len();
        check_book(&book, &Dump::corpus(&corpus, name), &mut tally);
        eprintln!("{name}: {} mismatches", tally.failures.len() - before);
    }
    eprintln!(
        "{} books, {} files, {} book rules, {} @font-face rules, {} elements, {} values",
        tally.books.len(),
        tally.files,
        tally.rules,
        tally.font_faces,
        tally.elements,
        tally.values
    );
    tally.assert_clean(&allowed());
    assert!(slow.is_empty(), "past {TEXT_BOOK_LIMIT:?}: {slow:?}");
}
