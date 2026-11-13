// SPDX-License-Identifier: GPL-3.0-only
//! Stylesheets to rules (phase 2 plan Task 5): each spine file's stylesheets parsed as Calibre
//! 9.18's first-pass Stylizer holds them, against `cascade/<book>/rules/NNN.json` and
//! `cascade/ua_rules.json`, recorded inside a kfxgen run.
//!
//! Expected strings not read from a dump were read from css_parser 1.1.1 inside calibre 9.18
//! (`calibre-debug`), the parser Calibre runs.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use deckle_kfx::epub_zip::{EpubReader, join, percent_decode};
use deckle_kfx::fixtures;
use deckle_kfxgen::css::{
    DEFAULT_PROFILE, Decl, Decls, Error, HTML_CSS, Origin, Sheet, media_ok, parse_sheet,
    parse_style_attr, serialize_value,
};
use deckle_kfxgen::dom;
use deckle_kfxgen::epub::read_package;
use deckle_limits::{MAX_STYLE_DECLS, MAX_STYLE_RULES};
use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};

use crate::support::{self, BOOKS, dir_of};

/// A JSON object read in its written order (serde_json's `Map` sorts its keys).
#[derive(Debug, PartialEq)]
struct Ordered(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object of strings")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = map.next_entry::<String, String>()? {
                    out.push(entry);
                }
                Ok(Ordered(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// `[origin, selector, declarations, important, key]`.
type DumpRule = (u8, String, Ordered, Vec<String>, Vec<i64>);

#[derive(Deserialize)]
struct RulesDump {
    rules: Vec<DumpRule>,
    /// Not in `ua_rules.json`.
    #[serde(default)]
    font_face: Vec<Ordered>,
}

/// A rule as compared: origin, selector, declarations in order, sorted `!important` names.
#[derive(Debug, Clone, PartialEq)]
struct Flat {
    origin: u8,
    selector: String,
    decls: Vec<(String, String)>,
    important: Vec<String>,
}

fn flat(origin: Origin, selector: &str, decls: &Decls) -> Flat {
    let mut important: Vec<String> = decls
        .iter()
        .filter(|(_, d)| d.important)
        .map(|(n, _)| n.clone())
        .collect();
    important.sort();
    Flat {
        origin: match origin {
            Origin::UserAgent => 0,
            Origin::Book => 1,
        },
        selector: selector.to_owned(),
        decls: decls
            .iter()
            .map(|(n, d)| (n.clone(), d.value.clone()))
            .collect(),
        important,
    }
}

/// The dump's rules in source order: sorted by `key`'s last item, Calibre's rule index. The
/// selectors of one list share an index and come in specificity order, so within an index
/// they are sorted by text, on both sides.
fn dump_in_source_order(rules: &[DumpRule]) -> Vec<Flat> {
    let mut by_index: BTreeMap<i64, Vec<Flat>> = BTreeMap::new();
    for (origin, selector, decls, important, key) in rules {
        by_index
            .entry(*key.last().unwrap())
            .or_default()
            .push(Flat {
                origin: *origin,
                selector: selector.clone(),
                decls: decls.0.clone(),
                important: important.clone(),
            });
    }
    by_index
        .into_values()
        .flat_map(|mut group| {
            group.sort_by(|a, b| a.selector.cmp(&b.selector));
            group
        })
        .collect()
}

/// The port's rules of `sheets`, normalized as the dump's are (`flatten_style`, Task 6), in
/// the order of their index, which runs on across the sheets; each selector list's rules
/// sorted by text.
fn port_in_source_order(sheets: &mut [Sheet]) -> Vec<Flat> {
    let mut by_index: BTreeMap<u32, Vec<Flat>> = BTreeMap::new();
    for sheet in sheets {
        sheet.normalize(&DEFAULT_PROFILE);
        for r in &sheet.rules {
            by_index
                .entry(r.index)
                .or_default()
                .push(flat(r.origin, &r.selector, &r.decls));
        }
    }
    by_index
        .into_values()
        .flat_map(|mut group| {
            group.sort_by(|a, b| a.selector.cmp(&b.selector));
            group
        })
        .collect()
}

fn first_difference(got: &[Flat], want: &[Flat]) -> String {
    let i = got.iter().zip(want).take_while(|(g, w)| g == w).count();
    format!(
        "rule {i}: got {:?}, want {:?} ({} and {} rules)",
        got.get(i),
        want.get(i),
        got.len(),
        want.len()
    )
}

/// A spine file's stylesheets in Calibre's order ([`support::file_sheets`]) and their
/// `@font-face` rules.
fn spine_file_sheets(
    zip: &mut EpubReader,
    css_items: &[String],
    href: &str,
) -> (Vec<Sheet>, Vec<Decls>) {
    let bytes = zip
        .read_bytes(href)
        .or_else(|_| zip.read_bytes(&percent_decode(href)))
        .unwrap();
    let dom = dom::parse(&bytes).unwrap();
    let sheets = support::file_sheets(zip, css_items, href, &dom);
    let font_face = support::font_faces(&sheets);
    (sheets, font_face)
}

#[test]
fn book_rules_match_calibre_in_text_and_order() {
    let ua_dump: RulesDump =
        serde_json::from_slice(&std::fs::read(fixtures::kfxgen("cascade/ua_rules.json")).unwrap())
            .unwrap();
    let ua = parse_sheet(HTML_CSS, Origin::UserAgent, "", 0, &mut |_| None).unwrap();
    let ua_got = port_in_source_order(&mut [ua]);
    let ua_want = dump_in_source_order(&ua_dump.rules);
    assert_eq!(
        ua_got,
        ua_want,
        "html.css: {}",
        first_difference(&ua_got, &ua_want)
    );

    let mut matched = 0;
    let mut total = 0;
    let mut rules_compared = 0;
    let mut faces_compared = 0;
    let mut failed = Vec::new();
    for book in BOOKS {
        let dir = fixtures::kfxgen(&format!("cascade/{book}"));
        let files: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(dir.join("files.json")).unwrap()).unwrap();
        let path = fixtures::kfxgen(&format!("sources/{book}.epub"));
        let mut zip = EpubReader::open(&path).unwrap();
        let pkg = read_package(&mut zip).unwrap();
        let opf_dir = dir_of(&pkg.opf_path).to_owned();
        let css_items: Vec<String> = pkg
            .manifest
            .iter()
            .filter(|i| i.media_type == "text/css")
            .map(|i| join(&opf_dir, &percent_decode(&i.href)))
            .collect();
        for (href, n) in &files {
            total += 1;
            let dump: RulesDump = serde_json::from_slice(
                &std::fs::read(dir.join(format!("rules/{n}.json"))).unwrap(),
            )
            .unwrap();
            let (mut sheets, font_face) = spine_file_sheets(&mut zip, &css_items, href);
            let got = port_in_source_order(&mut sheets);
            let want = dump_in_source_order(&dump.rules);
            let got_ff: Vec<Vec<(String, String)>> = font_face
                .iter()
                .map(|d| {
                    d.iter()
                        .map(|(k, v)| (k.clone(), v.value.clone()))
                        .collect()
                })
                .collect();
            let want_ff: Vec<Vec<(String, String)>> =
                dump.font_face.into_iter().map(|o| o.0).collect();
            rules_compared += want.len();
            faces_compared += want_ff.len();
            if got == want && got_ff == want_ff {
                matched += 1;
            } else if got != want {
                failed.push(format!("{book} {href}: {}", first_difference(&got, &want)));
            } else {
                failed.push(format!(
                    "{book} {href}: font faces {got_ff:?}, want {want_ff:?}"
                ));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{matched} of {total} files match; differences:\n{}",
        failed.join("\n")
    );
    // Every spine file of the 13 books, with their 306 rules and 3 font faces.
    assert_eq!((matched, total), (49, 49));
    assert_eq!((rules_compared, faces_compared), (306, 3));
}

/// `rules/001.json` of `css_quirks` (`sheet.xhtml`) without the shorthands Task 6 expands:
/// imports first, `@media` flattened, `mso-` dropped and `panose-1` kept, the values as
/// css_parser writes them, the selector list split, one `@font-face` of two.
#[test]
fn sheet_quirks_match_calibre() {
    let path = fixtures::kfxgen("sources/css_quirks.epub");
    let mut zip = EpubReader::open(&path).unwrap();
    let css: Vec<String> = ["sheet", "imported", "deep", "cycle_a", "cycle_b"]
        .iter()
        .map(|n| format!("OEBPS/{n}.css"))
        .collect();
    let (sheets, font_face) = spine_file_sheets(&mut zip, &css, "OEBPS/sheet.xhtml");
    let got: Vec<(String, Vec<(String, String)>)> = sheets
        .iter()
        .flat_map(|s| &s.rules)
        .map(|r| {
            (
                r.selector.clone(),
                r.decls
                    .iter()
                    .map(|(k, d)| (k.clone(), d.value.clone()))
                    .collect(),
            )
        })
        .collect();
    let d = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    let want = vec![
        ("p.deep".to_owned(), d(&[("orphans", "11")])),
        ("p.imp".to_owned(), d(&[("orphans", "10")])),
        ("p.sc".to_owned(), d(&[("orphans", "3")])),
        ("p.kf".to_owned(), d(&[("orphans", "4")])),
        ("p.or".to_owned(), d(&[("orphans", "6")])),
        ("p.au".to_owned(), d(&[("orphans", "7")])),
        (
            "p.mso".to_owned(),
            d(&[("panose-1", "2 2 6 3"), ("color", "red")]),
        ),
        (
            "p.v1".to_owned(),
            d(&[
                ("margin-left", "0.5em"),
                ("margin-right", "0"),
                ("color", "RED"),
                ("text-align", "Center"),
            ]),
        ),
        (
            "p.v2".to_owned(),
            d(&[
                ("font-family", "\"Times New Roman\", serif"),
                ("width", "10px"),
                ("height", "1em"),
                ("widows", "2"),
            ]),
        ),
        ("p.v3".to_owned(), d(&[("orphans", "8")])),
        (
            "p.v4".to_owned(),
            d(&[("margin-left", "foo"), ("height", "10 px")]),
        ),
        (
            "p.v5".to_owned(),
            d(&[
                ("margin-top", "-0.25em"),
                ("text-indent", "+1.5em"),
                ("line-height", "1.2"),
            ]),
        ),
        (
            "p.v6".to_owned(),
            d(&[
                ("color", "#ABC"),
                ("background-color", "rgb(10, 20, 30)"),
                ("quotes", "\"\u{201c}\" \"\u{201d}\""),
            ]),
        ),
        ("p.v7".to_owned(), d(&[("margin-left", "2em")])),
        ("h4".to_owned(), d(&[("orphans", "9")])),
        ("h5.x".to_owned(), d(&[("orphans", "9")])),
        ("p.st".to_owned(), d(&[("orphans", "12")])),
        ("p.ca".to_owned(), d(&[("orphans", "13")])),
    ];
    assert_eq!(got, want);
    // `h4, h5.x` is one rule: its two selectors share an index.
    let rules: Vec<_> = sheets.iter().flat_map(|s| &s.rules).collect();
    assert_eq!(rules[14].index, rules[15].index);
    assert_ne!(rules[13].index, rules[14].index);
    let ff: Vec<Vec<(String, String)>> = font_face
        .iter()
        .map(|d| {
            d.iter()
                .map(|(k, v)| (k.clone(), v.value.clone()))
                .collect()
        })
        .collect();
    assert_eq!(
        ff,
        vec![d(&[("font-family", "\"Two\""), ("font-weight", "bold")])]
    );
    // The `<style>` element's `@page` is kept for the caller; sheet.css's is dropped.
    assert_eq!(sheets[1].page.len(), 1);
}

#[test]
fn serialize_value_cases() {
    let cases = [
        (".5em", "0.5em"),
        ("0pt", "0"),
        ("0.0EM", "0"),
        ("-.25EM", "-0.25em"),
        ("+1.50em", "+1.5em"),
        ("1.20", "1.2"),
        ("10PX", "10px"),
        ("100.0%", "100%"),
        ("0%", "0%"),
        ("0rem", "0rem"),
        ("-0.0%", "0%"),
        ("007px", "7px"),
        ("1.0000001", "1.0"),
        ("0.1234567891234", "0.123457"),
        ("99999.9999999", "100000.0"),
        ("12345678901234567890", "12345678901234567890"),
        ("1E3px", "1e3px"),
        ("1e999", "1e999"),
        ("RED", "RED"),
        ("Center", "Center"),
        ("#ABC", "#ABC"),
        ("#aabbcc", "#abc"),
        ("#AaBbCc", "#AaBbCc"),
        ("rgb(10,20,30)", "rgb(10, 20, 30)"),
        ("RGB( 10 , 20 , 30 )", "rgb(10, 20, 30)"),
        ("'Times New Roman', serif", "\"Times New Roman\", serif"),
        ("Arial ,Helvetica", "Arial, Helvetica"),
        ("\"\\201C\" '\\201D'", "\"\u{201c}\" \"\u{201d}\""),
        ("'a\"b'", "\"a\\\"b\""),
        ("\"a\\\"b\"", "\"a\\\"b\""),
        ("'\\''", "\"'\""),
        ("\"\\A\"", "\"\\a \""),
        ("url(pic.png)", "url(pic.png)"),
        ("url( \"pic.png\" )", "url(pic.png)"),
        ("url('a b.png')", "url(\"a b.png\")"),
        (
            "url(data:image/png;base64,iVBOR=)",
            "url(\"data:image/png;base64,iVBOR=\")",
        ),
        ("URL(a.png)", "url(a.png)"),
        ("12px / 30px Georgia", "12px/30px Georgia"),
        ("1em  2em", "1em 2em"),
        ("url(x.png)no-repeat", "url(x.png) no-repeat"),
        ("#fff#000", "#fff #000"),
        ("a /* c */ b", "a b"),
        ("\\41 bc", "Abc"),
        ("a\\:b", "a\\:b"),
        ("CALC( 100%  -  2em )", "CALC(100% - 2em)"),
        ("calc(1px*2)", "calc(1px * 2)"),
        // `calc()`: `+` and `-` need white space on both sides; numbers and `var()` only.
        ("CALC( 1px + 2px )", "CALC(1px + 2px)"),
        ("calc(1px - -2px)", "calc(1px - -2px)"),
        ("calc(-2px + 1px)", "calc(-2px + 1px)"),
        ("calc(1px + +2px)", "calc(1px + +2px)"),
        ("calc(1px*-2)", "calc(1px * -2)"),
        ("calc(1px *2)", "calc(1px * 2)"),
        ("calc(1px\t+\t2px)", "calc(1px + 2px)"),
        ("calc(VAR(x) + 1px)", "calc(var(x) + 1px)"),
        ("calc( -1px )", "calc(-1px)"),
        ("calc(0px + .5em)", "calc(0 + 0.5em)"),
        // A string's line continuations are dropped after its escapes are resolved: `\\a`
        // (a backslash, then an escaped line feed) is nothing, `\` and a line feed then `31`
        // is `31`.
        ("\"a\\\nb\"", "\"ab\""),
        ("'a\\\nb'", "\"ab\""),
        ("\"a\\\r\nb\"", "\"ab\""),
        ("\"a\\\u{c}b\"", "\"ab\""),
        ("\"a\\\\\\\nb\"", "\"a\\\\b\""),
        ("\"\\\\a\"", "\"\""),
        ("\"\\\n31\"", "\"31\""),
        // `url(` and more than one item: a function css_parser writes as it is.
        ("url('a''b')", "url(\"a\" \"b\")"),
        ("URL('a' b)", "url(\"a\" b)"),
        ("url('a', 'b')", "url(\"a\", \"b\")"),
        ("url('a'1px)", "url(\"a\" 1px)"),
        ("F(a)", "f(a)"),
        ("counter(a,upper-roman)", "counter(a, upper-roman)"),
        ("alpha(opacity=50)", "alpha(opacity=50)"),
        ("U+0025-00FF", "u+0025-00ff"),
        ("10 px", "10 px"),
        ("foo", "foo"),
        // Dropped: empty, or not a value css_parser reads.
        ("", ""),
        ("   ", ""),
        ("50.%", ""),
        ("#", ""),
        ("#abcd", ""),
        ("@foo", ""),
        ("[a]", ""),
        ("a + b", ""),
        ("a,", ""),
        ("--x", ""),
        ("var(--x)", ""),
        ("f(a=b)", ""),
        ("rgb(a, b, c)", ""),
        ("'unterminated", ""),
        ("url(a'b)", ""),
        ("calc(1px+2px)", ""),
        ("calc(1px -2px)", ""),
        ("calc(1px +2px)", ""),
        ("calc(1px+ 2px)", ""),
        ("calc(50% -1px)", ""),
        ("calc(1px 2px)", ""),
        ("calc(1px +)", ""),
        ("calc(+ 1px)", ""),
        ("calc(1px * * 2)", ""),
        ("calc((1px + 2px) * 2)", ""),
        ("calc(1px + calc(2px))", ""),
        ("calc(a + b)", ""),
        ("calc(\"a\" + 1px)", ""),
        ("calc(foo(x) + 1px)", ""),
        ("calc(var(x)+1px)", ""),
        ("calc(1px, 2px)", ""),
        ("calc()", ""),
        ("url('a' + 'b')", ""),
        ("url('a'/'b')", ""),
        ("url(\"a\" , )", ""),
    ];
    let mut wrong = Vec::new();
    for (input, want) in cases {
        let got = serialize_value(input);
        if got != want {
            wrong.push(format!("{input:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn declarations_as_css_parser_lists_them() {
    let decls = |s: &str| -> Vec<(String, String, bool)> {
        parse_style_attr(s)
            .into_iter()
            .map(|(k, d)| (k, d.value, d.important))
            .collect()
    };
    let t = |k: &str, v: &str, i: bool| (k.to_owned(), v.to_owned(), i);
    // A name is listed where it was last declared; the last !important value wins, else the
    // last value.
    assert_eq!(
        decls("margin-left: .5em; COLOR: red; margin-left: 3em"),
        vec![t("color", "red", false), t("margin-left", "3em", false)]
    );
    assert_eq!(
        decls("color: red !important; color: blue; width: 1px ! IMPORTANT"),
        vec![t("color", "red", true), t("width", "1px", true)]
    );
    // `mso-` declarations go; so do custom properties and names that are not identifiers.
    assert_eq!(
        decls("mso-foo: 1; --x: 1; *zoom: 1; _height: 1px; color: "),
        vec![t("_height", "1px", false)]
    );
    // A priority other than important is not important; the value before the `!` is kept.
    assert_eq!(decls("x: 1 !foo"), vec![t("x", "1", false)]);
    for junk in [
        "!important!important",
        "!important !important",
        "!",
        "!1",
        "!important x",
        "!foo !important",
        "!(x)",
    ] {
        assert_eq!(
            decls(&format!("color:red {junk}")),
            vec![t("color", "red", false)],
            "{junk}"
        );
    }
    assert_eq!(
        decls("color:red ! important"),
        vec![t("color", "red", true)]
    );
    assert_eq!(decls("color: !important"), vec![]);
    // css_parser drops `calc(1px+2px)`, so the first width stays.
    assert_eq!(
        decls("width:CALC( 1px + 2px ); width:calc(1px+2px)"),
        vec![t("width", "CALC(1px + 2px)", false)]
    );
}

#[test]
fn selectors_as_css_parser_writes_them() {
    let cases: &[(&str, &[&str])] = &[
        ("h3+p, h3>p,h3~p", &["h3 + p", "h3 > p", "h3 ~ p"]),
        ("a  >  b", &["a > b"]),
        ("a\tb", &["a b"]),
        ("[ a = b ]", &["[a=b]"]),
        ("[a='b']", &["[a=\"b\"]"]),
        ("[|a]", &["[a]"]),
        ("[*|a]", &["[*|a]"]),
        ("P#X.y:HOVER::BEFORE", &["P#X.y:hover::before"]),
        ("::first-letter", &["::first-letter"]),
        (":is(p ,li)", &[":is(p, li)"]),
        (":not( p )", &[":not(p)"]),
        (":has(>em)", &[":has(> em)"]),
        (":has(a+b)", &[":has(a + b)"]),
        (":NTH-CHILD(odd)", &[":nth-child(odd)"]),
        (":nth-child(  2n  +  1  )", &[":nth-child( 2n + 1)"]),
        (":nth-child(2N+1)", &[":nth-child(2N+1)"]),
        (":lang('fr')", &[":lang(\"fr\")"]),
        (".\\31 x", &[".1x"]),
        (".a\\.b", &[".a\\.b"]),
        ("*|*", &["*|*"]),
        ("|p", &["|p"]),
        ("::before p", &["::before p"]),
        // A list with one bad selector is dropped whole.
        ("p, [title=\"X\" i]", &[]),
        ("li:nth-child(2 of .k)", &[]),
        (":lang(fr, de)", &[]),
        ("[a=1]", &[]),
        ("ns|p", &[]),
        ("[ns|a]", &[]),
        ("p::before:hover", &[]),
        ("> p", &[]),
        ("p >", &[]),
        ("p > > a", &[]),
        ("a,", &[]),
        (",a", &[]),
        (":foo()", &[]),
        (":not()", &[]),
        // css_parser reads no selector list in these: an expression of identifiers only.
        ("p:matches(.a)", &[]),
        ("p:any(.a)", &[]),
        ("p:-moz-any(.a)", &[]),
        ("p:-webkit-any(.a)", &[]),
        ("p:any(a, b)", &[]),
        ("p:MATCHES( a  b )", &["p:matches( a b)"]),
        ("p:-webkit-any(a)", &["p:-webkit-any(a)"]),
    ];
    let mut wrong = Vec::new();
    for (selector, want) in cases {
        let sheet = parse_sheet(
            &format!("{selector} {{ a: 1 }}"),
            Origin::Book,
            "",
            0,
            &mut |_| None,
        )
        .unwrap();
        let got: Vec<&str> = sheet.rules.iter().map(|r| r.selector.as_str()).collect();
        if got != *want {
            wrong.push(format!("{selector:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // A declared namespace prefix is kept.
    let sheet = parse_sheet(
        "@namespace epub \"http://www.idpf.org/2007/ops\";\n[epub|type~=footnote] { a: 1 }",
        Origin::Book,
        "",
        0,
        &mut |_| None,
    )
    .unwrap();
    assert_eq!(sheet.rules[0].selector, "[epub|type~=footnote]");
}

#[test]
fn media_queries() {
    assert!(media_ok("screen"));
    assert!(!media_ok("print"));
    assert!(!media_ok("screen and (min-width: 600px)"));
    assert!(media_ok("amzn-kf8"));
    // Calibre's own `test_media_ok`.
    assert!(media_ok(""));
    assert!(!media_ok("amzn-mobi"));
    assert!(media_ok("only screen"));
    assert!(!media_ok("not screen"));
    assert!(!media_ok("(device-width:10px)"));
    assert!(media_ok("screen, (device-width:10px)"));
    assert!(!media_ok("screen and (device-width:10px)"));
    // The media type is compared as written; features other than sizes, colour and
    // resolution pass.
    assert!(!media_ok("SCREEN"));
    assert!(media_ok("all and (orientation: portrait)"));
    assert!(!media_ok("screen and (min-aspect-ratio: 16/9)"));
    assert!(media_ok("not all and (monochrome)"));
    assert!(media_ok("(MIN-WIDTH: 10px)"));
    // A malformed query is read as `not all`.
    assert!(!media_ok("garbage!!"));
    // A query that leaves a bracket or string open swallows the `{}` Calibre parses it with,
    // so tinycss finds no rule and `media_ok` returns true.
    for open in [
        "(",
        "screen and (",
        "all and (color",
        "[",
        "f(",
        "\"",
        "screen /*",
    ] {
        assert!(media_ok(open), "{open}");
    }
}

#[test]
fn media_rules_are_flattened_one_level() {
    let css = "@media print { p.pr { a: 1 } }\n\
               @media screen { p.sc { a: 2 } @media all { p.nested { a: 3 } } @page { margin: 1em } }\n\
               @media screen and { p.bad { a: 4 } }\n\
               @media SCREEN { p.upper { a: 5 } }\n\
               @media not print { p.np { a: 6 } }\n\
               @media { p.empty { a: 7 } }\n\
               @font-face { font-family: a; font-family: b }\n\
               @font-face { src: url(x) !important; font-family: a }\n\
               p.z { a: 8 }";
    let sheet = parse_sheet(css, Origin::Book, "", 0, &mut |_| None).unwrap();
    let sel: Vec<&str> = sheet.rules.iter().map(|r| r.selector.as_str()).collect();
    assert_eq!(sel, ["p.sc", "p.np", "p.z"]);
    assert_eq!(sheet.page.len(), 1);
    // One property named twice is one declaration: that @font-face is dropped.
    assert_eq!(sheet.font_face.len(), 1);
    let ff: Vec<(&str, &str)> = sheet.font_face[0]
        .iter()
        .map(|(k, d)| (k.as_str(), d.value.as_str()))
        .collect();
    assert_eq!(ff, [("src", "url(x)"), ("font-family", "a")]);
}

#[test]
fn imports_come_first_with_their_urls_rebased() {
    let files: BTreeMap<&str, &str> = [
        (
            "sub/a.css",
            "@import \"b.css\";\np.a { background: url(pic.png) }\n@page { margin: 2em }",
        ),
        ("sub/b.css", "p.b { background: url(../up.png) }"),
        ("print.css", "p.print { a: 1 }"),
    ]
    .into_iter()
    .collect();
    let mut asked = Vec::new();
    let mut import = |h: &str| {
        asked.push(h.to_owned());
        files.get(h).map(|s| (*s).to_owned())
    };
    let css = "@import url(sub/a.css);\n@import 'print.css' print;\n@import \"missing.css\";\n\
               p.own { background: url(own.png) }\n@import \"late.css\";";
    let sheet = parse_sheet(css, Origin::Book, "OEBPS/css/top.css", 0, &mut import).unwrap();
    assert_eq!(asked, ["sub/a.css", "sub/b.css", "missing.css"]);
    let got: Vec<(&str, &str)> = sheet
        .rules
        .iter()
        .map(|r| (r.selector.as_str(), r.decls[0].1.value.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("p.b", "url(OEBPS/css/up.png)"),
            ("p.a", "url(OEBPS/css/sub/pic.png)"),
            ("p.own", "url(OEBPS/css/own.png)"),
        ]
    );
    // An imported sheet's @page is dropped, as Calibre drops it from every linked sheet.
    assert!(sheet.page.is_empty());
    assert!(sheet.rules.windows(2).all(|w| w[0].index < w[1].index));
}

#[test]
fn import_cycle_ends() {
    let files: BTreeMap<&str, &str> = [
        (
            "cycle_a.css",
            "@import \"cycle_b.css\";\np.ca { orphans: 13 }",
        ),
        (
            "cycle_b.css",
            "@import \"cycle_a.css\";\np.cb { orphans: 14 }",
        ),
        ("self.css", "@import \"self.css\";\np.self { a: 1 }"),
        ("ok.css", "p.ok { a: 2 }"),
    ]
    .into_iter()
    .collect();
    let mut import = |h: &str| files.get(h).map(|s| (*s).to_owned());
    // cycle_a.css linked: cycle_b's import of it closes the loop, and cycle_b is lost, as
    // `css_quirks`'s sheet.xhtml shows (rules/001.json: p.ca, no p.cb).
    let sheet = parse_sheet(files["cycle_a.css"], Origin::Book, "", 0, &mut import).unwrap();
    let sel: Vec<&str> = sheet.rules.iter().map(|r| r.selector.as_str()).collect();
    assert_eq!(sel, ["p.ca"]);
    // An import whose chain loops is dropped whole; the others stay.
    let sheet = parse_sheet(
        "@import \"ok.css\";\n@import \"cycle_a.css\";\n@import \"self.css\";\np.top { a: 3 }",
        Origin::Book,
        "",
        0,
        &mut import,
    )
    .unwrap();
    let sel: Vec<&str> = sheet.rules.iter().map(|r| r.selector.as_str()).collect();
    assert_eq!(sel, ["p.ok", "p.top"]);
}

#[test]
fn import_chain_ends_past_eight_levels() {
    // l0 imports l1, …, l11 imports nothing: levels past 8 are dropped.
    let mut import = |h: &str| {
        let n: u32 = h
            .trim_start_matches('l')
            .trim_end_matches(".css")
            .parse()
            .ok()?;
        Some(if n < 11 {
            format!("@import \"l{}.css\";\np.l{n} {{ a: 1 }}", n + 1)
        } else {
            format!("p.l{n} {{ a: 1 }}")
        })
    };
    let sheet = parse_sheet(
        "@import \"l1.css\";\np.top { a: 1 }",
        Origin::Book,
        "",
        0,
        &mut import,
    )
    .unwrap();
    let sel: Vec<&str> = sheet.rules.iter().map(|r| r.selector.as_str()).collect();
    assert_eq!(
        sel,
        [
            "p.l8", "p.l7", "p.l6", "p.l5", "p.l4", "p.l3", "p.l2", "p.l1", "p.top"
        ]
    );
}

#[test]
fn fifty_thousand_and_one_rules_is_too_large() {
    let at_limit = "p{a:1}\n".repeat(MAX_STYLE_RULES);
    let sheet = parse_sheet(&at_limit, Origin::Book, "", 0, &mut |_| None).unwrap();
    assert_eq!(sheet.rules.len(), MAX_STYLE_RULES);
    let over = format!("{at_limit}p{{a:1}}\n");
    assert!(matches!(
        parse_sheet(&over, Origin::Book, "", 0, &mut |_| None),
        Err(Error::TooLarge(_))
    ));
}

/// Runs `f`, failing past `COST_LIMIT`. (Release-build times of
/// these cases are in the phase 2 Task 5 report.)
fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    assert!(took < crate::support::COST_LIMIT, "{what} took {took:?}");
    out
}

#[test]
fn a_sheet_imported_under_many_spellings_is_read_once() {
    // A 2 MB sheet of `@page` declarations imported under 300 spellings of one address
    // (`p.css`, `./p.css`, `././p.css`…): read once, and refused as its declarations are
    // read, though an imported sheet's `@page` rules are dropped.
    let big: String = (0..200_000).map(|i| format!("a{i}:1;")).collect();
    let big = format!("@page{{{big}}}");
    let top: String = (0..300)
        .map(|k| format!("@import '{}p.css';", "./".repeat(k)))
        .collect();
    let mut asked = Vec::new();
    // Each spelling names the sheet, as a resolver of addresses finds it.
    let mut import = |h: &str| {
        asked.push(h.to_owned());
        h.ends_with("p.css").then(|| big.clone())
    };
    let got = timed("a 2 MB sheet under 300 names", || {
        parse_sheet(&top, Origin::Book, "", 0, &mut import)
    });
    assert!(matches!(got, Err(Error::TooLarge(MAX_STYLE_DECLS))));
    assert_eq!(asked, ["p.css"]);

    // A small sheet under 300 spellings, `a/../` among them: one sheet, asked for once.
    let top: String = (0..300)
        .map(|k| format!("@import '{}p.css';@import 'a/../p.css';", "./".repeat(k)))
        .collect();
    let mut asked = Vec::new();
    let mut import = |h: &str| {
        asked.push(h.to_owned());
        h.ends_with("p.css")
            .then(|| "@page{margin:1em} p{a:1}".to_owned())
    };
    let sheet = parse_sheet(&top, Origin::Book, "", 0, &mut import).unwrap();
    assert_eq!(asked, ["p.css"]);
    assert!(sheet.page.is_empty());
    assert_eq!(sheet.rules.len(), 600);

    // 300 different 2 MB sheets of comments: each sheet's text is charged as it is loaded,
    // so the fifth is refused.
    let comment = format!("/*{}*/", "x".repeat(2 << 20));
    let top: String = (0..300).map(|k| format!("@import 'x{k}.css';")).collect();
    let mut asked = 0;
    let mut import = |_: &str| {
        asked += 1;
        Some(comment.clone())
    };
    let got = parse_sheet(&top, Origin::Book, "", 0, &mut import);
    assert!(matches!(got, Err(Error::TooLarge(MAX_STYLE_DECLS))));
    assert_eq!(asked, 5);
}

#[test]
fn declarations_past_the_limit_are_refused() {
    // 125,000 short declarations and their 1.2 MB of text fit in MAX_STYLE_DECLS; 150,000
    // do not.
    let decls = |n: usize| -> String { (0..n).map(|i| format!("a{i}:1;")).collect() };
    let sheet = parse_sheet(
        &format!("p{{{}}}", decls(125_000)),
        Origin::Book,
        "",
        0,
        &mut |_| None,
    )
    .unwrap();
    assert_eq!(sheet.rules[0].decls.len(), 125_000);
    assert!(matches!(
        parse_sheet(
            &format!("p{{{}}}", decls(150_000)),
            Origin::Book,
            "",
            0,
            &mut |_| None
        ),
        Err(Error::TooLarge(MAX_STYLE_DECLS))
    ));
    // A shorthand counts once for each declaration it becomes: `border` 12.
    let border = |n: usize| format!("p{{{}}}", "border:0;".repeat(n));
    assert!(parse_sheet(&border(12_000), Origin::Book, "", 0, &mut |_| None).is_ok());
    assert!(matches!(
        parse_sheet(&border(12_600), Origin::Book, "", 0, &mut |_| None),
        Err(Error::TooLarge(MAX_STYLE_DECLS))
    ));
    // A stylesheet's text counts once for each 64 bytes, read or not.
    let comment = format!("/*{}*/", "x".repeat((MAX_STYLE_DECLS + 1) * 64));
    assert!(matches!(
        parse_sheet(&comment, Origin::Book, "", 0, &mut |_| None),
        Err(Error::TooLarge(MAX_STYLE_DECLS))
    ));
}

#[test]
fn selectors_of_a_list_share_one_block() {
    // 50,000 selectors sharing 19 declarations of 60 bytes: one block, normalized once.
    let list: Vec<String> = (0..50_000).map(|i| format!(".c{i}")).collect();
    let decls: String = (0..18)
        .map(|i| format!("x{i}:{};", "v".repeat(58)))
        .collect();
    let css = format!("{}{{margin:1px 2px;{decls}}}", list.join(","));
    let mut sheet = timed("50,000 selectors × 19 declarations", || {
        parse_sheet(&css, Origin::Book, "", 0, &mut |_| None).unwrap()
    });
    assert_eq!(sheet.rules.len(), 50_000);
    let shared = |sheet: &Sheet| {
        sheet
            .rules
            .iter()
            .all(|r| Arc::ptr_eq(&r.decls, &sheet.rules[0].decls))
    };
    assert!(shared(&sheet));
    sheet.normalize(&DEFAULT_PROFILE);
    assert!(shared(&sheet));
    assert_eq!(sheet.rules[0].decls[0].0, "margin-top");
    assert_eq!(sheet.rules[49_999].decls.len(), 18 + 4);
}

#[test]
fn rule_index_runs_on_across_sheets() {
    let a = parse_sheet(
        "p{a:1} q,r{b:2} @font-face{src:url(x);font-family:y}",
        Origin::Book,
        "",
        0,
        &mut |_| None,
    )
    .unwrap();
    let index = |s: &Sheet| s.rules.iter().map(|r| r.index).collect::<Vec<_>>();
    assert_eq!(index(&a), [0, 1, 1]);
    assert_eq!(a.next_index, 3);
    let b = parse_sheet(
        "@import 'i.css'; s{c:3}",
        Origin::Book,
        "",
        a.next_index,
        &mut |_| Some("t{d:4}".to_owned()),
    )
    .unwrap();
    assert_eq!(index(&b), [3, 4]);
    assert_eq!(b.next_index, 5);
}

/// `url(a\)b.png)` makes css_parser drop that declaration and the rest of the sheet; the port
/// reads on
/// (DIVERGENCES.md, "An escaped parenthesis in url()").
#[test]
fn an_escaped_parenthesis_in_a_url_keeps_the_sheet() {
    let sheet = parse_sheet(
        "p{background:url(a\\)b.png)} q{color:red}",
        Origin::Book,
        "",
        0,
        &mut |_| None,
    )
    .unwrap();
    let got: Vec<(&str, &str)> = sheet
        .rules
        .iter()
        .map(|r| (r.selector.as_str(), r.decls[0].1.value.as_str()))
        .collect();
    assert_eq!(got, [("p", "url(\"a\\)b.png\")"), ("q", "red")]);
}

#[test]
fn hostile_sheets_finish_or_are_refused() {
    // 5 MB of one-declaration rules: far past the rule limit, refused.
    let many = "p.x{color:red}\n".repeat(5 * 1024 * 1024 / 15);
    timed("5 MB of rules", || {
        assert!(matches!(
            parse_sheet(&many, Origin::Book, "", 0, &mut |_| None),
            Err(Error::TooLarge(_))
        ));
    });
    // One rule, 100,000 declarations of one property: the last wins, in linear time.
    let decls: String = (0..100_000)
        .map(|i| format!("margin-left:{i}px;"))
        .collect();
    let sheet = timed("100,000 declarations", || {
        parse_sheet(&format!("p{{{decls}}}"), Origin::Book, "", 0, &mut |_| None).unwrap()
    });
    assert_eq!(
        *sheet.rules[0].decls,
        vec![(
            "margin-left".to_owned(),
            Decl {
                value: "99999px".to_owned(),
                important: false
            }
        )]
    );
    // 100,000 selectors in one list: refused.
    let list = vec!["p"; 100_000].join(",");
    timed("100,000 selectors", || {
        assert!(matches!(
            parse_sheet(&format!("{list}{{a:1}}"), Origin::Book, "", 0, &mut |_| {
                None
            }),
            Err(Error::TooLarge(_))
        ));
    });
    // 40,000 selectors sharing one 1 MB value: one copy, shared.
    let list = vec!["p"; 40_000].join(",");
    let value = "a".repeat(1 << 20);
    let sheet = timed("40,000 selectors × a 1 MB value", || {
        parse_sheet(
            &format!("{list}{{font-family:{value}}}"),
            Origin::Book,
            "",
            0,
            &mut |_| None,
        )
        .unwrap()
    });
    assert_eq!(sheet.rules.len(), 40_000);
    assert!(
        sheet
            .rules
            .iter()
            .all(|r| Arc::ptr_eq(&r.decls, &sheet.rules[0].decls))
    );
    // A 500 KB selector in a sheet imported 255 times over (a diamond, as below): 127 MB of
    // copies, refused (each copy is charged again; the limit allows about 9.6 MB of text).
    let long = format!("p.{}", "x".repeat(500_000));
    let mut import = |h: &str| {
        let n: u32 = h
            .trim_start_matches('d')
            .trim_end_matches(".css")
            .parse()
            .ok()?;
        Some(format!(
            "@import \"d{0}.css\";\n@import \"d{0}.css\";\n{long} {{ a: 1 }}",
            n + 1
        ))
    };
    timed("a long selector imported 255 times", || {
        assert!(matches!(
            parse_sheet("@import \"d1.css\";", Origin::Book, "", 0, &mut import),
            Err(Error::TooLarge(_))
        ));
    });
    // @media nested 10,000 deep: only the first level is read.
    let nested = format!(
        "{}p.deep{{a:1}}{}p.after{{a:2}}",
        "@media screen{".repeat(10_000),
        "}".repeat(10_000)
    );
    let sheet = timed("@media 10,000 deep", || {
        parse_sheet(&nested, Origin::Book, "", 0, &mut |_| None).unwrap()
    });
    let sel: Vec<&str> = sheet.rules.iter().map(|r| r.selector.as_str()).collect();
    assert_eq!(sel, ["p.after"]);
    // :not() nested 10,000 deep: the selector is dropped, nothing overflows.
    let deep = format!(
        "{}p{}{{a:1}} q{{a:2}}",
        ":not(".repeat(10_000),
        ")".repeat(10_000)
    );
    let sheet = timed(":not() 10,000 deep", || {
        parse_sheet(&deep, Origin::Book, "", 0, &mut |_| None).unwrap()
    });
    assert_eq!(sheet.rules.len(), 1);
    // A function nested 10,000 deep in a value: the declaration is dropped.
    let deep = format!("p{{a:{}1{};b:2}}", "f(".repeat(10_000), ")".repeat(10_000));
    let sheet = timed("value 10,000 deep", || {
        parse_sheet(&deep, Origin::Book, "", 0, &mut |_| None).unwrap()
    });
    let names: Vec<&str> = sheet.rules[0]
        .decls
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(names, ["b"]);
    // A 1 MB value.
    let long = format!("p{{font-family:{}}}", "a, ".repeat(350_000) + "b");
    let sheet = timed("1 MB value", || {
        parse_sheet(&long, Origin::Book, "", 0, &mut |_| None).unwrap()
    });
    assert!(sheet.rules[0].decls[0].1.value.len() > 1_000_000);
    // A diamond of imports eight levels deep, each sheet importing the next twice: 2^8 visits
    // of one tiny sheet, each read once.
    let mut reads = 0;
    let mut import = |h: &str| {
        reads += 1;
        let n: u32 = h
            .trim_start_matches('d')
            .trim_end_matches(".css")
            .parse()
            .ok()?;
        Some(format!(
            "@import \"d{0}.css\";\n@import \"d{0}.css\";\np.d{n} {{ a: 1 }}",
            n + 1
        ))
    };
    let sheet = timed("import diamond", || {
        parse_sheet("@import \"d1.css\";", Origin::Book, "", 0, &mut import).unwrap()
    });
    assert_eq!(reads, 8);
    assert_eq!(sheet.rules.len(), (1 << 8) - 1);
    // A ring of five sheets, each importing the next: the chain loops and is dropped.
    let mut import = |h: &str| {
        let n: u32 = h
            .trim_start_matches('r')
            .trim_end_matches(".css")
            .parse()
            .ok()?;
        Some(format!(
            "@import \"r{}.css\";\np.r{n} {{ a: 1 }}",
            (n + 1) % 5
        ))
    };
    let sheet = timed("import ring", || {
        parse_sheet(
            "@import \"r0.css\";\np.top{a:1}",
            Origin::Book,
            "",
            0,
            &mut import,
        )
        .unwrap()
    });
    assert_eq!(sheet.rules.len(), 1);
}

/// css_parser's own spacing rules for `*` and `/` in `calc()`, which the port does not follow
/// (DIVERGENCES.md, "CSS spellings css_parser reads its own way"): css_parser drops these.
#[test]
fn calc_spacing_css_parser_drops_is_kept() {
    for (input, port) in [
        ("calc(1px* 2)", "calc(1px * 2)"),
        ("calc(1px/ 2)", "calc(1px / 2)"),
        ("calc(1px/2/3)", "calc(1px / 2 / 3)"),
        ("calc(1px*2/3)", "calc(1px * 2 / 3)"),
    ] {
        assert_eq!(serialize_value(input), port, "{input}");
    }
}
