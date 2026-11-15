// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's marks on a spine file, made after Calibre's first cascade and before its flattener
//! (phase 3 Task 5). Calibre's `CSSFlattener` calls the output plugin's
//! `specialize_css_for_output(item, stylizer)` on each file just before flattening it; kfxgen's
//! (`plugin/__init__.py`) runs `mark_source_classes`, `mark_link_state_colours` and
//! `mark_vertical_margins` (`plugin/kfxgen/converter.py`, kfxgen `ad38ec8`; GPL v3, `NOTICE`),
//! which write `data-kfxgen-*` attributes the converter reads once the classes are renamed and
//! the book's own CSS is gone.
//!
//! Hostile input: each function walks the tree once, as a loop. The two that match selectors
//! build one [`DocIndex`] of the file each and run each rule's selector once over the whole
//! file, as the cascade does, never once per element; their steps are charged on from the
//! first cascade's ([`deckle_limits::MAX_SELECTOR_WORK`] for the file, and through the
//! flattener the book's limit). A link colour, which the book writes and may make long, is
//! copied onto each link it marks, so each copy is charged like a declaration against
//! [`deckle_limits::MAX_CASCADE_DECLS`]. The other marks are a few short words per element.

use crate::css::{
    Cascaded, Decl, DocIndex, Error, Origin, Parsed, Units, decl_units, parse_selector, select,
};
use crate::dom::{Dom, NodeId};
use crate::generate::py;

/// On the file's root.
const SOURCE: &str = "data-kfxgen-source";
/// On each element whose own class holds the `toc` token (#329).
const TOC: &str = "data-kfxgen-toc";
/// On each element whose own class holds a front matter token (#393).
const FRONT: &str = "data-kfxgen-front";
/// An element's `align=` attribute, stripped and in lower case (#621).
const ALIGN: &str = "data-kfxgen-align";
/// A link's colour from a `:link` or `:visited` rule that wins (#733).
const LINK_COLOUR: &str = "data-kfxgen-link-colour";
/// In a vertical-rl file, the margins and paddings the book sets on an element, and the user
/// agent's own (#247).
const V_AUTHOR: &str = "data-kfxgen-v-author";
const V_UA: &str = "data-kfxgen-v-ua";
/// On the first horizontal-tb element of a vertical-rl file's branch (#247).
const HORIZONTAL: &str = "data-kfxgen-horizontal";
/// On the first vertical-rl element inside such a horizontal one (#247).
const VERTICAL: &str = "data-kfxgen-vertical";

/// `_FRONT_MATTER`.
const FRONT_MATTER: [&str; 5] = [
    "dedication",
    "epigraph",
    "copyright",
    "copyright-page",
    "colophon",
];
/// `_BOX_PROPS`: margins then paddings, each top, right, bottom, left.
const BOX_PROPS: [&str; 8] = [
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
];
const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
/// `_OTHER_STATE`'s names.
const OTHER_STATES: [&str; 14] = [
    "active",
    "after",
    "before",
    "checked",
    "disabled",
    "enabled",
    "first-letter",
    "first-line",
    "focus",
    "focus-visible",
    "focus-within",
    "hover",
    "target",
    "target-within",
];

/// `specialize_css_for_output`: the three marks on `dom`, in kfxgen's order, from `first`, the
/// file's first cascade, whose selector steps and units this goes on charging. Run it after
/// [`crate::css::cascade`] and before [`crate::css::flatten`].
pub(crate) fn mark(dom: &mut Dom, first: &mut Cascaded) -> Result<(), Error> {
    mark_source_classes(dom);
    mark_link_state_colours(dom, first)?;
    mark_vertical_margins(dom, first)
}

/// `mark_source_classes`: the root marked, and each element's class tokens and `align=` as the
/// book wrote them.
fn mark_source_classes(dom: &mut Dom) {
    let root = dom.root();
    dom.set_attr(root, SOURCE, Some("1"));
    let elements: Vec<NodeId> = dom.descendants().collect();
    for e in elements {
        let class = dom.attr(e, "class").unwrap_or("").to_lowercase();
        let tokens: Vec<&str> = class
            .split(py::is_space)
            .filter(|t| !t.is_empty())
            .collect();
        let align = dom
            .attr(e, "align")
            .filter(|a| !a.is_empty())
            .map(|a| py::strip(a).to_lowercase());
        if tokens.contains(&"toc") {
            dom.set_attr(e, TOC, Some("1"));
        }
        if tokens.iter().any(|t| FRONT_MATTER.contains(t)) {
            dom.set_attr(e, FRONT, Some("1"));
        }
        if let Some(align) = align {
            dom.set_attr(e, ALIGN, Some(&align));
        }
    }
}

/// `mark_link_state_colours`: on each link, the colour of the last colour rule matching it in
/// cascade order, where that rule is a `:link` or `:visited` one. A rule with `:hover` and the
/// like counts for nothing, and one with a bare `a:visited` is dropped, as Previewer drops it;
/// a link without an href, or with a colour in its `style`, is left alone. `!important` is not
/// looked at, as kfxgen does not.
fn mark_link_state_colours(dom: &mut Dom, first: &mut Cascaded) -> Result<(), Error> {
    // (rule index, its colour, whether it is a `:link` or `:visited` rule)
    let mut rules: Vec<(usize, &Decl, bool)> = Vec::new();
    for (i, r) in first.rules.iter().enumerate() {
        let text: Vec<char> = r.selector.chars().collect();
        if bare_a_visited(&text) {
            continue;
        }
        let colour = r.decls.iter().find(|(k, _)| k == "color").map(|(_, d)| d);
        if let Some(d) = colour.filter(|d| !d.value.is_empty())
            && !other_state(&text)
        {
            rules.push((i, d, link_state(&text)));
        }
    }
    if !rules.iter().any(|&(_, _, state)| state) {
        return Ok(());
    }
    let mut link_of = vec![usize::MAX; dom.node_count()];
    let links: Vec<NodeId> = dom
        .descendants()
        .filter(|&e| {
            dom.tag(e) == "a"
                && dom.attr(e, "href").is_some()
                && !inline_colour(dom.attr(e, "style").unwrap_or(""))
        })
        .collect();
    if links.is_empty() {
        return Ok(());
    }
    for (i, e) in links.iter().enumerate() {
        link_of[e.index()] = i;
    }
    let index = DocIndex::new(dom);
    index.spend(first.spent.work);
    // Each link's winning colour, None when the last rule matching it is not a link state's.
    let mut found: Vec<Option<&Decl>> = vec![None; links.len()];
    for &(i, d, state) in &rules {
        // A selector `Select` refuses: `except Exception: continue`.
        let Parsed::Ok { selector, .. } = parse_selector(&first.rules[i].selector) else {
            continue;
        };
        for e in select(&selector, dom, &index)? {
            if let Some(slot) = found.get_mut(link_of[e.index()]) {
                *slot = state.then_some(d);
            }
        }
    }
    let work = index.work();
    let mut units = Units(first.spent.units);
    for (e, d) in links.iter().zip(found) {
        if let Some(d) = d {
            units.charge(decl_units("color", d))?;
            dom.set_attr(*e, LINK_COLOUR, Some(&d.value));
        }
    }
    first.spent.units = units.0;
    first.spent.work = work;
    Ok(())
}

/// `mark_vertical_margins`: in a file whose `<body>` (else its `<html>`) sets `writing-mode:
/// vertical-rl`, on each element of the body that stays vertical, which of its margins and
/// paddings the book's rules and `style` set ([`V_AUTHOR`]) and what the user agent's rules give
/// them ([`V_UA`]); the first horizontal-tb element of a branch and the first vertical-rl one
/// inside it are marked as such.
fn mark_vertical_margins(dom: &mut Dom, first: &mut Cascaded) -> Result<(), Error> {
    let Some(body) = dom.descendants().find(|&e| dom.tag(e) == "body") else {
        return Ok(());
    };
    // `stylizer.style(e).get('writing-mode')`, when set and not empty.
    let specified = &first.specified;
    let set_mode = |e: NodeId| {
        specified[e.index()]
            .iter()
            .find(|(k, _)| k == "writing-mode")
            .map(|(_, d)| d.value.as_str())
            .filter(|v| !v.is_empty())
    };
    let mode = [Some(body), dom.parent(body)]
        .into_iter()
        .flatten()
        .find_map(set_mode)
        .map(|v| py::strip(v).to_lowercase());
    if mode.as_deref() != Some("vertical-rl") {
        return Ok(());
    }
    let n = dom.node_count();
    let index = DocIndex::new(dom);
    index.spend(first.spent.work);
    // Each element's box properties from the book's rules, as bits in `BOX_PROPS` order, and
    // from the user agent's, in the order first set (a dictionary), a later rule's value winning.
    let mut authored = vec![0u8; n];
    let mut ua: Vec<Vec<(usize, &str)>> = vec![Vec::new(); n];
    for r in &first.rules {
        let props: Vec<(usize, &str)> = BOX_PROPS
            .iter()
            .enumerate()
            .filter_map(|(p, name)| {
                r.decls
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, d)| (p, d.value.as_str()))
            })
            .collect();
        // `not rule[0]` is never true: a sort key is never empty.
        if props.is_empty() {
            continue;
        }
        let Parsed::Ok { selector, .. } = parse_selector(&r.selector) else {
            // kfxgen's oddness: `select()` is a generator, so `Select` raises on the selector
            // while the loop outside the `try` reads it. The exception leaves the function, and
            // the plugin's `try` around it logs it: the file gets none of these marks.
            first.spent.work = index.work();
            return Ok(());
        };
        let bits = props.iter().fold(0u8, |b, &(p, _)| b | 1 << p);
        for e in select(&selector, dom, &index)? {
            if r.origin == Origin::UserAgent {
                let own = &mut ua[e.index()];
                for &(p, v) in &props {
                    match own.iter_mut().find(|(q, _)| *q == p) {
                        Some(slot) => slot.1 = v,
                        None => own.push((p, v)),
                    }
                }
            } else {
                authored[e.index()] |= bits;
            }
        }
    }
    first.spent.work = index.work();

    // Each element's writing mode: its own when vertical-rl or horizontal-tb, else its parent's.
    // A horizontal one and all inside it are left unmarked but for the first.
    let mut horizontal = vec![false; n];
    let elements: Vec<NodeId> = dom.subtree(body).filter(|&e| !dom.is_comment(e)).collect();
    let mut marks: Vec<(NodeId, &str, String)> = Vec::new();
    for e in elements {
        if e != body {
            let own = set_mode(e).map(|v| py::strip(v).to_lowercase());
            let above = dom.parent(e).is_some_and(|p| horizontal[p.index()]);
            let h = match own.as_deref() {
                Some("horizontal-tb") => true,
                Some("vertical-rl") => false,
                _ => above,
            };
            horizontal[e.index()] = h;
            if h {
                if !above {
                    marks.push((e, HORIZONTAL, "1".to_owned()));
                }
                continue;
            }
            if above {
                marks.push((e, VERTICAL, "1".to_owned()));
            }
        }
        let own = authored[e.index()] | style_box_props(dom.attr(e, "style").unwrap_or(""));
        let names: Vec<&str> = BOX_PROPS
            .iter()
            .enumerate()
            .filter(|&(p, _)| own & 1 << p != 0)
            .map(|(_, name)| *name)
            .collect();
        marks.push((e, V_AUTHOR, names.join(" ")));
        let values: Vec<String> = ua[e.index()]
            .iter()
            .map(|&(p, v)| format!("{}:{v}", BOX_PROPS[p]))
            .collect();
        marks.push((e, V_UA, values.join(";")));
    }
    for (e, name, value) in marks {
        dom.set_attr(e, name, Some(&value));
    }
    Ok(())
}

/// The box properties a `style` attribute sets, as bits in `BOX_PROPS` order: `_STYLE_DECL`,
/// `(?:^|;)\s*(margin|padding)(?:-(top|right|bottom|left))?\s*:` (`re.I`), found at its start
/// and after each `;`. A match no other can overlap: each holds one `;` at most, its first.
/// ASCII case is enough: a word matched through `re.I`'s other folds (`ı`, `ſ`) names no box
/// property once kfxgen lowers it.
fn style_box_props(style: &str) -> u8 {
    let s: Vec<char> = style.chars().collect();
    let starts = std::iter::once(0).chain((0..s.len()).filter(|&i| s[i] == ';').map(|i| i + 1));
    let mut bits = 0u8;
    for start in starts {
        let at = skip_space(&s, start);
        let Some((kind, at)) = [(0, "margin"), (4, "padding")]
            .into_iter()
            .find_map(|(k, w)| ascii_word(&s, at, w).map(|end| (k, end)))
        else {
            continue;
        };
        let colon = |i: usize| s.get(skip_space(&s, i)) == Some(&':');
        let side = (s.get(at) == Some(&'-'))
            .then(|| {
                SIDES
                    .iter()
                    .enumerate()
                    .find_map(|(j, w)| ascii_word(&s, at + 1, w).map(|end| (j, end)))
            })
            .flatten()
            .filter(|&(_, end)| colon(end));
        bits |= match side {
            Some((j, _)) => 1 << (kind + j),
            None if colon(at) => 0b1111 << kind,
            None => 0,
        };
    }
    bits
}

/// `_INLINE_COLOUR`, `(?:^|;)\s*color\s*:` (`re.I`), searched in a `style` attribute.
fn inline_colour(style: &str) -> bool {
    let s: Vec<char> = style.chars().collect();
    std::iter::once(0)
        .chain((0..s.len()).filter(|&i| s[i] == ';').map(|i| i + 1))
        .any(|start| {
            re_i_word(&s, skip_space(&s, start), "color")
                .is_some_and(|end| s.get(skip_space(&s, end)) == Some(&':'))
        })
}

/// `_LINK_STATE`, `:(?:link|visited)(?![\w-])` (`re.I`).
fn link_state(s: &[char]) -> bool {
    (0..s.len()).any(|i| s[i] == ':' && ["link", "visited"].iter().any(|w| state_at(s, i + 1, w)))
}

/// `_OTHER_STATE`, `:{1,2}(?:active|after|…)(?![\w-])` (`re.I`): a name after any colon, the
/// second of two included.
fn other_state(s: &[char]) -> bool {
    (0..s.len()).any(|i| s[i] == ':' && OTHER_STATES.iter().any(|w| state_at(s, i + 1, w)))
}

/// `_BARE_A_VISITED`, `(?:a|(?<![^\s>+~,(])A):(?i:visited)(?![\w-])`: a lower-case `a` after
/// anything (so `.xa:visited` too, kfxgen's reading of Previewer), an upper-case one only at the
/// start or after white space or `>+~,(`.
fn bare_a_visited(s: &[char]) -> bool {
    (0..s.len()).any(|i| {
        let a = s[i] == 'a'
            || (s[i] == 'A' && (i == 0 || py::is_space(s[i - 1]) || ">+~,(".contains(s[i - 1])));
        a && s.get(i + 1) == Some(&':') && state_at(s, i + 2, "visited")
    })
}

/// `word` at `i` under `re.I`, not followed by a word character or `-` (`(?![\w-])`).
fn state_at(s: &[char], i: usize, word: &str) -> bool {
    re_i_word(s, i, word).is_some_and(|end| {
        s.get(end)
            .is_none_or(|&c| !(c == '-' || c == '_' || py::is_alnum(c)))
    })
}

/// Where `word` (lower-case ASCII) ends when it is at `i` under Python's `re.I`: each letter in
/// either case, and also the letters `re` folds to it beyond ASCII (K, İ, ı, ſ).
fn re_i_word(s: &[char], i: usize, word: &str) -> Option<usize> {
    let end = i + word.len();
    let same = |c: char, p: char| {
        c.to_ascii_lowercase() == p
            || matches!(
                (p, c),
                ('k', '\u{212A}') | ('i', '\u{130}' | '\u{131}') | ('s', '\u{17F}')
            )
    };
    (s.get(i..end)?
        .iter()
        .zip(word.chars())
        .all(|(&c, p)| same(c, p)))
    .then_some(end)
}

/// Where `word` (lower-case ASCII) ends when it is at `i`, in any ASCII case.
fn ascii_word(s: &[char], i: usize, word: &str) -> Option<usize> {
    let end = i + word.len();
    (s.get(i..end)?
        .iter()
        .zip(word.chars())
        .all(|(c, p)| c.to_ascii_lowercase() == p))
    .then_some(end)
}

/// The first position from `i` that is not Python white space (`\s`).
fn skip_space(s: &[char], i: usize) -> usize {
    (i..s.len())
        .find(|&j| !py::is_space(s[j]))
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// kfxgen's regular expressions, on the cases its comments name (P741m/n/o/r).
    #[test]
    fn selector_patterns_read_as_kfxgens() {
        for (text, want) in [
            ("a:visited", true),
            ("p a:visited", true),
            ("a:visited.x", true),
            (".a:visited", true),
            ("#a:visited", true),
            (".xa:visited", true),
            ("A:visited", true),
            ("p>A:VISITED", true),
            (":visited", false),
            ("*:visited", false),
            ("a.x:visited", false),
            ("a[href]:visited", false),
            (".A:VISITED", false),
            ("a:visited-x", false),
        ] {
            assert_eq!(bare_a_visited(&chars(text)), want, "{text}");
        }
        assert!(link_state(&chars("a:LINK")));
        assert!(link_state(&chars("a:lın\u{212A}")));
        assert!(!link_state(&chars("a:links")));
        assert!(other_state(&chars("a:link:hover")));
        assert!(other_state(&chars("p::before")));
        assert!(other_state(&chars("a:focus-visible")));
        assert!(!other_state(&chars("a:focus-x")));
        assert!(inline_colour("font-weight: bold; COLOR : red"));
        assert!(!inline_colour("background-color: red"));
    }

    /// `page` with `css` as its stylesheet, through the first cascade.
    fn cascaded(page: &str, css: &str) -> (Dom, Cascaded) {
        use crate::css::{DEFAULT_PROFILE, cascade, parse_sheet};
        let mut dom = crate::dom::parse(page.as_bytes()).unwrap();
        let sheet = parse_sheet(css, Origin::Book, "c.xhtml", 0, &mut |_| None).unwrap();
        let first = cascade(&mut dom, "c.xhtml", vec![sheet], &DEFAULT_PROFILE).unwrap();
        (dom, first)
    }

    /// [`cascaded`], then the marks.
    fn marked(page: &str, css: &str) -> Dom {
        let (mut dom, mut first) = cascaded(page, css);
        mark(&mut dom, &mut first).unwrap();
        dom
    }

    /// Each copy of a link colour is charged as a declaration, on from the first cascade's
    /// units: 1 for a short one, 1 more for each 64 bytes.
    #[test]
    fn link_colours_are_charged_copy_by_copy() {
        use deckle_limits::MAX_CASCADE_DECLS;
        let page = format!(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head><body>{}</body></html>"#,
            r#"<a href="b.xhtml">b</a>"#.repeat(10)
        );
        let colour = "c".repeat(200);
        let css = format!("a:link {{ color: {colour} }}");
        let (mut dom, mut first) = cascaded(&page, &css);
        let before = first.spent.units;
        mark(&mut dom, &mut first).unwrap();
        assert_eq!(first.spent.units - before, 10 * (1 + (5 + 200) / 64));
        let (mut dom, mut first) = cascaded(&page, &css);
        first.spent.units = MAX_CASCADE_DECLS - 30;
        assert!(matches!(
            mark(&mut dom, &mut first),
            Err(Error::Cascade(MAX_CASCADE_DECLS))
        ));
    }

    fn first_of<'a>(dom: &'a Dom, tag: &str, name: &str) -> Option<&'a str> {
        let e = dom.descendants().find(|&e| dom.tag(e) == tag)?;
        dom.attr(e, name)
    }

    /// A selector `Select` refuses is skipped for link colours, inside kfxgen's `try`, but
    /// ends the vertical marks of the file, outside it.
    #[test]
    fn a_refused_selector_skips_a_link_rule_and_ends_the_vertical_marks() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head><body style="writing-mode: vertical-rl"><p><a href="b.xhtml">b</a></p></body></html>"#;
        let dom = marked(page, "a:link { color: red } a::marker { color: blue }");
        assert_eq!(first_of(&dom, "a", LINK_COLOUR), Some("red"));
        assert_eq!(first_of(&dom, "p", V_AUTHOR), Some(""));
        let dom = marked(page, "p { margin-top: 1em }");
        assert_eq!(first_of(&dom, "p", V_AUTHOR), Some("margin-top"));
        let dom = marked(page, "p { margin-top: 1em } p::marker { margin-left: 0 }");
        assert_eq!(first_of(&dom, "p", V_AUTHOR), None);
        assert_eq!(first_of(&dom, "html", SOURCE), Some("1"));
    }

    #[test]
    fn style_attribute_box_properties() {
        let names = |s: &str| {
            let b = style_box_props(s);
            BOX_PROPS
                .iter()
                .enumerate()
                .filter(|&(p, _)| b & 1 << p != 0)
                .map(|(_, n)| *n)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("margin: 0"), &BOX_PROPS[..4]);
        assert_eq!(names("x: 1; PADDING-Left : 2em"), ["padding-left"]);
        assert_eq!(names("margin-top:0;padding :1em"), {
            let mut v = vec!["margin-top"];
            v.extend(&BOX_PROPS[4..]);
            v
        });
        assert!(names("margin-x: 0; x-margin: 0; margin-top 0").is_empty());
    }
}
