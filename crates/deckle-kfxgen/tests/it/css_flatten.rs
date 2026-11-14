// SPDX-License-Identifier: GPL-3.0-only
//! The flattener (phase 2 plan Task 9): each spine file's first-pass styles turned into classes
//! as Calibre 9.18's `CSSFlattener` turns them, `RemoveFakeMargins` over the book, and the
//! styles kfxgen's own `Stylizer` then reads, against the dumps under
//! `fixtures/kfxgen/cascade/` (`styles/NNN.json` for the styles, `dom/NNN.xhtml` for the
//! classes; [`check_specified`], run on `read_book` by `book.rs` and `corpus_cascade.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use deckle_kfxgen::book;
use deckle_kfxgen::css::{
    ClassNamer, DEFAULT_PROFILE, Decls, Error, Flat, Origin, cascade, flatten, parse_sheet,
    prepare_body, remove_fake_margins,
};
use deckle_kfxgen::dom::{self, Dom, NodeId};
use deckle_limits::roxmltree::{Node, ParsingOptions};
use deckle_limits::{MAX_BOOK_CASCADE_DECLS, MAX_CASCADE_DECLS, MAX_STYLE_RULES};
use serde_json::Value;

use crate::support::{Book, Dump, Tally, find, path_of, read_json};

/// Fixture nodes whose styles or class the port gives differently from kfxgen's dump, each
/// with its `DIVERGENCES.md` section: (book, file, path, section).
const ALLOWED: &[(&str, &str, &[u32], &str)] = &[];

/// Runs `f`, failing past `COST_LIMIT`.
fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    eprintln!("{what}: {took:?}");
    assert!(took < crate::support::COST_LIMIT, "{what} took {took:?}");
    out
}

fn sorted(decls: &Decls) -> BTreeMap<String, String> {
    decls
        .iter()
        .map(|(k, d)| (k.clone(), d.value.clone()))
        .collect()
}

fn json_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
        .collect()
}

fn pseudo_map(list: &[(&'static str, Decls)]) -> BTreeMap<String, BTreeMap<String, String>> {
    list.iter()
        .map(|(k, d)| ((*k).to_owned(), sorted(d)))
        .collect()
}

fn class_set(class: Option<&str>) -> Option<BTreeSet<String>> {
    class.map(|c| c.split_ascii_whitespace().map(str::to_owned).collect())
}

/// Path → (local name, class attribute) of every element of a `dom/` dump.
fn dump_elements(xhtml: &str) -> BTreeMap<Vec<u32>, (String, Option<String>)> {
    let doc = deckle_limits::parse_xml(xhtml, ParsingOptions::default()).unwrap();
    let mut out = BTreeMap::new();
    let mut stack = vec![(doc.root_element(), Vec::new())];
    while let Some((n, path)) = stack.pop() {
        out.insert(
            path.clone(),
            (
                n.tag_name().name().to_owned(),
                n.attribute("class").map(str::to_owned),
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

/// [`ALLOWED`] as (book, the key [`check_specified`] records when it uses an entry).
pub(crate) fn allowed() -> Vec<(String, String)> {
    ALLOWED
        .iter()
        .map(|(b, file, p, section)| {
            (
                (*b).to_owned(),
                format!("specified {b} {file} {p:?} ({section})"),
            )
        })
        .collect()
}

/// `read_book`'s declared styles against the dump (Task 9): each element's declarations,
/// pseudo-class declarations and classes against kfxgen's `Stylizer` (`styles/NNN.json`) and
/// tree (`dom/NNN.xhtml`), every element of the port's tree in the dump.
pub(crate) fn check_specified(got: &book::Book, dump: &Dump, tally: &mut Tally) {
    let name = dump.name.as_str();
    for f in &got.files {
        let nnn = dump.nnn(&f.href);
        let styles = dump.json("styles", &f.href);
        let elements = dump_elements(&dump.xhtml(&f.href));
        let table = styles["table"].as_array().unwrap();
        let mut seen = BTreeSet::new();
        for node in styles["nodes"].as_array().unwrap() {
            let path = path_of(&node[0]);
            let want = &table[node[1].as_u64().unwrap() as usize];
            let (tag, want_class) = &elements[&path];
            let Some(n) = find(&f.dom, &path) else {
                // Calibre's flattener links the book's two new stylesheets at the end of
                // <head> (`flatten_head`); the port's tree has no such links.
                if !(path.len() == 2 && path[0] == 0 && tag == "link") {
                    tally
                        .failures
                        .push(format!("{name} {}: {path:?} ({tag}) is missing", f.href));
                }
                continue;
            };
            seen.insert(n);
            tally.elements += 1;
            let got = (
                f.dom.tag(n).rsplit(':').next().unwrap().to_owned(),
                sorted(f.flat.specified(n)),
                pseudo_map(f.flat.pseudo(n)),
                class_set(f.flat.class(n)),
                class_set(f.dom.attr(n, "class")),
            );
            let want_pseudo: BTreeMap<String, BTreeMap<String, String>> = want["pseudo"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), json_map(v)))
                .collect();
            let want = (
                tag.clone(),
                json_map(&want["specified"]),
                want_pseudo,
                class_set(want_class.as_deref()),
                class_set(want_class.as_deref()),
            );
            if got != want {
                match ALLOWED
                    .iter()
                    .find(|(b, file, p, _)| *b == name && *file == nnn && *p == path)
                {
                    Some((b, file, p, section)) => {
                        tally
                            .allowed_used
                            .insert(format!("specified {b} {file} {p:?} ({section})"));
                    }
                    None => tally.failures.push(format!(
                        "{name} {} {path:?}:\n  got  {got:?}\n  want {want:?}",
                        f.href
                    )),
                }
            }
        }
        // Every element of the port's tree is in the dump.
        let ours = f.dom.descendants().count();
        if seen.len() != ours {
            tally.failures.push(format!(
                "{name} {}: {ours} elements, {} of them in the dump",
                f.href,
                seen.len()
            ));
        }
    }
}

/// One page, its `<head>` holding `css`, flattened alone.
pub(crate) fn flat_page(html_attrs: &str, body: &str, css: &str) -> (Dom, Flat) {
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"{html_attrs}><head><title>T</title></head><body>{body}</body></html>"
    );
    let mut dom = dom::parse(src.as_bytes()).unwrap();
    prepare_body(&mut dom);
    let sheet = parse_sheet(css, Origin::Book, "OEBPS/a.xhtml", 0, &mut |_| None).unwrap();
    let first = cascade(&mut dom, "OEBPS/a.xhtml", vec![sheet], &DEFAULT_PROFILE).unwrap();
    let mut namer = ClassNamer::default();
    let flat = flatten(
        &mut dom,
        "OEBPS/a.xhtml",
        first,
        &mut namer,
        &DEFAULT_PROFILE,
    )
    .unwrap();
    (dom, flat)
}

pub(crate) fn body(dom: &Dom) -> NodeId {
    dom.children(dom.root())
        .iter()
        .copied()
        .find(|&c| dom.tag(c) == "body")
        .unwrap()
}

fn value<'a>(flat: &'a Flat, n: NodeId, name: &str) -> Option<&'a str> {
    flat.specified(n)
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, d)| d.value.as_str())
}

#[test]
fn invalid_values_are_kept() {
    // Review Focus 5: css_parser keeps what it can read whatever the property, and so does
    // the class it goes to.
    let (dom, flat) = flat_page(
        "",
        "<p class=\"bad\">x</p>",
        "p.bad { margin-left: foo; height: 10 px }",
    );
    let p = dom.children(body(&dom))[0];
    assert_eq!(value(&flat, p, "margin-left"), Some("foo"));
    assert_eq!(value(&flat, p, "height"), Some("10 px"));
    assert_eq!(dom.attr(p, "class"), Some("bad"));
}

#[test]
fn body_style_is_the_one_calibre_writes() {
    // `data/css_cascade_probe.json` holds the style attribute `stylize_spine` gave <body> in
    // css_quirks's cascade.xhtml, read from Calibre.
    let probe = read_json(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/it/data/css_cascade_probe.json"),
    );
    let mut book = Book::open("css_quirks");
    let mut dom = dom::parse(&book.read("OEBPS/cascade.xhtml")).unwrap();
    prepare_body(&mut dom);
    assert_eq!(
        dom.attr(body(&dom), "style"),
        probe["cascade"]["body_style"].as_str()
    );
    // <html>'s style goes first, joined with `;`.
    let mut dom = dom::parse(
        b"<html xmlns=\"http://www.w3.org/1999/xhtml\" style=\"color: red\"><head><title>T</title></head><body style=\"margin: 1em\"><p>x</p></body></html>",
    )
    .unwrap();
    prepare_body(&mut dom);
    assert_eq!(dom.attr(dom.root(), "style"), None);
    assert_eq!(
        dom.attr(body(&dom), "style"),
        Some(
            "color: red; margin: 1em; margin-top: 0pt; margin-bottom: 0pt; margin-left : 5pt; \
             margin-right : 5pt; padding-left: 0pt; padding-right: 0pt"
        )
    );
}

#[test]
fn body_margins_are_added() {
    let (dom, flat) = flat_page(" style=\"color: #333\"", "<p>x</p>", "");
    let b = body(&dom);
    let want = [
        ("color", "#333"),
        ("margin-top", "0"),
        ("margin-bottom", "0"),
        ("margin-left", "5pt"),
        ("margin-right", "5pt"),
        ("padding-left", "0"),
        ("padding-right", "0"),
    ];
    for (k, v) in want {
        assert_eq!(value(&flat, b, k), Some(v), "{k}");
    }
    // The style moved from <html> to <body>, and the flattener took both away.
    assert_eq!(dom.attr(dom.root(), "style"), None);
    assert_eq!(dom.attr(b, "style"), None);
    assert_eq!(dom.attr(b, "class"), Some("calibre"));
}

#[test]
fn font_tag_size_plus_one_is_13_5pt() {
    let (dom, flat) = flat_page(
        "",
        "<p><font size=\"+1\">a</font><font size=\" 7 \">b</font><font size=\"9\">c</font>\
         <font size=\"-9\">d</font><font size=\"x\">e</font></p><font size=\"5\"><p>f</p></font>",
        "",
    );
    let b = body(&dom);
    let p = dom.children(b)[0];
    let sizes: Vec<(&str, Option<&str>)> = dom
        .children(p)
        .iter()
        .map(|&f| (dom.tag(f), value(&flat, f, "font-size")))
        .collect();
    assert_eq!(
        sizes,
        [
            ("span", Some("13.5pt")),
            // Written `22.0pt`, read back by css_parser as `22pt`.
            ("span", Some("22pt")),
            // Not a size Calibre has: size 3's.
            ("span", Some("12pt")),
            // 3 - 9, raised to 1.
            ("span", Some("5pt")),
            ("span", Some("12pt")),
        ]
    );
    // A <font> around a block becomes a <div>.
    let div = dom.children(b)[1];
    assert_eq!(dom.tag(div), "div");
    assert_eq!(value(&flat, div, "font-size"), Some("17pt"));
    assert_eq!(value(&flat, div, "display"), Some("block"));
    assert!(
        dom.children(p)
            .iter()
            .all(|&f| dom.attr(f, "size").is_none())
    );
}

#[test]
fn attributes_become_css() {
    let (dom, flat) = flat_page(
        "",
        "<p align=\"center\" color=\"blue\">a</p>\
         <div align=\"center\"><table><tr><td valign=\"top\" bgcolor=\"#eeeeee\">b</td></tr></table></div>\
         <p><img src=\"a.png\" align=\"left\"/><img src=\"a.png\" align=\"middle\"/></p>\
         <ol type=\"a\"><li>c</li></ol>\
         <p class=\"fw\">d</p><p><span class=\"il\">e</span><span class=\"sup\">f</span></p>",
        "p.fw { font-weight: medium } span.il { display: in-line } span.sup { vertical-align: sup }",
    );
    let kids = dom.children(body(&dom)).to_vec();
    assert_eq!(value(&flat, kids[0], "text-align"), Some("center"));
    assert_eq!(value(&flat, kids[0], "color"), Some("blue"));
    assert_eq!(dom.attr(kids[0], "align"), None);
    assert_eq!(dom.attr(kids[0], "color"), None);
    // align=center on a block centres the tables directly in it.
    let table = dom.children(kids[1])[0];
    assert_eq!(value(&flat, table, "margin-left"), Some("auto"));
    assert_eq!(value(&flat, table, "margin-right"), Some("auto"));
    let td = dom.children(dom.children(table)[0])[0];
    assert_eq!(value(&flat, td, "vertical-align"), Some("top"));
    assert_eq!(value(&flat, td, "background-color"), Some("#eee"));
    assert_eq!(dom.attr(td, "valign"), None);
    let imgs = dom.children(kids[2]).to_vec();
    assert_eq!(value(&flat, imgs[0], "float"), Some("left"));
    assert_eq!(value(&flat, imgs[1], "vertical-align"), Some("middle"));
    assert_eq!(dom.attr(kids[3], "type"), None);
    assert_eq!(
        value(&flat, kids[3], "list-style-type"),
        Some("lower-alpha")
    );
    assert_eq!(value(&flat, kids[4], "font-weight"), Some("normal"));
    let spans = dom.children(kids[5]).to_vec();
    assert_eq!(value(&flat, spans[0], "display"), Some("inline"));
    assert_eq!(value(&flat, spans[1], "vertical-align"), Some("super"));
}

#[test]
fn classes_are_named_as_calibre_names_them() {
    // css_quirks's flatten.xhtml: `Note2 big` → note, `note3` → note1, `NOTE` with a style
    // attribute → note2; the same styles share a class, wherever they are.
    let (dom, _) = flat_page(
        "",
        "<p class=\"Note2 big\">a</p><p class=\"note3\">b</p><p class=\"NOTE\" style=\"text-indent: 5em\">c</p>\
         <p class=\"note3\">d</p><p>e</p><p class=\"x-1\" style=\"color: red\">f</p><p class=\"ps\">g</p>\
         <p class=\"Note2\">h</p>",
        "p.big { text-indent: 3em } p.note3 { text-indent: 4em } p.ps:hover { color: red }",
    );
    let classes: Vec<Option<&str>> = dom
        .children(body(&dom))
        .iter()
        .map(|&p| dom.attr(p, "class"))
        .collect();
    assert_eq!(
        classes,
        [
            Some("note"),
            Some("note1"),
            Some("note2"),
            Some("note1"),
            Some("calibre1"),
            Some("x"),
            Some("calibre1 pcalibre"),
            // A <p> styled as plain ones are, under the name of its own first class.
            Some("calibre1"),
        ]
    );
    assert_eq!(dom.attr(body(&dom), "class"), Some("calibre"));
}

#[test]
fn elements_outside_xhtml_keep_their_attributes() {
    let (dom, flat) = flat_page(
        "",
        "<x:thing xmlns:x=\"urn:example:other\" class=\"kept\" style=\"color: red\"><p class=\"in\">a</p></x:thing>\
         <svg xmlns=\"http://www.w3.org/2000/svg\" style=\"color: blue\"><text>b</text></svg>",
        "",
    );
    let thing = dom.children(body(&dom))[0];
    assert_eq!(dom.attr(thing, "class"), Some("kept"));
    assert_eq!(dom.attr(thing, "style"), Some("color: red"));
    assert!(flat.specified(thing).is_empty());
    // Calibre does not walk into them: the <p> inside keeps its class.
    let p = dom.children(thing)[0];
    assert_eq!(dom.attr(p, "class"), Some("in"));
    // SVG is flattened, but the first cascade reads XHTML elements' `style` attributes only
    // (`//h:*[@style]`): an SVG element's is lost.
    let svg = dom.children(body(&dom))[1];
    assert_eq!(dom.attr(svg, "style"), None);
    assert_eq!(value(&flat, svg, "color"), None);
}

#[test]
fn style_and_link_elements_go() {
    let (dom, _) = flat_page(
        "",
        "<p>a</p><style>p { color: red }</style><link rel=\"stylesheet\" href=\"a.css\"/><p>b</p>",
        "",
    );
    let tags: Vec<&str> = dom
        .children(body(&dom))
        .iter()
        .map(|&c| dom.tag(c))
        .collect();
    assert_eq!(tags, ["p", "p"]);
}

/// A book of one file per entry: `count` paragraphs of class `class`, and 30 plain ones.
fn margins_book(files: &[(&str, usize)], css: &str) -> (Vec<Dom>, Vec<Flat>) {
    let mut namer = ClassNamer::default();
    let mut doms = Vec::new();
    let mut flats = Vec::new();
    for (i, (class, count)) in files.iter().enumerate() {
        let href = format!("OEBPS/{i}.xhtml");
        let body = format!("<p class=\"{class}\">x</p>").repeat(*count);
        let src = format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{body}</body></html>"
        );
        let mut dom = dom::parse(src.as_bytes()).unwrap();
        prepare_body(&mut dom);
        let sheet = parse_sheet(css, Origin::Book, &href, 0, &mut |_| None).unwrap();
        let first = cascade(&mut dom, &href, vec![sheet], &DEFAULT_PROFILE).unwrap();
        flats.push(flatten(&mut dom, &href, first, &mut namer, &DEFAULT_PROFILE).unwrap());
        doms.push(dom);
    }
    let mut docs: Vec<(&Dom, &mut Flat)> = doms.iter().zip(flats.iter_mut()).collect();
    remove_fake_margins(&mut namer, &mut docs);
    (doms, flats)
}

#[test]
fn fake_margin_shared_by_95_percent_is_removed() {
    let css = "p.m { margin-left: 3em; margin-right: 2em } p.o { margin-left: 1em }";
    // 96 of 100 paragraphs at depth 1 across two files: removed, from both files.
    let (doms, flats) = margins_book(&[("m", 50), ("m", 46), ("o", 4)], css);
    for (dom, flat) in doms.iter().zip(&flats).take(2) {
        let p = dom.children(body(dom))[0];
        // html.css's <p> margin shows through.
        assert_eq!(value(flat, p, "margin-left"), Some("0"));
        assert_eq!(value(flat, p, "margin-right"), Some("0"));
    }
    let p = doms[2].children(body(&doms[2]))[0];
    assert_eq!(value(&flats[2], p, "margin-left"), Some("1em"));
    // 94 of 100: kept.
    let (doms, flats) = margins_book(&[("m", 94), ("o", 6)], css);
    let p = doms[0].children(body(&doms[0]))[0];
    assert_eq!(value(&flats[0], p, "margin-left"), Some("3em"));
    // Fewer than 25 paragraphs at a level: not counted.
    let (doms, flats) = margins_book(&[("m", 24)], css);
    let p = doms[0].children(body(&doms[0]))[0];
    assert_eq!(value(&flats[0], p, "margin-left"), Some("3em"));
    // A negative text-indent at the level: the level is left alone.
    let css = "p.m { margin-left: 3em } p.n { margin-left: 3em; text-indent: -1em }";
    let (doms, flats) = margins_book(&[("m", 40), ("n", 1)], css);
    let p = doms[0].children(body(&doms[0]))[0];
    assert_eq!(value(&flats[0], p, "margin-left"), Some("3em"));
}

#[test]
fn fake_margins_keep_every_class_of_the_same_name() {
    // `a¹` is numbered from `a1` (`ascii_text`), and the second class numbered from `a` is
    // `a1` too: Calibre writes two `.a1` rules, and every `class="a1"` element gets both.
    let css = "div.a { color: blue } p.a { margin-left: 3em; text-indent: 1em }";
    let page = format!(
        "<div class=\"a\">x</div>{}<p class=\"a¹\" style=\"margin-left: 3em; color: red\">y</p>",
        "<p class=\"a\">x</p>".repeat(30)
    );
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{page}</body></html>"
    );
    let mut dom = dom::parse(src.as_bytes()).unwrap();
    prepare_body(&mut dom);
    let sheet = parse_sheet(css, Origin::Book, "OEBPS/a.xhtml", 0, &mut |_| None).unwrap();
    let first = cascade(&mut dom, "OEBPS/a.xhtml", vec![sheet], &DEFAULT_PROFILE).unwrap();
    let mut namer = ClassNamer::default();
    let mut flat = flatten(
        &mut dom,
        "OEBPS/a.xhtml",
        first,
        &mut namer,
        &DEFAULT_PROFILE,
    )
    .unwrap();
    let kids = dom.children(body(&dom)).to_vec();
    let last = *kids.last().unwrap();
    assert_eq!(dom.attr(kids[1], "class"), Some("a1"));
    assert_eq!(dom.attr(last, "class"), Some("a1"));
    remove_fake_margins(&mut namer, &mut [(&dom, &mut flat)]);
    // All 31 paragraphs are counted by the last `.a1` (`selector_map`), whose 3em goes; the
    // first `.a1` keeps its own.
    for p in [kids[1], last] {
        assert_eq!(value(&flat, p, "margin-left"), Some("3em"));
        assert_eq!(value(&flat, p, "text-indent"), Some("1em"));
        assert_eq!(value(&flat, p, "color"), Some("red"));
    }
}

#[test]
fn important_in_an_attribute_value_is_read_as_important() {
    // Calibre writes `text-align: center !important` into stylesheet.css, and css_parser reads
    // it back as an important `center`.
    let (dom, flat) = flat_page(
        "",
        "<p align=\"center !important\">a</p><p><font face=\"A !important\">b</font></p>",
        "",
    );
    let kids = dom.children(body(&dom)).to_vec();
    let decl = |n: NodeId, name: &str| {
        flat.specified(n)
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, d)| (d.value.clone(), d.important))
    };
    assert_eq!(decl(kids[0], "text-align"), Some(("center".into(), true)));
    let font = dom.children(kids[1])[0];
    assert_eq!(decl(font, "font-family"), Some(("A".into(), true)));
}

#[test]
fn negative_line_heights_are_raised_to_zero() {
    let (dom, flat) = flat_page(
        "",
        "<p class=\"n\">a</p>\
         <p class=\"n\"><span>b</span></p>\
         <p class=\"pc\">c</p>\
         <p class=\"tiny\">d</p>\
         <p><span class=\"cap\">A</span>rest</p>\
         <div class=\"dc\"><p class=\"dcp\">B</p></div>",
        "p.n { line-height: -1px } p.pc { line-height: -5% } p.tiny { line-height: -1e-999 }\
         span.cap { float: left; font-size: 3em; line-height: -1px }\
         div.dc { float: left } p.dcp { font-size: 2em; line-height: -1px }",
    );
    let kids = dom.children(body(&dom)).to_vec();
    assert_eq!(value(&flat, kids[0], "line-height"), Some("0"));
    // Inherited: the <span> sets none, and gets 0 of its own.
    let span = dom.children(kids[1])[0];
    assert_eq!(value(&flat, span, "line-height"), Some("0"));
    assert_eq!(value(&flat, kids[2], "line-height"), Some("0"));
    // `float('-1e-999')` is -0.0, not below 0.
    assert_ne!(value(&flat, kids[3], "line-height"), Some("0"));
    // A drop cap, and the drop cap DOCX input makes, keep theirs.
    let cap = dom.children(kids[4])[0];
    assert_eq!(value(&flat, cap, "line-height"), Some("-1px"));
    let dcp = dom.children(kids[5])[0];
    assert_eq!(value(&flat, dcp, "line-height"), Some("-1px"));
}

/// `n` elements named `tag`, each with a class and a `style` attribute of its own: one
/// declaration, its value `pad` bytes longer than its number.
fn distinct_page(n: usize, tag: &str, pad: usize) -> Dom {
    let pad = "v".repeat(pad);
    let body: String = (0..n)
        .map(|i| format!("<{tag} class=\"c{i}\" style=\"z: {i}{pad}\">x</{tag}>"))
        .collect();
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{body}</body></html>"
    );
    dom::parse(src.as_bytes()).unwrap()
}

#[test]
fn distinct_styles_are_flattened_in_time() {
    let mut dom = distinct_page(20_000, "p", 0);
    prepare_body(&mut dom);
    let first = cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
    let mut namer = ClassNamer::default();
    let flat = timed("20,000 elements styled their own way", || {
        flatten(
            &mut dom,
            "OEBPS/a.xhtml",
            first,
            &mut namer,
            &DEFAULT_PROFILE,
        )
        .unwrap()
    });
    let last = *dom.children(body(&dom)).last().unwrap();
    assert_eq!(value(&flat, last, "z"), Some("19999"));
    // `c19999` with its digits stripped: the 20,000th `c`.
    assert_eq!(dom.attr(last, "class"), Some("c19999"));
}

#[test]
fn the_second_cascade_counts_against_the_files_limit() {
    // 9,000 paragraphs of a 1,200-byte declaration of their own: about 45 units each in the
    // first cascade (405,000), about 26 more each in the second.
    let mut dom = distinct_page(9_000, "p", 1_200);
    prepare_body(&mut dom);
    let first = cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
    let mut namer = ClassNamer::default();
    let got = timed("9,000 elements styled their own way, flattened", || {
        flatten(
            &mut dom,
            "OEBPS/a.xhtml",
            first,
            &mut namer,
            &DEFAULT_PROFILE,
        )
    });
    assert!(
        matches!(got, Err(Error::Cascade(MAX_CASCADE_DECLS))),
        "{:?}",
        got.map(|_| ())
    );
}

#[test]
fn more_classes_than_rules_allowed_are_refused() {
    // 50,000 spans styled their own way and <body>: one class more than a file may have.
    let mut dom = distinct_page(MAX_STYLE_RULES, "span", 0);
    prepare_body(&mut dom);
    let first = cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
    let mut namer = ClassNamer::default();
    let got = timed("50,001 classes", || {
        flatten(
            &mut dom,
            "OEBPS/a.xhtml",
            first,
            &mut namer,
            &DEFAULT_PROFILE,
        )
    });
    assert!(
        matches!(got, Err(Error::TooLarge(MAX_STYLE_RULES))),
        "{:?}",
        got.map(|_| ())
    );
}

/// The cost of the flattener on a hostile page, for the report: run alone in a release build
/// under `/usr/bin/time -l`. `cargo test --release -p deckle-kfxgen --test it -- --ignored
/// --exact css_flatten::fifty_thousand_distinct_elements_cost`.
#[test]
#[ignore = "a measurement, for a release build"]
fn fifty_thousand_distinct_elements_cost() {
    let n = std::env::var("DECKLE_FLATTEN_ELEMENTS").map_or(50_000, |v| v.parse().unwrap());
    let mut dom = distinct_page(n, "p", 0);
    // `DECKLE_FLATTEN_STOP=page` or `first`: the peak of what comes before the flattener.
    let stop = std::env::var("DECKLE_FLATTEN_STOP").unwrap_or_default();
    if stop == "page" {
        std::hint::black_box(&dom);
        return;
    }
    let start = Instant::now();
    prepare_body(&mut dom);
    let first = cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
    let cascaded = start.elapsed();
    if stop == "first" {
        std::hint::black_box(&first);
        return;
    }
    let mut namer = ClassNamer::default();
    let got = flatten(
        &mut dom,
        "OEBPS/a.xhtml",
        first,
        &mut namer,
        &DEFAULT_PROFILE,
    );
    eprintln!(
        "{n} distinct elements: first cascade {cascaded:?}, flattened {:?} ({})",
        start.elapsed() - cascaded,
        if got.is_ok() { "kept" } else { "refused" }
    );
    std::hint::black_box(&got);
}

/// A novel's stylesheet of the kind Gutenberg and Standard Ebooks books carry.
const NOVEL_CSS: &str = "
body { font-family: serif; margin: 0 5% }
h1, h2, h3 { text-align: center; font-weight: bold; margin: 2em 0 1em; page-break-before: always }
p { text-indent: 1.5em; margin: 0; line-height: 1.4 }
p.first, h2 + p { text-indent: 0 }
p.center { text-align: center; text-indent: 0 }
.smallcaps, .sc { font-variant: small-caps }
.dropcap:first-letter { float: left; font-size: 3em; line-height: 1 }
blockquote { margin: 1em 2em; font-size: 0.9em }
blockquote p { text-indent: 0 }
div.poem { margin: 1em 3em }
div.poem p.line { text-indent: -1em; margin-left: 1em }
div.poem p.indent2 { margin-left: 2em }
hr.tb { width: 30%; margin: 1em auto; border: none; border-top: 1px solid }
a.noteref { vertical-align: super; font-size: 0.75em; text-decoration: none }
div.footnote { font-size: 0.85em; margin-top: 2em; border-top: 1px solid #888 }
div.footnote p { text-indent: 0 }
span.lang { font-style: italic }
img { max-width: 100% }
div.figure { text-align: center; margin: 1em 0 }
p.caption { font-size: 0.9em; font-style: italic; text-indent: 0 }
p.letter { margin-left: 2em; margin-right: 2em }
p.signature { text-align: right; font-style: italic }
p.date { text-align: right }
a:link, a:visited { color: #036 }
";

/// Chapter `n` of a novel: a heading, about 120 paragraphs with italics, note references, a
/// quotation, a poem, a letter, a picture and footnotes.
fn novel_chapter(n: usize) -> String {
    let mut b = format!(
        "<h2>Chapter {n}</h2><p class=\"first\"><span class=\"dropcap\">I</span>t was <span class=\"sc\">late</span>.</p>"
    );
    for i in 0..100 {
        b.push_str("<p>Some text with <i>italics</i>, <em>stress</em> and <span class=\"lang\">aliena</span>");
        if i % 10 == 0 {
            b.push_str(&format!("<a class=\"noteref\" href=\"#n{i}\">{i}</a>"));
        }
        b.push_str(".</p>");
        if i == 30 {
            b.push_str("<blockquote><p>One.</p><p>Two.</p><p>Three.</p></blockquote>");
        }
        if i == 50 {
            b.push_str("<div class=\"poem\">");
            for l in 0..8 {
                let c = if l % 2 == 0 { "line" } else { "indent2" };
                b.push_str(&format!("<p class=\"{c}\">A line of verse</p>"));
            }
            b.push_str("</div><hr class=\"tb\"/>");
        }
        if i == 70 {
            b.push_str(
                "<p class=\"date\">May 1</p><p class=\"letter\">Dear sir,</p>\
                 <p class=\"signature\">Yours</p><p style=\"text-align: center\">* * *</p>\
                 <div class=\"figure\"><img src=\"a.png\" alt=\"\"/><p class=\"caption\">A picture</p></div>",
            );
        }
    }
    b.push_str(
        "<div class=\"footnote\"><p id=\"n0\">A note.</p><p>Another.</p><p>A third.</p></div>",
    );
    b
}

#[test]
fn an_ordinary_long_book_is_far_below_the_book_limit() {
    // 200 chapters of a novel: the book's units, ten times over, stay within the limit.
    let mut namer = ClassNamer::default();
    let mut flats = Vec::new();
    for n in 0..200 {
        let href = format!("OEBPS/c{n}.xhtml");
        let src = format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{}</body></html>",
            novel_chapter(n)
        );
        let mut dom = dom::parse(src.as_bytes()).unwrap();
        prepare_body(&mut dom);
        let sheet = parse_sheet(NOVEL_CSS, Origin::Book, &href, 0, &mut |_| None).unwrap();
        let first = cascade(&mut dom, &href, vec![sheet], &DEFAULT_PROFILE).unwrap();
        flats.push(flatten(&mut dom, &href, first, &mut namer, &DEFAULT_PROFILE).unwrap());
    }
    eprintln!("200 chapters of a novel: {} units", namer.units());
    assert!(
        namer.units() * 10 <= MAX_BOOK_CASCADE_DECLS,
        "{}",
        namer.units()
    );
}

/// One file of the book-wide limit's hostile book: 6,000 paragraphs, 2,000 `style` attributes
/// of their own each used three times, none used by another file.
fn styled_file(file: usize, elements: usize, distinct: usize) -> Dom {
    let body: String = (0..elements)
        .map(|i| format!("<p style=\"z: {}\">x</p>", file * distinct + i % distinct))
        .collect();
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{body}</body></html>"
    );
    dom::parse(src.as_bytes()).unwrap()
}

/// A book of `files` files of [`styled_file`], flattened in order and its fake margins
/// removed; `Err` where a file is refused, with how many were flattened.
fn styled_book(
    files: usize,
    elements: usize,
    distinct: usize,
) -> (Result<(), Error>, usize, usize) {
    let mut namer = ClassNamer::default();
    let mut doms = Vec::new();
    let mut flats = Vec::new();
    for f in 0..files {
        let mut dom = styled_file(f, elements, distinct);
        prepare_body(&mut dom);
        let first = cascade(&mut dom, "OEBPS/a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
        match flatten(
            &mut dom,
            "OEBPS/a.xhtml",
            first,
            &mut namer,
            &DEFAULT_PROFILE,
        ) {
            Ok(flat) => flats.push(flat),
            Err(e) => return (Err(e), f, namer.units()),
        }
        doms.push(dom);
    }
    let mut docs: Vec<(&Dom, &mut Flat)> = doms.iter().zip(flats.iter_mut()).collect();
    remove_fake_margins(&mut namer, &mut docs);
    std::hint::black_box(&docs);
    (Ok(()), files, namer.units())
}

/// The book-wide limit, for its doc comment: run alone in a release build under
/// `/usr/bin/time -l`, with `DECKLE_BOOK_FILES` files (400 by default) and
/// `DECKLE_BOOK_STOP=doms` for the peak of the trees alone.
/// `cargo test --release -p deckle-kfxgen --test it -- --ignored --exact
/// css_flatten::a_book_of_many_styled_files_cost`.
#[test]
#[ignore = "a measurement, for a release build"]
fn a_book_of_many_styled_files_cost() {
    let var =
        |name: &str, default: usize| std::env::var(name).map_or(default, |v| v.parse().unwrap());
    let files = var("DECKLE_BOOK_FILES", 400);
    let elements = var("DECKLE_BOOK_ELEMENTS", 6_000);
    let distinct = var("DECKLE_BOOK_DISTINCT", 2_000);
    if std::env::var("DECKLE_BOOK_STOP").is_ok_and(|v| v == "doms") {
        let doms: Vec<Dom> = (0..files)
            .map(|f| styled_file(f, elements, distinct))
            .collect();
        std::hint::black_box(&doms);
        return;
    }
    let start = Instant::now();
    let (got, done, units) = styled_book(files, elements, distinct);
    eprintln!(
        "{files} files: {done} flattened, {units} units, {:?}, {:?}",
        start.elapsed(),
        got
    );
}

/// Release build: `cargo test --release -p deckle-kfxgen --test it -- --ignored
/// css_flatten::a_book_of_many_styled_files_is_refused`.
#[test]
#[ignore = "200 files of 6,000 elements: seconds in a debug build"]
fn a_book_of_many_styled_files_is_refused() {
    // About 38,000 units a file: refused at the 79th of 200.
    let start = Instant::now();
    let (got, done, units) = styled_book(200, 6_000, 2_000);
    let took = start.elapsed();
    eprintln!("refused after {done} files, {units} units, {took:?}");
    assert!(
        matches!(got, Err(Error::BookCascade(MAX_BOOK_CASCADE_DECLS))),
        "{got:?}"
    );
    assert!(done < 200);
    assert!(took < crate::support::COST_LIMIT, "{took:?}");
}
