// SPDX-License-Identifier: GPL-3.0-only
//! A rule's declarations as Calibre's Stylizer keeps them (`stylizer.py`
//! `StylizerRules.flatten_style`, with `normalize_css.py`'s normalizers): shorthands expanded,
//! font-size keywords turned into `rem`, `-epub-` properties copied (phase 2 plan, Task 6).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use cssparser::{Parser, ParserInput, Token};
use regex::Regex;

use super::defaults::default_of;
use super::font_shorthand::normalize_font;
use super::{Decl, Decls};

/// The output profile Calibre's Stylizer measures with: font sizes and the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// The base font size, in points.
    pub fbase: f64,
    pub dpi: f64,
    pub width_pts: f64,
    pub height_pts: f64,
    /// The font-size keywords, `xx-small` to `xx-large`, in points.
    pub fnames: [(&'static str, f64); 7],
    /// The sizes of HTML's `<font size="1">` to `size="7"`, in points.
    pub fnums: [f64; 7],
}

/// Calibre's `default` output profile, which the Stylizer uses whatever the conversion's
/// output profile is (`Stylizer.__init__`): `customize/profiles.py` `Plugin`'s `fbase`,
/// `font_sizes` zipped with `FONT_SIZES`, and a 1600 × 1200 screen at 100 dpi in points.
pub const DEFAULT_PROFILE: Profile = Profile {
    fbase: 12.0,
    dpi: 100.0,
    width_pts: 1152.0,
    height_pts: 864.0,
    fnames: [
        ("xx-small", 5.0),
        ("x-small", 7.0),
        ("small", 9.0),
        ("medium", 12.0),
        ("large", 13.5),
        ("x-large", 17.0),
        ("xx-large", 20.0),
    ],
    fnums: [5.0, 9.0, 12.0, 13.5, 17.0, 20.0, 22.0],
};

/// The most declarations [`normalize`] makes of one declaration of `name`: its parts for a
/// shorthand, the `-webkit-` and unprefixed copies for an `-epub-` one, else 1.
pub(crate) fn expanded_len(name: &str) -> usize {
    match name {
        "border" => 12,
        "font" => 7,
        "margin" | "padding" | "border-style" | "border-width" | "border-color" => 4,
        "border-top" | "border-right" | "border-bottom" | "border-left" | "list-style" => 3,
        "-epub-writing-mode"
        | "-epub-text-emphasis"
        | "-epub-text-emphasis-color"
        | "-epub-text-emphasis-position"
        | "-epub-text-emphasis-style" => 2,
        _ => 1,
    }
}

/// `StylizerRules.flatten_style` applied to declarations [`super::parse_sheet`] or
/// [`super::parse_style_attr`] read:
///
/// - `margin`, `padding`, `border`, `border-{top,right,bottom,left}`, `border-style`,
///   `border-width`, `border-color`, `list-style` and `font` become their parts
///   (`normalize_css.py` `normalizers`); `background`, `text-decoration` and `outline` stay.
///   A part takes the place of an earlier declaration of it, and is `!important` when its
///   shorthand is or when an earlier declaration of it was (Calibre's set of important names
///   only grows).
/// - A `font-size` keyword (`normal` read as `medium`, `smallest` as `xx-small`) becomes
///   `{:.1}rem` of the profile's size over `fbase`; the test is case-sensitive, so `LARGE`
///   stays.
/// - `-epub-writing-mode` and `-epub-text-emphasis*` move to the end as their `-webkit-` and
///   unprefixed names, where those are not set (`cleanup_epub_prefixed_properties`).
///
/// A value no rule reads (`margin-left: foo`) is kept as it is. `decls` hold each name once,
/// as [`super::parse_sheet`] and [`super::parse_style_attr`] give them.
pub fn normalize(decls: Decls, profile: &Profile) -> Decls {
    debug_assert!(
        decls
            .iter()
            .map(|(k, _)| k)
            .collect::<std::collections::HashSet<_>>()
            .len()
            == decls.len(),
        "normalize takes each name once"
    );
    let mut style = Style::default();
    for (name, d) in decls {
        match expand(&name, &d.value) {
            Some(parts) => {
                for (k, v) in parts {
                    style.set(k, v, d.important);
                }
            }
            None => style.set(name, d.value, d.important),
        }
    }
    if let Some(&i) = style.at.get("font-size") {
        let size = style.v[i].1.value.as_str();
        let size = match size {
            "normal" => "medium",
            "smallest" => "xx-small",
            s => s,
        };
        if let Some((_, pts)) = profile.fnames.iter().find(|(n, _)| *n == size) {
            style.v[i].1.value = format!("{:.1}rem", pts / profile.fbase);
        }
    }
    style.cleanup_epub_prefixed();
    style.v
}

/// The names a shorthand or an `-epub-` property becomes: the only names [`normalize`] can set
/// twice, since its input holds each name once.
static PART_NAMES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    const EDGES: [&str; 4] = ["top", "right", "bottom", "left"];
    let mut names = HashSet::new();
    for edge in EDGES {
        names.insert(format!("margin-{edge}"));
        names.insert(format!("padding-{edge}"));
        for part in ["color", "style", "width"] {
            names.insert(format!("border-{edge}-{part}"));
        }
    }
    for name in [
        "list-style-type",
        "list-style-position",
        "list-style-image",
        "font-style",
        "font-variant",
        "font-weight",
        "font-size",
        "line-height",
        "font-family",
        "font-stretch",
    ] {
        names.insert(name.to_owned());
    }
    for name in EPUB_PREFIXED {
        names.insert(name.to_owned());
        names.insert(format!("-webkit-{name}"));
    }
    names
});

/// The properties `cleanup_epub_prefixed_properties` copies from their `-epub-` names.
const EPUB_PREFIXED: [&str; 5] = [
    "writing-mode",
    "text-emphasis",
    "text-emphasis-color",
    "text-emphasis-position",
    "text-emphasis-style",
];

/// A Python dict of declarations: a name set again keeps its place. Only [`PART_NAMES`] can be
/// set again, so only they are looked up; any other name is added at the end.
#[derive(Default)]
struct Style {
    v: Decls,
    at: HashMap<String, usize>,
}

impl Style {
    fn set(&mut self, name: String, value: String, important: bool) {
        if !PART_NAMES.contains(&name) {
            self.v.push((name, Decl { value, important }));
            return;
        }
        match self.at.get(&name) {
            Some(&i) => {
                let d = &mut self.v[i].1;
                d.value = value;
                d.important |= important;
            }
            None => {
                self.at.insert(name.clone(), self.v.len());
                self.v.push((name, Decl { value, important }));
            }
        }
    }

    /// `stylizer.py` `cleanup_epub_prefixed_properties`.
    fn cleanup_epub_prefixed(&mut self) {
        let found: Vec<String> = self
            .v
            .iter()
            .map(|(k, _)| k)
            .filter(|k| {
                k.strip_prefix("-epub-")
                    .is_some_and(|u| EPUB_PREFIXED.contains(&u))
            })
            .cloned()
            .collect();
        for x in found {
            let Some(pos) = self.v.iter().position(|(k, _)| *k == x) else {
                continue;
            };
            let (_, d) = self.v.remove(pos);
            self.at = self
                .v
                .iter()
                .enumerate()
                .filter(|(_, (k, _))| PART_NAMES.contains(k))
                .map(|(i, (k, _))| (k.clone(), i))
                .collect();
            let unprefixed = &x["-epub-".len()..];
            for key in [format!("-webkit-{unprefixed}"), unprefixed.to_owned()] {
                if !self.at.contains_key(&key) {
                    // A plain dict assignment: not marked important.
                    self.set(key, d.value.clone(), false);
                }
            }
        }
    }
}

/// The parts a shorthand sets, or `None` for a property Calibre has no normalizer for.
fn expand(name: &str, value: &str) -> Option<Vec<(String, String)>> {
    const EDGES: [&str; 4] = ["top", "right", "bottom", "left"];
    match name {
        "margin" | "padding" | "border-style" | "border-width" | "border-color" => {
            Some(normalize_edge(name, value))
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            Some(simple_composition(value, &border_parts(name), false))
        }
        "border" => {
            // `normalize_border`: `border-top`'s parts, copied to the other edges.
            let top = simple_composition(value, &border_parts("border-top"), false);
            let mut out = top.clone();
            for edge in &EDGES[1..] {
                out.extend(
                    top.iter()
                        .map(|(k, v)| (k.replacen("top", edge, 1), v.clone())),
                );
            }
            Some(out)
        }
        "list-style" => Some(simple_composition(
            value,
            &[
                "list-style-type".to_owned(),
                "list-style-position".to_owned(),
                "list-style-image".to_owned(),
            ],
            true,
        )),
        "font" => Some(normalize_font(value)),
        _ => None,
    }
}

fn border_parts(prefix: &str) -> [String; 3] {
    ["color", "style", "width"].map(|p| format!("{prefix}-{p}"))
}

/// The items of a value css_parser wrote, as its `PropertyValue` lists them: each top-level
/// value (a function or `url()` whole), without the commas and slashes between them.
fn primitives(value: &str) -> Vec<String> {
    let mut input = ParserInput::new(value);
    let mut p = Parser::new(&mut input);
    let mut out = Vec::new();
    loop {
        p.skip_whitespace();
        let start = p.position();
        let Ok(tok) = p.next_including_whitespace() else {
            break;
        };
        if matches!(tok, Token::Comma | Token::Delim('/')) {
            continue;
        }
        if matches!(
            tok,
            Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock
        ) {
            let _ = p.parse_nested_block(|_| Ok::<(), cssparser::ParseError<'_, ()>>(()));
        }
        out.push(p.slice_from(start).to_owned());
    }
    out
}

/// `normalize_css.py` `normalize_edge`: one to four values for the four edges, more than four
/// cut to four.
fn normalize_edge(name: &str, value: &str) -> Vec<(String, String)> {
    let mut items = primitives(value);
    items.truncate(4);
    let values: [String; 4] = match items.as_slice() {
        [] => return Vec::new(),
        [a] => [a.clone(), a.clone(), a.clone(), a.clone()],
        [v, h] => [v.clone(), h.clone(), v.clone(), h.clone()],
        [t, h, b] => [t.clone(), h.clone(), b.clone(), h.clone()],
        [t, r, b, l, ..] => [t.clone(), r.clone(), b.clone(), l.clone()],
    };
    let names: [String; 4] = match name.split_once('-') {
        Some((l, r)) => ["top", "right", "bottom", "left"].map(|e| format!("{l}-{e}-{r}")),
        None => ["top", "right", "bottom", "left"].map(|e| format!("{name}-{e}")),
    };
    names.into_iter().zip(values).collect()
}

/// `normalize_css.py` `normalize_simple_composition`: each part from its default, then the
/// value's items read from the last, each given to the first part that accepts it
/// (css_parser's `profile.validate`), so an earlier item wins.
fn simple_composition(
    value: &str,
    composition: &[String],
    check_inherit: bool,
) -> Vec<(String, String)> {
    if check_inherit && value == "inherit" {
        return composition
            .iter()
            .map(|k| (k.clone(), "inherit".to_owned()))
            .collect();
    }
    let mut style: Vec<(String, String)> = composition
        .iter()
        .map(|k| (k.clone(), default_of(k).to_owned()))
        .collect();
    for item in primitives(value).iter().rev() {
        if let Some(slot) = style.iter_mut().find(|(k, _)| validates(k, item)) {
            slot.1 = item.clone();
        }
    }
    style
}

/// A css_parser profile pattern, matched whole and without regard to case.
fn pattern(body: &str) -> Regex {
    Regex::new(&format!("(?i)^(?:{body})$")).expect("profile pattern")
}

/// css_parser 1.1.1's macros, as its `profile` object holds them (`_TOKEN_MACROS`, the CSS
/// Color Module Level 3 `color`).
const NUM: &str = r"-?\d+|-?\d*\.\d+";
const NAMED_COLOR: &str = "currentcolor|transparent|aqua|black|blue|fuchsia|gray|green|lime|maroon|navy|olive|orange|purple|red|silver|teal|white|yellow";
const X11_COLOR: &str = "aliceblue|antiquewhite|aqua|aquamarine|azure|beige|bisque|black|blanchedalmond|blue|blueviolet|brown|burlywood|cadetblue|chartreuse|chocolate|coral|cornflowerblue|cornsilk|crimson|cyan|darkblue|darkcyan|darkgoldenrod|darkgray|darkgreen|darkgrey|darkkhaki|darkmagenta|darkolivegreen|darkorange|darkorchid|darkred|darksalmon|darkseagreen|darkslateblue|darkslategray|darkslategrey|darkturquoise|darkviolet|deeppink|deepskyblue|dimgray|dimgrey|dodgerblue|firebrick|floralwhite|forestgreen|fuchsia|gainsboro|ghostwhite|gold|goldenrod|gray|green|greenyellow|grey|honeydew|hotpink|indianred|indigo|ivory|khaki|lavender|lavenderblush|lawngreen|lemonchiffon|lightblue|lightcoral|lightcyan|lightgoldenrodyellow|lightgray|lightgreen|lightgrey|lightpink|lightsalmon|lightseagreen|lightskyblue|lightslategray|lightslategrey|lightsteelblue|lightyellow|lime|limegreen|linen|magenta|maroon|mediumaquamarine|mediumblue|mediumorchid|mediumpurple|mediumseagreen|mediumslateblue|mediumspringgreen|mediumturquoise|mediumvioletred|midnightblue|mintcream|mistyrose|moccasin|navajowhite|navy|oldlace|olive|olivedrab|orange|orangered|orchid|palegoldenrod|palegreen|paleturquoise|palevioletred|papayawhip|peachpuff|peru|pink|plum|powderblue|purple|red|rosybrown|royalblue|saddlebrown|salmon|sandybrown|seagreen|seashell|sienna|silver|skyblue|slateblue|slategray|slategrey|snow|springgreen|steelblue|tan|teal|thistle|tomato|turquoise|violet|wheat|white|whitesmoke|yellow|yellowgreen";

static BORDER_COLOR: LazyLock<Regex> = LazyLock::new(|| {
    let int = r"-?\d+";
    let w = r"\s*";
    let rgb = format!(
        r"rgb\({w}{int}{w},{w}{int}{w},{w}{int}{w}\)|rgb\({w}(?:{NUM})%{w},{w}(?:{NUM})%{w},{w}(?:{NUM})%{w}\)"
    );
    let rgba = format!(
        r"rgba\({w}{int}{w},{w}{int}{w},{w}{int}{w},{w}(?:{NUM}){w}\)|rgba\({w}(?:{NUM})%{w},{w}(?:{NUM})%{w},{w}(?:{NUM})%{w},{w}(?:{NUM}){w}\)"
    );
    let hsl = format!(
        r"hsl\({w}{int}{w},{w}(?:{NUM})%{w},{w}(?:{NUM})%{w}\)|hsla\({w}{int}{w},{w}(?:{NUM})%{w},{w}(?:{NUM})%{w},{w}(?:{NUM}){w}\)"
    );
    pattern(&format!(
        "{NAMED_COLOR}|#[0-9a-f]{{3}}|#[0-9a-f]{{6}}|{rgb}|{rgba}|{hsl}|{X11_COLOR}|inherit"
    ))
});
static BORDER_STYLE: LazyLock<Regex> = LazyLock::new(|| {
    pattern("none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|inherit")
});
static BORDER_WIDTH: LazyLock<Regex> = LazyLock::new(|| {
    pattern(&format!(
        "0|(?:{NUM})(?:em|ex|px|in|cm|mm|pt|pc)|thin|medium|thick|inherit"
    ))
});
static LIST_STYLE_TYPE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        "disc|circle|square|decimal|decimal-leading-zero|lower-roman|upper-roman|lower-greek\
         |lower-(?:latin|alpha)|upper-(?:latin|alpha)|armenian|georgian|none|inherit",
    )
});
static LIST_STYLE_POSITION: LazyLock<Regex> = LazyLock::new(|| pattern("inside|outside|inherit"));
static LIST_STYLE_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r#"url\(\s*(?:"(?:\\"|[^"])*"|'(?:\\'|[^'])*'|(?:\\\)|[^)])+)\s*\)|none|inherit"#)
});

/// css_parser's `profile.validate(name, value)` for the parts Calibre's simple normalizers
/// fill.
fn validates(name: &str, value: &str) -> bool {
    let re: &Regex = if name.ends_with("-color") {
        &BORDER_COLOR
    } else if name.ends_with("-style") && name.starts_with("border") {
        &BORDER_STYLE
    } else if name.ends_with("-width") {
        &BORDER_WIDTH
    } else {
        match name {
            "list-style-type" => &LIST_STYLE_TYPE,
            "list-style-position" => &LIST_STYLE_POSITION,
            "list-style-image" => &LIST_STYLE_IMAGE,
            _ => return false,
        }
    };
    re.is_match(value)
}
