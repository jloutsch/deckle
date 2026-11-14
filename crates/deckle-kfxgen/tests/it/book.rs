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

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// The stored zip of an EPUB with `opf` at `content.opf` and `files`.
fn zip_of(opf: &[u8], files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut entries: Vec<(&str, &[u8], u64)> = vec![
        ("mimetype", b"application/epub+zip", 20),
        ("META-INF/container.xml", CONTAINER, CONTAINER.len() as u64),
        ("content.opf", opf, opf.len() as u64),
    ];
    entries.extend(files.iter().map(|&(n, d)| (n, d, d.len() as u64)));
    fixtures::stored_zip(&entries)
}

/// [`zip_of`] written to a temp file.
fn epub_of(opf: &[u8], files: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), zip_of(opf, files)).unwrap();
    file
}

/// An OPF whose spine is `c.xhtml`, with `items` in its manifest too.
fn opf_with(items: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>{items}</manifest><spine><itemref idref="c"/></spine></package>"#
    )
}

/// A page whose `<head>` holds `head`.
fn page_with(head: &str) -> String {
    format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title>{head}</head><body><p>Hi.</p></body></html>"#
    )
}

/// The selectors of the book's own first-pass rules in its first file.
fn book_selectors(book: &deckle_kfxgen::book::Book) -> Vec<String> {
    book.files[0]
        .first_pass_rules
        .iter()
        .filter(|r| r.origin == deckle_kfxgen::css::Origin::Book)
        .map(|r| r.selector.clone())
        .collect()
}

#[test]
fn an_opf_that_is_not_utf8_names_the_file() {
    let opf = "<?xml version=\"1.0\"?><package xmlns=\"http://www.idpf.org/2007/opf\" \
               version=\"2.0\"><metadata><dc:title xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
               Caf\u{e9}</dc:title></metadata><manifest/><spine/></package>";
    // Latin-1: é as one byte, which is not UTF-8.
    let latin1: Vec<u8> = opf.chars().map(|c| c as u8).collect();
    let file = epub_of(&latin1, &[]);
    match read_book(file.path()) {
        Err(Error::Encoding { file }) => assert_eq!(file, "content.opf"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// An OPF whose bytes fail the zip's checksum is a read error, not "not UTF-8".
#[test]
fn a_corrupt_opf_is_not_called_an_encoding_error() {
    let opf = opf_with("");
    let mut zip = zip_of(opf.as_bytes(), &[("c.xhtml", page_with("").as_bytes())]);
    // Stored, so the OPF's bytes are in the zip as written: change one, keeping it UTF-8.
    let at = zip
        .windows(opf.len())
        .position(|w| w == opf.as_bytes())
        .unwrap()
        + opf.find("<dc:title>T").unwrap()
        + "<dc:title>".len();
    zip[at] = b'U';
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), zip).unwrap();
    match read_book(file.path()) {
        Err(Error::Io(_)) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// Calibre's manifest lower-cases a media type and guesses an empty one from the extension
/// (`reader.py` `_manifest_from_opf`, as `epub::oeb_view` ports it); a file's links reach every
/// item it then holds as a stylesheet (`base.py` `OEB_STYLES`).
#[test]
fn stylesheets_are_the_items_calibres_manifest_holds_as_css() {
    let opf = opf_with(
        r#"<item id="a" href="a.css" media-type="TEXT/CSS"/><item id="b" href="b.css" media-type=""/><item id="o" href="o.css" media-type="text/x-oeb1-css"/><item id="x" href="x.css" media-type="text/plain"/>"#,
    );
    let page = page_with(
        r#"<link rel="stylesheet" href="a.css"/><link rel="stylesheet" href="b.css"/><link rel="stylesheet" href="o.css"/><link rel="stylesheet" href="x.css"/>"#,
    );
    let file = epub_of(
        opf.as_bytes(),
        &[
            ("c.xhtml", page.as_bytes()),
            ("a.css", b"p.a { color: red }"),
            ("b.css", b"p.b { color: green }"),
            ("o.css", b"p.o { color: blue }"),
            ("x.css", b"p.x { color: gray }"),
        ],
    );
    let book = read_book(file.path()).unwrap();
    assert_eq!(book_selectors(&book), ["p.a", "p.b", "p.o"]);
}

/// `stylizer.py` `Stylizer.__init__`: a `<style>`'s `type` is one of `OEB_STYLES` as written,
/// a `<link>`'s lower-cased.
#[test]
fn style_and_link_types_are_calibres() {
    let opf = opf_with(
        r#"<item id="a" href="a.css" media-type="text/css"/><item id="b" href="b.css" media-type="text/css"/>"#,
    );
    let page = page_with(
        r#"<style type="text/x-oeb1-css">p.s1 { color: red }</style><style type="TEXT/CSS">p.s2 { color: red }</style><style type="text/plain">p.s3 { color: red }</style><link rel="stylesheet" type="TEXT/X-OEB-CSS" href="a.css"/><link rel="stylesheet" type="text/plain" href="b.css"/>"#,
    );
    let file = epub_of(
        opf.as_bytes(),
        &[
            ("c.xhtml", page.as_bytes()),
            ("a.css", b"p.l1 { color: red }"),
            ("b.css", b"p.l2 { color: red }"),
        ],
    );
    let book = read_book(file.path()).unwrap();
    assert_eq!(book_selectors(&book), ["p.s1", "p.l1"]);
}

/// A stylesheet that is not UTF-8 is decoded as Calibre decodes it (`OEBBook.decode`), not
/// skipped.
#[test]
fn a_latin1_stylesheet_is_read() {
    let opf = opf_with(r#"<item id="a" href="a.css" media-type="text/css"/>"#);
    let page = page_with(r#"<link rel="stylesheet" href="a.css"/>"#);
    let css: Vec<u8> = "p.caf\u{e9} { color: red }"
        .chars()
        .map(|c| c as u8)
        .collect();
    assert!(std::str::from_utf8(&css).is_err());
    let file = epub_of(
        opf.as_bytes(),
        &[("c.xhtml", page.as_bytes()), ("a.css", &css)],
    );
    let book = read_book(file.path()).unwrap();
    assert_eq!(book_selectors(&book), ["p.caf\u{e9}"]);
}

/// The limits on a file's CSS hold for all of its stylesheets together: one large sheet
/// linked many times is refused once the file's links pass them, before the rest is read.
#[test]
fn many_links_to_a_large_sheet_are_too_large() {
    // One rule that matches nothing, 100,000 declarations: under MAX_STYLE_DECLS alone, past
    // it twice.
    let mut css = String::from("x-none {");
    for i in 0..100_000 {
        css.push_str(&format!("a{i}: 1;"));
    }
    css.push('}');
    let opf = opf_with(r#"<item id="a" href="a.css" media-type="text/css"/>"#);
    let page = page_with(&r#"<link rel="stylesheet" href="a.css"/>"#.repeat(30));
    let file = epub_of(
        opf.as_bytes(),
        &[("c.xhtml", page.as_bytes()), ("a.css", css.as_bytes())],
    );
    let start = std::time::Instant::now();
    let got = read_book(file.path());
    let took = start.elapsed();
    match got {
        Err(Error::TooLarge { what }) => assert!(what.starts_with("c.xhtml: "), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(took < crate::support::COST_LIMIT, "took {took:?}");
}

/// A spine file past a limit stops the book with the file named, never a panic.
#[test]
fn a_file_past_a_limit_is_too_large_and_named() {
    let opf = opf_with("");
    // Not well-formed, so read as HTML, and nested past `MAX_DEPTH`.
    let page = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{}</body></html>",
        "<div>".repeat(deckle_limits::MAX_DEPTH * 2)
    );
    let file = epub_of(opf.as_bytes(), &[("c.xhtml", page.as_bytes())]);
    match read_book(file.path()) {
        Err(Error::TooLarge { what }) => assert!(what.starts_with("c.xhtml: "), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// A page of `n` empty elements.
fn elements_page(n: usize) -> String {
    format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head><body>{}</body></html>"#,
        "<a/>".repeat(n)
    )
}

/// A file of more nodes than `MAX_DOM_NODES` is refused while it is parsed, the file named.
#[test]
fn a_file_of_too_many_elements_is_too_large() {
    let page = elements_page(deckle_limits::MAX_DOM_NODES + 1);
    let file = epub_of(opf_with("").as_bytes(), &[("c.xhtml", page.as_bytes())]);
    let start = std::time::Instant::now();
    match read_book(file.path()) {
        Err(Error::TooLarge { what }) => assert!(what.starts_with("c.xhtml: "), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
}

/// Files each under `MAX_DOM_NODES` that together pass `MAX_BOOK_DOM_NODES` are refused.
#[test]
fn a_book_of_too_many_elements_is_too_large() {
    let per_file = deckle_limits::MAX_DOM_NODES * 9 / 10;
    let files = deckle_limits::MAX_BOOK_DOM_NODES / per_file + 1;
    let opf = opf_of_files(files, "");
    let page = elements_page(per_file);
    let names: Vec<String> = (0..files).map(|i| format!("f{i}.xhtml")).collect();
    let small = page_with("");
    let mut entries: Vec<(&str, &[u8])> = vec![("c.xhtml", small.as_bytes())];
    entries.extend(names.iter().map(|n| (n.as_str(), page.as_bytes())));
    let file = epub_of(opf.as_bytes(), &entries);
    let start = std::time::Instant::now();
    match read_book(file.path()) {
        Err(Error::TooLarge { what }) => assert!(what.contains("book"), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
}

/// An OPF whose spine is `c.xhtml` then `f0.xhtml` … `f{n-1}.xhtml`, with `items` too.
fn opf_of_files(n: usize, items: &str) -> String {
    let files: String = (0..n)
        .map(|i| {
            format!(r#"<item id="f{i}" href="f{i}.xhtml" media-type="application/xhtml+xml"/>"#)
        })
        .collect();
    let spine: String = (0..n)
        .map(|i| format!(r#"<itemref idref="f{i}"/>"#))
        .collect();
    opf_with(&format!("{files}{items}")).replace(
        r#"<itemref idref="c"/>"#,
        &format!(r#"<itemref idref="c"/>{spine}"#),
    )
}

/// Every file's first-pass rules are kept until the book is done: they count against the
/// book's limit, so files each linking a large sheet that matches nothing are refused.
#[test]
fn first_pass_rules_count_against_the_book_limit() {
    // One rule of 120,000 declarations: under the file's limits, a 25th of the book's.
    let mut css = String::from("x-none {");
    for i in 0..120_000 {
        css.push_str(&format!("a{i}:1;"));
    }
    css.push('}');
    let files = MAX_BOOK_CASCADE_DECLS / 120_000 + 1;
    let opf = opf_of_files(
        files,
        r#"<item id="a" href="a.css" media-type="text/css"/>"#,
    );
    let page = page_with(r#"<link rel="stylesheet" href="a.css"/>"#);
    let names: Vec<String> = (0..files).map(|i| format!("f{i}.xhtml")).collect();
    let mut entries: Vec<(&str, &[u8])> =
        vec![("c.xhtml", page.as_bytes()), ("a.css", css.as_bytes())];
    entries.extend(names.iter().map(|n| (n.as_str(), page.as_bytes())));
    let file = epub_of(opf.as_bytes(), &entries);
    let start = std::time::Instant::now();
    match read_book(file.path()) {
        Err(Error::TooLarge { what }) => assert!(what.contains("this book"), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
}

/// A class's name is written into the `class` attribute of every element that has it: each
/// copy counts against the file's limit, so one long name on many elements is refused.
#[test]
fn a_long_class_name_on_many_elements_is_too_large() {
    let name = "k".repeat(100 * 1024);
    let page = format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head><body><p class="{name}">x</p>{}</body></html>"#,
        "<p>x</p>".repeat(2_000)
    );
    let file = epub_of(opf_with("").as_bytes(), &[("c.xhtml", page.as_bytes())]);
    let start = std::time::Instant::now();
    match read_book(file.path()) {
        Err(Error::TooLarge { what }) => assert!(what.starts_with("c.xhtml: "), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
}

/// Read as HTML, each repeated `<html>` start tag adds its attributes to the root: the
/// attributes already there that each is compared with count against the file's budget, so
/// many such tags stop instead of taking time that grows with their square.
#[test]
fn many_html_start_tags_stop() {
    let tags: String = (0..30_000).map(|i| format!("<html a{i}=1>")).collect();
    // Not XML: the paragraph is not closed.
    let page = format!("<html><head><title>t</title></head><body><p>unclosed{tags}");
    let file = epub_of(opf_with("").as_bytes(), &[("c.xhtml", page.as_bytes())]);
    let start = std::time::Instant::now();
    match read_book(file.path()) {
        Err(Error::TooLarge { what }) => assert!(what.starts_with("c.xhtml: "), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
}

/// Calibre keeps hrefs in their quoted form (`My%20Book/c.xhtml`) and finds a stylesheet a
/// link or a `<style>`'s `@import` names relative to it (`stylizer.py` `Stylizer.__init__`,
/// `Manifest.Item.abshref`); so does the port, in a folder whose name needs quoting.
#[test]
fn stylesheets_are_found_in_a_folder_that_needs_quoting() {
    let container = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="My Book/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let opf = opf_with(
        r#"<item id="a" href="a.css" media-type="text/css"/><item id="b" href="b.css" media-type="text/css"/>"#,
    );
    let page = page_with(r#"<link rel="stylesheet" href="a.css"/><style>@import "b.css";</style>"#);
    let entries: Vec<(&str, &[u8])> = vec![
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", container),
        ("My Book/content.opf", opf.as_bytes()),
        ("My Book/c.xhtml", page.as_bytes()),
        ("My Book/a.css", b"p.a { color: red }"),
        ("My Book/b.css", b"p.b { color: red }"),
    ];
    let entries: Vec<(&str, &[u8], u64)> = entries
        .into_iter()
        .map(|(n, d)| (n, d, d.len() as u64))
        .collect();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), fixtures::stored_zip(&entries)).unwrap();
    let book = read_book(file.path()).unwrap();
    assert_eq!(book.files[0].href, "My%20Book/c.xhtml");
    assert_eq!(book_selectors(&book), ["p.a", "p.b"]);
}

/// Each link and import is looked up among the manifest's stylesheets in constant time: many
/// stylesheet items and many links to sheets not listed finish in time.
#[test]
fn many_stylesheet_items_and_links_finish() {
    const N: usize = 100_000;
    let pre = "a-long-shared-prefix-for-every-stylesheet-name-";
    let items: String = (0..N)
        .map(|i| format!(r#"<item id="s{i}" href="{pre}{i}.css" media-type="text/css"/>"#))
        .collect();
    let links: String = (0..N)
        .map(|i| format!(r#"<link rel="stylesheet" href="{pre}x{i}.css"/>"#))
        .collect();
    let opf = opf_with(&items);
    let page = page_with(&links);
    let file = epub_of(opf.as_bytes(), &[("c.xhtml", page.as_bytes())]);
    let start = std::time::Instant::now();
    let book = read_book(file.path()).unwrap();
    let took = start.elapsed();
    assert!(book_selectors(&book).is_empty());
    assert!(took < crate::support::COST_LIMIT, "{took:?}");
}

/// `stylizer.py` `Stylizer.__init__` reads `{xhtml}style`, `{svg}style` and `{xhtml}link`
/// only: XML names are case-sensitive, and a `style` in another namespace is not CSS.
#[test]
fn style_elements_are_calibres() {
    let opf = opf_with(r#"<item id="a" href="a.css" media-type="text/css"/>"#);
    let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><STYLE>p.upper { color: red }</STYLE><LINK rel="stylesheet" href="a.css"/><x:style xmlns:x="urn:x">p.other { color: red }</x:style></head><body><svg xmlns="http://www.w3.org/2000/svg"><style>p.svg { color: red }</style></svg><p>Hi.</p></body></html>"#;
    let file = epub_of(
        opf.as_bytes(),
        &[
            ("c.xhtml", page.as_bytes()),
            ("a.css", b"p.link { color: red }"),
        ],
    );
    let book = read_book(file.path()).unwrap();
    assert_eq!(book_selectors(&book), ["p.svg"]);
}
