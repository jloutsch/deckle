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
//! The memos hold a few words per element. Every copy, parse or case-fold of a value from the
//! book is charged first ([`Resolver::charge`]), so a long value read for many elements ends in
//! [`css::Error::Cascade`] after bounded work; kfxgen does that work for every element.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::LazyLock;

use deckle_limits::{MAX_BOOK_TEXT_BUDGET, MAX_CASCADE_DECLS};

use regex::Regex;

use super::block_style::StyleError;
use super::inline_style::{
    Raised, Typography, css_font_size, css_rgb, css_typography, is_transparent,
};
use crate::book::File;
use crate::css::{self, Profile, Styles, TEXT_UNIT, Units, Value};
use crate::dom::{Dom, NodeId};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// The style text one book file's conversion may copy or read per element, in
/// [`MAX_CASCADE_DECLS`](deckle_limits::MAX_CASCADE_DECLS) units: one for every [`TEXT_UNIT`]
/// bytes, as the cascade charges a declaration's text. Made once for the file and handed to
/// every [`Resolver`] built for it (kfxgen builds one per pass and one in `_footnote_keys`), so
/// building another does not add to the allowance. Past the limit, [`css::Error::Cascade`], as
/// a cascade past it gives: the file is refused. The budgets of one pass over a book
/// ([`TextBudget::for_book`]) also count together, against
/// [`MAX_BOOK_TEXT_BUDGET`](deckle_limits::MAX_BOOK_TEXT_BUDGET): past that the same error
/// stops the book, before the copy is made.
pub struct TextBudget {
    own: RefCell<Units>,
    /// The units all the book's budgets were charged, shared by them.
    book: Option<Rc<Cell<usize>>>,
}

impl Default for TextBudget {
    fn default() -> Self {
        TextBudget {
            own: RefCell::new(Units(0)),
            book: None,
        }
    }
}

impl TextBudget {
    pub fn new() -> TextBudget {
        TextBudget::default()
    }

    /// A budget for each of `files` files, counting together as one book's.
    pub fn for_book(files: usize) -> Vec<TextBudget> {
        let book = Rc::new(Cell::new(0));
        (0..files)
            .map(|_| TextBudget {
                book: Some(Rc::clone(&book)),
                ..TextBudget::default()
            })
            .collect()
    }

    /// The units charged so far.
    pub fn charged(&self) -> usize {
        self.own.borrow().0
    }

    /// The units all the budgets of its book were charged so far; 0 for a budget of no book.
    pub fn book_charged(&self) -> usize {
        self.book.as_ref().map_or(0, |b| b.get())
    }

    /// Charges `bytes` of book text about to be copied or read through. A value under
    /// [`TEXT_UNIT`] bytes costs nothing, so the free work of each read is a constant.
    pub(crate) fn charge(&self, bytes: usize) -> Result<(), css::Error> {
        let units = bytes / TEXT_UNIT;
        if let Some(book) = &self.book {
            book.set(book.get().saturating_add(units));
            if book.get() > MAX_BOOK_TEXT_BUDGET {
                return Err(css::Error::Cascade(MAX_CASCADE_DECLS));
            }
        }
        self.own.borrow_mut().charge(units)
    }
}

/// The bytes a value read out of a [`Resolved`] is charged as: its strings, and a
/// [`TEXT_UNIT`] for each entry of a dict (a `::first-letter` rule can hold any number).
fn cost(v: &PyValue) -> usize {
    match v {
        PyValue::Str(s) => s.len(),
        PyValue::List(items) | PyValue::Tuple(items) | PyValue::Set(items) => {
            items.iter().map(cost).fold(0, usize::saturating_add)
        }
        PyValue::Dict(pairs) => pairs
            .iter()
            .map(|(k, v)| TEXT_UNIT.saturating_add(cost(k)).saturating_add(cost(v)))
            .fold(0, usize::saturating_add),
        PyValue::Obj { state, .. } => cost(state),
        _ => 0,
    }
}

/// An element's resolved CSS: `resolve(elem)`'s dict, its keys in kfxgen's order. Every value
/// read out of it is charged to the file's [`TextBudget`] first ([`Resolved::get`],
/// [`Resolved::to_py`], [`Resolved::typography`]), so a caller cannot hand a long value to a
/// parser for every element without paying for it. It cannot be cloned.
pub struct Resolved<'b> {
    /// Each key, its value and the value's [`cost`].
    values: Vec<(&'static str, PyValue, usize)>,
    budget: &'b TextBudget,
}

impl<'b> Resolved<'b> {
    fn new(budget: &'b TextBudget) -> Resolved<'b> {
        Resolved {
            values: Vec::with_capacity(48),
            budget,
        }
    }

    fn push(&mut self, key: &'static str, value: PyValue) {
        let c = cost(&value);
        self.values.push((key, value, c));
    }

    fn set(&mut self, key: &str, value: PyValue) {
        let c = cost(&value);
        if let Some(slot) = self.values.iter_mut().find(|(k, _, _)| *k == key) {
            slot.1 = value;
            slot.2 = c;
        }
    }

    /// The value of `key`, uncharged: only for what charges it first.
    fn raw(&self, key: &str) -> Option<&PyValue> {
        self.values
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, v, _)| v)
    }

    /// `css.get(key)`, its value charged: `Some(&PyValue::None)` for a key holding None, None
    /// for a key the dict does not have.
    pub fn get(&self, key: &str) -> Result<Option<&PyValue>, css::Error> {
        let Some((_, v, c)) = self.values.iter().find(|(k, _, _)| *k == key) else {
            return Ok(None);
        };
        self.budget.charge(*c)?;
        Ok(Some(v))
    }

    /// `key in css`: whether the dict has the key, None or not. Reads no value.
    pub(crate) fn has(&self, key: &str) -> bool {
        self.raw(key).is_some()
    }

    /// The dict as Python had it, every value charged.
    pub fn to_py(&self) -> Result<PyValue, css::Error> {
        let mut pairs = Vec::with_capacity(self.values.len());
        for (k, v, c) in &self.values {
            self.budget.charge(*c)?;
            pairs.push((PyValue::Str((*k).to_owned()), v.clone()));
        }
        Ok(PyValue::Dict(pairs))
    }

    /// `css_typography(css)`, the values it reads charged first.
    pub(crate) fn typography(&self) -> Result<Typography, StyleError> {
        for key in [
            "font-variant",
            "text-transform",
            "letter-spacing",
            "color",
            "text-orientation",
            "text-emphasis-style",
            "background-behind",
        ] {
            self.get(key)?;
        }
        Ok(css_typography(|k| self.raw(k))?)
    }

    /// `css_font_size(css)`: a number, nothing to charge.
    pub(crate) fn font_size(&self) -> Option<&PyValue> {
        css_font_size(|k| self.raw(k))
    }

    /// A dict of the given keys only, as kfxgen's tests hand `compute_block_style`.
    #[cfg(test)]
    pub(super) fn of(pairs: Vec<(&'static str, PyValue)>, budget: &'b TextBudget) -> Self {
        let mut r = Resolved::new(budget);
        for (k, v) in pairs {
            r.push(k, v);
        }
        r
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
    /// The file's budget for the text copied or read ([`TextBudget`]).
    budget: &'a TextBudget,
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
    /// `budget` is the file's, shared by every resolver built for it.
    pub fn new(file: &'a File, profile: &'a Profile, budget: &'a TextBudget) -> Resolver<'a> {
        let n = file.dom.node_count();
        Resolver {
            dom: &file.dom,
            styles: file.styles(profile),
            keyword_px: keyword_table(profile),
            budget,
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

    /// [`TextBudget::charge`]: called before every copy, parse or case-fold of a value.
    pub(crate) fn charge(&self, bytes: usize) -> Result<(), css::Error> {
        self.budget.charge(bytes)
    }

    /// `{}`: a style with no values, as kfxgen hands `compute_block_style({})`.
    pub(crate) fn no_style(&self) -> Resolved<'a> {
        Resolved::new(self.budget)
    }

    /// `resolve(elem)`: the element's resolved CSS, or None where kfxgen's `resolve` catches an
    /// exception (the fill behind it, `_box_background` on a colour Python's `float()` or
    /// `int()` refuses). A new value on every call, as kfxgen's; a caller that needs it again
    /// keeps it. [`css::Error::Cascade`] once the text copied and read passes the budget
    /// ([`Self::charge`]).
    pub fn resolve(&self, n: NodeId) -> Result<Option<Resolved<'a>>, css::Error> {
        // The only part of `resolve`'s body that can raise; every value is worked out without
        // side effects, so the order does not matter.
        let Ok(behind) = self.behind(n)? else {
            return Ok(None);
        };
        let tag = self.dom.tag(n);
        let own = |prop: &str| self.text(self.styles.get(n, prop));
        let inherited = |i: usize| self.text(self.specified_inherited(n, i)?);
        let computed = |prop: &str| self.computed(n, prop);
        let mut css = Resolved::new(self.budget);
        let mut put = |k: &'static str, v: PyValue| css.push(k, v);
        put(
            "font-size",
            PyValue::Float(py::round(self.font_size_px(n)?, 3)),
        );
        put(
            "line-height",
            self.line_height_factor(n)?
                .map_or(PyValue::None, PyValue::Float),
        );
        put("line-height-declared", inherited(0)?);
        put(
            "first-letter",
            if tag == "p" {
                self.first_letter_css(n)?
            } else {
                PyValue::None
            },
        );
        put(
            "q-css",
            if tag == "q" {
                self.q_css(n)?
            } else {
                PyValue::None
            },
        );
        put("float", own("float")?);
        put("clear", own("clear")?);
        put("text-align", computed("text-align")?);
        put("own-text-align", own("text-align")?);
        put("text-indent", own("text-indent")?);
        put("margin-left", own("margin-left")?);
        put("margin-right", own("margin-right")?);
        put("margin-top", own("margin-top")?);
        put("margin-bottom", own("margin-bottom")?);
        put("font-family", computed("font-family")?);
        put("font-weight", inherited(1)?);
        put("text-decoration", self.decoration_lines(n)?);
        put("letter-spacing", inherited(2)?);
        put("font-variant", inherited(3)?);
        put("text-transform", inherited(4)?);
        put("text-orientation", inherited(5)?);
        put("text-emphasis-style", inherited(6)?);
        put("text-combine-upright", own("text-combine-upright")?);
        put(
            "writing-mode",
            if matches!(tag, "html" | "body") {
                inherited(7)?
            } else {
                PyValue::None
            },
        );
        put("color", self.text(self.specified_colour(n)?)?);
        put("font-style", computed("font-style")?);
        put("vertical-align", own("vertical-align")?);
        put("own-direction", own("direction")?);
        put("own-unicode-bidi", own("unicode-bidi")?);
        put("width", own("width")?);
        put(
            "height",
            if tag == "hr" {
                // `_book_height`: calibre's own `height: 2px` for every <hr> is not the book's.
                match self.styles.get(n, "height") {
                    Some(v) => {
                        self.charge(v.len())?;
                        if py::strip(v) == "2px" {
                            PyValue::None
                        } else {
                            self.text(Some(v))?
                        }
                    }
                    None => PyValue::None,
                }
            } else {
                own("height")?
            },
        );
        put("background-image", own("background-image")?);
        put("background-color", own("background-color")?);
        put("background", own("background")?);
        put(
            "background-behind",
            behind.map_or(PyValue::None, PyValue::Int),
        );
        put("background-repeat", own("background-repeat")?);
        put("page-break-before", own("page-break-before")?);
        put("page-break-after", own("page-break-after")?);
        put("break-before", own("break-before")?);
        put("break-after", own("break-after")?);
        put("white-space", computed("white-space")?);
        put("list-style-type", computed("list-style-type")?);
        put("list-style-position", computed("list-style-position")?);
        put("display", own("display")?);
        let mut padding = Vec::with_capacity(4);
        let mut widths = Vec::with_capacity(4);
        let mut borders = Vec::with_capacity(4);
        for side in SIDES {
            padding.push(own(&format!("padding-{side}"))?);
            widths.push(own(&format!("border-{side}-width"))?);
            let mut border = Vec::with_capacity(3);
            for part in ["style", "width", "color"] {
                border.push(computed(&format!("border-{side}-{part}"))?);
            }
            borders.push(PyValue::Tuple(border));
        }
        put("padding-sides", PyValue::Tuple(padding));
        put("border-collapse", computed("border-collapse")?);
        put("border-widths", PyValue::Tuple(widths));
        put("border-sides", PyValue::Tuple(borders));
        self.vertical_box(&mut css, n)?;
        Ok(Some(css))
    }

    /// A copy of `v`, charged.
    fn text(&self, v: Option<&str>) -> Result<PyValue, css::Error> {
        Ok(match v {
            Some(s) => {
                self.charge(s.len())?;
                PyValue::Str(s.to_owned())
            }
            None => PyValue::None,
        })
    }

    /// `str(v).strip().lower()`, charged.
    fn folded(&self, v: &str) -> Result<String, css::Error> {
        self.charge(v.len())?;
        Ok(py::strip(v).to_lowercase())
    }

    /// `_computed_value(st, prop)`: `st[prop]`, or `st.get(prop)` where that raises. The
    /// value it reads and copies is charged first.
    fn computed(&self, n: NodeId, prop: &str) -> Result<PyValue, css::Error> {
        if let Ok(v) = self.styles.inherited(n, prop) {
            self.charge(v.len())?;
        }
        Ok(match self.styles.computed(n, prop) {
            Ok(Value::Pt(v)) => PyValue::Float(v),
            Ok(Value::Text(s)) => PyValue::Str(s),
            Ok(Value::Absent) => PyValue::None,
            Err(_) => self.text(self.styles.get(n, prop))?,
        })
    }

    /// A walk up from `n` (`n` itself first) to where `step` ends it, kept in `memo` for every
    /// element on the way.
    fn nearest(
        &self,
        memo: &[Cell<Memo>],
        n: NodeId,
        step: impl Fn(NodeId) -> Result<Step, css::Error>,
    ) -> Result<Option<NodeId>, css::Error> {
        let mut chain = Vec::new();
        let mut cur = n;
        let found = loop {
            let known = memo[cur.index()].get();
            if known != Memo::Unknown {
                break known;
            }
            chain.push(cur);
            match step(cur)? {
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
        Ok(match found {
            Memo::At(at) => Some(at),
            _ => None,
        })
    }

    /// Whether `v` is truthy and not `inherit` once stripped and lowered, charged.
    fn declares(&self, v: Option<&str>) -> Result<bool, css::Error> {
        Ok(match v {
            Some(v) if !v.is_empty() => self.folded(v)? != "inherit",
            _ => false,
        })
    }

    /// `_specified_inherited(stylizer, elem, INHERITED[i])`: the value on `n` or its nearest
    /// ancestor that sets it, as written; `inherit` skipped.
    fn specified_inherited(&self, n: NodeId, i: usize) -> Result<Option<&'a str>, css::Error> {
        let prop = INHERITED[i];
        let at = self.nearest(&self.inherited[i], n, |a| {
            Ok(if self.declares(self.styles.get(a, prop))? {
                Step::Here
            } else {
                Step::Up
            })
        })?;
        Ok(at.and_then(|at| self.styles.get(at, prop)))
    }

    /// `_specified_colour`: `_specified_inherited`'s colour, a link's `data-kfxgen-link-colour`
    /// mark in place of an element's colour; `inherit` and `currentColor` there take the colour
    /// around the link.
    fn specified_colour(&self, n: NodeId) -> Result<Option<&'a str>, css::Error> {
        let at = self.nearest(&self.colour, n, |a| {
            let decides = match self.dom.attr(a, SOURCE_LINK_COLOUR) {
                None => self.declares(self.styles.get(a, "color"))?,
                Some(mark) => {
                    let mark = self.folded(mark)?;
                    mark != "inherit" && mark != "currentcolor"
                }
            };
            Ok(if decides { Step::Here } else { Step::Up })
        })?;
        Ok(
            at.and_then(|at| match self.dom.attr(at, SOURCE_LINK_COLOUR) {
                Some(mark) => Some(mark),
                None => self.styles.get(at, "color"),
            }),
        )
    }

    /// `_decoration_lines`: the decoration lines drawn through `n`'s text, from it and every
    /// ancestor, as one sorted string, or None.
    fn decoration_lines(&self, n: NodeId) -> Result<PyValue, css::Error> {
        const KNOWN: u8 = 0x80;
        let own = |a: NodeId| -> Result<u8, css::Error> {
            let mut bits = 0;
            for prop in ["text-decoration", "text-decoration-line"] {
                let v = self.styles.get(a, prop).unwrap_or_default();
                self.charge(v.len())?;
                for word in v.to_lowercase().split(py::is_space) {
                    bits |= match word {
                        "underline" => 1,
                        "line-through" => 2,
                        _ => 0,
                    };
                }
            }
            Ok(bits)
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
                above = KNOWN | above | own(m)?;
                self.decoration[m.index()].set(above);
            }
        }
        Ok(match self.decoration[n.index()].get() & 3 {
            1 => PyValue::Str("underline".into()),
            2 => PyValue::Str("line-through".into()),
            3 => PyValue::Str("line-through underline".into()),
            _ => PyValue::None,
        })
    }

    /// `_font_size_px`: `n`'s font size in CSS px, from each specified value up its ancestors,
    /// 16px above the root.
    fn font_size_px(&self, n: NodeId) -> Result<f64, css::Error> {
        if let Some(v) = self.sizes[n.index()].get() {
            return Ok(v);
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
            let size = self.size_px(m, self.styles.get(m, "font-size"), inherited)?;
            self.sizes[m.index()].set(Some(size));
            inherited = size;
        }
        Ok(inherited)
    }

    /// A font-size `value` on `n` in CSS px: `_calibre_keyword_px`, else `_css_size_px`
    /// against `inherited`. The root's size is asked first, where Python asks it only when the
    /// value needs it: it is memoized and changes nothing.
    fn size_px(&self, n: NodeId, value: Option<&str>, inherited: f64) -> Result<f64, css::Error> {
        self.charge(value.map_or(0, str::len))?;
        let root = self.root_size_px(n)?;
        Ok(self
            .calibre_keyword_px(value, root)
            .unwrap_or_else(|| css_size_px(value, inherited, root)))
    }

    /// `_root_size_px`: the root's font size, which `rem` is measured by; 16px for the root.
    fn root_size_px(&self, n: NodeId) -> Result<f64, css::Error> {
        let root = self.dom.root();
        if n == root {
            Ok(FONT_PX)
        } else {
            self.font_size_px(root)
        }
    }

    /// `_calibre_keyword_px`: the px of the font-size keyword Calibre turned into `value`, as
    /// Kindle Previewer sizes it; only under a root of 16px.
    fn calibre_keyword_px(&self, value: Option<&str>, root_px: f64) -> Option<f64> {
        let text = py::strip(value.unwrap_or_default()).to_lowercase();
        let &(_, px) = self.keyword_px.iter().find(|(k, _)| *k == text)?;
        // Python's `abs(root_px() - 16) > 0.01`, false for a NaN.
        if (root_px - FONT_PX).abs() > 0.01 {
            return None;
        }
        Some(px)
    }

    /// `_line_height_factor`: `n`'s line height as a factor of its own font size, from the
    /// nearest element declaring one; 1.2 for `normal` or none, None for a length kfxgen can't
    /// read or a font size of 0.
    fn line_height_factor(&self, n: NodeId) -> Result<Option<f64>, css::Error> {
        let declared =
            |a: NodeId| self.folded(self.styles.get(a, "line-height").unwrap_or_default());
        let own = self.font_size_px(n)?;
        let Some(a) = self.nearest(&self.line_height, n, |a| {
            let v = declared(a)?;
            Ok(if v.is_empty() || v == "inherit" {
                Step::Up
            } else {
                Step::Here
            })
        })?
        else {
            return Ok(Some(1.2));
        };
        let value = declared(a)?;
        if value == "normal" {
            return Ok(Some(1.2));
        }
        if let Some(v) = py::py_float(&value) {
            return Ok(Some(py::round(v, 4)));
        }
        if !FONT_SIZE_RE.is_match(&value.replace(' ', "")) || own == 0.0 {
            return Ok(None);
        }
        let px = self.size_length_px(a, &value)?;
        Ok(Some(py::round(px / own, 4)))
    }

    /// `_css_size_px(value, _font_size_px(a), root)`: a length declared on `a` in CSS px.
    fn size_length_px(&self, a: NodeId, value: &str) -> Result<f64, css::Error> {
        let inherited = self.font_size_px(a)?;
        let root = self.root_size_px(a)?;
        Ok(css_size_px(Some(value), inherited, root))
    }

    /// `_first_letter_css`: a <p>'s `::first-letter` declarations, the font size in CSS px, or
    /// None.
    fn first_letter_css(&self, n: NodeId) -> Result<PyValue, css::Error> {
        let Some(rule) = self
            .styles
            .pseudo(n, "first-letter")
            .filter(|r| !r.is_empty())
        else {
            return Ok(PyValue::None);
        };
        // A unit for each declaration copied, however short.
        self.charge(TEXT_UNIT.saturating_mul(rule.len()))?;
        let mut pairs = Vec::with_capacity(rule.len());
        for (k, d) in rule {
            self.charge(k.len())?;
            let v = if k == "font-size" && !d.value.is_empty() {
                let value = Some(d.value.as_str());
                let size = self.size_px(n, value, self.font_size_px(n)?)?;
                PyValue::Float(py::round(size, 3))
            } else {
                self.text(Some(&d.value))?
            };
            pairs.push((PyValue::Str(k.clone()), v));
        }
        Ok(PyValue::Dict(pairs))
    }

    /// `_lang_of`: the nearest `lang` at or above `n`.
    pub(crate) fn lang_of(&self, n: NodeId) -> Result<Option<&'a str>, css::Error> {
        let at = self.nearest(&self.lang, n, |a| {
            Ok(if self.dom.attr(a, "lang").is_some() {
                Step::Here
            } else {
                Step::Up
            })
        })?;
        Ok(at.and_then(|at| self.dom.attr(at, "lang")))
    }

    /// `_starts_a_language`: whether `n` sets a `lang` its parent doesn't have.
    fn starts_a_language(&self, n: NodeId) -> Result<bool, css::Error> {
        let Some(own) = self.dom.attr(n, "lang") else {
            return Ok(false);
        };
        let around = match self.dom.parent(n) {
            Some(p) => self.lang_of(p)?,
            None => None,
        };
        // The comparison reads `own`.
        self.charge(own.len())?;
        Ok(Some(own) != around)
    }

    /// `_q_css`: a <q>'s `quotes`, from the nearest element declaring it up to one starting a
    /// language, and its `::before` and `::after` `content`.
    fn q_css(&self, n: NodeId) -> Result<PyValue, css::Error> {
        let declared = |a: NodeId| self.styles.get(a, "quotes").unwrap_or_default();
        let at = self.nearest(&self.quotes, n, |a| {
            let raw = declared(a);
            self.charge(raw.len())?;
            let v = py::strip(raw);
            Ok(if !v.is_empty() && v.to_lowercase() != "inherit" {
                Step::Here
            } else if self.starts_a_language(a)? {
                Step::Stop
            } else {
                Step::Up
            })
        })?;
        if let Some(a) = at {
            self.charge(declared(a).len())?;
        }
        let quotes = self.text(at.map(|a| py::strip(declared(a))))?;
        let content = |name: &str| {
            let decls = self.styles.pseudo(n, name);
            // A unit for each declaration looked through.
            self.charge(TEXT_UNIT.saturating_mul(decls.map_or(0, Vec::len)))?;
            self.text(decls.and_then(|d| {
                d.iter()
                    .find(|(k, _)| k == "content")
                    .map(|(_, d)| d.value.as_str())
            }))
        };
        let s = |k: &str| PyValue::Str(k.to_owned());
        Ok(PyValue::Dict(vec![
            (s("quotes"), quotes),
            (s("before"), content("before")?),
            (s("after"), content("after")?),
        ]))
    }

    /// `behind(elem)`: the fill of the nearest box kfxgen draws around `n`, or its own, as
    /// 0xRRGGBB, or None for the page; what `_box_background` raised for that box.
    fn behind(&self, n: NodeId) -> Result<Fill, css::Error> {
        if let Some(known) = &self.fills.borrow()[n.index()] {
            return Ok(known.clone());
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
                match self.box_background(cur)? {
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
        Ok(found)
    }

    /// `_box_background(stylizer.style(elem))`: the element's own fill as the ARGB integer
    /// Previewer writes, or None for none, transparent, white or unreadable. Both values it
    /// may read are charged first.
    pub(crate) fn box_background(&self, n: NodeId) -> Result<Fill, css::Error> {
        let colour = self.styles.get(n, "background-color");
        let shorthand = self.styles.get(n, "background").unwrap_or_default();
        self.charge(colour.map_or(0, str::len))?;
        self.charge(shorthand.len())?;
        Ok((|| {
            let mut value = colour.filter(|v| !v.is_empty());
            let use_shorthand = match value {
                None => true,
                Some(v) => is_transparent(v)?,
            };
            if use_shorthand {
                // calibre leaves the shorthand unexpanded: its colour is the part that reads
                // as one (`next(...)` stops at the first).
                value = None;
                for part in background_parts(shorthand) {
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
        })())
    }

    /// `_vertical_box`: in a vertical-rl file, the margins and paddings put in kfxgen's slots,
    /// each from the book's value of the physical side it stands for, else the user agent's
    /// own for that slot (`data-kfxgen-v-author`, `-v-ua`).
    fn vertical_box(&self, css: &mut Resolved<'_>, n: NodeId) -> Result<(), css::Error> {
        let Some(authored) = self.dom.attr(n, V_AUTHOR) else {
            return Ok(());
        };
        if self.dom.attr(n, VERTICAL).is_some() {
            return Ok(());
        }
        let ua = self.dom.attr(n, V_UA).unwrap_or_default();
        self.charge(authored.len())?;
        self.charge(ua.len())?;
        let authored: Vec<&str> = authored
            .split(py::is_space)
            .filter(|w| !w.is_empty())
            .collect();
        // `dict(item.split(":", 1) for item in ... if ":" in item)`: the last of a name wins.
        let ua: Vec<(&str, &str)> = ua
            .split(';')
            .filter_map(|item| item.split_once(':'))
            .collect();
        let value = |kind: &str, slot: &str| {
            let side = format!("{kind}-{}", vertical_side(slot));
            if authored.contains(&side.as_str()) {
                return self.text(self.styles.get(n, &side));
            }
            let own = format!("{kind}-{slot}");
            self.text(ua.iter().rev().find(|(k, _)| *k == own).map(|(_, v)| *v))
        };
        for slot in SIDES {
            let key = match slot {
                "top" => "margin-top",
                "right" => "margin-right",
                "bottom" => "margin-bottom",
                _ => "margin-left",
            };
            css.set(key, value("margin", slot)?);
        }
        let mut padding = Vec::with_capacity(4);
        for slot in SIDES {
            padding.push(value("padding", slot)?);
        }
        css.set("padding-sides", PyValue::Tuple(padding));
        Ok(())
    }
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
/// size when there is none or kfxgen can't read it; `root_px` is the root's, for rem.
fn css_size_px(value: Option<&str>, inherited: f64, root_px: f64) -> f64 {
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
        "rem" => n * root_px,
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
        let root = 20.0;
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
