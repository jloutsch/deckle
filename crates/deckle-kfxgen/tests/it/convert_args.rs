// SPDX-License-Identifier: GPL-3.0-only
//! The book-level arguments of `generate_full_book`, the facts kfxgen reads in the OPF and NCX
//! Calibre rewrote, and kfxgen's href helpers on hostile input (plan Task 6).

use std::collections::{BTreeMap, HashSet};

use deckle_kfx::epub_zip::percent_decode;
use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::convert;
use deckle_kfxgen::convert::{CalibreOpf, ConvertOptions, Ncx, book_args};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;
use roxmltree::{Document, Node, ParsingOptions};

use crate::convert_support::model_diff;

/// Arguments that cannot match yet, with why: (source, argument). Empty is the goal.
const SKIP: &[(&str, &str)] = &[];

/// Every source fixture: each has a model kfxgen made from it.
fn sources() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixtures::kfxgen("sources"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".epub").map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

fn dict(v: &PyValue) -> BTreeMap<String, &PyValue> {
    let PyValue::Dict(items) = v else {
        panic!("not a dict: {v:?}")
    };
    items
        .iter()
        .map(|(k, v)| match k {
            PyValue::Str(k) => (k.clone(), v),
            k => panic!("a key that is not a string: {k:?}"),
        })
        .collect()
}

#[test]
fn book_arguments_match_every_model() {
    let names = sources();
    assert_eq!(names.len(), 55, "the source fixtures");
    let mut failures = Vec::new();
    let mut compared = 0;
    for name in &names {
        let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let model = load_model(&fixtures::kfxgen(&format!("models/{name}")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let want = dict(&model.args);
        let opf = CalibreOpf::new(&book);
        let ours = book_args(&book, &opf, &ConvertOptions::default());
        assert_eq!(ours.len(), 13);
        for (arg, value) in &ours {
            if SKIP.contains(&(name.as_str(), arg.as_str())) {
                continue;
            }
            let Some(dumped) = want.get(arg) else {
                failures.push(format!("{name}: {arg} not in the model"));
                continue;
            };
            for line in model_diff(value, dumped) {
                failures.push(format!("{name}: [{arg:?}]{line}"));
            }
            compared += 1;
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(compared, 55 * 13 - SKIP.len());
}

#[test]
fn options_change_their_arguments() {
    let book = read_book(&fixtures::kfxgen("sources/page_list.epub")).unwrap();
    let opf = CalibreOpf::new(&book);
    let opts = ConvertOptions {
        open_at_cover: true,
        disable_approximate_pages: true,
        ..ConvertOptions::default()
    };
    let args: BTreeMap<String, PyValue> = book_args(&book, &opf, &opts).into_iter().collect();
    let landmarks = PyValue::List(vec![PyValue::Tuple(
        ["toc", "Contents", "contents.xhtml"]
            .map(|s| PyValue::Str(s.into()))
            .to_vec(),
    )]);
    assert_eq!(args["landmarks"], landmarks, "no start landmark");
    assert_eq!(args["approximate_pages"], PyValue::Bool(false));
}

/// `text` parsed, or None when it is not well-formed, as lxml's `etree.parse` fails on it.
fn parse(text: &str) -> Option<Document<'_>> {
    let opts = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, opts).ok()
}

fn named(n: &Node, local: &str) -> bool {
    n.is_element() && n.tag_name().name() == local
}

fn text_under(n: &Node) -> String {
    n.descendants()
        .filter(|d| d.is_text())
        .filter_map(|d| d.text())
        .collect()
}

/// `posixpath.normpath` for the plain relative paths of the fixtures.
fn normpath(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// What kfxgen reads in `calibre_opf.xml` and `calibre_ncx.xml`, read here with XPath's rules
/// written out again: the comparison for [`CalibreOpf`].
fn calibre_facts(name: &str) -> CalibreOpf {
    let dir = fixtures::kfxgen(&format!("convert/{name}"));
    let text = std::fs::read_to_string(dir.join("calibre_opf.xml")).unwrap();
    let doc = parse(&text).unwrap();
    let parent_is = |n: &Node, local: &str| n.parent().is_some_and(|p| named(&p, local));
    let items: Vec<Node> = doc
        .descendants()
        .filter(|n| named(n, "item") && parent_is(n, "manifest"))
        .collect();
    let mut ids = BTreeMap::new();
    for i in &items {
        ids.insert(
            i.attribute("id").unwrap_or(""),
            i.attribute("href").unwrap_or(""),
        );
    }
    let file = |href: &str| {
        let path = percent_decode(href.split('#').next().unwrap());
        path.rsplit('/').next().unwrap().to_owned()
    };
    let path = |href: &str| normpath(&percent_decode(href));
    let nav_path = items
        .iter()
        .find(|i| format!(" {} ", i.attribute("properties").unwrap_or("")).contains(" nav "))
        .map(|i| path(i.attribute("href").unwrap()));
    let ncx_path = items
        .iter()
        .find(|i| i.attribute("media-type") == Some("application/x-dtbncx+xml"))
        .map(|i| path(i.attribute("href").unwrap()));
    let ncx_text = ncx_path
        .as_ref()
        .map(|_| std::fs::read_to_string(dir.join("calibre_ncx.xml")).unwrap());
    // kfxgen reads an NCX that is not well-formed as none.
    let ncx_doc = ncx_text.as_deref().and_then(parse);
    let ncx = ncx_path.zip(ncx_doc).map(|(ncx_path, doc)| {
        let src = |n: &Node| {
            n.children()
                .filter(|c| named(c, "content"))
                .find_map(|c| c.attribute("src"))
                .unwrap_or("")
                .to_owned()
        };
        let page_targets = doc
            .descendants()
            .filter(|n| named(n, "pageTarget"))
            .map(|t| {
                let label = t
                    .children()
                    .filter(|c| named(c, "navLabel"))
                    .map(|l| text_under(&l))
                    .collect();
                (label, src(&t))
            })
            .collect();
        fn walk(
            n: &Node,
            depth: usize,
            src: &dyn Fn(&Node) -> String,
            out: &mut Vec<(String, String, usize)>,
        ) {
            for p in n.children().filter(|c| named(c, "navPoint")) {
                let label = p
                    .children()
                    .filter(|c| named(c, "navLabel"))
                    .flat_map(|l| l.children().filter(|t| named(t, "text")))
                    .map(|t| text_under(&t))
                    .collect();
                out.push((label, src(&p), depth));
                walk(&p, depth + 1, src, out);
            }
        }
        let mut points = Vec::new();
        for map in doc.descendants().filter(|n| named(n, "navMap")) {
            walk(&map, 0, &src, &mut points);
        }
        Ncx {
            path: ncx_path,
            page_targets,
            points,
        }
    });
    CalibreOpf {
        dates: doc
            .descendants()
            .filter(|n| named(n, "date") && parent_is(n, "metadata"))
            .map(|n| text_under(&n))
            .collect(),
        listed: doc
            .descendants()
            .filter(|n| named(n, "itemref") && parent_is(n, "spine"))
            .map(|r| {
                file(
                    ids.get(r.attribute("idref").unwrap_or(""))
                        .copied()
                        .unwrap_or(""),
                )
            })
            .collect::<HashSet<_>>(),
        guide: doc
            .descendants()
            .filter(|n| named(n, "reference") && parent_is(n, "guide"))
            .map(|r| {
                let a = |k| r.attribute(k).unwrap_or("").to_owned();
                (a("type"), a("title"), a("href"))
            })
            .collect(),
        nav_path,
        nav: None,
        ncx,
    }
}

#[test]
fn calibre_opf_facts_match_what_calibre_wrote() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for name in sources() {
        let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub"))).unwrap();
        let ours = CalibreOpf {
            // The nav's links are not in the dumps; the models check them (landmarks, page
            // list).
            nav: None,
            ..CalibreOpf::new(&book)
        };
        let want = calibre_facts(&name);
        if ours != want {
            failures.push(format!("{name}:\n  ours {ours:?}\n  dump {want:?}"));
        }
        checked += 1;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(checked, 55);
}

/// `n` layers of percent-encoding of `s`: every byte `%XX`, then each `%` written `%25` once
/// per further layer, as `urllib.parse.quote(s, safe="")` writes it.
fn layers(s: &str, n: usize) -> String {
    let once: String = s.bytes().map(|b| format!("%{b:02X}")).collect();
    once.replace('%', &format!("%{}", "25".repeat(n - 1)))
}

/// (href, `_is_unsafe_href`, `_normalize_href`, `_resolve_doc_path`, `_key_doc`,
/// `_resolve_link_target`).
type Row = (
    String,
    bool,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

#[test]
fn unsafe_hrefs_give_an_empty_target() {
    let base = "text/c1.xhtml";
    // Each from `base`, as kfxgen `ad38ec8` gives them (run with CPython on these inputs).
    let mut rows: Vec<Row> = vec![];
    for href in [
        "../../etc/passwd",
        "..\\..\\etc\\passwd",
        "/etc/passwd",
        "\\\\server\\share\\x",
        "C:/Windows/x",
        "c:x",
        "file:///etc/passwd",
        "FILE:x",
        "data:text/html,x",
        "javascript:alert(1)",
        "http://evil/x",
        "/",
        "%2F",
        "%2e%2e/%2e%2e/etc/passwd",
        "%252e%252e/etc/passwd",
        "a/..%2f..%2fetc",
        "text/../../x",
        "..",
    ] {
        rows.push((href.to_owned(), true, "", "", "", None));
    }
    rows.push((layers("../etc/passwd", 1), true, "", "", "", None));
    rows.push((layers("../etc/passwd", 16), true, "", "", "", None));
    rows.extend([
        // A leading `..` skips the check; the rest is one name in `text/`, decoded in the key.
        (
            "..%2f..%2fetc%2fpasswd".to_owned(),
            true,
            "",
            "text/..%2f..%2fetc%2fpasswd",
            "text/../../etc/passwd",
            Some("text/../../etc/passwd"),
        ),
        (
            "../http://evil".to_owned(),
            true,
            "",
            "http:/evil",
            "http:/evil",
            Some("http:/evil"),
        ),
        (
            "%ff%fe.xhtml".to_owned(),
            false,
            "%ff%fe.xhtml",
            "text/%ff%fe.xhtml",
            "text/\u{fffd}\u{fffd}.xhtml",
            Some("text/\u{fffd}\u{fffd}.xhtml"),
        ),
        (
            "x\0y.xhtml".to_owned(),
            false,
            "x\0y.xhtml",
            "text/x\0y.xhtml",
            "text/x\0y.xhtml",
            Some("text/x\0y.xhtml"),
        ),
        (
            "a\\b.xhtml".to_owned(),
            false,
            "b.xhtml",
            "text/a/b.xhtml",
            "text/a/b.xhtml",
            Some("text/a/b.xhtml"),
        ),
        (
            "#frag".to_owned(),
            false,
            "",
            "text/#frag",
            "text/#frag",
            Some("text/c1.xhtml#frag"),
        ),
        (
            "foo/javascript:alert(1)".to_owned(),
            false,
            "",
            "text/foo/javascript:alert(1)",
            "text/foo/javascript:alert(1)",
            Some("text/foo/javascript:alert(1)"),
        ),
        (
            "....\\".to_owned(),
            false,
            "",
            "text/....",
            "text/....",
            Some("text/...."),
        ),
        (".".to_owned(), false, ".", "text", "text", Some("text")),
        (
            String::new(),
            false,
            "",
            "text/c1.xhtml",
            "text/c1.xhtml",
            None,
        ),
    ]);
    for (href, unsafe_, normalized, resolved, key, target) in &rows {
        assert_eq!(convert::is_unsafe(href), *unsafe_, "is_unsafe {href:?}");
        assert_eq!(convert::normalize(href), *normalized, "normalize {href:?}");
        assert_eq!(
            convert::resolve_doc_path(base, href),
            *resolved,
            "resolve {href:?}"
        );
        assert_eq!(convert::key_doc(base, href), *key, "key_doc {href:?}");
        assert_eq!(
            convert::resolve_link_target(href, base).as_deref(),
            *target,
            "link {href:?}"
        );
    }
    // Past 16 layers decoding stops short, so kfxgen lets it through: as one name in `text/`,
    // still encoded once decoded for the key. The plan's `%252e%252e/` encoded 16 times more
    // is such a name.
    let mut beyond = "%252e%252e/etc/passwd".to_owned();
    for _ in 0..16 {
        beyond = beyond.replace('%', "%25").replace('/', "%2F");
    }
    for href in [layers("../etc/passwd", 17), beyond] {
        let resolved = format!("text/{href}");
        let key = convert::unquote(&resolved);
        assert!(key.contains("%25"), "{key}");
        assert!(!convert::is_unsafe(&href));
        assert_eq!(convert::normalize(&href), href);
        assert_eq!(convert::resolve_doc_path(base, &href), resolved);
        assert_eq!(convert::key_doc(base, &href), key);
        assert_eq!(convert::resolve_link_target(&href, base), Some(key));
    }
    // No result leaves the book, and a long or deep input costs time in proportion.
    let long = "../".repeat(200_000) + &"%25".repeat(200_000);
    let start = std::time::Instant::now();
    assert_eq!(convert::key_doc(base, &long), "");
    assert_eq!(convert::resolve_link_target(&long, base), None);
    assert!(start.elapsed() < crate::support::COST_LIMIT);
}
