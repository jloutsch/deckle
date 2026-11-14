// SPDX-License-Identifier: GPL-3.0-only
//! `read_book` (phase 2 plan Task 11): the whole of phase 2 behind one call, against every
//! cascade fixture's dump at four levels (book view, element tree, declared styles, computed
//! values).

use deckle_kfx::fixtures;
use deckle_kfxgen::book::{Error, manifest_bytes, read_book};
use deckle_kfxgen::css::DEFAULT_PROFILE;
use deckle_limits::MAX_BOOK_CASCADE_DECLS;

use crate::support::{BOOKS, Dump, Tally, allowed, check_book, check_tools};

fn source(name: &str) -> std::path::PathBuf {
    fixtures::kfxgen(&format!("sources/{name}.epub"))
}

#[test]
fn read_book_matches_every_dump() {
    check_tools(&fixtures::kfxgen("cascade")).unwrap();
    let mut tally = Tally::default();
    let mut most_units = 0;
    for name in BOOKS {
        let book = read_book(&source(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        most_units = most_units.max(book.style_units);
        check_book(&book, &Dump::fixture(name), &mut tally);
    }
    tally.assert_clean(&allowed());
    assert_eq!(tally.books.len(), 13);
    assert_eq!(
        (tally.files, tally.rules, tally.font_faces),
        (49, 306, 3),
        "files, book rules, @font-face rules"
    );
    assert!(tally.marks >= 3, "{} marks", tally.marks);
    assert!(tally.elements > 1_000, "{} elements", tally.elements);
    assert!(tally.values > 30_000, "{} values", tally.values);
    // The book-wide limit stays far above what the fixture books use.
    eprintln!("the most units a fixture book used: {most_units}");
    assert!(most_units * 10 <= MAX_BOOK_CASCADE_DECLS, "{most_units}");
}

#[test]
fn read_book_twice_is_identical() {
    for name in BOOKS {
        let first = format!("{:?}", read_book(&source(name)).unwrap());
        let second = format!("{:?}", read_book(&source(name)).unwrap());
        assert!(first == second, "{name}: two reads differ");
    }
}

#[test]
fn dump_from_another_calibre_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let tools = std::fs::read_to_string(fixtures::kfxgen("cascade/TOOLS")).unwrap();
    std::fs::write(dir.path().join("TOOLS"), &tools).unwrap();
    check_tools(dir.path()).unwrap();

    let other = tools.replace("(calibre 9.18.0)", "(calibre 9.17.0)");
    assert_ne!(other, tools);
    std::fs::write(dir.path().join("TOOLS"), other).unwrap();
    let err = check_tools(dir.path()).unwrap_err();
    assert!(err.contains("9.18.0"), "{err}");

    let local_time = tools.replace("TZ: UTC\n", "");
    assert_ne!(local_time, tools);
    std::fs::write(dir.path().join("TOOLS"), local_time).unwrap();
    assert!(check_tools(dir.path()).unwrap_err().contains("UTC"));

    std::fs::remove_file(dir.path().join("TOOLS")).unwrap();
    assert!(check_tools(dir.path()).is_err());
}

#[test]
fn styles_of_a_file_answer_as_calibre() {
    let book = read_book(&source("css_quirks")).unwrap();
    let f = book
        .files
        .iter()
        .find(|f| f.href == "OEBPS/values.xhtml")
        .unwrap();
    let styles = f.styles(&DEFAULT_PROFILE);
    let body = crate::support::find(&f.dom, &[1]).unwrap();
    // `stylize_spine` gives <body> kfxgen's 5pt side margins.
    assert_eq!(styles.get(body, "margin-left"), Some("5pt"));
    // A property Calibre has no default for is the book error kfxgen's KeyError becomes.
    let err: Error = styles
        .inherited(body, "no-such-property")
        .unwrap_err()
        .into();
    assert!(
        matches!(&err, Error::NoDefault { prop } if prop == "no-such-property"),
        "{err:?}"
    );
}

#[test]
fn font_faces_are_every_files_in_spine_order() {
    let book = read_book(&source("css_quirks")).unwrap();
    let families: Vec<&str> = book
        .font_face
        .iter()
        .map(|f| {
            f.iter()
                .find(|(k, _)| k == "font-family")
                .map(|(_, d)| d.value.as_str())
                .unwrap()
        })
        .collect();
    assert_eq!(families, ["\"Two\"", "\"Quirk\""]);
}

#[test]
fn manifest_bytes_reads_a_file_of_the_book() {
    let path = source("css_quirks");
    let png = manifest_bytes(&path, "OEBPS/pic.png").unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
    assert!(matches!(
        manifest_bytes(&path, "OEBPS/none.png"),
        Err(Error::Zip(_))
    ));
}

#[test]
fn a_file_that_is_not_an_epub_is_an_error() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"not a zip").unwrap();
    assert!(matches!(read_book(file.path()), Err(Error::Zip(_))));
    assert!(matches!(
        read_book(std::path::Path::new("/no/such/book.epub")),
        Err(Error::Io(_))
    ));
}

#[test]
fn an_opf_that_is_not_utf8_names_the_file() {
    let opf = "<?xml version=\"1.0\"?><package xmlns=\"http://www.idpf.org/2007/opf\" \
               version=\"2.0\"><metadata><dc:title xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
               Caf\u{e9}</dc:title></metadata><manifest/><spine/></package>";
    // Latin-1: é as one byte, which is not UTF-8.
    let latin1: Vec<u8> = opf.chars().map(|c| c as u8).collect();
    const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let epub = fixtures::stored_zip(&[
        ("mimetype", b"application/epub+zip", 20),
        ("META-INF/container.xml", CONTAINER, CONTAINER.len() as u64),
        ("content.opf", &latin1, latin1.len() as u64),
    ]);
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), epub).unwrap();
    match read_book(file.path()) {
        Err(Error::Encoding { file }) => assert_eq!(file, "content.opf"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// Calibre's manifest lower-cases a media type and guesses an empty one from the extension
/// (`reader.py` `_manifest_from_opf`, as `epub::oeb_view` ports it); a file's links reach every
/// item it then holds as a stylesheet (`base.py` `OEB_STYLES`).
#[test]
fn stylesheets_are_the_items_calibres_manifest_holds_as_css() {
    const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let opf = br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/><item id="a" href="a.css" media-type="TEXT/CSS"/><item id="b" href="b.css" media-type=""/><item id="o" href="o.css" media-type="text/x-oeb1-css"/><item id="x" href="x.css" media-type="text/plain"/></manifest><spine><itemref idref="c"/></spine></package>"#;
    let page = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><link rel="stylesheet" href="a.css"/><link rel="stylesheet" href="b.css"/><link rel="stylesheet" href="o.css"/><link rel="stylesheet" href="x.css"/></head><body><p>Hi.</p></body></html>"#;
    let entries: Vec<(&str, &[u8])> = vec![
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", CONTAINER),
        ("content.opf", opf),
        ("c.xhtml", page),
        ("a.css", b"p.a { color: red }"),
        ("b.css", b"p.b { color: green }"),
        ("o.css", b"p.o { color: blue }"),
        ("x.css", b"p.x { color: gray }"),
    ];
    let entries: Vec<(&str, &[u8], u64)> = entries
        .into_iter()
        .map(|(n, d)| (n, d, d.len() as u64))
        .collect();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), fixtures::stored_zip(&entries)).unwrap();
    let book = read_book(file.path()).unwrap();
    let selectors: Vec<&str> = book.files[0]
        .first_pass_rules
        .iter()
        .filter(|r| r.origin == deckle_kfxgen::css::Origin::Book)
        .map(|r| r.selector.as_str())
        .collect();
    assert_eq!(selectors, ["p.a", "p.b", "p.o"]);
}

/// A spine file past a limit stops the book with the file named, never a panic.
#[test]
fn a_file_past_a_limit_is_too_large_and_named() {
    const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let opf = br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="c" href="deep.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c"/></spine></package>"#;
    // Not well-formed, so read as HTML, and nested past `MAX_DEPTH`.
    let page = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{}</body></html>",
        "<div>".repeat(deckle_limits::MAX_DEPTH * 2)
    );
    let entries: Vec<(&str, &[u8], u64)> = [
        ("mimetype", b"application/epub+zip".as_slice()),
        ("META-INF/container.xml", CONTAINER),
        ("content.opf", opf),
        ("deep.xhtml", page.as_bytes()),
    ]
    .into_iter()
    .map(|(n, d)| (n, d, d.len() as u64))
    .collect();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), fixtures::stored_zip(&entries)).unwrap();
    match read_book(file.path()) {
        Err(Error::TooLarge { what }) => assert!(what.starts_with("deep.xhtml: "), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}
