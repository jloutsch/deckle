// SPDX-License-Identifier: GPL-3.0-only
//! Lists, at kfxgen `ad38ec8`: the typed markers of #201 (`_list_ordinals`,
//! `_list_style_type`, `_list_marker`, `_format_ordinal`, `_to_roman`, `_to_alpha`,
//! `_parse_int`, `_already_numbered`, `_prefix_marker`, the walk's `_take_marker` and the `li`
//! branch of `_walk_flow`), and the native lists of #768 (`_native_list`, `_list_settings`,
//! `_side_share`, `_list_box`, `_walk_list_item`, `_make_list`).
//!
//! A list written as Kindle Previewer writes one becomes a `box` block holding its items'
//! blocks (`box.list`, or `box.plain` for a list with no markers); every other list keeps
//! each item's marker typed into its text.
//!
//! Numbers: Python's int has no bound. `start` and `value` are read exactly within an i128
//! ([`Int`]); a number past that is [`Int::Big`], which no head of 16 characters can spell
//! (`_already_numbered` is false for it, as in Python). A number that must be written past
//! what the port can hold stops the file by name: a marker past an i128, a model value past
//! an i64 ([`TOO_BIG`]).

use std::collections::{HashMap, HashSet};

use super::spacing::{Pad, font_ratio, has_real_text, padding_em};
use super::walk::{HORIZONTAL, Walker};
use super::{Block, dict_set, s, unsupported};
use crate::convert::ItemError;
use crate::convert::inline::{keyword, text_of};
use crate::convert::inline_style::{Raised, Span};
use crate::convert::resolver::Resolved;
use crate::css::TEXT_UNIT;
use crate::dom::{Dom, NodeId};
use crate::generate::page_scale::em_percent;
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `_V_AUTHOR`: the mark on an element the book sets in vertical text.
const V_AUTHOR: &str = "data-kfxgen-v-author";
/// `f"{_BULLET} "`: a bullet item's marker.
const BULLET_MARKER: &str = "• ";
/// `_GREEK`.
const GREEK: [char; 24] = [
    'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π', 'ρ', 'σ', 'τ',
    'υ', 'φ', 'χ', 'ψ', 'ω',
];
/// `_ROMAN`.
const ROMAN: [(i128, &str); 13] = [
    (1000, "m"),
    (900, "cm"),
    (500, "d"),
    (400, "cd"),
    (100, "c"),
    (90, "xc"),
    (50, "l"),
    (40, "xl"),
    (10, "x"),
    (9, "ix"),
    (5, "v"),
    (4, "iv"),
    (1, "i"),
];
/// The name a list number too large to write stops the file with.
const TOO_BIG: &str = "list numbers too large";

/// `_LIST_TYPE_ATTR`: an `<ol type>` or `<li type>` value as its list-style-type. Case matters.
fn list_type_attr(v: &str) -> Option<&'static str> {
    Some(match v {
        "1" => "decimal",
        "a" => "lower-alpha",
        "A" => "upper-alpha",
        "i" => "lower-roman",
        "I" => "upper-roman",
        "disc" => "disc",
        "circle" => "circle",
        "square" => "square",
        _ => return None,
    })
}

/// `_BULLET_TYPES`.
fn is_bullet_type(style: &str) -> bool {
    matches!(style, "disc" | "circle" | "square")
}

/// `_LIST_STYLE_SYMBOLS`: a list's `$100` marker style, as Previewer writes it.
fn list_style_symbol(style: &str) -> Option<&'static str> {
    Some(match style {
        "disc" => "$340",
        "square" => "$341",
        "circle" => "$342",
        "decimal" => "$343",
        "lower-roman" => "$344",
        "upper-roman" => "$345",
        "lower-alpha" | "lower-latin" => "$346",
        "upper-alpha" | "upper-latin" => "$347",
        "lower-greek" => "$791",
        "decimal-leading-zero" => "$796",
        "armenian" => "$794",
        _ => return None,
    })
}

/// `_LIST_BOX`: a list's or an item's box, no border, fill, padding, width or float.
fn list_box_fields() -> Vec<(PyValue, PyValue)> {
    [
        ("border", PyValue::None),
        ("background", PyValue::None),
        ("padding", PyValue::None),
        ("font_px", PyValue::None),
        ("margin_left", PyValue::None),
        ("margin_right", PyValue::None),
        ("width", PyValue::None),
        ("width_px", PyValue::None),
        ("float", PyValue::None),
        ("centred", PyValue::Bool(false)),
        ("bidi", PyValue::None),
    ]
    .into_iter()
    .map(|(k, v)| (s(k), v))
    .collect()
}

/// A Python int as the list code holds one: exact within an i128, or past it (`int()` reads
/// at most 4300 digits, so such a number has at least 39 and at most 4300).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Int {
    Exact(i128),
    Big,
}

impl Int {
    /// `n + step`: past an i128 stays past it (coming back would take more than 10^38 items).
    fn plus(self, step: i128) -> Int {
        match self {
            Int::Exact(n) => n.checked_add(step).map_or(Int::Big, Int::Exact),
            Int::Big => Int::Big,
        }
    }
}

/// `_parse_int(value)`: `int(str(value).strip())`, None where that raises (`str(None)` is
/// `"None"`).
fn parse_int(value: Option<&str>) -> Option<Int> {
    let v = py::strip(value?);
    py::py_int_wide(v).map(|r| r.map_or(Int::Big, Int::Exact))
}

/// An [`Int`] written into the model, which holds an i64.
fn model_int(n: Int) -> Option<PyValue> {
    match n {
        Int::Exact(n) => i64::try_from(n).ok().map(PyValue::Int),
        Int::Big => None,
    }
}

/// `_to_roman(n)`, for `0 < n < 4000`.
fn to_roman(mut n: i128) -> String {
    let mut out = String::new();
    for (value, numeral) in ROMAN {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

/// `_to_alpha(n, letters)`: bijective base `letters.len()`, the CSS alphabetic system. At most
/// 28 letters for an i128.
fn to_alpha(mut n: i128, letters: &[char]) -> String {
    let base = letters.len() as i128;
    let mut out = Vec::new();
    while n > 0 {
        let rem = (n - 1).rem_euclid(base);
        n = (n - 1).div_euclid(base);
        // `rem` is in `0..base`.
        out.push(letters[usize::try_from(rem).unwrap_or(0)]);
    }
    out.iter().rev().collect()
}

const LOWER: [char; 26] = [
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z',
];

/// `_format_ordinal(n, style)`: `n` as the counter text of `style`, decimal outside a style's
/// range and for a style it does not know. None for a number past an i128.
fn format_ordinal(n: Int, style: &str) -> Option<String> {
    let Int::Exact(n) = n else {
        return None;
    };
    if style == "decimal-leading-zero" {
        return Some(if n >= 0 {
            format!("{n:02}")
        } else {
            format!("-{:02}", n.unsigned_abs())
        });
    }
    if n > 0 {
        match style {
            "lower-alpha" | "lower-latin" => return Some(to_alpha(n, &LOWER)),
            "upper-alpha" | "upper-latin" => return Some(to_alpha(n, &LOWER).to_uppercase()),
            "lower-greek" => return Some(to_alpha(n, &GREEK)),
            _ => {}
        }
        if n < 4000 {
            match style {
                "lower-roman" => return Some(to_roman(n)),
                "upper-roman" => return Some(to_roman(n).to_uppercase()),
                _ => {}
            }
        }
    }
    Some(n.to_string())
}

/// `text.lstrip()[:16]`: all `_already_numbered` and `_TYPED_BULLET_RE` read of a text.
fn head(text: &str) -> &str {
    py::take(py::lstrip(text), 16)
}

/// `head` with one `(` or `[` taken off its front: `[(\[]?` before a form that starts with
/// neither (backtracking to no bracket cannot match then).
fn unbracket(head: &str) -> &str {
    head.strip_prefix(['(', '[']).unwrap_or(head)
}

/// `_already_numbered(text, n)` on `head`: the text opens with item number `n` written by
/// hand, in decimal (with leading zeros, then `.`, `)`, `:`, `]`, white space or the end of
/// the head), or as its letter or roman numeral followed by `.`, `)` or `]`.
fn numbered_head(head: &str, n: Int) -> bool {
    // A number past an i128 has at least 39 digits and 28 letters: none fits in a head of
    // 16 characters.
    let Int::Exact(n) = n else {
        return false;
    };
    let rest = unbracket(head);
    // `0*{n}`: the zeros taken greedily; only for 0 can giving one back help.
    let zeros = rest.len() - rest.trim_start_matches('0').len();
    let after = if n == 0 {
        (zeros > 0).then(|| &rest[zeros..])
    } else {
        rest[zeros..].strip_prefix(n.to_string().as_str())
    };
    let ends = |a: &str| match a.chars().next() {
        None => true,
        Some(c) => matches!(c, '.' | ')' | ':' | ']') || py::is_space(c),
    };
    if after.is_some_and(ends) {
        return true;
    }
    if n <= 0 {
        return false;
    }
    let lowered = head.to_lowercase();
    let rest = unbracket(&lowered);
    let mut forms = vec![to_alpha(n, &LOWER)];
    if n < 4000 {
        forms.push(to_roman(n));
    }
    forms.iter().any(|f| {
        rest.strip_prefix(f.as_str())
            .and_then(|a| a.chars().next())
            .is_some_and(|c| matches!(c, '.' | ')' | ']'))
    })
}

/// `_already_numbered(text, n)`.
fn already_numbered(text: &str, n: Int) -> bool {
    numbered_head(head(text), n)
}

/// `_TYPED_BULLET_RE.match(text)` on `head`: a bullet glyph, or a dash or asterisk with white
/// space after it, past any white space.
fn typed_bullet_head(head: &str) -> bool {
    let mut chars = head.chars();
    match chars.next() {
        Some('•' | '◦' | '▪' | '▫' | '■' | '□' | '●' | '○' | '‣' | '⁃' | '·') => {
            true
        }
        Some('-' | '–' | '—' | '*') => chars.next().is_some_and(py::is_space),
        _ => false,
    }
}

/// `_TYPED_BULLET_RE.match(text)`.
fn typed_bullet(text: &str) -> bool {
    typed_bullet_head(head(text))
}

/// `_list_style_type(li, css)`: the list-style-type that governs `li`, keywords lowercased, a
/// quoted string as it is.
fn list_style_type(dom: &Dom, li: NodeId, css: Option<&Resolved>) -> Result<String, ItemError> {
    if let Some(t) = dom
        .attr(li, "type")
        .and_then(|own| list_type_attr(py::strip(own)))
    {
        return Ok(t.to_owned());
    }
    let computed = match css {
        Some(css) => css.get("list-style-type")?,
        None => None,
    };
    // `if computed:` then `str(computed).strip()`.
    let computed = text_of(computed);
    if !computed.is_empty() {
        let computed = py::strip(&computed);
        // A quoted string is the marker. `""[:1] in "\"'"` is true, so a value of only white
        // space is an empty marker, not none (Python's `in` on strings).
        return Ok(
            if computed.is_empty() || computed.starts_with(['"', '\'']) {
                computed.to_owned()
            } else {
                computed.to_lowercase()
            },
        );
    }
    // A comment has no children, and its tag is empty.
    if let Some(parent) = dom.parent(li)
        && dom.tag(parent) == "ol"
    {
        let t = py::strip(dom.attr(parent, "type").unwrap_or(""));
        return Ok(list_type_attr(t).unwrap_or("decimal").to_owned());
    }
    Ok("disc".to_owned())
}

/// A list marker waiting for its item's text: `(marker text, ordinal or None)`. A number is
/// written only when a block takes it, so one that never shows is never formatted.
#[derive(Debug)]
pub(super) enum Marker {
    Text(String),
    Number { n: Int, style: String },
}

impl Marker {
    /// The marker's text: `f"{_format_ordinal(n, style)}. "` for a number.
    fn text(&self) -> Option<String> {
        match self {
            Marker::Text(t) => Some(t.clone()),
            Marker::Number { n, style } => format_ordinal(*n, style).map(|o| o + ". "),
        }
    }
}

/// `_list_marker(li, css, ordinals)`: `li`'s marker, or None when it shows none.
fn list_marker(
    dom: &Dom,
    li: NodeId,
    css: Option<&Resolved>,
    ordinals: &HashMap<NodeId, Int>,
) -> Result<Option<Marker>, ItemError> {
    let display = keyword(css, "display")?;
    if !display.is_empty() && !display.contains("list-item") {
        return Ok(None); // `li { display: block }` suppresses the marker
    }
    let style = list_style_type(dom, li, css)?;
    if style == "none" {
        return Ok(None);
    }
    if style.is_empty() || style.starts_with(['"', '\'']) {
        // `style[1:-1]`.
        let mut chars = style.chars();
        chars.next();
        chars.next_back();
        return Ok(Some(Marker::Text(chars.as_str().to_owned())));
    }
    if is_bullet_type(&style) {
        return Ok(Some(Marker::Text(BULLET_MARKER.to_owned())));
    }
    Ok(ordinals.get(&li).map(|&n| Marker::Number { n, style }))
}

/// `_side_share(value, ratio)`: a side margin or padding as a share of the page, in %; an em
/// the element's own, a negative one counted; 0 when unset or unreadable.
fn side_share(value: &str, ratio: f64) -> Result<f64, Raised> {
    let value = py::strip(value);
    let sign = if value.starts_with('-') { -1.0 } else { 1.0 };
    Ok(match padding_em(value.trim_start_matches('-'), ratio)? {
        None => 0.0,
        Some(Pad::Pct(n)) => sign * n,
        Some(Pad::Em(em)) => sign * em * em_percent(),
    })
}

/// `x or None` for a float.
fn float_or_none(x: f64) -> PyValue {
    if x != 0.0 || x.is_nan() {
        PyValue::Float(x)
    } else {
        PyValue::None
    }
}

/// `_native_list(lst)`'s settings: how Previewer writes the list.
#[derive(Debug)]
pub(super) enum Native {
    /// `{"none": True, "pad": …, "right": …}`: no markers, a plain container indented by the
    /// list's margins and padding.
    Plain { pad: PyValue, right: PyValue },
    /// A `$276` list: its `$100` type, its `start` when not 1, `inside`, and per item its
    /// `value` and ordinal.
    Marked {
        kind: &'static str,
        start: Option<Int>,
        inside: bool,
        values: HashMap<NodeId, Option<Int>>,
        ordinals: HashMap<NodeId, Int>,
        bullets: bool,
    },
}

/// The list state of one file's walk: kfxgen's `pending_markers`, `list_ordinals`,
/// `native_lists`, `cell_depth`.
#[derive(Default)]
pub(super) struct Lists {
    /// Markers of items that have shown no text yet, the outermost first.
    pub(super) pending: Vec<Marker>,
    /// Per list element, `_list_ordinals` of it, for typed markers.
    ordinals: HashMap<NodeId, HashMap<NodeId, Int>>,
    /// Per list element, `_list_settings` of it.
    native: HashMap<NodeId, Option<Native>>,
    /// How many table cells the walk is in: lists in cells keep typed markers. Task 16's
    /// `_walk_cell` raises it.
    pub(super) cell_depth: usize,
}

/// What the `li` branch of `_walk_flow` does around `_walk_element(li)`.
pub(super) enum Item {
    /// `_walk_list_item(li, native)`: the item of the native list `list`.
    Native { list: NodeId },
    /// The item's marker pushed; the markers waiting before it were `before` long.
    Marked { before: usize },
    /// No marker: the item walked as any element.
    Unmarked,
}

impl Walker<'_, '_, '_, '_> {
    /// `_list_ordinals(list_elem, style_resolver)`: each shown `li` child's number, from
    /// `start` (or 1, or the count when `reversed`), a `value` restarting the count.
    fn list_ordinals(&mut self, list: NodeId) -> Result<HashMap<NodeId, Int>, ItemError> {
        let dom = self.dom;
        let mut items = Vec::new();
        for &c in dom.children(list) {
            if !dom.is_comment(c) && dom.tag(c) == "li" && !self.w.hidden(c)? {
                items.push(c);
            }
        }
        let reversed = dom.attr(list, "reversed").is_some();
        let start = parse_int(dom.attr(list, "start")).unwrap_or_else(|| {
            Int::Exact(if reversed {
                // At most the file's node count.
                i128::try_from(items.len()).unwrap_or(i128::MAX)
            } else {
                1
            })
        });
        let step = if reversed { -1 } else { 1 };
        let mut ordinals = HashMap::with_capacity(items.len());
        let mut n = start;
        for li in items {
            if let Some(value) = parse_int(dom.attr(li, "value")) {
                n = value;
            }
            ordinals.insert(li, n);
            n = n.plus(step);
        }
        Ok(ordinals)
    }

    /// The `li` branch of `_walk_flow(li)` up to its `_walk_element(li)`, for an `li` that is
    /// not hidden: a native list's item, or its marker pushed.
    pub(super) fn list_item(&mut self, li: NodeId) -> Result<Item, ItemError> {
        let dom = self.dom;
        // An `li` is always below the `<body>`.
        let Some(parent) = dom.parent(li) else {
            return Ok(Item::Unmarked);
        };
        if self.native_list(parent)?.is_some() {
            return Ok(Item::Native { list: parent });
        }
        // Worked out once per list (kfxgen's memo), taken out of the map while in use.
        let ordinals = match self.lists.ordinals.remove(&parent) {
            Some(ordinals) => ordinals,
            None => self.list_ordinals(parent)?,
        };
        let css = self.resolve(li)?;
        let entry = list_marker(dom, li, css.as_ref(), &ordinals)?;
        drop(css);
        self.lists.ordinals.insert(parent, ordinals);
        let Some(entry) = entry else {
            return Ok(Item::Unmarked);
        };
        // An outer item that has shown no text yet (`<li><ol><li>…`) keeps its marker ahead
        // of this one.
        let before = self.lists.pending.len();
        self.lists.pending.push(entry);
        Ok(Item::Marked { before })
    }

    /// The end of the `li` branch of `_walk_flow`: an item that held no text to carry its
    /// marker drops it, so it does not leak onto what follows the list.
    ///
    /// kfxgen copies the markers before the item (`before = pending_markers[:]`) and puts the
    /// copy back. Markers are only pushed by items, each taking its own back off, and only
    /// taken all at once, so the list is then exactly `before` and this item's marker: it is
    /// cut back to `before` entries, with no copy.
    pub(super) fn marker_end(&mut self, before: usize) {
        if !self.lists.pending.is_empty() {
            self.lists.pending.truncate(before);
        }
    }

    /// `_take_marker(text, spans, mark_offsets)`: the waiting markers put in front of a text
    /// block, unless the text has no words (a picture alone keeps them waiting) or types its
    /// own number or bullet.
    pub(super) fn take_marker(
        &mut self,
        text: &mut String,
        spans: &mut [Span],
        marks: &mut [(String, usize)],
    ) -> Result<(), ItemError> {
        if self.lists.pending.is_empty() || !has_real_text(text) {
            return Ok(());
        }
        let mut entries = std::mem::take(&mut self.lists.pending);
        let Some(last) = entries.pop() else {
            return Ok(());
        };
        let own = match &last {
            Marker::Number { n, .. } if already_numbered(text, *n) => None,
            // A typed bullet is the marker; only a bullet yields.
            Marker::Text(t) if t == BULLET_MARKER && typed_bullet(text) => None,
            m => Some(m),
        };
        let mut marker = String::new();
        for m in entries.iter().chain(own) {
            let t = m.text().ok_or_else(|| unsupported(self.w, TOO_BIG))?;
            // Each marker is written once: what an item's style gave it, charged.
            self.resolver.charge(t.len())?;
            marker.push_str(&t);
        }
        if marker.is_empty() {
            return Ok(());
        }
        prefix_marker(&marker, text, spans, marks);
        Ok(())
    }

    /// `_native_list(lst)`: the list's settings when it is written as Previewer writes it,
    /// worked out once per list.
    pub(super) fn native_list(&mut self, list: NodeId) -> Result<Option<&Native>, ItemError> {
        if !self.lists.native.contains_key(&list) {
            let settings = self.list_settings(list)?;
            self.lists.native.insert(list, settings);
        }
        Ok(self.lists.native.get(&list).and_then(Option::as_ref))
    }

    /// `_list_settings(lst)`: None for a list Previewer was not probed on (in a cell, in
    /// vertical text, `reversed`, holding anything but items, with an item that hides its
    /// marker or types its own, or with mixed or unknown styles).
    fn list_settings(&mut self, list: NodeId) -> Result<Option<Native>, ItemError> {
        let dom = self.dom;
        // `style_resolver is None` cannot be: a file always has a resolver here.
        // A comment's tag is empty, so the tag test leaves it out.
        if self.lists.cell_depth > 0
            || !matches!(dom.tag(list), "ul" | "ol")
            || dom.attr(list, "reversed").is_some()
        {
            return Ok(None);
        }
        let css = self.resolve(list)?;
        let turned = |n: NodeId| dom.attr(n, V_AUTHOR).is_some();
        if dom
            .ancestors(list)
            .any(|e| turned(e) || dom.attr(e, HORIZONTAL).is_some())
            || turned(list)
        {
            // In a vertical file, vertical or in a horizontal block of one: typed markers.
            return Ok(None);
        }
        let kids: Vec<NodeId> = dom
            .children(list)
            .iter()
            .copied()
            .filter(|&c| !dom.is_comment(c))
            .collect();
        if kids.is_empty() || kids.iter().any(|&c| dom.tag(c) != "li") {
            return Ok(None);
        }
        let mut items = Vec::with_capacity(kids.len());
        for c in kids {
            if !self.w.hidden(c)? {
                items.push(c);
            }
        }
        if items.is_empty() {
            return Ok(None);
        }
        let ordinals = self.list_ordinals(list)?;
        let mut kinds: HashSet<String> = HashSet::new();
        for &li in &items {
            let li_css = self.resolve(li)?;
            let display = keyword(li_css.as_ref(), "display")?;
            if !display.is_empty() && !display.contains("list-item") {
                return Ok(None);
            }
            let kind = list_style_type(dom, li, li_css.as_ref())?;
            drop(li_css);
            if kind != "none" {
                // `"".join(li.itertext())`: only its first characters are read.
                let text = self.itertext_head(li)?;
                if is_bullet_type(&kind) {
                    if typed_bullet_head(&text) {
                        return Ok(None);
                    }
                } else {
                    let n = ordinals.get(&li).copied().unwrap_or(Int::Exact(0));
                    if numbered_head(&text, n) {
                        return Ok(None);
                    }
                }
            }
            kinds.insert(kind);
        }
        if kinds.len() == 1 && kinds.contains("none") {
            let bidi = self.bidi.bidi(dom, self.resolver, list)?;
            if bidi
                .iter()
                .any(|(k, v)| k.as_str() == Some("direction") && v.as_str() == Some("rtl"))
            {
                // In rtl text Previewer writes the items as plain paragraphs.
                return Ok(None);
            }
            // No markers: a plain container, indented by the list's margin and its padding,
            // 40px unless the book sets one.
            let ratio = font_ratio(css.as_ref());
            let pad = match &css {
                Some(css) => match css.get("padding-sides")? {
                    Some(v) if v.is_truthy() => match v {
                        PyValue::Tuple(sides) | PyValue::List(sides) => match sides.get(3) {
                            Some(p) => p.clone(),
                            None => return Err(PADDING_INDEX.into()),
                        },
                        // Never a string or other value: calibre writes the four sides.
                        _ => PyValue::None,
                    },
                    _ => PyValue::None,
                },
                None => PyValue::None,
            };
            let get = |key: &str| -> Result<String, ItemError> {
                Ok(match &css {
                    Some(css) => text_of(css.get(key)?).into_owned(),
                    None => String::new(),
                })
            };
            let pad = if matches!(pad, PyValue::None) {
                "40px".to_owned()
            } else {
                text_of(Some(&pad)).into_owned()
            };
            let left = side_share(&get("margin-left")?, ratio)? + side_share(&pad, ratio)?;
            let right = side_share(&get("margin-right")?, ratio)?;
            return Ok(Some(Native::Plain {
                pad: float_or_none(left),
                right: float_or_none(right),
            }));
        }
        let Some(kind) = kinds.iter().next().filter(|_| kinds.len() == 1) else {
            return Ok(None);
        };
        let Some(symbol) = list_style_symbol(kind) else {
            return Ok(None);
        };
        let start = parse_int(dom.attr(list, "start")).filter(|&n| n != Int::Exact(1));
        let inside = keyword(css.as_ref(), "list-style-position")? == "inside";
        let values = items
            .iter()
            .map(|&li| (li, parse_int(dom.attr(li, "value"))))
            .collect();
        Ok(Some(Native::Marked {
            kind: symbol,
            start,
            inside,
            values,
            ordinals,
            bullets: is_bullet_type(kind),
        }))
    }

    /// `"".join(li.itertext()).lstrip()[:16]`, reading only as far as that needs. Each node
    /// read is charged a unit and its text's bytes read: a list nested in items is read
    /// again for each list round it, as kfxgen reads it.
    fn itertext_head(&self, li: NodeId) -> Result<String, ItemError> {
        let dom = self.dom;
        let mut head = String::new();
        let mut taken = 0;
        let mut started = false;
        // (node, entering): an element's text and children on entering, its tail on leaving;
        // a comment gives only its tail.
        let mut stack = vec![(li, true)];
        while let Some((m, entering)) = stack.pop() {
            let text = if !entering || (m != li && dom.is_comment(m)) {
                dom.tail(m)
            } else {
                dom.text(m)
            };
            let mut read = 0;
            for c in text.chars() {
                read += c.len_utf8();
                if !started && py::is_space(c) {
                    continue;
                }
                started = true;
                head.push(c);
                taken += 1;
                if taken == 16 {
                    break;
                }
            }
            self.resolver.charge(TEXT_UNIT + read)?;
            if taken == 16 {
                break;
            }
            if entering && !dom.is_comment(m) {
                if m != li {
                    stack.push((m, false));
                }
                stack.extend(dom.children(m).iter().rev().map(|&c| (c, true)));
            }
        }
        Ok(head)
    }

    /// `_list_box(kids, box, font_px)`: a box block holding `kids`, their texts and ids, its
    /// `box` `_LIST_BOX` updated with `extra`.
    fn list_box(
        &mut self,
        kids: Vec<Block>,
        extra: Vec<(&'static str, PyValue)>,
        font_px: PyValue,
    ) -> Result<Block, ItemError> {
        let mut bx = list_box_fields();
        for (k, v) in extra {
            dict_set(&mut bx, k, v);
        }
        // Each list round a nested list copies its blocks' texts and ids once more: charged.
        self.box_block(kids, None, font_px, bx)
    }

    /// The end of `_walk_list_item(li, native)`: an item of one paragraph is that paragraph,
    /// marked `list_item`; any other is a box of its blocks. An item of a list with no
    /// markers is left as its blocks.
    pub(super) fn list_item_end(
        &mut self,
        li: NodeId,
        list: NodeId,
        start: usize,
    ) -> Result<(), ItemError> {
        let (value, n) = match self.lists.native.get(&list).and_then(Option::as_ref) {
            Some(Native::Marked {
                values,
                ordinals,
                bullets,
                ..
            }) => (
                values.get(&li).copied().flatten(),
                if *bullets {
                    None
                } else {
                    ordinals.get(&li).copied()
                },
            ),
            // Plain paragraphs in the list's container.
            _ => return Ok(()),
        };
        let as_model = |n: Option<Int>| match n {
            None => Some(PyValue::None),
            Some(n) => model_int(n),
        };
        let (Some(value), Some(n)) = (as_model(value), as_model(n)) else {
            return Err(unsupported(self.w, TOO_BIG));
        };
        let mut item = vec![(s("value"), value), (s("n"), n)];
        let start = start.min(self.blocks.len());
        let mut real = self.blocks[start..]
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind() != Some("space"))
            .map(|(i, b)| (start + i, b.kind().is_none()));
        match (real.next(), real.next()) {
            // An empty item is an empty `$277`, which shows its number.
            (None, _) => item.push((s("empty"), PyValue::Bool(true))),
            (Some((at, true)), None) => {
                self.blocks[at].set("list_item", PyValue::Dict(item));
                return Ok(());
            }
            _ => {}
        }
        let kids = self.blocks.split_off(start);
        let bx = self.list_box(kids, vec![("item", PyValue::Dict(item))], PyValue::None)?;
        self.blocks.push(bx);
        Ok(())
    }

    /// `_make_list(elem, start, native)`: the blocks of a native list's items in one `$276`
    /// box, its margins noted as a box's.
    pub(super) fn make_list(&mut self, elem: NodeId, start: usize) -> Result<(), ItemError> {
        let start = start.min(self.blocks.len());
        if !self.blocks[start..]
            .iter()
            .any(|b| b.kind() != Some("space"))
        {
            return Ok(());
        }
        let own = match self.lists.native.get(&elem).and_then(Option::as_ref) {
            Some(Native::Plain { pad, right }) => vec![
                ("plain", PyValue::Bool(true)),
                ("margin_left", pad.clone()),
                ("margin_right", right.clone()),
            ],
            Some(Native::Marked {
                kind,
                start: first,
                inside,
                ..
            }) => {
                let first = match first {
                    None => PyValue::None,
                    Some(n) => model_int(*n).ok_or_else(|| unsupported(self.w, TOO_BIG))?,
                };
                vec![(
                    "list",
                    PyValue::Dict(vec![
                        (s("type"), s(kind)),
                        (s("start"), first),
                        (s("inside"), PyValue::Bool(*inside)),
                    ]),
                )]
            }
            None => return Ok(()),
        };
        let css = self.resolve(elem)?;
        let font_px = css
            .as_ref()
            .and_then(Resolved::font_size)
            .cloned()
            .unwrap_or(PyValue::None);
        let bidi = self.bidi.bidi(self.dom, self.resolver, elem)?;
        let mut extra = vec![("font_px", font_px.clone()), ("bidi", PyValue::Dict(bidi))];
        extra.extend(own);
        let kids = self.blocks.split_off(start);
        let bx = self.list_box(kids, extra, font_px)?;
        self.blocks.push(bx);
        self.note_box_margins(start, css.as_ref())
    }
}

/// `pad = (css.get("padding-sides") or (None,) * 4)[3]` on fewer than four sides.
const PADDING_INDEX: Raised = Raised {
    exception: "IndexError",
    note: "_list_settings: padding-sides holding fewer than four values",
};

/// `_prefix_marker(marker, text, spans, mark_offsets)`: `marker` put in front of the text,
/// the spans and anchor offsets after it moved along; an anchor at the very start stays at
/// the start, which is now the marker.
fn prefix_marker(
    marker: &str,
    text: &mut String,
    spans: &mut [Span],
    marks: &mut [(String, usize)],
) {
    let shift = py::len(marker);
    for span in spans.iter_mut() {
        span.start += shift;
    }
    for (_, at) in marks.iter_mut() {
        if *at != 0 {
            *at += shift;
        }
    }
    text.insert_str(0, marker);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `_format_ordinal(n, style)` at the pin, run in Python.
    const FORMATS: &[(&str, i128, &str)] = &[
        ("decimal", 0, "0"),
        ("decimal", 1, "1"),
        ("decimal", -1, "-1"),
        ("decimal", -7, "-7"),
        ("decimal", 9, "9"),
        ("decimal", 10, "10"),
        ("decimal", 26, "26"),
        ("decimal", 27, "27"),
        ("decimal", 52, "52"),
        ("decimal", 53, "53"),
        ("decimal", 702, "702"),
        ("decimal", 703, "703"),
        ("decimal", 3999, "3999"),
        ("decimal", 4000, "4000"),
        ("decimal", 4001, "4001"),
        ("decimal", -4000, "-4000"),
        ("decimal", 9223372036854775807, "9223372036854775807"),
        ("decimal", 9223372036854775808, "9223372036854775808"),
        ("decimal", -9223372036854775808, "-9223372036854775808"),
        (
            "decimal",
            170141183460469231731687303715884105727,
            "170141183460469231731687303715884105727",
        ),
        (
            "decimal",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("decimal-leading-zero", 0, "00"),
        ("decimal-leading-zero", 1, "01"),
        ("decimal-leading-zero", -1, "-01"),
        ("decimal-leading-zero", -7, "-07"),
        ("decimal-leading-zero", 9, "09"),
        ("decimal-leading-zero", 10, "10"),
        ("decimal-leading-zero", 26, "26"),
        ("decimal-leading-zero", 27, "27"),
        ("decimal-leading-zero", 52, "52"),
        ("decimal-leading-zero", 53, "53"),
        ("decimal-leading-zero", 702, "702"),
        ("decimal-leading-zero", 703, "703"),
        ("decimal-leading-zero", 3999, "3999"),
        ("decimal-leading-zero", 4000, "4000"),
        ("decimal-leading-zero", 4001, "4001"),
        ("decimal-leading-zero", -4000, "-4000"),
        (
            "decimal-leading-zero",
            9223372036854775807,
            "9223372036854775807",
        ),
        (
            "decimal-leading-zero",
            9223372036854775808,
            "9223372036854775808",
        ),
        (
            "decimal-leading-zero",
            -9223372036854775808,
            "-9223372036854775808",
        ),
        (
            "decimal-leading-zero",
            170141183460469231731687303715884105727,
            "170141183460469231731687303715884105727",
        ),
        (
            "decimal-leading-zero",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("lower-alpha", 0, "0"),
        ("lower-alpha", 1, "a"),
        ("lower-alpha", -1, "-1"),
        ("lower-alpha", -7, "-7"),
        ("lower-alpha", 9, "i"),
        ("lower-alpha", 10, "j"),
        ("lower-alpha", 26, "z"),
        ("lower-alpha", 27, "aa"),
        ("lower-alpha", 52, "az"),
        ("lower-alpha", 53, "ba"),
        ("lower-alpha", 702, "zz"),
        ("lower-alpha", 703, "aaa"),
        ("lower-alpha", 3999, "ewu"),
        ("lower-alpha", 4000, "ewv"),
        ("lower-alpha", 4001, "eww"),
        ("lower-alpha", -4000, "-4000"),
        ("lower-alpha", 9223372036854775807, "crpxnlskvljfhg"),
        ("lower-alpha", 9223372036854775808, "crpxnlskvljfhh"),
        ("lower-alpha", -9223372036854775808, "-9223372036854775808"),
        (
            "lower-alpha",
            170141183460469231731687303715884105727,
            "aapobtfhvhxoisijpvtfajfpdqzw",
        ),
        (
            "lower-alpha",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("lower-latin", 0, "0"),
        ("lower-latin", 1, "a"),
        ("lower-latin", -1, "-1"),
        ("lower-latin", -7, "-7"),
        ("lower-latin", 9, "i"),
        ("lower-latin", 10, "j"),
        ("lower-latin", 26, "z"),
        ("lower-latin", 27, "aa"),
        ("lower-latin", 52, "az"),
        ("lower-latin", 53, "ba"),
        ("lower-latin", 702, "zz"),
        ("lower-latin", 703, "aaa"),
        ("lower-latin", 3999, "ewu"),
        ("lower-latin", 4000, "ewv"),
        ("lower-latin", 4001, "eww"),
        ("lower-latin", -4000, "-4000"),
        ("lower-latin", 9223372036854775807, "crpxnlskvljfhg"),
        ("lower-latin", 9223372036854775808, "crpxnlskvljfhh"),
        ("lower-latin", -9223372036854775808, "-9223372036854775808"),
        (
            "lower-latin",
            170141183460469231731687303715884105727,
            "aapobtfhvhxoisijpvtfajfpdqzw",
        ),
        (
            "lower-latin",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("upper-alpha", 0, "0"),
        ("upper-alpha", 1, "A"),
        ("upper-alpha", -1, "-1"),
        ("upper-alpha", -7, "-7"),
        ("upper-alpha", 9, "I"),
        ("upper-alpha", 10, "J"),
        ("upper-alpha", 26, "Z"),
        ("upper-alpha", 27, "AA"),
        ("upper-alpha", 52, "AZ"),
        ("upper-alpha", 53, "BA"),
        ("upper-alpha", 702, "ZZ"),
        ("upper-alpha", 703, "AAA"),
        ("upper-alpha", 3999, "EWU"),
        ("upper-alpha", 4000, "EWV"),
        ("upper-alpha", 4001, "EWW"),
        ("upper-alpha", -4000, "-4000"),
        ("upper-alpha", 9223372036854775807, "CRPXNLSKVLJFHG"),
        ("upper-alpha", 9223372036854775808, "CRPXNLSKVLJFHH"),
        ("upper-alpha", -9223372036854775808, "-9223372036854775808"),
        (
            "upper-alpha",
            170141183460469231731687303715884105727,
            "AAPOBTFHVHXOISIJPVTFAJFPDQZW",
        ),
        (
            "upper-alpha",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("upper-latin", 0, "0"),
        ("upper-latin", 1, "A"),
        ("upper-latin", -1, "-1"),
        ("upper-latin", -7, "-7"),
        ("upper-latin", 9, "I"),
        ("upper-latin", 10, "J"),
        ("upper-latin", 26, "Z"),
        ("upper-latin", 27, "AA"),
        ("upper-latin", 52, "AZ"),
        ("upper-latin", 53, "BA"),
        ("upper-latin", 702, "ZZ"),
        ("upper-latin", 703, "AAA"),
        ("upper-latin", 3999, "EWU"),
        ("upper-latin", 4000, "EWV"),
        ("upper-latin", 4001, "EWW"),
        ("upper-latin", -4000, "-4000"),
        ("upper-latin", 9223372036854775807, "CRPXNLSKVLJFHG"),
        ("upper-latin", 9223372036854775808, "CRPXNLSKVLJFHH"),
        ("upper-latin", -9223372036854775808, "-9223372036854775808"),
        (
            "upper-latin",
            170141183460469231731687303715884105727,
            "AAPOBTFHVHXOISIJPVTFAJFPDQZW",
        ),
        (
            "upper-latin",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("lower-greek", 0, "0"),
        ("lower-greek", 1, "α"),
        ("lower-greek", -1, "-1"),
        ("lower-greek", -7, "-7"),
        ("lower-greek", 9, "ι"),
        ("lower-greek", 10, "κ"),
        ("lower-greek", 26, "αβ"),
        ("lower-greek", 27, "αγ"),
        ("lower-greek", 52, "βδ"),
        ("lower-greek", 53, "βε"),
        ("lower-greek", 702, "αεζ"),
        ("lower-greek", 703, "αεη"),
        ("lower-greek", 3999, "ζχο"),
        ("lower-greek", 4000, "ζχπ"),
        ("lower-greek", 4001, "ζχρ"),
        ("lower-greek", -4000, "-4000"),
        ("lower-greek", 9223372036854775807, "κμνηηβτψμιυωφη"),
        ("lower-greek", 9223372036854775808, "κμνηηβτψμιυωφθ"),
        ("lower-greek", -9223372036854775808, "-9223372036854775808"),
        (
            "lower-greek",
            170141183460469231731687303715884105727,
            "ιεληιδχτσμφβφωμλοτψτψνβρνχεη",
        ),
        (
            "lower-greek",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("lower-roman", 0, "0"),
        ("lower-roman", 1, "i"),
        ("lower-roman", -1, "-1"),
        ("lower-roman", -7, "-7"),
        ("lower-roman", 9, "ix"),
        ("lower-roman", 10, "x"),
        ("lower-roman", 26, "xxvi"),
        ("lower-roman", 27, "xxvii"),
        ("lower-roman", 52, "lii"),
        ("lower-roman", 53, "liii"),
        ("lower-roman", 702, "dccii"),
        ("lower-roman", 703, "dcciii"),
        ("lower-roman", 3999, "mmmcmxcix"),
        ("lower-roman", 4000, "4000"),
        ("lower-roman", 4001, "4001"),
        ("lower-roman", -4000, "-4000"),
        ("lower-roman", 9223372036854775807, "9223372036854775807"),
        ("lower-roman", 9223372036854775808, "9223372036854775808"),
        ("lower-roman", -9223372036854775808, "-9223372036854775808"),
        (
            "lower-roman",
            170141183460469231731687303715884105727,
            "170141183460469231731687303715884105727",
        ),
        (
            "lower-roman",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("upper-roman", 0, "0"),
        ("upper-roman", 1, "I"),
        ("upper-roman", -1, "-1"),
        ("upper-roman", -7, "-7"),
        ("upper-roman", 9, "IX"),
        ("upper-roman", 10, "X"),
        ("upper-roman", 26, "XXVI"),
        ("upper-roman", 27, "XXVII"),
        ("upper-roman", 52, "LII"),
        ("upper-roman", 53, "LIII"),
        ("upper-roman", 702, "DCCII"),
        ("upper-roman", 703, "DCCIII"),
        ("upper-roman", 3999, "MMMCMXCIX"),
        ("upper-roman", 4000, "4000"),
        ("upper-roman", 4001, "4001"),
        ("upper-roman", -4000, "-4000"),
        ("upper-roman", 9223372036854775807, "9223372036854775807"),
        ("upper-roman", 9223372036854775808, "9223372036854775808"),
        ("upper-roman", -9223372036854775808, "-9223372036854775808"),
        (
            "upper-roman",
            170141183460469231731687303715884105727,
            "170141183460469231731687303715884105727",
        ),
        (
            "upper-roman",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("armenian", 0, "0"),
        ("armenian", 1, "1"),
        ("armenian", -1, "-1"),
        ("armenian", -7, "-7"),
        ("armenian", 9, "9"),
        ("armenian", 10, "10"),
        ("armenian", 26, "26"),
        ("armenian", 27, "27"),
        ("armenian", 52, "52"),
        ("armenian", 53, "53"),
        ("armenian", 702, "702"),
        ("armenian", 703, "703"),
        ("armenian", 3999, "3999"),
        ("armenian", 4000, "4000"),
        ("armenian", 4001, "4001"),
        ("armenian", -4000, "-4000"),
        ("armenian", 9223372036854775807, "9223372036854775807"),
        ("armenian", 9223372036854775808, "9223372036854775808"),
        ("armenian", -9223372036854775808, "-9223372036854775808"),
        (
            "armenian",
            170141183460469231731687303715884105727,
            "170141183460469231731687303715884105727",
        ),
        (
            "armenian",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("disc", 0, "0"),
        ("disc", 1, "1"),
        ("disc", -1, "-1"),
        ("disc", -7, "-7"),
        ("disc", 9, "9"),
        ("disc", 10, "10"),
        ("disc", 26, "26"),
        ("disc", 27, "27"),
        ("disc", 52, "52"),
        ("disc", 53, "53"),
        ("disc", 702, "702"),
        ("disc", 703, "703"),
        ("disc", 3999, "3999"),
        ("disc", 4000, "4000"),
        ("disc", 4001, "4001"),
        ("disc", -4000, "-4000"),
        ("disc", 9223372036854775807, "9223372036854775807"),
        ("disc", 9223372036854775808, "9223372036854775808"),
        ("disc", -9223372036854775808, "-9223372036854775808"),
        (
            "disc",
            170141183460469231731687303715884105727,
            "170141183460469231731687303715884105727",
        ),
        (
            "disc",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
        ("foo", 0, "0"),
        ("foo", 1, "1"),
        ("foo", -1, "-1"),
        ("foo", -7, "-7"),
        ("foo", 9, "9"),
        ("foo", 10, "10"),
        ("foo", 26, "26"),
        ("foo", 27, "27"),
        ("foo", 52, "52"),
        ("foo", 53, "53"),
        ("foo", 702, "702"),
        ("foo", 703, "703"),
        ("foo", 3999, "3999"),
        ("foo", 4000, "4000"),
        ("foo", 4001, "4001"),
        ("foo", -4000, "-4000"),
        ("foo", 9223372036854775807, "9223372036854775807"),
        ("foo", 9223372036854775808, "9223372036854775808"),
        ("foo", -9223372036854775808, "-9223372036854775808"),
        (
            "foo",
            170141183460469231731687303715884105727,
            "170141183460469231731687303715884105727",
        ),
        (
            "foo",
            -170141183460469231731687303715884105728,
            "-170141183460469231731687303715884105728",
        ),
    ];

    #[test]
    fn ordinals_are_written_as_kfxgen_writes_them() {
        for &(style, n, want) in FORMATS {
            assert_eq!(
                format_ordinal(Int::Exact(n), style).as_deref(),
                Some(want),
                "{style} {n}"
            );
        }
        // Past an i128 the port writes nothing: the caller refuses the file.
        assert_eq!(format_ordinal(Int::Big, "decimal"), None);
    }

    #[test]
    fn start_and_value_are_read_as_kfxgen_reads_them() {
        let int = |v: &str| parse_int(Some(v));
        let exact = |n: i128| Some(Int::Exact(n));
        assert_eq!(int("5"), exact(5));
        assert_eq!(int(" 5 "), exact(5));
        assert_eq!(int("+5"), exact(5));
        assert_eq!(int("-0"), exact(0));
        assert_eq!(int("1_000"), exact(1000));
        assert_eq!(int("_1"), None);
        assert_eq!(int("1__0"), None);
        assert_eq!(int("\u{663}"), exact(3));
        // `str.strip()` takes U+001F off, which `int()` alone refuses.
        assert_eq!(int("5\u{1f}"), exact(5));
        assert_eq!(int(" \u{1f}5"), exact(5));
        assert_eq!(int(""), None);
        assert_eq!(int("None"), None);
        assert_eq!(int("1.0"), None);
        assert_eq!(int("0x10"), None);
        assert_eq!(int("  -12  "), exact(-12));
        assert_eq!(int(&"9".repeat(20)), exact(99_999_999_999_999_999_999));
        assert_eq!(int(&format!("-{}", "9".repeat(40))), Some(Int::Big));
        assert_eq!(int(&"9".repeat(4300)), Some(Int::Big));
        // `int()` reads at most 4300 digits.
        assert_eq!(int(&"9".repeat(4301)), None);
        assert_eq!(parse_int(None), None);
    }

    #[test]
    fn a_number_typed_by_hand_is_seen_as_kfxgen_sees_it() {
        // (text, n, `_already_numbered(text, n)` at the pin)
        let cases: &[(&str, i128, bool)] = &[
            ("1. The note", 1, true),
            ("01. x", 1, true),
            ("(1) x", 1, true),
            ("[1] x", 1, true),
            ("1x", 1, false),
            ("10. x", 1, false),
            ("1", 1, true),
            ("0. x", 0, true),
            ("00 x", 0, true),
            ("0x", 0, false),
            ("(0)", 0, true),
            ("-3. x", -3, true),
            ("a. x", 1, true),
            ("A) x", 1, true),
            ("(b] x", 2, true),
            ("iv. x", 4, true),
            ("IV) x", 4, true),
            ("i went", 1, false),
            // The ninth letter.
            ("I. x", 9, true),
            ("ix. x", 9, true),
            ("aa. x", 27, true),
            // The Kelvin sign lowers to "k", the eleventh letter.
            ("\u{212a}. x", 11, true),
            ("\u{130}. x", 9, false),
            ("   2: x", 2, true),
            ("2\nx", 2, true),
            // Cut at 16 characters, the end of the head ends the number.
            ("1234567890123456789", 1_234_567_890_123_456, true),
            // Past an i64, in fifteen letters.
            ("zzzzzzzzzzzzzzz.", 1_744_349_715_977_154_962_390, true),
            ("\u{663}. x", 3, false),
            ("5", 5, true),
            ("5.", 5, true),
        ];
        for &(text, n, want) in cases {
            assert_eq!(already_numbered(text, Int::Exact(n)), want, "{text:?} {n}");
        }
        assert!(!already_numbered("zzzzzzzzzzzzzzz. x", Int::Big));
    }

    #[test]
    fn a_bullet_typed_by_hand_is_seen_as_kfxgen_sees_it() {
        let cases: &[(&str, bool)] = &[
            ("• x", true),
            ("•x", true),
            ("- x", true),
            ("-5 degrees", false),
            ("*emph", false),
            ("* x", true),
            ("  ◦ x", true),
            ("— x", true),
            ("–\u{a0}x", true),
            ("x", false),
        ];
        for &(text, want) in cases {
            assert_eq!(typed_bullet(text), want, "{text:?}");
        }
    }

    #[test]
    fn a_marker_moves_the_spans_and_anchors_after_it() {
        let mut text = "ab".to_owned();
        let mut spans = vec![Span {
            start: 0,
            len: 2,
            flags: Default::default(),
        }];
        let mut marks = vec![("a".to_owned(), 0), ("b".to_owned(), 1)];
        prefix_marker("1. ", &mut text, &mut spans, &mut marks);
        // `('1. ab', [(3, 2, …)], {'a': 0, 'b': 4})` at the pin.
        assert_eq!(text, "1. ab");
        assert_eq!((spans[0].start, spans[0].len), (3, 2));
        assert_eq!(marks, vec![("a".to_owned(), 0), ("b".to_owned(), 4)]);
    }
}
