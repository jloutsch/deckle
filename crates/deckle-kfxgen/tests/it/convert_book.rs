// SPDX-License-Identifier: GPL-3.0-only
//! `convert`, every stage wired in `convert_oeb_to_kfx`'s order (plan Task 23): each source
//! fixture's whole model, and the KFX the generator makes from it.

use std::path::Path;

use deckle_kfx::fixtures;
use deckle_kfxgen::book::manifest_bytes;
use deckle_kfxgen::convert::{ConvertOptions, Converted, convert};
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::{Model, load_model};
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_args::sources;
use crate::convert_images::compare;
use crate::convert_support::model_diff;
use crate::whole_book::NOT_PORTED;

/// The arguments that hold pictures.
const PICTURES: [&str; 2] = ["images", "cover_image"];

fn without_pictures(args: &PyValue) -> PyValue {
    let PyValue::Dict(pairs) = args else {
        panic!("args: not a dict")
    };
    let kept = pairs
        .iter()
        .filter(|(k, _)| !k.as_str().is_some_and(|k| PICTURES.contains(&k)));
    PyValue::Dict(kept.cloned().collect())
}

/// Where `ours` differs from `want`, kfxgen's arguments for the EPUB at `path`: every
/// argument by `model_diff` but the pictures, which follow Task 18's rule (`compare`): the
/// same hrefs, each picture kfxgen kept as the book has it byte for byte, each it made of the
/// same format, size and JPEG components. The pictures' order is not compared: Calibre lists
/// a book's pictures in another order from run to run.
pub(crate) fn args_diff(ours: &PyValue, want: &PyValue, path: &Path) -> Vec<String> {
    let mut out = model_diff(&without_pictures(ours), &without_pictures(want));
    let pictures = |args: &PyValue| -> Vec<(String, Vec<u8>)> {
        let Some(PyValue::Dict(pairs)) = args.get("images") else {
            panic!("images: not a dict")
        };
        let mut v: Vec<(String, Vec<u8>)> = pairs
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().unwrap().to_owned(),
                    v.as_bytes().unwrap().to_vec(),
                )
            })
            .collect();
        v.sort();
        v
    };
    let (mine, theirs) = (pictures(ours), pictures(want));
    let hrefs = |v: &[(String, Vec<u8>)]| v.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>();
    if hrefs(&mine) != hrefs(&theirs) {
        out.push(format!(
            "[\"images\"]: {:?} where kfxgen has {:?}",
            hrefs(&mine),
            hrefs(&theirs)
        ));
    } else {
        for ((href, a), (_, b)) in mine.iter().zip(&theirs) {
            let raw = manifest_bytes(path, href).unwrap();
            out.extend(compare(&format!("[\"images\"][{href:?}]"), a, b, &raw));
        }
    }
    match (
        ours.get("cover_image").and_then(PyValue::as_bytes),
        want.get("cover_image").and_then(PyValue::as_bytes),
    ) {
        (None, None) => {}
        (Some(a), Some(b)) => {
            // The cover is one of the book's pictures, or made from one.
            let raw = (mine.iter().chain(&theirs))
                .map(|(h, _)| manifest_bytes(path, h).unwrap())
                .find(|r| r == b)
                .unwrap_or_default();
            out.extend(compare("[\"cover_image\"]", a, b, &raw));
        }
        (a, b) => out.push(format!(
            "[\"cover_image\"]: {} where kfxgen has {}",
            a.is_some(),
            b.is_some()
        )),
    }
    out
}

/// Whether the two models hold the same picture bytes.
fn same_pictures(ours: &PyValue, want: &PyValue) -> bool {
    let bytes = |args: &PyValue| {
        let mut v: Vec<(String, Vec<u8>)> = match args.get("images") {
            Some(PyValue::Dict(pairs)) => pairs
                .iter()
                .map(|(k, v)| (format!("{k:?}"), v.as_bytes().unwrap().to_vec()))
                .collect(),
            _ => Vec::new(),
        };
        v.sort();
        (
            v,
            args.get("cover_image")
                .and_then(PyValue::as_bytes)
                .map(<[u8]>::to_vec),
        )
    };
    bytes(ours) == bytes(want)
}

/// `ours` with `want`'s pictures, for a book whose pictures kfxgen made (re-encoded): the
/// rest of the book can still be compared byte for byte.
fn with_pictures_of(ours: &Model, want: &Model) -> Model {
    let PyValue::Dict(pairs) = &ours.args else {
        panic!("args: not a dict")
    };
    let pairs = pairs
        .iter()
        .map(|(k, v)| match k.as_str() {
            Some(name) if PICTURES.contains(&name) => {
                (k.clone(), want.args.get(name).unwrap().clone())
            }
            _ => (k.clone(), v.clone()),
        })
        .collect();
    Model {
        args: PyValue::Dict(pairs),
        ..ours.clone()
    }
}

/// Where the generator's bytes first differ from `expected.kfx` in `dir`, if they do.
fn kfx_diff(bytes: &[u8], dir: &Path) -> Option<String> {
    let want = std::fs::read(dir.join("expected.kfx")).unwrap();
    (bytes != want).then(|| {
        let at = bytes.iter().zip(&want).position(|(a, b)| a != b);
        format!(
            "bytes differ, first at offset {} (got {} bytes, want {})",
            at.unwrap_or(bytes.len().min(want.len())),
            bytes.len(),
            want.len()
        )
    })
}

/// How a converted book's KFX compared with kfxgen's.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Kfx {
    /// Byte for byte.
    Identical,
    /// Byte for byte once kfxgen's own pictures replace the ones made here.
    IdenticalWithKfxgensPictures,
    /// The generator refuses it, naming the feature.
    Unsupported(&'static str),
}

/// The KFX of `ours` against kfxgen's `expected.kfx` in `dir`: Err with where they differ.
pub(crate) fn check_kfx(ours: &Model, want: &Model, dir: &Path) -> Result<Kfx, String> {
    let out = match generate(ours) {
        Ok(out) => out,
        Err(GenerateError::Unsupported(f)) => return Ok(Kfx::Unsupported(f)),
        Err(e) => return Err(e.to_string()),
    };
    let Some(diff) = kfx_diff(&out.bytes, dir) else {
        return Ok(Kfx::Identical);
    };
    if same_pictures(&ours.args, &want.args) {
        return Err(diff);
    }
    let out = generate(&with_pictures_of(ours, want)).map_err(|e| e.to_string())?;
    match kfx_diff(&out.bytes, dir) {
        None => Ok(Kfx::IdenticalWithKfxgensPictures),
        Some(d) => Err(format!("with kfxgen's pictures, {d}")),
    }
}

fn converted(name: &str) -> (std::path::PathBuf, Converted) {
    let path = fixtures::kfxgen(&format!("sources/{name}.epub"));
    let got = convert(&path, &ConvertOptions::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
    (path, got)
}

/// For every source fixture, `convert` gives the model kfxgen gave: the whole argument tree,
/// pictures by Task 18's rule.
#[test]
fn convert_matches_every_model() {
    let names = sources();
    assert_eq!(names.len(), 55, "the source fixtures");
    let mut failures = Vec::new();
    for name in &names {
        let (path, got) = converted(name);
        let want = load_model(&fixtures::kfxgen(&format!("models/{name}")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        for line in args_diff(&got.model.args, &want.args, &path) {
            failures.push(format!("{name}: {line}"));
        }
    }
    for f in failures.iter().take(60) {
        eprintln!("{f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
}

/// For every source fixture, the generator makes kfxgen's KFX from `convert`'s model, byte
/// for byte: as it is where every picture passes through, with kfxgen's pictures where kfxgen
/// made some (`pictures_more`'s GIF, CMYK JPEG and oversize PNG). The four books the
/// generator does not port yet are refused by name.
#[test]
fn convert_then_generate_matches_expected_kfx() {
    let mut failures = Vec::new();
    let mut swapped = Vec::new();
    let mut identical = 0;
    for name in &sources() {
        let (_, got) = converted(name);
        let dir = fixtures::kfxgen(&format!("models/{name}"));
        let want = load_model(&dir).unwrap();
        let refused = NOT_PORTED.iter().find(|(n, _)| n == name).map(|(_, f)| *f);
        match (check_kfx(&got.model, &want, &dir), refused) {
            (Ok(Kfx::Identical), None) => identical += 1,
            (Ok(Kfx::IdenticalWithKfxgensPictures), None) => swapped.push(name.clone()),
            (Ok(Kfx::Unsupported(f)), Some(r)) if f == r => {}
            (got, _) => failures.push(format!("{name}: {got:?}, refused {refused:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(swapped, ["pictures_more"]);
    assert_eq!(identical, 55 - 1 - NOT_PORTED.len());
}
