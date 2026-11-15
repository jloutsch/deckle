// SPDX-License-Identifier: GPL-3.0-only
//! A block's inline content as text segments with run flags: kfxgen's `_walk_inline` and its
//! helpers (`converter.py`, at kfxgen `ad38ec8`): `_nest_shift`, `_whole_baseline_dropped`,
//! `_white_space_flags`, `_css_emphasis`, `_is_link_flag`, `_near_black`, `_last_shown`,
//! `_text_before`, `_is_all_its_link_shows`, `_draws_as_block`, `_turned_parts`,
//! `_ruby_parts`, `_own_anchor_ids`, `_subtree_anchor_ids`, `_epub_types`, `_is_non_rendered`,
//! `_is_hidden` and `_epub_switch_branch`. The segments go to `normalize_runs_with_anchors`.
//!
//! The model is the contract: segments, their flags and the serials of rubies, tate-chu-yoko,
//! turned text and directions are kfxgen's, in its order.
//!
//! Hostile input: kfxgen recurses once per nested element; the walk here keeps its own stack
//! of open elements ([`Job`]s) and stops with [`Error::TooLarge`] past [`MAX_DEPTH`] levels,
//! which no tree `read_book` gives reaches (it refuses a file nested deeper). The helpers that
//! look around an element (what a sibling shows last, the text before a link, whether an
//! element is hidden) are loops whose answers are kept per element, so a file costs one pass
//! of each. Each element's and each segment's flags are a copy (Python shares one set): the
//! text in them is charged to the file's [`TextBudget`](super::TextBudget), so a long link
//! target over many segments ends in refusing the file, not in work and memory without bound.
//! A `<math>` is refused, as the converter refuses every `<math>` (plan decision 2).

use std::borrow::Cow;

use deckle_limits::MAX_DEPTH;

use super::bidi::inline_bidi;
use super::block_style::font_families;
use super::font_table::{parse_style, parse_weight};
use super::href::{external_target, resolve_link_target};
use super::inline_style::{
    BASELINE_SHIFT, BIDI_FLAG, COLOUR_FLAG, FLAG_BOLD, FLAG_ITALIC, FLAG_NOT_BOLD, FLAG_NOT_ITALIC,
    FLAG_PRE, FLAG_PRE_LINE, FLAG_STRIKE, FLAG_SUB, FLAG_SUPER, FLAG_TEXT_BOTTOM, FLAG_TEXT_TOP,
    FLAG_UNDERLINE, Flag, Flags, HARD_BREAK, LANG_FLAG, LINE_START, LINK_COLOUR_FLAG, LINK_FLAG,
    MONO_FLAG, NOTEREF_FLAG, Raised, SHIFT_FLAGS, SIZE_FLAG, SPACING_NORMAL, Segment,
    TYPOGRAPHY_FLAGS, css_link_colour, is_monospace, make_anchor_mark, make_bidi_flag,
    make_link_flag, make_mono_flag, make_ruby_flag, make_size_flag, make_tcy_flag, make_turn_flag,
    nested_shift, normalize_runs_with_anchors, parse_vertical_align,
};
use super::quotes::{QCss, QMarks, document_q_marks, plain_attr};
use super::resolver::{Resolved, Resolver};
use super::{Error, ItemError};
use crate::book::File;
use crate::dom::{Dom, NodeId};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `BOLD_WEIGHT_THRESHOLD` (`font_table.py`).
const BOLD_WEIGHT: i64 = 600;
/// The namespace of `epub:` names.
const OPS_NS: &str = "http://www.idpf.org/2007/ops";
const V_AUTHOR: &str = "data-kfxgen-v-author";
const HORIZONTAL: &str = "data-kfxgen-horizontal";
const VERTICAL: &str = "data-kfxgen-vertical";
/// `_COLLAPSING`: the white space CSS collapses (a no-break space is not).
const COLLAPSING: [char; 5] = [' ', '\t', '\n', '\r', '\u{c}'];
/// The placeholder a tate-chu-yoko or a turned text stands on (U+FFFC).
const OBJECT: char = '\u{fffc}';

/// `_CELL_TAGS`.
fn is_cell(tag: &str) -> bool {
    matches!(tag, "td" | "th")
}

/// `_CELL_BLOCK_TAGS`.
fn is_cell_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "pre"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "section"
            | "article"
            | "figure"
    )
}

/// `_BODY_BLOCK_TAGS`.
fn is_body_block(tag: &str) -> bool {
    matches!(
        tag,
        "img"
            | "svg"
            | "address"
            | "aside"
            | "center"
            | "details"
            | "fieldset"
            | "figcaption"
            | "footer"
            | "form"
            | "header"
            | "hgroup"
            | "hr"
            | "main"
            | "menu"
            | "nav"
            | "summary"
    )
}

/// `_TABLE_PART_TAGS`.
fn is_table_part(tag: &str) -> bool {
    matches!(
        tag,
        "tr" | "thead" | "tbody" | "tfoot" | "caption" | "col" | "colgroup"
    )
}

/// `_LINK_INLINE_TAGS`: the inline tags the text around a link is looked for through.
fn is_link_inline(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "big"
            | "cite"
            | "code"
            | "del"
            | "dfn"
            | "em"
            | "font"
            | "i"
            | "ins"
            | "kbd"
            | "label"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strike"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "tt"
            | "u"
            | "var"
    )
}

/// `_TURN_TAGS`: `_LINK_INLINE_TAGS` but `sub` and `sup`.
fn is_turn_tag(tag: &str) -> bool {
    is_link_inline(tag) && !matches!(tag, "sub" | "sup")
}

/// The serials kfxgen's process-wide counters give rubies (`_ruby_serials`), tate-chu-yoko
/// (`_tcy_serials`), turned text (`_turn_serials`) and inline directions (`_BIDI_SERIAL`):
/// each field is the next one of its kind. One for the whole book, in walk order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Serials {
    pub ruby: i64,
    pub tcy: i64,
    pub turn: i64,
    pub bidi: i64,
}

/// `next(counter)`.
fn next(counter: &mut i64) -> i64 {
    let n = *counter;
    *counter = counter.saturating_add(1);
    n
}

/// One item of `_walk_inline`'s list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// A (text, flags) segment.
    Run(Segment),
    /// `(_TABLE_SPLIT, table)`: a table nested in the content, which the caller writes as a
    /// table of its own (with `split_tables`).
    Table(NodeId),
}

/// `_css_emphasis(css)`: (bold, italic, monospace families, font size, typography, colour as
/// given). The typography is `css_typography`'s, each value as its run flag carries it.
#[derive(Clone, Debug, PartialEq)]
struct Emphasis {
    bold: bool,
    italic: bool,
    mono: Option<Vec<String>>,
    size: Option<PyValue>,
    typography: [Option<PyValue>; 6],
    link_colour: Option<i64>,
}

/// A memo slot of a bool worked out once per element.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bit {
    Unknown,
    No,
    Yes,
}

/// A memo slot of `_last_shown`/`_text_before`: unknown, "" (none), or the last character
/// of what was found (all its callers read).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    Unknown,
    Known(Option<char>),
}

/// `str(value or "")` of a resolved value.
fn text_of(v: Option<&PyValue>) -> Cow<'_, str> {
    match v {
        Some(PyValue::Str(s)) => Cow::Borrowed(s),
        Some(PyValue::Float(f)) if *f != 0.0 => Cow::Owned(py::repr_float(*f)),
        Some(PyValue::Int(i)) if *i != 0 => Cow::Owned(i.to_string()),
        Some(PyValue::Bool(true)) => Cow::Borrowed("True"),
        _ => Cow::Borrowed(""),
    }
}

/// `str(css.get(key) or "").strip().lower()`, "" without a resolved style.
fn keyword(css: Option<&Resolved>, key: &str) -> Result<String, ItemError> {
    let Some(css) = css else {
        return Ok(String::new());
    };
    Ok(py::strip(&text_of(css.get(key)?)).to_lowercase())
}

/// `" ".join(s.split())`.
fn words(s: &str) -> String {
    s.split(py::is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The last character of `s`, or None for "".
fn last(s: &str) -> Option<char> {
    s.chars().next_back()
}

/// `_is_link_flag`.
fn is_link_flag(f: &Flag) -> bool {
    matches!(f, Flag::Pair(k, _) if k == LINK_FLAG)
}

/// Whether `f` is a (kind, value) flag of `kind`: `flag_value((f,), kind) is not None` (no
/// flag the walk makes holds None).
fn is_kind(f: &Flag, kind: &str) -> bool {
    matches!(f, Flag::Pair(k, _) if k == kind)
}

/// `f in SHIFT_FLAGS or nested_shift(f)`.
fn is_shift(f: &Flag) -> bool {
    matches!(f, Flag::Str(s) if SHIFT_FLAGS.contains(&s.as_str())) || nested_shift(f).is_some()
}

/// `_near_black`: black, or near enough for Previewer to write no link colour on the page.
fn near_black(argb: Option<i64>) -> bool {
    argb.is_some_and(|c| [16, 8, 0].iter().all(|s| (c >> s) & 0xFF <= 0x1A))
}

/// `_draws_as_block(local, css)`: an inline element its CSS draws as a block.
fn draws_as_block(local: &str, display: &str) -> bool {
    if is_cell_block(local) || is_cell(local) || is_body_block(local) {
        return false;
    }
    if is_table_part(local) || local == "table" {
        return false;
    }
    matches!(
        display,
        "block"
            | "list-item"
            | "table"
            | "table-row"
            | "table-caption"
            | "flex"
            | "grid"
            | "flow-root"
    )
}

/// The bytes of text a flag set holds, charged when a segment takes a copy of it.
fn flags_bytes(flags: &Flags) -> usize {
    fn value(v: &PyValue) -> usize {
        match v {
            PyValue::Str(s) => s.len(),
            PyValue::List(items) | PyValue::Tuple(items) | PyValue::Set(items) => {
                items.iter().map(value).fold(0, usize::saturating_add)
            }
            PyValue::Dict(pairs) => pairs
                .iter()
                .map(|(k, v)| value(k).saturating_add(value(v)))
                .fold(0, usize::saturating_add),
            _ => 0,
        }
    }
    flags
        .iter()
        .map(|f| match f {
            Flag::Str(s) => s.len(),
            Flag::Pair(k, v) => k.len().saturating_add(value(v)),
        })
        .fold(0, usize::saturating_add)
}

/// `_own_anchor_ids(elem)`: its `id`, and an `<a>`'s `name`, each when not empty.
fn own_anchor_ids(dom: &Dom, n: NodeId) -> impl Iterator<Item = &str> {
    let id = if dom.is_comment(n) {
        None
    } else {
        dom.attr(n, "id")
    };
    let name = if !dom.is_comment(n) && dom.tag(n) == "a" {
        dom.attr(n, "name")
    } else {
        None
    };
    id.into_iter().chain(name).filter(|s| !s.is_empty())
}

/// `_epub_types(elem)`: its `epub:type` words, lower case.
fn epub_types(dom: &Dom, n: NodeId) -> Vec<String> {
    let raw = dom
        .attr_ns(n, OPS_NS, "type")
        .filter(|t| !t.is_empty())
        .or_else(|| plain_attr(dom, n, "epub:type"))
        .unwrap_or("");
    raw.to_lowercase()
        .split(py::is_space)
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `_is_non_rendered(elem)`: `hidden`, a `<template>` or `<iframe>`, or an `epub:type` of a
/// navigation kind.
fn is_non_rendered(dom: &Dom, n: NodeId) -> bool {
    if dom.attr(n, "hidden").is_some() {
        return true;
    }
    if matches!(dom.tag(n), "template" | "iframe") {
        return true;
    }
    epub_types(dom, n)
        .iter()
        .any(|t| t == "page-list" || t == "landmarks")
}

/// `_epub_switch_branch(elem)`: for an `<epub:switch>`, the ids of it and its cases, and its
/// first `<epub:default>`.
fn epub_switch_branch(dom: &Dom, n: NodeId) -> Option<(Vec<String>, Option<NodeId>)> {
    let named = |m: NodeId, local: &str, prefixed: &str| match dom.qualified(m) {
        (Some(ns), tag) => ns == OPS_NS && tag == local,
        (None, tag) => tag == prefixed,
    };
    if dom.is_comment(n) || !named(n, "switch", "epub:switch") {
        return None;
    }
    let mut ids: Vec<String> = own_anchor_ids(dom, n).map(str::to_owned).collect();
    let mut default = None;
    for &child in dom.children(n) {
        if dom.is_comment(child) {
            continue;
        }
        if named(child, "default", "epub:default") {
            default.get_or_insert(child);
        } else {
            ids.extend(subtree_anchor_ids(dom, child));
        }
    }
    Some((ids, default))
}

/// `_subtree_anchor_ids(elem)`: the anchor ids of `n` and every element under it.
fn subtree_anchor_ids(dom: &Dom, n: NodeId) -> Vec<String> {
    dom.subtree(n)
        .flat_map(|m| own_anchor_ids(dom, m))
        .map(str::to_owned)
        .collect()
}

/// `_nest_shift(cur, inherited, own)`: the shift this element sets inside one it inherits,
/// as a nested one ("sub@2"), "=" after the depth when it keeps the parent's size.
fn nest_shift(mut cur: Flags, inherited: &Flags, own: &[&str]) -> Flags {
    let depth = inherited.iter().filter(|f| is_shift(f)).count();
    if depth == 0 || own.is_empty() {
        return cur;
    }
    // Superscript wins over another shift, as it does on one run.
    let Some(shift) = SHIFT_FLAGS
        .iter()
        .chain([&BASELINE_SHIFT])
        .find(|f| own.contains(f))
    else {
        return cur;
    };
    let size = inherited.value(SIZE_FLAG);
    let keeps = if size.is_some() && cur.value(SIZE_FLAG) == size {
        "="
    } else {
        ""
    };
    let nested = format!("{shift}@{}{keeps}", depth + 1);
    cur.retain(|f| !matches!(f, Flag::Str(s) if own.contains(&s.as_str()) && !inherited.has(s)));
    cur.insert(Flag::Str(nested));
    cur
}

/// `_whole_baseline_dropped(parts, depth)`: a shifted element's parts without its children's
/// baseline shift when every part with text has it. A table among them has none.
fn whole_baseline_dropped(parts: &mut [Part], depth: usize) {
    let baseline = |flags: &Flags| {
        flags
            .iter()
            .find(|f| {
                nested_shift(f).is_some_and(|(s, d, _)| {
                    s == BASELINE_SHIFT && usize::try_from(d).is_ok_and(|d| d == depth + 1)
                })
            })
            .cloned()
    };
    let mut any = false;
    for p in parts.iter() {
        match p {
            Part::Table(_) => return,
            Part::Run(s) if !s.text.is_empty() => {
                if baseline(&s.flags).is_none() {
                    return;
                }
                any = true;
            }
            Part::Run(_) => {}
        }
    }
    if !any {
        return;
    }
    for p in parts {
        if let Part::Run(s) = p
            && let Some(b) = baseline(&s.flags)
        {
            s.flags.retain(|f| *f != b);
        }
    }
}

/// `_css_emphasis(css)`, None without a resolved style. kfxgen raises where
/// `css_typography` or `css_link_colour` does.
fn css_emphasis(css: Option<&Resolved>) -> Result<Option<Emphasis>, ItemError> {
    let Some(css) = css else {
        return Ok(None);
    };
    let weight = py::strip(&text_of(css.get("font-weight")?)).to_lowercase();
    let bold = weight == "bolder" || parse_weight(Some(&PyValue::Str(weight))) >= BOLD_WEIGHT;
    let families = font_families(css.get("font-family")?);
    let mono = is_monospace(&families).then_some(families);
    let italic = parse_style(css.get("font-style")?);
    let size = css.font_size().cloned();
    let t = css.typography()?;
    let s = |v: &str| PyValue::Str(v.to_owned());
    let typography = [
        t.small_caps.map(s),
        t.transform.map(s),
        t.spacing
            .map(|(mag, unit)| PyValue::Tuple(vec![PyValue::Str(mag), s(unit)])),
        t.colour.map(PyValue::Int),
        t.orientation.map(s),
        t.emphasis.map(PyValue::Str),
    ];
    let colour = text_of(css.get("color")?);
    let link_colour = css_link_colour(if colour.is_empty() { "black" } else { &colour })?;
    Ok(Some(Emphasis {
        bold,
        italic,
        mono,
        size,
        typography,
        link_colour,
    }))
}

/// What an element's [`Job`] is doing.
enum Kind {
    /// Its children, then the `<q>`'s close mark if it is one.
    Normal(Option<String>),
    /// An `<epub:switch>`: its default, if it has one and it is not walked yet.
    Switch(Option<NodeId>),
    /// A `<ruby>`: its base so far is in the job's buffer (`_ruby_parts`).
    Ruby,
    /// A tate-chu-yoko: its text, and the flags of each piece of its content with words.
    Tcy { text: String, worded: Vec<Flags> },
    /// A turned text: its content goes to the job's buffer; the `<q>`'s close mark.
    Turned {
        mode: &'static str,
        close: Option<String>,
    },
}

/// What follows the child a [`Job`] just had walked.
enum After {
    Nothing,
    /// Its tail.
    Tail(NodeId),
    /// A space, then its tail (a block walked inline).
    Spaced(NodeId),
    /// Its parts' flags read for a tate-chu-yoko, and its tail's.
    Worded(NodeId),
}

/// One element of the walk whose children are being walked: one call of `_walk_inline`
/// open on kfxgen's stack.
struct Job {
    n: NodeId,
    kind: Kind,
    /// Its flags (`cur`), which its content and children take.
    cur: Flags,
    /// Its emphasis, its children's `parent_emphasis`.
    emphasis: Option<Emphasis>,
    own_shifts: Vec<&'static str>,
    /// The `split_tables` its children are walked with.
    split_tables: bool,
    /// The `split_tables` it was given (a turned text's children are walked without).
    split_tables_given: bool,
    depth: usize,
    /// The buffer its parts go to, and the one its children's go to.
    buf: usize,
    inner: usize,
    /// Where its parts start in `buf`.
    start: usize,
    /// Its next child to look at.
    next: usize,
    after: After,
    /// Drawn as a block; a table cell inside the block.
    line: bool,
    cell: bool,
}

/// What a child is walked with: `_walk_inline`'s arguments from its parent.
struct Opening<'f> {
    flags: &'f Flags,
    root: bool,
    split_tables: bool,
    parent_emphasis: Option<&'f Emphasis>,
    depth: usize,
    buf: usize,
}

/// The inline walk over one book file, for one pass of its extraction: `_walk_inline` with
/// the file's style resolver and its href as `base_href`. It keeps per-element answers
/// between calls, and the `<q>` marks of the file.
pub struct Walk<'a, 's> {
    file: &'a File,
    dom: &'a Dom,
    resolver: &'a Resolver<'a>,
    serials: &'s mut Serials,
    hidden: Vec<Bit>,
    last_shown: Vec<Last>,
    text_before: Vec<Last>,
    /// Elements whose `data-kfxgen-horizontal`/`-vertical` marks `_turned_parts` took off
    /// (it pops them from the tree).
    unturned: Vec<bool>,
    /// `_Q_DOCUMENT`'s marks for this file, once a `<q>` asks.
    quotes: Option<Result<QMarks, Raised>>,
}

impl<'a, 's> Walk<'a, 's> {
    pub fn new(file: &'a File, resolver: &'a Resolver<'a>, serials: &'s mut Serials) -> Self {
        let n = file.dom.node_count();
        Walk {
            file,
            dom: &file.dom,
            resolver,
            serials,
            hidden: vec![Bit::Unknown; n],
            last_shown: vec![Last::Unknown; n],
            text_before: vec![Last::Unknown; n],
            unturned: vec![false; n],
            quotes: None,
        }
    }

    /// The book's serial counters.
    pub fn serials(&mut self) -> &mut Serials {
        self.serials
    }

    /// `_walk_inline(elem, style_resolver=…, base_href=…, split_tables=…)`, as a block calls
    /// it: `n`'s inline content, `n` itself read as the block (its own shift, hiding,
    /// direction and drawn-as-block display are not its runs').
    ///
    /// kfxgen recurses once per nested element; here each element open is a [`Job`] on a
    /// stack, and the parts of a ruby's base, a tate-chu-yoko's or a turned text's content go
    /// to a buffer of their own until the element is done.
    ///
    /// Errors: [`ItemError::Raised`] where kfxgen raises (it leaves the spine item out);
    /// [`Error::Unsupported`] for a `<math>`; [`Error::TooLarge`] past [`MAX_DEPTH`] levels of
    /// nesting, or once the file's text budget is spent.
    pub fn inline(&mut self, n: NodeId, split_tables: bool) -> Result<Vec<Part>, ItemError> {
        let mut bufs = vec![Vec::new()];
        let mut stack: Vec<Job> = Vec::new();
        let root = Opening {
            flags: &Flags::default(),
            root: true,
            split_tables,
            parent_emphasis: None,
            depth: 0,
            buf: 0,
        };
        if let Some(job) = self.begin(n, root, &mut bufs)? {
            stack.push(job);
        }
        while let Some(top) = stack.last_mut() {
            match self.step(top, &mut bufs)? {
                Some((child, split_tables)) => {
                    let top = &stack[stack.len() - 1];
                    let opening = Opening {
                        flags: &top.cur,
                        root: false,
                        split_tables,
                        parent_emphasis: top.emphasis.as_ref(),
                        depth: top.depth + 1,
                        buf: top.inner,
                    };
                    if let Some(job) = self.begin(child, opening, &mut bufs)? {
                        stack.push(job);
                    }
                }
                None => {
                    let Some(job) = stack.pop() else { break };
                    if let Some(again) = self.finish(job, &mut bufs)? {
                        stack.push(again);
                    }
                }
            }
        }
        Ok(bufs.swap_remove(0))
    }

    fn unsupported(&self, feature: &'static str) -> ItemError {
        ItemError::Error(Error::Unsupported {
            feature,
            file: self.file.href.clone(),
        })
    }

    /// A segment of `text` with a copy of `flags`, the copy charged.
    fn run(&self, text: impl Into<String>, flags: &Flags) -> Result<Part, ItemError> {
        self.resolver.charge(flags_bytes(flags))?;
        Ok(Part::Run(Segment {
            text: text.into(),
            flags: flags.clone(),
        }))
    }

    fn mark(id: &str) -> Part {
        Part::Run(make_anchor_mark(id))
    }

    fn line_start() -> Part {
        Part::Run(Segment {
            text: String::new(),
            flags: [Flag::Str(LINE_START.to_owned())].into_iter().collect(),
        })
    }

    fn space() -> Part {
        Part::Run(Segment {
            text: " ".to_owned(),
            flags: Flags::default(),
        })
    }

    /// `_is_hidden(elem, style_resolver)`, given `n`'s resolved style when the caller has it.
    fn is_hidden(&mut self, n: NodeId, css: Option<Option<&Resolved>>) -> Result<bool, ItemError> {
        match self.hidden[n.index()] {
            Bit::Yes => return Ok(true),
            Bit::No => return Ok(false),
            Bit::Unknown => {}
        }
        let hidden = if is_non_rendered(self.dom, n) {
            true
        } else {
            let display = match css {
                Some(css) => keyword(css, "display")?,
                None => keyword(self.resolver.resolve(n)?.as_ref(), "display")?,
            };
            display == "none"
        };
        self.hidden[n.index()] = if hidden { Bit::Yes } else { Bit::No };
        Ok(hidden)
    }

    /// `_last_shown(elem, style_resolver)`'s last character: a space for a `<br>`, an object
    /// mark for a picture, else the last character of its last text; None for nothing. A loop
    /// down the last children, each answer kept.
    fn last_shown(&mut self, n: NodeId) -> Result<Option<char>, ItemError> {
        if let Last::Known(c) = self.last_shown[n.index()] {
            return Ok(c);
        }
        // (element, children not yet looked at from the end).
        let mut open: Vec<(NodeId, usize)> = Vec::new();
        let mut answer = self.enter_shown(n, &mut open)?;
        loop {
            // An answer for the element on top of `open` (or `n`) closes it.
            if let Some(a) = answer.take() {
                match open.pop() {
                    Some((m, _)) => {
                        self.last_shown[m.index()] = Last::Known(a);
                        if a.is_some() || open.is_empty() {
                            // Found, or nothing in `n`: every open element has it.
                            for &(e, _) in &open {
                                self.last_shown[e.index()] = Last::Known(a);
                            }
                            return Ok(a);
                        }
                        // Nothing there: the parent looks before it.
                        continue;
                    }
                    None => return Ok(a),
                }
            }
            let Some(&mut (m, ref mut left)) = open.last_mut() else {
                unreachable!("an open element or an answer");
            };
            if *left == 0 {
                answer = Some(last(self.dom.text(m)));
                continue;
            }
            *left -= 1;
            let child = self.dom.children(m)[*left];
            if let Some(c) = last(self.dom.tail(child)) {
                answer = Some(Some(c));
                continue;
            }
            match self.last_shown[child.index()] {
                Last::Known(Some(c)) => answer = Some(Some(c)),
                Last::Known(None) => {}
                Last::Unknown => {
                    if let Some(Some(c)) = self.enter_shown(child, &mut open)? {
                        answer = Some(Some(c));
                    }
                }
            }
        }
    }

    /// Starts `_last_shown(m)`: its answer when it needs no look inside (a comment, hidden, a
    /// `<br>`, a picture), else `m` opened on `open` and None.
    fn enter_shown(
        &mut self,
        m: NodeId,
        open: &mut Vec<(NodeId, usize)>,
    ) -> Result<Option<Option<char>>, ItemError> {
        let direct = if self.dom.is_comment(m) || self.is_hidden(m, None)? {
            Some(None)
        } else {
            match self.dom.tag(m) {
                "br" => Some(Some(' ')),
                "img" | "svg" | "math" | "object" | "video" => Some(Some(OBJECT)),
                _ => None,
            }
        };
        match direct {
            Some(a) => {
                self.last_shown[m.index()] = Last::Known(a);
                Ok(Some(a))
            }
            None => {
                open.push((m, self.dom.children(m).len()));
                Ok(None)
            }
        }
    }

    /// `_text_before(node, style_resolver)`'s last character: the nearest text before `node`
    /// in its block, through the inline elements around it; None at the block's start. Each
    /// node passed on the way is given the answer.
    fn text_before(&mut self, node: NodeId) -> Result<Option<char>, ItemError> {
        let mut passed = Vec::new();
        let mut node = node;
        let answer = loop {
            if let Last::Known(c) = self.text_before[node.index()] {
                break c;
            }
            passed.push(node);
            if let Some(sib) = self.dom.prev_sibling(node) {
                if let Some(c) = last(self.dom.tail(sib)) {
                    break Some(c);
                }
                if let Some(c) = self.last_shown(sib)? {
                    break Some(c);
                }
                node = sib;
                continue;
            }
            let parent = self.dom.parent(node);
            if let Some(c) = parent.and_then(|p| last(self.dom.text(p))) {
                break Some(c);
            }
            match parent {
                Some(p) if is_link_inline(self.dom.tag(p)) => node = p,
                _ => break None,
            }
        };
        for n in passed {
            self.text_before[n.index()] = Last::Known(answer);
        }
        Ok(answer)
    }

    /// `_is_all_its_link_shows(elem, style_resolver)`: whether `elem` is all the link around
    /// it shows. Its work is the depth of `elem` under the link times the children on the way.
    fn is_all_its_link_shows(&mut self, elem: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        let Some(link) = dom
            .ancestors(elem)
            .find(|&a| !dom.is_comment(a) && dom.tag(a) == "a")
        else {
            return Ok(false);
        };
        for &k in dom.children(elem) {
            if !dom.is_comment(k) && !self.is_hidden(k, None)? {
                return Ok(false);
            }
        }
        let has_text = |s: &str| !py::strip(s).is_empty();
        if !has_text(dom.text(elem)) && !dom.children(elem).iter().any(|&k| has_text(dom.tail(k))) {
            return Ok(false);
        }
        let ahead = self.text_before(link)?;
        let lead_goes = ahead.is_none_or(|c| COLLAPSING.contains(&c));
        let mut node = link;
        while node != elem {
            let mut shows = None;
            for &k in dom.children(node) {
                if !dom.is_comment(k) && !self.is_hidden(k, None)? {
                    if shows.is_some() {
                        return Ok(false);
                    }
                    shows = Some(k);
                }
            }
            let Some(child) = shows else {
                return Ok(false);
            };
            if child != elem && !dom.ancestors(elem).any(|a| a == child) {
                return Ok(false);
            }
            let goes = |text: &str, leading: bool| {
                text.is_empty()
                    || (text.trim_matches(COLLAPSING).is_empty() && leading && lead_goes)
            };
            if !goes(dom.text(node), true) {
                return Ok(false);
            }
            let mut after = false;
            for &k in dom.children(node) {
                after = after || k == child;
                if !goes(dom.tail(k), !after) {
                    return Ok(false);
                }
            }
            node = child;
        }
        Ok(true)
    }

    /// `_white_space_flags(elem, style_resolver, inherited)`.
    fn white_space_flags(
        &self,
        n: NodeId,
        css: Option<&Resolved>,
        inherited: &Flags,
    ) -> Result<Vec<&'static str>, ItemError> {
        let value = keyword(css, "white-space")?;
        Ok(match value.as_str() {
            "" if self.dom.tag(n) == "pre" => vec![FLAG_PRE],
            "" => [FLAG_PRE, FLAG_PRE_LINE]
                .into_iter()
                .filter(|f| inherited.has(f))
                .collect(),
            "pre" | "pre-wrap" | "break-spaces" => vec![FLAG_PRE],
            "pre-line" => vec![FLAG_PRE_LINE],
            _ => Vec::new(),
        })
    }

    /// `_q_marks(q, style_resolver)`: (open, close) for a `<q>`.
    fn q_marks(&mut self, q: NodeId) -> Result<(String, String), ItemError> {
        if self.quotes.is_none() {
            let resolver = self.resolver;
            let dom = self.dom;
            let marks = document_q_marks(dom, |e| -> Result<_, ItemError> {
                let css = match resolver.resolve(e)? {
                    Some(r) => match r.get("q-css")? {
                        Some(PyValue::Dict(pairs)) => {
                            let get = |k: &str| {
                                pairs.iter().find_map(|(key, v)| match (key, v) {
                                    (PyValue::Str(key), PyValue::Str(v)) if key == k => {
                                        Some(v.clone())
                                    }
                                    _ => None,
                                })
                            };
                            Some(QCss {
                                quotes: get("quotes"),
                                before: get("before"),
                                after: get("after"),
                            })
                        }
                        _ => None,
                    },
                    None => None,
                };
                Ok((css, resolver.lang_of(e)?.map(str::to_owned)))
            });
            self.quotes = Some(match marks {
                Ok(m) => Ok(m),
                Err(ItemError::Raised(r)) => Err(r),
                Err(e) => return Err(e),
            });
        }
        match &self.quotes {
            Some(Ok(m)) => Ok(m.get(&q).cloned().unwrap_or_default()),
            Some(Err(r)) => Err(ItemError::Raised(r.clone())),
            None => unreachable!("worked out above"),
        }
    }

    /// The start of `_walk_inline(elem, flags, …)` for one element, up to its children: what
    /// it writes first goes to `bufs[at.buf]`, and its job is returned; None when it is done
    /// already (a comment, hidden, a `<br>`).
    fn begin(
        &mut self,
        n: NodeId,
        at: Opening<'_>,
        bufs: &mut Vec<Vec<Part>>,
    ) -> Result<Option<Job>, ItemError> {
        let dom = self.dom;
        if dom.is_comment(n) {
            // Markup, not content; the caller keeps its tail.
            return Ok(None);
        }
        if at.depth > MAX_DEPTH {
            return Err(ItemError::Error(Error::TooLarge {
                what: format!(
                    "{}: an element nested more than {MAX_DEPTH} levels inside a paragraph",
                    self.file.href
                ),
            }));
        }
        let out = at.buf;
        let own_css = self.resolver.resolve(n)?;
        if !at.root && self.is_hidden(n, Some(own_css.as_ref()))? {
            // Its ids keep their place; the caller keeps its tail (#256).
            for id in subtree_anchor_ids(dom, n) {
                bufs[out].push(Self::mark(&id));
            }
            return Ok(None);
        }
        let local = dom.tag(n);
        if local == "br" {
            for id in own_anchor_ids(dom, n) {
                bufs[out].push(Self::mark(id));
            }
            let flags = at.flags.without(&[FLAG_PRE, FLAG_PRE_LINE]);
            bufs[out].push(self.run(HARD_BREAK, &flags)?);
            return Ok(None);
        }
        let mut job = Job {
            n,
            kind: Kind::Normal(None),
            cur: Flags::default(),
            emphasis: None,
            own_shifts: Vec::new(),
            split_tables: at.split_tables,
            split_tables_given: at.split_tables,
            depth: at.depth,
            buf: out,
            inner: out,
            start: bufs[out].len(),
            next: 0,
            after: After::Nothing,
            line: false,
            cell: false,
        };
        if let Some((ids, default)) = epub_switch_branch(dom, n) {
            for id in &ids {
                bufs[out].push(Self::mark(id));
            }
            // The default is walked as the switch's parent's content would be.
            self.resolver.charge(flags_bytes(at.flags))?;
            job.kind = Kind::Switch(default);
            job.cur = at.flags.clone();
            job.emphasis = at.parent_emphasis.cloned();
            return Ok(Some(job));
        }
        let (cur, emphasis, own_shifts) =
            self.element_flags(n, at.flags, at.root, own_css.as_ref(), at.parent_emphasis)?;
        // Every element takes a copy of the flags around it.
        self.resolver.charge(flags_bytes(&cur))?;
        job.cur = cur;
        job.emphasis = emphasis;
        job.own_shifts = own_shifts;
        // A <span> drawn as a block is a line of the paragraph (#233). kfxgen reads the
        // display only for an element that is not the block itself.
        let display = if at.root {
            String::new()
        } else {
            keyword(own_css.as_ref(), "display")?
        };
        job.line = draws_as_block(local, &display);
        job.cell = is_cell(local) && !at.root;
        if job.line {
            bufs[out].push(Self::line_start());
        }
        if job.cell {
            bufs[out].push(Self::space());
        }
        for id in own_anchor_ids(dom, n) {
            bufs[out].push(Self::mark(id));
        }
        if local == "ruby" {
            let text = dom.text(n);
            bufs.push(Vec::new());
            job.inner = bufs.len() - 1;
            if !text.is_empty() {
                let part = self.run(text, &job.cur)?;
                bufs[job.inner].push(part);
            }
            job.kind = Kind::Ruby;
            return Ok(Some(job));
        }
        if local == "math" {
            return Err(self.unsupported("math"));
        }
        // 縦中横, in a vertical-rl file only (mark_vertical_margins marked it).
        if !at.root
            && dom.attr(n, V_AUTHOR).is_some()
            && keyword(own_css.as_ref(), "text-combine-upright")? == "all"
        {
            let text = dom.itertext(n);
            // Read once per tate-chu-yoko; one inside another reads it again.
            self.resolver.charge(text.len())?;
            let tcy = words(&text);
            if !tcy.is_empty() {
                let mut worded = Vec::new();
                if !py::strip(dom.text(n)).is_empty() {
                    worded.push(job.cur.clone());
                }
                bufs.push(Vec::new());
                job.inner = bufs.len() - 1;
                job.kind = Kind::Tcy { text: tcy, worded };
                return Ok(Some(job));
            }
        }
        let turn = if self.unturned[n.index()] {
            None
        } else if dom.attr(n, HORIZONTAL).is_some() {
            Some("horizontal")
        } else if dom.attr(n, VERTICAL).is_some() {
            Some("vertical")
        } else {
            None
        };
        if let Some(mode) = turn
            && !at.root
            && is_turn_tag(local)
            && !matches!(
                keyword(own_css.as_ref(), "display")?.as_str(),
                "block" | "list-item" | "table" | "flex" | "grid"
            )
            // Previewer doesn't turn one in a table cell (QA's Q765 Y04).
            && !dom
                .ancestors(n)
                .any(|a| !dom.is_comment(a) && is_cell(dom.tag(a)))
            && self.begin_turned(&mut job, mode, bufs)?
        {
            return Ok(Some(job));
        }
        self.begin_normal(&mut job, bufs)?;
        Ok(Some(job))
    }

    /// `_walk_inline` past its special cases: the `<q>`'s open mark and the element's own
    /// text; its children come next.
    fn begin_normal(&mut self, job: &mut Job, bufs: &mut [Vec<Part>]) -> Result<(), ItemError> {
        let dom = self.dom;
        let quote = if dom.tag(job.n) == "q" {
            Some(self.q_marks(job.n)?)
        } else {
            None
        };
        if let Some((open, _)) = &quote {
            let part = self.run(open.as_str(), &job.cur)?;
            bufs[job.buf].push(part);
        }
        let mut text = dom.text(job.n);
        if dom.tag(job.n) == "pre" {
            // HTML ignores a newline straight after <pre>; the XML parser keeps it.
            text = text
                .strip_prefix("\r\n")
                .or_else(|| text.strip_prefix('\n'))
                .unwrap_or(text);
        }
        if !text.is_empty() {
            let part = self.run(text, &job.cur)?;
            bufs[job.buf].push(part);
        }
        job.kind = Kind::Normal(quote.map(|(_, close)| close));
        job.next = 0;
        job.after = After::Nothing;
        Ok(())
    }

    /// The start of `_turned_parts(elem, cur, …, mode)`: false when it holds a picture, a
    /// ruby or an equation (it stays in the line). Like kfxgen, it then takes the turn marks
    /// off every element inside it, for good, and walks its content into a buffer of its own.
    fn begin_turned(
        &mut self,
        job: &mut Job,
        mode: &'static str,
        bufs: &mut Vec<Vec<Part>>,
    ) -> Result<bool, ItemError> {
        let dom = self.dom;
        let n = job.n;
        let inside = || dom.subtree(n).skip(1).filter(|&d| !dom.is_comment(d));
        if inside().any(|d| matches!(dom.tag(d), "img" | "svg" | "math" | "ruby")) {
            return Ok(false);
        }
        for d in inside() {
            self.unturned[d.index()] = true;
        }
        let quote = if dom.tag(n) == "q" {
            Some(self.q_marks(n)?)
        } else {
            None
        };
        bufs.push(Vec::new());
        job.inner = bufs.len() - 1;
        if let Some((open, _)) = &quote {
            let part = self.run(open.as_str(), &job.cur)?;
            bufs[job.inner].push(part);
        }
        // kfxgen's None text and tails are left out; an empty one would add nothing.
        if !dom.text(n).is_empty() {
            let part = self.run(dom.text(n), &job.cur)?;
            bufs[job.inner].push(part);
        }
        job.kind = Kind::Turned {
            mode,
            close: quote.map(|(_, close)| close),
        };
        // Not passed on: `_turned_parts` walks its children with `split_tables` False.
        job.split_tables = false;
        Ok(true)
    }

    /// The next child of `job` to walk, and its `split_tables`; None when `job` has no more.
    /// What `job` writes between its children (tails, a picture's ids, a cell block's spaces,
    /// a ruby's readings) is written on the way.
    fn step(
        &mut self,
        job: &mut Job,
        bufs: &mut [Vec<Part>],
    ) -> Result<Option<(NodeId, bool)>, ItemError> {
        let dom = self.dom;
        let kids = dom.children(job.n);
        loop {
            // What follows the child just walked.
            match std::mem::replace(&mut job.after, After::Nothing) {
                After::Nothing => {}
                After::Tail(child) => self.tail(job, child, job.inner, bufs)?,
                After::Spaced(child) => {
                    bufs[job.inner].push(Self::space());
                    self.tail(job, child, job.inner, bufs)?;
                }
                After::Worded(child) => {
                    let Kind::Tcy { worded, .. } = &mut job.kind else {
                        unreachable!("only a tate-chu-yoko reads its children's words");
                    };
                    for p in bufs[job.inner].drain(..) {
                        if let Part::Run(s) = p
                            && !py::strip(&s.text).is_empty()
                        {
                            worded.push(s.flags);
                        }
                    }
                    if !py::strip(dom.tail(child)).is_empty() {
                        worded.push(job.cur.clone());
                    }
                }
            }
            if let Kind::Switch(default) = &mut job.kind {
                return Ok(default.take().map(|d| (d, job.split_tables)));
            }
            let Some(&child) = kids.get(job.next) else {
                return Ok(None);
            };
            job.next += 1;
            let clocal = if dom.is_comment(child) {
                ""
            } else {
                dom.tag(child)
            };
            match &job.kind {
                Kind::Switch(_) => unreachable!("answered above"),
                Kind::Ruby if clocal == "rt" => {
                    self.ruby_reading(job, child, bufs)?;
                    self.tail(job, child, job.inner, bufs)?;
                }
                Kind::Ruby if clocal == "rp" => self.tail(job, child, job.inner, bufs)?,
                Kind::Ruby | Kind::Turned { .. } => {
                    job.after = After::Tail(child);
                    return Ok(Some((child, job.split_tables)));
                }
                Kind::Tcy { .. } => {
                    job.after = After::Worded(child);
                    return Ok(Some((child, job.split_tables)));
                }
                Kind::Normal(_) => {
                    if job.split_tables && clocal == "table" {
                        // The caller writes it as a table of its own (#263).
                        bufs[job.buf].push(Part::Table(child));
                    } else if matches!(clocal, "img" | "svg") && self.is_hidden(child, None)? {
                        // A hidden picture, as hidden text: only its ids (#428).
                        for id in subtree_anchor_ids(dom, child) {
                            bufs[job.buf].push(Self::mark(&id));
                        }
                    } else if matches!(clocal, "img" | "svg") {
                        for id in own_anchor_ids(dom, child) {
                            bufs[job.buf].push(Self::mark(id));
                        }
                        // Task 10 writes the picture's token.
                        return Err(self.unsupported("pictures in text (Task 10)"));
                    } else if is_cell_block(clocal) || clocal == "option" {
                        // A block walked inline: its edges are a word boundary (#279, #477).
                        bufs[job.buf].push(Self::space());
                        job.after = After::Spaced(child);
                        return Ok(Some((child, job.split_tables)));
                    } else {
                        job.after = After::Tail(child);
                        return Ok(Some((child, job.split_tables)));
                    }
                    self.tail(job, child, job.buf, bufs)?;
                }
            }
        }
    }

    /// `if child.tail: parts.append((child.tail, cur))`: the tail sits inside `job`'s element,
    /// so it carries `cur` (#59).
    fn tail(
        &self,
        job: &Job,
        child: NodeId,
        to: usize,
        bufs: &mut [Vec<Part>],
    ) -> Result<(), ItemError> {
        let tail = self.dom.tail(child);
        if !tail.is_empty() {
            let part = self.run(tail, &job.cur)?;
            bufs[to].push(part);
        }
        Ok(())
    }

    /// An `<rt>` in `_ruby_parts`: its reading on each run of the base before it, then the
    /// base and the `<rt>`'s ids written.
    fn ruby_reading(
        &mut self,
        job: &Job,
        rt: NodeId,
        bufs: &mut [Vec<Part>],
    ) -> Result<(), ItemError> {
        let dom = self.dom;
        let reading = words(&dom.itertext(rt));
        let mut base = std::mem::take(&mut bufs[job.inner]);
        if !reading.is_empty() {
            let flag = make_ruby_flag(next(&mut self.serials.ruby), &reading);
            for p in &mut base {
                if let Part::Run(s) = p
                    && !s.text.is_empty()
                {
                    self.resolver.charge(reading.len())?;
                    s.flags.insert(flag.clone());
                }
            }
        }
        bufs[job.buf].append(&mut base);
        for id in subtree_anchor_ids(dom, rt) {
            bufs[job.buf].push(Self::mark(&id));
        }
        Ok(())
    }

    /// The end of `_walk_inline` for `job`'s element once its children are walked. A turned
    /// text with no text in it is walked again as ordinary content: its job is returned.
    fn finish(
        &mut self,
        mut job: Job,
        bufs: &mut Vec<Vec<Part>>,
    ) -> Result<Option<Job>, ItemError> {
        let dom = self.dom;
        let out = job.buf;
        match std::mem::replace(&mut job.kind, Kind::Normal(None)) {
            Kind::Switch(_) => {}
            Kind::Normal(close) => {
                if let Some(close) = close {
                    let part = self.run(close, &job.cur)?;
                    bufs[out].push(part);
                }
                if !job.own_shifts.is_empty() {
                    let shifts = job.cur.iter().filter(|f| is_shift(f)).count();
                    whole_baseline_dropped(&mut bufs[out][job.start..], shifts);
                }
                // Close a cell with a boundary so the next one cannot fuse onto it (#128).
                if job.cell {
                    bufs[out].push(Self::space());
                }
                if job.line {
                    bufs[out].push(Self::line_start());
                }
            }
            Kind::Ruby => {
                let mut base = bufs.pop().unwrap_or_default();
                bufs[out].append(&mut base);
                if job.line {
                    bufs[out].push(Self::line_start());
                }
            }
            Kind::Tcy { text, worded } => {
                bufs.pop();
                // The style all its words share (QA's P748 X06, P748d D1).
                let mut own = match worded.split_first() {
                    Some((first, rest)) => {
                        let mut own = first.clone();
                        own.retain(|f| rest.iter().all(|w| w.contains(f)));
                        own
                    }
                    None => job.cur.clone(),
                };
                for d in dom.subtree(job.n).skip(1) {
                    for id in own_anchor_ids(dom, d) {
                        bufs[out].push(Self::mark(id));
                    }
                }
                own.insert(make_tcy_flag(next(&mut self.serials.tcy), &text));
                let part = self.run(OBJECT, &own)?;
                bufs[out].push(part);
            }
            Kind::Turned { mode, close } => {
                let mut inner = bufs.pop().unwrap_or_default();
                if let Some(close) = close {
                    inner.push(self.run(close, &job.cur)?);
                }
                let inner: Vec<Segment> = inner
                    .into_iter()
                    .filter_map(|p| match p {
                        Part::Run(s) => Some(s),
                        Part::Table(_) => None,
                    })
                    .collect();
                let (mut text, spans, _) = normalize_runs_with_anchors(&inner);
                if py::strip(&text).is_empty() {
                    // It stays in the line, walked as ordinary content.
                    job.inner = out;
                    job.split_tables = job.split_tables_given;
                    self.begin_normal(&mut job, bufs)?;
                    return Ok(Some(job));
                }
                let raw_last = inner.iter().rev().find_map(|s| last(&s.text));
                if raw_last.is_some_and(|c| COLLAPSING.contains(&c)) && !text.ends_with(' ') {
                    // Previewer keeps a space it ends with, inside it (QA's Q765 Y08, Y12).
                    text.push(' ');
                }
                for d in dom.subtree(job.n).skip(1) {
                    for id in own_anchor_ids(dom, d) {
                        bufs[out].push(Self::mark(id));
                    }
                }
                let flag = make_turn_flag(next(&mut self.serials.turn), mode, &text, &spans);
                let part = self.run(OBJECT, &[flag].into_iter().collect())?;
                bufs[out].push(part);
            }
        }
        Ok(None)
    }

    /// The flags `_walk_inline` gives `n`'s own content (`cur`), its emphasis (passed to its
    /// children as `parent_emphasis`) and the shifts it sets itself. Kept out of [`Self::walk`]
    /// so the recursion's frames stay small.
    #[inline(never)]
    fn element_flags(
        &mut self,
        n: NodeId,
        flags: &Flags,
        root: bool,
        own_css: Option<&Resolved>,
        parent_emphasis: Option<&Emphasis>,
    ) -> Result<(Flags, Option<Emphasis>, Vec<&'static str>), ItemError> {
        let dom = self.dom;
        let local = dom.tag(n);
        let mut cur = flags.without(&[FLAG_PRE, FLAG_PRE_LINE]);
        for f in self.white_space_flags(n, own_css, flags)? {
            cur.insert(Flag::Str(f.to_owned()));
        }
        let str_flag = |s: &str| Flag::Str(s.to_owned());
        if matches!(local, "em" | "i") {
            cur.insert(str_flag(FLAG_ITALIC));
        }
        if matches!(local, "strong" | "b") {
            cur.insert(str_flag(FLAG_BOLD));
        }
        // The shifts this element sets itself (#536).
        let mut own_shifts = Vec::new();
        if local == "sup" {
            cur.insert(str_flag(FLAG_SUPER));
            own_shifts.push(FLAG_SUPER);
        }
        if local == "sub" {
            cur.insert(str_flag(FLAG_SUB));
            own_shifts.push(FLAG_SUB);
        }
        let mut starts_link = false;
        if local == "a" {
            let href = dom.attr(n, "href").unwrap_or("");
            let target =
                resolve_link_target(href, &self.file.href).or_else(|| external_target(href));
            if let Some(target) = target {
                starts_link = true;
                // A link inside a link: the inner one wins, without the outer's noteref or
                // colour (#317, #728).
                cur.retain(|f| {
                    !is_link_flag(f)
                        && !matches!(f, Flag::Str(s) if s == NOTEREF_FLAG)
                        && !is_kind(f, LINK_COLOUR_FLAG)
                });
                cur.insert(make_link_flag(&target));
                if epub_types(dom, n).iter().any(|t| t == "noteref") {
                    cur.insert(str_flag(NOTEREF_FLAG));
                }
            }
        }
        let emphasis = css_emphasis(own_css)?;
        let lines: Vec<String> = match own_css {
            // CSS decides, tags included (#360 review).
            Some(css) if css.has("text-decoration") => text_of(css.get("text-decoration")?)
                .split(py::is_space)
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect(),
            _ => {
                let mut lines = Vec::new();
                if matches!(local, "u" | "ins") {
                    lines.push("underline".to_owned());
                }
                if matches!(local, "s" | "strike" | "del") {
                    lines.push("line-through".to_owned());
                }
                lines
            }
        };
        if lines.iter().any(|l| l == "underline") {
            cur.insert(str_flag(FLAG_UNDERLINE));
        }
        if lines.iter().any(|l| l == "line-through") {
            cur.insert(str_flag(FLAG_STRIKE));
        }
        if !root {
            if let Some(css) = own_css {
                let align = text_of(css.get("vertical-align")?);
                let shift = parse_vertical_align(&align);
                match shift {
                    Some("super") => cur.insert(str_flag(FLAG_SUPER)),
                    Some("sub") => cur.insert(str_flag(FLAG_SUB)),
                    Some("text-top") => cur.insert(str_flag(FLAG_TEXT_TOP)),
                    Some("text-bottom") => cur.insert(str_flag(FLAG_TEXT_BOTTOM)),
                    _ => {}
                }
                match shift {
                    Some(s) if SHIFT_FLAGS.contains(&s) => own_shifts.push(s),
                    _ => {
                        if py::strip(&align).to_lowercase() == "baseline" {
                            // Inside a shift only (#638).
                            own_shifts.push(BASELINE_SHIFT);
                        }
                    }
                }
            }
            let computed;
            let parent_emphasis = match (parent_emphasis, dom.parent(n)) {
                (None, Some(p)) => {
                    computed = css_emphasis(self.resolver.resolve(p)?.as_ref())?;
                    computed.as_ref()
                }
                (pe, _) => pe,
            };
            if let (Some(e), Some(pe), Some(css)) = (&emphasis, parent_emphasis, own_css) {
                self.emphasis_flags(n, &mut cur, e, pe, css, starts_link)?;
            }
        }
        if matches!(local, "code" | "kbd" | "samp" | "tt")
            && !own_css.is_some_and(|c| c.has("font-family"))
        {
            cur.insert(make_mono_flag(&["monospace".to_owned()]));
        }
        if !root {
            let direction = keyword(own_css, "own-direction")?;
            let unicode_bidi = keyword(own_css, "own-unicode-bidi")?;
            let keys = inline_bidi(dom, n, &direction, &unicode_bidi);
            if keys.direction.is_some() || keys.unicode_bidi.is_some() {
                // Its own direction: a run over its whole text, its language in it (#710).
                let depth = 1 + cur.iter().filter(|f| is_kind(f, BIDI_FLAG)).count();
                let serial = next(&mut self.serials.bidi);
                cur.insert(make_bidi_flag(
                    serial,
                    i64::try_from(depth).unwrap_or(i64::MAX),
                    &keys.keys(),
                ));
            } else if let Some(lang) = keys.lang {
                // A language alone is a run style, as bold is.
                cur.retain(|f| !is_kind(f, LANG_FLAG));
                cur.insert(Flag::Pair(LANG_FLAG.to_owned(), PyValue::Str(lang)));
            }
        }
        let cur = nest_shift(cur, flags, &own_shifts);
        Ok((cur, emphasis, own_shifts))
    }

    /// The part of `_walk_inline` that compares an element's emphasis with its parent's:
    /// bold and italic starting or set back, monospace, size, typography and link colour.
    fn emphasis_flags(
        &mut self,
        n: NodeId,
        cur: &mut Flags,
        e: &Emphasis,
        pe: &Emphasis,
        css: &Resolved,
        starts_link: bool,
    ) -> Result<(), ItemError> {
        let str_flag = |s: &str| Flag::Str(s.to_owned());
        for (flag, not_flag, prop, now, before) in [
            (FLAG_BOLD, FLAG_NOT_BOLD, "font-weight", e.bold, pe.bold),
            (
                FLAG_ITALIC,
                FLAG_NOT_ITALIC,
                "font-style",
                e.italic,
                pe.italic,
            ),
        ] {
            if now && !before {
                cur.insert(str_flag(flag));
            } else if !now && (before || css.has(prop)) {
                // Normal here: under bold, or a <b>/<i> whose CSS says normal (#362).
                if cur.has(flag) {
                    cur.retain(|f| !matches!(f, Flag::Str(s) if s == flag));
                } else if before {
                    // The emphasis is the paragraph's own: say normal (#367 review).
                    cur.insert(str_flag(not_flag));
                }
            }
        }
        if e.mono != pe.mono {
            // Monospace starts, ends, or changes family (#242).
            cur.retain(|f| !is_kind(f, MONO_FLAG));
            if let Some(families) = &e.mono {
                cur.insert(make_mono_flag(families));
            }
        }
        if e.size != pe.size {
            // A size of its own, or back to its parent's (#241).
            cur.retain(|f| !is_kind(f, SIZE_FLAG));
            if let Some(size) = &e.size {
                cur.insert(make_size_flag(size.clone()));
            }
        }
        let spacing_normal = PyValue::Tuple(vec![
            PyValue::Str(SPACING_NORMAL.0.to_owned()),
            PyValue::Str(SPACING_NORMAL.1.to_owned()),
        ]);
        let unset = [
            Some(PyValue::Str("normal".to_owned())),
            Some(PyValue::Str("none".to_owned())),
            Some(spacing_normal),
            None,
            Some(PyValue::Str("mixed".to_owned())),
            Some(PyValue::Str("none".to_owned())),
        ];
        for (i, kind) in TYPOGRAPHY_FLAGS.into_iter().enumerate() {
            let (now, before) = (&e.typography[i], &pe.typography[i]);
            if kind == COLOUR_FLAG && cur.iter().any(is_link_flag) {
                // A link's own colour is the link's colour, `$576` (#728).
                if now.is_none() || e.link_colour == pe.link_colour {
                    continue;
                }
                let mut link_colour = e.link_colour;
                let behind = css.get("background-behind")?;
                if behind.is_none_or(|v| *v == PyValue::None) && near_black(link_colour) {
                    // On the page, black and near-black are no link colour.
                    link_colour = None;
                    if !self.is_all_its_link_shows(n)?
                        && cur.iter().any(|f| is_kind(f, LINK_COLOUR_FLAG))
                    {
                        // A black word inside a coloured link takes the link's colour.
                        continue;
                    }
                }
                if starts_link {
                    cur.retain(|f| !is_kind(f, kind));
                }
                cur.retain(|f| !is_kind(f, LINK_COLOUR_FLAG));
                let Some(now) = now else { continue };
                cur.insert(Flag::Pair(
                    LINK_COLOUR_FLAG.to_owned(),
                    PyValue::Tuple(vec![
                        link_colour.map_or(PyValue::None, PyValue::Int),
                        now.clone(),
                    ]),
                ));
                continue;
            }
            if now == before || (kind == COLOUR_FLAG && now.is_none()) {
                continue;
            }
            cur.retain(|f| !is_kind(f, kind));
            if let Some(value) = now.as_ref().or(unset[i].as_ref()) {
                cur.insert(Flag::Pair(kind.to_owned(), value.clone()));
            }
        }
        Ok(())
    }
}
