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
use deckle_kfxgen::css::{Parsed, Selector, matches, parse_selector, select};
use deckle_kfxgen::dom::{self, Dom};
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

fn paths(dom: &Dom, sel: &Selector) -> Vec<Vec<u32>> {
    select(sel, dom).into_iter().map(|n| dom.path(n)).collect()
}

/// Runs `f`, failing past 5 s: a debug build, tests run in parallel. (Release-build times are
/// in the phase 2 Task 7 report.)
fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
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
    assert_eq!(select(&sel, &dom), all_p);
    for n in dom.descendants() {
        assert_eq!(matches(&sel, &dom, n), dom.tag(n) == "p");
    }
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
            Parsed::Ok { selector, .. } => timed(text, || select(&selector, &dom)).len(),
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
        timed(text, || select(&sel, &dom));
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
        assert_eq!(timed(text, || select(&sel, &dom)).len(), want, "{text}");
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
            assert_eq!(select(&sel, &dom).len(), 1);
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
    assert!(timed("256 parts", || select(&sel, &dom)).is_empty());
    let (sel, ..) = ok(&parts(200));
    assert_eq!(select(&sel, &dom).len(), 1);
    // Parts of one compound count as well: Calibre stops with a RecursionError past about
    // 400 of them (probed with calibre-debug).
    assert!(matches!(
        parse_selector(&format!("p{}", ".a".repeat(256))),
        Parsed::Dropped
    ));
    let (sel, ..) = ok(&format!("p{}", ".a".repeat(255)));
    let dom = page(&"<p class=\"a\">x</p>".repeat(50_000));
    assert_eq!(timed("255 classes", || select(&sel, &dom)).len(), 50_000);
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
        let got = timed(text, || select(&sel, &dom)).len();
        if text == ":lang(en)" {
            assert_eq!(got, 0, "no lang attribute");
        } else {
            assert!(got >= all - 1, "{text}: {got} of {all}");
        }
    }
}

#[test]
fn fifty_thousand_selectors_of_twenty_parts() {
    let dom = page("<div class=\"a\"><p id=\"x\">a</p><p>b</p></div>");
    let texts: Vec<String> = (0..50_000)
        .map(|i| {
            // 20 compounds.
            let parts: Vec<String> = (0..10)
                .map(|j| format!("div.c{}#i{j} > p[title^=t{i}]", i % 7))
                .collect();
            parts.join(" ")
        })
        .collect();
    timed("50,000 selectors", || {
        for text in &texts {
            let (sel, ..) = ok(text);
            assert!(select(&sel, &dom).is_empty());
        }
    });
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
        assert_eq!(timed(text, || select(&sel, &dom)).len(), want, "{text}");
    }
}
