// SPDX-License-Identifier: GPL-3.0-only
//! Style values (phase 2 plan Task 10): what Calibre 9.18's `Style[prop]` gives kfxgen for each
//! element of the flattened files, against `computed` in the dumps under
//! `fixtures/kfxgen/cascade/` (`styles/NNN.json`), and the cost of asking for them all on large
//! pages.

use std::collections::BTreeSet;
use std::time::Instant;

use deckle_kfxgen::css::{
    ClassNamer, DEFAULT_PROFILE, Error, Flat, Origin, Styles, Value, cascade, flatten, parse_sheet,
    prepare_body,
};
use deckle_kfxgen::dom::{self, Dom, NodeId};

use crate::css_flatten::{File, body, find, fixtures_dir, flat_page, flatten_book, path_of, read};
use crate::support::BOOKS;

/// `tools/ref/kfxgen_hook/book_dump.py` `COMPUTED_PROPS`, in its order.
const COMPUTED_PROPS: [&str; 34] = [
    "text-align",
    "font-family",
    "font-style",
    "white-space",
    "list-style-type",
    "border-collapse",
    "border-top-style",
    "border-top-width",
    "border-top-color",
    "border-right-style",
    "border-right-width",
    "border-right-color",
    "border-bottom-style",
    "border-bottom-width",
    "border-bottom-color",
    "border-left-style",
    "border-left-width",
    "border-left-color",
    "font-size",
    "line-height",
    "width",
    "height",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "text-indent",
    "color",
    "background-color",
    "font-weight",
];

/// Fixture values the port computes differently from kfxgen's dump, each with its
/// `DIVERGENCES.md` section: (book, file, path, property, section).
const ALLOWED: &[(&str, &str, &[u32], &str, &str)] = &[];

/// Whether `got` is the dump's `want`: numbers within 1e-9 (NaN equal to NaN), text equal,
/// `null` for no value, `{"error": …}` for an error.
fn same(got: &Result<Value, Error>, want: &serde_json::Value) -> bool {
    use serde_json::Value as J;
    let number = |x: f64, w: f64| (x.is_nan() && w.is_nan()) || x == w || (x - w).abs() <= 1e-9;
    match (got, want) {
        (Ok(Value::Pt(x)), J::Number(w)) => number(*x, w.as_f64().unwrap()),
        (Ok(Value::Pt(x)), J::Object(o)) if o.contains_key("number") => {
            let w = match o["number"].as_str().unwrap() {
                "nan" => f64::NAN,
                "inf" => f64::INFINITY,
                "-inf" => f64::NEG_INFINITY,
                other => panic!("{other}"),
            };
            number(*x, w)
        }
        (Ok(Value::Text(t)), J::String(w)) => t == w,
        (Ok(Value::Absent), J::Null) => true,
        (Err(Error::NoDefault { .. }), J::Object(o)) => o.contains_key("error"),
        _ => false,
    }
}

#[test]
fn every_cascade_fixture_computed_matches_kfxgens_stylizer() {
    let mut failed = Vec::new();
    let mut allowed_used = BTreeSet::new();
    let mut compared = 0;
    for name in BOOKS {
        let (book, _) = flatten_book(name);
        for f in book {
            let File { nnn, dom, flat, .. } = &f;
            let styles = Styles::new(dom, flat, &DEFAULT_PROFILE);
            let dump = read(&fixtures_dir(name).join(format!("styles/{nnn}.json")));
            let table = dump["table"].as_array().unwrap();
            for node in dump["nodes"].as_array().unwrap() {
                let path = path_of(&node[0]);
                // Calibre's flattener links two new stylesheets at the end of <head>; the
                // port's tree has no such links (`css_flatten.rs` checks it).
                let Some(n) = find(dom, &path) else {
                    continue;
                };
                let want = &table[node[1].as_u64().unwrap() as usize]["computed"];
                for prop in COMPUTED_PROPS {
                    compared += 1;
                    let got = styles.computed(n, prop);
                    if same(&got, &want[prop]) {
                        continue;
                    }
                    let key = ALLOWED.iter().find(|(b, file, p, q, _)| {
                        *b == name && file == nnn && *p == path && *q == prop
                    });
                    match key {
                        Some(k) => {
                            allowed_used.insert((k.0, k.1, k.2, k.3));
                        }
                        None => failed.push(format!(
                            "{name} {} {path:?} {prop}: got {got:?}, want {}",
                            f.href, want[prop]
                        )),
                    }
                }
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{}\n{} mismatches",
        failed.join("\n"),
        failed.len()
    );
    for (b, file, p, q, _) in ALLOWED {
        assert!(
            allowed_used.contains(&(*b, *file, *p, *q)),
            "{b} {file} {p:?} {q} is allowed but matches: take it out"
        );
    }
    assert!(compared > 30_000, "{compared} values");
}

/// `css_quirks`'s `values.xhtml`, flattened with the rest of its book.
fn values_page() -> File {
    let (book, _) = flatten_book("css_quirks");
    book.into_iter()
        .find(|f| f.href == "OEBPS/values.xhtml")
        .unwrap()
}

fn pt(styles: &Styles<'_>, n: NodeId, prop: &str) -> f64 {
    match styles.computed(n, prop) {
        Ok(Value::Pt(v)) => v,
        other => panic!("{prop}: {other:?}"),
    }
}

fn at(f: &File, path: &[u32]) -> NodeId {
    find(&f.dom, path).unwrap()
}

#[test]
fn twelve_px_is_8_64pt() {
    let f = values_page();
    let styles = Styles::new(&f.dom, &f.flat, &DEFAULT_PROFILE);
    // <p class="u-px">: `font-size: 12px`, 12 × 72 / 100.
    assert_eq!(pt(&styles, at(&f, &[1, 1]), "font-size"), 8.64);
}

#[test]
fn font_size_large_is_13_2pt() {
    let f = values_page();
    let styles = Styles::new(&f.dom, &f.flat, &DEFAULT_PROFILE);
    // `large` is 13.5pt, which the first pass writes `1.1rem` (13.5 / 12 to one place).
    let n = at(&f, &[1, 16]);
    assert_eq!(styles.get(n, "font-size"), Some("1.1rem"));
    assert_eq!(pt(&styles, n, "font-size"), 1.1 * 12.0);
}

#[test]
fn line_height_inherits_as_points() {
    let f = values_page();
    let styles = Styles::new(&f.dom, &f.flat, &DEFAULT_PROFILE);
    // div.lh-parent { font-size: 20px; line-height: 1.5 } → 1.5 × 14.4; its <p>'s font is
    // 7.2pt, but it takes the parent's 21.6pt, not 1.5 × its own.
    let div = at(&f, &[1, 41]);
    let p = at(&f, &[1, 41, 0]);
    assert_eq!(pt(&styles, div, "line-height"), 1.5 * 14.4);
    assert_eq!(pt(&styles, p, "font-size"), 7.2);
    assert_eq!(pt(&styles, p, "line-height"), 1.5 * 14.4);
}

#[test]
fn text_indent_percent_is_of_own_width() {
    let f = values_page();
    let styles = Styles::new(&f.dom, &f.flat, &DEFAULT_PROFILE);
    // p.ti-pct { text-indent: 10%; width: 200px }: 10% of its own 144pt, not of the page.
    let n = at(&f, &[1, 27]);
    assert_eq!(pt(&styles, n, "width"), 144.0);
    assert!((pt(&styles, n, "text-indent") - 14.4).abs() < 1e-12);
}

#[test]
fn font_weight_700_computes_to_504() {
    let f = values_page();
    let styles = Styles::new(&f.dom, &f.flat, &DEFAULT_PROFILE);
    // A unitless number is read as pixels: 700 × 72 / 100.
    assert_eq!(pt(&styles, at(&f, &[1, 28]), "font-weight"), 504.0);
    assert_eq!(
        styles.computed(at(&f, &[1, 29]), "font-weight").unwrap(),
        Value::Text("bold".to_owned())
    );
}

#[test]
fn width_attribute_beats_css() {
    let f = values_page();
    let styles = Styles::new(&f.dom, &f.flat, &DEFAULT_PROFILE);
    // <table width="300"> under `width: 100px`: 300px.
    let table = at(&f, &[1, 43]);
    assert_eq!(styles.get(table, "width"), Some("100px"));
    assert_eq!(pt(&styles, table, "width"), 216.0);
    // <img width="64" style="width: 32px">: the attribute stays (CSS sizes it) and wins.
    assert_eq!(pt(&styles, at(&f, &[1, 44]), "width"), 46.08);
}

#[test]
fn a_property_with_no_default_is_an_error() {
    let (dom, flat) = flat_page(
        "",
        "<div style=\"x-own: inherit\"><p>x</p></div>",
        "p { x-other: 1px }",
    );
    let styles = Styles::new(&dom, &flat, &DEFAULT_PROFILE);
    let div = dom.children(body(&dom))[0];
    let p = dom.children(div)[0];
    // Not inherited and not declared: Calibre's `DEFAULTS[name]` raises KeyError.
    assert!(matches!(
        styles.inherited(div, "x-unknown"),
        Err(Error::NoDefault { prop }) if prop == "x-unknown"
    ));
    assert!(matches!(
        styles.computed(p, "x-unknown"),
        Err(Error::NoDefault { .. })
    ));
    // `inherit` asks the parent, up to the root, which has none either.
    assert!(matches!(
        styles.computed(div, "x-own"),
        Err(Error::NoDefault { .. })
    ));
    // Declared, it is the value.
    assert_eq!(styles.inherited(p, "x-other").unwrap(), "1px");
    assert_eq!(styles.computed(p, "x-other").unwrap(), Value::Pt(0.72));
    // Properties with a default never fail.
    assert_eq!(
        styles.computed(p, "font-style").unwrap(),
        Value::Text("normal".to_owned())
    );
}

#[test]
fn inherited_and_inherit_values_come_from_the_parent() {
    let (dom, flat) = flat_page(
        "",
        "<div><p class=\"a\">x</p><p class=\"b\">y</p></div>",
        "div { font-family: serif; margin-left: 3pt } p.a { margin-left: inherit } \
         p.b { color: inherit; font-style: initial }",
    );
    let styles = Styles::new(&dom, &flat, &DEFAULT_PROFILE);
    let div = dom.children(body(&dom))[0];
    let a = dom.children(div)[0];
    let b = dom.children(div)[1];
    assert_eq!(styles.get(a, "font-family"), None);
    assert_eq!(styles.inherited(a, "font-family").unwrap(), "serif");
    assert_eq!(styles.inherited(a, "margin-left").unwrap(), "3pt");
    // margin-left is not inherited: <p class="b"> has html.css's own.
    assert_eq!(styles.computed(b, "margin-left").unwrap(), Value::Pt(0.0));
    assert_eq!(
        styles.computed(b, "color").unwrap(),
        Value::Text("black".to_owned())
    );
    // `initial` and `unset` come back as text.
    assert_eq!(
        styles.computed(b, "font-style").unwrap(),
        Value::Text("initial".to_owned())
    );
}

#[test]
fn colours_and_backgrounds() {
    let (dom, flat) = flat_page(
        "",
        "<p class=\"a\">x</p><p class=\"b\">y</p><p class=\"c\">z</p>",
        "p.a { color: bogus; background: url(x.png) #abc no-repeat } \
         p.b { color: RGB(1, 2, 3); background-color: nope; background: none } \
         p.c { background-color: transparent; background: red }",
    );
    let styles = Styles::new(&dom, &flat, &DEFAULT_PROFILE);
    let kids = dom.children(body(&dom));
    let (a, b, c) = (kids[0], kids[1], kids[2]);
    // A colour css_parser's CSS 2.1 profile does not read is black.
    assert_eq!(
        styles.computed(a, "color").unwrap(),
        Value::Text("black".to_owned())
    );
    // As css_parser writes it.
    assert_eq!(
        styles.computed(b, "color").unwrap(),
        Value::Text("rgb(1, 2, 3)".to_owned())
    );
    // The first colour among `background`'s values.
    assert_eq!(
        styles.computed(a, "background-color").unwrap(),
        Value::Text("#abc".to_owned())
    );
    // Neither names a colour: no background colour.
    assert_eq!(
        styles.computed(b, "background-color").unwrap(),
        Value::Absent
    );
    // `transparent` is a colour, ahead of `background`.
    assert_eq!(
        styles.computed(c, "background-color").unwrap(),
        Value::Text("transparent".to_owned())
    );
    // Not inherited: the body has none.
    assert_eq!(
        styles.computed(body(&dom), "background-color").unwrap(),
        Value::Absent
    );
}

#[test]
fn pseudo_classes_are_kept_by_name() {
    let (dom, flat) = flat_page(
        "",
        "<a href=\"#x\">x</a>",
        "a:hover { color: red } a { color: blue }",
    );
    let styles = Styles::new(&dom, &flat, &DEFAULT_PROFILE);
    let a = dom.children(body(&dom))[0];
    let hover = styles.pseudo(a, "hover").unwrap();
    assert_eq!(hover.len(), 1);
    assert_eq!(hover[0].0, "color");
    assert_eq!(hover[0].1.value, "red");
    assert!(styles.pseudo(a, "active").is_none());
    assert_eq!(styles.get(a, "color"), Some("blue"));
}

/// A page of `total` elements under `<body>`: one branch of `deep` nested `<div>`s, each with
/// a `<span>`, and the rest `<p>`s side by side under `<body>`; styled so that every value is
/// worked out from the parent's.
fn big_page(deep: usize, total: usize) -> (Dom, Flat) {
    let mut dom = dom::parse(
        b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body/></html>",
    )
    .unwrap();
    let b = body(&dom);
    let mut n = b;
    let mut made = 0;
    for i in 0..deep {
        n = dom.append_child(n, "div");
        if i % 7 == 0 {
            dom.set_attr(n, "style", Some("font-size: 101%; line-height: 1.1"));
        }
        if i % 11 == 0 {
            dom.set_attr(n, "width", Some("99%"));
        }
        let s = dom.append_child(n, "span");
        dom.set_text(s, "x");
        made += 2;
    }
    while made < total {
        let p = dom.append_child(b, "p");
        dom.set_text(p, "x");
        made += 1;
    }
    prepare_body(&mut dom);
    let css = "div { margin-left: 1%; padding-left: 1em; color: inherit; text-indent: 2% } \
               span { width: 50%; margin-right: inherit; font-family: inherit } \
               p { height: 10%; line-height: normal }";
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

/// Asks for every property of every element, deepest first (so that no cache is warm), and
/// fails past `COST_LIMIT`.
fn every_value(what: &str, dom: &Dom, flat: &Flat) -> usize {
    let mut all: Vec<NodeId> = dom.descendants().collect();
    all.reverse();
    let start = Instant::now();
    let styles = Styles::new(dom, flat, &DEFAULT_PROFILE);
    let mut numbers = 0;
    for &n in &all {
        for prop in COMPUTED_PROPS {
            if let Ok(Value::Pt(_)) = styles.computed(n, prop) {
                numbers += 1;
            }
        }
    }
    let took = start.elapsed();
    eprintln!("{what}: {} elements, {took:?}", all.len());
    assert!(took < crate::support::COST_LIMIT, "{what} took {took:?}");
    numbers
}

#[test]
fn every_value_of_a_page_20_000_deep_cost() {
    let (dom, flat) = big_page(20_000, 100_000);
    assert!(dom.descendants().count() > 100_000);
    let numbers = every_value("20,000 deep", &dom, &flat);
    assert!(numbers > 1_000_000, "{numbers}");
    // The deepest <span>: 20,000 percentages of percentages, all worked out.
    let styles = Styles::new(&dom, &flat, &DEFAULT_PROFILE);
    let mut n = body(&dom);
    while let Some(&d) = dom.children(n).iter().find(|&&c| dom.tag(c) == "div") {
        n = d;
    }
    let span = dom.children(n)[0];
    assert!(matches!(styles.computed(span, "width"), Ok(Value::Pt(w)) if w >= 0.0));
}

#[test]
fn every_value_of_a_wide_page_cost() {
    let (dom, flat) = big_page(0, 100_000);
    let numbers = every_value("100,000 wide", &dom, &flat);
    assert!(numbers > 1_000_000, "{numbers}");
}
