// SPDX-License-Identifier: GPL-3.0-only
//! Native tables, at kfxgen `ad38ec8`: the table branch of `_walk_element` (`_table_is_native`,
//! `_is_notes_table`, the contents fallback, `_emit_fostered`, captions), `_table_block` with its
//! closures (`take`, `build_cell`, `holds_box`, `spaced_block`, `clamp_rowspans`, `own_bidi`,
//! `part_border`), `_walk_cell`, and the module helpers they call: `_fostered_content`,
//! `_foster_parents`, `_is_contentless_anchor`, `_table_start_ids`, `_cell_columns`,
//! `_table_width`, `_column_widths`, `_width_pct`, `_span_attr`, `_attr_border_px`,
//! `_cell_valign`, `_only_block_child`, `_table_dir`, `_cell_block_style`, `_cell_start_end`,
//! `_header_not_bold`, `_table_cell`, `_paragraph_cell`, `_cell_ids`, `_cell_padding`,
//! `_text_length`, `_cell_paragraph_count`, `_loose_cell_text`, `_longest_cell_run`,
//! `_row_anchor_ids` and `_NOTE_MARKER_RE`.
//!
//! kfxgen builds a table while it walks it: `_table_block` walks a cell of several blocks with
//! the ordinary block walk (`_walk_cell`), and the table then walks what sits outside its cells
//! and its caption the same way. Here the table being built is a [`TableJob`] on the walk's
//! stack ([`Frame::Table`], [`Frame::CellEnd`]), so no walk recurses on the book's nesting. A
//! native table holds no table (`_table_is_native`), so a cell's walk never starts another.
//!
//! Work is linear in the table: every cell is built once, its subtree read once for what
//! `build_cell` and `_table_is_native` count in it (kept per node, [`Counts`]), and the walks up
//! from a cell (`_cell_valign`, a header cell's `inherit`) are kept per element. Spans are
//! clamped to 1 to 1,000 as kfxgen clamps them, and a native table is at most 24 columns wide,
//! so a row's carried rowspans are at most 24 long. A `<col span>` repeats a width up to 1,000
//! times in the model: each repeat is charged to the file's budget first.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::mem::size_of;

use deckle_limits::MAX_DEPTH;

use super::boxes::{border_py, is_box_tag};
use super::rules::{BORDER_SIDES, css_border};
use super::spacing::{Pad, margin_em, padding_em, space_between, truthy};
use super::walk::{
    Aside, Frame, HORIZONTAL, Walker, anchors_follow_rows, has_words, is_block_tag,
    is_empty_anchor, is_nav_listing, offsets, text_field,
};
use super::{Block, BlockId, Field, dedupe, dict_set, s, strs};
use crate::convert::block_bidi::start_end_align;
use crate::convert::block_style::{align_keyword, compute_block_style};
use crate::convert::font_table::parse_weight;
use crate::convert::href::key_doc;
use crate::convert::inline::{
    epub_switch_branch, keyword, own_anchor_ids, subtree_anchor_ids, text_of, words,
};
use crate::convert::inline_style::{Flags, Segment, make_anchor_mark, normalize_runs_with_anchors};
use crate::convert::resolver::Resolved;
use crate::convert::{ItemError, Raised};
use crate::dom::{Dom, NodeId};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// The style text budget units each table cell is charged before the table is read (not in
/// kfxgen; see `DIVERGENCES.md`).
const CELL_UNITS: usize = 3;

/// `_MAX_NATIVE_CELL_CHARS`: the longest cell entry a native table of two or more columns takes.
const MAX_NATIVE_CELL_CHARS: usize = 2000;
/// `_MAX_NATIVE_COLUMNS`: the widest native table.
const MAX_NATIVE_COLUMNS: usize = 24;
/// `_DEFAULT_PADDING_EM`: the cell padding the generator writes anyway.
const DEFAULT_PADDING_EM: f64 = 0.45 / 12.0;
/// `_NOTES_TABLE_LINKED_SHARE`.
const NOTES_TABLE_LINKED_SHARE: f64 = 0.8;
/// `_SOURCE_ALIGN`: a cell's `align` attribute as the book wrote it.
const SOURCE_ALIGN: &str = "data-kfxgen-align";
/// `_shown_tree` copies a table holding an `<epub:switch>` and asks the style resolver and
/// `_is_nav_listing` about the copies, which calibre never styled.
const SWITCH: &str = "tables holding an epub:switch";

const WIDTH_NOT_A_NUMBER: Raised = Raised {
    exception: "ValueError",
    note: "_width_pct: float() of a width's points and digits, such as \".%\" or \"1.2.3%\"",
};
const BORDER_DIGITS: Raised = Raised {
    exception: "ValueError",
    note: "_attr_border_px: int() of a border attribute of more than 4300 digits",
};
const BORDER_TOO_LARGE: Raised = Raised {
    exception: "OverflowError",
    note: "_table_block: a border attribute too large to convert to float",
};
const NO_ALIGN: Raised = Raised {
    exception: "KeyError",
    note: "_cell_block_style: a header cell's paragraph whose block style has no align",
};

/// `_CELL_TAGS`.
pub(super) fn is_cell(local: &str) -> bool {
    matches!(local, "td" | "th")
}

/// `_ROW_GROUP_TAGS`.
fn is_row_group_tag(local: &str) -> bool {
    matches!(local, "table" | "thead" | "tbody" | "tfoot")
}

/// `_ROW_GROUPS`.
fn row_group(local: &str) -> Option<&'static str> {
    match local {
        "thead" => Some("head"),
        "tbody" => Some("body"),
        "tfoot" => Some("foot"),
        _ => None,
    }
}

/// `_ROW_GROUP_ORDER`.
fn group_order(group: &str) -> u8 {
    match group {
        "head" => 0,
        "body" => 1,
        _ => 2,
    }
}

/// `_TABLE_PART_TAGS | _CELL_TAGS`.
fn is_table_part(local: &str) -> bool {
    matches!(
        local,
        "tr" | "thead" | "tbody" | "tfoot" | "caption" | "col" | "colgroup" | "td" | "th"
    )
}

/// `_CELL_BLOCK_TAGS`.
fn is_cell_block(local: &str) -> bool {
    matches!(
        local,
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

/// `_NON_TEXT_TAGS`.
pub(super) fn is_non_text(local: &str) -> bool {
    matches!(
        local,
        "img" | "svg" | "image" | "math" | "video" | "audio" | "object" | "embed" | "iframe"
    )
}

/// `_HEADING_TAGS`.
fn is_heading(local: &str) -> bool {
    matches!(local, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// `_local_tag(n.tag)`: None for a comment.
fn local(dom: &Dom, n: NodeId) -> Option<&str> {
    (!dom.is_comment(n)).then(|| dom.tag(n))
}

/// What `_table_is_native` and `build_cell` count in a node's subtree, the node included,
/// worked out once per node, children first.
#[derive(Clone, Copy, Default)]
struct Counts {
    /// `len("".join(n.itertext()))`: code points of its text and of the text and tails below.
    chars: usize,
    /// The same in UTF-8 bytes.
    bytes: usize,
    /// `<br>`s, itself included.
    brs: usize,
    /// `_CELL_BLOCK_TAGS` elements below it (`_cell_paragraph_count`).
    blocks: usize,
    /// The `_text_length` of the cell blocks below it that hold no cell block: their sum and
    /// the longest.
    leaf_sum: usize,
    leaf_max: usize,
    /// A heading, itself included.
    heading: bool,
    /// An `<img>`, itself included.
    img: bool,
}

impl Counts {
    /// `_text_length(n)`: its characters, plus one per `<br>`.
    fn length(&self) -> usize {
        self.chars.saturating_add(self.brs)
    }

    /// `_longest_cell_run(cell)`: its whole text with fewer than two blocks, else its longest
    /// innermost block or its loose text (all text less its innermost blocks').
    fn longest_run(&self) -> usize {
        if self.blocks < 2 {
            self.length()
        } else {
            self.leaf_max
                .max(self.length().saturating_sub(self.leaf_sum))
        }
    }
}

/// The [`Counts`] of `n`, worked out for its subtree where not known yet, with no recursion.
fn counts(dom: &Dom, memo: &mut HashMap<NodeId, Counts>, n: NodeId) -> Counts {
    if let Some(c) = memo.get(&n) {
        return *c;
    }
    // (node, children done)
    let mut stack = vec![(n, false)];
    while let Some((m, done)) = stack.pop() {
        if memo.contains_key(&m) {
            continue;
        }
        if !done {
            stack.push((m, true));
            for &c in dom.children(m) {
                if !dom.is_comment(c) {
                    stack.push((c, false));
                }
            }
            continue;
        }
        let text = dom.text(m);
        let tag = dom.tag(m);
        let mut out = Counts {
            chars: text.chars().count(),
            bytes: text.len(),
            brs: usize::from(tag == "br"),
            heading: is_heading(tag),
            img: tag == "img",
            ..Counts::default()
        };
        for &c in dom.children(m) {
            let tail = dom.tail(c);
            out.chars = out.chars.saturating_add(tail.chars().count());
            out.bytes = out.bytes.saturating_add(tail.len());
            // A comment's own text is not in `itertext()`; its tail is.
            if dom.is_comment(c) {
                continue;
            }
            let k = memo.get(&c).copied().unwrap_or_default();
            out.chars = out.chars.saturating_add(k.chars);
            out.bytes = out.bytes.saturating_add(k.bytes);
            out.brs = out.brs.saturating_add(k.brs);
            out.heading |= k.heading;
            out.img |= k.img;
            let block = is_cell_block(dom.tag(c));
            out.blocks = out
                .blocks
                .saturating_add(k.blocks)
                .saturating_add(usize::from(block));
            let leaf = if block && k.blocks == 0 {
                k.length()
            } else {
                k.leaf_sum
            };
            out.leaf_sum = out.leaf_sum.saturating_add(leaf);
            out.leaf_max = out.leaf_max.max(if block && k.blocks == 0 {
                k.length()
            } else {
                k.leaf_max
            });
        }
        memo.insert(m, out);
    }
    memo.get(&n).copied().unwrap_or_default()
}

/// The table state of one file's walk: kfxgen's `leaf_cells`, `native_made`, `notes_tables`,
/// `tables_seen`, `notes_seen`, and what the table code works out once and reads again.
#[derive(Default)]
pub(super) struct Tables {
    /// Cells `_walk_cell` is walking: one holding no block is a paragraph of its own.
    pub(super) leaf_cells: HashSet<NodeId>,
    native_made: HashSet<NodeId>,
    notes: HashSet<NodeId>,
    pub(super) seen: Vec<BlockId>,
    pub(super) notes_seen: Vec<BlockId>,
    counts: HashMap<NodeId, Counts>,
    /// `_cell_valign`'s answer from each element up.
    valign: HashMap<NodeId, Option<String>>,
    /// A header cell's `inherit` alignment: the answer from each element up.
    inherited_align: HashMap<NodeId, String>,
    /// `_table_dir(cell).lower() == "rtl"` per table.
    rtl: HashMap<NodeId, bool>,
    /// Tables below a table already counted and searched for a switch.
    scanned: HashSet<NodeId>,
}

/// A row as `take` builds it, before `source_order`.
struct Row {
    group: &'static str,
    border: PyValue,
    bidi: PyValue,
    group_bidi: PyValue,
    group_border: PyValue,
    anchor_ids: Vec<String>,
    /// Each cell's dict, in its key order.
    cells: Vec<Vec<(PyValue, PyValue)>>,
    source_order: usize,
}

/// A row whose cells are being built.
struct PartialRow {
    tr: NodeId,
    depth: usize,
    row: Row,
    /// Its next child, as an index into its children.
    next: usize,
}

/// `take(container, group)` part-way through the container's children.
struct Take {
    container: NodeId,
    group: &'static str,
    follow: bool,
    /// `last`: the index in `rows` of the row just built in this container.
    last: Option<usize>,
    /// `run`: this row group's rows, as indexes into `rows`.
    run: Vec<usize>,
    next: usize,
    depth: usize,
}

/// One piece of `_fostered_content(table)`: loose text, or an element and its depth.
enum Fostered {
    Text(String),
    Elem(NodeId, usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Rows,
    Fostered,
    Captions,
}

/// The table block once its rows are built: what `_table_block` returns and the walk then
/// changes (its own and carried ids).
struct Built {
    /// `table["table"]["anchor_ids"]`.
    own: Vec<String>,
    anchor_ids: Vec<String>,
    anchor_offsets: PyValue,
    text: String,
    rows: PyValue,
    column_widths: PyValue,
    width: PyValue,
    bidi: PyValue,
    horizontal: bool,
}

/// A native table being built: `_table_block`'s state, then the table branch's.
pub(super) struct TableJob {
    table: NodeId,
    depth: usize,
    /// `listings`: how many contents listings were noted before the table.
    listings: usize,
    collapse: bool,
    table_border: PyValue,
    attr_cell_border: PyValue,
    rows: Vec<Row>,
    carry: Vec<String>,
    captions: Vec<(NodeId, usize)>,
    takes: Vec<Take>,
    row: Option<PartialRow>,
    stage: Stage,
    built: Option<Built>,
    fostered: Vec<Fostered>,
    next_fostered: usize,
    parts: Vec<Segment>,
    next_caption: usize,
    /// `_width_pct(cell)` of each cell built, which `_column_widths` reads again.
    widths: HashMap<NodeId, Option<f64>>,
}

/// `_walk_cell(cell)` waiting for its `_walk_element(cell)` to finish.
pub(super) struct CellEnd {
    job: Box<TableJob>,
    cell: NodeId,
    start: usize,
    listings: usize,
    aside: Aside,
}

/// What `_attr_border_px` reads: 0, or a number (infinite when too large for a float).
#[derive(Clone, Copy, Debug, PartialEq)]
enum BorderPx {
    Zero,
    Px(f64),
}

/// `_attr_border_px(table)`: the `border` attribute's leading digits as HTML reads them (an
/// empty value is 1), or 0 without one. `int()` raises past 4300 digits.
fn attr_border_px(dom: &Dom, table: NodeId) -> Result<BorderPx, Raised> {
    let Some(border) = dom.attr(table, "border") else {
        return Ok(BorderPx::Zero);
    };
    let digits: String = border
        .trim_start_matches(py::is_space)
        .chars()
        .map_while(|c| py::digit_value(c).map(|d| char::from(b'0' + d as u8)))
        .collect();
    if digits.is_empty() {
        return Ok(BorderPx::Px(1.0));
    }
    if digits.len() > 4300 {
        return Err(BORDER_DIGITS);
    }
    if digits.bytes().all(|b| b == b'0') {
        return Ok(BorderPx::Zero);
    }
    // Correctly rounded, as Python converts an int to a float; infinite past the largest.
    Ok(BorderPx::Px(digits.parse().unwrap_or(f64::INFINITY)))
}

/// `_span_attr(cell, name)`: colspan, rowspan or span as an int in 1..1000; anything `int()`
/// refuses counts as 1.
fn span_attr(dom: &Dom, n: NodeId, name: &str) -> usize {
    let v = dom.attr(n, name).filter(|v| !v.is_empty()).unwrap_or("1");
    match py::py_int(py::strip(v)) {
        // In 1..=1000, so the cast is exact.
        Some(n) => n.clamp(1, 1000) as usize,
        None => 1,
    }
}

/// `_width_pct(elem, style_resolver)` given the element's resolved style: its CSS width when
/// it declares one, else its `width` attribute, as a percentage, or None.
fn width_pct(dom: &Dom, elem: NodeId, css: Option<&Resolved>) -> Result<Option<f64>, ItemError> {
    let declared = match css {
        Some(css) => css.get("width")?,
        None => None,
    };
    let value: Cow<str> = match declared {
        Some(v) if !matches!(v, PyValue::None) => text_of(Some(v)),
        _ => Cow::Borrowed(dom.attr(elem, "width").unwrap_or("")),
    };
    // `re.fullmatch(r"\s*([\d.]+)\s*%\s*", value)`.
    let rest = value.trim_start_matches(py::is_space);
    let end = rest
        .find(|c: char| !(c == '.' || py::digit_value(c).is_some()))
        .unwrap_or(rest.len());
    if end == 0 {
        return Ok(None);
    }
    let (number, after) = rest.split_at(end);
    let Some(after) = after.trim_start_matches(py::is_space).strip_prefix('%') else {
        return Ok(None);
    };
    if !after.chars().all(py::is_space) {
        return Ok(None);
    }
    Ok(Some(py::py_float(number).ok_or(WIDTH_NOT_A_NUMBER)?))
}

/// `_cell_padding(css)`: a cell's padding per side, or None when it is the default on every
/// side. An unreadable side keeps the default.
fn cell_padding(css: Option<&Resolved>) -> Result<PyValue, ItemError> {
    let Some(css) = css else {
        return Ok(PyValue::None);
    };
    let sides = match css.get("padding-sides")? {
        Some(PyValue::Tuple(v) | PyValue::List(v)) if !v.is_empty() => v,
        _ => return Ok(PyValue::None),
    };
    let mut out = Vec::with_capacity(4);
    for (side, value) in BORDER_SIDES.iter().zip(sides) {
        let v = match padding_em(&text_of(Some(value)), 1.0)? {
            None => PyValue::Float(DEFAULT_PADDING_EM),
            Some(Pad::Em(em)) => PyValue::Float(em),
            Some(Pad::Pct(n)) => PyValue::Tuple(vec![s("%"), PyValue::Float(n)]),
        };
        out.push((s(side), v));
    }
    if out
        .iter()
        .all(|(_, v)| matches!(v, PyValue::Float(f) if (f - DEFAULT_PADDING_EM).abs() < 1e-9))
    {
        return Ok(PyValue::None);
    }
    Ok(PyValue::Dict(out))
}

/// `_only_block_child(cell)`: the one block element a cell holds and nothing beside it.
fn only_block_child(dom: &Dom, cell: NodeId) -> Option<NodeId> {
    let mut kids = dom.children(cell).iter().filter(|&&k| !dom.is_comment(k));
    let (Some(&kid), None) = (kids.next(), kids.next()) else {
        return None;
    };
    if !(matches!(dom.tag(kid), "p" | "div" | "blockquote") || is_heading(dom.tag(kid))) {
        return None;
    }
    if has_words(dom.text(cell)) || has_words(dom.tail(kid)) {
        return None;
    }
    Some(kid)
}

/// `_loose_cell_text(cell)`: text written straight in the cell beside its blocks.
fn loose_cell_text(dom: &Dom, cell: NodeId) -> bool {
    has_words(dom.text(cell))
        || dom
            .children(cell)
            .iter()
            .any(|&d| local(dom, d).is_some_and(is_cell_block) && has_words(dom.tail(d)))
}

/// `_is_contentless_anchor(elem)`: an `<a>` with no content.
fn is_contentless_anchor(dom: &Dom, n: NodeId) -> bool {
    local(dom, n) == Some("a") && !has_words(dom.text(n)) && dom.children(n).is_empty()
}

/// `_fostered_content(table)`: what sits in the table, its row groups and its rows outside the
/// cells, in document order. The parents (`_foster_parents`) are found from the table down,
/// not by testing every element below it against their list.
fn fostered_content(dom: &Dom, table: NodeId, depth: usize) -> Vec<Fostered> {
    let mut parents = vec![(table, depth)];
    for &c in dom.children(table) {
        match local(dom, c) {
            Some(tag) if row_group(tag).is_some() => {
                parents.push((c, depth + 1));
                for &r in dom.children(c) {
                    if local(dom, r) == Some("tr") {
                        parents.push((r, depth + 2));
                    }
                }
            }
            Some("tr") => parents.push((c, depth + 1)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (e, d) in parents {
        if has_words(dom.text(e)) {
            out.push(Fostered::Text(dom.text(e).to_owned()));
        }
        for &c in dom.children(e) {
            match local(dom, c) {
                None => {}
                Some(tag) if is_table_part(tag) => {}
                Some(_) if !is_contentless_anchor(dom, c) => out.push(Fostered::Elem(c, d + 1)),
                Some(_) => {}
            }
            if has_words(dom.tail(c)) {
                out.push(Fostered::Text(dom.tail(c).to_owned()));
            }
        }
    }
    out
}

/// `_cell_columns(table)`: each cell with the grid column it starts in and its colspan, row by
/// row, the table's own rows and those of its row groups (not of a group in a group). `each`
/// returns false to stop. The carried rowspans grow to the widest row: callers stop past 24
/// columns or only see native tables, which are no wider.
fn cell_columns(
    dom: &Dom,
    table: NodeId,
    mut each: impl FnMut(NodeId, usize, usize) -> Result<bool, ItemError>,
) -> Result<(), ItemError> {
    let groups = std::iter::once(table).chain(
        dom.children(table)
            .iter()
            .copied()
            .filter(|&c| local(dom, c).and_then(row_group).is_some()),
    );
    for group in groups {
        let mut carry: Vec<usize> = Vec::new();
        for &tr in dom.children(group) {
            if local(dom, tr) != Some("tr") {
                continue;
            }
            let cells = || {
                dom.children(tr)
                    .iter()
                    .copied()
                    .filter(|&c| local(dom, c).is_some_and(is_cell))
            };
            if cells().next().is_none() {
                // A row with no cells is never written, so it ends no rowspan.
                continue;
            }
            let mut col = 0;
            for cell in cells() {
                while carry.get(col).is_some_and(|&n| n > 0) {
                    col += 1;
                }
                let span = span_attr(dom, cell, "colspan");
                if !each(cell, col, span)? {
                    return Ok(());
                }
                let end = col + span;
                if carry.len() < end {
                    carry.resize(end, 0);
                }
                let rowspan = span_attr(dom, cell, "rowspan");
                carry[col..end].fill(rowspan);
                col = end;
            }
            for n in &mut carry {
                *n = n.saturating_sub(1);
            }
        }
    }
    Ok(())
}

/// `_table_width(table)`: the columns the widest row covers, counting stopped once past 24.
fn table_width(dom: &Dom, table: NodeId) -> Result<usize, ItemError> {
    let mut width = 0;
    cell_columns(dom, table, |_, col, span| {
        let end = col + span;
        if end > MAX_NATIVE_COLUMNS {
            width = end;
            return Ok(false);
        }
        width = width.max(end);
        Ok(true)
    })?;
    Ok(width)
}

/// `_NOTE_MARKER_RE.match(marker)`: a note marker alone (a number, a roman numeral or
/// symbols), optionally bracketed and followed by a full stop or colon. Python's
/// `IGNORECASE` folds `ı` and `İ` to `i`; `\d` is any decimal digit.
fn is_note_marker(marker: &str) -> bool {
    let chars: Vec<char> = marker.chars().collect();
    let rest = match chars.first() {
        Some('[' | '(') => &chars[1..],
        _ => &chars[..],
    };
    // After the marker: `[\])]?[.:]?$`.
    let tail_ok = |t: &[char]| match t {
        [] => true,
        [a] => matches!(a, ']' | ')' | '.' | ':'),
        [a, b] => matches!(a, ']' | ')') && matches!(b, '.' | ':'),
        _ => false,
    };
    let fold = |c: char| match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '\u{130}' | '\u{131}' => 'i',
        _ => c,
    };
    // A roman numeral is at most 16 letters (`mmmm` and three places of four), so only that
    // prefix is folded: the match stays linear in the marker's length.
    let folded: Vec<char> = rest.iter().take(17).map(|&c| fold(c)).collect();
    let romans = roman_ends(&folded);
    let n = rest.len();
    (n.saturating_sub(2)..=n).any(|l| {
        let (head, tail) = rest.split_at(l);
        if !tail_ok(tail) || head.is_empty() {
            return false;
        }
        let digits = head.len() <= 4 && head.iter().all(|&c| py::digit_value(c).is_some());
        let symbols = head
            .iter()
            .all(|c| matches!(c, '*' | '\u{2020}' | '\u{2021}' | '\u{a7}' | '\u{b6}' | '#'));
        digits || symbols || romans.contains(&l)
    })
}

/// The lengths of the prefixes of `f` (folded to lower case) that `(?=[mdclxvi])m{0,4}
/// (?:cm|cd|d?c{0,3})(?:xc|xl|l?x{0,3})(?:ix|iv|v?i{0,3})` matches whole.
fn roman_ends(f: &[char]) -> Vec<usize> {
    if !f.first().is_some_and(|c| "mdclxvi".contains(*c)) {
        return Vec::new();
    }
    let run = |at: usize, c: char, most: usize| {
        let mut ends = vec![at];
        let mut i = at;
        while i < f.len() && f[i] == c && ends.len() <= most {
            i += 1;
            ends.push(i);
        }
        ends
    };
    // One place value: `nine|four|five?one{0,3}` from `at`.
    let place = |at: usize, one: char, five: char, ten: char| {
        let mut ends = Vec::new();
        for pair in [[one, ten], [one, five]] {
            if f.get(at..at + 2) == Some(&pair[..]) {
                ends.push(at + 2);
            }
        }
        let starts = if f.get(at) == Some(&five) {
            vec![at, at + 1]
        } else {
            vec![at]
        };
        for s in starts {
            ends.extend(run(s, one, 3));
        }
        ends
    };
    let mut out = Vec::new();
    for a in run(0, 'm', 4) {
        for b in place(a, 'c', 'd', 'm') {
            for c in place(b, 'x', 'l', 'c') {
                out.extend(place(c, 'i', 'v', 'x'));
            }
        }
    }
    out
}

/// `_table_start_ids(table_block)`: the ids that name a native table's start.
fn table_start_ids<'r>(own: &'r [String], rows: &'r [Row]) -> HashSet<&'r str> {
    let mut start: HashSet<&str> = own.iter().map(String::as_str).collect();
    for row in rows {
        start.extend(row.anchor_ids.iter().map(String::as_str));
        if let Some(first) = row.cells.first() {
            start.extend(str_list(field(first, "anchor_ids")));
            if let Some(PyValue::List(paragraphs)) = field(first, "paragraphs")
                && let Some(p) = paragraphs.first()
            {
                start.extend(str_list(p.get("anchor_ids")));
            }
            break;
        }
    }
    start
}

/// `d.get(key)` on a dict's pairs.
fn field<'p>(pairs: &'p [(PyValue, PyValue)], key: &str) -> Option<&'p PyValue> {
    pairs
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

/// A list of strings' values.
fn str_list(v: Option<&PyValue>) -> impl Iterator<Item = &str> {
    v.and_then(PyValue::as_list)
        .unwrap_or(&[])
        .iter()
        .filter_map(PyValue::as_str)
}

/// `_cell_ids(cell)`: its own ids and its paragraphs'.
fn cell_ids(cell: &[(PyValue, PyValue)]) -> impl Iterator<Item = &str> {
    let paragraphs = match field(cell, "paragraphs") {
        Some(PyValue::List(p)) => p.as_slice(),
        _ => &[],
    };
    str_list(field(cell, "anchor_ids")).chain(
        paragraphs
            .iter()
            .flat_map(|p| str_list(p.get("anchor_ids"))),
    )
}

/// `cell["rowspan"] = min(cell["rowspan"], most)`.
fn clamp_rowspan(cell: &mut [(PyValue, PyValue)], most: usize) {
    if let Some((_, PyValue::Int(n))) = cell.iter_mut().find(|(k, _)| k.as_str() == Some("rowspan"))
    {
        *n = (*n).min(i64::try_from(most).unwrap_or(i64::MAX));
    }
}

/// `clamp_rowspans(group_rows)`: a rowspan ends at its row group's last row with cells.
fn clamp_rowspans(rows: &mut [Row], run: &[usize]) {
    let written: Vec<usize> = run
        .iter()
        .copied()
        .filter(|&i| !rows[i].cells.is_empty())
        .collect();
    let n = written.len();
    for (k, &i) in written.iter().enumerate() {
        for cell in &mut rows[i].cells {
            clamp_rowspan(cell, n - k);
        }
    }
}

/// `dict.fromkeys(ids, 0)`.
fn zeros(ids: &[String]) -> PyValue {
    PyValue::Dict(ids.iter().map(|i| (s(i), PyValue::Int(0))).collect())
}

/// `{**style, "align": align}` on a dict block style.
fn set_align(style: &mut PyValue, align: PyValue) {
    if let PyValue::Dict(pairs) = style {
        dict_set(pairs, "align", align);
    }
}

impl Walker<'_, '_, '_, '_> {
    fn counts_of(&mut self, n: NodeId) -> Counts {
        counts(self.dom, &mut self.tables.counts, n)
    }

    /// `str((style_resolver(elem) or {}).get(key) or "").strip().lower()`.
    fn raw_style(&self, n: NodeId, key: &str) -> Result<String, ItemError> {
        keyword(self.resolve(n)?.as_ref(), key)
    }

    /// `_box_background(css)` of `n`'s resolved style.
    fn fill(&self, n: NodeId, css: Option<&Resolved>) -> Result<PyValue, ItemError> {
        Ok(match css {
            Some(_) => self
                .resolver
                .box_background(n)?
                .map_err(ItemError::Raised)?
                .map_or(PyValue::None, PyValue::Int),
            None => PyValue::None,
        })
    }

    /// The table branch of `_walk_element` with `native_tables` set: false when kfxgen writes
    /// the table as rows instead (it is not native, or is a notes table). A contents entry
    /// naming the table past its start is the rows path too, refused once its rows are built.
    pub(super) fn table(&mut self, elem: NodeId, depth: usize) -> Result<bool, ItemError> {
        // Not in kfxgen: each cell below the table, nested tables' cells too, costs
        // `CELL_UNITS` of the file's style text budget, counted from the tree before anything
        // is resolved. A table converts in about four resolves a cell, so one too large for the
        // budget is refused after this one pass rather than after minutes of resolving. A
        // table inside one counted already was counted and searched with it, so nesting does
        // not repeat the pass.
        if self.tables.scanned.insert(elem) {
            let dom = self.dom;
            let (mut cells, mut switch) = (0usize, false);
            for d in dom.subtree(elem) {
                match local(dom, d) {
                    Some(t) if is_cell(t) => cells += 1,
                    Some("table") => {
                        self.tables.scanned.insert(d);
                    }
                    _ => {}
                }
                switch = switch || epub_switch_branch(dom, d).is_some();
            }
            self.resolver
                .charge(cells.saturating_mul(CELL_UNITS * crate::css::TEXT_UNIT))?;
            if switch {
                return Err(self.unsupported(SWITCH));
            }
        }
        if !self.table_is_native(elem)? {
            return Ok(false);
        }
        if self.is_notes_table(elem)? {
            // A notes section reads better as paragraphs, one per note (#268).
            self.tables.notes.insert(elem);
            return Ok(false);
        }
        let job = self.table_job(elem, depth)?;
        self.table_run(job)?;
        Ok(true)
    }

    /// `_table_is_native(table, style_resolver)` past `_shown_tree`, whose switch search is in
    /// [`Self::table`].
    fn table_is_native(&mut self, table: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        let (mut rows, mut cells, mut captions) = (0usize, 0usize, 0usize);
        let mut long_cell = false;
        for e in dom.subtree(table) {
            let Some(tag) = local(dom, e) else {
                continue;
            };
            if (is_row_group_tag(tag) || tag == "tr" || is_cell(tag))
                && e != table
                && (self.w.hidden(e)? || is_nav_listing(dom, e))
            {
                return Ok(false);
            }
            if tag == "caption" {
                // A hidden caption would be shown; only the first is kept.
                if self.w.hidden(e)? {
                    return Ok(false);
                }
                captions += 1;
                if captions > 1 {
                    return Ok(false);
                }
            }
            let parent = dom.parent(e).and_then(|p| local(dom, p)).unwrap_or("");
            if tag == "tr" {
                if !is_row_group_tag(parent) {
                    return Ok(false);
                }
                rows += 1;
            } else if (tag == "table" && e != table) || (is_non_text(tag) && tag != "img") {
                return Ok(false);
            } else if is_cell(tag) {
                if parent != "tr" {
                    return Ok(false);
                }
                cells += 1;
                long_cell = long_cell || self.counts_of(e).longest_run() > MAX_NATIVE_CELL_CHARS;
            }
        }
        let width = table_width(dom, table)?;
        if long_cell && width > 1 {
            return Ok(false);
        }
        Ok(rows > 0 && cells > 0 && width <= MAX_NATIVE_COLUMNS)
    }

    /// `_is_notes_table(table, base_href, link_targets)`, for a native table (so every row
    /// below it is its own: it holds no table).
    fn is_notes_table(&mut self, table: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        let links = self.opts.link_targets;
        let href = &self.w.file().href;
        if links.is_empty() || href.is_empty() {
            return Ok(false);
        }
        let doc = key_doc(href, "");
        let cells_of = |tr: NodeId| {
            dom.children(tr)
                .iter()
                .copied()
                .filter(|&c| local(dom, c).is_some_and(is_cell))
        };
        let rows: Vec<NodeId> = dom
            .subtree(table)
            .filter(|&tr| local(dom, tr) == Some("tr") && cells_of(tr).next().is_some())
            .collect();
        if rows.len() < 3 {
            return Ok(false);
        }
        for &row in &rows {
            let mut cells = cells_of(row);
            let (Some(first), Some(_), None) = (cells.next(), cells.next(), cells.next()) else {
                return Ok(false);
            };
            // The first cell's whole text: a row nested in a cell is read again in the row
            // round it, so each read is charged.
            let bytes = self.counts_of(first).bytes;
            self.resolver.charge(bytes)?;
            if !is_note_marker(&words(&dom.itertext(first))) {
                return Ok(false);
            }
        }
        // Whether a node or anything below it has an id the book links to, once per node.
        let is_linked = |id: &str| links.contains(&format!("{doc}#{id}"));
        let mut linked_below: HashMap<NodeId, bool> = HashMap::new();
        let mut stack = vec![(table, false)];
        while let Some((m, done)) = stack.pop() {
            if !done {
                stack.push((m, true));
                stack.extend(dom.children(m).iter().map(|&c| (c, false)));
                continue;
            }
            let own = !dom.is_comment(m) && own_anchor_ids(dom, m).any(is_linked);
            let below = own
                || dom
                    .children(m)
                    .iter()
                    .any(|c| linked_below.get(c).copied().unwrap_or(false));
            linked_below.insert(m, below);
        }
        let mut follows: HashMap<NodeId, bool> = HashMap::new();
        let mut linked = 0usize;
        for &row in &rows {
            let group = dom.parent(row).unwrap_or(row);
            let follow = *follows
                .entry(group)
                .or_insert_with(|| anchors_follow_rows(dom, group));
            // `_row_anchor_ids(row, follow)`: the row's ids, and the empty anchors beside it on
            // the side its group puts them.
            let mut hit = linked_below.get(&row).copied().unwrap_or(false);
            let step = |n: NodeId| {
                if follow {
                    dom.next_sibling(n)
                } else {
                    dom.prev_sibling(n)
                }
            };
            let mut sib = step(row);
            while let Some(a) = sib.filter(|&a| is_empty_anchor(dom, a)) {
                hit = hit || own_anchor_ids(dom, a).any(is_linked);
                sib = step(a);
            }
            linked += usize::from(hit);
        }
        // Python compares an int with a float exactly; both are far below 2^53 here.
        Ok(linked as f64 >= NOTES_TABLE_LINKED_SHARE * rows.len() as f64)
    }

    /// The start of `_table_block(table, ...)`: the table's frame, and `take(table, "body")`
    /// about to begin.
    fn table_job(&mut self, table: NodeId, depth: usize) -> Result<Box<TableJob>, ItemError> {
        let dom = self.dom;
        let css = self.resolve(table)?;
        let collapse = match &css {
            Some(css) => text_of(css.get("border-collapse")?) == "collapse",
            None => false,
        };
        // Its CSS border, else the `border` attribute's outset grey frame (#264).
        let table_border = match css_border(css.as_ref())? {
            Some(b) => border_py(&Some(b)),
            None => match attr_border_px(dom, table)? {
                BorderPx::Zero => PyValue::None,
                BorderPx::Px(px) => {
                    if px.is_infinite() {
                        return Err(BORDER_TOO_LARGE.into());
                    }
                    let side = PyValue::Tuple(vec![
                        s("outset"),
                        PyValue::Float(py::round(0.45 * px, 4)),
                        PyValue::Int(0xFF80_8080),
                    ]);
                    PyValue::Dict(BORDER_SIDES.iter().map(|k| (s(k), side.clone())).collect())
                }
            },
        };
        // The attribute's inset rule round every cell (#264).
        let attr_cell_border = match attr_border_px(dom, table)? {
            BorderPx::Zero => PyValue::None,
            BorderPx::Px(_) => {
                let side = PyValue::Tuple(vec![s("inset"), PyValue::Float(0.45), PyValue::None]);
                PyValue::Dict(BORDER_SIDES.iter().map(|k| (s(k), side.clone())).collect())
            }
        };
        Ok(Box::new(TableJob {
            table,
            depth,
            listings: self.nav_listing_at.len(),
            collapse,
            table_border,
            attr_cell_border,
            rows: Vec::new(),
            carry: Vec::new(),
            captions: Vec::new(),
            takes: vec![Take {
                container: table,
                group: "body",
                follow: anchors_follow_rows(dom, table),
                last: None,
                run: Vec::new(),
                next: 0,
                depth,
            }],
            row: None,
            stage: Stage::Rows,
            built: None,
            fostered: Vec::new(),
            next_fostered: 0,
            parts: Vec::new(),
            next_caption: 0,
            widths: HashMap::new(),
        }))
    }

    /// Builds the table until a walk is needed (a cell of blocks, a fostered block, a caption,
    /// each a frame), or the table block is written.
    pub(super) fn table_run(&mut self, mut job: Box<TableJob>) -> Result<(), ItemError> {
        loop {
            match job.stage {
                Stage::Rows => {
                    if let Some((cell, depth)) = self.rows_step(&mut job)? {
                        return self.walk_cell(job, cell, depth);
                    }
                    if !self.rows_done(&mut job)? {
                        // The contents fallback: written as rows instead.
                        return self.walk_rest(job.table, job.depth);
                    }
                }
                Stage::Fostered => {
                    if let Some((el, depth)) = self.fostered_step(&mut job)? {
                        self.stack.push(Frame::Table(job));
                        return self.enter(el, depth);
                    }
                    if !job.captions.is_empty()
                        && let Some(built) = &mut job.built
                    {
                        // The table's own ids name the caption's first paragraph; they come
                        // back to the table if the caption holds no text.
                        let own = std::mem::take(&mut built.own);
                        let gone: HashSet<&String> = own.iter().collect();
                        built.anchor_ids.retain(|a| !gone.contains(a));
                        self.pending_ids.extend(own);
                    }
                    job.stage = Stage::Captions;
                }
                Stage::Captions => {
                    if let Some(&(caption, depth)) = job.captions.get(job.next_caption) {
                        job.next_caption += 1;
                        self.stack.push(Frame::Table(job));
                        return self.enter(caption, depth);
                    }
                    return self.table_done(*job);
                }
            }
        }
    }

    /// `take`'s loop from where it stopped: the next cell `_walk_cell` must walk, or None when
    /// every row is built.
    fn rows_step(&mut self, job: &mut TableJob) -> Result<Option<(NodeId, usize)>, ItemError> {
        let dom = self.dom;
        loop {
            // The cells of the row being built.
            let next = match &mut job.row {
                Some(row) => {
                    let at = dom.children(row.tr).get(row.next).copied();
                    row.next += 1;
                    Some((at, row.depth + 1))
                }
                None => None,
            };
            match next {
                Some((Some(c), depth)) => {
                    if !local(dom, c).is_some_and(is_cell) {
                        continue;
                    }
                    if depth > MAX_DEPTH {
                        return Err(self.too_deep());
                    }
                    if self.cell_needs_walk(c)? {
                        return Ok(Some((c, depth)));
                    }
                    let css = self.resolve(c)?;
                    let built = self.text_cell(c, job.table, css.as_ref())?;
                    self.finish_cell(job, c, css.as_ref(), built)?;
                    continue;
                }
                Some((None, _)) => {
                    if let Some(PartialRow { row, .. }) = job.row.take() {
                        let i = job.rows.len();
                        job.rows.push(row);
                        if let Some(take) = job.takes.last_mut() {
                            take.run.push(i);
                            take.last = Some(i);
                        }
                    }
                    continue;
                }
                None => {}
            }
            let Some(take) = job.takes.last_mut() else {
                return Ok(None);
            };
            let Some(&child) = dom.children(take.container).get(take.next) else {
                let run = std::mem::take(&mut take.run);
                job.takes.pop();
                clamp_rowspans(&mut job.rows, &run);
                continue;
            };
            take.next += 1;
            let (container, group, follow, depth) =
                (take.container, take.group, take.follow, take.depth);
            let Some(tag) = local(dom, child) else {
                continue;
            };
            if let Some(inner) = row_group(tag) {
                let run = std::mem::take(&mut take.run);
                take.last = None;
                clamp_rowspans(&mut job.rows, &run);
                job.carry
                    .extend(own_anchor_ids(dom, child).map(str::to_owned));
                if depth + 1 > MAX_DEPTH {
                    return Err(self.too_deep());
                }
                job.takes.push(Take {
                    container: child,
                    group: inner,
                    follow: anchors_follow_rows(dom, child),
                    last: None,
                    run: Vec::new(),
                    next: 0,
                    depth: depth + 1,
                });
            } else if tag == "caption" {
                job.captions.push((child, depth + 1));
            } else if tag == "tr" {
                let border = self.part_border(job.collapse, child)?;
                // A row's or row group's own direction and language (P605 T04, T05).
                let bidi = self.own_bidi(child)?;
                let grouped = row_group(dom.tag(container)).is_some();
                let group_bidi = if grouped {
                    self.own_bidi(container)?
                } else {
                    PyValue::None
                };
                let group_border = if grouped {
                    self.part_border(job.collapse, container)?
                } else {
                    PyValue::None
                };
                let mut ids = std::mem::take(&mut job.carry);
                ids.extend(own_anchor_ids(dom, child).map(str::to_owned));
                for &c in dom.children(child) {
                    if is_empty_anchor(dom, c) {
                        ids.extend(own_anchor_ids(dom, c).map(str::to_owned));
                    }
                }
                job.row = Some(PartialRow {
                    tr: child,
                    depth: depth + 1,
                    row: Row {
                        group,
                        border,
                        bidi,
                        group_bidi,
                        group_border,
                        anchor_ids: ids,
                        cells: Vec::new(),
                        source_order: 0,
                    },
                    next: 0,
                });
            } else if is_empty_anchor(dom, child) {
                let ids = own_anchor_ids(dom, child).map(str::to_owned);
                match take.last {
                    Some(i) if follow => job.rows[i].anchor_ids.extend(ids),
                    _ => job.carry.extend(ids),
                }
            }
        }
    }

    /// `part_border(elem)`: a row's or row group's border, drawn only when the table's
    /// borders collapse.
    fn part_border(&self, collapse: bool, elem: NodeId) -> Result<PyValue, ItemError> {
        if !collapse {
            return Ok(PyValue::None);
        }
        Ok(border_py(&css_border(self.resolve(elem)?.as_ref())?))
    }

    /// `own_bidi(elem)`: its direction and language when it sets one itself, else None.
    fn own_bidi(&mut self, elem: NodeId) -> Result<PyValue, ItemError> {
        let bidi = self.bidi.bidi(self.dom, self.resolver, elem)?;
        Ok(
            if bidi.iter().any(|(k, _)| k.as_str() == Some("set_here")) {
                PyValue::Dict(bidi)
            } else {
                PyValue::None
            },
        )
    }

    /// `build_cell`'s test for walking the cell as the body is: several blocks, a picture, a
    /// bordered or filled block, loose text beside a block, or a block with margins.
    fn cell_needs_walk(&mut self, cell: NodeId) -> Result<bool, ItemError> {
        let c = self.counts_of(cell);
        Ok(c.blocks >= 2
            || c.img
            || self.holds_box(cell)?
            || (c.blocks > 0 && loose_cell_text(self.dom, cell))
            || self.spaced_block(cell)?)
    }

    /// `holds_box(cell)`: a bordered or filled block in the cell.
    fn holds_box(&self, cell: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        for d in dom.subtree(cell).skip(1) {
            if !local(dom, d).is_some_and(is_box_tag) {
                continue;
            }
            let css = self.resolve(d)?;
            if css_border(css.as_ref())?.is_some()
                || !matches!(self.fill(d, css.as_ref())?, PyValue::None)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `spaced_block(cell)`: a block in the cell with a margin above or below.
    fn spaced_block(&self, cell: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        for d in dom.subtree(cell).skip(1) {
            if !local(dom, d).is_some_and(is_cell_block) {
                continue;
            }
            let css = self.resolve(d)?;
            if truthy(margin_em(css.as_ref(), "top", 1.0)?)
                || truthy(margin_em(css.as_ref(), "bottom", 1.0)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `_walk_cell(cell)` up to its `_walk_element(cell)`; the rest is its [`Frame::CellEnd`].
    fn walk_cell(
        &mut self,
        job: Box<TableJob>,
        cell: NodeId,
        depth: usize,
    ) -> Result<(), ItemError> {
        let aside = self.set_aside();
        let start = self.blocks.len();
        let listings = self.nav_listing_at.len();
        // A cell of words and pictures is one paragraph, as a <div> is (#565).
        self.tables.leaf_cells.insert(cell);
        self.lists.cell_depth += 1;
        self.stack.push(Frame::CellEnd(Box::new(CellEnd {
            job,
            cell,
            start,
            listings,
            aside,
        })));
        self.walk_element(cell, depth)
    }

    /// The end of `_walk_cell(cell)`, then `_paragraph_cell` (or `_table_cell` when the walk
    /// left no text) and the rest of `build_cell`, and the table goes on.
    pub(super) fn cell_end(&mut self, end: CellEnd) -> Result<(), ItemError> {
        let CellEnd {
            mut job,
            cell,
            start,
            listings,
            aside,
        } = end;
        self.lists.cell_depth = self.lists.cell_depth.saturating_sub(1);
        self.tables.leaf_cells.remove(&cell);
        let out = self.blocks.split_off(start.min(self.blocks.len()));
        // Spaced by their margins, as outside a table (#645, #655, #675).
        let mut out = space_between(out, &mut self.noted.margins, true, &mut self.noted.paddings);
        // A contents listing in the cell belongs to the table, where the cell's blocks began.
        if let Some(noted) = self.nav_listing_at.get_mut(listings..) {
            for (at, _) in noted {
                *at = start;
            }
        }
        if !self.pending_ids.is_empty()
            && let Some(last) = out.last_mut()
        {
            add_ids(last, &self.pending_ids);
        }
        self.put_back(aside);
        let paragraphs = self.cell_paragraphs(cell, job.table, out)?;
        // The cell's style, resolved once for the rest of `build_cell` (resolving reads
        // nothing it keeps).
        let css = self.resolve(cell)?;
        let built = match paragraphs {
            Some(paragraphs) => self.paragraph_cell(cell, job.table, css.as_ref(), paragraphs)?,
            None => self.text_cell(cell, job.table, css.as_ref())?,
        };
        self.finish_cell(&mut job, cell, css.as_ref(), built)?;
        self.table_run(job)
    }

    /// The start of `_paragraph_cell`: the cell's blocks that hold text, each made the
    /// cell's (`in_cell`); None when none holds text.
    fn cell_paragraphs(
        &mut self,
        cell: NodeId,
        table: NodeId,
        out: Vec<Block>,
    ) -> Result<Option<Vec<Block>>, ItemError> {
        let mut paragraphs = Vec::new();
        for mut p in out {
            if !text_field(&p).is_empty() {
                self.in_cell(cell, table, &mut p)?;
                paragraphs.push(p);
            }
        }
        Ok((!paragraphs.is_empty()).then_some(paragraphs))
    }

    /// The rest of `_paragraph_cell(cell, style_resolver, walk_cell)`: a cell holding several
    /// blocks, as a container of paragraphs (#261).
    fn paragraph_cell(
        &mut self,
        cell: NodeId,
        table: NodeId,
        css: Option<&Resolved>,
        paragraphs: Vec<Block>,
    ) -> Result<Vec<(PyValue, PyValue)>, ItemError> {
        let text = paragraphs
            .iter()
            .map(text_field)
            .collect::<Vec<_>>()
            .join(" ");
        let style = self.cell_style(cell, table, css)?;
        let dom = self.dom;
        let header = dom.tag(cell) == "th";
        Ok(vec![
            (s("text"), PyValue::Str(text)),
            (s("spans"), PyValue::List(Vec::new())),
            (s("anchor_ids"), PyValue::List(Vec::new())),
            (s("anchor_offsets"), PyValue::Dict(Vec::new())),
            (s("block_style"), style),
            (s("header"), PyValue::Bool(header)),
            (s("not_bold"), PyValue::Bool(header_not_bold(header, css)?)),
            (s("colspan"), span_py(dom, cell, "colspan")),
            (s("rowspan"), span_py(dom, cell, "rowspan")),
            (s("valign"), self.cell_valign(cell, css)?),
            (s("background"), self.fill(cell, css)?),
            (
                s("paragraphs"),
                PyValue::List(paragraphs.iter().map(Block::to_py).collect()),
            ),
        ])
    }

    /// `in_cell(p)`: a cell paragraph's block style made the cell's (`_cell_block_style`); a
    /// box's paragraphs each so, however deep, the box's own left as it is.
    fn in_cell(&mut self, cell: NodeId, table: NodeId, p: &mut Block) -> Result<(), ItemError> {
        let mut stack: Vec<&mut Block> = vec![p];
        while let Some(b) = stack.pop() {
            if b.kind() == Some("box") {
                if let Some(kids) = b.blocks_mut() {
                    stack.extend(kids.iter_mut().rev());
                }
                continue;
            }
            let style = b.get("block_style").cloned().unwrap_or(PyValue::None);
            let style = self.cell_block_style(cell, table, None, style)?;
            b.set("block_style", style);
        }
        Ok(())
    }

    /// `_table_cell(cell, style_resolver, base_href)`: a cell as one text entry.
    fn text_cell(
        &mut self,
        cell: NodeId,
        table: NodeId,
        css: Option<&Resolved>,
    ) -> Result<Vec<(PyValue, PyValue)>, ItemError> {
        let dom = self.dom;
        let segments = self.w.inline(cell)?;
        let (text, spans, marks) = normalize_runs_with_anchors(&segments);
        drop(segments);
        let mut ids: Vec<String> = own_anchor_ids(dom, cell).map(str::to_owned).collect();
        ids.extend(marks.iter().map(|(id, _)| id.clone()));
        let ids = dedupe(ids);
        let style = self.cell_style(cell, table, css)?;
        let header = dom.tag(cell) == "th";
        let not_bold = header_not_bold(header, css)?;
        let heading = self.counts_of(cell).heading;
        Ok(vec![
            (s("text"), PyValue::Str(text)),
            (
                s("spans"),
                PyValue::List(spans.iter().map(|s| s.to_py()).collect()),
            ),
            (s("anchor_ids"), strs(&ids)),
            (s("anchor_offsets"), offsets(&ids, &marks)),
            (s("block_style"), style),
            (s("header"), PyValue::Bool(header)),
            (s("not_bold"), PyValue::Bool(not_bold)),
            // A heading written in a cell: the section's opening (#492).
            (s("heading"), PyValue::Bool(heading)),
            (s("colspan"), span_py(dom, cell, "colspan")),
            (s("rowspan"), span_py(dom, cell, "rowspan")),
            (s("valign"), self.cell_valign(cell, css)?),
            // Its fill, as Kindle Previewer writes it on the cell (#502).
            (s("background"), self.fill(cell, css)?),
        ])
    }

    /// The end of `build_cell`: the cell's border (its own CSS one, else the attribute's),
    /// padding and whether it sets its width, and the cell goes on its row.
    fn finish_cell(
        &mut self,
        job: &mut TableJob,
        cell: NodeId,
        css: Option<&Resolved>,
        mut built: Vec<(PyValue, PyValue)>,
    ) -> Result<(), ItemError> {
        let border = match css_border(css)? {
            Some(b) => border_py(&Some(b)),
            None => job.attr_cell_border.clone(),
        };
        built.push((s("border"), border));
        built.push((s("padding"), cell_padding(css)?));
        // A cell giving itself a width takes Previewer's border-box sizing (#264).
        let width = width_pct(self.dom, cell, css)?;
        job.widths.insert(cell, width);
        built.push((s("width_set"), PyValue::Bool(width.is_some())));
        if let Some(row) = &mut job.row {
            row.row.cells.push(built);
        }
        Ok(())
    }

    /// `_cell_start_end(_with_bidi(_cell_block_style(cell, css, ...), cell, ...), cell, css)`.
    fn cell_style(
        &mut self,
        cell: NodeId,
        table: NodeId,
        css: Option<&Resolved>,
    ) -> Result<PyValue, ItemError> {
        let style = self.cell_block_style(cell, table, css, PyValue::None)?;
        let style = self
            .bidi
            .with_bidi(self.dom, self.resolver, style, cell, true)?;
        if self.dom.tag(cell) == "th" {
            // A header cell's start stays centred (#616).
            return Ok(style);
        }
        Ok(start_end_align(style, &keyword(css, "text-align")?))
    }

    /// `_cell_block_style(cell, css, block_style, style_resolver)`: the cell's (or a cell
    /// paragraph's) block style with the alignment Previewer gives it.
    fn cell_block_style(
        &mut self,
        cell: NodeId,
        table: NodeId,
        css: Option<&Resolved>,
        block_style: PyValue,
    ) -> Result<PyValue, ItemError> {
        let dom = self.dom;
        let mut style = block_style;
        if matches!(style, PyValue::None)
            && let Some(css) = css
        {
            style = compute_block_style(css)?;
        }
        if dom.tag(cell) == "th" {
            if !style.is_truthy() {
                style = compute_block_style(&self.resolver.no_style())?;
            }
            let mine = self.raw_style(cell, "text-align")?;
            let own = self.raw_style(cell, "own-text-align")?;
            // `block_style["align"]`.
            let current = match &style {
                PyValue::Dict(pairs) => field(pairs, "align").cloned(),
                _ => None,
            };
            let Some(current) = current else {
                return Err(NO_ALIGN.into());
            };
            let own_paragraph = current != align_keyword(&mine).map_or(PyValue::None, s);
            if !own_paragraph {
                // Keywords as Previewer reads them on a header cell (#616, #621).
                let own = if own == "inherit" {
                    self.inherited_align(cell)?
                } else {
                    own
                };
                let rtl = self.table_rtl(table);
                let own = match own.as_str() {
                    "initial" if rtl => "right",
                    "initial" => "left",
                    "start" => "center",
                    "end" if rtl => "left",
                    other => other,
                };
                set_align(&mut style, s(align_keyword(own).unwrap_or("center")));
            }
            // `<th><p style="text-align: right">`: the paragraph's own (#532).
            if let Some(inner) = only_block_child(dom, cell) {
                let raw = self.raw_style(inner, "text-align")?;
                if raw != mine
                    && let Some(own) = align_keyword(&raw)
                {
                    set_align(&mut style, s(own));
                }
            }
            return Ok(style);
        }
        if let PyValue::Dict(pairs) = &style
            && field(pairs, "align").is_some_and(PyValue::is_truthy)
        {
            return Ok(style);
        }
        if let Some(inner) = only_block_child(dom, cell) {
            // `<td><p style="text-align: right">`: the paragraph's own (QA on #532).
            let inner_style = match self.resolve(inner)? {
                Some(css) => compute_block_style(&css)?,
                None => compute_block_style(&self.resolver.no_style())?,
            };
            let own = inner_style.get("align").cloned().unwrap_or(PyValue::None);
            if own.is_truthy() {
                let mut base = if style.is_truthy() {
                    style
                } else {
                    compute_block_style(&self.resolver.no_style())?
                };
                set_align(&mut base, own);
                return Ok(base);
            }
        }
        Ok(style)
    }

    /// A header cell's `text-align: inherit`: the nearest alignment set in CSS above it, not by
    /// an `align` attribute, else left (#621). Kept per element, so cells do not walk up
    /// their ancestors each.
    fn inherited_align(&mut self, cell: NodeId) -> Result<String, ItemError> {
        let dom = self.dom;
        let mut chain = Vec::new();
        let mut at = dom.parent(cell);
        let found = loop {
            let Some(a) = at else {
                break "left".to_owned();
            };
            if let Some(known) = self.tables.inherited_align.get(&a) {
                break known.clone();
            }
            chain.push(a);
            let a_own = self.raw_style(a, "own-text-align")?;
            let attr = dom
                .attr(a, SOURCE_ALIGN)
                .filter(|v| !v.is_empty())
                .or_else(|| dom.attr(a, "align").filter(|v| !v.is_empty()))
                .unwrap_or("");
            if !a_own.is_empty() && a_own != "inherit" && a_own != py::strip(attr).to_lowercase() {
                break a_own;
            }
            at = dom.parent(a);
        };
        for a in chain {
            self.tables.inherited_align.insert(a, found.clone());
        }
        Ok(found)
    }

    /// `_table_dir(cell).lower() == "rtl"`: the `dir` of the cell's table or of what holds it
    /// (a cell's or row's own does not count, #570). A cell's nearest table is its own: a
    /// native table holds no other.
    fn table_rtl(&mut self, table: NodeId) -> bool {
        let dom = self.dom;
        *self.tables.rtl.entry(table).or_insert_with(|| {
            std::iter::once(table)
                .chain(dom.ancestors(table))
                .find_map(|a| dom.attr(a, "dir").filter(|d| !d.is_empty()))
                .unwrap_or("")
                .to_lowercase()
                == "rtl"
        })
    }

    /// `_cell_valign(cell, style_resolver)`: the cell's vertical-align, else its row's, else
    /// its row group's (#269). Kept per element.
    fn cell_valign(&mut self, cell: NodeId, css: Option<&Resolved>) -> Result<PyValue, ItemError> {
        let dom = self.dom;
        let mut chain = Vec::new();
        let mut at = Some(cell);
        let found = loop {
            let Some(e) = at.filter(|&e| {
                local(dom, e)
                    .is_some_and(|t| is_cell(t) || matches!(t, "tr" | "thead" | "tbody" | "tfoot"))
            }) else {
                break None;
            };
            if let Some(known) = self.tables.valign.get(&e) {
                break known.clone();
            }
            chain.push(e);
            let declared = if e == cell {
                keyword(css, "vertical-align")?
            } else {
                self.raw_style(e, "vertical-align")?
            };
            let attr = py::strip(dom.attr(e, "valign").unwrap_or("")).to_lowercase();
            let attr = if matches!(attr.as_str(), "top" | "middle" | "bottom" | "baseline") {
                attr
            } else {
                String::new()
            };
            // calibre's UA sheet gives row groups and `table > tr` `middle`: there the
            // attribute wins.
            let ua_middle = declared == "middle" && !is_cell(dom.tag(e));
            if !declared.is_empty() && declared != "inherit" && !(ua_middle && !attr.is_empty()) {
                break Some(declared);
            }
            if !attr.is_empty() {
                break Some(attr);
            }
            at = dom.parent(e);
        };
        for e in chain {
            self.tables.valign.insert(e, found.clone());
        }
        Ok(found.map_or(PyValue::None, PyValue::Str))
    }

    /// `_column_widths(table, style_resolver)`: ("col", [...]) from `<col>`s, else ("cell",
    /// [...]) from single-column cells, else None. Each width a `span` repeats is charged.
    fn column_widths(
        &mut self,
        table: NodeId,
        built: &HashMap<NodeId, Option<f64>>,
    ) -> Result<PyValue, ItemError> {
        let dom = self.dom;
        let mut cols: Vec<PyValue> = Vec::new();
        for &child in dom.children(table) {
            let members: Vec<NodeId> = match local(dom, child) {
                Some("col") => vec![child],
                Some("colgroup") => dom
                    .children(child)
                    .iter()
                    .copied()
                    .filter(|&c| local(dom, c) == Some("col"))
                    .collect(),
                _ => Vec::new(),
            };
            for col in members {
                let css = self.resolve(col)?;
                let w = width_pct(dom, col, css.as_ref())?.map_or(PyValue::None, PyValue::Float);
                let span = span_attr(dom, col, "span");
                self.resolver
                    .charge(span.saturating_mul(size_of::<PyValue>()))?;
                cols.extend(std::iter::repeat_n(w, span));
            }
        }
        if cols.iter().any(|w| !matches!(w, PyValue::None)) {
            return Ok(PyValue::Tuple(vec![s("col"), PyValue::List(cols)]));
        }
        let mut widths: Vec<PyValue> = Vec::new();
        cell_columns(dom, table, |cell, col, span| {
            // A native table is at most 24 columns wide.
            if widths.len() < col + span {
                widths.resize(col + span, PyValue::None);
            }
            if span == 1 {
                // `take` built every cell `_cell_columns` yields and read its width.
                if let Some(pct) = built.get(&cell).copied().flatten() {
                    widths[col] = PyValue::Float(pct);
                }
            }
            Ok(true)
        })?;
        if widths.iter().any(|w| !matches!(w, PyValue::None)) {
            return Ok(PyValue::Tuple(vec![s("cell"), PyValue::List(widths)]));
        }
        Ok(PyValue::None)
    }

    /// The end of `_table_block` and the contents fallback: rows ordered head, body, foot;
    /// the table's ids, text, widths and direction. False when a contents entry names an id
    /// past the table's start, which writes the table as rows (#219).
    fn rows_done(&mut self, job: &mut TableJob) -> Result<bool, ItemError> {
        let dom = self.dom;
        let table = job.table;
        for (i, row) in job.rows.iter_mut().enumerate() {
            row.source_order = i;
        }
        // Stable: rows of one kind keep their order.
        job.rows.sort_by_key(|r| group_order(r.group));
        let own: Vec<String> = own_anchor_ids(dom, table).map(str::to_owned).collect();
        let mut every = own.clone();
        for row in &job.rows {
            every.extend(row.anchor_ids.iter().cloned());
        }
        for row in &job.rows {
            for cell in &row.cells {
                every.extend(cell_ids(cell).map(str::to_owned));
            }
        }
        let every = dedupe(every);
        let text = job
            .rows
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .filter_map(|c| field(c, "text").and_then(PyValue::as_str))
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let widths = std::mem::take(&mut job.widths);
        let column_widths = self.column_widths(table, &widths)?;
        let css = self.resolve(table)?;
        let width = width_pct(dom, table, css.as_ref())?.map_or(PyValue::None, PyValue::Float);
        drop(css);
        let bidi = PyValue::Dict(self.bidi.bidi(dom, self.resolver, table)?);
        let horizontal = dom.attr(table, HORIZONTAL).is_some();
        let toc = self.opts.toc_targets;
        if !toc.is_empty() {
            let start = table_start_ids(&own, &job.rows);
            if every
                .iter()
                .any(|a| !start.contains(a.as_str()) && toc.contains(a))
            {
                // A chapter cannot start inside one block: the rows path keeps 5.8.8's
                // chapters, and records its own listings (#282).
                self.nav_listing_at.truncate(job.listings);
                return Ok(false);
            }
        }
        let rows = std::mem::take(&mut job.rows)
            .into_iter()
            .map(|r| {
                PyValue::Dict(vec![
                    (s("group"), s(r.group)),
                    (s("border"), r.border),
                    (s("bidi"), r.bidi),
                    (s("group_bidi"), r.group_bidi),
                    (s("group_border"), r.group_border),
                    (s("anchor_ids"), strs(&r.anchor_ids)),
                    (
                        s("cells"),
                        PyValue::List(r.cells.into_iter().map(PyValue::Dict).collect()),
                    ),
                    (
                        s("source_order"),
                        PyValue::Int(i64::try_from(r.source_order).unwrap_or(i64::MAX)),
                    ),
                ])
            })
            .collect();
        job.built = Some(Built {
            own,
            anchor_offsets: zeros(&every),
            anchor_ids: every,
            text,
            rows: PyValue::List(rows),
            column_widths,
            width,
            bidi,
            horizontal,
        });
        job.fostered = fostered_content(dom, table, job.depth);
        job.stage = Stage::Fostered;
        Ok(true)
    }

    /// `_emit_fostered(table)` from where it stopped: the next block element to walk, or None
    /// when all is written.
    fn fostered_step(&mut self, job: &mut TableJob) -> Result<Option<(NodeId, usize)>, ItemError> {
        let dom = self.dom;
        while let Some(item) = job.fostered.get_mut(job.next_fostered) {
            job.next_fostered += 1;
            match item {
                Fostered::Text(text) => job.parts.push(Segment {
                    text: std::mem::take(text),
                    flags: Flags::default(),
                }),
                &mut Fostered::Elem(el, depth) => {
                    if dom.tag(el) == "img" {
                        // A picture runs with the words, as Previewer writes it.
                        if self.w.hidden(el)? {
                            job.parts.extend(
                                subtree_anchor_ids(dom, el)
                                    .iter()
                                    .map(|a| make_anchor_mark(a)),
                            );
                            continue;
                        }
                        job.parts
                            .extend(own_anchor_ids(dom, el).map(make_anchor_mark));
                        let token = self.w.picture_token_unfloated(el)?;
                        job.parts.push(Segment {
                            text: token,
                            flags: Flags::default(),
                        });
                    } else if is_block_tag(dom, el) {
                        self.fostered_flush(job)?;
                        return Ok(Some((el, depth)));
                    } else {
                        let segments = self.w.inline_child(el, &Flags::default())?;
                        job.parts.extend(segments);
                    }
                }
            }
        }
        self.fostered_flush(job)?;
        Ok(None)
    }

    /// `_emit_fostered`'s `flush()`: the loose text and inline elements so far, one paragraph
    /// in the language and direction round the table (#718).
    fn fostered_flush(&mut self, job: &mut TableJob) -> Result<(), ItemError> {
        if job.parts.is_empty() {
            return Ok(());
        }
        let parts = std::mem::take(&mut job.parts);
        let (text, spans, marks) = normalize_runs_with_anchors(&parts);
        drop(parts);
        if !has_words(&text) {
            self.pending_ids.extend(marks.into_iter().map(|(id, _)| id));
            return Ok(());
        }
        let mut ids = self.take_pending();
        ids.extend(marks.iter().map(|(id, _)| id.clone()));
        let ids = dedupe(ids);
        let around = self.dom.parent(job.table).unwrap_or(job.table);
        let style = self
            .bidi
            .with_bidi(self.dom, self.resolver, PyValue::None, around, true)?;
        let block = self.new_block(vec![
            ("text", Field::Py(PyValue::Str(text))),
            (
                "spans",
                Field::Py(PyValue::List(spans.iter().map(|s| s.to_py()).collect())),
            ),
            ("block_style", Field::Py(style)),
            ("anchor_ids", Field::Py(strs(&ids))),
            ("anchor_offsets", Field::Py(offsets(&ids, &marks))),
        ]);
        self.blocks.push(block);
        Ok(())
    }

    /// The end of the table branch: the ids carried from before the table name its start, the
    /// table block is written, and the ids no row took carry past it.
    fn table_done(&mut self, job: TableJob) -> Result<(), ItemError> {
        let Some(mut built) = job.built else {
            return Ok(());
        };
        if !self.pending_ids.is_empty() {
            let pending = self.take_pending();
            let mut own = pending.clone();
            own.append(&mut built.own);
            built.own = dedupe(own);
            let mut ids = pending;
            ids.append(&mut built.anchor_ids);
            built.anchor_ids = dedupe(ids);
            built.anchor_offsets = zeros(&built.anchor_ids);
        }
        let mut tbl = vec![
            (s("anchor_ids"), strs(&built.own)),
            (s("rows"), built.rows),
            (s("border"), job.table_border),
            (s("column_widths"), built.column_widths),
            (s("width"), built.width),
            (s("bidi"), built.bidi),
        ];
        if built.horizontal {
            // A horizontal-tb table in vertical-rl text (#247).
            tbl.push((s("horizontal"), PyValue::Bool(true)));
        }
        let block = self.new_block(vec![
            ("type", Field::Py(s("table"))),
            ("text", Field::Py(PyValue::Str(built.text))),
            ("spans", Field::Py(PyValue::List(Vec::new()))),
            ("block_style", Field::Py(PyValue::None)),
            ("anchor_ids", Field::Py(strs(&built.anchor_ids))),
            ("anchor_offsets", Field::Py(built.anchor_offsets)),
            ("table", Field::Py(PyValue::Dict(tbl))),
        ]);
        self.blocks.push(block);
        self.tables.native_made.insert(job.table);
        self.pending_ids.extend(job.carry);
        Ok(())
    }

    /// The end of `_walk_flow` for a `<table>`: one that made blocks but no table block is
    /// counted, its first block noted.
    pub(super) fn table_seen(&mut self, elem: NodeId, start: usize) {
        if self.blocks.len() <= start || self.tables.native_made.contains(&elem) {
            return;
        }
        let first = self.blocks[start].id();
        if self.tables.notes.contains(&elem) {
            self.tables.notes_seen.push(first);
        } else {
            self.tables.seen.push(first);
        }
    }
}

/// `_header_not_bold(cell, css)`: a header cell whose weight comes to normal (#597).
fn header_not_bold(header: bool, css: Option<&Resolved>) -> Result<bool, ItemError> {
    if !header {
        return Ok(false);
    }
    let weight = keyword(css, "font-weight")?;
    // `bolder` reads as 400 here; the cell's block style makes it bold.
    Ok(!weight.is_empty() && parse_weight(Some(&PyValue::Str(weight))) < 600)
}

/// `_span_attr(cell, name)` as the model holds it.
fn span_py(dom: &Dom, n: NodeId, name: &str) -> PyValue {
    // At most 1000.
    PyValue::Int(span_attr(dom, n, name) as i64)
}

/// `_walk_cell`'s ids left pending at the cell's end, onto its last block where it lacks them:
/// `last["anchor_ids"].append(aid)`, `last["anchor_offsets"][aid] = 0`. Each id is looked up,
/// not searched for.
pub(super) fn add_ids(last: &mut Block, pending: &[String]) {
    let mut ids: Vec<String> = str_list(last.get("anchor_ids"))
        .map(str::to_owned)
        .collect();
    let mut have: HashSet<String> = ids.iter().cloned().collect();
    let mut offs = match last.get("anchor_offsets") {
        Some(PyValue::Dict(pairs)) => pairs.clone(),
        _ => Vec::new(),
    };
    let mut at: HashMap<String, usize> = offs
        .iter()
        .enumerate()
        .filter_map(|(i, (k, _))| Some((k.as_str()?.to_owned(), i)))
        .collect();
    for aid in pending {
        if !have.insert(aid.clone()) {
            continue;
        }
        ids.push(aid.clone());
        match at.get(aid) {
            Some(&i) => offs[i].1 = PyValue::Int(0),
            None => {
                at.insert(aid.clone(), offs.len());
                offs.push((s(aid), PyValue::Int(0)));
            }
        }
    }
    last.set("anchor_ids", strs(&ids));
    last.set("anchor_offsets", PyValue::Dict(offs));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    /// A `<td>` (in a row, a body and a table) whose `name` attribute is `value`.
    fn cell_with(name: &str, value: Option<&str>) -> (Dom, NodeId) {
        let mut dom = dom::parse(b"<table><tbody><tr><td/></tr></tbody></table>").unwrap();
        let td = dom.descendants().find(|&n| dom.tag(n) == "td").unwrap();
        dom.set_attr(td, name, value);
        (dom, td)
    }

    /// Values printed by the pinned `_span_attr`.
    #[test]
    fn spans_are_read_as_kfxgen_reads_them() {
        let cases: [(Option<&str>, usize); 18] = [
            (None, 1),
            (Some(""), 1),
            (Some("2"), 2),
            (Some(" 3 "), 3),
            (Some("0"), 1),
            (Some("-4"), 1),
            (Some("1000"), 1000),
            (Some("1001"), 1000),
            (Some("10000000000000000000000"), 1000),
            (Some("\u{663}"), 3),
            (Some("1_0"), 10),
            (Some("1__0"), 1),
            (Some("_1"), 1),
            (Some("+5"), 5),
            (Some("2.5"), 1),
            (Some("abc"), 1),
            (Some("\u{a0}5\u{a0}"), 5),
            (Some("\u{1c}7\u{1c}"), 7),
        ];
        for (value, want) in cases {
            let (dom, td) = cell_with("colspan", value);
            assert_eq!(span_attr(&dom, td, "colspan"), want, "{value:?}");
        }
        // Past 4300 digits `int()` raises, leading zeros counted: 1.
        for (digits, want) in [
            (format!("{}7", "0".repeat(4299)), 7),
            (format!("{}7", "0".repeat(4300)), 1),
        ] {
            let (dom, td) = cell_with("colspan", Some(&digits));
            assert_eq!(
                span_attr(&dom, td, "colspan"),
                want,
                "{} digits",
                digits.len()
            );
        }
        let (dom, td) = cell_with("colspan", Some(&"9".repeat(5000)));
        assert_eq!(span_attr(&dom, td, "colspan"), 1);
    }

    /// Values printed by the pinned `_width_pct` with no style resolver.
    #[test]
    fn widths_in_percent_are_read_as_kfxgen_reads_them() {
        let cases: [(Option<&str>, Option<f64>); 13] = [
            (None, None),
            (Some("50%"), Some(50.0)),
            (Some(" 50 % "), Some(50.0)),
            (Some("50"), None),
            (Some("50 px"), None),
            (Some("\u{663}\u{665}%"), Some(35.0)),
            (Some(".5%"), Some(0.5)),
            (Some("5.%"), Some(5.0)),
            (Some("1e2%"), None),
            (Some("%"), None),
            (Some("-5%"), None),
            (Some("\u{a0}20%\u{2003}"), Some(20.0)),
            (Some("\u{1c}20%"), Some(20.0)),
        ];
        for (value, want) in cases {
            let (dom, td) = cell_with("width", value);
            assert_eq!(width_pct(&dom, td, None).unwrap(), want, "{value:?}");
        }
        for value in [".%", "1.2.3%", "..%"] {
            let (dom, td) = cell_with("width", Some(value));
            match width_pct(&dom, td, None) {
                Err(ItemError::Raised(r)) => assert_eq!(r.exception, "ValueError"),
                other => panic!("{value}: {other:?}"),
            }
        }
    }

    /// Values printed by the pinned `_attr_border_px`, and `round(0.45 * px, 4)`.
    #[test]
    fn border_attributes_are_read_as_kfxgen_reads_them() {
        let cases: [(Option<&str>, Option<f64>); 9] = [
            (None, None),
            (Some(""), Some(0.45)),
            (Some("0"), None),
            (Some("1"), Some(0.45)),
            (Some("3px"), Some(1.35)),
            (Some(" 2"), Some(0.9)),
            (Some("x"), Some(0.45)),
            (Some("\u{663}"), Some(1.35)),
            (Some("00"), None),
        ];
        for (value, want) in cases {
            let (mut dom, _) = cell_with("x", None);
            let table = dom.descendants().find(|&n| dom.tag(n) == "table").unwrap();
            dom.set_attr(table, "border", value);
            let got = match attr_border_px(&dom, table).unwrap() {
                BorderPx::Zero => None,
                BorderPx::Px(px) => Some(py::round(0.45 * px, 4)),
            };
            assert_eq!(got, want, "{value:?}");
        }
        let px = |digits: &str| {
            let (mut dom, _) = cell_with("x", None);
            let table = dom.descendants().find(|&n| dom.tag(n) == "table").unwrap();
            dom.set_attr(table, "border", Some(digits));
            attr_border_px(&dom, table)
        };
        // 0.45 * int: 308 ones fit a float, 309 still do, 400 raise OverflowError.
        assert_eq!(
            px(&"1".repeat(308)).unwrap(),
            BorderPx::Px(1.1111111111111111e307)
        );
        assert!(matches!(px(&"1".repeat(400)), Ok(BorderPx::Px(p)) if p.is_infinite()));
        assert!(matches!(px(&"1".repeat(4300)), Ok(BorderPx::Px(_))));
        assert!(matches!(px(&"1".repeat(4301)), Err(r) if r.exception == "ValueError"));
    }

    /// Values printed by the pinned `_NOTE_MARKER_RE.match`.
    #[test]
    fn note_markers_are_kfxgen_s() {
        let yes = [
            "1",
            "[12]",
            "(3).",
            "4:",
            "iv",
            "IV",
            "\u{131}v",
            "\u{130}x",
            "mcmxciv",
            "*",
            "**",
            "\u{2020}.",
            "#]",
            "(x)",
            "x)",
            "cm",
            "cdxliv",
            "ix.",
            "[iv]:",
            "1)",
            "\u{661}\u{662}",
            "(*):",
            "**.",
        ];
        let no = [
            "12345", "mild", "civil", "[", "", "a", "mmmmm", "vv", "iiii", "viiii", ")1",
            "\u{2170}",
        ];
        for m in yes {
            assert!(is_note_marker(m), "{m:?}");
        }
        for m in no {
            assert!(!is_note_marker(m), "{m:?}");
        }
    }

    /// A differential set run through the pinned `_NOTE_MARKER_RE.match` under
    /// `python3 -I`: Python's `IGNORECASE` folds `ı` and `İ` to `i` but not the Kelvin sign
    /// or the roman numeral signs to a numeral letter, and `\d` is any Unicode decimal digit
    /// (`Nd`) but not `²` or `①`.
    #[test]
    fn note_markers_match_kfxgen_s_on_unicode_and_long_text() {
        let cases: &[(&str, bool)] = &[
            ("\u{131}", true),
            ("\u{130}", true),
            ("\u{131}v", true),
            ("x\u{130}", true),
            ("\u{130}\u{130}\u{130}", true),
            ("\u{131}\u{131}\u{131}\u{131}", false),
            ("x\u{130}v", true),
            ("(\u{130}X).", true),
            ("[\u{131}]:", true),
            ("\u{212a}", false),
            ("\u{212a}i", false),
            ("\u{17f}", false),
            ("\u{2160}", false),
            ("\u{2170}", false),
            ("\u{216b}", false),
            ("\u{3b9}", false),
            ("\u{345}", false),
            ("\u{df}", false),
            ("\u{1e9e}", false),
            ("K", false),
            ("MCMXCIX", true),
            ("mmmmcmxcix", true),
            ("MMMMM", false),
            ("mmmmcccxxxiii", true),
            ("xxxx", false),
            ("lx", true),
            ("ll", false),
            ("dd", false),
            ("vx", false),
            ("il", false),
            ("cccd", false),
            ("\u{661}\u{662}\u{663}\u{664}", true),
            ("\u{661}\u{662}\u{663}\u{664}\u{665}", false),
            ("\u{967}\u{968}", true),
            ("\u{ff11}\u{ff12}", true),
            ("\u{e51}", true),
            ("\u{7c0}", true),
            ("\u{1d7ce}", true),
            ("\u{b2}", false),
            ("\u{2460}", false),
            ("1\u{b2}", false),
            ("(4]:", true),
            ("4)).", false),
            ("4..", false),
            ("()", false),
            ("*1", false),
        ];
        for &(m, want) in cases {
            assert_eq!(is_note_marker(m), want, "{m:?}");
        }
        assert!(!is_note_marker(&"i".repeat(10_000)));
        assert!(!is_note_marker(&"1".repeat(10_000)));
        assert!(is_note_marker(&"*".repeat(100_000)));
        assert!(is_note_marker(&format!("[{}]:", "*".repeat(50_000))));
        // Linear: a million-character marker is read in a few passes.
        let start = std::time::Instant::now();
        assert!(!is_note_marker(&format!("{}x", "*".repeat(1_000_000))));
        assert!(!is_note_marker(&"m".repeat(1_000_000)));
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    /// `_table_width` and `_cell_columns` as the pinned functions print them for rows of
    /// `(colspan, rowspan)` cells.
    #[test]
    fn columns_are_counted_as_kfxgen_counts_them() {
        let table_of = |rows: &[&[(usize, usize)]]| {
            let mut dom = dom::parse(b"<table/>").unwrap();
            let table = dom.root();
            let tbody = dom.append_child(table, "tbody");
            for row in rows {
                let tr = dom.append_child(tbody, "tr");
                for &(c, r) in *row {
                    let td = dom.append_child(tr, "td");
                    dom.set_attr(td, "colspan", Some(&c.to_string()));
                    dom.set_attr(td, "rowspan", Some(&r.to_string()));
                }
            }
            (dom, table)
        };
        // Rows of (colspan, rowspan), the width, the first cells' (column, colspan).
        type Case<'c> = (&'c [&'c [(usize, usize)]], usize, &'c [(usize, usize)]);
        let cases: [Case; 5] = [
            (&[&[(1, 1); 3]], 3, &[(0, 1), (1, 1), (2, 1)]),
            (
                &[&[(2, 2), (1, 1)], &[(1, 1)]],
                3,
                &[(0, 2), (2, 1), (2, 1)],
            ),
            (
                &[&[(1, 3), (1, 1)], &[(1, 1)], &[(1, 1), (1, 1)]],
                3,
                &[(0, 1), (1, 1), (1, 1), (1, 1), (2, 1)],
            ),
            (&[&[(25, 1)]], 25, &[(0, 25)]),
            (
                &[&[(1, 1000); 24], &[(1, 1)]],
                25,
                &[(0, 1), (1, 1), (2, 1)],
            ),
        ];
        for (rows, width, starts) in cases {
            let (dom, table) = table_of(rows);
            assert_eq!(table_width(&dom, table).unwrap(), width);
            let mut got = Vec::new();
            cell_columns(&dom, table, |_, col, span| {
                got.push((col, span));
                Ok(true)
            })
            .unwrap();
            assert_eq!(&got[..starts.len()], starts);
        }
    }
}
