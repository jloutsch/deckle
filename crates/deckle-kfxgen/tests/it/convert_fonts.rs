// SPDX-License-Identifier: GPL-3.0-only
//! The book's embedded fonts as kfxgen hands them to `generate_full_book` (plan Task 19):
//! against every model, and on hostile fonts.

use std::path::{Path, PathBuf};

use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::convert::{self, ConvertOptions, Error};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_args::{dict, sources};
use crate::convert_support::model_diff;

const MIB: usize = 1024 * 1024;

#[test]
fn font_tables_match_every_model() {
    let mut failures = Vec::new();
    let mut faces = 0;
    for name in sources() {
        let path = fixtures::kfxgen(&format!("sources/{name}.epub"));
        let book = read_book(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let model = load_model(&fixtures::kfxgen(&format!("models/{name}")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let want = dict(&model.args)["font_table"];
        match convert::font_table(&book, &path, &ConvertOptions::default()) {
            Ok(ours) => {
                failures.extend(
                    model_diff(&ours, want)
                        .into_iter()
                        .map(|l| format!("{name}: {l}")),
                );
                faces += ours
                    .get("faces")
                    .and_then(PyValue::as_list)
                    .map_or(0, <[_]>::len);
            }
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // `fonts` (a TTF) and `fonts_woff` (a WOFF unpacked as fontTools writes it).
    assert_eq!(faces, 2);
}

// ---------------------------------------------------------------------------------------
// Books made in the test
// ---------------------------------------------------------------------------------------

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// An EPUB at `dir/book.epub` whose page links `s.css` holding `css`, with `fonts` in its
/// manifest and archive (`(href, bytes)`, each of media type `font/ttf`) and `extra` in its
/// archive only.
fn font_book(dir: &Path, css: &str, fonts: &[(&str, &[u8])], extra: &[(&str, &[u8])]) -> PathBuf {
    let items: String = fonts
        .iter()
        .enumerate()
        .map(|(i, (href, _))| format!(r#"<item id="f{i}" href="{href}" media-type="font/ttf"/>"#))
        .collect();
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/><item id="s" href="s.css" media-type="text/css"/>{items}</manifest><spine><itemref idref="c"/></spine></package>"#
    );
    let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><link rel="stylesheet" type="text/css" href="s.css"/></head><body><p>Hi.</p></body></html>"#;
    let mut entries: Vec<(&str, &[u8], u64)> = vec![
        ("mimetype", b"application/epub+zip", 20),
        ("META-INF/container.xml", CONTAINER, CONTAINER.len() as u64),
        ("content.opf", opf.as_bytes(), opf.len() as u64),
        ("c.xhtml", page.as_bytes(), page.len() as u64),
        ("s.css", css.as_bytes(), css.len() as u64),
    ];
    entries.extend(
        fonts
            .iter()
            .chain(extra)
            .map(|&(n, d)| (n, d, d.len() as u64)),
    );
    let path = dir.join("book.epub");
    std::fs::write(&path, fixtures::stored_zip(&entries)).unwrap();
    path
}

fn table_of(path: &Path) -> Result<PyValue, Error> {
    let book = read_book(path).unwrap();
    convert::font_table(&book, path, &ConvertOptions::default())
}

/// The faces' `(css_family, data)`.
fn faces(table: &PyValue) -> Vec<(String, Vec<u8>)> {
    table
        .get("faces")
        .and_then(PyValue::as_list)
        .unwrap()
        .iter()
        .map(|f| {
            (
                f.get("css_family")
                    .and_then(PyValue::as_str)
                    .unwrap()
                    .to_owned(),
                f.get("data").and_then(PyValue::as_bytes).unwrap().to_vec(),
            )
        })
        .collect()
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// A TrueType font of `tables` (tag, data), each where its directory says, with what
/// `_check_sfnt` asks for when `tables` is empty. `pad` zero bytes go after the last table.
fn sfnt(tables: &[(&[u8; 4], Vec<u8>)], pad: usize) -> Vec<u8> {
    let default: Vec<(&[u8; 4], Vec<u8>)>;
    let tables = if tables.is_empty() {
        default = [b"cmap", b"glyf", b"head", b"hhea", b"hmtx", b"maxp"]
            .into_iter()
            .enumerate()
            .map(|(i, t)| (t, vec![i as u8 + 1; 54]))
            .collect();
        &default[..]
    } else {
        tables
    };
    let mut out = vec![0, 1, 0, 0];
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([0; 6]);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in tables {
        out.extend(*tag);
        out.extend(checksum(data).to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend(body);
    out.resize(out.len() + pad, 0);
    out
}

#[test]
fn woff2_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut woff2 = b"wOF2\x00\x01\x00\x00".to_vec();
    woff2.resize(64, 0);
    let css = r#"@font-face { font-family: "Two"; src: url(two.woff2) format("woff2") }"#;
    let path = font_book(dir.path(), css, &[("two.woff2", &woff2)], &[]);
    let err = table_of(&path).unwrap_err();
    assert!(
        matches!(&err, Error::Unsupported { feature: "WOFF2 fonts", file } if file == "two.woff2"),
        "{err:?}"
    );
    // A TTF given for the same face first is chosen, and the WOFF2 never read.
    let css = r#"@font-face { font-family: "Two"; src: url(two.woff2), url(one.ttf) }"#;
    let ttf = sfnt(&[], 0);
    let path = font_book(
        dir.path(),
        css,
        &[("two.woff2", &woff2), ("one.ttf", &ttf)],
        &[],
    );
    assert_eq!(faces(&table_of(&path).unwrap()), [("two".to_owned(), ttf)]);
}

#[test]
fn a_font_past_50_mib_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let big = sfnt(&[], 50 * MIB + 1 - sfnt(&[], 0).len());
    assert_eq!(big.len(), 50 * MIB + 1);
    let small = sfnt(&[], 4);
    let css = r#"@font-face { font-family: "Big"; src: url(big.ttf), url(small.ttf) }
        @font-face { font-family: "Only big"; src: url(big.ttf) }"#;
    let path = font_book(
        dir.path(),
        css,
        &[("big.ttf", &big), ("small.ttf", &small)],
        &[],
    );
    assert_eq!(
        faces(&table_of(&path).unwrap()),
        [("big".to_owned(), small)]
    );
}
