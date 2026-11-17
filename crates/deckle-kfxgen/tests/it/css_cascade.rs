// SPDX-License-Identifier: GPL-3.0-only
//! The first cascade (phase 2 plan Task 8): each spine file's rules sorted and applied as
//! Calibre 9.18's first-pass `Stylizer` applies them, the chapter marks `DetectStructure` puts
//! in, and the contents it makes for a book with none, against the dumps under
//! `fixtures/kfxgen/cascade/` ([`check_tree_and_rules`], run on `read_book` by `book.rs` and
//! `corpus_cascade.rs`) and against `data/css_cascade_probe.json`, read from Calibre with
//! `calibre-debug` (its `how` says how).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use deckle_kfxgen::book;
use deckle_kfxgen::css::{
    Cascaded, DEFAULT_PROFILE, Decls, Error, HTML_CSS, Origin, Rule, Sheet, cascade, parse_sheet,
};
use deckle_kfxgen::dom::{self, Dom, NodeId, generated_toc, mark_chapters};
use deckle_kfxgen::epub::TocNode;
use deckle_limits::roxmltree::{Node, ParsingOptions};
use deckle_limits::{MAX_CASCADE_DECLS, MAX_STYLE_RULES};
use serde_json::Value;

use crate::support::{Book, Dump, Tally, path_of, read_json};

/// The style of the `<div>` Calibre puts before a chapter (`structure.py` `detect_chapters`).
const MARK: &str = "display: block; page-break-after: always";

fn probe() -> Value {
    read_json(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/it/data/css_cascade_probe.json"),
    )
}

/// Runs `f`, failing past `COST_LIMIT`.
fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    eprintln!("{what}: {took:?}");
    assert!(took < crate::support::COST_LIMIT, "{what} took {took:?}");
    out
}

fn page(body: &str) -> Dom {
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{body}</body></html>"
    );
    dom::parse(src.as_bytes()).unwrap()
}

fn pairs(decls: &Decls) -> Vec<(String, String)> {
    decls
        .iter()
        .map(|(k, d)| (k.clone(), d.value.clone()))
        .collect()
}

fn important(decls: &Decls) -> Vec<String> {
    let mut names: Vec<String> = decls
        .iter()
        .filter(|(_, d)| d.important)
        .map(|(k, _)| k.clone())
        .collect();
    names.sort();
    names
}

fn strings(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p[0].as_str().unwrap().to_owned(),
                p[1].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn find(dom: &Dom, path: &[u32]) -> NodeId {
    crate::support::find(dom, path).unwrap()
}

fn body(dom: &Dom) -> NodeId {
    dom.children(dom.root())
        .iter()
        .copied()
        .find(|&c| dom.tag(c) == "body")
        .unwrap()
}

/// One spine file of a cascade fixture book, with its chapter marks, cascaded. `prepare`
/// changes the tree first, as Calibre's steps between the marks and the Stylizer do.
fn cascaded(book: &mut Book, href: &str, prepare: impl FnOnce(&mut Dom)) -> (Dom, Cascaded) {
    let mut dom = dom::parse(&book.read(href)).unwrap();
    mark_chapters(&mut dom);
    prepare(&mut dom);
    let sheets = book.sheets(href, &dom);
    let c = cascade(&mut dom, href, sheets, &DEFAULT_PROFILE).unwrap();
    (dom, c)
}

/// The probe's first pass of `css_quirks`'s `cascade.xhtml`, and the port's.
fn cascade_xhtml() -> (Dom, Cascaded, Value) {
    let probe = probe();
    let want = probe["cascade"].clone();
    let mut book = Book::open("css_quirks");
    let (dom, c) = cascaded(&mut book, "OEBPS/cascade.xhtml", |dom| {
        // `flatcss.py` `CSSFlattener.stylize_spine` sets <body>'s style before it builds the
        // Stylizer (Task 9).
        let b = body(dom);
        dom.set_attr(b, "style", want["body_style"].as_str());
    });
    (dom, c, want)
}

#[test]
fn important_and_order_cases() {
    let (dom, c, want) = cascade_xhtml();
    let at = |path: &[u32]| c.specified(find(&dom, path));
    let value = |path: &[u32], name: &str| {
        at(path)
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, d)| (d.value.clone(), d.important))
    };
    // `p.i1 { color: red !important }` then `p.i1 { color: blue }`: the important one stays.
    assert_eq!(value(&[1, 2], "color"), Some(("red".into(), true)));
    // `p.i2 { color: green !important }` and `style="color: purple"`.
    assert_eq!(value(&[1, 3], "color"), Some(("green".into(), true)));
    // Two rules of equal specificity: the later; then the style attribute over both.
    assert_eq!(value(&[1, 4], "color"), Some(("navy".into(), false)));
    assert_eq!(value(&[1, 5], "color"), Some(("purple".into(), false)));
    // An id over a class, whatever the order.
    assert_eq!(value(&[1, 6], "color"), Some(("teal".into(), false)));
    // A link, a <style>, a second link: the last.
    assert_eq!(value(&[1, 7], "color"), Some(("blue".into(), false)));
    // html.css gives <hr> a height.
    assert_eq!(value(&[1, 8], "height"), Some(("2px".into(), false)));
    // html.css's `float: none !important` loses to the book's `float: right !important`.
    assert_eq!(value(&[1, 22], "float"), Some(("right".into(), true)));

    // And every element of the file as the probe read Calibre's first pass.
    let elements = want["elements"].as_array().unwrap();
    assert_eq!(elements.len(), dom.descendants().count());
    let mut failed = Vec::new();
    for e in elements {
        let path = path_of(&e[0]);
        let n = find(&dom, &path);
        let got = (pairs(c.specified(n)), important(c.specified(n)));
        let mut want_imp: Vec<String> = e[2]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect();
        want_imp.sort();
        let want = (strings(&e[1]), want_imp);
        if got != want {
            failed.push(format!("{path:?}: got {got:?}\n  want {want:?}"));
        }
        let pseudo: Vec<(String, Vec<(String, String)>)> = c
            .pseudo(n)
            .iter()
            .map(|(k, d)| ((*k).to_owned(), pairs(d)))
            .collect();
        let want_pseudo: Vec<(String, Vec<(String, String)>)> = e[3]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().to_owned(), strings(&p[1])))
            .collect();
        if pseudo != want_pseudo {
            failed.push(format!(
                "{path:?} pseudo: got {pseudo:?}, want {want_pseudo:?}"
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn img_size_attributes_fold_into_css() {
    let (dom, c, _) = cascade_xhtml();
    let img = |i: u32| find(&dom, &[1, i]);
    let size = |n: NodeId| (dom.attr(n, "width"), dom.attr(n, "height"));
    let decls = |n: NodeId| {
        pairs(c.specified(n))
            .into_iter()
            .filter(|(k, _)| k == "width" || k == "height")
            .collect::<Vec<_>>()
    };
    let wh = |w: &str, h: &str| vec![("width".into(), w.into()), ("height".into(), h.into())];
    // width="30" height="40": both become CSS in px, and leave the tree.
    assert_eq!(size(img(10)), (None, None));
    assert_eq!(decls(img(10)), wh("30px", "40px"));
    // width="50%": kept as written; html.css's `height: auto` stays.
    assert_eq!(size(img(11)), (None, None));
    assert_eq!(decls(img(11)), wh("50%", "auto"));
    // CSS sets a width: the attributes stay, and CSS wins.
    assert_eq!(size(img(12)), (Some("30"), Some("40")));
    assert_eq!(decls(img(12)), wh("50%", "auto"));
    // A style attribute sets a height: the same.
    assert_eq!(size(img(13)), (Some("30"), Some("40")));
    assert_eq!(decls(img(13)), wh("auto", "2em"));
}

/// An element as the tree comparison sees it: (local name, id, width, height, kfxgen's marks).
/// The size attributes are those `img` keeps (Task 8 folds the others into CSS); the marks are
/// its `data-kfxgen-*` attributes, sorted (phase 3 Task 5).
type Shape = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Vec<(String, String)>,
);

/// The `data-kfxgen-*` attributes of `attrs`, sorted.
fn kfxgen_marks<'a>(attrs: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = attrs
        .filter(|(k, _)| k.starts_with("data-kfxgen-"))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    out.sort();
    out
}

/// Path → [`Shape`] of every element of a `dom/` dump.
fn dump_tree(xhtml: &str) -> BTreeMap<Vec<u32>, Shape> {
    let doc = deckle_limits::parse_xml(xhtml, ParsingOptions::default()).unwrap();
    let mut out = BTreeMap::new();
    let mut stack = vec![(doc.root_element(), Vec::new())];
    while let Some((n, path)) = stack.pop() {
        let attr = |a: &str| n.attribute(a).map(str::to_owned);
        let img = n.tag_name().name() == "img";
        out.insert(
            path.clone(),
            (
                n.tag_name().name().to_owned(),
                attr("id"),
                attr("width").filter(|_| img),
                attr("height").filter(|_| img),
                kfxgen_marks(n.attributes().map(|a| (a.name(), a.value()))),
            ),
        );
        for (i, c) in n.children().filter(Node::is_element).enumerate() {
            let mut p = path.clone();
            p.push(i as u32);
            stack.push((c, p));
        }
    }
    out
}

/// Path → [`Shape`] of every element of the port's tree.
fn tree(dom: &Dom) -> BTreeMap<Vec<u32>, Shape> {
    dom.descendants()
        .map(|n| {
            let attr = |a: &str| dom.attr(n, a).map(str::to_owned);
            let img = dom.tag(n) == "img";
            let tag = dom.tag(n);
            (
                dom.path(n),
                (
                    tag.rsplit(':').next().unwrap_or(tag).to_owned(),
                    attr("id"),
                    attr("width").filter(|_| img),
                    attr("height").filter(|_| img),
                    kfxgen_marks(dom.attrs(n).map(|(k, _, v)| (k, v))),
                ),
            )
        })
        .collect()
}

/// The paths of the dump's chapter marks: elements whose style is exactly the mark's.
fn dump_marks(styles: &Value) -> BTreeSet<Vec<u32>> {
    let table = styles["table"].as_array().unwrap();
    let mark = serde_json::json!({"display": "block", "page-break-after": "always"});
    styles["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| table[e[1].as_u64().unwrap() as usize]["specified"] == mark)
        .map(|e| path_of(&e[0]))
        .collect()
}

/// The marks the port put in: the `<div>`s styled as a mark.
fn marks(dom: &Dom) -> BTreeSet<Vec<u32>> {
    dom.descendants()
        .filter(|&n| dom.tag(n) == "div" && dom.attr(n, "style") == Some(MARK))
        .map(|n| dom.path(n))
        .collect()
}

/// A rule as compared: (selector, declarations, sorted `!important` names), in Calibre's
/// cascade order.
type RuleShape = (String, Vec<(String, String)>, Vec<String>);

fn dumped_rules(rules: &Value) -> Vec<RuleShape> {
    rules
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let decls = r[2]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                .collect();
            let imp = r[3]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect();
            (r[1].as_str().unwrap().to_owned(), decls, imp)
        })
        .collect()
}

/// serde_json's maps sort their keys: declarations are compared as sorted pairs.
fn ported_rules(rules: &[Rule], origin: Origin) -> Vec<RuleShape> {
    rules
        .iter()
        .filter(|r| r.origin == origin)
        .map(|r| {
            let mut decls = pairs(&r.decls);
            decls.sort();
            (r.selector.clone(), decls, important(&r.decls))
        })
        .collect()
}

fn sorted_pairs(decls: &Decls) -> Vec<(String, String)> {
    let mut p = pairs(decls);
    p.sort();
    p
}

/// `read_book`'s trees and first-pass rules against the dump (Task 8): every element of
/// `dom/NNN.xhtml` at the same path with the same name, id and picture size (the chapter marks
/// and generated-contents ids among them), the rules of the first cascade against
/// `rules/NNN.json` and the user-agent rules, and the book's `@font-face` rules against those
/// kfxgen's `Stylizer` reports (`styles/NNN.json`) and the first pass's (`rules/NNN.json`), in
/// spine order.
pub(crate) fn check_tree_and_rules(got: &book::Book, dump: &Dump, tally: &mut Tally) {
    let name = &dump.name;
    let ua_want = dumped_rules(&read_json(&dump.ua_rules)["rules"]);
    // The `@font-face` rules kfxgen's `Stylizer` reports, and the first pass's.
    let mut faces_want: Vec<Vec<(String, String)>> = Vec::new();
    let mut faces_first: Vec<Vec<(String, String)>> = Vec::new();
    let faces_of = |v: &Value, out: &mut Vec<Vec<(String, String)>>| {
        for face in v["font_face"].as_array().unwrap() {
            let mut p: Vec<(String, String)> = face
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                .collect();
            p.sort();
            out.push(p);
        }
    };
    for f in &got.files {
        let href = &f.href;
        let styles = dump.json("styles", href);
        tally.marks += dump_marks(&styles).len();
        faces_of(&styles, &mut faces_want);

        // The tree.
        let want = dump_tree(&dump.xhtml(href));
        let ours = tree(&f.dom);
        for (path, w) in &want {
            match ours.get(path) {
                Some(g) if g == w => {}
                Some(g) => tally
                    .failures
                    .push(format!("{name} {href} {path:?}: got {g:?}, want {w:?}")),
                // Calibre's flattener links the book's two new stylesheets at the end of
                // <head> (`flatten_head`); the port's tree has no such links.
                None if path.len() == 2 && path[0] == 0 && w.0 == "link" => {}
                None => tally
                    .failures
                    .push(format!("{name} {href} {path:?}: {w:?} is missing")),
            }
        }
        for (path, g) in &ours {
            if !want.contains_key(path) {
                tally
                    .failures
                    .push(format!("{name} {href} {path:?}: {g:?} is not in the dump"));
            }
        }

        // The first cascade's rules.
        let rules = dump.json("rules", href);
        faces_of(&rules, &mut faces_first);
        if ported_rules(&f.first_pass_rules, Origin::UserAgent) != ua_want {
            tally
                .failures
                .push(format!("{name} {href}: html.css rules"));
        }
        let want = dumped_rules(&rules["rules"]);
        let ours = ported_rules(&f.first_pass_rules, Origin::Book);
        if ours != want {
            let i = ours.iter().zip(&want).take_while(|(g, w)| g == w).count();
            tally.failures.push(format!(
                "{name} {href}: rule {i}: got {:?}, want {:?}",
                ours.get(i),
                want.get(i)
            ));
        }
        tally.files += 1;
        tally.rules += ours.len();
    }
    let faces: Vec<Vec<(String, String)>> = got.font_face.iter().map(sorted_pairs).collect();
    for (whose, want) in [
        ("kfxgen's", &faces_want),
        ("the first pass's", &faces_first),
    ] {
        if &faces != want {
            tally.failures.push(format!(
                "{name}: @font-face rules against {whose}: got {faces:?}, want {want:?}"
            ));
        }
    }
    tally.font_faces += faces.len();
}

/// The trees of a book's spine, in `oeb.json`'s spine order, as `generated_toc` takes them.
fn spine_order<'a>(
    oeb: &Value,
    doms: &'a mut [(String, String, Dom)],
) -> Vec<(&'a str, bool, &'a mut Dom)> {
    let order: Vec<(String, bool)> = oeb["spine"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["href"].as_str().unwrap().to_owned(),
                s["linear"].as_bool().unwrap(),
            )
        })
        .collect();
    let mut by_href: BTreeMap<&str, (&'a str, &'a mut Dom)> = doms
        .iter_mut()
        .map(|(h, _, d)| (h.as_str(), (h.as_str(), d)))
        .collect();
    order
        .iter()
        .map(|(h, linear)| {
            let (href, dom) = by_href.remove(h.as_str()).unwrap();
            (href, *linear, dom)
        })
        .collect()
}

fn toc_pairs(nodes: &[TocNode]) -> Vec<(String, String)> {
    nodes
        .iter()
        .map(|n| {
            assert!(n.children.is_empty());
            (n.title.clone(), n.href.clone())
        })
        .collect()
}

#[test]
fn generated_contents_match_calibre() {
    let mut book = Book::open("no_toc");
    let oeb = read_json(&book.dir.join("oeb.json"));
    let mut doms: Vec<(String, String, Dom)> = Vec::new();
    for (href, n) in book.files.clone() {
        let mut dom = dom::parse(&book.read(&href)).unwrap();
        mark_chapters(&mut dom);
        doms.push((href, n, dom));
    }
    let mut files = spine_order(&oeb, &mut doms);
    let toc = generated_toc(&mut files);
    let want: Vec<(String, String)> = oeb["toc"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["title"].as_str().unwrap().to_owned(),
                t["href"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        want,
        [
            (
                "Chapter One".to_owned(),
                "OEBPS/one.xhtml#calibre_toc_2".to_owned()
            ),
            (
                "Part Two".to_owned(),
                "OEBPS/two.xhtml#calibre_toc_3".to_owned()
            ),
        ]
    );
    assert_eq!(toc_pairs(&toc), want);
    let (_, _, one) = &doms[0];
    assert_eq!(one.attr(find(one, &[1, 0]), "id"), Some("calibre_toc_2"));
}

/// Books made for the probe: chapter expressions, marks near the start, contents from
/// chapters, from links, and from the spine.
#[test]
fn chapter_marks_and_contents_match_probes() {
    let probe = probe();
    for case in probe["structure"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let pages: Vec<(String, String)> = strings(&case["pages"]);
        let mut doms: Vec<(String, Dom)> = pages
            .iter()
            .map(|(href, text)| {
                let mut dom = dom::parse(text.as_bytes()).unwrap();
                mark_chapters(&mut dom);
                (href.clone(), dom)
            })
            .collect();
        let linear: BTreeMap<String, bool> = case["spine"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s[0].as_str().unwrap().to_owned(), s[1].as_bool().unwrap()))
            .collect();
        let mut files: Vec<(&str, bool, &mut Dom)> = doms
            .iter_mut()
            .map(|(h, d)| (h.as_str(), linear[h.as_str()], d))
            .collect();
        let toc = generated_toc(&mut files);
        assert_eq!(toc_pairs(&toc), strings(&case["toc"]), "{name}: contents");
        for (href, dom) in &doms {
            let want: Vec<(Vec<u32>, String, Option<String>, bool)> = case["elements"]
                [href.as_str()]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    path_of(&e[0]),
                    e[1].as_str().unwrap().to_owned(),
                    e[2].as_str().map(str::to_owned),
                    e[3].as_bool().unwrap(),
                )
            })
            .collect();
            let got: Vec<_> = dom
                .descendants()
                .filter(|&n| dom.path(n).first() == Some(&1))
                .map(|n| {
                    (
                        dom.path(n),
                        dom.tag(n).to_owned(),
                        dom.attr(n, "id").map(str::to_owned),
                        dom.attr(n, "style") == Some(MARK),
                    )
                })
                .collect();
            assert_eq!(got, want, "{name} {href}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Hostile input
// ---------------------------------------------------------------------------------------

/// `n` paragraphs, `p<i % classes>` classed.
fn paragraphs(n: usize, classes: usize) -> Dom {
    let mut body = String::with_capacity(n * 24);
    for i in 0..n {
        body.push_str(&format!("<p class=\"c{}\">x</p>", i % classes));
    }
    page(&body)
}

fn book_sheet(css: &str) -> Sheet {
    parse_sheet(css, Origin::Book, "OEBPS/a.xhtml", 0, &mut |_| None).unwrap()
}

#[test]
fn many_rules_on_many_elements_use_the_index() {
    // 10,000 class rules over 20,000 paragraphs of 1,000 classes: each rule looks only at
    // its class.
    let mut dom = paragraphs(20_000, 1_000);
    let css: String = (0..10_000).map(|i| format!(".c{i}{{color:red}}")).collect();
    let c = timed("10,000 rules, 20,000 elements", || {
        cascade(
            &mut dom,
            "OEBPS/a.xhtml",
            vec![book_sheet(&css)],
            &DEFAULT_PROFILE,
        )
        .unwrap()
    });
    let p = dom.children(body(&dom))[0];
    assert_eq!(
        pairs(c.specified(p)).last().unwrap(),
        &("color".into(), "red".into())
    );
}

/// Release build: `cargo test --release -p deckle-kfxgen --test it -- --ignored css_cascade`.
#[test]
#[ignore = "about a second in a release build, much longer in a debug one"]
fn fifty_thousand_rules_on_a_hundred_thousand_elements() {
    let mut dom = paragraphs(100_000, 1_000);
    let css: String = (0..50_000).map(|i| format!(".c{i}{{color:red}}")).collect();
    let start = Instant::now();
    let got = cascade(
        &mut dom,
        "OEBPS/a.xhtml",
        vec![book_sheet(&css)],
        &DEFAULT_PROFILE,
    );
    eprintln!("50,000 rules, 100,000 elements: {:?}", start.elapsed());
    assert!(got.is_ok());
    assert!(start.elapsed() < crate::support::COST_LIMIT);
}

#[test]
fn elements_matched_alike_share_their_declarations() {
    // 300 rules matching every one of 10,000 paragraphs: one set of declarations for all.
    let mut dom = paragraphs(10_000, 1);
    let css: String = (0..300).map(|i| format!("p{{x{i}:y}}")).collect();
    let c = timed("300 rules matching all", || {
        cascade(
            &mut dom,
            "OEBPS/a.xhtml",
            vec![book_sheet(&css)],
            &DEFAULT_PROFILE,
        )
        .unwrap()
    });
    let ps = dom.children(body(&dom)).to_vec();
    let a = c.specified_arc(ps[0]);
    assert!(ps.iter().all(|&p| Arc::ptr_eq(c.specified_arc(p), a)));
    assert!(a.len() > 300, "{}", a.len());
}

/// 50,000 copies of `rule` over 100,000 paragraphs of class `c`, all of which it matches: the
/// selector work runs out, in under 5 seconds.
fn matching_everything_stops(rule: &str) {
    let mut dom = paragraphs(100_000, 1);
    let css = rule.repeat(50_000);
    let start = Instant::now();
    let got = cascade(
        &mut dom,
        "OEBPS/a.xhtml",
        vec![book_sheet(&css)],
        &DEFAULT_PROFILE,
    );
    eprintln!(
        "50,000 {rule} rules matching 100,000 elements: {:?}",
        start.elapsed()
    );
    assert!(
        matches!(got, Err(Error::Selectors(_))),
        "{:?}",
        got.map(|_| ())
    );
    assert!(start.elapsed() < crate::support::COST_LIMIT);
}

/// Release build: `cargo test --release -p deckle-kfxgen --test it -- --ignored css_cascade`.
#[test]
#[ignore = "100 million selector steps: about a second in a release build"]
fn a_rule_matching_everything_many_times_stops() {
    matching_everything_stops("p{color:red}");
}

/// Release build: `cargo test --release -p deckle-kfxgen --test it -- --ignored css_cascade`.
#[test]
#[ignore = "100 million selector steps: about 2 seconds in a release build"]
fn a_class_rule_matching_everything_many_times_stops() {
    // The Task 8 review's case: a class step is among the slowest.
    matching_everything_stops(".c0{color:red}");
}

#[test]
fn elements_each_styled_their_own_way_stop() {
    // 40,000 sets of 106 declarations: the declarations stored are bounded.
    let (mut dom, css) = styled_their_own_way(100);
    let got = timed("40,000 distinct elements, 100 rules over all", || {
        cascade(
            &mut dom,
            "OEBPS/a.xhtml",
            vec![book_sheet(&css)],
            &DEFAULT_PROFILE,
        )
    });
    assert!(
        matches!(got, Err(Error::Cascade(MAX_CASCADE_DECLS))),
        "{:?}",
        got.map(|_| ())
    );
}

/// 40,000 paragraphs of their own id, each given a value of its own by a rule, and `shared`
/// rules over all of them: 40,000 sets of `shared + 1` declarations.
fn styled_their_own_way(shared: usize) -> (Dom, String) {
    let mut body = String::new();
    for i in 0..40_000 {
        body.push_str(&format!("<p id=\"p{i}\">x</p>"));
    }
    let mut css: String = (0..40_000).map(|i| format!("#p{i}{{a{i}:b}}")).collect();
    for i in 0..shared {
        css.push_str(&format!("p{{z{i}:y}}"));
    }
    (page(&body), css)
}

#[test]
fn styles_just_under_the_limit_are_kept() {
    // 40,000 sets of 11 declarations (5 from html.css's `p`, 5 shared, one of its own) and
    // a unit for each set: about 480,000 units. One more shared rule: about 520,000.
    for (shared, kept) in [(5, true), (6, false)] {
        let (mut dom, css) = styled_their_own_way(shared);
        let got = timed(
            &format!("40,000 distinct elements, {shared} rules over all"),
            || {
                cascade(
                    &mut dom,
                    "OEBPS/a.xhtml",
                    vec![book_sheet(&css)],
                    &DEFAULT_PROFILE,
                )
            },
        );
        assert_eq!(got.is_ok(), kept, "{shared}");
    }
}

/// The memory the styles take just under [`MAX_CASCADE_DECLS`], for its doc comment: run
/// alone in a release build under `/usr/bin/time -l` (macOS) and compare the peak with
/// `DECKLE_CASCADE_BASELINE=1`, which builds the same page and CSS without cascading them.
/// `cargo test --release -p deckle-kfxgen --test it -- --ignored --exact
/// css_cascade::styles_just_under_the_limit_peak_memory`.
#[test]
#[ignore = "a measurement, for a release build"]
fn styles_just_under_the_limit_peak_memory() {
    let (mut dom, css) = styled_their_own_way(5);
    let sheet = book_sheet(&css);
    if std::env::var_os("DECKLE_CASCADE_BASELINE").is_some() {
        std::hint::black_box((&dom, &sheet));
        return;
    }
    let c = cascade(&mut dom, "OEBPS/a.xhtml", vec![sheet], &DEFAULT_PROFILE).unwrap();
    std::hint::black_box(&c);
}

#[test]
fn many_pseudo_class_rules_are_charged_for_what_they_add() {
    // 1,000 `*::before` and 1,000 `p:hover` rules on 100 paragraphs: 100,000 declarations
    // under each name, each charged once.
    let mut css = String::new();
    for i in 0..1_000 {
        css.push_str(&format!("*::before{{x{i}:y}}p:hover{{x{i}:y}}"));
    }
    let mut dom = paragraphs(100, 1);
    let c = timed("2,000 pseudo-class rules on 100 paragraphs", || {
        cascade(
            &mut dom,
            "OEBPS/a.xhtml",
            vec![book_sheet(&css)],
            &DEFAULT_PROFILE,
        )
        .unwrap()
    });
    let p = dom.children(body(&dom))[99];
    let ps = c.pseudo(p);
    assert_eq!(
        ps.len(),
        2,
        "{:?}",
        ps.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );
    assert!(ps.iter().all(|(_, d)| d.len() == 1_000));
}

#[test]
fn large_style_attributes_finish_or_stop() {
    // 5,000 paragraphs, each a 1 KB style attribute of its own of 50 declarations: about
    // 70 units each.
    let style: String = (0..50)
        .map(|i| format!("x{i}: {};", "v".repeat(14)))
        .collect();
    let mut src = String::new();
    for i in 0..5_000 {
        src.push_str(&format!("<p style=\"{style}y: {i}\">x</p>"));
    }
    let mut dom = page(&src);
    let c = timed("5,000 1 KB style attributes", || {
        cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap()
    });
    let p = dom.children(body(&dom))[4_999];
    assert_eq!(
        pairs(c.specified(p)).last().unwrap(),
        &("y".into(), "4999".into())
    );
    // The same attribute on every paragraph is read once.
    let mut dom = page(&format!("<p style=\"{style}\">x</p>").repeat(20_000));
    let c = timed("20,000 1 KB style attributes alike", || {
        cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap()
    });
    let ps = dom.children(body(&dom)).to_vec();
    assert!(
        ps.iter()
            .all(|&p| Arc::ptr_eq(c.specified_arc(p), c.specified_arc(ps[0])))
    );
}

/// Release build: `cargo test --release -p deckle-kfxgen --test it -- --ignored css_cascade`.
#[test]
#[ignore = "a 100 MB file: seconds to parse in a debug build"]
fn a_hundred_thousand_large_style_attributes_stop() {
    let style: String = (0..50)
        .map(|i| format!("x{i}: {};", "v".repeat(14)))
        .collect();
    let mut src = String::new();
    for i in 0..100_000 {
        src.push_str(&format!("<p style=\"{style}y: {i}\">x</p>"));
    }
    let mut dom = page(&src);
    let start = Instant::now();
    let got = cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE);
    eprintln!("100,000 1 KB style attributes: {:?}", start.elapsed());
    assert!(
        matches!(got, Err(Error::Cascade(MAX_CASCADE_DECLS))),
        "{:?}",
        got.map(|_| ())
    );
    assert!(start.elapsed() < crate::support::COST_LIMIT);
}

#[test]
fn many_style_elements_are_too_many_rules() {
    let styles = "<style>p{color:red}</style>".repeat(100_000);
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head>{styles}</head><body><p>x</p></body></html>"
    );
    let mut dom = dom::parse(src.as_bytes()).unwrap();
    let sheets: Vec<Sheet> = timed("100,000 <style> elements read", || {
        dom.descendants()
            .filter(|&n| dom.tag(n) == "style")
            .map(|n| book_sheet(dom.text(n)))
            .collect()
    });
    let got = timed("100,000 <style> elements cascaded", || {
        cascade(&mut dom, "OEBPS/a.xhtml", sheets, &DEFAULT_PROFILE)
    });
    assert!(
        matches!(got, Err(Error::TooLarge(MAX_STYLE_RULES))),
        "{:?}",
        got.map(|_| ())
    );
}

#[test]
fn a_hundred_thousand_headings_are_marked_and_listed() {
    let body: String = (0..100_000)
        .map(|i| format!("<h1>Chapter {i}</h1><p>x</p>"))
        .collect();
    let mut dom = page(&body);
    timed("100,000 chapters marked", || mark_chapters(&mut dom));
    // All but the first, which is at the start of the file.
    assert_eq!(marks(&dom).len(), 99_999);
    let toc = timed("100,000 chapters listed", || {
        generated_toc(&mut [("OEBPS/a.xhtml", true, &mut dom)])
    });
    assert_eq!(toc.len(), 100_000);
    assert_eq!(toc[99_999].href, "OEBPS/a.xhtml#calibre_toc_100001");
}

#[test]
fn nested_headings_are_tested_once_each() {
    // 250 nested headings around 4 MB of text: each heading's text is the whole 4 MB.
    let text = "word ".repeat(800_000);
    let body = format!("{}{text}Chapter{}", "<h1>".repeat(250), "</h1>".repeat(250));
    let mut dom = std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || page(&body))
        .unwrap()
        .join()
        .unwrap();
    timed("250 nested headings of 4 MB", || mark_chapters(&mut dom));
    // "Chapter" ends each heading's text, with no space after it: none matches.
    assert!(marks(&dom).is_empty());
    let toc = timed("250 nested headings listed", || {
        generated_toc(&mut [("OEBPS/a.xhtml", true, &mut dom)])
    });
    assert!(toc.is_empty());
}

#[test]
fn a_ten_megabyte_heading_is_tested() {
    let text = "x".repeat(10 << 20);
    let mut dom = page(&format!("<p>a</p><h2>{text} prologue</h2><h2>{text}</h2>"));
    timed("10 MB heading", || mark_chapters(&mut dom));
    assert_eq!(marks(&dom).len(), 1);
    let toc = timed("10 MB heading listed", || {
        generated_toc(&mut [("OEBPS/a.xhtml", true, &mut dom)])
    });
    assert_eq!(toc.len(), 1);
    // The title keeps 1,000 characters.
    assert_eq!(toc[0].title.chars().count(), 1_000);
}

#[test]
fn html_css_is_the_first_sheet() {
    // The port parses html.css itself; a caller passes only the book's sheets.
    let mut dom = page("<p>x</p>");
    let c = cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
    let ua = parse_sheet(HTML_CSS, Origin::UserAgent, "", 0, &mut |_| None).unwrap();
    assert_eq!(c.rules.len(), ua.rules.len());
    assert!(c.rules.iter().all(|r| r.origin == Origin::UserAgent));
}
