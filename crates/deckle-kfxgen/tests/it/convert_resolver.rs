// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's style resolver and `compute_block_style` (phase 3 plan, Task 8), against the
//! resolver dumps (`fixtures/kfxgen/convert/<name>/resolver/NNN.json.gz`), and the cost of
//! resolving every element of large pages.

use std::collections::BTreeMap;
use std::time::Instant;

use deckle_kfx::fixtures;
use deckle_kfxgen::book::{File, read_book};
use deckle_kfxgen::convert::{Resolver, StyleError, TextBudget, compute_block_style};
use deckle_kfxgen::css::{
    self, ClassNamer, DEFAULT_PROFILE, Origin, cascade, flatten, parse_sheet, prepare_body,
};
use deckle_kfxgen::dom::{self, NodeId};
use deckle_kfxgen::pyvalue::PyValue;
use deckle_limits::MAX_CASCADE_DECLS;

use crate::convert_support::{dump, model_diff};
use crate::support::{COST_LIMIT, find};

/// Files whose rows cannot match yet, with why: (source, NNN). Empty is the goal.
const SKIP: &[(&str, &str)] = &[];

/// Every source fixture with convert dumps.
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

#[test]
fn resolver_matches_the_dump_for_every_element() {
    let names = sources();
    assert_eq!(names.len(), 55, "the source fixtures");
    let mut failures = Vec::new();
    let (mut files_compared, mut elements, mut link_rows) = (0, 0, 0);
    // The most units any file's resolver charged, and where.
    let mut most = (0, String::new());
    for name in &names {
        let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let listed: BTreeMap<String, String> = serde_json::from_slice(
            &std::fs::read(fixtures::kfxgen(&format!("convert/{name}/files.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            listed.len(),
            book.files.len(),
            "{name}: files.json against read_book's spine"
        );
        for file in &book.files {
            let nnn = listed
                .get(&file.href)
                .unwrap_or_else(|| panic!("{name}: {} not in files.json", file.href));
            if SKIP.contains(&(name.as_str(), nnn.as_str())) {
                continue;
            }
            let PyValue::List(rows) = dump(name, &format!("resolver/{nnn}.json.gz")) else {
                panic!("{name} {nnn}: no list");
            };
            let budget = TextBudget::new();
            let resolver = Resolver::new(file, &DEFAULT_PROFILE, &budget);
            let head = find(&file.dom, &[0]).unwrap();
            let head_kids = file
                .dom
                .children(head)
                .iter()
                .filter(|&&c| !file.dom.is_comment(c))
                .count();
            let (mut seen, mut links) = (0, 0);
            for row in &rows {
                let [path, css, block] = row.as_list().unwrap() else {
                    panic!("{name} {nnn}: a row of other than three items");
                };
                let path: Vec<u32> = path
                    .as_list()
                    .unwrap()
                    .iter()
                    .map(|i| i.as_int().unwrap() as u32)
                    .collect();
                let n = match find(&file.dom, &path) {
                    Some(n) => {
                        seen += 1;
                        n
                    }
                    // The `<link>`s to stylesheet.css and page_styles.css that Calibre's
                    // flattener writes at the end of `<head>`: `read_book` leaves stylesheet
                    // links out of `File::dom`. They have no style of their own, so each
                    // resolves as `<head>` does.
                    None if path.len() == 2 && path[0] == 0 && path[1] as usize >= head_kids => {
                        links += 1;
                        head
                    }
                    None => {
                        failures.push(format!("{name} {nnn} {path:?}: no such element"));
                        continue;
                    }
                };
                let got = resolver.resolve(n).unwrap();
                let ours = got.as_ref().map_or(PyValue::None, |r| r.to_py().unwrap());
                for line in model_diff(&ours, css) {
                    failures.push(format!("{name} {nnn} {path:?} resolve{line}"));
                }
                let ours = match &got {
                    None => Ok(PyValue::None),
                    Some(r) => compute_block_style(r),
                };
                match ours {
                    Ok(ours) => {
                        for line in model_diff(&ours, block) {
                            failures.push(format!("{name} {nnn} {path:?} block style{line}"));
                        }
                    }
                    Err(e) => failures.push(format!("{name} {nnn} {path:?}: raised {e:?}")),
                }
            }
            assert_eq!(
                seen,
                file.dom.descendants().count(),
                "{name} {nnn}: a row per element"
            );
            assert_eq!(links, 2, "{name} {nnn}: the flattener's two links");
            if budget.charged() > most.0 {
                most = (budget.charged(), format!("{name} {nnn}"));
            }
            files_compared += 1;
            elements += seen;
            link_rows += links;
        }
    }
    assert!(
        failures.is_empty(),
        "{}\n{} differences",
        failures.join("\n"),
        failures.len()
    );
    eprintln!(
        "{files_compared} files, {elements} elements and {link_rows} stylesheet links compared; \
         most units charged: {} ({})",
        most.0, most.1
    );
    // Far below the budget: an ordinary book reads no value of TEXT_UNIT bytes many times.
    assert!(most.0 < MAX_CASCADE_DECLS / 1000, "{most:?}");
    assert_eq!(files_compared, 184 - SKIP.len());
    assert_eq!(elements + link_rows, 3941);
}

/// A spine file of `dom` styled with `css`, flattened as `read_book` would.
fn file_of(mut dom: dom::Dom, css: &str) -> File {
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
    File {
        href: "OEBPS/a.xhtml".into(),
        dom,
        flat,
        first_pass_rules: Vec::new(),
    }
}

fn empty_page() -> (dom::Dom, NodeId) {
    let dom = dom::parse(
        b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body/></html>",
    )
    .unwrap();
    let body = dom.descendants().find(|&n| dom.tag(n) == "body").unwrap();
    (dom, body)
}

/// Resolves every element, deepest first (so no memo is warm), within `COST_LIMIT`.
fn resolve_all<'a>(what: &str, file: &'a File, budget: &'a TextBudget) -> Resolver<'a> {
    let mut all: Vec<NodeId> = file.dom.descendants().collect();
    all.reverse();
    let start = Instant::now();
    let resolver = Resolver::new(file, &DEFAULT_PROFILE, budget);
    for &n in &all {
        let r = resolver.resolve(n).unwrap().unwrap();
        assert!(compute_block_style(&r).is_ok());
    }
    let took = start.elapsed();
    eprintln!("{what}: {} elements, {took:?}", all.len());
    assert!(took < COST_LIMIT, "{what} took {took:?}");
    resolver
}

fn get(r: &Resolver, n: NodeId, key: &str) -> PyValue {
    r.resolve(n)
        .unwrap()
        .unwrap()
        .get(key)
        .unwrap()
        .unwrap()
        .clone()
}

#[test]
fn a_deep_tree_resolves_in_one_pass() {
    const DEEP: usize = 20_000;
    let (mut dom, body) = empty_page();
    let mut n = body;
    for i in 0..DEEP {
        n = dom.append_child(n, "span");
        dom.set_text(n, "x");
        if i % 1000 == 0 {
            dom.set_attr(n, "class", Some("big"));
        }
    }
    let deepest = n;
    let file = file_of(
        dom,
        "body { text-decoration: underline; color: #777; letter-spacing: 0.1em; \
         line-height: 1.5 } \
         .big { font-size: 101% } \
         span { text-decoration: line-through }",
    );
    let budget = TextBudget::new();
    let resolver = resolve_all("20,000 nested spans", &file, &budget);
    // Twenty sizes of 101% of the one above, from 16px.
    let mut want = 16.0_f64;
    for _ in 0..DEEP / 1000 {
        want = 101.0 * want / 100.0;
    }
    let want = format!("{want:.3}").parse::<f64>().unwrap();
    assert_eq!(get(&resolver, deepest, "font-size"), PyValue::Float(want));
    assert_eq!(
        get(&resolver, deepest, "text-decoration"),
        PyValue::Str("line-through underline".into())
    );
    assert_eq!(
        get(&resolver, deepest, "color"),
        PyValue::Str("#777".into())
    );
    assert_eq!(
        get(&resolver, deepest, "letter-spacing"),
        PyValue::Str("0.1em".into())
    );
    assert_eq!(get(&resolver, deepest, "line-height"), PyValue::Float(1.5));
}

/// 50,000 siblings under 2,000 nested `<div>`s. At 200 deep, as planned, walking every
/// sibling's ancestors again for each inherited value took 6.7 s in a debug build, inside
/// `COST_LIMIT`; at 2,000 the walks are ten times as many.
#[test]
fn siblings_do_not_rewalk_their_ancestors() {
    const DEEP: usize = 2_000;
    let (mut dom, body) = empty_page();
    let mut n = body;
    for _ in 0..DEEP {
        n = dom.append_child(n, "div");
    }
    let mut last = n;
    for _ in 0..50_000 {
        last = dom.append_child(n, "span");
        dom.set_text(last, "x");
    }
    let file = file_of(
        dom,
        "body { font-weight: bold; font-variant: small-caps; text-transform: uppercase; \
         text-orientation: upright; text-emphasis-style: filled dot; color: red; \
         text-decoration: underline; background-color: #eee; line-height: 2 } \
         div { font-size: 1.01em; background-color: #ddd }",
    );
    let budget = TextBudget::new();
    let resolver = resolve_all("50,000 siblings 2,000 deep", &file, &budget);
    assert_eq!(
        get(&resolver, last, "font-weight"),
        PyValue::Str("bold".into())
    );
    assert_eq!(
        get(&resolver, last, "background-behind"),
        PyValue::Int(0xdddddd)
    );
    assert_eq!(
        get(&resolver, last, "text-decoration"),
        PyValue::Str("underline".into())
    );
}

/// A page of `body` styled with `css`.
fn page(body: &str, css: &str) -> File {
    let html = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head>\
         <body>{body}</body></html>"
    );
    file_of(dom::parse(html.as_bytes()).unwrap(), css)
}

/// The `i`th element named `tag`, in document order.
fn nth(file: &File, tag: &str, i: usize) -> NodeId {
    file.dom
        .descendants()
        .filter(|&n| file.dom.tag(n) == tag)
        .nth(i)
        .unwrap()
}

fn s(v: &str) -> PyValue {
    PyValue::Str(v.to_owned())
}

fn dict(pairs: &[(&str, PyValue)]) -> PyValue {
    PyValue::Dict(pairs.iter().map(|(k, v)| (s(k), v.clone())).collect())
}

/// What no dump row reaches: expected values from the pinned `converter.py`'s resolver, its
/// functions run in plain Python over these pages' declared styles (a stand-in for Calibre's
/// `Stylizer` giving each element's declarations as `read_book` has them).
#[test]
fn values_no_dump_row_reaches_are_kfxgens() {
    // A fill Python's float() turns into inf, then int(nan): `resolve` gives None for the box
    // and what lies on it, but not for a box inside with its own fill.
    let f = page(
        "<div class=\"a\"><span>a</span><p class=\"fill\">b<span>c</span></p></div>\
         <div class=\"pct\"><p>p</p></div>",
        &format!(
            ".a {{ background-color: rgb({}%, 0, 0) }} .fill {{ background-color: #333 }} \
             .pct {{ background: url(a.png) rgb(1,2,3) }}",
            "9".repeat(400)
        ),
    );
    let b = TextBudget::new();
    let r = Resolver::new(&f, &DEFAULT_PROFILE, &b);
    assert!(r.resolve(nth(&f, "div", 0)).unwrap().is_none());
    assert!(r.resolve(nth(&f, "span", 0)).unwrap().is_none());
    assert_eq!(
        get(&r, nth(&f, "span", 1), "background-behind"),
        PyValue::Int(0x333333)
    );
    assert_eq!(
        get(&r, nth(&f, "p", 1), "background-behind"),
        PyValue::Int(0x010203)
    );

    // A <q>'s marks: `quotes` up to a change of language, and its ::before and ::after.
    let f = page(
        "<div lang=\"fr\" class=\"quotes\"><p><q>x</q></p><p lang=\"de\"><q>y</q></p>\
         <p lang=\"fr\"><q>w</q></p></div><q class=\"marks\">z</q>\
         <hr/><hr class=\"tall\"/><hr class=\"thin\"/>",
        ".quotes { quotes: \"\u{ab}\" \"\u{bb}\" } .marks::before { content: \"[\" } \
         .marks::after { content: \"]\" } .tall { height: 2px } .thin { height: 1px }",
    );
    let b = TextBudget::new();
    let r = Resolver::new(&f, &DEFAULT_PROFILE, &b);
    let q = |quotes: PyValue, before: PyValue, after: PyValue| {
        dict(&[("quotes", quotes), ("before", before), ("after", after)])
    };
    let marks = s("\"\u{ab}\" \"\u{bb}\"");
    let none = || PyValue::None;
    assert_eq!(
        get(&r, nth(&f, "q", 0), "q-css"),
        q(marks.clone(), none(), none())
    );
    assert_eq!(get(&r, nth(&f, "q", 1), "q-css"), q(none(), none(), none()));
    assert_eq!(get(&r, nth(&f, "q", 2), "q-css"), q(marks, none(), none()));
    assert_eq!(
        get(&r, nth(&f, "q", 3), "q-css"),
        q(none(), s("\"[\""), s("\"]\""))
    );
    // calibre's own `height: 2px` on every <hr> is not the book's; 1px is.
    assert_eq!(get(&r, nth(&f, "hr", 1), "height"), PyValue::None);
    assert_eq!(get(&r, nth(&f, "hr", 2), "height"), s("1px"));

    // rem under a root of 10px, and calibre's rem for `small`, which is not read as the
    // keyword there.
    let f = page(
        "<p>a<span>b</span><span class=\"k\">c</span></p><p class=\"lh\">d<span class=\"big\">e</span></p>",
        "html { font-size: 62.5% } p { font-size: 1.1rem } .k { font-size: small } \
         .lh { line-height: 2em } .big { font-size: 2em }",
    );
    let b = TextBudget::new();
    let r = Resolver::new(&f, &DEFAULT_PROFILE, &b);
    let size = |n| get(&r, n, "font-size");
    assert_eq!(size(f.dom.root()), PyValue::Float(10.0));
    assert_eq!(size(nth(&f, "p", 0)), PyValue::Float(11.0));
    assert_eq!(size(nth(&f, "span", 1)), PyValue::Float(8.0));
    assert_eq!(size(nth(&f, "span", 2)), PyValue::Float(22.0));
    assert_eq!(get(&r, nth(&f, "p", 1), "line-height"), PyValue::Float(2.0));
    assert_eq!(
        get(&r, nth(&f, "span", 2), "line-height"),
        PyValue::Float(1.0)
    );

    // Under 16px: the keyword's Previewer size; a drop cap's size; decoration lines a `none`
    // can't take away; a % line height and `larger`.
    let f = page(
        "<p class=\"u\">a<span class=\"k\">b</span><span class=\"n\">c</span></p>\
         <p class=\"lh\">d<span class=\"big\">e</span></p>",
        ".k { font-size: small } .lh { line-height: 150% } .big { font-size: larger } \
         p::first-letter { font-size: x-large; float: left } .u { text-decoration: underline } \
         .n { text-decoration: none line-through }",
    );
    let b = TextBudget::new();
    let r = Resolver::new(&f, &DEFAULT_PROFILE, &b);
    assert_eq!(
        get(&r, nth(&f, "span", 0), "font-size"),
        PyValue::Float(13.0)
    );
    assert_eq!(
        get(&r, nth(&f, "span", 1), "text-decoration"),
        s("line-through underline")
    );
    assert_eq!(
        get(&r, nth(&f, "p", 0), "first-letter"),
        dict(&[("float", s("left")), ("font-size", PyValue::Float(24.0))])
    );
    assert_eq!(get(&r, nth(&f, "p", 1), "line-height"), PyValue::Float(1.5));
    assert_eq!(
        get(&r, nth(&f, "span", 2), "font-size"),
        PyValue::Float(19.2)
    );
    assert_eq!(
        get(&r, nth(&f, "span", 2), "line-height"),
        PyValue::Float(1.25)
    );

    // Link colour marks, and margins and paddings in kfxgen's slots in a vertical-rl file.
    let f = page(
        "<p class=\"red\"><a data-kfxgen-link-colour=\"currentColor\">a</a>\
         <a data-kfxgen-link-colour=\" #ABC \">b</a><a data-kfxgen-link-colour=\"\">c</a></p>\
         <div class=\"box\" data-kfxgen-v-author=\"margin-right padding-top\" \
         data-kfxgen-v-ua=\"margin-top:1em;margin-left:2em;padding-right:3px;margin-top:4em\">v</div>\
         <div class=\"box\" data-kfxgen-vertical=\"\" data-kfxgen-v-author=\"margin-right\">h</div>",
        ".red { color: red } .box { margin-right: 5px; padding-top: 6px; margin-top: 7px }",
    );
    let b = TextBudget::new();
    let r = Resolver::new(&f, &DEFAULT_PROFILE, &b);
    assert_eq!(get(&r, nth(&f, "a", 0), "color"), s("red"));
    assert_eq!(get(&r, nth(&f, "a", 1), "color"), s(" #ABC "));
    assert_eq!(get(&r, nth(&f, "a", 2), "color"), s(""));
    let v = nth(&f, "div", 0);
    assert_eq!(get(&r, v, "margin-top"), s("5px"));
    assert_eq!(get(&r, v, "margin-right"), PyValue::None);
    assert_eq!(get(&r, v, "margin-bottom"), PyValue::None);
    assert_eq!(get(&r, v, "margin-left"), s("2em"));
    assert_eq!(
        get(&r, v, "padding-sides"),
        PyValue::Tuple(vec![PyValue::None, s("3px"), PyValue::None, s("6px")])
    );
    let h = nth(&f, "div", 1);
    assert_eq!(get(&r, h, "margin-top"), s("7px"));
    assert_eq!(get(&r, h, "margin-right"), s("5px"));
    assert_eq!(
        get(&r, h, "padding-sides"),
        PyValue::Tuple(vec![s("6px"), PyValue::None, PyValue::None, PyValue::None])
    );
}

/// A long value read for many elements ends in `TooLarge` once its copies and reads pass the
/// budget, after work bounded by the budget, not by its length times the elements (kfxgen
/// copies or parses it for every element).
#[test]
fn a_long_value_read_for_every_element_is_too_large() {
    const SPANS: usize = 2_000;
    let long = "a".repeat(1 << 20);
    for (what, css) in [
        // Inherited from <body>, copied into every element's `font-family`.
        (
            "font-family on body",
            format!("body {{ font-family: {long} }}"),
        ),
        // Declared on every span, read by the decoration walk.
        (
            "text-decoration on spans",
            format!("span {{ text-decoration: underline {long} }}"),
        ),
    ] {
        let (mut dom, body) = empty_page();
        for _ in 0..SPANS {
            let s = dom.append_child(body, "span");
            dom.set_text(s, "x");
        }
        let file = file_of(dom, &css);
        let start = Instant::now();
        let budget = TextBudget::new();
        let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
        let mut resolved = 0;
        let stopped = file
            .dom
            .descendants()
            .find_map(|n| match resolver.resolve(n) {
                Ok(Some(r)) => match compute_block_style(&r) {
                    Ok(_) => {
                        resolved += 1;
                        None
                    }
                    Err(StyleError::TooLarge(e)) => Some(e),
                    Err(e) => panic!("{what}: {e:?}"),
                },
                Ok(None) => panic!("{what}: resolved to None"),
                Err(e) => Some(e),
            });
        let took = start.elapsed();
        eprintln!("{what}: TooLarge after {resolved} elements, {took:?}");
        assert!(
            matches!(stopped, Some(css::Error::Cascade(MAX_CASCADE_DECLS))),
            "{what}: {stopped:?}"
        );
        // 1 MiB is 16,384 units: the budget lasts about 30 reads of it.
        assert!(resolved < 40, "{what}: {resolved}");
        // Past the limit by at most one value's charge.
        assert!(budget.charged() <= MAX_CASCADE_DECLS + (1 << 20) / 64 * 2);
        assert!(took < COST_LIMIT, "{what} took {took:?}");
        // Another resolver for the same file shares its budget: no new allowance.
        let again = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
        let last = file.dom.descendants().last().unwrap();
        assert!(again.resolve(last).is_err(), "{what}: a second resolver");
    }
}

/// A `::first-letter` rule of 1,000 declarations on 100 paragraphs (the cascade already
/// counts each paragraph's pseudo-class declarations, so it refuses many more), each paragraph
/// resolved again and again, as the walks do: every resolution copies the rule, a unit a
/// declaration, so the budget ends it after about 500.
#[test]
fn a_long_first_letter_rule_is_too_large() {
    const PARAS: usize = 100;
    let (mut dom, body) = empty_page();
    for _ in 0..PARAS {
        let p = dom.append_child(body, "p");
        dom.set_text(p, "x");
    }
    let mut css = String::from("p::first-letter {");
    for i in 0..1_000 {
        css.push_str(&format!("x{i}:1;"));
    }
    css.push('}');
    let file = file_of(dom, &css);
    let paras: Vec<NodeId> = file
        .dom
        .descendants()
        .filter(|&n| file.dom.tag(n) == "p")
        .collect();
    let start = Instant::now();
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut resolved = 0;
    let stopped = paras
        .iter()
        .cycle()
        .take(100_000)
        .find_map(|&n| match resolver.resolve(n) {
            Ok(r) => {
                assert!(r.is_some());
                resolved += 1;
                None
            }
            Err(e) => Some(e),
        });
    let took = start.elapsed();
    eprintln!("first-letter of 1,000: TooLarge after {resolved} resolutions, {took:?}");
    assert!(
        matches!(stopped, Some(css::Error::Cascade(MAX_CASCADE_DECLS))),
        "{stopped:?}"
    );
    assert!((450..=501).contains(&resolved), "{resolved}");
    assert!(took < COST_LIMIT, "took {took:?}");
}
