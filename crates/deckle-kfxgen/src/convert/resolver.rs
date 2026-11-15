// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's per-file style resolver: `converter.py` `_build_style_resolver` and the helpers it
//! calls (`_font_size_px`, `_line_height_factor`, `_specified_inherited`, `_specified_colour`,
//! `_decoration_lines`, `_first_letter_css`, `_q_css`, `_book_height`, `_vertical_box`,
//! `_box_background`), at kfxgen `ad38ec8`. It answers what the block and inline walks ask of
//! an element's CSS, from the styles kfxgen's `Stylizer` gives it ([`Styles`]).
//!
//! Hostile input: kfxgen walks the ancestors for many of the values, and `_font_size_px`
//! recurses once per level. Here every walk up the tree is a loop that stops at the nearest
//! element already answered and gives each element on the way the answer, so each element is
//! walked through once per kind of walk, however many descendants ask: a file costs one pass.
//! The memos hold a few words per element.

use std::cell::{Cell, RefCell};
use std::sync::LazyLock;

use regex::Regex;

use super::inline_style::{Raised, css_rgb, is_transparent};
use crate::book::File;
use crate::css::{Profile, Styles, Value};
use crate::dom::{Dom, NodeId};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// An element's resolved CSS: `resolve(elem)`'s dict, its keys in kfxgen's order.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved(Vec<(&'static str, PyValue)>);

impl Resolved {
    /// `css.get(key)`: `Some(&PyValue::None)` for a key holding None, None for a key the dict
    /// does not have.
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.0.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// The dict as Python had it.
    pub fn to_py(&self) -> PyValue {
        PyValue::Dict(
            self.0
                .iter()
                .map(|(k, v)| (PyValue::Str((*k).to_owned()), v.clone()))
                .collect(),
        )
    }

    /// A dict of the given keys only, as kfxgen's tests hand `compute_block_style`.
    #[cfg(test)]
    pub(super) fn of(pairs: Vec<(&'static str, PyValue)>) -> Resolved {
        Resolved(pairs)
    }

    fn set(&mut self, key: &str, value: PyValue) {
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        }
    }
}

/// CSS px to the unit, for font-size (`_FONT_UNIT_PX`). The em is 16px (`_FONT_PX`).
const FONT_PX: f64 = 16.0;
const FONT_UNIT_PX: [(&str, f64); 7] = [
    ("px", 1.0),
    ("pt", 4.0 / 3.0),
    ("pc", 16.0),
    ("in", 96.0),
    ("cm", 96.0 / 2.54),
    ("mm", 96.0 / 25.4),
    ("q", 96.0 / 101.6),
];
/// `_FONT_KEYWORD_PX`.
const FONT_KEYWORD_PX: [(&str, f64); 8] = [
    ("xx-small", 9.0),
    ("x-small", 10.0),
    ("small", 13.0),
    ("medium", 16.0),
    ("large", 18.0),
    ("x-large", 24.0),
    ("xx-large", 32.0),
    ("xxx-large", 48.0),
];
/// `_FONT_SIZE_RE`. Its `$` would also match before a final newline; every value it is
/// given has been stripped.
static FONT_SIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]*\.?[0-9]+)(em|rem|ex|ch|%|[a-z]+)$").unwrap());

/// The tags whose own fill kfxgen draws (`_BOX_TAGS | _CELL_TAGS`).
const FILL_TAGS: [&str; 9] = [
    "div",
    "aside",
    "section",
    "blockquote",
    "article",
    "p",
    "pre",
    "td",
    "th",
];
/// `_BORDER_SIDES` (and `_SIDES`, the same four).
const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
/// `_VERTICAL_SLOTS`: the physical side each of kfxgen's slots takes in vertical-rl text.
fn vertical_side(slot: &str) -> &'static str {
    match slot {
        "top" => "right",
        "bottom" => "left",
        "left" => "top",
        _ => "bottom",
    }
}
const V_AUTHOR: &str = "data-kfxgen-v-author";
const V_UA: &str = "data-kfxgen-v-ua";
const VERTICAL: &str = "data-kfxgen-vertical";
/// `_SOURCE_LINK_COLOUR`.
const SOURCE_LINK_COLOUR: &str = "data-kfxgen-link-colour";

/// The properties `resolve` reads with `_specified_inherited`, each with its memo.
const INHERITED: [&str; 8] = [
    "line-height",
    "font-weight",
    "letter-spacing",
    "font-variant",
    "text-transform",
    "text-orientation",
    "text-emphasis-style",
    "writing-mode",
];

/// Where a walk up the ancestors ended: not walked yet, at no element, or at one.
#[derive(Clone, Copy, PartialEq)]
enum Memo {
    Unknown,
    Nowhere,
    At(NodeId),
}

/// What a walk up the ancestors does at one element.
enum Step {
    /// Ends here, with this element.
    Here,
    /// Ends here, at no element.
    Stop,
    /// Goes on to the parent (at the root: ends at no element).
    Up,
}

/// The fill behind an element (0xRRGGBB, or None for the page), or what `_box_background`
/// raised for the box it reached.
type Fill = Result<Option<i64>, Raised>;

fn memos<T: Copy>(n: usize, v: T) -> Box<[Cell<T>]> {
    (0..n).map(|_| Cell::new(v)).collect()
}

/// `_build_style_resolver`'s resolver for one spine file.
pub struct Resolver<'a> {
    dom: &'a Dom,
    styles: Styles<'a>,
    /// `_calibre_keyword_table`: the rem value Calibre writes for a keyword, and Kindle
    /// Previewer's px for it.
    keyword_px: Vec<(String, f64)>,
    /// `sizes`: each element's font size in CSS px.
    sizes: Box<[Cell<Option<f64>>]>,
    /// Each [`INHERITED`] property's nearest element declaring it.
    inherited: [Box<[Cell<Memo>]>; 8],
    /// `_line_height_factor`'s nearest element declaring a line height.
    line_height: Box<[Cell<Memo>]>,
    /// `_specified_colour`'s element whose link mark or colour wins.
    colour: Box<[Cell<Memo>]>,
    /// `_decoration_lines`: bit 0 underline, bit 1 line-through, bit 7 worked out.
    decoration: Box<[Cell<u8>]>,
    /// `_lang_of`: the nearest element with a `lang`.
    lang: Box<[Cell<Memo>]>,
    /// `_q_css`'s nearest element declaring `quotes`, short of a change of language.
    quotes: Box<[Cell<Memo>]>,
    /// `fills`: the fill behind each element, or what `_box_background` raised.
    fills: RefCell<Vec<Option<Fill>>>,
}

impl<'a> Resolver<'a> {
    /// `_build_style_resolver(oeb_book, item, log)` for `file`. `profile` is what Calibre's
    /// `Stylizer` measures with and the input profile Calibre rewrote font-size keywords with
    /// (both are Calibre's `default` in a kfxgen conversion: [`crate::css::DEFAULT_PROFILE`]).
    pub fn new(file: &'a File, profile: &'a Profile) -> Resolver<'a> {
        let n = file.dom.node_count();
        Resolver {
            dom: &file.dom,
            styles: file.styles(profile),
            keyword_px: keyword_table(profile),
            sizes: memos(n, None),
            inherited: std::array::from_fn(|_| memos(n, Memo::Unknown)),
            line_height: memos(n, Memo::Unknown),
            colour: memos(n, Memo::Unknown),
            decoration: memos(n, 0),
            lang: memos(n, Memo::Unknown),
            quotes: memos(n, Memo::Unknown),
            fills: RefCell::new(vec![None; n]),
        }
    }

    /// `resolve(elem)`: the element's resolved CSS, or None where kfxgen's `resolve` catches an
    /// exception (the fill behind it, `_box_background` on a colour Python's `float()` or
    /// `int()` refuses).
    pub fn resolve(&self, n: NodeId) -> Option<Resolved> {
        // The only part of `resolve`'s body that can raise; every value is worked out without
        // side effects, so the order does not matter.
        let behind = self.behind(n).ok()?;
        let tag = self.dom.tag(n);
        let own = |prop: &str| str_or_none(self.styles.get(n, prop));
        let inherited = |i: usize| str_or_none(self.specified_inherited(n, i));
        let computed = |prop: &str| self.computed(n, prop);
        let mut css = Resolved(Vec::with_capacity(48));
        let mut put = |k: &'static str, v: PyValue| css.0.push((k, v));
        put(
            "font-size",
            PyValue::Float(py::round(self.font_size_px(n), 3)),
        );
        put(
            "line-height",
            self.line_height_factor(n)
                .map_or(PyValue::None, PyValue::Float),
        );
        put("line-height-declared", inherited(0));
        put(
            "first-letter",
            if tag == "p" {
                self.first_letter_css(n)
            } else {
                PyValue::None
            },
        );
        put(
            "q-css",
            if tag == "q" {
                self.q_css(n)
            } else {
                PyValue::None
            },
        );
        put("float", own("float"));
        put("clear", own("clear"));
        put("text-align", computed("text-align"));
        put("own-text-align", own("text-align"));
        put("text-indent", own("text-indent"));
        put("margin-left", own("margin-left"));
        put("margin-right", own("margin-right"));
        put("margin-top", own("margin-top"));
        put("margin-bottom", own("margin-bottom"));
        put("font-family", computed("font-family"));
        put("font-weight", inherited(1));
        put("text-decoration", self.decoration_lines(n));
        put("letter-spacing", inherited(2));
        put("font-variant", inherited(3));
        put("text-transform", inherited(4));
        put("text-orientation", inherited(5));
        put("text-emphasis-style", inherited(6));
        put("text-combine-upright", own("text-combine-upright"));
        put(
            "writing-mode",
            if matches!(tag, "html" | "body") {
                inherited(7)
            } else {
                PyValue::None
            },
        );
        put("color", str_or_none(self.specified_colour(n)));
        put("font-style", computed("font-style"));
        put("vertical-align", own("vertical-align"));
        put("own-direction", own("direction"));
        put("own-unicode-bidi", own("unicode-bidi"));
        put("width", own("width"));
        put(
            "height",
            if tag == "hr" {
                // `_book_height`: calibre's own `height: 2px` for every <hr> is not the book's.
                match self.styles.get(n, "height") {
                    Some(v) if py::strip(v) == "2px" => PyValue::None,
                    v => str_or_none(v),
                }
            } else {
                own("height")
            },
        );
        put("background-image", own("background-image"));
        put("background-color", own("background-color"));
        put("background", own("background"));
        put(
            "background-behind",
            behind.map_or(PyValue::None, PyValue::Int),
        );
        put("background-repeat", own("background-repeat"));
        put("page-break-before", own("page-break-before"));
        put("page-break-after", own("page-break-after"));
        put("break-before", own("break-before"));
        put("break-after", own("break-after"));
        put("white-space", computed("white-space"));
        put("list-style-type", computed("list-style-type"));
        put("list-style-position", computed("list-style-position"));
        put("display", own("display"));
        put(
            "padding-sides",
            PyValue::Tuple(SIDES.map(|s| own(&format!("padding-{s}"))).to_vec()),
        );
        put("border-collapse", computed("border-collapse"));
        put(
            "border-widths",
            PyValue::Tuple(SIDES.map(|s| own(&format!("border-{s}-width"))).to_vec()),
        );
        put(
            "border-sides",
            PyValue::Tuple(
                SIDES
                    .map(|s| {
                        PyValue::Tuple(
                            ["style", "width", "color"]
                                .map(|p| computed(&format!("border-{s}-{p}")))
                                .to_vec(),
                        )
                    })
                    .to_vec(),
            ),
        );
        self.vertical_box(&mut css, n);
        Some(css)
    }

    /// `_computed_value(st, prop)`: `st[prop]`, or `st.get(prop)` where that raises.
    fn computed(&self, n: NodeId, prop: &str) -> PyValue {
        match self.styles.computed(n, prop) {
            Ok(Value::Pt(v)) => PyValue::Float(v),
            Ok(Value::Text(s)) => PyValue::Str(s),
            Ok(Value::Absent) => PyValue::None,
            Err(_) => str_or_none(self.styles.get(n, prop)),
        }
    }

    /// A walk up from `n` (`n` itself first) to where `step` ends it, kept in `memo` for every
    /// element on the way.
    fn nearest(
        &self,
        memo: &[Cell<Memo>],
        n: NodeId,
        step: impl Fn(NodeId) -> Step,
    ) -> Option<NodeId> {
        let mut chain = Vec::new();
        let mut cur = n;
        let found = loop {
            let known = memo[cur.index()].get();
            if known != Memo::Unknown {
                break known;
            }
            chain.push(cur);
            match step(cur) {
                Step::Here => break Memo::At(cur),
                Step::Stop => break Memo::Nowhere,
                Step::Up => match self.dom.parent(cur) {
                    Some(p) => cur = p,
                    None => break Memo::Nowhere,
                },
            }
        };
        for c in chain {
            memo[c.index()].set(found);
        }
        match found {
            Memo::At(at) => Some(at),
            _ => None,
        }
    }

    /// `_specified_inherited(stylizer, elem, INHERITED[i])`: the value on `n` or its nearest
    /// ancestor that sets it, as written; `inherit` skipped.
    fn specified_inherited(&self, n: NodeId, i: usize) -> Option<&'a str> {
        let prop = INHERITED[i];
        let decides = |a: NodeId| {
            self.styles
                .get(a, prop)
                .is_some_and(|v| !v.is_empty() && !is_inherit(v))
        };
        let at = self.nearest(&self.inherited[i], n, |a| {
            if decides(a) { Step::Here } else { Step::Up }
        })?;
        self.styles.get(at, prop)
    }

    /// `_specified_colour`: `_specified_inherited`'s colour, a link's `data-kfxgen-link-colour`
    /// mark in place of an element's colour; `inherit` and `currentColor` there take the colour
    /// around the link.
    fn specified_colour(&self, n: NodeId) -> Option<&'a str> {
        let at = self.nearest(&self.colour, n, |a| {
            let decides = match self.dom.attr(a, SOURCE_LINK_COLOUR) {
                None => self
                    .styles
                    .get(a, "color")
                    .is_some_and(|v| !v.is_empty() && !is_inherit(v)),
                Some(mark) => {
                    let mark = py::strip(mark).to_lowercase();
                    mark != "inherit" && mark != "currentcolor"
                }
            };
            if decides { Step::Here } else { Step::Up }
        })?;
        match self.dom.attr(at, SOURCE_LINK_COLOUR) {
            Some(mark) => Some(mark),
            None => self.styles.get(at, "color"),
        }
    }

    /// `_decoration_lines`: the decoration lines drawn through `n`'s text, from it and every
    /// ancestor, as one sorted string, or None.
    fn decoration_lines(&self, n: NodeId) -> PyValue {
        const KNOWN: u8 = 0x80;
        let own = |a: NodeId| {
            let mut bits = 0;
            for prop in ["text-decoration", "text-decoration-line"] {
                let v = self.styles.get(a, prop).unwrap_or_default().to_lowercase();
                for word in v.split(py::is_space) {
                    bits |= match word {
                        "underline" => 1,
                        "line-through" => 2,
                        _ => 0,
                    };
                }
            }
            bits
        };
        // The uncached ancestors, nearest first, then each from the topmost down.
        if self.decoration[n.index()].get() & KNOWN == 0 {
            let mut chain = vec![n];
            let mut above = 0;
            let mut cur = n;
            while let Some(p) = self.dom.parent(cur) {
                let known = self.decoration[p.index()].get();
                if known & KNOWN != 0 {
                    above = known;
                    break;
                }
                chain.push(p);
                cur = p;
            }
            for &m in chain.iter().rev() {
                above = KNOWN | above | own(m);
                self.decoration[m.index()].set(above);
            }
        }
        match self.decoration[n.index()].get() & 3 {
            1 => PyValue::Str("underline".into()),
            2 => PyValue::Str("line-through".into()),
            3 => PyValue::Str("line-through underline".into()),
            _ => PyValue::None,
        }
    }

    /// `_font_size_px`: `n`'s font size in CSS px, from each specified value up its ancestors,
    /// 16px above the root.
    fn font_size_px(&self, n: NodeId) -> f64 {
        if let Some(v) = self.sizes[n.index()].get() {
            return v;
        }
        let mut chain = vec![n];
        let mut inherited = FONT_PX;
        let mut cur = n;
        while let Some(p) = self.dom.parent(cur) {
            if let Some(v) = self.sizes[p.index()].get() {
                inherited = v;
                break;
            }
            chain.push(p);
            cur = p;
        }
        for &m in chain.iter().rev() {
            let value = self.styles.get(m, "font-size");
            let root = || self.root_size_px(m);
            let size = self
                .calibre_keyword_px(value, root)
                .unwrap_or_else(|| css_size_px(value, inherited, root));
            self.sizes[m.index()].set(Some(size));
            inherited = size;
        }
        inherited
    }

    /// `_root_size_px`: the root's font size, which `rem` is measured by; 16px for the root.
    fn root_size_px(&self, n: NodeId) -> f64 {
        let root = self.dom.root();
        if n == root {
            FONT_PX
        } else {
            self.font_size_px(root)
        }
    }

    /// `_calibre_keyword_px`: the px of the font-size keyword Calibre turned into `value`, as
    /// Kindle Previewer sizes it; only under a root of 16px.
    fn calibre_keyword_px(
        &self,
        value: Option<&str>,
        root_px: impl FnOnce() -> f64,
    ) -> Option<f64> {
        let text = py::strip(value.unwrap_or_default()).to_lowercase();
        let &(_, px) = self.keyword_px.iter().find(|(k, _)| *k == text)?;
        // Python's `abs(root_px() - 16) > 0.01`, false for a NaN.
        if (root_px() - FONT_PX).abs() > 0.01 {
            return None;
        }
        Some(px)
    }

    /// `_line_height_factor`: `n`'s line height as a factor of its own font size, from the
    /// nearest element declaring one; 1.2 for `normal` or none, None for a length kfxgen can't
    /// read or a font size of 0.
    fn line_height_factor(&self, n: NodeId) -> Option<f64> {
        let declared = |a: NodeId| {
            py::strip(self.styles.get(a, "line-height").unwrap_or_default()).to_lowercase()
        };
        let own = self.font_size_px(n);
        let Some(a) = self.nearest(&self.line_height, n, |a| {
            let v = declared(a);
            if v.is_empty() || v == "inherit" {
                Step::Up
            } else {
                Step::Here
            }
        }) else {
            return Some(1.2);
        };
        let value = declared(a);
        if value == "normal" {
            return Some(1.2);
        }
        if let Some(v) = py::py_float(&value) {
            return Some(py::round(v, 4));
        }
        if !FONT_SIZE_RE.is_match(&value.replace(' ', "")) || own == 0.0 {
            return None;
        }
        let px = css_size_px(Some(&value), self.font_size_px(a), || self.root_size_px(a));
        Some(py::round(px / own, 4))
    }

    /// `_first_letter_css`: a <p>'s `::first-letter` declarations, the font size in CSS px, or
    /// None.
    fn first_letter_css(&self, n: NodeId) -> PyValue {
        let Some(rule) = self
            .styles
            .pseudo(n, "first-letter")
            .filter(|r| !r.is_empty())
        else {
            return PyValue::None;
        };
        let pairs = rule
            .iter()
            .map(|(k, d)| {
                let v = if k == "font-size" && !d.value.is_empty() {
                    let value = Some(d.value.as_str());
                    let root = || self.root_size_px(n);
                    let size = self
                        .calibre_keyword_px(value, root)
                        .unwrap_or_else(|| css_size_px(value, self.font_size_px(n), root));
                    PyValue::Float(py::round(size, 3))
                } else {
                    PyValue::Str(d.value.clone())
                };
                (PyValue::Str(k.clone()), v)
            })
            .collect();
        PyValue::Dict(pairs)
    }

    /// `_lang_of`: the nearest `lang` at or above `n`.
    fn lang_of(&self, n: NodeId) -> Option<&'a str> {
        let at = self.nearest(&self.lang, n, |a| {
            if self.dom.attr(a, "lang").is_some() {
                Step::Here
            } else {
                Step::Up
            }
        })?;
        self.dom.attr(at, "lang")
    }

    /// `_starts_a_language`: whether `n` sets a `lang` its parent doesn't have.
    fn starts_a_language(&self, n: NodeId) -> bool {
        let Some(own) = self.dom.attr(n, "lang") else {
            return false;
        };
        Some(own) != self.dom.parent(n).and_then(|p| self.lang_of(p))
    }

    /// `_q_css`: a <q>'s `quotes`, from the nearest element declaring it up to one starting a
    /// language, and its `::before` and `::after` `content`.
    fn q_css(&self, n: NodeId) -> PyValue {
        let declared = |a: NodeId| py::strip(self.styles.get(a, "quotes").unwrap_or_default());
        let quotes = self
            .nearest(&self.quotes, n, |a| {
                let v = declared(a);
                if !v.is_empty() && !is_inherit(v) {
                    Step::Here
                } else if self.starts_a_language(a) {
                    Step::Stop
                } else {
                    Step::Up
                }
            })
            .map(declared);
        let content = |name: &str| {
            let decls = self.styles.pseudo(n, name);
            str_or_none(decls.and_then(|d| {
                d.iter()
                    .find(|(k, _)| k == "content")
                    .map(|(_, d)| d.value.as_str())
            }))
        };
        let s = |k: &str| PyValue::Str(k.to_owned());
        PyValue::Dict(vec![
            (s("quotes"), str_or_none(quotes)),
            (s("before"), content("before")),
            (s("after"), content("after")),
        ])
    }

    /// `behind(elem)`: the fill of the nearest box kfxgen draws around `n`, or its own, as
    /// 0xRRGGBB, or None for the page; what `_box_background` raised for that box.
    fn behind(&self, n: NodeId) -> Fill {
        if let Some(known) = &self.fills.borrow()[n.index()] {
            return known.clone();
        }
        // The elements up to the nearest one already answered or with a fill of its own.
        let mut chain = Vec::new();
        let mut cur = n;
        let found = loop {
            if let Some(known) = &self.fills.borrow()[cur.index()] {
                break known.clone();
            }
            chain.push(cur);
            if FILL_TAGS.contains(&self.dom.tag(cur)) {
                match self.box_background(cur) {
                    Ok(None) => {}
                    Ok(Some(fill)) => break Ok(Some(fill & 0xFF_FFFF)),
                    Err(e) => break Err(e),
                }
            }
            match self.dom.parent(cur) {
                Some(p) => cur = p,
                None => break Ok(None),
            }
        };
        let mut fills = self.fills.borrow_mut();
        for c in chain {
            fills[c.index()] = Some(found.clone());
        }
        found
    }

    /// `_box_background(stylizer.style(elem))`: the element's own fill as the ARGB integer
    /// Previewer writes, or None for none, transparent, white or unreadable.
    fn box_background(&self, n: NodeId) -> Result<Option<i64>, Raised> {
        let mut value = self
            .styles
            .get(n, "background-color")
            .filter(|v| !v.is_empty());
        let shorthand = match value {
            None => true,
            Some(v) => is_transparent(v)?,
        };
        if shorthand {
            // calibre leaves the shorthand unexpanded: its colour is the part that reads as
            // one (`next(...)` stops at the first).
            value = None;
            for part in background_parts(self.styles.get(n, "background").unwrap_or_default()) {
                if css_rgb(part)?.is_some() {
                    value = Some(part);
                    break;
                }
            }
        }
        let Some(value) = value else {
            return Ok(None);
        };
        if is_transparent(value)? {
            return Ok(None);
        }
        Ok(match css_rgb(value)? {
            None | Some(0xFF_FFFF) => None,
            Some(rgb) => Some(0xFF00_0000 | rgb),
        })
    }

    /// `_vertical_box`: in a vertical-rl file, the margins and paddings put in kfxgen's slots,
    /// each from the book's value of the physical side it stands for, else the user agent's
    /// own for that slot (`data-kfxgen-v-author`, `-v-ua`).
    fn vertical_box(&self, css: &mut Resolved, n: NodeId) {
        let Some(authored) = self.dom.attr(n, V_AUTHOR) else {
            return;
        };
        if self.dom.attr(n, VERTICAL).is_some() {
            return;
        }
        let authored: Vec<&str> = authored
            .split(py::is_space)
            .filter(|w| !w.is_empty())
            .collect();
        // `dict(item.split(":", 1) for item in ... if ":" in item)`: the last of a name wins.
        let ua: Vec<(&str, &str)> = self
            .dom
            .attr(n, V_UA)
            .unwrap_or_default()
            .split(';')
            .filter_map(|item| item.split_once(':'))
            .collect();
        let value = |kind: &str, slot: &str| {
            let side = format!("{kind}-{}", vertical_side(slot));
            if authored.contains(&side.as_str()) {
                return str_or_none(self.styles.get(n, &side));
            }
            let own = format!("{kind}-{slot}");
            str_or_none(ua.iter().rev().find(|(k, _)| *k == own).map(|(_, v)| *v))
        };
        for slot in SIDES {
            css.set(
                match slot {
                    "top" => "margin-top",
                    "right" => "margin-right",
                    "bottom" => "margin-bottom",
                    _ => "margin-left",
                },
                value("margin", slot),
            );
        }
        css.set(
            "padding-sides",
            PyValue::Tuple(SIDES.map(|s| value("padding", s)).to_vec()),
        );
    }
}

fn str_or_none(v: Option<&str>) -> PyValue {
    v.map_or(PyValue::None, |s| PyValue::Str(s.to_owned()))
}

/// `str(value).strip().lower() == "inherit"`.
fn is_inherit(v: &str) -> bool {
    py::strip(v).to_lowercase() == "inherit"
}

/// `_calibre_keyword_table`: {the rem value Calibre writes for a keyword: Previewer's px}, for
/// the profile Calibre rewrote keywords with; a value two keywords share, and `medium`, left
/// out.
fn keyword_table(profile: &Profile) -> Vec<(String, f64)> {
    let base = profile.fbase;
    let mut seen: Vec<(String, Vec<&str>)> = Vec::new();
    for &(keyword, pt) in &profile.fnames {
        if !FONT_KEYWORD_PX.iter().any(|(k, _)| *k == keyword) || keyword == "medium" || base == 0.0
        {
            continue;
        }
        let written = format!("{:.1}rem", pt / base);
        match seen.iter_mut().find(|(w, _)| *w == written) {
            Some((_, kws)) => kws.push(keyword),
            None => seen.push((written, vec![keyword])),
        }
    }
    seen.into_iter()
        .filter(|(_, kws)| kws.len() == 1)
        .filter_map(|(written, kws)| {
            let px = FONT_KEYWORD_PX.iter().find(|(k, _)| *k == kws[0])?.1;
            Some((written, px))
        })
        .collect()
}

/// `_css_size_px`: a font-size value in CSS px against the `inherited` size, or the inherited
/// size when there is none or kfxgen can't read it; `root_px()` gives the root's, for rem.
fn css_size_px(value: Option<&str>, inherited: f64, root_px: impl FnOnce() -> f64) -> f64 {
    let value = py::strip(value.unwrap_or_default()).to_lowercase();
    if let Some(&(_, px)) = FONT_KEYWORD_PX.iter().find(|(k, _)| *k == value) {
        return px;
    }
    match value.as_str() {
        // Previewer's step: <small> is 0.833333, <big> 1.2.
        "smaller" => return inherited / 1.2,
        "larger" => return inherited * 1.2,
        _ => {}
    }
    let squeezed = value.replace(' ', "");
    let Some(m) = FONT_SIZE_RE.captures(&squeezed) else {
        return inherited;
    };
    // `[0-9]*\.?[0-9]+` always reads as a float.
    let n = py::py_float(&m[1]).unwrap_or(0.0);
    if n <= 0.0 {
        return inherited;
    }
    match &m[2] {
        "em" => n * inherited,
        "ex" | "ch" => n * inherited / 2.0,
        "%" => n * inherited / 100.0,
        "rem" => n * root_px(),
        unit => match FONT_UNIT_PX.iter().find(|(u, _)| *u == unit) {
            Some(&(_, per)) => n * per,
            None => inherited,
        },
    }
}

/// `_background_parts`: a `background:` shorthand's parts, split at spaces, commas and slashes
/// outside parentheses.
fn background_parts(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth: i64 = 0;
    for (i, ch) in value.char_indices() {
        depth += i64::from(ch == '(') - i64::from(ch == ')');
        if depth <= 0 && matches!(ch, ' ' | '\t' | '\n' | ',' | '/') {
            if start < i {
                parts.push(&value[start..i]);
            }
            start = i + ch.len_utf8();
            depth = 0;
        }
    }
    if start < value.len() {
        parts.push(&value[start..]);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::DEFAULT_PROFILE;

    #[test]
    fn keyword_table_is_calibres_default_profile() {
        let table = keyword_table(&DEFAULT_PROFILE);
        let want = [
            ("0.4rem", 9.0),
            ("0.6rem", 10.0),
            ("0.8rem", 13.0),
            ("1.1rem", 18.0),
            ("1.4rem", 24.0),
            ("1.7rem", 32.0),
        ];
        assert_eq!(
            table,
            want.map(|(w, px)| (w.to_owned(), px)).to_vec(),
            "{table:?}"
        );
    }

    #[test]
    fn background_parts_keep_functions_whole() {
        assert_eq!(
            background_parts("#eee url(a b.png) no-repeat"),
            ["#eee", "url(a b.png)", "no-repeat"]
        );
        assert_eq!(
            background_parts("linear-gradient(rgb(1, 2, 3), red),  blue/"),
            ["linear-gradient(rgb(1, 2, 3), red)", "blue"]
        );
        // A `)` too many: the depth goes below 0, splits there and starts again at 0.
        assert_eq!(background_parts("a) b"), ["a)", "b"]);
        assert!(background_parts("").is_empty());
    }

    #[test]
    fn font_sizes_as_kfxgen_reads_them() {
        let root = || 20.0;
        assert_eq!(css_size_px(Some("1.5em"), 10.0, root), 15.0);
        assert_eq!(css_size_px(Some(" 1.5 EM "), 10.0, root), 15.0);
        assert_eq!(css_size_px(Some("2rem"), 10.0, root), 40.0);
        assert_eq!(css_size_px(Some("50%"), 10.0, root), 5.0);
        assert_eq!(css_size_px(Some("2ex"), 10.0, root), 10.0);
        assert_eq!(css_size_px(Some("12pt"), 10.0, root), 12.0 * (4.0 / 3.0));
        assert_eq!(css_size_px(Some("x-large"), 10.0, root), 24.0);
        assert_eq!(css_size_px(Some("smaller"), 12.0, root), 10.0);
        // Unread, zero, negative or an unknown unit: the inherited size.
        for v in ["calc(1em)", "0em", "-1em", "3vw", "", "1e3px"] {
            assert_eq!(css_size_px(Some(v), 10.0, root), 10.0, "{v}");
        }
        assert_eq!(css_size_px(None, 10.0, root), 10.0);
    }
}
