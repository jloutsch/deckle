// SPDX-License-Identifier: GPL-3.0-only
//! `convert` on the Gutenberg corpus against kfxgen (phase 3 plan, Task 23): each book's
//! model, its KFX, and how long it takes. The recorded models and EPUBs stay outside git:
//!
//! ```text
//! DECKLE_KFXGEN_CORPUS=gutenberg/models DECKLE_KFXGEN_CORPUS_EPUBS=gutenberg \
//!   cargo test --release -p deckle-kfxgen --test it corpus_convert -- --ignored --nocapture
//! ```
//!
//! A relative path is taken from the workspace root. The EPUBs are looked for in
//! `DECKLE_KFXGEN_CORPUS_EPUBS`, else in the corpus folder's parent (`gutenberg/`).
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use deckle_kfxgen::convert::{ConvertOptions, convert};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_book::{Kfx, args_diff, check_kfx};
use crate::support::check_tools;

/// How long `convert` and the generator may take on a book with no picture but its cover, in
/// a release build.
const TEXT_BOOK_LIMIT: Duration = Duration::from_secs(2);
/// How long the nine books may take together, in a release build.
const CORPUS_LIMIT: Duration = Duration::from_secs(39);

fn from_root(p: std::ffi::OsString) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(p)
}

#[test]
#[ignore = "needs DECKLE_KFXGEN_CORPUS, the folder of corpus models"]
fn corpus_matches_kfxgen() {
    let corpus =
        from_root(std::env::var_os("DECKLE_KFXGEN_CORPUS").expect("set DECKLE_KFXGEN_CORPUS"));
    let epubs = std::env::var_os("DECKLE_KFXGEN_CORPUS_EPUBS")
        .map(from_root)
        .unwrap_or_else(|| corpus.join(".."));
    check_tools(&corpus).unwrap();
    let commit = |dir: &Path| std::fs::read_to_string(dir.join("KFXGEN_COMMIT")).unwrap();
    assert_eq!(
        commit(&corpus),
        commit(&deckle_kfx::fixtures::kfxgen("models")),
        "corpus and fixtures recorded from different kfxgen commits"
    );
    let books: Vec<String> = std::fs::read_to_string(corpus.join("BOOKS"))
        .expect("the corpus has no BOOKS: its last dump failed")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(books.len(), 9, "the corpus books");
    let (mut failures, mut slow, mut total) = (Vec::new(), Vec::new(), Duration::ZERO);
    for name in &books {
        let path = epubs.join(format!("{name}.epub"));
        let dir = corpus.join(name);
        let want = load_model(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        let start = Instant::now();
        let got = match convert(&path, &ConvertOptions::default()) {
            Ok(got) => got,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        let converted = start.elapsed();
        let diffs = args_diff(&got.model.args, &want.args, &path);
        let start = Instant::now();
        let kfx = check_kfx(&got.model, &want, &dir);
        // `check_kfx` generates a second time only when kfxgen made pictures.
        let generated = start.elapsed();
        let took = converted + generated;
        total += took;
        let pictures = match want.args.get("images") {
            Some(PyValue::Dict(p)) => p.len(),
            _ => 0,
        };
        eprintln!(
            "{name}: convert {converted:?}, generate {generated:?}, {pictures} pictures, {} \
             chapters, {} model differences, KFX {kfx:?}, {} warnings",
            got.model
                .args
                .get("chapters")
                .and_then(PyValue::as_list)
                .map_or(0, <[_]>::len),
            diffs.len(),
            got.warnings.len()
        );
        for w in &got.warnings {
            eprintln!("{name}: warning: {w}");
        }
        failures.extend(diffs.iter().take(20).map(|d| format!("{name}: {d}")));
        match kfx {
            Ok(Kfx::Identical | Kfx::IdenticalWithKfxgensPictures) => {}
            other => failures.push(format!("{name}: KFX {other:?}")),
        }
        if pictures <= 1 && !cfg!(debug_assertions) && took >= TEXT_BOOK_LIMIT {
            slow.push(format!("{name}: {took:?}"));
        }
    }
    eprintln!("all nine: {total:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(slow.is_empty(), "past {TEXT_BOOK_LIMIT:?}: {slow:?}");
    if !cfg!(debug_assertions) {
        assert!(total < CORPUS_LIMIT, "all nine took {total:?}");
    }
}
