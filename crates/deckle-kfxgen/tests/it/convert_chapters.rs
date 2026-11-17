// SPDX-License-Identifier: GPL-3.0-only
//! The chapters (plan Task 21): kfxgen's per-file loop and chapter assembly
//! (`extract_chapters_from_oeb`, `_assemble_chapters_by_coordinate`) against the assembly dump
//! (`convert/<name>/assembly.json.gz`, A): one record per pass, the files right before
//! assembly and the chapters right before the title-page step.

use deckle_kfx::fixtures;
use deckle_kfxgen::book::{Book, read_book};
use deckle_kfxgen::convert::{
    CalibreOpf, Chapter, ConvertOptions, Error, Serials, SpineItem, TextBudget, TocEntry, assemble,
    spine_items, split_by_writing_mode,
};
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_args::sources;
use crate::convert_support::{dump, model_diff};
use crate::convert_toc::book_of;

fn s(v: &str) -> PyValue {
    PyValue::Str(v.to_owned())
}

/// A's records, one per pass.
fn records(name: &str) -> Vec<PyValue> {
    match dump(name, "assembly.json.gz") {
        PyValue::List(r) => r,
        other => panic!("{name}: no list of passes: {other:?}"),
    }
}

/// The contents entries kfxgen assembled with (T's, after respelling).
fn entries_of(name: &str) -> Vec<TocEntry> {
    dump(name, "toc.json.gz")
        .get("toc_entries")
        .and_then(PyValue::as_list)
        .unwrap()
        .iter()
        .map(|e| TocEntry {
            title: e.get("title").and_then(PyValue::as_str).unwrap().to_owned(),
            href: e.get("href").and_then(PyValue::as_str).unwrap().to_owned(),
            level: e.get("level").and_then(PyValue::as_int).unwrap() as usize,
        })
        .collect()
}

fn entries_py(entries: &[TocEntry]) -> PyValue {
    PyValue::List(
        entries
            .iter()
            .map(|e| {
                PyValue::Dict(vec![
                    (s("title"), s(&e.title)),
                    (s("href"), s(&e.href)),
                    (s("level"), PyValue::Int(e.level as i64)),
                ])
            })
            .collect(),
    )
}

fn items_py(items: &[SpineItem]) -> PyValue {
    PyValue::List(items.iter().map(SpineItem::to_py).collect())
}

fn chapters_py(chapters: &[Chapter]) -> PyValue {
    PyValue::List(chapters.iter().map(Chapter::to_py).collect())
}

/// A record's files as kfxgen handed them to assembly.
fn items_of(record: &PyValue) -> Vec<SpineItem> {
    record
        .get("spine_items_ordered")
        .and_then(PyValue::as_list)
        .unwrap()
        .iter()
        .map(|i| SpineItem::from_py(i).unwrap_or_else(|| panic!("not a spine item: {i:?}")))
        .collect()
}

fn budgets(book: &Book) -> Vec<TextBudget> {
    book.files.iter().map(|_| TextBudget::new()).collect()
}

/// For every source and each of its passes, assembly of the files kfxgen assembled gives the
/// chapters kfxgen had right before its title-page step.
#[test]
fn assembly_matches_the_dump() {
    let names = sources();
    assert_eq!(names.len(), 55, "the source fixtures");
    let mut failures = Vec::new();
    let mut chapters = 0;
    for name in &names {
        let entries = entries_of(name);
        for (p, record) in records(name).iter().enumerate() {
            let cover = record.get("cover_href").and_then(PyValue::as_str);
            let got = assemble(items_of(record), &entries, cover);
            chapters += got.len();
            for d in model_diff(&chapters_py(&got), record.get("chapters").unwrap()) {
                failures.push(format!("{name} pass {p} chapters{d}"));
            }
        }
    }
    for f in failures.iter().take(60) {
        eprintln!("{f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
    assert!(chapters > 150, "{chapters} chapters compared");
}

/// For every source, the per-file loop over the book gives the files kfxgen handed assembly,
/// on each pass recorded (the second with `keep_nav_listings`), and the contents entries it
/// assembled with. The serials run on across the passes, as kfxgen's counters do.
#[test]
fn the_per_file_loop_feeds_assembly_what_python_fed_it() {
    let mut failures = Vec::new();
    let mut second_passes = 0;
    for name in &sources() {
        let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let opf = CalibreOpf::new(&book).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let budgets = budgets(&book);
        let mut serials = Serials::default();
        let want_entries = entries_py(&entries_of(name));
        for (p, record) in records(name).iter().enumerate() {
            second_passes += p;
            let got = spine_items(
                &book,
                &opf,
                &ConvertOptions::default(),
                &budgets,
                &mut serials,
                p == 1,
            );
            let (items, entries) = match got {
                Ok(got) => got,
                Err(e) => {
                    failures.push(format!("{name} pass {p}: {e}"));
                    continue;
                }
            };
            let want = record.get("spine_items_ordered").unwrap();
            for d in model_diff(&items_py(&items), want) {
                failures.push(format!("{name} pass {p} spine_items_ordered{d}"));
            }
            for d in model_diff(&entries_py(&entries), &want_entries) {
                failures.push(format!("{name} pass {p} toc_entries{d}"));
            }
        }
    }
    for f in failures.iter().take(60) {
        eprintln!("{f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
    assert_eq!(second_passes, 2, "listing_rerun and refused_selector");
}

const XHTML: &str = "application/xhtml+xml";

/// An EPUB 2 book of the pages `files` (name, body content or a whole `<body>`), in that spine
/// order, each linking the stylesheet `css`, and no contents.
fn book_of_pages(files: &[(&str, &str)], css: &str) -> (Book, CalibreOpf) {
    let mut items = String::from(r#"<item id="css" href="s.css" media-type="text/css"/>"#);
    let mut spine = String::new();
    for (i, (name, _)) in files.iter().enumerate() {
        items += &format!(r#"<item id="f{i}" href="{name}" media-type="{XHTML}"/>"#);
        spine += &format!(r#"<itemref idref="f{i}"/>"#);
    }
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#
    );
    let pages: Vec<String> = files
        .iter()
        .map(|(_, body)| {
            let body = if body.starts_with("<body") {
                body.to_string()
            } else {
                format!("<body>{body}</body>")
            };
            format!(
                r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><link rel="stylesheet" type="text/css" href="s.css"/></head>{body}</html>"#
            )
        })
        .collect();
    let mut zipped: Vec<(&str, &[u8])> = vec![("s.css", css.as_bytes())];
    for ((name, _), page) in files.iter().zip(&pages) {
        zipped.push((name, page.as_bytes()));
    }
    book_of(&opf, &zipped)
}

fn run(book: &Book, opf: &CalibreOpf) -> Result<(Vec<SpineItem>, Vec<TocEntry>), Error> {
    spine_items(
        book,
        opf,
        &ConvertOptions::default(),
        &budgets(book),
        &mut Serials::default(),
        false,
    )
}

/// A book none of whose files shows a word is refused as showing nothing, as kfxgen raises
/// `ValueError` there; so is one whose only shown file kfxgen raises on.
#[test]
fn a_book_that_shows_nothing_is_an_error() {
    let (book, opf) = book_of_pages(&[("a.xhtml", r#"<div id="e"></div>"#), ("b.xhtml", "")], "");
    assert!(matches!(run(&book, &opf), Err(Error::NoContent)));
    let (book, opf) = book_of_pages(
        &[
            ("a.xhtml", "<p> </p>"),
            ("b.xhtml", r#"<p class="a">x</p>"#),
        ],
        ".a { text-align: 5px }",
    );
    assert!(matches!(run(&book, &opf), Err(Error::NoContent)));
}

/// What stops a file goes where kfxgen's `try` round each file sends it: what kfxgen raises
/// on leaves that file out and keeps the rest; a refusal by name stops the book.
#[test]
fn a_file_kfxgen_raises_on_is_left_out_and_a_refusal_stops_the_book() {
    let (book, opf) = book_of_pages(
        &[
            ("a.xhtml", "<p>One.</p>"),
            ("b.xhtml", r#"<p class="a">x</p>"#),
            ("c.xhtml", "<p>Three.</p>"),
        ],
        ".a { text-align: 5px }",
    );
    let (items, _) = run(&book, &opf).unwrap();
    let hrefs: Vec<&str> = items.iter().map(|i| i.href.as_str()).collect();
    assert_eq!(hrefs, ["a.xhtml", "c.xhtml"]);

    let math = r#"<p>x</p><math xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math>"#;
    let (book, opf) = book_of_pages(&[("a.xhtml", "<p>One.</p>"), ("b.xhtml", math)], "");
    match run(&book, &opf) {
        Err(Error::Unsupported { file, .. }) => assert_eq!(file, "b.xhtml"),
        other => panic!("{:?}", other.map(|(i, _)| i.len())),
    }
}

fn strings(v: Option<&PyValue>) -> Vec<String> {
    v.and_then(PyValue::as_list)
        .unwrap_or(&[])
        .iter()
        .map(|i| i.as_str().unwrap().to_owned())
        .collect()
}

/// The keys of a file that shows nothing (its own and its ids') and of the ids ending a file go
/// first on the next shown file's first block, offset 0, and leave the file they end; with no
/// shown file after them, on the book's last block, at the end of its text. Calibre's own
/// contents page is left out.
#[test]
fn keys_of_what_shows_nothing_go_to_the_next_shown_block() {
    let (book, opf) = book_of_pages(
        &[
            ("a.xhtml", r#"<p>One.</p><p><a id="t"></a></p>"#),
            ("b.xhtml", r#"<div id="e"><a name="n"></a></div>"#),
            ("c.xhtml", "<p>Three.</p>"),
            ("d.xhtml", r#"<div id="z"></div>"#),
        ],
        "",
    );
    let (items, _) = run(&book, &opf).unwrap();
    let hrefs: Vec<&str> = items.iter().map(|i| i.href.as_str()).collect();
    assert_eq!(hrefs, ["a.xhtml", "c.xhtml"]);
    let a = items[0].blocks[0].to_py();
    assert_eq!(strings(a.get("anchor_keys")), ["a.xhtml"]);
    assert!(items[0].trailing_ids.contains("t"));
    let c = items[1].blocks[0].to_py();
    assert_eq!(
        strings(c.get("anchor_keys")),
        [
            "a.xhtml#t",
            "b.xhtml",
            "b.xhtml#e",
            "b.xhtml#n",
            "c.xhtml",
            "d.xhtml",
            "d.xhtml#z"
        ]
    );
    let offsets = c.get("anchor_offsets").unwrap();
    assert_eq!(offsets.get("b.xhtml#n"), Some(&PyValue::Int(0)));
    // "Three." is six characters long.
    assert_eq!(offsets.get("d.xhtml#z"), Some(&PyValue::Int(6)));

    let (book, opf) = book_of_pages(
        &[
            (
                "toc.xhtml",
                r#"<body id="calibre_generated_inline_toc"><p>Listed.</p></body>"#,
            ),
            ("a.xhtml", "<p>One.</p>"),
        ],
        "",
    );
    let (items, _) = run(&book, &opf).unwrap();
    let hrefs: Vec<&str> = items.iter().map(|i| i.href.as_str()).collect();
    assert_eq!(hrefs, ["a.xhtml"]);
}

/// A file whose conversion spends its budget is refused, named, with a message that says what
/// the budget counts (not only CSS): a table whose `<col span>`s add up to two million column
/// widths, each charged before it is made.
#[test]
fn a_file_past_its_budget_is_refused_by_name() {
    let body = format!(
        "<table>{}<tr><td>x</td></tr></table>",
        r#"<col span="1000"/>"#.repeat(2_000)
    );
    let (book, opf) = book_of_pages(&[("a.xhtml", "<p>One.</p>"), ("b.xhtml", &body)], "");
    let start = std::time::Instant::now();
    let r = run(&book, &opf);
    match r {
        Err(Error::TooLarge { what }) => assert!(
            what.starts_with("b.xhtml: converting this file copies or reads more than"),
            "{what}"
        ),
        other => panic!("{:?}", other.map(|(i, _)| i.len())),
    }
    assert!(start.elapsed() < crate::support::COST_LIMIT);
}

/// A block of `text`.
fn block(text: &str) -> PyValue {
    PyValue::Dict(vec![
        (s("text"), s(text)),
        (s("spans"), PyValue::List(Vec::new())),
        (s("anchor_ids"), PyValue::List(Vec::new())),
    ])
}

fn item(href: &str, blocks: Vec<PyValue>) -> SpineItem {
    let text: Vec<&str> = blocks
        .iter()
        .filter_map(|b| b.get("text").and_then(PyValue::as_str))
        .collect();
    SpineItem::from_py(&PyValue::Dict(vec![
        (s("href"), s(href)),
        (s("text"), s(&text.join("\n\n"))),
        (s("blocks"), PyValue::List(blocks)),
        (s("nav_listing_at"), PyValue::List(Vec::new())),
        (s("note_ids"), PyValue::Set(Vec::new())),
        (s("trailing_ids"), PyValue::Set(Vec::new())),
        (s("breaks_known"), PyValue::Bool(true)),
    ]))
    .unwrap()
}

fn titles_and_texts(chapters: &[Chapter]) -> Vec<(String, String)> {
    chapters
        .iter()
        .map(|c| {
            let py = c.to_py();
            let get = |k| py.get(k).and_then(PyValue::as_str).unwrap_or("").to_owned();
            (get("title"), get("text"))
        })
        .collect()
}

/// With no contents entry, each file that shows something is a chapter of its own: titled
/// by its opening heading, else "Section N" (N its place among the files), its title never
/// printed; a page of the cover alone is left out.
#[test]
fn files_are_chapters_when_no_entry_lands() {
    let cover = "\u{0}IMG\u{1}images/Cover.JPG\u{1}\u{0}";
    let mut heading = block("Chapter  One");
    if let PyValue::Dict(pairs) = &mut heading {
        pairs.push((s("heading"), PyValue::Bool(true)));
    }
    let items = vec![
        item("cover.xhtml", vec![block(cover)]),
        item("a.xhtml", vec![heading, block("Text.")]),
        item("b.xhtml", vec![block("More.")]),
    ];
    let entries = [TocEntry {
        title: "Gone".into(),
        href: "missing.xhtml".into(),
        level: 0,
    }];
    let got = assemble(items, &entries, Some("cover.jpg"));
    assert_eq!(
        titles_and_texts(&got),
        [
            ("Chapter One".into(), "Chapter  One\n\nText.".into()),
            ("Section 3".into(), "More.".into()),
        ]
    );
    let first = got[0].to_py();
    let keys: Vec<&str> = match &first {
        PyValue::Dict(pairs) => pairs.iter().filter_map(|(k, _)| k.as_str()).collect(),
        _ => panic!(),
    };
    assert_eq!(
        keys,
        [
            "title",
            "text",
            "_from_spine",
            "_omit_title_heading",
            "blocks"
        ]
    );
}

/// Assembly of crafted books gives the chapters kfxgen's own functions gave them
/// (`tests/it/data/assembly_probe.json`, which says how they were run): a listing kept as a
/// page of no blocks, a listing's chapter, entries merged into one section and split again
/// round a listing, entries on one block, a head before the first entry with and without an
/// entry's id, files named by a file name, and chapters cut by writing mode.
#[test]
fn assembly_of_crafted_books_matches_kfxgen() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/data/assembly_probe.json");
    let probe: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let cases = probe["cases"].as_object().unwrap();
    assert_eq!(cases.len(), 6);
    let mut failures = Vec::new();
    for (name, case) in cases {
        let py = |key: &str| PyValue::from_json(&case[key], &path, name).unwrap();
        let items = py("items")
            .as_list()
            .unwrap()
            .iter()
            .map(|i| SpineItem::from_py(i).unwrap())
            .collect();
        let entries: Vec<TocEntry> = case["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| TocEntry {
                title: e["title"].as_str().unwrap().to_owned(),
                href: e["href"].as_str().unwrap().to_owned(),
                level: e["level"].as_u64().unwrap() as usize,
            })
            .collect();
        let mut got = assemble(items, &entries, case["cover"].as_str());
        if case["split"].as_bool() == Some(true) {
            got = split_by_writing_mode(got);
        }
        for d in model_diff(&chapters_py(&got), &py("chapters")) {
            failures.push(format!("{name}{d}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A block of `text` carrying the id `id`, keyed in `file`.
fn block_with_id(file: &str, text: &str, id: &str) -> PyValue {
    PyValue::Dict(vec![
        (s("text"), s(text)),
        (s("anchor_ids"), PyValue::List(vec![s(id)])),
        (
            s("anchor_keys"),
            PyValue::List(vec![s(&format!("{file}#{id}"))]),
        ),
    ])
}

/// What kfxgen does in time quadratic in blocks, entries or listings, done here once each:
/// an entry for each of 50,000 blocks, each followed by one out of document order (kfxgen
/// scans the blocks back to it for a page break), a listing after every other block (tested
/// against every section), and 20,000 entries naming ids a one-block file lacks (each key
/// tested against the block's keys, and a chain of entries on one block followed link by
/// link).
#[test]
fn assembly_of_many_blocks_and_entries_costs_little() {
    const BLOCKS: usize = 50_000;
    const MISSING: usize = 20_000;
    let f: Vec<PyValue> = (0..BLOCKS)
        .map(|i| block_with_id("f.xhtml", &format!("b{i}"), &format!("i{i}")))
        .collect();
    let mut f = item("f.xhtml", f);
    f.nav_listing_at = (0..BLOCKS)
        .step_by(2)
        .map(|i| (i, vec![format!("l{i}")]))
        .collect();
    let g = item("g.xhtml", vec![block("g")]);
    let entry = |href: String| TocEntry {
        title: href.clone(),
        href,
        level: 0,
    };
    let mut entries = Vec::new();
    for i in 0..BLOCKS {
        entries.push(entry(format!("f.xhtml#i{i}")));
        if i > 0 {
            entries.push(entry("f.xhtml#i0".into()));
        }
    }
    entries.extend((0..MISSING).map(|j| entry(format!("g.xhtml#gone{j}"))));
    let start = std::time::Instant::now();
    let got = assemble(vec![f, g], &entries, None);
    let took = start.elapsed();
    eprintln!(
        "{BLOCKS} blocks, {} entries: {} chapters in {took:?}",
        entries.len(),
        got.len()
    );
    assert_eq!(got.len(), BLOCKS + 1);
    let last = got[BLOCKS].to_py();
    let subs = last.get("sub_entries").and_then(PyValue::as_list).unwrap();
    assert_eq!(subs.len(), MISSING - 1);
    let keys = got[BLOCKS].blocks()[0].to_py();
    assert_eq!(strings(keys.get("anchor_keys")).len(), MISSING);
    assert!(took < crate::support::COST_LIMIT, "{took:?}");
}

/// The per-file loop and assembly of `book`, timed, with the bytes they allocated.
fn measure(what: &str, book: &Book, opf: &CalibreOpf) {
    let start = std::time::Instant::now();
    let ((items, entries), bytes) = crate::support::allocated_by(|| run(book, opf).unwrap());
    let walked = start.elapsed();
    let blocks: usize = items.iter().map(|i| i.blocks.len()).sum();
    let (chapters, more) = crate::support::allocated_by(|| assemble(items, &entries, None));
    eprintln!(
        "{what}: {} files, {blocks} blocks, {} entries: loop {walked:?} ({} MB allocated), \
         assembly {:?} ({} MB), {} chapters",
        book.files.len(),
        entries.len(),
        bytes >> 20,
        start.elapsed() - walked,
        more >> 20,
        chapters.len()
    );
}

/// Release-build timings of the loop and assembly: a chapter of 20,000 paragraphs, a book of
/// 2,000 small files with a contents entry each (Calibre's, from their headings), and the
/// largest fixture.
#[test]
#[ignore = "a measurement, for a release build"]
fn chapters_cost_in_a_release_build() {
    let paragraphs: String = (0..20_000)
        .map(|i| format!("<p>Paragraph {i} of the keeper's long night at the lamp.</p>"))
        .collect();
    let (book, opf) = book_of_pages(&[("a.xhtml", &paragraphs)], "");
    measure("20,000 paragraphs", &book, &opf);

    let names: Vec<String> = (0..2_000).map(|i| format!("f{i}.xhtml")).collect();
    let bodies: Vec<String> = (0..2_000)
        .map(|i| {
            format!(
                "<h1>Chapter {i}</h1>{}",
                "<p>A short page of text.</p>".repeat(5)
            )
        })
        .collect();
    let files: Vec<(&str, &str)> = names
        .iter()
        .zip(&bodies)
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    let (book, opf) = book_of_pages(&files, "");
    measure("2,000 small files", &book, &opf);

    let book = read_book(&fixtures::kfxgen("sources/page_list.epub")).unwrap();
    let opf = CalibreOpf::new(&book).unwrap();
    measure("page_list", &book, &opf);
}

/// The key of an id ending a file whose last block is a native table leaves the table, its
/// rows, cells and cell paragraphs too, and goes on to the next shown file.
#[test]
fn keys_of_ids_ending_a_file_leave_its_table() {
    let (book, opf) = book_of_pages(
        &[
            (
                "a.xhtml",
                r#"<p>One.</p><table><tr><td><p>x</p></td></tr></table><p><a id="t"></a></p>"#,
            ),
            ("b.xhtml", "<p>Two.</p>"),
        ],
        "",
    );
    let (items, _) = run(&book, &opf).unwrap();
    let a = PyValue::List(items[0].blocks.iter().map(|b| b.to_py()).collect());
    assert!(!format!("{a:?}").contains("a.xhtml#t"), "{a:?}");
    assert!(format!("{a:?}").contains("\"t\""), "the id stays: {a:?}");
    let b = items[1].blocks[0].to_py();
    assert_eq!(strings(b.get("anchor_keys")), ["a.xhtml#t", "b.xhtml"]);
}
