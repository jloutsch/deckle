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
use crate::support::allocated_by;
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

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// An EPUB 2 book at `path` of `a.xhtml` (a heading, a paragraph, an SVG drawing a word, an
/// audio clip, a table, a picture `x.png` that holds text), a stylesheet listed in the spine,
/// and `b.xhtml`, whose one paragraph kfxgen raises on (`text-align: 5px`); its NCX names
/// `a.xhtml`, an id `a.xhtml` lacks, and `b.xhtml`; `a.xhtml` has a read-aloud narration.
fn book_kfxgen_warns_about(path: &Path) {
    let opf = r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml" media-overlay="smil"/><item id="b" href="b.xhtml" media-type="application/xhtml+xml"/><item id="css" href="s.css" media-type="text/css"/><item id="x" href="x.png" media-type="image/png"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="a"/><itemref idref="css"/><itemref idref="b"/></spine></package>"#;
    let point = |n: usize, label: &str, src: &str| {
        format!(
            r#"<navPoint id="p{n}" playOrder="{n}"><navLabel><text>{label}</text></navLabel><content src="{src}"/></navPoint>"#
        )
    };
    let ncx = format!(
        r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head><meta name="dtb:uid" content="x"/></head><docTitle><text>T</text></docTitle><navMap>{}{}{}</navMap></ncx>"#,
        point(1, "One", "a.xhtml"),
        point(2, "Gone", "a.xhtml#nowhere"),
        point(3, "Two", "b.xhtml"),
    );
    let a = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head><body><h1>One</h1><p>Text.</p><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text x="0" y="5">Hi</text></svg><audio src="x.mp3">Fallback.</audio><table><tr><td>a</td><td>b</td></tr></table><p><img src="x.png" alt=""/></p></body></html>"#;
    let b = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><link rel="stylesheet" type="text/css" href="s.css"/></head><body><p class="a">x</p></body></html>"#;
    let css = ".a { text-align: 5px }";
    let entries: Vec<(&str, &[u8], u64)> = vec![
        ("mimetype", b"application/epub+zip", 20),
        ("META-INF/container.xml", CONTAINER, CONTAINER.len() as u64),
        ("content.opf", opf.as_bytes(), opf.len() as u64),
        ("toc.ncx", ncx.as_bytes(), ncx.len() as u64),
        ("a.xhtml", a.as_bytes(), a.len() as u64),
        ("b.xhtml", b.as_bytes(), b.len() as u64),
        ("s.css", css.as_bytes(), css.len() as u64),
        ("x.png", b"text", 4),
    ];
    std::fs::write(path, fixtures::stored_zip(&entries)).unwrap();
}

/// What kfxgen logs as a warning and Calibre's spine reader too, in the order they log it:
/// a spine item that is no document, a picture kfxgen cannot read, a file kfxgen raises on, words in an SVG picture, a
/// contents entry naming an id no block has and one naming a file that is left out, a table
/// written as rows, and the narration and clip left out. The texts are kfxgen's (a Python
/// `repr` written as Rust's `{:?}`), but for Calibre's, whose words are the port's.
#[test]
fn convert_says_what_kfxgen_warns_about() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    book_kfxgen_warns_about(&path);
    let opts = ConvertOptions {
        disable_native_tables: true,
        ..ConvertOptions::default()
    };
    let got = convert(&path, &opts).unwrap();
    let want = [
        r#"  Spine item "s.css" is not a document: left out"#,
        "  Skipping image \"x.png\": unsupported format (magic bytes 74657874); only JPEG and \
         PNG are emitted as KFX resources",
        "  Spine item 2 parse failed (b.xhtml): AttributeError: compute_block_style: a \
         text-align Calibre reads as a number has no strip()",
        "  Words drawn in SVG pictures in 1 file are written after the picture, not over it \
         (#231)",
        // "One" lands on block 0, so the next missing id snaps to block 1.
        "  TOC anchor #nowhere not found in a.xhtml; snapping to block 1",
        r#"  TOC entry "Two" dropped: not in spine"#,
        "  1 table in 1 file written as one paragraph per row, so columns do not line up: \
         native tables are turned off (#219)",
        "  Left out, since kfxgen writes no audio or video: this book's read-aloud narration \
         and 1 audio or video clip in 1 file. The text and pictures are converted, with each \
         clip's fallback text (#248)",
    ];
    assert_eq!(got.warnings, want);
}

/// An EPUB 2 book at `path` of the pages `files` (name, `<body>` content) in that spine
/// order, with an NCX of `points` (label, src).
fn book_of_pages(path: &Path, files: &[(&str, &str)], points: &[(&str, &str)]) {
    book_with(path, files, points, "", None);
}

/// [`book_of_pages`] with `meta` in the OPF's metadata and, if given, a PNG `cover.png` that
/// the metadata names as the cover. A page's content starting `<body` is its whole `<body>`.
fn book_with(
    path: &Path,
    files: &[(&str, &str)],
    points: &[(&str, &str)],
    meta: &str,
    cover: Option<&[u8]>,
) {
    let mut items =
        String::from(r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#);
    let mut meta = meta.to_owned();
    if cover.is_some() {
        items += r#"<item id="cov" href="cover.png" media-type="image/png"/>"#;
        meta += r#"<meta name="cover" content="cov"/>"#;
    }
    let mut spine = String::new();
    for (i, (name, _)) in files.iter().enumerate() {
        items += &format!(r#"<item id="f{i}" href="{name}" media-type="application/xhtml+xml"/>"#);
        spine += &format!(r#"<itemref idref="f{i}"/>"#);
    }
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language>{meta}</metadata><manifest>{items}</manifest><spine toc="ncx">{spine}</spine></package>"#
    );
    let nav: String = (points.iter().enumerate())
        .map(|(n, (label, src))| {
            format!(
                r#"<navPoint id="p{n}" playOrder="{n}"><navLabel><text>{label}</text></navLabel><content src="{src}"/></navPoint>"#
            )
        })
        .collect();
    let ncx = format!(
        r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head><meta name="dtb:uid" content="x"/></head><docTitle><text>T</text></docTitle><navMap>{nav}</navMap></ncx>"#
    );
    let pages: Vec<(String, String)> = (files.iter())
        .map(|(name, body)| {
            let body = if body.starts_with("<body") {
                (*body).to_owned()
            } else {
                format!("<body>{body}</body>")
            };
            let page = format!(
                r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head>{body}</html>"#
            );
            ((*name).to_owned(), page)
        })
        .collect();
    let mut entries: Vec<(&str, &[u8], u64)> = vec![
        ("mimetype", b"application/epub+zip", 20),
        ("META-INF/container.xml", CONTAINER, CONTAINER.len() as u64),
        ("content.opf", opf.as_bytes(), opf.len() as u64),
        ("toc.ncx", ncx.as_bytes(), ncx.len() as u64),
    ];
    for (name, page) in &pages {
        entries.push((name, page.as_bytes(), page.len() as u64));
    }
    if let Some(png) = cover {
        entries.push(("cover.png", png, png.len() as u64));
    }
    std::fs::write(path, fixtures::stored_zip(&entries)).unwrap();
}

/// A table whose first block goes with its page (a "Contents" page, which the title-page step
/// rebuilds from the entries) is not counted, as kfxgen counts only tables still in a chapter;
/// the block of another file with the same place in its file does not stand in for it.
#[test]
fn only_tables_still_in_a_chapter_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    let files = [
        (
            "c.xhtml",
            r#"<h1>Contents</h1><table><tr><td><a href="a.xhtml">One</a></td></tr></table>"#,
        ),
        ("a.xhtml", "<h1>One</h1><p>Text.</p><p>More.</p>"),
    ];
    book_of_pages(
        &path,
        &files,
        &[("Contents", "c.xhtml"), ("One", "a.xhtml")],
    );
    let opts = ConvertOptions {
        disable_native_tables: true,
        ..ConvertOptions::default()
    };
    let got = convert(&path, &opts).unwrap();
    assert_eq!(got.warnings, Vec::<String>::new());
}

/// The page's file name is not copied once for each table written as rows (kfxgen's tuples
/// share one string): 2,000 tables in a file named by 20,000 characters cost about what they
/// cost in a file of a short name, where a copy each would add 40 MB.
#[test]
fn tables_written_as_rows_do_not_copy_their_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let body = "<table><tr><td>a</td></tr></table><p>x</p>".repeat(2_000);
    let opts = ConvertOptions {
        disable_native_tables: true,
        ..ConvertOptions::default()
    };
    let cost = |name: &str| {
        let path = dir.path().join(format!("{}.epub", name.len()));
        book_of_pages(&path, &[(name, &body)], &[("One", name)]);
        let (got, bytes) = allocated_by(|| convert(&path, &opts).unwrap());
        assert!(
            got.warnings
                .iter()
                .any(|w| w.starts_with("  2000 tables in 1 file"))
        );
        bytes
    };
    let short = cost("a.xhtml");
    let long = cost(&format!("{}.xhtml", "a".repeat(20_000)));
    assert!(
        long.saturating_sub(short) < 20_000_000,
        "{short} bytes, {long} with the long name"
    );
}

/// A PNG of `w` × `h` grey pixels.
fn png(w: u32, h: u32) -> Vec<u8> {
    let img =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([90, 90, 90])));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn chapters_of(got: &Converted) -> &[PyValue] {
    got.model
        .args
        .get("chapters")
        .and_then(PyValue::as_list)
        .unwrap()
}

/// The cover found with the pictures reaches chapter assembly: a page showing only the cover
/// picture makes no chapter of its own (kfxgen's `_page_shows_something` with the cover's
/// href). Here no contents entry lands, so each other file is a chapter ("Section N").
#[test]
fn a_page_of_the_cover_alone_makes_no_chapter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    let files = [
        ("cover.xhtml", r#"<p><img src="cover.png" alt=""/></p>"#),
        ("a.xhtml", "<p>Text.</p>"),
    ];
    let cover = png(40, 60);
    book_with(
        &path,
        &files,
        &[("Gone", "missing.xhtml")],
        "",
        Some(&cover),
    );
    let got = convert(&path, &ConvertOptions::default()).unwrap();
    let titles: Vec<&str> = (chapters_of(&got).iter())
        .map(|c| c.get("title").and_then(PyValue::as_str).unwrap())
        .collect();
    assert_eq!(titles, ["Section 2"]);
    assert!(
        got.model
            .args
            .get("cover_image")
            .unwrap()
            .as_bytes()
            .is_some()
    );
}

/// A chapter running from a horizontal page onto a vertical one (no entry names the vertical
/// page, so it joins the chapter before) is cut where the writing mode changes, the second part
/// left out of the contents with no title printed (`_split_by_writing_mode`).
#[test]
fn a_chapter_joining_a_vertical_page_is_cut_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    let files = [
        ("a.xhtml", "<h1>One</h1><p>Text.</p>"),
        (
            "b.xhtml",
            r#"<body style="writing-mode: vertical-rl"><p>Down.</p></body>"#,
        ),
        ("c.xhtml", "<h1>Two</h1><p>More.</p>"),
    ];
    book_of_pages(&path, &files, &[("One", "a.xhtml"), ("Two", "c.xhtml")]);
    let got = convert(&path, &ConvertOptions::default()).unwrap();
    let chapters = chapters_of(&got);
    let texts: Vec<&str> = (chapters.iter())
        .map(|c| c.get("text").and_then(PyValue::as_str).unwrap())
        .collect();
    assert_eq!(texts, ["One\n\nText.", "Down.", "Two\n\nMore."]);
    assert_eq!(
        chapters[1].get("_omit_from_toc"),
        Some(&PyValue::Bool(true))
    );
    assert_eq!(
        chapters[1].get("_omit_title_heading"),
        Some(&PyValue::Bool(true))
    );
}

/// A book whose contents listing must be printed is read twice (`listing_rerun`'s shape). The
/// ruby's serial number runs on from the first pass, as kfxgen's counter does, so the one ruby
/// of the book is numbered 1 in the model; and the warning about the entry that snaps is said
/// once, the second pass's, not also the first's.
#[test]
fn the_second_pass_numbers_on_and_warns_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    let files = [
        (
            "opening.xhtml",
            r#"<h1>Listing Rerun</h1><p>By Fixture Author</p><div class="toc"><p><a href="c1.xhtml">Chapter One</a></p><p><a href="c2.xhtml">Chapter Two</a></p></div>"#,
        ),
        (
            "c1.xhtml",
            "<h1>Chapter One</h1><p><ruby>漢<rt>かん</rt></ruby> first.</p>",
        ),
        ("c2.xhtml", "<h1>Chapter Two</h1><p>The second chapter.</p>"),
    ];
    let points = [
        ("Opening", "opening.xhtml"),
        ("Chapter One", "c1.xhtml"),
        ("Gone", "c1.xhtml#nowhere"),
        ("Chapter Two", "c2.xhtml"),
    ];
    book_of_pages(&path, &files, &points);
    let got = convert(&path, &ConvertOptions::default()).unwrap();
    // The listing is printed: the second pass made the model.
    let opening = chapters_of(&got)[0]
        .get("text")
        .and_then(PyValue::as_str)
        .unwrap();
    assert!(opening.contains("Chapter One"), "{opening:?}");
    let model = format!("{:?}", got.model.args);
    assert!(
        model.contains(r#"Tuple([Int(1), Str("かん")])"#),
        "the ruby is not numbered 1"
    );
    let snaps = (got.warnings.iter())
        .filter(|w| w.contains("TOC anchor #nowhere"))
        .count();
    assert_eq!(snaps, 1, "{:?}", got.warnings);
}

/// A book whose one file has no name: the OPF at an empty path (`full-path=""`) lists itself
/// as its document (`href=""`). kfxgen moves the keys of the ids ending a file to the next
/// block only for a file with an href (`if end_ids and href`); with none, no key is made of
/// the ids `f0` and `tail` ending it.
#[test]
fn ids_ending_a_file_of_no_name_make_no_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    let container = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let opf = r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="f0" href="" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="f0" id="tail"/></spine></package>"#;
    let entries: Vec<(&str, &[u8], u64)> = vec![
        ("mimetype", b"application/epub+zip", 20),
        ("META-INF/container.xml", container, container.len() as u64),
        ("", opf.as_bytes(), opf.len() as u64),
    ];
    std::fs::write(&path, fixtures::stored_zip(&entries)).unwrap();
    let got = convert(&path, &ConvertOptions::default()).unwrap();
    let blocks = chapters_of(&got)[0]
        .get("blocks")
        .and_then(PyValue::as_list)
        .unwrap();
    let last = blocks.last().unwrap();
    assert_eq!(last.get("text").and_then(PyValue::as_str), Some("en"));
    assert_eq!(last.get("anchor_keys"), Some(&PyValue::List(Vec::new())));
}

/// An EPUB 2 book at `path` of the pages `files` and an NCX of `points`, each given owned.
fn book_of(path: &Path, files: &[(String, String)], points: &[(String, String)]) {
    let files: Vec<(&str, &str)> = (files.iter())
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let points: Vec<(&str, &str)> = (points.iter())
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    book_of_pages(path, &files, &points);
}

/// The message of the refusal `convert` gives the book at `path`; it panics on a book that
/// converts.
fn refusal(path: &Path) -> String {
    match convert(path, &ConvertOptions::default()) {
        Ok(_) => panic!("{} converted", path.display()),
        Err(e) => e.to_string(),
    }
}

/// A book of `files` pages, each a link to an id of 32,000 letters round 240 words in italics:
/// each word's run copies the link's target, charged to its file's text budget, every file
/// well under its own.
fn book_of_long_links(path: &Path, files: usize) {
    let target = format!("c.xhtml#{}", "A".repeat(32_000));
    let body = format!(
        r#"<p><a href="{target}">{}</a></p>"#,
        "<i>w</i> ".repeat(240)
    );
    let files: Vec<(String, String)> = (0..files)
        .map(|i| (format!("f{i}.xhtml"), body.clone()))
        .collect();
    book_of(path, &files, &[("One".into(), "f0.xhtml".into())]);
}

/// The text budgets of a pass count together (`MAX_BOOK_TEXT_BUDGET`): a book whose files each
/// copy a long link target into every word is refused once they have copied 128 MB together,
/// before every file has made its copies; a book of fewer such files converts.
#[test]
fn files_that_together_copy_too_much_text_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("few.epub");
    book_of_long_links(&path, 5);
    convert(&path, &ConvertOptions::default()).unwrap();
    let path = dir.path().join("many.epub");
    book_of_long_links(&path, 20);
    assert_eq!(
        refusal(&path),
        "converting this book copies or reads more than 128000000 bytes of its text and styles"
    );
}

/// One page and an NCX of an entry naming it, an entry `R` naming an id of `len` letters no
/// block has (it snaps to the page's last block), and `twins` entries each naming another
/// missing id, which snaps to the same block: each is `R`'s twin, and its contents entry a
/// copy of `R`'s key.
fn book_of_twins(path: &Path, len: usize, twins: usize) {
    let files = [("a.xhtml".to_owned(), "<h1>H</h1><p>one</p>".to_owned())];
    let mut points = vec![
        ("Head".to_owned(), "a.xhtml".to_owned()),
        ("R".to_owned(), format!("a.xhtml#{}", "z".repeat(len))),
    ];
    points.extend((0..twins).map(|n| ("E".to_owned(), format!("a.xhtml#q{n}"))));
    book_of(path, &files, &points);
}

/// Contents entries on one block copy the first one's key, which kfxgen shares: 335 copies of
/// a key of 100,008 bytes are under 33,554,432 bytes and convert, 336 are refused.
#[test]
fn entries_on_one_block_repeat_its_key_up_to_a_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("under.epub");
    book_of_twins(&path, 100_000, 335);
    convert(&path, &ConvertOptions::default()).unwrap();
    let path = dir.path().join("over.epub");
    book_of_twins(&path, 100_000, 336);
    assert_eq!(
        refusal(&path),
        "the contents repeat more than 33554432 bytes of titles and keys"
    );
}

/// `files` pages, alternately horizontal and vertical, and an NCX naming the first, titled with
/// `title` letters, and the last: the first chapter spans the others and is cut at each change
/// of writing mode, each part after the first a copy of its title.
fn book_of_turns(path: &Path, title: usize, files: usize) {
    let files: Vec<(String, String)> = (0..files)
        .map(|i| {
            let body = if i % 2 == 1 {
                r#"<body style="writing-mode: vertical-rl"><p>Down.</p></body>"#
            } else {
                "<p>Across.</p>"
            };
            (format!("f{i}.xhtml"), body.to_owned())
        })
        .collect();
    let last = files.last().unwrap().0.clone();
    let points = [
        ("A".repeat(title), "f0.xhtml".to_owned()),
        ("End".to_owned(), last),
    ];
    book_of(path, &files, &points);
}

/// A chapter cut at every change of writing mode copies its title into each part, which
/// kfxgen's parts share: 33 copies of a title of 1,000,000 bytes convert, 34 are refused.
#[test]
fn a_chapter_cut_many_times_repeats_its_title_up_to_a_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("under.epub");
    book_of_turns(&path, 1_000_000, 35);
    convert(&path, &ConvertOptions::default()).unwrap();
    let path = dir.path().join("over.epub");
    book_of_turns(&path, 1_000_000, 36);
    assert_eq!(
        refusal(&path),
        "the contents repeat more than 33554432 bytes of titles and keys"
    );
}

/// A page with a contents listing of 5,000 ids and `entries` contents entries titled
/// "Copyright" (a page kfxgen always starts), naming the listing's first id and the page by
/// turns: each entry naming the page makes a page of no blocks that keeps the listing's keys.
fn book_of_kept_listings(path: &Path, entries: usize) {
    let ids: String = (0..5_000).map(|n| format!(r#"<a id="i{n}"/>"#)).collect();
    let files = [(
        "a.xhtml".to_owned(),
        format!(r#"<p>Text</p><div class="toc">{ids}</div>"#),
    )];
    let points: Vec<(String, String)> = (0..entries)
        .map(|n| {
            let src = if n % 2 == 0 { "a.xhtml#i0" } else { "a.xhtml" };
            ("Copyright".to_owned(), src.to_owned())
        })
        .collect();
    book_of(path, &files, &points);
}

/// Each page of no blocks that keeps a dropped listing makes its keys anew (kfxgen too); they
/// are counted, each key its bytes and 48 more, and refused past 33,554,432 bytes. A page here
/// counts 303,890 bytes (5,000 keys of `a.xhtml#`, 56 bytes each, and the ids' 23,890): 220
/// entries make 110 such pages and convert, 222 make 111 and are refused.
#[test]
fn pages_keeping_a_listing_repeat_its_keys_up_to_a_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("under.epub");
    book_of_kept_listings(&path, 220);
    let got = convert(&path, &ConvertOptions::default()).unwrap();
    let kept = (chapters_of(&got).iter())
        .filter(|c| c.get("_listing_keys").is_some())
        .count();
    assert!(kept > 100, "{kept} pages keep the listing");
    let path = dir.path().join("over.epub");
    book_of_kept_listings(&path, 222);
    assert_eq!(
        refusal(&path),
        "the contents repeat more than 33554432 bytes of titles and keys"
    );
}
