// SPDX-License-Identifier: GPL-3.0-only
//! Shorthands and font-size keywords (phase 2 plan Task 6): `css::normalize` against
//! `css_quirks`'s `shorthand.xhtml` (`cascade/css_quirks/rules/002.json`), the declarations
//! Calibre 9.18's `StylizerRules.flatten_style` made of `shorthand.css`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use deckle_kfx::epub_zip::EpubReader;
use deckle_kfx::fixtures;
use deckle_kfxgen::css::{
    DEFAULT_PROFILE, DEFAULTS, Decl, Decls, INHERITED, Origin, normalize, parse_sheet,
    parse_style_attr,
};
use serde_json::Value;

/// `decls` as `(name, value)` pairs, `!` before the name of an important one.
fn pairs(decls: &Decls) -> Vec<(String, String)> {
    decls
        .iter()
        .map(|(k, d)| {
            let k = if d.important {
                format!("!{k}")
            } else {
                k.clone()
            };
            (k, d.value.clone())
        })
        .collect()
}

fn norm(style: &str) -> Vec<(String, String)> {
    pairs(&normalize(parse_style_attr(style), &DEFAULT_PROFILE))
}

fn want(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// Every rule of `shorthand.css`, normalized, against the dump, in order: the margin,
/// padding, border, list-style and font shorthands, the font-size keywords, the shorthands
/// Calibre leaves alone and the `-epub-` properties.
#[test]
fn shorthands_expand_as_calibre() {
    let path = fixtures::kfxgen("sources/css_quirks.epub");
    let mut zip = EpubReader::open(&path).unwrap();
    let css = zip.read_string("OEBPS/shorthand.css").unwrap();
    let mut sheet = parse_sheet(&css, Origin::Book, &mut |_| None).unwrap();
    sheet.rebase_urls("OEBPS/shorthand.css");
    let dump: Value = serde_json::from_slice(
        &std::fs::read(fixtures::kfxgen("cascade/css_quirks/rules/002.json")).unwrap(),
    )
    .unwrap();
    // The dump's objects in written order: serde_json's Map sorts keys, so read the order
    // from the text of each rule.
    let text =
        std::fs::read_to_string(fixtures::kfxgen("cascade/css_quirks/rules/002.json")).unwrap();
    let mut wanted: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (rule, line) in dump["rules"]
        .as_array()
        .unwrap()
        .iter()
        .zip(text.lines().filter(|l| l.trim_start().starts_with("[1, ")))
    {
        let selector = rule[1].as_str().unwrap().to_owned();
        let decls = rule[2].as_object().unwrap();
        let important: Vec<&str> = rule[3]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let mut names: Vec<(usize, &String)> = decls
            .keys()
            .map(|k| (line.find(&format!("\"{k}\":")).unwrap(), k))
            .collect();
        names.sort();
        let ordered = names
            .into_iter()
            .map(|(_, k)| {
                let shown = if important.contains(&k.as_str()) {
                    format!("!{k}")
                } else {
                    k.clone()
                };
                (shown, decls[k].as_str().unwrap().to_owned())
            })
            .collect();
        wanted.insert(selector, ordered);
    }
    assert_eq!(wanted.len(), sheet.rules.len());
    let mut wrong = Vec::new();
    for rule in &sheet.rules {
        let got = pairs(&normalize(rule.decls.clone(), &DEFAULT_PROFILE));
        if Some(&got) != wanted.get(&rule.selector) {
            wrong.push(format!(
                "{}: got {got:?}, want {:?}",
                rule.selector,
                wanted.get(&rule.selector)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn font_size_large_is_1_1rem() {
    assert_eq!(norm("font-size: large"), want(&[("font-size", "1.1rem")]));
    assert_eq!(norm("font-size: small"), want(&[("font-size", "0.8rem")]));
    assert_eq!(norm("font-size: normal"), want(&[("font-size", "1.0rem")]));
    assert_eq!(
        norm("font-size: smallest"),
        want(&[("font-size", "0.4rem")])
    );
    assert_eq!(
        norm("font: bold x-large serif"),
        want(&[
            ("font-style", "normal"),
            ("font-variant", "normal"),
            ("font-weight", "bold"),
            ("font-size", "1.4rem"),
            ("line-height", "normal"),
            ("font-family", "serif"),
        ])
    );
}

#[test]
fn font_size_upper_case_keyword_is_left_alone() {
    // Review Focus 5: Calibre's keyword test is case-sensitive, and an invalid value stays.
    assert_eq!(norm("font-size: LARGE"), want(&[("font-size", "LARGE")]));
    assert_eq!(norm("font-size: SMALL"), want(&[("font-size", "SMALL")]));
    assert_eq!(
        norm("margin-left: foo; height: 10 px"),
        want(&[("margin-left", "foo"), ("height", "10 px")])
    );
}

#[test]
fn background_is_not_expanded() {
    assert_eq!(
        norm("background: #eee url(pic.png) no-repeat"),
        want(&[("background", "#eee url(pic.png) no-repeat")])
    );
    assert_eq!(
        norm("text-decoration: underline overline; outline: 1px solid red"),
        want(&[
            ("text-decoration", "underline overline"),
            ("outline", "1px solid red"),
        ])
    );
}

#[test]
fn epub_prefix_is_copied() {
    // `-epub-writing-mode` becomes `-webkit-writing-mode` and `writing-mode`, after the
    // rest; `-epub-hyphens` is not one Calibre copies.
    assert_eq!(
        norm("-epub-hyphens: auto; -epub-writing-mode: vertical-rl"),
        want(&[
            ("-epub-hyphens", "auto"),
            ("-webkit-writing-mode", "vertical-rl"),
            ("writing-mode", "vertical-rl"),
        ])
    );
    // A copy does not replace a property already set.
    assert_eq!(
        norm("writing-mode: horizontal-tb; -epub-text-emphasis-style: dot"),
        want(&[
            ("writing-mode", "horizontal-tb"),
            ("-webkit-text-emphasis-style", "dot"),
            ("text-emphasis-style", "dot"),
        ])
    );
}

#[test]
fn important_marks_each_part_and_stays() {
    assert_eq!(
        norm("margin: 1em 2em !important"),
        want(&[
            ("!margin-top", "1em"),
            ("!margin-right", "2em"),
            ("!margin-bottom", "1em"),
            ("!margin-left", "2em"),
        ])
    );
    // A later plain margin-left replaces the value; Calibre's set of important names keeps
    // it (`style_map.important_properties` only grows).
    assert_eq!(
        norm("margin: 1em !important; margin-left: 2em"),
        want(&[
            ("!margin-top", "1em"),
            ("!margin-right", "1em"),
            ("!margin-bottom", "1em"),
            ("!margin-left", "2em"),
        ])
    );
}

#[test]
fn shorthand_edge_cases() {
    assert_eq!(
        norm("border: inherit"),
        want(&[
            ("border-top-color", "inherit"),
            ("border-top-style", "none"),
            ("border-top-width", "medium"),
            ("border-right-color", "inherit"),
            ("border-right-style", "none"),
            ("border-right-width", "medium"),
            ("border-bottom-color", "inherit"),
            ("border-bottom-style", "none"),
            ("border-bottom-width", "medium"),
            ("border-left-color", "inherit"),
            ("border-left-style", "none"),
            ("border-left-width", "medium"),
        ])
    );
    assert_eq!(
        norm("list-style: inherit"),
        want(&[
            ("list-style-type", "inherit"),
            ("list-style-position", "inherit"),
            ("list-style-image", "inherit"),
        ])
    );
    // Parts read from the end; one no part accepts is ignored, an earlier one wins.
    assert_eq!(
        norm("border-top: red blue bogus 3PX"),
        want(&[
            ("border-top-color", "red"),
            ("border-top-style", "none"),
            ("border-top-width", "3px"),
        ])
    );
    // Commas and slashes are not parts; keywords keep their case.
    assert_eq!(
        norm("margin: 1em, 2em"),
        want(&[
            ("margin-top", "1em"),
            ("margin-right", "2em"),
            ("margin-bottom", "1em"),
            ("margin-left", "2em"),
        ])
    );
    assert_eq!(
        norm("border-left: 2px/SOLID Red"),
        want(&[
            ("border-left-color", "Red"),
            ("border-left-style", "SOLID"),
            ("border-left-width", "2px"),
        ])
    );
    // More than four sides: the first four.
    assert_eq!(
        norm("padding: 1px 2px 3px 4px 5px"),
        want(&[
            ("padding-top", "1px"),
            ("padding-right", "2px"),
            ("padding-bottom", "3px"),
            ("padding-left", "4px"),
        ])
    );
    assert_eq!(
        norm("font: caption"),
        want(&[
            ("font-style", "normal"),
            ("font-variant", "normal"),
            ("font-weight", "normal"),
            ("font-size", "1.0rem"),
            ("line-height", "normal"),
            ("font-family", "serif"),
        ])
    );
    assert_eq!(
        norm("font: condensed 700 1em/2 'My Font', SansSerif, and-x"),
        want(&[
            ("font-style", "normal"),
            ("font-variant", "normal"),
            ("font-weight", "700"),
            ("font-size", "1em"),
            ("line-height", "2"),
            ("font-family", "\"My Font\", sans-serif, \"and-x\""),
            ("font-stretch", "condensed"),
        ])
    );
}

#[test]
fn tables_match_calibre() {
    assert_eq!(DEFAULTS.len(), 101);
    assert!(DEFAULTS.contains(&("border-top-color", "currentColor")));
    assert!(DEFAULTS.contains(&("margin-left", "0")));
    assert!(DEFAULTS.contains(&("quotes", "'\u{201c}' '\u{201d}' '\u{2018}' '\u{2019}'")));
    assert_eq!(INHERITED.len(), 41);
    assert!(INHERITED.contains(&"text-indent"));
    assert_eq!(DEFAULT_PROFILE.fbase, 12.0);
    assert_eq!(DEFAULT_PROFILE.fnames[4], ("large", 13.5));
}

#[test]
fn a_shorthand_of_100_000_parts_is_linear() {
    let parts = vec!["1px"; 100_000].join(" ");
    let start = Instant::now();
    let got = normalize(
        vec![
            (
                "margin".to_owned(),
                Decl {
                    value: parts.clone(),
                    important: false,
                },
            ),
            (
                "border".to_owned(),
                Decl {
                    value: parts.clone(),
                    important: false,
                },
            ),
            (
                "font".to_owned(),
                Decl {
                    value: format!("{parts} serif"),
                    important: false,
                },
            ),
            (
                "list-style".to_owned(),
                Decl {
                    value: parts,
                    important: false,
                },
            ),
        ],
        &DEFAULT_PROFILE,
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(
        got[0],
        (
            "margin-top".to_owned(),
            Decl {
                value: "1px".to_owned(),
                important: false
            }
        )
    );
}
