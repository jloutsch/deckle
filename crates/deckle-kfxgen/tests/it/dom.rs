// SPDX-License-Identifier: GPL-3.0-only
//! The element tree (phase 2 plan Task 4): each spine file of the `cascade/` books parsed and
//! cleaned as Calibre 9.18 does, against the `<body>` of the tree Calibre's kfxgen run recorded
//! (`cascade/<book>/dom/NNN.xhtml`).

use std::collections::{BTreeMap, BTreeSet};

use deckle_kfx::epub_zip::{EpubReader, percent_decode};
use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::dom::{self, Dom, Error, NodeId};
use deckle_limits::MAX_DEPTH;
use deckle_limits::roxmltree::{Node, ParsingOptions};
use serde_json::Value;

use crate::support::Dump;

/// Every book under `fixtures/kfxgen/cascade/`.
const BOOKS: [&str; 14] = [
    "comments_in_tree",
    "css_quirks",
    "directions",
    "fake_margins",
    "fonts",
    "frames",
    "lists",
    "malformed",
    "navigation",
    "no_toc",
    "notes",
    "package_quirks",
    "rtl",
    "vertical",
];

/// Spine files whose body does not match the dump yet, and why. Each must still differ, and
/// match once the differences named are allowed, so an entry goes when the port catches up and
/// hides nothing else.
struct NotYet {
    book: &'static str,
    href: &'static str,
    /// Tags a later step renames: (from, the names it may become).
    renames: &'static [(&'static str, &'static [&'static str])],
    /// Attributes a later step removes.
    removed: &'static [&'static str],
    /// Attributes a later step adds.
    added: &'static [&'static str],
    why: &'static str,
}

const NOT_YET: &[NotYet] = &[
    NotYet {
        book: "css_quirks",
        href: "OEBPS/flatten.xhtml",
        renames: &[("font", &["span", "div"])],
        removed: &[
            "size", "face", "color", "align", "bgcolor", "valign", "type",
        ],
        added: &[],
        why: "Calibre's CSS flattener makes each <font> a <div> if it holds a block, else a \
              <span>, and turns presentational attributes into CSS \
              (flatcss.py CSSFlattener.flatten_node), after parse_html: Task 9",
    },
    NotYet {
        book: "malformed",
        href: "OEBPS/unclosed.xhtml",
        renames: &[("font", &["span"])],
        removed: &["size"],
        added: &[],
        why: "the same flattener rule for the <font> left open",
    },
    NotYet {
        book: "malformed",
        href: "OEBPS/html4.xhtml",
        renames: &[],
        removed: &["align"],
        added: &[],
        why: "the flattener turns align into CSS (flatcss.py CSSFlattener.flatten_node)",
    },
    NotYet {
        book: "lists",
        href: "OEBPS/c1.xhtml",
        renames: &[],
        removed: &["type"],
        added: &[],
        why: "the flattener turns an <ol>'s type into CSS (flatcss.py CSSFlattener.flatten_node)",
    },
    NotYet {
        book: "no_toc",
        href: "OEBPS/one.xhtml",
        renames: &[],
        removed: &[],
        added: &["id"],
        why: "with no contents file Calibre gives each heading it lists an id calibre_toc_N \
              (oeb/transforms/structure.py DetectStructure): dom::generated_toc, after the \
              parse, compared in css_cascade.rs",
    },
    NotYet {
        book: "no_toc",
        href: "OEBPS/two.xhtml",
        renames: &[],
        removed: &[],
        added: &["id"],
        why: "as one.xhtml",
    },
];

/// The style Calibre's chapter marks carry (`DetectStructure`, phase 2 Task 8).
fn is_chapter_mark(specified: &Value) -> bool {
    *specified == serde_json::json!({"display": "block", "page-break-after": "always"})
}

/// An element as the comparison sees it: its path, tag, text, tail and attributes but for
/// those a later step owns.
#[derive(Debug, Clone, PartialEq)]
struct El {
    path: Vec<u32>,
    tag: String,
    text: String,
    tail: String,
    attrs: Vec<(String, String)>,
}

/// Attributes Calibre's later steps own: classes and styles (the flattener), kfxgen's own,
/// and a picture's size (the Stylizer removes it).
fn compared(tag: &str, name: &str) -> bool {
    !(name == "class"
        || name == "style"
        || name.starts_with("data-kfxgen-")
        || (tag == "img" && (name == "width" || name == "height")))
}

/// The text of the text nodes from `first` on, up to the next element or comment: lxml's text
/// or tail.
fn text_run(first: Option<Node>) -> String {
    std::iter::successors(first, |n| n.next_sibling())
        .take_while(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

/// `<body>` and every element under it in the dump, chapter marks left out (their tails moved
/// as removing them would) and the paths counted without them.
fn dump_body(xhtml: &str, marks: &BTreeSet<Vec<u32>>) -> Vec<El> {
    let doc = deckle_limits::parse_xml(xhtml, ParsingOptions::default()).unwrap();
    let root = doc.root_element();
    let el = |n: Node, path: Vec<u32>| {
        let tag = n.tag_name().name().to_owned();
        let mut attrs: Vec<(String, String)> = n
            .attributes()
            .map(|a| {
                let name = match a.namespace() {
                    Some("http://www.w3.org/XML/1998/namespace") => format!("xml:{}", a.name()),
                    Some(uri) => format!("{}:{}", n.lookup_prefix(uri).unwrap_or(""), a.name()),
                    None => a.name().to_owned(),
                };
                (name, a.value().to_owned())
            })
            .filter(|(name, _)| compared(&tag, name))
            .collect();
        attrs.sort();
        El {
            path,
            text: text_run(n.first_child()),
            tail: text_run(n.next_sibling()),
            tag,
            attrs,
        }
    };
    let mut out: Vec<El> = Vec::new();
    // roxmltree node index → its record.
    let mut record: BTreeMap<usize, usize> = BTreeMap::new();
    // A removed mark's tail: (the kept sibling before it, else its parent; the tail).
    let mut moved: Vec<(Node, bool, String)> = Vec::new();
    // (node, its path in the dump, its path once the marks are gone)
    let mut stack: Vec<(Node, Vec<u32>, Vec<u32>)> = Vec::new();
    for (i, c) in root.children().filter(Node::is_element).enumerate() {
        if c.tag_name().name() == "body" {
            stack.push((c, vec![i as u32], vec![i as u32]));
        }
    }
    while let Some((n, dump_path, path)) = stack.pop() {
        record.insert(n.id().get_usize(), out.len());
        out.push(el(n, path.clone()));
        let mut kids = Vec::new();
        let mut prev_kept: Option<Node> = None;
        for (i, c) in n.children().filter(Node::is_element).enumerate() {
            let mut dp = dump_path.clone();
            dp.push(i as u32);
            if marks.contains(&dp) {
                let tail = text_run(c.next_sibling());
                match prev_kept {
                    Some(prev) => moved.push((prev, true, tail)),
                    None => moved.push((n, false, tail)),
                }
                continue;
            }
            let mut p = path.clone();
            p.push(kids.len() as u32);
            prev_kept = Some(c);
            kids.push((c, dp, p));
        }
        stack.extend(kids.into_iter().rev());
    }
    for (n, is_tail, extra) in moved {
        let e = &mut out[record[&n.id().get_usize()]];
        if is_tail {
            e.tail.push_str(&extra);
        } else {
            e.text.push_str(&extra);
        }
    }
    out
}

fn body(dom: &Dom) -> Option<NodeId> {
    dom.children(dom.root())
        .iter()
        .copied()
        .find(|&c| dom.tag(c) == "body")
}

/// `<body>` and every element under it, comments left out.
fn parsed_body(dom: &Dom) -> Vec<El> {
    let Some(b) = body(dom) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stack = vec![b];
    while let Some(n) = stack.pop() {
        let tag = dom.tag(n).to_owned();
        let mut attrs: Vec<(String, String)> = dom
            .attrs(n)
            .filter(|(name, ..)| compared(&tag, name))
            .map(|(name, _, value)| (name.to_owned(), value.to_owned()))
            .collect();
        attrs.sort();
        out.push(El {
            path: dom.path(n),
            text: dom.text(n).to_owned(),
            tail: dom.tail(n).to_owned(),
            tag,
            attrs,
        });
        stack.extend(
            dom.children(n)
                .iter()
                .rev()
                .filter(|&&c| !dom.is_comment(c)),
        );
    }
    out
}

/// The text of `n` and everything under it, in document order (lxml's `itertext`: a comment's
/// own text left out, its tail kept).
fn all_text(dom: &Dom, n: NodeId) -> String {
    let mut out = String::new();
    // (node, entering): on entering, its text then its children; on leaving, its tail.
    let mut stack = vec![(n, true)];
    while let Some((m, entering)) = stack.pop() {
        if entering && dom.is_comment(m) {
            out.push_str(dom.tail(m));
        } else if entering {
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

fn first_difference(got: &[El], want: &[El]) -> String {
    let i = got.iter().zip(want).take_while(|(g, w)| g == w).count();
    format!(
        "element {i}: got {:?}, want {:?} ({} and {} elements)",
        got.get(i),
        want.get(i),
        got.len(),
        want.len()
    )
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
            if let Some(ny) = NOT_YET.iter().find(|ny| ny.book == book && ny.href == href) {
                let allowed = |g: &El| El {
                    tag: ny
                        .renames
                        .iter()
                        .find(|(from, _)| *from == g.tag)
                        .map_or(g.tag.clone(), |_| "*renamed*".to_owned()),
                    attrs: g
                        .attrs
                        .iter()
                        .filter(|(name, _)| !ny.removed.contains(&name.as_str()))
                        .cloned()
                        .collect(),
                    ..g.clone()
                };
                let want_allowed = |g: &El, w: &El| {
                    let mut w = w.clone();
                    w.attrs
                        .retain(|(name, _)| !ny.added.contains(&name.as_str()));
                    if ny
                        .renames
                        .iter()
                        .any(|(from, to)| *from == g.tag && to.contains(&w.tag.as_str()))
                    {
                        w.tag = "*renamed*".to_owned();
                    }
                    w
                };
                assert!(
                    got.len() == want.len()
                        && got
                            .iter()
                            .zip(&want)
                            .all(|(g, w)| allowed(g) == want_allowed(g, w)),
                    "{book} {href}: differs by more than {:?}, {:?} and {:?}:\n{}",
                    ny.renames,
                    ny.removed,
                    ny.added,
                    first_difference(&got, &want)
                );
            }
            failed.push((book.to_owned(), href.clone(), first_difference(&got, &want)));
        }
    }
    let unexpected: Vec<_> = failed
        .iter()
        .filter(|(b, h, _)| !NOT_YET.iter().any(|ny| ny.book == b && ny.href == h))
        .collect();
    assert!(
        unexpected.is_empty(),
        "{matched} files match; these do not:\n{unexpected:#?}"
    );
    for ny in NOT_YET {
        assert!(
            failed
                .iter()
                .any(|(fb, fh, _)| fb == ny.book && fh == ny.href),
            "{} {} matches now; take it off NOT_YET ({})",
            ny.book,
            ny.href,
            ny.why
        );
    }
    assert_eq!(
        matched + failed.len(),
        50,
        "the 14 books have 50 spine files"
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
fn undeclared_non_utf8_decodes_as_windows_1252() {
    // Calibre takes uchardet's guess (`chardet.py` `force_encoding`); the port guesses
    // windows-1252, uchardet's answer for most Western text.
    assert_eq!(first_p_text(&dom::parse(&latin1_cafe()).unwrap()), "café");
}

/// `chapter("")` with `raw` as the paragraph's bytes and `decl` before it.
fn declared(decl: &str, raw: &[u8]) -> Vec<u8> {
    let mut bytes = decl.as_bytes().to_vec();
    let page = chapter("@");
    let (a, b) = page.split_once('@').unwrap();
    bytes.extend_from_slice(a.as_bytes());
    bytes.extend_from_slice(raw);
    bytes.extend_from_slice(b.as_bytes());
    bytes
}

#[test]
fn declared_legacy_encodings_decode() {
    // windows-1252 as Python's cp1252: curly quotes, the euro, and U+FFFD for 0x81, which
    // Python leaves undefined.
    let cp = declared(
        "<?xml version=\"1.0\" encoding=\"windows-1252\"?>",
        b"\x93Quoted\x94 \x805 \x81",
    );
    assert_eq!(
        first_p_text(&dom::parse(&cp).unwrap()),
        "\u{201c}Quoted\u{201d} \u{20ac}5 \u{fffd}"
    );
    // Another encoding the WHATWG table has.
    let koi = declared(
        "<?xml version=\"1.0\" encoding=\"KOI8-R\"?>",
        b"\xf0\xd2\xc9\xd7\xc5\xd4",
    );
    assert_eq!(first_p_text(&dom::parse(&koi).unwrap()), "Привет");
    // A name no codec has: UTF-8 with replacement characters, as `detect_xml_encoding` falls
    // back when `codecs.lookup` fails.
    let unknown = declared("<?xml version=\"1.0\" encoding=\"x-no-such\"?>", b"caf\xe9");
    assert_eq!(first_p_text(&dom::parse(&unknown).unwrap()), "caf\u{fffd}");
}

#[test]
fn formatting_rebuilt_with_many_attributes_stops() {
    // One `<b>` of 250 attributes (under `MAX_TAG_ATTRIBUTES`), left open, which the HTML
    // algorithm copies, attributes and all, into each of 150,000 paragraphs: 37.5 million
    // attributes from 1.2 MB. Unbounded, 36 million took 90 s in a debug build.
    let attrs: String = (0..250).map(|i| format!(" a{i}=v")).collect();
    let src = format!(
        "<html><body><p><b{attrs}>x</p>{}",
        "<p>x</p>".repeat(150_000)
    );
    let start = std::time::Instant::now();
    let r = dom::parse(src.as_bytes());
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
    assert!(matches!(r, Err(Error::TooLarge(_))), "{:?}", r.map(|_| ()));
}

#[test]
fn entities_past_16_mb_are_refused() {
    // A 1,000-character entity named 20,000 times: 20 MB from 81 KB.
    let src = format!(
        "<!DOCTYPE html [<!ENTITY a \"{}\">]><html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>{}</p></body></html>",
        "X".repeat(1000),
        "&a;".repeat(20_000)
    );
    match dom::parse(src.as_bytes()) {
        Err(e @ Error::EntityText(_)) => assert!(e.to_string().contains("16 MB"), "{e}"),
        other => panic!("expected EntityText, got {:?}", other.map(|_| ())),
    }
    // Under the cap they are replaced.
    let src = "<!DOCTYPE html [<!ENTITY who \"the author\">]><html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>By &who;.</p></body></html>";
    assert_eq!(
        first_p_text(&dom::parse(src.as_bytes()).unwrap()),
        "By the author."
    );
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
fn siblings_and_text_edits() {
    let mut dom = dom::parse(chapter("a<b>b</b>c<i>i</i>d<u>u</u>").as_bytes()).unwrap();
    let p = dom.children(body(&dom).unwrap())[0];
    let [b, i, u] = dom.children(p) else { panic!() };
    let (b, i, u) = (*b, *i, *u);
    assert_eq!([b, i, u].map(|n| dom.index_in_parent(n)), [0, 1, 2]);
    assert_eq!(dom.prev_sibling(b), None);
    assert_eq!(dom.prev_sibling(i), Some(b));
    assert_eq!(dom.next_sibling(i), Some(u));
    assert_eq!(dom.next_sibling(u), None);
    assert_eq!(dom.index_in_parent(dom.root()), 0);

    dom.set_text(i, "I");
    dom.set_tail(i, "D");
    let s = dom.append_child(p, "span");
    assert_eq!(dom.parent(s), Some(p));
    assert_eq!(dom.next_sibling(u), Some(s));
    assert!(dom::to_xhtml(&dom).contains("<p>a<b>b</b>c<i>I</i>D<u>u</u><span/></p>"));

    // Before an element already removed, or the root: nothing goes into the tree.
    dom.remove(b);
    let before = dom::to_xhtml(&dom);
    for at in [b, dom.root()] {
        let n = dom.insert_before(at, "div");
        assert_eq!(dom.parent(n), None);
        assert_eq!(dom::to_xhtml(&dom), before);
    }
}

#[test]
fn many_edits_on_wide_parents_are_linear() {
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>{}</body></html>",
        "<p>a</p>t".repeat(100_000)
    );
    let mut dom = dom::parse(src.as_bytes()).unwrap();
    let body = body(&dom).unwrap();
    let kids = dom.children(body).to_vec();

    let start = std::time::Instant::now();
    let items: Vec<(NodeId, &str)> = kids.iter().map(|&k| (k, "div")).collect();
    let made = dom.insert_many_before(&items);
    // One `insert_before` per paragraph took 68 s in a debug build.
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
    assert_eq!(dom.children(body).len(), 200_000);
    assert_eq!(dom.children(body)[0], made[0]);
    assert_eq!(dom.index_in_parent(kids[99_999]), 199_999);

    let start = std::time::Instant::now();
    dom.remove_many(&kids);
    // One `remove` per paragraph took 136 s in a debug build.
    assert!(
        start.elapsed() < crate::support::COST_LIMIT,
        "{:?}",
        start.elapsed()
    );
    assert_eq!(dom.children(body), made.as_slice());
    assert_eq!(dom.index_in_parent(made[99_999]), 99_999);
    // Each removed paragraph's tail joined the new element before it.
    assert_eq!(dom.tail(made[5]), "t");
    assert_eq!(all_text(&dom, body).len(), 100_000);
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
                start.elapsed() < crate::support::COST_LIMIT,
                "{:?}",
                start.elapsed()
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn a_trailing_comment_hides_lang_from_the_lower_casing() {
    // `parse_html` lower-cases every name when the root's last child (lxml's `root[-1]`) has
    // `LANG` or `DIR`. A comment after it is that last child, and has neither.
    let page = |after: &str| {
        format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><BODY LANG=\"en\"><P>x</P></BODY>{after}</html>"
        )
    };
    let tags = |src: String| {
        let dom = dom::parse(src.as_bytes()).unwrap();
        dom.descendants()
            .map(|n| dom.tag(n).to_owned())
            .collect::<Vec<_>>()
    };
    assert!(tags(page("")).contains(&"p".to_owned()));
    let kept = tags(page("<!-- c -->"));
    assert!(kept.contains(&"P".to_owned()), "{kept:?}");
}

/// The element of `dom` whose `id` is `id`.
fn by_id(dom: &Dom, id: &str) -> NodeId {
    dom.descendants()
        .find(|&n| dom.attr(n, "id") == Some(id))
        .unwrap_or_else(|| panic!("no element with id {id}"))
}

#[test]
fn a_comment_is_a_child_as_in_lxml() {
    let src = chapter("one<a id=\"x\"><!--c--></a>two<!--d-->three<?pi data?>four<b>b</b>");
    // Read as XML, then as HTML (a stray `</i>` makes it not well-formed).
    for src in [src.clone(), src.replace("four", "four</i>")] {
        let dom = dom::parse(src.as_bytes()).unwrap();
        let a = by_id(&dom, "x");
        // lxml's `len(a)`: the comment is a child.
        let [c] = dom.children(a) else {
            panic!("{:?}", dom.children(a))
        };
        assert!(dom.is_comment(*c));
        assert!(!dom.is_comment(a));
        assert_eq!(dom.parent(*c), Some(a));
        assert_eq!(dom.text(*c), "c");
        assert_eq!(dom.text(a), "");
        assert_eq!(dom.itertext(a), "");

        // Around a comment the text reads as before: lxml's `text` stops at it, its tail
        // goes on, and `itertext` leaves out its own text.
        let p = dom.parent(a).unwrap();
        let kids = dom.children(p).to_vec();
        assert_eq!(
            kids.len(),
            4,
            "a, the comment, the PI (a comment as HTML), b"
        );
        let (d, b) = (kids[1], kids[3]);
        assert_eq!(dom.prev_sibling(d), Some(a));
        assert_eq!(dom.next_sibling(a), Some(d));
        assert_eq!(dom.prev_sibling(b), Some(kids[2]));
        assert_eq!(dom.text(p), "one");
        assert_eq!(dom.tail(a), "two");
        assert_eq!(dom.text(d), "d");
        assert_eq!(dom.tail(d), "three");
        assert_eq!(dom.tail(kids[2]), "four");
        assert_eq!(dom.itertext(p), "onetwothreefourb");
        assert_eq!(all_text(&dom, p), "onetwothreefourb");

        // Paths, `index_in_parent` and `descendants` count elements only.
        assert_eq!(dom.index_in_parent(b), 1);
        assert_eq!(dom.path(b).last(), Some(&1));
        assert!(dom.descendants().all(|n| !dom.is_comment(n)));
        // `subtree` is lxml's `iter()`: comments and processing instructions included.
        assert_eq!(
            dom.subtree(p).collect::<Vec<_>>(),
            [p, a, *c, d, kids[2], b]
        );
        assert_eq!(dom.subtree(d).collect::<Vec<_>>(), [d]);
        assert_eq!(dom.ancestors(*c).next(), Some(a));
        assert_eq!(dom.ancestors(*c).last(), Some(dom.root()));
        assert_eq!(dom.ancestors(dom.root()).next(), None);
    }
}

#[test]
fn attr_by_namespace_ignores_the_prefix() {
    let src = "<html xmlns=\"http://www.w3.org/1999/xhtml\" \
        xmlns:e=\"http://www.idpf.org/2007/ops\"><head><title>T</title></head>\
        <body><aside id=\"n\" e:type=\"footnote\" xml:lang=\"fr\" type=\"plain\">x</aside>\
        </body></html>";
    let dom = dom::parse(src.as_bytes()).unwrap();
    let n = by_id(&dom, "n");
    let ops = "http://www.idpf.org/2007/ops";
    assert_eq!(dom.attr_ns(n, ops, "type"), Some("footnote"));
    assert_eq!(dom.attr(n, "epub:type"), None);
    assert_eq!(dom.attr(n, "e:type"), Some("footnote"));
    assert_eq!(
        dom.attr_ns(n, "http://www.w3.org/XML/1998/namespace", "lang"),
        Some("fr")
    );
    // No namespace: the attribute with no prefix.
    assert_eq!(dom.attr_ns(n, "", "type"), Some("plain"));
    assert_eq!(dom.attr_ns(n, ops, "lang"), None);
    assert_eq!(dom.attr_ns(n, "http://example.com/", "type"), None);
    assert_eq!(
        dom.qualified(n),
        (Some("http://www.w3.org/1999/xhtml"), "aside")
    );
}

/// The text of every text node under the `<body>` of a `dom/` dump, in document order: lxml's
/// `itertext()`, which leaves out the text of comments and processing instructions.
fn dump_body_text(xhtml: &str) -> String {
    let doc = deckle_limits::parse_xml(xhtml, ParsingOptions::default()).unwrap();
    let body = doc
        .root_element()
        .children()
        .find(|c| c.tag_name().name() == "body")
        .unwrap();
    body.descendants()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .collect()
}

#[test]
fn itertext_matches_the_dump() {
    // The tree as kfxgen gets it, which the dump recorded: flattened, chapter marks in.
    let mut files = 0;
    for book in BOOKS {
        let read = read_book(&fixtures::kfxgen(&format!("sources/{book}.epub")))
            .unwrap_or_else(|e| panic!("{book}: {e}"));
        let dump = Dump::fixture(book);
        for f in &read.files {
            // `<body>`: the dump's `<head>` still holds Calibre's stylesheet links and their
            // tails, which kfxgen's tree has dropped.
            assert_eq!(
                f.dom.itertext(body(&f.dom).unwrap()),
                dump_body_text(&dump.xhtml(&f.href)),
                "{book} {}",
                f.href
            );
            files += 1;
        }
    }
    assert_eq!(files, 50, "the 14 books have 50 spine files");
}

#[test]
fn head_whitespace_resets_comment_tails_as_calibre() {
    // `comments_in_tree` has a comment between the head's children: `parse_html` resets the
    // tail of each child of the head, the comment's too. The dump's head ends in the
    // stylesheet links Calibre adds later, after the children compared here.
    let dump = Dump::fixture("comments_in_tree");
    let doc_text = dump.xhtml("OEBPS/c1.xhtml");
    let doc = deckle_limits::parse_xml(&doc_text, ParsingOptions::default()).unwrap();
    let head = doc.root_element().first_element_child().unwrap();
    let want: Vec<(bool, String, String)> = head
        .children()
        .filter(|n| n.is_element() || n.is_comment())
        .map(|n| {
            let name = if n.is_comment() {
                n.text().unwrap_or("").to_owned()
            } else {
                n.tag_name().name().to_owned()
            };
            (n.is_comment(), name, text_run(n.next_sibling()))
        })
        .collect();

    let mut zip = EpubReader::open(&fixtures::kfxgen("sources/comments_in_tree.epub")).unwrap();
    let dom = dom::parse(&read_spine_file(&mut zip, "OEBPS/c1.xhtml")).unwrap();
    let head = dom.children(dom.root())[0];
    // The book's own `<link>` and `<style>` go when the styles are flattened.
    let got: Vec<(bool, String, String)> = dom
        .children(head)
        .iter()
        .filter(|&&c| !matches!(dom.tag(c), "link" | "style"))
        .map(|&c| {
            let name = if dom.is_comment(c) {
                dom.text(c)
            } else {
                dom.tag(c)
            };
            (dom.is_comment(c), name.to_owned(), dom.tail(c).to_owned())
        })
        .collect();
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got, want[..3]);
    assert!(got[1].0, "the comment is the head's second child");
}
