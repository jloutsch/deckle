// SPDX-License-Identifier: GPL-3.0-only
//! The contents entries, the order of the book's files, the ids that are notes and the
//! book's link targets (plan Task 20), against kfxgen's own values (`convert/<name>/toc.json.gz`)
//! and the `footnote_keys` of every model.

use deckle_kfx::fixtures;
use deckle_kfxgen::book::{Book, read_book};
use deckle_kfxgen::convert::{
    CalibreOpf, TextBudget, TocEntry, book_link_targets, footnote_keys, note_pair_ids,
    note_target_ids, respell_toc_hrefs, spine_in_order, toc_entries,
};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_args::{dict, sources};
use crate::convert_support::{dump, model_diff};

fn s(v: &str) -> PyValue {
    PyValue::Str(v.to_owned())
}

/// A set as the dumps write one: its items sorted.
fn set<'a>(items: impl IntoIterator<Item = &'a String>) -> PyValue {
    let mut items: Vec<&String> = items.into_iter().collect();
    items.sort();
    PyValue::Set(items.into_iter().map(|i| s(i)).collect())
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

fn budgets(book: &Book) -> Vec<TextBudget> {
    book.files.iter().map(|_| TextBudget::new()).collect()
}

fn read(name: &str) -> (Book, CalibreOpf) {
    let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub")))
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let opf = CalibreOpf::new(&book).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    (book, opf)
}

/// The entries and spine as `extract_chapters_from_oeb` makes them: entries, the spine in
/// kfxgen's order, then the entries respelled to it.
fn contents(book: &Book, opf: &CalibreOpf) -> (Vec<TocEntry>, Vec<usize>) {
    let mut entries = toc_entries(book, opf);
    let spine = spine_in_order(book, opf, &entries);
    respell_toc_hrefs(&mut entries, book, &spine);
    (entries, spine)
}

/// For every source, the contents entries after respelling and the spine files in kfxgen's
/// order are the dump's (T).
#[test]
fn contents_entries_and_file_order_match_the_dump() {
    let names = sources();
    assert_eq!(names.len(), 55, "the source fixtures");
    let mut failures = Vec::new();
    for name in &names {
        let (book, opf) = read(name);
        let (entries, spine) = contents(&book, &opf);
        let t = dump(name, "toc.json.gz");
        for d in model_diff(&entries_py(&entries), t.get("toc_entries").unwrap()) {
            failures.push(format!("{name} toc_entries{d}"));
        }
        let hrefs = PyValue::List(spine.iter().map(|&i| s(&book.files[i].href)).collect());
        for d in model_diff(&hrefs, t.get("spine").unwrap()) {
            failures.push(format!("{name} spine{d}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// For every source, each spine file's note ids, the note pairs and the book's link targets
/// are the dump's (T), which kfxgen's hook works out on every book.
#[test]
fn note_ids_and_link_targets_match_the_dump() {
    let mut failures = Vec::new();
    for name in &sources() {
        let (book, opf) = read(name);
        let (_, spine) = contents(&book, &opf);
        let t = dump(name, "toc.json.gz");
        let ids = PyValue::Dict(
            spine
                .iter()
                .map(|&i| {
                    let f = &book.files[i];
                    (s(&f.href), set(&note_target_ids(&f.dom)))
                })
                .collect(),
        );
        for d in model_diff(&ids, t.get("note_target_ids").unwrap()) {
            failures.push(format!("{name} note_target_ids{d}"));
        }
        let pairs = note_pair_ids(&book, &budgets(&book)).unwrap();
        let mut files: Vec<&String> = pairs.keys().collect();
        files.sort();
        let pairs = PyValue::Dict(files.iter().map(|f| (s(f), set(&pairs[*f]))).collect());
        for d in model_diff(&pairs, t.get("note_pair_ids").unwrap()) {
            failures.push(format!("{name} note_pair_ids{d}"));
        }
        let mut targets: Vec<String> = book_link_targets(&book, &budgets(&book))
            .unwrap()
            .into_iter()
            .collect();
        targets.sort();
        let targets = PyValue::List(targets.iter().map(|t| s(t)).collect());
        for d in model_diff(&targets, t.get("link_targets").unwrap()) {
            failures.push(format!("{name} link_targets{d}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// For every source, `footnote_keys` is the model's argument.
#[test]
fn footnote_keys_match_every_model() {
    let mut failures = Vec::new();
    let mut with_notes = 0;
    for name in &sources() {
        let (book, _) = read(name);
        let model = load_model(&fixtures::kfxgen(&format!("models/{name}")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let keys = footnote_keys(&book, &budgets(&book)).unwrap();
        with_notes += usize::from(!keys.is_empty());
        let want = dict(&model.args);
        for d in model_diff(&set(&keys), want["footnote_keys"]) {
            failures.push(format!("{name} footnote_keys{d}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(with_notes > 0, "a source with footnotes");
}

/// `zip_of`'s book written to a temp file and read, with its Calibre OPF. A debug build's XML
/// parser needs more than a test thread's stack for files nested near `MAX_DEPTH`, as in the
/// other deep tests.
fn book_of(opf: &str, files: &[(&str, &[u8])]) -> (Book, CalibreOpf) {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), crate::book::zip_of(opf.as_bytes(), files)).unwrap();
    let path = file.path().to_owned();
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let book = read_book(&path).unwrap();
            let opf = CalibreOpf::new(&book).unwrap();
            (book, opf)
        })
        .unwrap()
        .join()
        .unwrap()
}

const XHTML: &str = "application/xhtml+xml";

fn page(body: &str) -> String {
    format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>T</title></head><body>{body}</body></html>"#
    )
}

/// Many contents entries over many files, the files Calibre adds among them, and contents
/// nested nearly as deep as a file may be: each step is a pass through maps, so the whole
/// costs little.
#[test]
fn many_entries_over_many_files_cost_little() {
    const OWN: usize = 500;
    const ADDED: usize = 1_500;
    const ENTRIES: usize = 60_000;
    const DEEP: usize = 240;
    let mut items = String::new();
    let mut spine = String::new();
    for i in 0..OWN {
        items += &format!(r#"<item id="o{i}" href="o{i}.xhtml" media-type="{XHTML}"/>"#);
        spine += &format!(r#"<itemref idref="o{i}"/>"#);
    }
    for i in 0..ADDED {
        items += &format!(r#"<item id="a{i}" href="t/a{i}.xhtml" media-type="{XHTML}"/>"#);
    }
    items += r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#;
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest>{items}</manifest><spine toc="ncx">{spine}</spine></package>"#
    );
    // Entries name the added files last first, so they are sorted; every tenth has a blank
    // label, so the NCX is walked for them.
    let point = |label: &str, src: String, inner: &str| {
        format!(
            r#"<navPoint><navLabel><text>{label}</text></navLabel><content src="{src}"/>{inner}</navPoint>"#
        )
    };
    let mut nav = String::new();
    for e in 0..ENTRIES {
        let label = if e % 10 == 9 {
            " ".to_owned()
        } else {
            format!("E{e}")
        };
        nav += &point(&label, format!("t/a{}.xhtml#x", ADDED - 1 - e % ADDED), "");
    }
    let mut deep = String::new();
    for d in (0..DEEP).rev() {
        deep = point(&format!("D{d}"), format!("o{}.xhtml", d % OWN), &deep);
    }
    nav += &deep;
    let ncx = format!(
        r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/><docTitle><text>T</text></docTitle><navMap>{nav}</navMap></ncx>"#
    );
    let links: String = (0..ADDED)
        .map(|i| format!(r#"<a href="t/a{i}.xhtml">{i}</a>"#))
        .collect();
    let first = page(&format!("<p>{links}</p>"));
    let plain = page("<p>x</p>");
    let names: Vec<String> = (0..OWN)
        .map(|i| format!("o{i}.xhtml"))
        .chain((0..ADDED).map(|i| format!("t/a{i}.xhtml")))
        .collect();
    let mut files: Vec<(&str, &[u8])> = vec![("toc.ncx", ncx.as_bytes())];
    for (i, n) in names.iter().enumerate() {
        files.push((
            n,
            if i == 0 {
                first.as_bytes()
            } else {
                plain.as_bytes()
            },
        ));
    }
    let (book, opf) = book_of(&opf, &files);
    assert_eq!(book.files.len(), OWN + ADDED);

    let start = std::time::Instant::now();
    let (entries, spine) = contents(&book, &opf);
    let took = start.elapsed();
    assert_eq!(entries.iter().map(|e| e.level).max(), Some(DEEP - 1));
    // Every NCX entry, the blank ones put back.
    assert_eq!(entries.len(), ENTRIES + DEEP);
    assert_eq!(entries[9].title, " ");
    assert_eq!(spine.len(), OWN + ADDED);
    // The added files in the entries' order: the last named first.
    assert_eq!(
        book.files[spine[OWN]].href,
        format!("t/a{}.xhtml", ADDED - 1)
    );
    assert_eq!(entries[0].href, format!("t/a{}.xhtml#x", ADDED - 1));
    assert!(took < crate::support::COST_LIMIT, "{took:?}");
    eprintln!(
        "{} entries over {} files: {took:?}",
        entries.len(),
        spine.len()
    );
}

/// An NCX nested deeper than `MAX_DEPTH` is not read, by Calibre's view or kfxgen's (an
/// existing limit): the book's contents are the ones Calibre makes.
#[test]
fn contents_nested_past_the_limit_are_not_read() {
    let mut deep = String::new();
    for d in (0..1_000).rev() {
        deep = format!(
            r#"<navPoint><navLabel><text>D{d}</text></navLabel><content src="c.xhtml"/>{deep}</navPoint>"#
        );
    }
    let ncx = format!(
        r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/><docTitle><text>T</text></docTitle><navMap>{deep}</navMap></ncx>"#
    );
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="c" href="c.xhtml" media-type="{XHTML}"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="c"/></spine></package>"#
    );
    let c = page("<h1>One</h1><p>x</p>");
    let (book, opf) = book_of(
        &opf,
        &[("toc.ncx", ncx.as_bytes()), ("c.xhtml", c.as_bytes())],
    );
    assert!(book.oeb.auto_generated_toc);
    assert!(opf.ncx.is_none());
    let (entries, _) = contents(&book, &opf);
    assert!(entries.iter().all(|e| e.level == 0), "{entries:?}");
}

/// A page of many note markers in one paragraph, their notes linking back, and ids inside
/// footnotes nested nearly as deep as a file may be: each note scan walks the file a fixed
/// number of times, so it costs little. Under a long path the keys, each repeating it, pass
/// the file's text budget.
#[test]
fn many_notes_and_links_cost_little() {
    const NOTES: usize = 40_000;
    const NESTED: usize = 200;
    let markers: String = (0..NOTES)
        .map(|i| {
            format!(r##" word <sup id="r{i}"><a epub:type="noteref" href="#n{i}">{i}</a></sup>"##)
        })
        .collect();
    let notes: String = (0..NOTES)
        .map(|i| format!(r##"<li id="n{i}">Note <a href="#r{i}">back</a></li>"##))
        .collect();
    let open: String = (0..NESTED)
        .map(|i| format!(r#"<aside epub:type="footnote" id="f{i}">"#))
        .collect();
    let body = format!(
        "<p>{markers}</p>{open}<ol>{notes}</ol>{}",
        "</aside>".repeat(NESTED)
    );
    let make = |dir: usize| {
        let href = format!("{}/c.xhtml", "d".repeat(dir));
        let opf = format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="c" href="{href}" media-type="{XHTML}"/></manifest><spine><itemref idref="c"/></spine></package>"#
        );
        let c = page(&body);
        (book_of(&opf, &[(href.as_str(), c.as_bytes())]).0, href)
    };
    let (book, href) = make(100);

    let start = std::time::Instant::now();
    let ids = note_target_ids(&book.files[0].dom);
    let pairs = note_pair_ids(&book, &budgets(&book)).unwrap();
    let targets = book_link_targets(&book, &budgets(&book)).unwrap();
    let footnotes = footnote_keys(&book, &budgets(&book)).unwrap();
    let took = start.elapsed();
    // The markers' ids (a noteref inside them) and the footnotes', each pair, every link
    // target, and every id inside the footnotes.
    assert_eq!(ids.len(), NOTES + NESTED);
    assert_eq!(pairs[&href].len(), 2 * NOTES);
    assert_eq!(targets.len(), 2 * NOTES);
    assert_eq!(footnotes.len(), NESTED + NOTES);
    assert!(took < crate::support::COST_LIMIT, "{took:?}");
    eprintln!("note scans over {} notes: {took:?}", NOTES);

    let (book, _) = make(2_000);
    let start = std::time::Instant::now();
    for err in [
        note_pair_ids(&book, &budgets(&book)).unwrap_err(),
        book_link_targets(&book, &budgets(&book)).unwrap_err(),
    ] {
        assert!(err.to_string().contains("500000"), "{err}");
    }
    assert!(start.elapsed() < crate::support::COST_LIMIT);
}
