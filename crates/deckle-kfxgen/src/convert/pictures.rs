// SPDX-License-Identifier: GPL-3.0-only
//! Pictures in a paragraph's text: kfxgen's `_make_img_token`, `_strip_token_controls`,
//! `_img_size_hint`, `_img_inline_extras`, `_displays_as_block`, `_float_side`, `_img_float`,
//! `_css_background_image`, `_svg_image_refs`, `_svg_text_paragraphs`, `_svg_own_style`,
//! `_svg_draws_shapes_only` and `_resolve_img_src` (`converter.py`, at kfxgen `ad38ec8`).
//!
//! A picture goes into the text as a token (`_img_tokens.py`) naming it by href, with the size,
//! float and extras its markup and CSS ask for. Nothing here opens a file or reads a picture's
//! bytes: the href is only a key the generator looks up in the book's manifest.
//!
//! Hostile input: the href, alt, sizes and SVG content come from the book. Every SVG walk is a
//! loop with a stack of its own, once over the `<svg>`'s subtree (no recursion however deep it
//! nests); the size and length patterns are matched by hand in one pass; no number is parsed
//! here but by `parse_css_length`. The callers charge the text a token copies.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

use super::href::{is_unsafe, resolve_doc_path};
use super::inline::{keyword, text_of, words};
use super::inline_style::parse_css_length;
use super::resolver::{Resolved, Resolver};
use super::{Error, ItemError};
use crate::css;
use crate::dom::{Dom, NodeId};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `IMG_TOKEN_DELIM`, `IMG_TOKEN_FIELD`, `IMG_TOKEN_SPACE` (`_img_tokens.py`).
const DELIM: char = '\0';
const FIELD: char = '\u{1}';
const SPACE: char = '\u{2}';

/// The feature a picture embedded as a `data:` URI is refused as. calibre's conversion turns
/// an `<img src="data:image/…">` into a file of the manifest before kfxgen runs (`DataURL`,
/// `ebooks/oeb/transforms/data_url.py`, from `plumber.py`); `read_book` has no such step yet,
/// so every `data:` picture source is refused until it does (Task 18).
pub(crate) const DATA_URI: &str = "pictures embedded as data: URIs";

/// Whether a picture source is a `data:` URI: it starts with `data:` in any case, with
/// nothing before it. That is `_is_unsafe_href`'s test on the value as written
/// (`lower().startswith("data:")`; no character outside ASCII lowers to one of those letters),
/// and calibre's `DataURL` test (`raw.startswith('data:')`, case kept) is the lower-case part
/// of it. Neither strips white space first, and lxml keeps an attribute's spaces, so ` data:`
/// is a relative path to both. Reads at most five bytes and copies nothing.
pub(crate) fn is_data_uri(src: &str) -> bool {
    src.get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("data:"))
}

/// `SVG_IMAGE_SIZE`: the size hint of a picture an `<svg>` wraps.
pub(crate) const SVG_IMAGE_SIZE: &str = "h=100%";

const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// `_strip_token_controls`: `value` without the token's delimiters.
fn strip_token_controls(value: &str) -> impl Iterator<Item = char> + '_ {
    value
        .chars()
        .filter(|c| !matches!(*c, DELIM | FIELD | SPACE))
}

/// `_make_img_token(href, alt, size, float_side, extra)`: the token, its trailing empty fields
/// left out. The size, float and extra fields are written as given.
pub(crate) fn img_token(
    href: &str,
    alt: &str,
    size: Option<&str>,
    float_side: Option<&str>,
    extra: Option<&str>,
) -> String {
    let fields = [
        size.unwrap_or(""),
        float_side.unwrap_or(""),
        extra.unwrap_or(""),
    ];
    let kept = fields
        .iter()
        .rposition(|f| !f.is_empty())
        .map_or(0, |i| i + 1);
    let mut out = String::with_capacity(href.len() + alt.len() + 16);
    out.push(DELIM);
    out.push_str("IMG");
    out.push(FIELD);
    out.extend(strip_token_controls(href));
    out.push(FIELD);
    out.extend(strip_token_controls(alt).map(|c| if c == ' ' { SPACE } else { c }));
    for f in &fields[..kept] {
        out.push(FIELD);
        out.push_str(f);
    }
    out.push(DELIM);
    out
}

/// One or more decimal digits of any script (`\d+`); the bytes taken.
fn digits(s: &str) -> usize {
    s.char_indices()
        .find(|&(_, c)| py::digit_value(c).is_none())
        .map_or(s.len(), |(i, _)| i)
}

/// `$`: the end, or a last `"\n"`.
fn at_end(s: &str) -> bool {
    s.is_empty() || s == "\n"
}

/// `_IMG_SIZE_RE.match(s)`: `^\d+(?:\.\d+)?(?:%|px|em|pt)?$`.
pub(crate) fn is_img_size(s: &str) -> bool {
    let n = digits(s);
    if n == 0 {
        return false;
    }
    let mut rest = &s[n..];
    if let Some(frac) = rest.strip_prefix('.') {
        let m = digits(frac);
        if m > 0 {
            rest = &frac[m..];
        }
    }
    for unit in ["%", "px", "em", "pt"] {
        if let Some(after) = rest.strip_prefix(unit)
            && at_end(after)
        {
            return true;
        }
    }
    at_end(rest)
}

/// `_INLINE_VALIGN_LENGTH_RE.match(s)`:
/// `^[-+]?(?:\d+(?:\.\d+)?|\.\d+)(?:px|pt|em|rem|%)$|^[-+]?0(?:\.0+)?$`.
fn is_valign_length(s: &str) -> bool {
    let s = s.strip_prefix(['-', '+']).unwrap_or(s);
    // A signed 0, "0.00" and the like, with no unit.
    if let Some(rest) = s.strip_prefix('0') {
        let after = match rest.strip_prefix('.') {
            Some(r) if r.starts_with('0') => r.trim_start_matches('0'),
            _ => rest,
        };
        if at_end(after) {
            return true;
        }
    }
    let n = digits(s);
    let rest = if n > 0 {
        let rest = &s[n..];
        match rest.strip_prefix('.').map(|f| (f, digits(f))) {
            Some((f, m)) if m > 0 => &f[m..],
            _ => rest,
        }
    } else {
        let Some(frac) = s.strip_prefix('.') else {
            return false;
        };
        let m = digits(frac);
        if m == 0 {
            return false;
        }
        &frac[m..]
    };
    ["px", "pt", "em", "rem", "%"]
        .iter()
        .any(|unit| rest.strip_prefix(unit).is_some_and(at_end))
}

/// `css.get(key)` of a style that may be None (`style_resolver(elem) or {}`).
fn get<'r>(css: Option<&'r Resolved>, key: &str) -> Result<Option<&'r PyValue>, css::Error> {
    match css {
        Some(css) => css.get(key),
        None => Ok(None),
    }
}

/// `_img_size_hint(elem, style_resolver)`, given the element's resolved style: `w=<width>`,
/// else `h=<height>`, from the CSS or else the attribute, when it is a length the hint
/// accepts; None for the natural size.
pub(crate) fn img_size_hint(
    dom: &Dom,
    elem: NodeId,
    css: Option<&Resolved>,
) -> Result<Option<String>, css::Error> {
    for axis in ["width", "height"] {
        let raw = match get(css, axis)? {
            None | Some(PyValue::None) => Cow::Borrowed(dom.attr(elem, axis).unwrap_or("")),
            Some(PyValue::Str(s)) if s.is_empty() => {
                Cow::Borrowed(dom.attr(elem, axis).unwrap_or(""))
            }
            v => text_of(v),
        };
        let raw = py::strip(&raw);
        if !raw.is_empty() && raw != "auto" && is_img_size(raw) {
            return Ok(Some(format!("{}={raw}", &axis[..1])));
        }
    }
    Ok(None)
}

/// `_INLINE_VALIGNS`.
const INLINE_VALIGNS: [&str; 8] = [
    "middle",
    "top",
    "bottom",
    "text-top",
    "text-bottom",
    "baseline",
    "super",
    "sub",
];

/// `_img_inline_extras(elem, style_resolver)`, given the element's resolved style:
/// `h=…;va=…;ml=…;mr=…;c=…` (each when it applies), or None. kfxgen raises where
/// `parse_css_length` does on a side margin.
pub(crate) fn img_inline_extras(
    dom: &Dom,
    elem: NodeId,
    css: Option<&Resolved>,
) -> Result<Option<String>, ItemError> {
    let mut out = Vec::new();
    let height = text_of(get(css, "height")?);
    let raw = if height.is_empty() {
        Cow::Borrowed(dom.attr(elem, "height").unwrap_or(""))
    } else {
        height
    };
    let raw = py::strip(&raw);
    if is_img_size(raw) {
        out.push(format!("h={raw}"));
    }
    let valign = keyword(css, "vertical-align")?;
    if INLINE_VALIGNS.contains(&valign.as_str()) || is_valign_length(&valign) {
        out.push(format!("va={valign}"));
    }
    for (side, key) in [("margin-left", "ml"), ("margin-right", "mr")] {
        let value = text_of(get(css, side)?);
        let raw = py::strip(&value);
        if parse_css_length(raw, true)?.is_some() {
            out.push(format!("{key}={raw}"));
        }
    }
    let left = keyword(css, "margin-left")? == "auto";
    let right = keyword(css, "margin-right")? == "auto";
    if displays_as_block(css)? && left {
        out.push(if right { "c=1" } else { "c=r" }.to_owned());
    }
    Ok((!out.is_empty()).then(|| out.join(";")))
}

/// `_displays_as_block(css)`.
pub(crate) fn displays_as_block(css: Option<&Resolved>) -> Result<bool, ItemError> {
    Ok(keyword(css, "display")? == "block")
}

/// `_float_side(css)`: "left" or "right".
pub(crate) fn float_side(css: Option<&Resolved>) -> Result<Option<&'static str>, ItemError> {
    Ok(match keyword(css, "float")?.as_str() {
        "left" => Some("left"),
        "right" => Some("right"),
        _ => None,
    })
}

/// `_img_float(img, parent, size, style_resolver)`, given the picture's resolved style: (its
/// float side, its size hint). Its own float wins; alone in a floated `parent`, it floats with
/// it, at the parent's width when its own size is none or `w=100%`.
pub(crate) fn img_float(
    resolver: &Resolver,
    dom: &Dom,
    img: NodeId,
    img_css: Option<&Resolved>,
    parent: Option<NodeId>,
    mut size: Option<String>,
) -> Result<(Option<&'static str>, Option<String>), ItemError> {
    let side = float_side(img_css)?;
    let Some(parent) = parent.filter(|_| side.is_none()) else {
        return Ok((side, size));
    };
    let pcss = resolver.resolve(parent)?;
    let side = float_side(pcss.as_ref())?;
    // lxml's `len(parent)` and `parent[0]` count comments too.
    let only_image = dom.children(parent) == [img];
    if side.is_none() || !only_image {
        return Ok((None, size));
    }
    let shown = dom.itertext(parent);
    resolver.charge(shown.len())?;
    if !py::strip(&shown).is_empty() {
        return Ok((None, size));
    }
    let value = text_of(get(pcss.as_ref(), "width")?);
    let width = py::strip(&value);
    if !width.is_empty()
        && width != "auto"
        && is_img_size(width)
        && matches!(size.as_deref(), None | Some("w=100%"))
    {
        size = Some(format!("w={width}"));
    }
    Ok((side, size))
}

/// `_CSS_URL_RE`: `url\(\s*['"]?([^'")]+)`, `\s` being Python's white space.
static CSS_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"url\([\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]*['"]?([^'")]+)"#,
    )
    .unwrap()
});

/// `_css_background_image(elem, css)`: the href of a picture an element with nothing else in
/// it draws as a `no-repeat` background (#168), or None. A `data:` href is refused
/// ([`DATA_URI`]) before it is copied; `file` names the book file for the refusal.
pub fn css_background_image(
    dom: &Dom,
    elem: NodeId,
    css: Option<&Resolved>,
    file: &str,
) -> Result<Option<String>, ItemError> {
    // `if not css`: a resolved style always has keys.
    let Some(css) = css else {
        return Ok(None);
    };
    if !dom.children(elem).is_empty() || !py::strip(dom.text(elem)).is_empty() {
        return Ok(None);
    }
    if py::strip(&text_of(css.get("background-repeat")?)).to_lowercase() != "no-repeat" {
        return Ok(None);
    }
    let image = text_of(css.get("background-image")?);
    match css_url(&image) {
        Some(href) if is_data_uri(href) => Err(Error::Unsupported {
            feature: DATA_URI,
            file: file.to_owned(),
        }
        .into()),
        href => Ok(href.map(str::to_owned)),
    }
}

/// The first `url(…)` `_CSS_URL_RE` finds in a CSS value, stripped; None for none or an empty
/// one. Its backtracking is Python's: `url(   )` gives the last space, so nothing, and the
/// search does not go on to a later `url(`.
fn css_url(value: &str) -> Option<&str> {
    let m = CSS_URL.captures(value)?;
    let href = py::strip(m.get(1)?.as_str());
    (!href.is_empty()).then_some(href)
}

/// Whether `local`, lower-cased, is in `set`.
fn lower_in(local: &str, set: &[&str]) -> bool {
    let lower = local.to_lowercase();
    set.contains(&lower.as_str())
}

/// `_SVG_NON_RENDERED_CONTAINERS`: definitions, painted only where a `<use>` points.
const NOT_PAINTED: [&str; 6] = ["defs", "symbol", "mask", "clippath", "pattern", "marker"];

/// `_SVG_TEXT_SKIP`: those, and the parts whose text no reader sees.
const TEXT_SKIP: [&str; 12] = [
    "defs",
    "symbol",
    "mask",
    "clippath",
    "pattern",
    "marker",
    "title",
    "desc",
    "style",
    "script",
    "metadata",
    "foreignobject",
];

/// `_SVG_CONDITIONS`.
const CONDITIONS: [&str; 3] = ["systemLanguage", "requiredFeatures", "requiredExtensions"];

/// `_SVG_SHAPES`.
const SHAPES: [&str; 8] = [
    "path", "rect", "circle", "ellipse", "line", "polyline", "polygon", "use",
];

/// `n`'s child nodes, last first, to push on a stack.
fn kids_reversed(dom: &Dom, n: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    dom.children(n).iter().rev().copied()
}

/// `_local_tag(node.tag)`: None (here "") for a comment.
fn local(dom: &Dom, n: NodeId) -> &str {
    if dom.is_comment(n) { "" } else { dom.tag(n) }
}

/// `_svg_image_refs(svg)`'s hrefs: each `<image>` the `<svg>` draws (not one inside
/// definitions), its `xlink:href` or else `href`, in document order. kfxgen pairs each with an
/// empty alt, and takes the href as written, not resolved.
pub(crate) fn svg_image_refs(dom: &Dom, svg: NodeId) -> Vec<&str> {
    let mut refs = Vec::new();
    let mut stack: Vec<NodeId> = kids_reversed(dom, svg).collect();
    while let Some(n) = stack.pop() {
        let local = local(dom, n);
        if local.is_empty() || lower_in(local, &NOT_PAINTED) {
            continue;
        }
        if local == "image" {
            let href = dom
                .attr_ns(n, XLINK_NS, "href")
                .filter(|h| !h.is_empty())
                .or_else(|| dom.attr(n, "href"))
                .unwrap_or("");
            if !href.is_empty() {
                refs.push(href);
            }
        }
        stack.extend(kids_reversed(dom, n));
    }
    refs
}

/// `run(node)` in `_svg_text_paragraphs`: a `<text>`'s text, its `<tspan>`, `<textPath>` and
/// `<a>` children's within it (a space before one placed with `x` or `y`), every child's tail.
fn svg_run(dom: &Dom, text: NodeId) -> String {
    let mut s = dom.text(text).to_owned();
    // (node, next child, where its own `s` starts in `s`).
    let mut stack = vec![(text, 0usize, 0usize)];
    while let Some(&mut (node, ref mut next, start)) = stack.last_mut() {
        let Some(&child) = dom.children(node).get(*next) else {
            stack.pop();
            if !stack.is_empty() {
                s.push_str(dom.tail(node));
            }
            continue;
        };
        *next += 1;
        if matches!(local(dom, child), "tspan" | "textPath" | "a") {
            let placed = dom.attr(child, "x").is_some() || dom.attr(child, "y").is_some();
            if placed && s.len() > start && !s.chars().next_back().is_some_and(py::is_space) {
                s.push(' ');
            }
            let own = s.len();
            s.push_str(dom.text(child));
            stack.push((child, 0, own));
            // Its tail is added when it is done.
            continue;
        }
        s.push_str(dom.tail(child));
    }
    s
}

/// `_svg_text_paragraphs(svg)`: the words of each `<text>` the `<svg>` draws, with the
/// `<text>`, in document order (#231). A `<switch>` draws its first child with no condition,
/// else its first.
pub(crate) fn svg_text_paragraphs(dom: &Dom, svg: NodeId) -> Vec<(String, NodeId)> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = kids_reversed(dom, svg).collect();
    while let Some(node) = stack.pop() {
        let local = local(dom, node);
        if local.is_empty() || lower_in(local, &TEXT_SKIP) {
            continue;
        }
        if local == "text" {
            let text = words(&svg_run(dom, node));
            if !text.is_empty() {
                out.push((text, node));
            }
        } else if local == "switch" {
            let mut kids = dom
                .children(node)
                .iter()
                .copied()
                .filter(|&c| !self::local(dom, c).is_empty());
            let first = kids.clone().next();
            let plain = kids.find(|&c| {
                !CONDITIONS
                    .iter()
                    .any(|a| dom.attr(c, a).is_some_and(|v| !v.is_empty()))
            });
            stack.extend(plain.or(first));
        } else {
            stack.extend(kids_reversed(dom, node));
        }
    }
    out
}

/// `_svg_own_style(svg)[key]`: the last declaration of `key` in the `<svg>`'s `style`, lower
/// case; None when it has none.
fn svg_own_style(dom: &Dom, svg: NodeId, key: &str) -> Option<String> {
    let mut found = None;
    for decl in dom.attr(svg, "style").unwrap_or("").split(';') {
        let (k, v) = decl.split_once(':').unwrap_or((decl, ""));
        let (k, v) = (py::strip(k), py::strip(v));
        if !k.is_empty() && !v.is_empty() && k.to_lowercase() == key {
            found = Some(v.to_lowercase());
        }
    }
    found
}

/// `_svg_draws_shapes_only(svg)`: whether an `<svg>` draws shapes and nothing kfxgen takes
/// from it (no `<image>`, no `<text>`), and is not hidden by its own style or attributes.
/// kfxgen draws such an `<svg>` as a picture of its own (#709); the converter refuses it.
pub(crate) fn svg_draws_shapes_only(dom: &Dom, svg: NodeId) -> bool {
    if !svg_image_refs(dom, svg).is_empty() || !svg_text_paragraphs(dom, svg).is_empty() {
        return false;
    }
    let display = svg_own_style(dom, svg, "display")
        .unwrap_or_else(|| py::strip(dom.attr(svg, "display").unwrap_or("")).to_lowercase());
    let visibility = svg_own_style(dom, svg, "visibility")
        .unwrap_or_else(|| dom.attr(svg, "visibility").unwrap_or("").to_lowercase());
    if display == "none" || matches!(py::strip(&visibility), "hidden" | "collapse") {
        return false;
    }
    let mut stack: Vec<NodeId> = kids_reversed(dom, svg).collect();
    while let Some(n) = stack.pop() {
        let local = local(dom, n);
        // Qt draws no `<svg>` inside another.
        if local.is_empty() || lower_in(local, &NOT_PAINTED) || local == "svg" {
            continue;
        }
        if SHAPES.contains(&local) {
            return true;
        }
        stack.extend(kids_reversed(dom, n));
    }
    false
}

/// `_resolve_img_src(base_href, src)`: an `<img src>` as the path from the book's root the
/// manifest uses; `src` as written when it is empty, unsafe (a leading `..` aside: that one is
/// resolved) or resolves to nothing. A key only: no file is opened by it.
pub(crate) fn resolve_img_src(base_href: &str, src: &str) -> String {
    if src.is_empty() || (is_unsafe(src) && !src.starts_with("..")) {
        return src.to_owned();
    }
    match resolve_doc_path(base_href, src) {
        r if r.is_empty() => src.to_owned(),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_patterns_are_kfxgens() {
        for s in [
            "1",
            "12",
            "1.5",
            "50%",
            "200px",
            "2em",
            "10pt",
            "\u{661}\u{660}px",
            "3\n",
        ] {
            assert!(is_img_size(s), "{s:?}");
        }
        for s in [
            "", "1.", ".5", "1.5.", "1e3", "-1", "2ex", "1 px", "1px\n\n", "auto",
        ] {
            assert!(!is_img_size(s), "{s:?}");
        }
        for s in [
            "0", "-0", "+0.000", "1px", "-1.5em", ".5rem", "+2%", "0\n", "0px",
        ] {
            assert!(is_valign_length(s), "{s:?}");
        }
        for s in [
            "", "1", "0.", "-", "1.px", "px", ".px", "1 px", "0.0x", "00",
        ] {
            assert!(!is_valign_length(s), "{s:?}");
        }
    }

    /// Expected values from the pinned `_CSS_URL_RE` and `_svg_*` helpers under Python 3.14
    /// with lxml 6.0.2.
    #[test]
    fn url_pattern_backtracks_as_pythons() {
        let cases = [
            ("url(   )", None),
            ("url('') url(x.png)", Some("x.png")),
            ("url( \"scan.jpg\" )", Some("scan.jpg")),
            ("url(\"\")", None),
            ("url(a\nb)", Some("a\nb")),
            ("URL(x)", None),
            ("url(\u{3000}\u{3000}x)", Some("x")),
            ("url(\u{1c})", None),
            ("url(\u{1c}x)", Some("x")),
            ("none", None),
            ("url('a\"b)", Some("a")),
        ];
        for (value, want) in cases {
            assert_eq!(css_url(value), want, "{value:?}");
        }
    }

    fn svg(markup: &str) -> (Dom, NodeId) {
        let xml = format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body>{}\
             </body></html>",
            markup.replacen("<svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"", 1)
        );
        let dom = crate::dom::parse(xml.as_bytes()).unwrap();
        let first = std::iter::once(dom.root())
            .chain(dom.descendants())
            .find(|&n| dom.tag(n) == "svg")
            .unwrap();
        (dom, first)
    }

    #[test]
    fn svg_shapes_only_reads_its_own_style_and_attributes_as_kfxgen() {
        let cases = [
            ("<svg style=\"Display : NONE\"><rect/></svg>", false),
            (
                "<svg style=\"display:none;display:block\"><rect/></svg>",
                true,
            ),
            ("<svg display=\" None \"><rect/></svg>", false),
            ("<svg style=\"visibility: collapse\"><rect/></svg>", false),
            (
                "<svg visibility=\"visible\" style=\"visibility:hidden\"><rect/></svg>",
                false,
            ),
            ("<svg visibility=\" Hidden \"><circle/></svg>", false),
            ("<svg><g><Rect/></g></svg>", false),
            ("<svg><mar\u{212a}er><rect/></mar\u{212a}er></svg>", false),
            ("<svg><clipPath><rect/></clipPath><use/></svg>", true),
            ("<svg><a><image href=\"x\"/></a><rect/></svg>", false),
            ("<svg><text> </text><rect/></svg>", true),
            ("<svg><defs><path/></defs></svg>", false),
            ("<svg><svg><rect/></svg></svg>", false),
        ];
        for (markup, want) in cases {
            let (dom, root) = svg(markup);
            assert_eq!(svg_draws_shapes_only(&dom, root), want, "{markup}");
        }
    }

    #[test]
    fn data_uris_are_told_as_kfxgen_and_calibre_tell_them() {
        for src in [
            "data:image/png;base64,AAAA",
            "DATA:image/png,x",
            "Data:",
            "data:",
        ] {
            assert!(is_data_uri(src), "{src:?}");
        }
        for src in [
            " data:image/png,x",
            "\u{a0}data:x",
            "%64ata:x",
            "data",
            "dat\u{e9}:x",
            "",
        ] {
            assert!(!is_data_uri(src), "{src:?}");
        }
    }

    #[test]
    fn tokens_strip_their_own_delimiters() {
        assert_eq!(
            img_token("a b", "x y\u{1}z", None, None, Some("va=top")),
            "\0IMG\u{1}a b\u{1}x\u{2}yz\u{1}\u{1}\u{1}va=top\0"
        );
        assert_eq!(
            img_token("p\0.png", "", None, None, None),
            "\0IMG\u{1}p.png\u{1}\0"
        );
        assert_eq!(
            img_token("p.png", "", Some("w=5px"), Some("left"), None),
            "\0IMG\u{1}p.png\u{1}\u{1}w=5px\u{1}left\0"
        );
    }

    #[test]
    fn img_sources_resolve_as_kfxgen() {
        let base = "OEBPS/text/c1.xhtml";
        assert_eq!(
            resolve_img_src(base, "../images/a.png"),
            "OEBPS/images/a.png"
        );
        assert_eq!(resolve_img_src(base, "a.png"), "OEBPS/text/a.png");
        assert_eq!(resolve_img_src(base, ""), "");
        // Unsafe, or leaving the book: as written.
        for src in [
            "/etc/passwd",
            "file:///etc/passwd",
            "data:image/png;base64,AAAA",
            "http://example.com/a.png",
            "C:/a.png",
            "../../../etc/passwd",
            "%252e%252e/%252e%252e/x.png",
        ] {
            assert_eq!(resolve_img_src(base, src), src, "{src}");
        }
    }
}
