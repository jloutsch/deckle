// SPDX-License-Identifier: GPL-3.0-only
//! Selector matching (phase 2 plan Task 7) against Calibre 9.18's own engine, `css_selectors`:
//! the matches recorded inside a kfxgen run (`cascade/css_quirks`'s `selectors.xhtml`, where
//! selector n sets `orphans: n`), the specificity css_parser gave every recorded rule, and
//! `data/css_select_probe.json`, Calibre's answers for a page and 266 selectors (how it was
//! made is in the file).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use deckle_kfx::epub_zip::EpubReader;
use deckle_kfx::fixtures;
use deckle_kfxgen::css::{DocIndex, Parsed, Selector, TooLarge, parse_selector, select};
use deckle_kfxgen::dom::{self, Dom, NodeId};
use deckle_limits::MAX_SELECTOR_WORK;
use serde_json::Value;

/// Every book under `fixtures/kfxgen/cascade/`.
const BOOKS: [&str; 13] = [
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

/// Recorded selectors Calibre's `Select` refuses (probed with calibre-debug: every recorded
/// selector run through `Select(…, ignore_inappropriate_pseudo_classes=True)`).
const CALIBRE_DROPS: [&str; 1] = ["#s37 li::marker"];

fn read(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn ok(text: &str) -> (Selector, Option<&'static str>, (u32, u32, u32)) {
    match parse_selector(text) {
        Parsed::Ok {
            selector,
            pseudo,
            specificity,
        } => (selector, pseudo, specificity),
        Parsed::Dropped => panic!("{text:?} dropped"),
    }
}

/// The elements `sel` matches, over an index of their own.
fn run(sel: &Selector, dom: &Dom) -> Vec<NodeId> {
    select(sel, dom, &DocIndex::new(dom)).unwrap()
}

fn paths(dom: &Dom, sel: &Selector) -> Vec<Vec<u32>> {
    run(sel, dom).into_iter().map(|n| dom.path(n)).collect()
}

/// Runs `f`, failing past 5 s: a debug build, tests run in parallel. (Release-build times are
/// in the phase 2 Task 7 report.)
fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    eprintln!("{what}: {took:?}");
    assert!(took < Duration::from_secs(5), "{what} took {took:?}");
    out
}

fn page(body: &str) -> Dom {
    let src = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{body}</body></html>"
    );
    // A tree hundreds of levels deep takes more than a test thread's 2 MiB to read in a debug
    // build (the XML parser); the selectors then run on the test's own thread.
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || dom::parse(src.as_bytes()).unwrap())
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn selector_matches_agree_with_calibre() {
    let dir = fixtures::kfxgen("cascade/css_quirks");
    let mut zip = EpubReader::open(&fixtures::kfxgen("sources/css_quirks.epub")).unwrap();
    let dom = dom::parse(&zip.read_bytes("OEBPS/selectors.xhtml").unwrap()).unwrap();
    let rules = read(&dir.join("rules/003.json"));
    let styles = read(&dir.join("styles/003.json"));

    // Which elements each selector matched in Calibre, and the pseudo-class it stored under.
    let table = styles["table"].as_array().unwrap();
    let mut want: BTreeMap<String, (BTreeSet<Vec<u32>>, Option<String>)> = BTreeMap::new();
    for node in styles["nodes"].as_array().unwrap() {
        let path: Vec<u32> = serde_json::from_value(node[0].clone()).unwrap();
        let style = &table[node[1].as_u64().unwrap() as usize];
        if let Some(n) = style["specified"]["orphans"].as_str() {
            let e = want.entry(n.to_owned()).or_default();
            e.0.insert(path.clone());
        }
        for (name, decls) in style["pseudo"].as_object().into_iter().flatten() {
            if let Some(n) = decls["orphans"].as_str() {
                let e = want.entry(n.to_owned()).or_default();
                e.0.insert(path.clone());
                e.1 = Some(name.clone());
            }
        }
    }

    let mut agreed = 0;
    let mut failed = Vec::new();
    let mut compared = 0;
    for rule in rules["rules"].as_array().unwrap() {
        if rule[0] != 1 {
            continue;
        }
        compared += 1;
        let text = rule[1].as_str().unwrap();
        let n = rule[2]["orphans"].as_str().unwrap();
        let key: Vec<u32> = serde_json::from_value(rule[4].clone()).unwrap();
        let (want_paths, want_pseudo) = want.remove(n).unwrap_or_default();
        let got = match parse_selector(text) {
            Parsed::Dropped => None,
            Parsed::Ok {
                selector,
                pseudo,
                specificity,
            } => Some((
                paths(&dom, &selector).into_iter().collect::<BTreeSet<_>>(),
                pseudo.map(str::to_owned),
                specificity,
            )),
        };
        let agrees = match &got {
            // Calibre's Stylizer logged and skipped it: nothing matched.
            None => want_paths.is_empty() && CALIBRE_DROPS.contains(&text),
            Some((p, pseudo, spec)) => {
                *p == want_paths && *pseudo == want_pseudo && *spec == (key[2], key[3], key[4])
            }
        };
        if agrees {
            agreed += 1;
        } else {
            failed.push(format!(
                "{text}: got {got:?}, want {want_paths:?} {want_pseudo:?} {:?}",
                &key[2..5]
            ));
        }
    }
    assert_eq!(compared, 39, "selectors.xhtml's rules");
    assert!(
        failed.is_empty(),
        "{agreed} of {compared} agree; differ:\n{}",
        failed.join("\n")
    );
    assert!(want.is_empty(), "matched in Calibre by no rule: {want:?}");
}

#[test]
fn recorded_selectors_have_calibres_specificity() {
    // key = [origin, a, b, c, d, index]: css_parser's (a, b, c, d), where a counts a style
    // attribute and is 0 for a rule, so the port's (ids, classes, types) is key[2..5].
    let mut rules: Vec<Value> = read(&fixtures::kfxgen("cascade/ua_rules.json"))["rules"]
        .as_array()
        .unwrap()
        .clone();
    for book in BOOKS {
        let dir = fixtures::kfxgen(&format!("cascade/{book}/rules"));
        for entry in std::fs::read_dir(dir).unwrap() {
            let dump = read(&entry.unwrap().path());
            rules.extend(dump["rules"].as_array().unwrap().iter().cloned());
        }
    }
    let mut seen = BTreeSet::new();
    let mut failed = Vec::new();
    for rule in &rules {
        let text = rule[1].as_str().unwrap();
        let key: Vec<u32> = serde_json::from_value(rule[4].clone()).unwrap();
        if !seen.insert((text.to_owned(), key.clone())) {
            continue;
        }
        assert_eq!(key[1], 0, "{text}: a rule's a");
        match parse_selector(text) {
            Parsed::Ok { specificity, .. } if specificity == (key[2], key[3], key[4]) => {}
            Parsed::Dropped if CALIBRE_DROPS.contains(&text) => {}
            Parsed::Ok { specificity, .. } => {
                failed.push(format!("{text}: {specificity:?}, want {:?}", &key[2..5]))
            }
            Parsed::Dropped => failed.push(format!("{text}: dropped")),
        }
    }
    assert!(seen.len() > 300, "{} selectors", seen.len());
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Probe cases where the port and Calibre part on purpose (`DIVERGENCES.md`, "Selectors
/// Calibre's engine reads its own way"): (selector, why).
const PROBE_DIVERGENCES: [(&str, &str); 8] = [
    // Calibre's evaluation is lazy: an unknown pseudo-class it never reaches raises nothing.
    (":not(:foo.zz)", "unreached unknown pseudo-class"),
    (":has(:foo zz)", "unreached unknown pseudo-class"),
    // Calibre calls a dispatch function of another signature and raises TypeError or
    // KeyError, which stops its conversion; the port drops the rule.
    ("p:root()", "crash"),
    ("p:empty()", "crash"),
    (":class(a)", "crash"),
    ("p:function(x)", "crash"),
    ("p:root(x)", "crash"),
    ("[title!=x]", "crash"),
];

#[test]
fn probed_selectors_agree_with_calibre() {
    let probe = read(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/it/data/css_select_probe.json"),
    );
    let dom = dom::parse(probe["page"].as_str().unwrap().as_bytes()).unwrap();
    let divergent: BTreeMap<&str, &str> = PROBE_DIVERGENCES.into_iter().collect();
    let mut agreed = 0;
    let mut failed = Vec::new();
    let cases = probe["cases"].as_array().unwrap();
    for case in cases {
        let text = case["selector"].as_str().unwrap();
        let got = parse_selector(text);
        let agrees = match (&got, case.get("matches")) {
            (
                Parsed::Ok {
                    selector, pseudo, ..
                },
                Some(want),
            ) => {
                let want: Vec<Vec<u32>> = serde_json::from_value(want.clone()).unwrap();
                let mut want = want;
                want.sort();
                // Calibre's pseudo_pat keeps the name as the text spells it; the port reports
                // it in lower case.
                let want_pseudo = case["pseudo"].as_str().map(str::to_ascii_lowercase);
                paths(&dom, selector) == want && pseudo.map(str::to_owned) == want_pseudo
            }
            // A crash stops Calibre's conversion: never what the port does.
            (Parsed::Dropped, None) => case["error"]
                .as_str()
                .is_some_and(|e| !e.starts_with("crash")),
            _ => false,
        };
        if agrees {
            agreed += 1;
            continue;
        }
        if divergent.contains_key(text) {
            assert!(
                matches!(got, Parsed::Dropped),
                "{text}: diverges, but not dropped"
            );
            continue;
        }
        let got = match &got {
            Parsed::Ok {
                selector, pseudo, ..
            } => format!("{:?} {pseudo:?}", paths(&dom, selector)),
            Parsed::Dropped => "dropped".to_owned(),
        };
        failed.push(format!("{text:?}: got {got}, want {case}"));
    }
    assert!(
        failed.is_empty(),
        "{agreed} of {} agree; differ:\n{}",
        cases.len(),
        failed.join("\n")
    );
    assert_eq!(agreed + PROBE_DIVERGENCES.len(), cases.len());
}

#[test]
fn specificity_cases() {
    // From css_parser 1.1.1 inside calibre 9.18 (`Selector.specificity`).
    for (text, want) in [
        ("*", (0, 0, 0)),
        ("svg|svg", (0, 0, 1)),
        (":where(#a)", (0, 0, 0)),
        (":not(#x)", (1, 0, 0)),
        (":not(p, #a)", (1, 0, 0)),
        (":not(:not(#a))", (1, 0, 0)),
        (":not(p .a)", (0, 1, 1)),
        (":is(#a, p)", (1, 0, 0)),
        (":is(a b c d, .x)", (0, 1, 0)),
        (":is(.a .b .c, .d #e)", (1, 1, 0)),
        (":has(> p .a)", (0, 1, 1)),
        (":has(.a, p p p)", (0, 1, 0)),
        ("p:not(.a):not(.b)", (0, 2, 1)),
        (":nth-child(2n+1)", (0, 1, 0)),
        ("p:lang(fr)", (0, 1, 1)),
        ("p:hover", (0, 1, 1)),
        ("p::first-letter", (0, 0, 2)),
        ("p:first-line", (0, 0, 2)),
        (":before", (0, 0, 1)),
        ("p::first-child", (0, 0, 2)),
        (":is(p::before, li)", (0, 0, 2)),
        (":where(p::before)", (0, 0, 0)),
        (":is(p:hover, li)", (0, 1, 1)),
        (":first-child:root", (0, 2, 0)),
        ("p:descendant(x)", (0, 1, 1)),
        // The `of` list adds nothing: the pseudo-class counts as one class.
        ("li:nth-child(2n+1 of li)", (0, 1, 1)),
        (":nth-last-child(1 of span)", (0, 1, 0)),
        (":nth-child(2 of li span)", (0, 1, 0)),
        // css_parser reads :matches() as a function of no selectors.
        (":matches(p)", (0, 1, 0)),
        ("a > b + c ~ d", (0, 0, 4)),
    ] {
        assert_eq!(ok(text).2, want, "{text}");
    }
}

#[test]
fn hover_rule_reports_pseudo_and_matches_all_p() {
    let dom = page("<p>a</p><div><p>b</p></div><span>c</span>");
    let (sel, pseudo, spec) = ok("p:hover");
    assert_eq!(pseudo, Some("hover"));
    assert_eq!(spec, (0, 1, 1));
    let all_p: Vec<_> = dom.descendants().filter(|&n| dom.tag(n) == "p").collect();
    assert_eq!(run(&sel, &dom), all_p);
    // The name is found anywhere in the text, as stylizer.py's pseudo_pat finds it.
    assert_eq!(ok("a:not(:hover)").1, Some("hover"));
    assert_eq!(ok("[title=\":visited\"]").1, Some("visited"));
    assert_eq!(ok(".x:focus-within").1, Some("focus-within"));
    assert_eq!(ok("p:first-child").1, None);
}

#[test]
fn marker_rule_is_dropped() {
    for text in [
        "li::marker",
        "p::selection",
        "p::foo(2)",
        "p:foo",
        "p:foo(x)",
        ":not(::before)",
        ":has(:has(b))",
        "li:nth-child(ODD)",
        "p:lang(en, fr)",
        "",
        "p >",
    ] {
        assert!(matches!(parse_selector(text), Parsed::Dropped), "{text:?}");
    }
}

/// 254 nested divs under `<body>` (the deepest tree `dom::parse` keeps), the innermost holding
/// `wide` paragraphs with a span each: about 2 × `wide` elements.
fn deep_and_wide(wide: usize) -> Dom {
    let leaves = "<p class=\"k\"><span>x</span></p>".repeat(wide);
    page(&format!(
        "{}{leaves}{}<p>tail</p>",
        "<div>".repeat(253),
        "</div>".repeat(253)
    ))
}

#[test]
fn has_over_a_deep_tree_finishes() {
    let dom = deep_and_wide(50_000);
    assert!(dom.descendants().count() > 100_000);
    for (text, want) in [
        ("div:has(p)", 253),
        ("div:has(> p span)", 1),
        ("div:has(div div div p > span)", 250),
        ("p:has(~ p)", 49_999),
        ("div:has(+ p)", 1),
        (":has(span:not(:has(i)))", 0),
    ] {
        let got = match parse_selector(text) {
            Parsed::Ok { selector, .. } => timed(text, || run(&selector, &dom)).len(),
            Parsed::Dropped => 0,
        };
        assert_eq!(got, want, "{text}");
    }
    for text in [
        "div div div div div div p span",
        "body div > div div > div p > span",
        "div ~ p",
        "p + p + p + p",
        "div:not(:has(p)) span",
        ":is(div div p, span) span",
    ] {
        let (sel, ..) = ok(text);
        timed(text, || run(&sel, &dom));
    }
}

#[test]
fn nth_over_a_hundred_thousand_siblings_finishes() {
    let dom = page(&format!(
        "<ul>{}</ul>",
        "<li class=\"k\">a</li><li>b</li>".repeat(50_000)
    ));
    for (text, want) in [
        ("li:nth-child(2n+1)", 50_000),
        ("li:nth-last-child(3)", 1),
        ("li:nth-of-type(odd)", 50_000),
        ("li:nth-last-of-type(-n+10)", 10),
        ("li:last-of-type", 1),
        ("li:only-of-type", 0),
        ("li:first-child", 1),
        ("li:nth-child(2 of .k)", 1),
        ("li:nth-last-child(odd of .k)", 25_000),
        ("li:nth-child(n of :nth-child(n of .k))", 50_000),
        ("li + li ~ li", 99_998),
        ("li.x ~ li", 0),
        ("li:not(li.x ~ li)", 100_000),
        ("li:not(:nth-child(n of li.x))", 100_000),
    ] {
        let (sel, ..) = ok(text);
        assert_eq!(timed(text, || run(&sel, &dom)).len(), want, "{text}");
    }
}

#[test]
fn deep_nesting_is_dropped_without_overflow() {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            for f in ["not", "is", "where", "has"] {
                let text = format!("{}p{}", format!(":{f}(").repeat(10_000), ")".repeat(10_000));
                let got = timed(f, || parse_selector(&text));
                assert!(matches!(got, Parsed::Dropped), "{f}");
            }
            let text = format!("{}p{}", ":is(".repeat(10_000), ")".repeat(9_999));
            assert!(matches!(parse_selector(&text), Parsed::Dropped));
            let text = format!(
                "li:nth-child({}1 of p{})",
                "1 of :nth-child(".repeat(5_000),
                ")".repeat(5_000)
            );
            assert!(matches!(parse_selector(&text), Parsed::Dropped));
            // 32 levels, the depth css_parser's reading keeps (`MAX_NESTING`), match.
            let text = format!("{}p{}", ":not(:not(".repeat(16), "))".repeat(16));
            let (sel, ..) = ok(&text);
            let dom = page("<p>a</p><i>b</i>");
            assert_eq!(run(&sel, &dom).len(), 1);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn thousand_part_selector_is_dropped() {
    let parts = |n: usize| vec!["p"; n].join(" ");
    assert!(matches!(parse_selector(&parts(1_000)), Parsed::Dropped));
    assert!(matches!(parse_selector(&parts(257)), Parsed::Dropped));
    // Parts inside :is() count too.
    let text = format!(":is({}) p", parts(256));
    assert!(matches!(parse_selector(&text), Parsed::Dropped));
    let (sel, _, spec) = ok(&parts(256));
    assert_eq!(spec, (0, 0, 256));
    let dom = page(&format!("{}x{}", "<p>".repeat(200), "</p>".repeat(200)));
    assert!(timed("256 parts", || run(&sel, &dom)).is_empty());
    let (sel, ..) = ok(&parts(200));
    assert_eq!(run(&sel, &dom).len(), 1);
    // Parts of one compound count as well: Calibre stops with a RecursionError past about
    // 400 of them (probed with calibre-debug).
    assert!(matches!(
        parse_selector(&format!("p{}", ".a".repeat(256))),
        Parsed::Dropped
    ));
    let (sel, ..) = ok(&format!("p{}", ".a".repeat(255)));
    let dom = page(&"<p class=\"a\">x</p>".repeat(50_000));
    assert_eq!(timed("255 classes", || run(&sel, &dom)).len(), 50_000);
}

#[test]
fn selectors_matching_everything_finish() {
    let dom = deep_and_wide(50_000);
    let all = dom.descendants().count();
    for text in [
        "*",
        ":hover",
        "*:not(:has(b))",
        ":is(*, p, div)",
        "* *",
        ":lang(en)",
    ] {
        let (sel, ..) = ok(text);
        let got = timed(text, || run(&sel, &dom)).len();
        if text == ":lang(en)" {
            assert_eq!(got, 0, "no lang attribute");
        } else {
            assert!(got >= all - 1, "{text}: {got} of {all}");
        }
    }
}

/// 1,000 chapters of 100 elements: a `div` (classes `chapter` and `c<i>`, id `ch<i>`, and
/// `data-v<i % 100>`), an `h2`, and 49 paragraphs (class `p<j>`) holding a `span` each.
fn chapters() -> Dom {
    let mut body = String::new();
    for i in 0..1_000 {
        body.push_str(&format!(
            "<div class=\"chapter c{i}\" id=\"ch{i}\" data-v{}=\"x\"><h2>T</h2>",
            i % 100
        ));
        for j in 0..49 {
            body.push_str(&format!("<p class=\"p{j}\"><span>x</span></p>"));
        }
        body.push_str("</div>");
    }
    page(&body)
}

/// Selector `i` of the kinds books use: a class, a name and class, `a b`, `a > b`, `[attr]`.
fn realistic(i: usize) -> String {
    match i % 5 {
        0 => format!(".c{i}"),
        1 => format!("p.p{}", i % 50),
        2 => format!("div.c{i} span"),
        3 => format!("#ch{i} > h2"),
        _ => format!("[data-v{}]", i % 200),
    }
}

/// Matches `n` realistic selectors against [`chapters`] over one index: the matches, and the
/// work charged.
fn match_realistic(n: usize) -> (usize, u64) {
    let dom = chapters();
    assert!(dom.descendants().count() > 100_000);
    let sels: Vec<Selector> = (0..n).map(|i| ok(&realistic(i)).0).collect();
    let index = DocIndex::new(&dom);
    let found = timed("realistic selectors", || {
        sels.iter()
            .map(|sel| select(sel, &dom, &index).unwrap().len())
            .sum()
    });
    (found, index.work())
}

/// The work of matching every rule of a file: Calibre's user-agent rules and `rules`.
fn rules_work(dom: &Dom, rules: &[String]) -> u64 {
    let ua = read(&fixtures::kfxgen("cascade/ua_rules.json"));
    let ua = ua["rules"].as_array().unwrap().iter();
    let index = DocIndex::new(dom);
    for text in ua
        .map(|r| r[1].as_str().unwrap().to_owned())
        .chain(rules.iter().cloned())
    {
        if let Parsed::Ok { selector, .. } = parse_selector(&text) {
            select(&selector, dom, &index).unwrap();
        }
    }
    index.work()
}

/// A book of 20,000 elements as converters write them: chapters of headings, paragraphs of
/// generated classes holding emphasis, links and notes, lists, tables and quotations.
fn big_book() -> Dom {
    let mut body = String::new();
    for ch in 0..115 {
        body.push_str(&format!(
            "<div xmlns:epub=\"http://www.idpf.org/2007/ops\" class=\"chapter\" id=\"c{ch}\">\
             <h1 class=\"calibre{}\">Title</h1>",
            ch % 3
        ));
        for i in 0..40 {
            body.push_str(&format!(
                "<p class=\"calibre{}\">Text <em>a</em> <span class=\"s{}\">b</span> \
                 <a href=\"#n{i}\" epub:type=\"noteref\">1</a></p>",
                i % 30,
                i % 7
            ));
        }
        body.push_str("<blockquote><p class=\"q\">x</p></blockquote>");
        body.push_str("<ul><li>a</li><li>b</li><li><p>c</p></li></ul>");
        body.push_str("<table><tr><td>a</td><td>b</td></tr></table>");
        body.push_str("<aside epub:type=\"footnote\" id=\"n1\"><p>note</p></aside></div>");
    }
    let dom = page(&body);
    assert!((19_000..21_000).contains(&dom.descendants().count()));
    dom
}

/// 2,000 rules of the kinds books hold.
fn big_book_rules() -> Vec<String> {
    (0..2_000)
        .map(|i| match i % 20 {
            0 => format!(".calibre{}", i % 40),
            1 => format!("p.calibre{}", i % 40),
            2 => "p".to_owned(),
            3 => format!("span.s{}", i % 9),
            4 => ".chapter p".to_owned(),
            5 => "div > p".to_owned(),
            6 => "h1 + p".to_owned(),
            7 => "p + p".to_owned(),
            8 => "blockquote p".to_owned(),
            9 => "a:hover".to_owned(),
            10 => "li > p".to_owned(),
            11 => "[type~=footnote]".to_owned(),
            12 => "p:first-child".to_owned(),
            13 => "table td".to_owned(),
            14 => "p:first-letter".to_owned(),
            15 => format!("#c{} h1", i % 120),
            16 => ".chapter > h1 ~ p".to_owned(),
            17 => "em, i".to_owned(),
            18 => "a[href]".to_owned(),
            _ => "*".to_owned(),
        })
        .collect()
}

#[test]
fn real_books_stay_far_below_the_work_limit() {
    let mut largest = (0, String::new());
    for book in BOOKS {
        let dir = fixtures::kfxgen(&format!("cascade/{book}"));
        for entry in std::fs::read_dir(dir.join("rules")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let rules: Vec<String> = read(&path)["rules"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r[1].as_str().unwrap().to_owned())
                .collect();
            let dom =
                dom::parse(&std::fs::read(dir.join(format!("dom/{name}.xhtml"))).unwrap()).unwrap();
            let work = rules_work(&dom, &rules);
            if work > largest.0 {
                largest = (work, format!("{book}/{name}"));
            }
        }
    }
    let dom = big_book();
    let big = timed("2,000 rules", || rules_work(&dom, &big_book_rules()));
    eprintln!(
        "largest fixture file {}: {} steps; 2,000 rules × 20,000 elements: {big} steps",
        largest.1, largest.0
    );
    assert!(largest.0 < MAX_SELECTOR_WORK / 1_000, "{largest:?}");
    // About 12 million steps, 12% of the limit (a fifth here since the Task 8 review halved
    // the limit from 200 million).
    assert!(big < MAX_SELECTOR_WORK / 5, "{big}");
}

#[test]
fn five_thousand_realistic_selectors_use_the_index() {
    // A pass over every element for each selector would be 500 million tests, past the limit.
    let (found, work) = match_realistic(5_000);
    // .c<i> and div.c<i> span: 200 and 200 × 49 for i < 1,000; #ch<i> > h2: 200; p.p<k>:
    // 1,000 × 1,000; [data-v<k>]: 25 per k < 100 that is 4 mod 5, on 10 divs each.
    assert_eq!(found, 200 + 1_000_000 + 9_800 + 200 + 20 * 25 * 10);
    // p.p<k> is 1,000 selectors over 1,000 elements, two tests each.
    assert!(work < 3_000_000, "{work}");
}

/// Release build: `cargo test --release -p deckle-kfxgen --test it -- --ignored fifty_thousand`.
#[test]
#[ignore = "about a second in a release build, much longer in a debug one"]
fn fifty_thousand_realistic_selectors_finish() {
    let (found, work) = match_realistic(50_000);
    assert_eq!(found, 200 + 10_000_000 + 9_800 + 200 + 20 * 250 * 10);
    assert!(work < MAX_SELECTOR_WORK / 4, "{work}");
}

/// Matches `sels` over one index of `dom` until one gives [`TooLarge`]: how many matched
/// before it.
fn until_too_large(dom: &Dom, sels: impl Iterator<Item = String>) -> usize {
    let index = DocIndex::new(dom);
    let mut done = 0;
    for text in sels {
        let (sel, ..) = ok(&text);
        match select(&sel, dom, &index) {
            Ok(_) => done += 1,
            Err(TooLarge) => {
                // And every call after it.
                assert_eq!(select(&ok("p").0, dom, &index), Err(TooLarge));
                return done;
            }
        }
    }
    panic!("never too large: {} steps", index.work());
}

#[test]
fn worst_case_selectors_are_charged() {
    // 256 steps an element for 255 classes; every element a step for each of 256 compounds.
    let dom = page(&"<p class=\"a\">x</p>".repeat(10_000));
    let index = DocIndex::new(&dom);
    let (sel, ..) = ok(&format!("p{}", ".a".repeat(255)));
    assert_eq!(select(&sel, &dom, &index).unwrap().len(), 10_000);
    assert!(index.work() >= 256 * 10_000, "{}", index.work());
    let dom = deep_and_wide(2_000);
    let index = DocIndex::new(&dom);
    let (sel, ..) = ok(&vec!["*"; 256].join(" "));
    // The paragraphs and spans, 256 and 257 levels down.
    assert_eq!(select(&sel, &dom, &index).unwrap().len(), 4_000);
    assert!(index.work() > 250 * 4_000, "{}", index.work());
    // So at most this many such selectors run on a document of 100,000 elements.
    const { assert!(MAX_SELECTOR_WORK / (256 * 100_000) < 10) };
}

/// Release build: `cargo test --release -p deckle-kfxgen --test it -- --ignored worst_case`.
#[test]
#[ignore = "about two seconds in a release build, much longer in a debug one"]
fn worst_case_selectors_are_refused() {
    // 255 classes on 100,000 elements, and 256 compounds each over the deep tree's 100,000:
    // about 25 million steps a selector.
    let dom = page(&"<p class=\"a\">x</p>".repeat(100_000));
    let classes = format!("p{}", ".a".repeat(255));
    let done = timed("255 classes", || {
        until_too_large(&dom, std::iter::repeat_n(classes, 50_000))
    });
    assert!(done < 50, "{done}");
    let dom = deep_and_wide(50_000);
    let stars = vec!["*"; 256].join(" ");
    let done = timed("256 compounds", || {
        until_too_large(&dom, std::iter::repeat_n(stars, 50_000))
    });
    assert!(done < 50, "{done}");
}

/// How long each kind of work takes per step, to size [`MAX_SELECTOR_WORK`]: run in a release
/// build with `cargo test --release -p deckle-kfxgen --test it -- --ignored --nocapture
/// selector_work_rates`.
#[test]
#[ignore = "a measurement, for a release build"]
fn selector_work_rates() {
    let classes = page(&"<p class=\"a\" title=\"Ab\">x</p>".repeat(100_000));
    let deep = deep_and_wide(50_000);
    let long = page(&format!("<p title=\"{}\">x</p>", "Ab".repeat(1_000)).repeat(10_000));
    let cases: [(&str, &Dom, String); 8] = [
        ("255 classes", &classes, format!("p{}", ".a".repeat(255))),
        (
            "255 attribute tests",
            &classes,
            format!("p{}", "[title=Ab]".repeat(255)),
        ),
        (
            "127 :not()",
            &classes,
            format!("p{}", ":not(.b)".repeat(127)),
        ),
        ("256 compounds", &deep, vec!["*"; 256].join(" ")),
        ("256 child compounds", &deep, vec!["*"; 256].join(" > ")),
        (
            "127 :has()",
            &deep,
            format!("*{}", ":has(span)".repeat(127)),
        ),
        ("*= i on long values", &long, "[title*=b i]".repeat(100)),
        (
            "127 :nth-child(of)",
            &classes,
            format!("p{}", ":nth-child(2n+1 of .a)".repeat(127)),
        ),
    ];
    for (what, dom, text) in cases {
        let (sel, ..) = ok(&text);
        let index = DocIndex::new(dom);
        let start = Instant::now();
        let mut runs = 0;
        while select(&sel, dom, &index).is_ok() {
            runs += 1;
        }
        let took = start.elapsed();
        eprintln!(
            "{what}: {runs} selectors, {} steps a selector, {took:?} to the limit, {:.2} ns a step",
            index.work() / (runs + 1),
            took.as_nanos() as f64 / index.work() as f64
        );
    }
}

#[test]
fn long_selector_is_dropped_unread() {
    let text = format!(".{}", "a".repeat(1 << 20));
    assert!(matches!(
        timed("1 MB", || parse_selector(&text)),
        Parsed::Dropped
    ));
    let text = format!(".{}", "a".repeat(64 * 1024 - 1));
    assert!(matches!(parse_selector(&text), Parsed::Ok { .. }));
    let text = format!(".{}", "a".repeat(64 * 1024));
    assert!(matches!(parse_selector(&text), Parsed::Dropped));
}

#[test]
fn long_attribute_values_are_read_once() {
    // A value of 30,000 bytes, in the selector, is looked at when the selector is read, not
    // again for each element.
    let dom = page(&"<p title=\"a b\" lang=\"x\">x</p>".repeat(100_000));
    let big = "x".repeat(30_000);
    let spaced = format!("{big} {big}");
    for (text, want) in [
        (format!("[title~=\"{spaced}\"]"), 0),
        (format!("[title~=\"{big}\"]"), 0),
        (format!("[title~=\"{big}\" i]"), 0),
        (format!("[title=\"{big}\" i]"), 0),
        (format!("[title*=\"{big}\" i]"), 0),
        (format!("[title^=\"{big}\" i]"), 0),
        ("[title~=\"A\" i]".to_owned(), 100_000),
        ("[title~=\"a b\"]".to_owned(), 0),
        ("[title~=\"\"]".to_owned(), 0),
        ("[title=\"A B\" i]".to_owned(), 100_000),
        ("[title|=\"A\" i]".to_owned(), 0),
        ("[title$=\" B\" i]".to_owned(), 100_000),
        ("[title*=\"A \" i]".to_owned(), 100_000),
    ] {
        let (sel, ..) = ok(&text);
        assert_eq!(
            timed("long value", || run(&sel, &dom)).len(),
            want,
            "{}",
            &text[..text.len().min(40)]
        );
    }
}

#[test]
fn lang_with_many_subtags_finishes() {
    // Calibre lists every combination of the subtags (2^n); the port does not.
    let lang = (0..2_000)
        .map(|i| format!("s{i}"))
        .collect::<Vec<_>>()
        .join("-");
    let dom = page(&format!(
        "<div lang=\"de-{lang}\">{}</div>",
        "<p>x</p>".repeat(20_000)
    ));
    for (text, want) in [
        (":lang(de-s5-s1999)", 20_001),
        (":lang(de-s1999-s5)", 0),
        (":lang(de)", 20_001),
        (":lang(s5)", 0),
    ] {
        let (sel, ..) = ok(text);
        assert_eq!(timed(text, || run(&sel, &dom)).len(), want, "{text}");
    }
}
