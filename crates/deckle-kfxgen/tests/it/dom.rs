// SPDX-License-Identifier: GPL-3.0-only
//! The element tree (phase 2 plan Task 4): each spine file of the `cascade/` books parsed and
//! cleaned as Calibre 9.18 does, against the `<body>` of the tree Calibre's kfxgen run recorded
//! (`cascade/<book>/dom/NNN.xhtml`).

use std::collections::BTreeSet;

use deckle_kfx::epub_zip::{EpubReader, percent_decode};
use deckle_kfx::fixtures;
use deckle_kfxgen::dom::{self, Dom, Error, NodeId};
use deckle_limits::MAX_DEPTH;
use deckle_limits::roxmltree::{Node, ParsingOptions};
use serde_json::Value;

/// Every book under `fixtures/kfxgen/cascade/`.
const BOOKS: [&str; 12] = [
    "css_quirks",
    "directions",
    "fake_margins",
    "fonts",
    "frames",
    "lists",
    "navigation",
    "no_toc",
    "notes",
    "package_quirks",
    "rtl",
    "vertical",
];

/// Tags a later step renames: (from, the names it may become).
type Renames = &'static [(&'static str, &'static [&'static str])];

/// Spine files whose body does not match the dump yet: (book, href, the tags a later step
/// renames and what to, why). Each must still differ, and match but for those renames, so an
/// entry goes when the port catches up and hides nothing else.
const NOT_YET: &[(&str, &str, Renames, &str)] = &[(
    "css_quirks",
    "OEBPS/flatten.xhtml",
    &[("font", &["span", "div"])],
    "Calibre's CSS flattener makes each <font> a <div> if it holds a block, else a <span> \
     (flatcss.py CSSFlattener.flatten_node), after parse_html: Task 9",
)];

/// The style Calibre's chapter marks carry (`DetectStructure`, phase 2 Task 8).
fn is_chapter_mark(specified: &Value) -> bool {
    *specified == serde_json::json!({"display": "block", "page-break-after": "always"})
}

/// `(path, tag)` of `<body>` and every element under it in the dump, chapter marks left out
/// and the paths counted without them.
fn dump_body(xhtml: &str, marks: &BTreeSet<Vec<u32>>) -> Vec<(Vec<u32>, String)> {
    let doc = deckle_limits::parse_xml(xhtml, ParsingOptions::default()).unwrap();
    let root = doc.root_element();
    let mut out = Vec::new();
    // (node, its path in the dump, its path once the marks are gone)
    let mut stack: Vec<(Node, Vec<u32>, Vec<u32>)> = Vec::new();
    for (i, c) in root.children().filter(Node::is_element).enumerate() {
        if c.tag_name().name() == "body" {
            stack.push((c, vec![i as u32], vec![i as u32]));
        }
    }
    while let Some((n, dump_path, path)) = stack.pop() {
        out.push((path.clone(), n.tag_name().name().to_owned()));
        let mut kids = Vec::new();
        let mut kept = 0;
        for (i, c) in n.children().filter(Node::is_element).enumerate() {
            let mut dp = dump_path.clone();
            dp.push(i as u32);
            if marks.contains(&dp) {
                continue;
            }
            let mut p = path.clone();
            p.push(kept);
            kept += 1;
            kids.push((c, dp, p));
        }
        stack.extend(kids.into_iter().rev());
    }
    out
}

fn body(dom: &Dom) -> Option<NodeId> {
    dom.children(dom.root())
        .iter()
        .copied()
        .find(|&c| dom.tag(c) == "body")
}

/// `(path, tag)` of `<body>` and every element under it.
fn parsed_body(dom: &Dom) -> Vec<(Vec<u32>, String)> {
    let Some(b) = body(dom) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stack = vec![b];
    while let Some(n) = stack.pop() {
        out.push((dom.path(n), dom.tag(n).to_owned()));
        stack.extend(dom.children(n).iter().rev());
    }
    out
}

/// The text of `n` and everything under it, in document order (lxml's `itertext`).
fn all_text(dom: &Dom, n: NodeId) -> String {
    let mut out = String::new();
    // (node, entering): on entering, its text then its children; on leaving, its tail.
    let mut stack = vec![(n, true)];
    while let Some((m, entering)) = stack.pop() {
        if entering {
            out.push_str(dom.text(m));
            if m != n {
                stack.push((m, false));
            }
            stack.extend(dom.children(m).iter().rev().map(|&c| (c, true)));
        } else {
            out.push_str(dom.tail(m));
        }
    }
    out
}

fn read_spine_file(zip: &mut EpubReader, href: &str) -> Vec<u8> {
    zip.read_bytes(href)
        .or_else(|_| zip.read_bytes(&percent_decode(href)))
        .unwrap()
}

#[test]
fn parse_cleanups_match_calibre() {
    let mut matched = 0;
    let mut failed: Vec<(String, String, String)> = Vec::new();
    for book in BOOKS {
        let dir = fixtures::kfxgen(&format!("cascade/{book}"));
        let files: serde_json::Map<String, Value> =
            serde_json::from_slice(&std::fs::read(dir.join("files.json")).unwrap()).unwrap();
        let mut zip = EpubReader::open(&fixtures::kfxgen(&format!("sources/{book}.epub"))).unwrap();
        for (href, n) in &files {
            let n = n.as_str().unwrap();
            let styles: Value = serde_json::from_slice(
                &std::fs::read(dir.join(format!("styles/{n}.json"))).unwrap(),
            )
            .unwrap();
            let table = styles["table"].as_array().unwrap();
            let marks: BTreeSet<Vec<u32>> = styles["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| is_chapter_mark(&table[e[1].as_u64().unwrap() as usize]["specified"]))
                .map(|e| {
                    e[0].as_array()
                        .unwrap()
                        .iter()
                        .map(|i| i.as_u64().unwrap() as u32)
                        .collect()
                })
                .collect();
            let xhtml = std::fs::read_to_string(dir.join(format!("dom/{n}.xhtml"))).unwrap();
            let want = dump_body(&xhtml, &marks);
            let dom = dom::parse(&read_spine_file(&mut zip, href))
                .unwrap_or_else(|e| panic!("{book} {href}: {e}"));
            let got = parsed_body(&dom);
            if got == want {
                matched += 1;
                continue;
            }
            if let Some((.., renames, _)) =
                NOT_YET.iter().find(|(b, h, ..)| *b == book && h == href)
            {
                let same = |(gp, gt): &(Vec<u32>, String), (wp, wt): &(Vec<u32>, String)| {
                    gp == wp
                        && (gt == wt
                            || renames
                                .iter()
                                .any(|(from, to)| from == gt && to.contains(&wt.as_str())))
                };
                assert!(
                    got.len() == want.len() && got.iter().zip(&want).all(|(g, w)| same(g, w)),
                    "{book} {href}: differs by more than {renames:?}:\n{got:?}\n{want:?}"
                );
            }
            let i = got.iter().zip(&want).take_while(|(g, w)| g == w).count();
            failed.push((
                book.to_owned(),
                href.clone(),
                format!(
                    "first difference at element {i}: got {:?}, want {:?} ({} and {} elements)",
                    got.get(i),
                    want.get(i),
                    got.len(),
                    want.len()
                ),
            ));
        }
    }
    let unexpected: Vec<_> = failed
        .iter()
        .filter(|(b, h, _)| !NOT_YET.iter().any(|(nb, nh, ..)| nb == b && nh == h))
        .collect();
    assert!(
        unexpected.is_empty(),
        "{matched} files match; these do not:\n{unexpected:#?}"
    );
    for (b, h, _, why) in NOT_YET {
        assert!(
            failed.iter().any(|(fb, fh, _)| fb == b && fh == h),
            "{b} {h} matches now; take it off NOT_YET ({why})"
        );
    }
    assert_eq!(
        matched + failed.len(),
        45,
        "the 12 books have 45 spine files"
    );
}

#[test]
fn malformed_xhtml_loads_with_all_its_text() {
    // Not well-formed (an unclosed `<p>` and `<img>`, a stray `</i>`, a bare `&`): Calibre
    // falls back to its HTML parser, the port to html5ever.
    let src = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
        <html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head>\n\
        <body><p>One &nbsp;two <b>three<p>four</i> five & six</p><img src=\"a.png\"> seven\
        <BR>eight</body></html>";
    let dom = dom::parse(src.as_bytes()).unwrap();
    let text = all_text(&dom, body(&dom).unwrap());
    let mut at = 0;
    for word in [
        "One",
        "\u{a0}two",
        "three",
        "four",
        "five & six",
        "seven",
        "eight",
    ] {
        let found = text[at..].find(word);
        assert!(found.is_some(), "{word:?} missing after {at} in {text:?}");
        at += found.unwrap() + word.len();
    }
    // Cleaned as Calibre cleans: a head with its title and the charset meta, names in lower
    // case, in the XHTML namespace.
    let head = dom.children(dom.root())[0];
    let head_tags: Vec<&str> = dom.children(head).iter().map(|&c| dom.tag(c)).collect();
    assert_eq!(head_tags, ["title", "meta"]);
    assert!(dom.descendants().any(|n| dom.tag(n) == "br"));
    assert!(dom.descendants().all(|n| dom.tag(n) != "BR"));
}

fn chapter(body: &str) -> String {
    format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body><p>{body}</p></body></html>"
    )
}

/// A chapter whose paragraph is `café` in Latin-1, with no encoding declared.
fn latin1_cafe() -> Vec<u8> {
    let mut bytes = chapter("caf").into_bytes();
    let at = bytes.windows(3).rposition(|w| w == b"caf").unwrap() + 3;
    bytes.insert(at, 0xe9);
    bytes
}

fn first_p_text(dom: &Dom) -> String {
    let p = dom.children(body(dom).unwrap())[0];
    dom.text(p).to_owned()
}

#[test]
fn utf16_and_latin1_decode() {
    let text = chapter("café 漢字 😀");
    let le: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    let be: Vec<u8> = [0xfe, 0xff]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
        .collect();
    for bytes in [le, be] {
        assert_eq!(first_p_text(&dom::parse(&bytes).unwrap()), "café 漢字 😀");
    }

    // Latin-1 by its XML declaration, and by a `<meta>` charset.
    for decl in [
        "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>",
        "<?xml version='1.0' encoding='latin1'?>",
    ] {
        let mut bytes = decl.as_bytes().to_vec();
        bytes.extend_from_slice(&latin1_cafe());
        assert_eq!(first_p_text(&dom::parse(&bytes).unwrap()), "café", "{decl}");
    }
    let mut meta = b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\"/><title>T</title></head><body><p>\xbfqu\xe9?</p></body></html>".to_vec();
    let dom = dom::parse(&meta).unwrap();
    assert_eq!(first_p_text(&dom), "¿qué?");
    // The encoding's `<meta>` is gone; Calibre's own is last in the head.
    let head = dom.children(dom.root())[0];
    let metas: Vec<NodeId> = dom
        .children(head)
        .iter()
        .copied()
        .filter(|&c| dom.tag(c) == "meta")
        .collect();
    assert_eq!(metas.len(), 1);
    assert_eq!(
        dom.attr(metas[0], "content"),
        Some("text/html; charset=utf-8")
    );

    // Declared UTF-8 that is not: decoded with replacement characters, as Python's
    // `decode('utf-8', 'replace')`.
    meta.splice(
        0..0,
        b"<?xml version=\"1.0\" encoding=\"utf-8\"?>"
            .iter()
            .copied(),
    );
    let text = first_p_text(&dom::parse(&meta).unwrap());
    assert_eq!(text, "\u{fffd}qu\u{fffd}?");
}

#[test]
fn undeclared_non_utf8_is_a_typed_error() {
    let bytes = latin1_cafe();
    assert!(matches!(dom::parse(&bytes), Err(Error::UndeclaredEncoding)));
    // An encoding the port does not decode is named.
    let mut koi = b"<?xml version=\"1.0\" encoding=\"KOI8-R\"?>".to_vec();
    koi.extend_from_slice(&bytes);
    match dom::parse(&koi) {
        Err(e @ Error::UnsupportedEncoding(_)) => assert!(e.to_string().contains("KOI8-R"), "{e}"),
        other => panic!("expected UnsupportedEncoding, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn ten_thousand_nested_spans_do_not_overflow() {
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{}deep{}</body></html>",
        "<span>".repeat(10_000),
        "</span>".repeat(10_000)
    );
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || {
            // Too deep for `parse_xml`, so read as HTML, where it is refused when the tree
            // passes MAX_DEPTH, before `html5ever` has to scan an open-element stack that
            // deep for every tag.
            match dom::parse(src.as_bytes()) {
                Err(Error::TooDeep) => {}
                Ok(dom) => panic!(
                    "read, {} deep",
                    dom.descendants().map(|n| dom.path(n).len()).max().unwrap()
                ),
                Err(e) => panic!("{e}"),
            }
        })
        .unwrap()
        .join()
        .unwrap();
    // 255 levels under `<body>` is within the limit, and read as XML.
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{}deep{}</body></html>",
        "<span>".repeat(254),
        "</span>".repeat(254)
    );
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(move || {
            let dom = dom::parse(src.as_bytes()).unwrap();
            let deepest = dom.descendants().map(|n| dom.path(n).len()).max().unwrap();
            assert_eq!(deepest, 255);
            assert!(deepest <= MAX_DEPTH);
            assert_eq!(all_text(&dom, body(&dom).unwrap()), "deep");
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn edits_keep_paths_and_text() {
    let mut dom = dom::parse(chapter("a<b>b</b>c<i>i</i>d").as_bytes()).unwrap();
    let body = body(&dom).unwrap();
    let p = dom.children(body)[0];
    let (b, i) = (dom.children(p)[0], dom.children(p)[1]);
    assert_eq!(dom.path(i), [1, 0, 1]);
    assert_eq!(dom.parent(i), Some(p));

    let mark = dom.insert_before(p, "div");
    assert_eq!(dom.children(body), [mark, p]);
    assert_eq!(dom.path(i), [1, 1, 1]);
    assert_eq!(dom.tag(mark), "div");

    // A removed element's tail stays where it was.
    dom.remove(b);
    assert_eq!(dom.children(p), [i]);
    assert_eq!(dom.text(p), "ac");
    assert_eq!(dom.path(i), [1, 1, 0]);
    assert_eq!(dom.parent(b), None);
    assert_eq!(all_text(&dom, p), "acid");

    dom.rename(i, "em");
    dom.set_attr(i, "class", Some("x"));
    assert_eq!(dom.attr(i, "class"), Some("x"));
    dom.set_attr(i, "class", None);
    assert_eq!(dom.attr(i, "class"), None);
    assert!(dom::to_xhtml(&dom).contains("<p>ac<em>i</em>d</p>"));
}

#[test]
fn to_xhtml_reads_back_as_the_same_tree() {
    let mut zip = EpubReader::open(&fixtures::kfxgen("sources/css_quirks.epub")).unwrap();
    let dom = dom::parse(&zip.read_bytes("OEBPS/cleanups.xhtml").unwrap()).unwrap();
    let out = dom::to_xhtml(&dom);
    let again = dom::parse(out.as_bytes()).unwrap();
    assert_eq!(parsed_body(&again), parsed_body(&dom));
    assert_eq!(dom::to_xhtml(&again), out);
    // Namespaced attributes keep their prefix, elements in no namespace say so.
    assert!(
        out.contains("<span xmlns=\"\">no namespace</span>"),
        "{out}"
    );
}

#[test]
fn html_that_rebuilds_formatting_at_every_block_stops() {
    // 200 open `<b>`s closed by `</div>` stay in the HTML algorithm's list of formatting
    // elements, which rebuilds all 200 in each of 5,000 paragraphs: a million elements from
    // 60 KB. The parse stops at a budget linear in the input.
    let bold: String = (0..200).map(|i| format!("<b id=\"b{i}\">")).collect();
    let src = format!(
        "<html><body><div>{bold}</div>{}</body></html>",
        "<p>x</p>".repeat(5000)
    );
    // The XML attempt comes first, and `roxmltree` recurses 200 levels before it finds the
    // stray `</div>`: more than a debug build's 2 MiB test thread holds.
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let start = std::time::Instant::now();
            assert!(matches!(
                dom::parse(src.as_bytes()),
                Err(Error::TooLarge(_))
            ));
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "{:?}",
                start.elapsed()
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
