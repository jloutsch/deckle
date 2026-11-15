// SPDX-License-Identifier: GPL-3.0-only
//! Margins and the space between blocks, at kfxgen `ad38ec8`: what the walk notes on and
//! beside its blocks (`_note_margins`, `_note_padding`, `_box_share`,
//! `_carry_container_margins`, `_emit_space` for spacers), and `_space_between`, which turns
//! those margins into each block's space above and below once the walk is done
//! (`_place_margin`, `_mark_tight_runs`), with the module helpers they call (`_margin_em`,
//! `_padding_em`, `_font_ratio`, `_collapse`, `_hidden_rule_em`, `_spacer_em`, `_page_share`,
//! `_add_margin`, `_has_content`, `_has_real_text`, `_ems_own`, `_takes_margin`,
//! `_spaces_itself`).
//!
//! Float arithmetic follows the Python operation for operation, in its order; `max` and `min`
//! keep the first of equal arguments, as Python's do ([`py_max`], [`py_min`]).
//!
//! A box (a native list's, Task 13) is spaced as kfxgen spaces any: `_note_box_margins`, its
//! side margins in `_carry_container_margins`, and the box part of `_space_between` (its
//! blocks spaced inside it, what leaves it). A box drawn vertical, or horizontal in vertical
//! text, is Task 15's: each such branch stops the file with "boxes (Task 15)".

use std::collections::HashMap;

use super::walk::{BORDER_SIDES, Walker, css_border, has_words, text_field, unit_percent};
use super::{Block, BlockId, Field, dict_set, s, unsupported};
use crate::convert::block_style::compute_block_style;
use crate::convert::inline::{keyword, text_of};
use crate::convert::inline_style::parse_css_length;
use crate::convert::resolver::Resolved;
use crate::convert::{Error, ItemError, Raised};
use crate::css::TEXT_UNIT;
use crate::dom::{Dom, NodeId};
use crate::generate::images::IMG_TOKEN;
use crate::generate::page_scale::page_pt;
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `_LINE_SPACE_EM`: one blank line, what a spacer adds without a resolved style.
const LINE_SPACE_EM: f64 = 1.2;
/// `_HIDDEN_RULE_SHARE`: the share of an invisible rule's lengths Previewer keeps.
const HIDDEN_RULE_SHARE: f64 = 0.6;
/// `_MARGIN_BREAK_EM`: the least gap margins alone make that is a scene break.
const MARGIN_BREAK_EM: f64 = 1.5;
/// `_SHORT_LINE`, `_SHORT_SHARE`: a tight run's short lines.
const SHORT_LINE: usize = 60;
const SHORT_SHARE: f64 = 0.9;
/// `_MAX_CARRIED_MARGINS`: the most of the page width carried margins may leave unused.
const MAX_CARRIED_MARGINS: f64 = 50.0;
/// `_NO_CARRY_TAGS`: a table's margins do not reach its rows.
const NO_CARRY_TAGS: [&str; 9] = [
    "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup",
];

/// `float()` of a number `[\d.]+` matched that has no digit or more than one point (`.`,
/// `1.2.3`).
const NOT_A_NUMBER: Raised = Raised {
    exception: "ValueError",
    note: "_padding_em, _hidden_rule_em: float() of a length's number of points and digits, \
           such as \".\" or \"1.2.3\"",
};
/// `round()` of a NaN to an int.
const ROUND_NAN: Raised = Raised {
    exception: "ValueError",
    note: "_spacer_em: round() of a NaN line height or font size",
};
/// `round()` of an infinity to an int.
const ROUND_INF: Raised = Raised {
    exception: "OverflowError",
    note: "_spacer_em: round() of an infinite line height or font size",
};

/// What the walk keeps beside its blocks by identity (kfxgen's `_ById` tables), and what
/// `_box_share` reads again and again, kept once per file.
pub(super) struct Noted {
    pub(super) margins: HashMap<BlockId, [f64; 2]>,
    pub(super) paddings: HashMap<BlockId, [f64; 4]>,
    /// `_has_content(n)` per node.
    has_content: Vec<Option<bool>>,
    /// Per node: whether it keeps a margin from collapsing past it (`_box_share`'s `near`),
    /// once worked out.
    blocks_margin: Vec<Option<bool>>,
    /// Per node known not to keep a margin: the index among its parent's child nodes from
    /// which the next one that might is looked for (0: not known, as a later index is
    /// always above 0). Each node is looked at once, however many children ask.
    next_maybe: Vec<usize>,
    /// Per block, what `_carry_container_margins` finds in its text: worked out at the first
    /// container round it and kept, as a block's text never changes once it is made.
    carry_text: HashMap<BlockId, CarryText>,
    /// Per element and side (top: true): whether its border or padding there stops a margin.
    stops: HashMap<(NodeId, bool), bool>,
    /// Per element: the share `_box_share`'s second loop has worked out once it reaches it.
    shares: HashMap<NodeId, f64>,
}

impl Noted {
    pub(super) fn new(dom: &Dom) -> Noted {
        Noted {
            margins: HashMap::new(),
            paddings: HashMap::new(),
            has_content: vec![None; dom.node_count()],
            blocks_margin: vec![None; dom.node_count()],
            next_maybe: vec![0; dom.node_count()],
            stops: HashMap::new(),
            shares: HashMap::new(),
            carry_text: HashMap::new(),
        }
    }
}

/// What a block's text is to `_carry_container_margins`.
#[derive(Clone, Copy, PartialEq)]
enum CarryText {
    /// Words outside the picture tokens (`_has_real_text`): its style takes the margins.
    Words,
    /// Picture tokens and no words: its `picture_boxes` take them.
    Pictures,
    /// Neither: left as it is.
    Nothing,
}

/// The bytes of the strings in a `(magnitude, unit)` pair, or none.
fn pair_bytes(v: &PyValue) -> usize {
    match v {
        PyValue::Tuple(parts) => parts
            .iter()
            .map(|p| match p {
                PyValue::Str(t) => t.len(),
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

/// Python's truth of a float: NaN is true.
fn truthy(x: f64) -> bool {
    x != 0.0
}

/// `max(a, b, …)`: the first of equal arguments, and a NaN first stays (nothing is greater).
fn py_max(items: &[f64]) -> f64 {
    let mut m = items[0];
    for &x in &items[1..] {
        if x > m {
            m = x;
        }
    }
    m
}

/// `min(a, b, …)`, as [`py_max`].
fn py_min(items: &[f64]) -> f64 {
    let mut m = items[0];
    for &x in &items[1..] {
        if x < m {
            m = x;
        }
    }
    m
}

/// `_collapse(a, b)`: two touching margins as one, the largest positive plus the most
/// negative.
pub(super) fn collapse(a: f64, b: f64) -> f64 {
    py_max(&[0.0, a, b]) + py_min(&[0.0, a, b])
}

/// A number the model holds, as Python's arithmetic reads it; anything else as 0.
fn number(v: Option<&PyValue>) -> f64 {
    match v {
        Some(PyValue::Float(f)) => *f,
        Some(PyValue::Int(i)) => *i as f64,
        Some(PyValue::Bool(b)) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

/// `v or …`: the number when it is a true one.
fn true_number(v: Option<&PyValue>) -> Option<f64> {
    v.filter(|v| v.is_truthy()).map(|v| number(Some(v)))
}

/// A length as `\s*(-?[\d.]+)\s*(px|pt|em|rem|%)?\s*` matches the whole of it (the `-` only
/// with `signed`): its number and its unit. `\s` and `\d` are Python's (Unicode white space
/// and decimal digits).
fn match_length(value: &str, signed: bool) -> Option<(&str, Option<&'static str>)> {
    let rest = value.trim_start_matches(py::is_space);
    let sign = usize::from(signed && rest.starts_with('-'));
    let digits = rest[sign..]
        .find(|c: char| !(c == '.' || py::digit_value(c).is_some()))
        .unwrap_or(rest.len() - sign);
    if digits == 0 {
        return None;
    }
    let (number, after) = rest.split_at(sign + digits);
    let after = after.trim_start_matches(py::is_space);
    let (unit, after) = ["px", "pt", "em", "rem", "%"]
        .into_iter()
        .find_map(|u| after.strip_prefix(u).map(|a| (Some(u), a)))
        .unwrap_or((None, after));
    after.chars().all(py::is_space).then_some((number, unit))
}

/// A padding or margin read by `_padding_em`: ems, or `("%", n)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pad {
    Em(f64),
    Pct(f64),
}

/// `_padding_em(value, ratio)`: a padding in the 16px root's ems (an em the element's own,
/// `ratio` its font size against 16px), a percentage, or None when unreadable. Raises where
/// `float()` does.
pub(super) fn padding_em(value: &str, ratio: f64) -> Result<Option<Pad>, Raised> {
    let Some((number, unit)) = match_length(value, false) else {
        return Ok(None);
    };
    let n = py::py_float(number).ok_or(NOT_A_NUMBER)?;
    Ok(match unit {
        Some("%") => Some(Pad::Pct(n)),
        None => (n == 0.0).then_some(Pad::Em(0.0)),
        Some(unit) => {
            // `_PADDING_EM_PER_UNIT`.
            let per = match unit {
                "px" => 0.45 / 12.0,
                "pt" => 1.0 / 12.0,
                _ => 1.0,
            };
            Some(Pad::Em(n * per * if unit == "em" { ratio } else { 1.0 }))
        }
    })
}

/// `_font_ratio(css)`: the element's font size against the 16px root, 1.0 without one.
pub(super) fn font_ratio(css: Option<&Resolved>) -> f64 {
    match css.and_then(Resolved::font_size) {
        Some(px) => number(Some(px)) / 16.0,
        None => 1.0,
    }
}

/// `_margin_em(css, side, share)`: the element's margin-top or -bottom in the 16px root's
/// ems, a percentage of `share` of the page, negative as the book sets it, 0 when unset.
fn margin_em(css: Option<&Resolved>, side: &str, share: f64) -> Result<f64, ItemError> {
    let key = if side == "top" {
        "margin-top"
    } else {
        "margin-bottom"
    };
    let raw = match css {
        Some(css) => css.get(key)?,
        None => None,
    };
    let text = text_of(raw);
    let value = py::strip(&text);
    let sign = if value.starts_with('-') { -1.0 } else { 1.0 };
    Ok(
        match padding_em(value.trim_start_matches('-'), font_ratio(css))? {
            Some(Pad::Pct(n)) => sign * n * (page_pt() / 1200.0) * share,
            Some(Pad::Em(em)) => sign * em,
            None => 0.0,
        },
    )
}

/// `_hidden_rule_em(css)` from the values it reads: (margin-top, height, margin-bottom) of an
/// `<hr>` that draws nothing, in the 16px root's ems, 0.6 of each; with no height its margins
/// collapse into one. Task 14's invisible rule calls it too.
pub(super) fn hidden_rule_em(
    margin_top: &str,
    margin_bottom: &str,
    height: &str,
    ratio: f64,
) -> Result<(f64, f64, f64), Raised> {
    let em = |value: &str| -> Result<f64, Raised> {
        let Some((number, Some(unit))) = match_length(value, true) else {
            return Ok(0.0);
        };
        // `_HIDDEN_RULE_EM_PER_UNIT`.
        let per = match unit {
            "px" => 1.0 / 16.0,
            "pt" => 1.0 / 12.0,
            "%" => 0.74,
            _ => 1.0,
        };
        let n = py::py_float(number).ok_or(NOT_A_NUMBER)? * per;
        Ok(if unit == "em" { n * ratio } else { n })
    };
    let (top, bottom) = (em(margin_top)?, em(margin_bottom)?);
    let height = py_max(&[0.0, em(height)?]);
    let share = HIDDEN_RULE_SHARE;
    if height == 0.0 {
        return Ok((collapse(top, bottom) * share, 0.0, 0.0));
    }
    Ok((top * share, height * share, bottom * share))
}

/// `str(round(x))`: the nearest integer, half to even, as Python writes it. Raises where
/// `round()` does.
fn round_to_int_text(x: f64) -> Result<String, Raised> {
    if x.is_nan() {
        return Err(ROUND_NAN);
    }
    if x.is_infinite() {
        return Err(ROUND_INF);
    }
    // `+ 0.0`: Python's int 0 has no sign.
    Ok(format!("{:.0}", x.round_ties_even() + 0.0))
}

/// `_spacer_em(css)`: (margin-top, height, margin-bottom) of a spacer paragraph, as an
/// invisible rule of its line's height in whole px.
fn spacer_em(css: &Resolved) -> Result<(f64, f64, f64), ItemError> {
    let own = css.font_size().map_or(16.0, |px| number(Some(px)));
    let factor = match css.get("line-height")? {
        Some(v @ (PyValue::Float(_) | PyValue::Int(_) | PyValue::Bool(_))) => Some(number(Some(v))),
        _ => None,
    };
    let declared = keyword(Some(css), "line-height-declared")?;
    let line = if declared == "normal" {
        round_to_int_text(own * 4.0 / 3.0)?
    } else if let Some(factor) = factor
        && (!declared.is_empty() || (factor - 1.2).abs() > 1e-6)
    {
        round_to_int_text(factor * own)?
    } else {
        // `_SPACER_LINE_PX`, a float: "19.0px".
        "19.0".to_owned()
    };
    let top = text_of(css.get("margin-top")?).into_owned();
    let bottom = text_of(css.get("margin-bottom")?).into_owned();
    Ok(hidden_rule_em(
        &top,
        &bottom,
        &format!("{line}px"),
        font_ratio(Some(css)),
    )?)
}

/// `_page_share(margin)`: a `(magnitude, unit)` margin as a share of the page width, in %.
fn page_share(margin: &PyValue) -> Result<f64, Raised> {
    let PyValue::Tuple(parts) = margin else {
        return Ok(0.0);
    };
    let (Some(PyValue::Str(mag)), Some(PyValue::Str(unit))) = (parts.first(), parts.get(1)) else {
        return Ok(0.0);
    };
    // A magnitude `parse_css_length` or `_add_margin` wrote: always one `float()` reads.
    Ok(py::py_float(mag).ok_or(NOT_A_NUMBER)? * unit_percent(unit))
}

/// A `(magnitude, unit)` length as the model holds it, or None.
fn length_py(l: &Option<(String, &'static str)>) -> PyValue {
    match l {
        Some((mag, unit)) => PyValue::Tuple(vec![s(mag), s(unit)]),
        None => PyValue::None,
    }
}

/// `f"{x:.4f}"` written into `buf`.
fn fixed_4(x: f64, buf: &mut String) {
    use std::fmt::Write as _;
    buf.clear();
    if x.is_nan() {
        buf.push_str("nan");
    } else {
        // Writing to a String cannot fail.
        let _ = write!(buf, "{x:.4}");
    }
}

/// `_add_margin(own, extra)`: a paragraph's margin with a container's added, as a share of
/// the page width; with no container margin the paragraph's own as it is.
#[cfg(test)]
fn add_margin(own: &PyValue, extra: &PyValue) -> Result<PyValue, Raised> {
    let mut buf = String::new();
    Ok(if add_margin_into(own, extra, &mut buf)? {
        PyValue::Tuple(vec![s(&buf), s("$314")])
    } else {
        own.clone()
    })
}

/// `_add_margin(own, extra)` without making the value: true with the sum's magnitude in
/// `buf` (its unit `$314`), false for no container margin (the margin is `own` as it is).
fn add_margin_into(own: &PyValue, extra: &PyValue, buf: &mut String) -> Result<bool, Raised> {
    if matches!(extra, PyValue::None) {
        return Ok(false);
    }
    let total = page_share(own)? + page_share(extra)?;
    fixed_4(total, buf);
    let kept = buf.trim_end_matches('0').trim_end_matches('.').len();
    buf.truncate(kept);
    Ok(true)
}

/// `_page_share` of what `_add_margin` gave: the sum in `buf`, or `own`.
fn added_share(own: &PyValue, added: bool, buf: &str) -> Result<f64, Raised> {
    if !added {
        return page_share(own);
    }
    Ok(py::py_float(buf).ok_or(NOT_A_NUMBER)? * unit_percent("$314"))
}

/// `style[key] = new` for what `_add_margin` gave: a `(magnitude, unit)` pair already there
/// is overwritten in place, so nested containers make no new value at each step.
fn set_added(style: &mut Vec<(PyValue, PyValue)>, key: &str, own: PyValue, added: bool, buf: &str) {
    if !added {
        dict_set(style, key, own);
        return;
    }
    if let Some((_, PyValue::Tuple(parts))) =
        style.iter_mut().find(|(k, _)| k.as_str() == Some(key))
        && let [PyValue::Str(mag), PyValue::Str(unit)] = parts.as_mut_slice()
    {
        mag.clear();
        mag.push_str(buf);
        if unit != "$314" {
            unit.clear();
            unit.push_str("$314");
        }
        return;
    }
    dict_set(style, key, PyValue::Tuple(vec![s(buf), s("$314")]));
}

/// `_has_real_text(text)`: words once the picture tokens are taken out.
pub(super) fn has_real_text(text: &str) -> bool {
    has_words(&IMG_TOKEN.replace_all(text, ""))
}

/// `_takes_margin(block)`: a paragraph or a box.
fn takes_margin(b: &Block) -> bool {
    matches!(b.kind(), None | Some("box"))
}

/// `_spaces_itself(block)`: a rule.
fn spaces_itself(b: &Block) -> bool {
    b.kind() == Some("rule")
}

/// `_ems_own(block)`: its font size against 16px; a box's is its container's.
fn ems_own(b: &Block) -> f64 {
    let px = if b.kind() == Some("box") {
        true_number(b.get("font_px"))
    } else {
        None
    };
    let size = px
        .or_else(|| true_number(b.get("block_style").and_then(|st| st.get("font_size"))))
        .unwrap_or(16.0);
    size / 16.0
}

/// Whether `b` is a box drawn horizontal in vertical text (`box.horizontal`).
fn horizontal_box(b: &Block) -> bool {
    b.get("box")
        .and_then(|bx| bx.get("horizontal"))
        .is_some_and(PyValue::is_truthy)
}

/// The first and last of `blocks[start:]` that are not a space or a rule (`mine[0]`,
/// `mine[-1]`), as indexes.
fn mine(blocks: &[Block], start: usize) -> Option<(usize, usize)> {
    let counts = |b: &Block| !matches!(b.kind(), Some("space" | "rule"));
    let start = start.min(blocks.len());
    let first = start + blocks[start..].iter().position(counts)?;
    let last = start + blocks[start..].iter().rposition(counts)?;
    Some((first, last))
}

impl Walker<'_, '_, '_, '_> {
    /// `_emit_space(margin, line, height, margin_after)`: space for the next block to take.
    fn emit_space(&mut self, margin: f64, line: f64, height: f64, margin_after: f64) {
        let f = |x: f64| Field::Py(PyValue::Float(x));
        let block = self.new_block(vec![
            ("text", Field::Py(s(""))),
            ("spans", Field::Py(PyValue::List(Vec::new()))),
            ("block_style", Field::Py(PyValue::None)),
            ("anchor_ids", Field::Py(PyValue::List(Vec::new()))),
            ("anchor_offsets", Field::Py(PyValue::Dict(Vec::new()))),
            ("type", Field::Py(s("space"))),
            ("margin", f(margin)),
            ("line", f(line)),
            ("height", f(height)),
            ("margin_after", f(margin_after)),
        ]);
        self.blocks.push(block);
    }

    /// The leaf branch's spacer: a blank line the book put in on purpose.
    pub(super) fn spacer(&mut self, elem: NodeId) -> Result<(), ItemError> {
        match self.resolve(elem)? {
            None => self.emit_space(0.0, LINE_SPACE_EM, 0.0, 0.0),
            Some(css) => {
                let (top, height, bottom) = spacer_em(&css)?;
                self.emit_space(top, 0.0, height, bottom);
            }
        }
        Ok(())
    }

    /// `_note_padding(elem, start)`: an unboxed container's padding-top on the first block it
    /// made and its padding-bottom on the last, added to what is there. Returns which of the
    /// two it noted.
    pub(super) fn note_padding(
        &mut self,
        elem: NodeId,
        start: usize,
    ) -> Result<(bool, bool), ItemError> {
        let Some(css) = self.resolve(elem)? else {
            return Ok((false, false));
        };
        let sides = match css.get("padding-sides")? {
            Some(PyValue::Tuple(sides) | PyValue::List(sides)) if sides.len() == 4 => sides,
            _ => return Ok((false, false)),
        };
        let ratio = font_ratio(Some(&css));
        let em = |value: &PyValue| -> Result<f64, Raised> {
            Ok(match padding_em(&text_of(Some(value)), ratio)? {
                Some(Pad::Pct(n)) => n * (page_pt() / 1200.0),
                Some(Pad::Em(v)) if truthy(v) => v,
                _ => 0.0,
            })
        };
        let (top, bottom) = (em(&sides[0])?, em(&sides[2])?);
        drop(css);
        if !(truthy(top) || truthy(bottom)) {
            return Ok((false, false));
        }
        let Some((first, last)) = mine(&self.blocks, start) else {
            return Ok((false, false));
        };
        // A horizontal block first or last, in vertical text: its padding goes on its sides.
        if (truthy(top) && horizontal_box(&self.blocks[first]))
            || (truthy(bottom) && horizontal_box(&self.blocks[last]))
        {
            return Err(self.unsupported_box());
        }
        let (first, last) = (self.blocks[first].id(), self.blocks[last].id());
        let pads = &mut self.noted.paddings;
        let p = pads.entry(first).or_insert([0.0; 4]);
        if truthy(top) {
            p[0] += p[2] + top;
            p[2] = 0.0;
        }
        let p = pads.entry(last).or_insert([0.0; 4]);
        if truthy(bottom) {
            p[1] += p[3] + bottom;
            p[3] = 0.0;
        }
        Ok((truthy(top), truthy(bottom)))
    }

    /// `_note_margins(elem, start, padded)`: `elem`'s margin-top on the first block it made
    /// and its margin-bottom on the last, collapsed with what is there; outside a padding
    /// noted on that side, apart from the block's own.
    pub(super) fn note_margins(
        &mut self,
        elem: NodeId,
        start: usize,
        padded: (bool, bool),
    ) -> Result<(), ItemError> {
        let css = self.resolve(elem)?;
        let share = self.box_share(elem, true)?;
        let top = margin_em(css.as_ref(), "top", share)?;
        let share = self.box_share(elem, false)?;
        let bottom = margin_em(css.as_ref(), "bottom", share)?;
        let unit = keyword(css.as_ref(), "margin-bottom")?;
        let in_em = unit.ends_with("em");
        let ratio = font_ratio(css.as_ref());
        drop(css);
        let mine = mine(&self.blocks, start);
        if let Some((_, last)) = mine {
            let b = &mut self.blocks[last];
            if b.kind().is_none() && !b.contains("own_bottom_em") {
                b.set("own_bottom_em", PyValue::Bool(!truthy(bottom) || in_em));
            }
        }
        if !(truthy(top) || truthy(bottom)) {
            return Ok(());
        }
        let Some((first, last)) = mine else {
            return Ok(());
        };
        // A horizontal block first or last: the margins go on its sides.
        if (truthy(top) && horizontal_box(&self.blocks[first]))
            || (truthy(bottom) && horizontal_box(&self.blocks[last]))
        {
            return Err(self.unsupported_box());
        }
        let first_id = self.blocks[first].id();
        let last_id = self.blocks[last].id();
        let noted = &mut self.noted;
        // `paddings[mine[0]]`: there whenever `padded[0]` (`_note_padding` set it).
        if padded.0 || noted.paddings.get(&first_id).is_some_and(|p| truthy(p[0])) {
            let outer = noted.paddings.entry(first_id).or_insert([0.0; 4]);
            outer[2] = collapse(outer[2], top);
        } else {
            let m = noted.margins.entry(first_id).or_insert([0.0; 2]);
            m[0] = collapse(m[0], top);
        }
        if padded.1 || noted.paddings.get(&last_id).is_some_and(|p| truthy(p[1])) {
            let outer = noted.paddings.entry(last_id).or_insert([0.0; 4]);
            if truthy(bottom) && bottom >= outer[3] {
                let value = if in_em {
                    PyValue::Float(ratio)
                } else {
                    PyValue::None
                };
                self.blocks[last].set("outer_em_ratio", value);
            }
            outer[3] = collapse(outer[3], bottom);
        } else {
            let m = noted.margins.entry(last_id).or_insert([0.0; 2]);
            if truthy(bottom) && bottom >= m[1] {
                self.blocks[last].set("bottom_in_em", PyValue::Bool(in_em));
            }
            m[1] = collapse(m[1], bottom);
        }
        Ok(())
    }

    /// `_carry_container_margins(elem, start)`: a container's side margins added to each text
    /// block it made, as a share of the page; a picture alone keeps them as written
    /// (`picture_boxes`).
    pub(super) fn carry_container_margins(
        &mut self,
        elem: NodeId,
        start: usize,
    ) -> Result<(), ItemError> {
        if NO_CARRY_TAGS.contains(&self.dom.tag(elem)) {
            return Ok(());
        }
        let css = self.resolve(elem)?;
        let side = |key: &str| -> Result<Option<(String, &'static str)>, ItemError> {
            let raw = match &css {
                Some(css) => text_of(css.get(key)?).into_owned(),
                None => String::new(),
            };
            Ok(parse_css_length(&raw, false)?)
        };
        let (left, right) = (side("margin-left")?, side("margin-right")?);
        drop(css);
        if left.is_none() && right.is_none() {
            return Ok(());
        }
        let (left, right) = (length_py(&left), length_py(&right));
        let mut empty_style = None;
        let (mut left_buf, mut right_buf) = (String::new(), String::new());
        let pushed = pair_bytes(&left).saturating_add(pair_bytes(&right));
        for i in start.min(self.blocks.len())..self.blocks.len() {
            // Each block costs a step at each container round it: nested containers over
            // many blocks are refused after bounded work.
            self.resolver.charge(TEXT_UNIT)?;
            match self.blocks[i].kind() {
                Some("box") if horizontal_box(&self.blocks[i]) => {
                    return Err(self.unsupported_box());
                }
                Some("box") => {
                    // The box takes them, not its paragraphs: `(x or 0) + share or None`.
                    let (left, right) = (page_share(&left)?, page_share(&right)?);
                    if let Some(PyValue::Dict(bx)) = self.blocks[i].get_mut("box") {
                        for (key, share) in [("margin_left", left), ("margin_right", right)] {
                            let had = bx
                                .iter()
                                .find(|(k, _)| k.as_str() == Some(key))
                                .map(|(_, v)| v);
                            let sum = match had {
                                Some(v) if v.is_truthy() => number(Some(v)) + share,
                                // `0 + share`: Python's int 0 added to a float.
                                _ => 0.0 + share,
                            };
                            let value = if truthy(sum) {
                                PyValue::Float(sum)
                            } else {
                                PyValue::None
                            };
                            dict_set(bx, key, value);
                        }
                    }
                    continue;
                }
                Some("table") => continue,
                _ => {}
            }
            let id = self.blocks[i].id();
            let found = match self.noted.carry_text.get(&id) {
                Some(&known) => known,
                None => {
                    let text = text_field(&self.blocks[i]);
                    let found = if has_real_text(text) {
                        CarryText::Words
                    } else if IMG_TOKEN.is_match(text) {
                        CarryText::Pictures
                    } else {
                        CarryText::Nothing
                    };
                    self.noted.carry_text.insert(id, found);
                    found
                }
            };
            if found != CarryText::Words {
                if found == CarryText::Pictures {
                    // Pictures alone: each container's margins kept as written, the
                    // innermost first. Each copy of them is charged.
                    self.resolver.charge(pushed)?;
                    let pair = PyValue::Tuple(vec![left.clone(), right.clone()]);
                    let b = &mut self.blocks[i];
                    match b.get_mut("picture_boxes") {
                        Some(PyValue::Tuple(items)) => items.push(pair),
                        _ => b.set("picture_boxes", PyValue::Tuple(vec![pair])),
                    }
                }
                continue;
            }
            let has_own = |b: &Block| matches!(b.get("block_style"), Some(PyValue::Dict(pairs)) if !pairs.is_empty());
            if !has_own(&self.blocks[i]) && empty_style.is_none() {
                empty_style = Some(compute_block_style(&self.resolver.no_style())?);
            }
            let empty: &[(PyValue, PyValue)] = match &empty_style {
                Some(PyValue::Dict(pairs)) => pairs,
                _ => &[],
            };
            // kfxgen copies the style before adding (`dict(...)`), which keeps a style two
            // blocks share apart; each block owns its style here, so it is changed in place,
            // and only once the sum is under the cap.
            let style: &[(PyValue, PyValue)] = match self.blocks[i].get("block_style") {
                Some(PyValue::Dict(pairs)) if !pairs.is_empty() => pairs,
                _ => empty,
            };
            let get = |key: &str| {
                style
                    .iter()
                    .find(|(k, _)| k.as_str() == Some(key))
                    .map_or(&PyValue::None, |(_, v)| v)
            };
            let (own_left, own_right) = (get("margin_left"), get("margin_right"));
            let added_left = add_margin_into(own_left, &left, &mut left_buf)?;
            let added_right = add_margin_into(own_right, &right, &mut right_buf)?;
            if added_share(own_left, added_left, &left_buf)?
                + added_share(own_right, added_right, &right_buf)?
                > MAX_CARRIED_MARGINS
            {
                // Nested indents would leave a narrow column: the paragraph's own margins.
                continue;
            }
            // Cloned only when no container margin is added on that side: None or a pair.
            let own_left = if added_left {
                PyValue::None
            } else {
                own_left.clone()
            };
            let own_right = if added_right {
                PyValue::None
            } else {
                own_right.clone()
            };
            let b = &mut self.blocks[i];
            let put = |style: &mut Vec<(PyValue, PyValue)>| {
                set_added(style, "margin_left", own_left, added_left, &left_buf);
                set_added(style, "margin_right", own_right, added_right, &right_buf);
            };
            match b.get_mut("block_style") {
                Some(PyValue::Dict(pairs)) if !pairs.is_empty() => put(pairs),
                _ => {
                    let mut style = empty.to_vec();
                    put(&mut style);
                    b.set("block_style", PyValue::Dict(style));
                }
            }
        }
        Ok(())
    }

    /// `_note_box_margins(box, css)` for the box at `self.blocks[at]`: its margins, spaced
    /// as a paragraph's; on a side with no border or padding, the margin of the block inside
    /// meets the box's and collapses into it. A box drawn vertical (`box.vertical`) is Task
    /// 15's.
    pub(super) fn note_box_margins(
        &mut self,
        at: usize,
        css: Option<&Resolved>,
    ) -> Result<(), ItemError> {
        let mut top = margin_em(css, "top", 1.0)?;
        let mut bottom = margin_em(css, "bottom", 1.0)?;
        // In em or rem, a spacer after it reads it in the box's own size.
        let in_em = truthy(bottom) && keyword(css, "margin-bottom")?.ends_with("em");
        let Some(b) = self.blocks.get_mut(at) else {
            return Ok(());
        };
        b.set("bottom_in_em", PyValue::Bool(in_em));
        b.set("own_bottom_em", PyValue::Bool(!truthy(bottom) || in_em));
        let b = &self.blocks[at];
        let bx = b.get("box");
        let side_of = |key: &str, side: &str| {
            bx.and_then(|bx| bx.get(key))
                .and_then(|d| d.get(side))
                .is_some_and(PyValue::is_truthy)
        };
        let edge = |side: &str| side_of("border", side) || side_of("padding", side);
        let (edge_top, edge_bottom) = (edge("top"), edge("bottom"));
        if bx
            .and_then(|bx| bx.get("vertical"))
            .is_some_and(PyValue::is_truthy)
        {
            return Err(unsupported(self.w, "boxes (Task 15)"));
        }
        let id = b.id();
        let kids = b.blocks().unwrap_or(&[]);
        let counts = |k: &&Block| !matches!(k.kind(), Some("space" | "rule"));
        let first = kids.iter().find(counts).map(Block::id);
        let last = kids.iter().rev().find(counts).map(Block::id);
        // A spacer or rule after the last block meets its margin first.
        let spaced_end = kids
            .last()
            .is_some_and(|k| matches!(k.kind(), Some("space" | "rule")));
        if let Some(inner) = first.and_then(|k| self.noted.margins.get_mut(&k))
            && !edge_top
        {
            top = collapse(top, inner[0]);
            inner[0] = 0.0;
        }
        if let Some(inner) = last.and_then(|k| self.noted.margins.get_mut(&k))
            && !edge_bottom
            && !spaced_end
        {
            bottom = collapse(bottom, inner[1]);
            inner[1] = 0.0;
        }
        self.noted.margins.insert(id, [top, bottom]);
        Ok(())
    }

    fn unsupported_box(&self) -> ItemError {
        unsupported(self.w, "boxes (Task 15)")
    }

    /// `_box_share(elem, side)`: the share of the page a % margin on that side of `elem` is
    /// measured against. Walking out, a block the margin collapses out of is skipped; from
    /// the first that holds it, outwards, each block's width (or the page less its side
    /// borders and padding) narrows it.
    fn box_share(&mut self, elem: NodeId, top: bool) -> Result<f64, ItemError> {
        let dom = self.dom;
        let mut child = elem;
        let mut held = None;
        for a in dom.ancestors(elem) {
            if matches!(dom.tag(a), "body" | "html") {
                break;
            }
            let stops = self.stops(a, top)?;
            let at = dom.node_index(child);
            let text = if top { dom.text(a) } else { dom.tail(child) };
            // As Python's `or` and `any`: the children on that side are read only when the
            // text has no words, in order, up to the first that keeps the margin.
            let at_edge = !(has_words(text) || {
                let kids = dom.children(a);
                let (from, to) = if top { (0, at) } else { (at + 1, kids.len()) };
                self.first_blocker(kids, from, to)?.is_some()
            });
            if at_edge && !stops {
                child = a;
                continue;
            }
            held = Some(a);
            break;
        }
        let Some(held) = held else {
            return Ok(1.0);
        };
        let share = self.share_from(held)?;
        Ok(py_max(&[share, 0.05]))
    }

    /// Whether `a`'s border or padding on that side keeps a margin inside it
    /// (`_box_share`'s `stops`).
    fn stops(&mut self, a: NodeId, top: bool) -> Result<bool, ItemError> {
        if let Some(&known) = self.noted.stops.get(&(a, top)) {
            return Ok(known);
        }
        let side = if top { "top" } else { "bottom" };
        let css = self.resolve(a)?;
        let drawn = css_border(css.as_ref())?.is_some_and(|b| b.iter().any(|s| s.side == side));
        let stops = drawn
            || match padding_em(py::strip(&side_padding(css.as_ref(), side)?), 1.0)? {
                Some(Pad::Pct(_)) => true,
                Some(Pad::Em(v)) => truthy(v),
                None => false,
            };
        self.noted.stops.insert((a, top), stops);
        Ok(stops)
    }

    /// Whether a margin cannot collapse past `k`, a child of the block it is in: `k` shows
    /// something (and is not `display: none`), or its tail holds words.
    fn blocks_margin(&mut self, k: NodeId) -> Result<bool, ItemError> {
        if self.has_content(k) {
            let css = self.resolve(k)?;
            let display = match &css {
                Some(css) => text_of(css.get("display")?).into_owned(),
                None => String::new(),
            };
            if py::strip(&display) != "none" {
                return Ok(true);
            }
        }
        Ok(has_words(self.dom.tail(k)))
    }

    /// The first of `kids[from..to]` that keeps a margin from collapsing past it, the
    /// children looked at in order, as `any` does over them.
    fn first_blocker(
        &mut self,
        kids: &[NodeId],
        from: usize,
        to: usize,
    ) -> Result<Option<usize>, ItemError> {
        let mut j = self.next_maybe(kids, from);
        while j < to.min(kids.len()) {
            let k = kids[j];
            let blocks = match self.noted.blocks_margin[k.index()] {
                Some(known) => known,
                None => {
                    let found = self.blocks_margin(k)?;
                    self.noted.blocks_margin[k.index()] = Some(found);
                    found
                }
            };
            if blocks {
                return Ok(Some(j));
            }
            self.noted.next_maybe[k.index()] = j + 1;
            j = self.next_maybe(kids, j + 1);
        }
        Ok(None)
    }

    /// The first index from `from` whose child is not known to let a margin past: the
    /// children known to are skipped, and the skips shortened for the next call.
    fn next_maybe(&mut self, kids: &[NodeId], from: usize) -> usize {
        let next = &mut self.noted.next_maybe;
        let mut j = from;
        while j < kids.len() && next[kids[j].index()] != 0 {
            j = next[kids[j].index()];
        }
        let mut i = from;
        while i < j {
            let after = next[kids[i].index()];
            next[kids[i].index()] = j;
            i = after;
        }
        j
    }

    /// `_has_content(n)`: text in it, or a picture, rule or table; false for a comment. Each
    /// node is worked out once, children first, without recursion.
    fn has_content(&mut self, n: NodeId) -> bool {
        let dom = self.dom;
        if dom.is_comment(n) {
            return false;
        }
        let memo = &mut self.noted.has_content;
        let mut stack = vec![(n, false)];
        while let Some((m, expanded)) = stack.pop() {
            if memo[m.index()].is_some() {
                continue;
            }
            let kids = dom.children(m);
            if !expanded {
                if has_words(dom.text(m))
                    || matches!(dom.tag(m), "img" | "svg" | "hr" | "table")
                    || kids.iter().any(|&c| has_words(dom.tail(c)))
                {
                    memo[m.index()] = Some(true);
                    continue;
                }
                stack.push((m, true));
                for &c in kids {
                    if !dom.is_comment(c) && memo[c.index()].is_none() {
                        stack.push((c, false));
                    }
                }
            } else {
                let any = kids
                    .iter()
                    .any(|&c| !dom.is_comment(c) && memo[c.index()] == Some(true));
                memo[m.index()] = Some(any);
            }
        }
        memo[n.index()] == Some(true)
    }

    /// The share `_box_share`'s second loop gives once it has applied every block from the
    /// outermost below the body down to `held`. Each block's step depends only on the share
    /// coming in, so each is worked out once per file, outermost first as kfxgen does.
    fn share_from(&mut self, held: NodeId) -> Result<f64, ItemError> {
        let dom = self.dom;
        let mut todo = Vec::new();
        let mut share = 1.0;
        let mut n = held;
        loop {
            if let Some(&known) = self.noted.shares.get(&n) {
                share = known;
                break;
            }
            todo.push(n);
            match dom.parent(n) {
                Some(p) if !matches!(dom.tag(p), "body" | "html") => n = p,
                _ => break,
            }
        }
        for a in todo.into_iter().rev() {
            share = self.share_step(a, share)?;
            self.noted.shares.insert(a, share);
        }
        Ok(share)
    }

    /// One block of `_box_share`'s second loop.
    fn share_step(&self, a: NodeId, mut share: f64) -> Result<f64, ItemError> {
        let pp = page_pt();
        let css = self.resolve(a)?;
        let width = match &css {
            Some(css) => text_of(css.get("width")?).into_owned(),
            None => String::new(),
        };
        if let Some((mag, unit)) = parse_css_length(&width, false)? {
            let w = py::py_float(&mag).ok_or(NOT_A_NUMBER)?;
            if unit == "$314" && 0.0 < w && w < 100.0 {
                return Ok(share * (w / 100.0));
            }
            // `_CSS_PX`.
            let px = match unit {
                "$319" => Some(1.0),
                "$308" | "$505" => Some(16.0),
                "$318" => Some(4.0 / 3.0),
                "$316" => Some(96.0 / 25.4),
                _ => None,
            };
            if let Some(px) = px {
                return Ok(w * px * 0.45 / pp);
            }
        }
        let border = css_border(css.as_ref())?.unwrap_or_default();
        let pt = |side: &str| border.iter().find(|b| b.side == side).map_or(0.0, |b| b.pt);
        // `sum(…)` starts from the int 0.
        share -= 0.0 + pt("left") / pp + pt("right") / pp;
        let outer = share;
        for side in ["left", "right"] {
            share -= match padding_em(py::strip(&side_padding(css.as_ref(), side)?), 1.0)? {
                Some(Pad::Pct(n)) => n / 100.0 * outer,
                Some(Pad::Em(v)) if truthy(v) => v / (pp / 12.0),
                _ => 0.0 / (pp / 12.0),
            };
        }
        Ok(share)
    }
}

/// `str(dict(zip(_BORDER_SIDES, css.get("padding-sides") or ())).get(side) or "")`.
fn side_padding(css: Option<&Resolved>, side: &str) -> Result<String, ItemError> {
    let Some(css) = css else {
        return Ok(String::new());
    };
    let sides = match css.get("padding-sides")? {
        Some(PyValue::Tuple(v) | PyValue::List(v)) => v,
        _ => return Ok(String::new()),
    };
    Ok(BORDER_SIDES
        .iter()
        .zip(sides.iter())
        .find(|(s, _)| **s == side)
        .map(|(_, v)| text_of(Some(v)).into_owned())
        .unwrap_or_default())
}

/// How a gap above a paragraph came about (`_space_between`'s `how`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum How {
    Spacer,
    Margin,
}

/// One `_space_between` call part-way through its blocks.
struct Run {
    blocks: std::vec::IntoIter<Block>,
    out: Vec<Block>,
    /// (index in `out`, gap, how): `how` None beside a heading.
    gaps: Vec<(usize, f64, Option<How>)>,
    above: f64,
    lines: f64,
    fixed: f64,
    held: f64,
    on_purpose: bool,
}

impl Run {
    fn new(blocks: Vec<Block>) -> Run {
        Run {
            out: Vec::with_capacity(blocks.len()),
            blocks: blocks.into_iter(),
            gaps: Vec::new(),
            above: 0.0,
            lines: 0.0,
            fixed: 0.0,
            held: 0.0,
            on_purpose: false,
        }
    }
}

/// A box whose blocks are being spaced: the run it stands in, and its margin-bottom there.
struct OpenBox {
    run: Run,
    bx: Block,
    bottom: f64,
}

/// `_space_between(blocks, margins, measured, paddings)`: `blocks` with the space blocks
/// taken out and each block's space above and below written (`margin_above`,
/// `margin_below`, `padding_below`, `trailing_space`), a run of short lines marked `tight`
/// and a scene break's gap written as `space_above`. A box's blocks are spaced the same way,
/// inside it, and what leaves its last one meets the box's margin-bottom.
///
/// kfxgen calls itself for each box; here each box being spaced is an [`OpenBox`] on a stack,
/// one per box nested, and boxes nest no deeper than the elements that make them.
///
/// Linear in the blocks: one pass builds each new list, a second marks tight runs, a third
/// writes the breaks; the gaps are counted in a hash map.
pub(super) fn space_between(
    href: &str,
    blocks: Vec<Block>,
    margins: &mut HashMap<BlockId, [f64; 2]>,
    measured: bool,
    paddings: &mut HashMap<BlockId, [f64; 4]>,
) -> Result<Vec<Block>, ItemError> {
    let mut open: Vec<OpenBox> = Vec::new();
    let mut run = Run::new(blocks);
    loop {
        let Some(mut b) = run.blocks.next() else {
            let out = finish_run(run, margins, measured, paddings);
            let Some(OpenBox {
                run: outer,
                mut bx,
                bottom,
            }) = open.pop()
            else {
                return Ok(out);
            };
            run = outer;
            let bottom = box_tail(href, &mut bx, out, margins, bottom)?;
            next_block(&mut run, bx, None, bottom, Some("box"), paddings);
            continue;
        };
        let kind = b.kind().map(str::to_owned);
        if kind.as_deref() == Some("space") {
            let margin = number(b.get("margin"));
            if let Some(prev) = run.out.last()
                && measured
                && !run.on_purpose
                && let Some(prev_pads) = paddings.get_mut(&prev.id())
                && truthy(prev_pads[1])
            {
                // After a padded block the spacer meets the margin outside its padding.
                run.held = run.above + prev_pads[1];
                run.above = prev_pads[3];
                prev_pads[3] = 0.0;
                if let Some(ratio) = true_number(prev.get("outer_em_ratio"))
                    && truthy(margin)
                {
                    run.above /= ratio;
                }
            }
            if truthy(margin)
                && !run.on_purpose
                && !truthy(run.held)
                && let Some(prev) = run.out.last()
                && prev.get("bottom_in_em").is_some_and(PyValue::is_truthy)
                && prev.get("own_bottom_em").is_some_and(PyValue::is_truthy)
            {
                // The margin waiting above, in em or rem, read in the block's own size.
                run.above /= ems_own(prev);
            }
            let height = number(b.get("height"));
            if truthy(height) {
                run.fixed += collapse(run.above, margin) + height;
                run.above = number(b.get("margin_after"));
            } else {
                run.above = py_max(&[run.above, margin]);
            }
            run.lines += number(b.get("line"));
            run.on_purpose = true;
            continue;
        }
        let [top, bottom] = margins.get(&b.id()).copied().unwrap_or([0.0; 2]);
        if measured {
            let up_pads = run
                .out
                .last()
                .and_then(|p| paddings.get(&p.id()).copied())
                .unwrap_or([0.0; 4]);
            let own_pads = paddings.get(&b.id()).copied().unwrap_or([0.0; 4]);
            place_margin(
                run.out.last_mut(),
                &mut b,
                Placing {
                    above: run.above,
                    top,
                    lines: run.lines,
                    on_purpose: run.on_purpose,
                    fixed: run.fixed + run.held,
                    pad_up: if truthy(run.held) { 0.0 } else { up_pads[1] },
                    pad_own: own_pads[0],
                    outer_up: if truthy(run.held) { 0.0 } else { up_pads[3] },
                    outer_own: own_pads[2],
                },
            );
            if kind.as_deref() == Some("box") {
                // Its paragraphs are spaced the same way, inside it (#640).
                let kids = b.take_blocks();
                let outer = std::mem::replace(&mut run, Run::new(kids));
                open.push(OpenBox {
                    run: outer,
                    bx: b,
                    bottom,
                });
                continue;
            }
        }
        next_block(&mut run, b, Some(top), bottom, kind.as_deref(), paddings);
    }
}

/// The rest of `_space_between`'s loop for a block that is not a space: a paragraph's gap
/// noted (`top` its margin-top), and its margin-bottom (`bottom`) left waiting.
fn next_block(
    run: &mut Run,
    b: Block,
    top: Option<f64>,
    bottom: f64,
    kind: Option<&str>,
    paddings: &HashMap<BlockId, [f64; 4]>,
) {
    if kind.is_none()
        && let Some(prev) = run.out.last()
    {
        let beside = b.get("heading").is_some_and(PyValue::is_truthy)
            || prev.get("heading").is_some_and(PyValue::is_truthy);
        let mut how = if beside {
            None
        } else if run.on_purpose {
            Some(How::Spacer)
        } else {
            Some(How::Margin)
        };
        if prev.kind().is_some() && how != Some(How::Spacer) {
            // After a table, a box or a rule: a margin there is theirs to space.
            how = None;
        }
        let padding = paddings.get(&prev.id()).map_or(0.0, |p| p[1])
            + paddings.get(&b.id()).map_or(0.0, |p| p[0]);
        let gap = py::round(
            collapse(run.above, top.unwrap_or(0.0)) + run.fixed + run.lines + padding,
            4,
        );
        run.gaps.push((run.out.len(), gap, how));
    }
    // A box's margin-bottom waits for the next block, as a paragraph's.
    run.above = if matches!(kind, None | Some("box")) {
        bottom
    } else {
        0.0
    };
    run.lines = 0.0;
    run.fixed = 0.0;
    run.held = 0.0;
    run.on_purpose = false;
    run.out.push(b);
}

/// What leaves a box once its blocks are spaced (`out`): spacers after its last block, or
/// the space below a last block with words when nothing marks the box's bottom edge, go in
/// its padding (with an edge) or meet its margin-bottom. Returns that margin-bottom.
fn box_tail(
    href: &str,
    bx: &mut Block,
    mut out: Vec<Block>,
    margins: &mut HashMap<BlockId, [f64; 2]>,
    mut bottom: f64,
) -> Result<f64, ItemError> {
    let look = bx.get("box");
    let side = |key: &str| look.and_then(|d| d.get(key)).filter(|v| v.is_truthy());
    if look
        .and_then(|d| d.get("vertical"))
        .is_some_and(PyValue::is_truthy)
    {
        // A vertical block's blocks end at its left: Task 15.
        return Err(ItemError::Error(Error::Unsupported {
            feature: "boxes (Task 15)",
            file: href.to_owned(),
        }));
    }
    let edge = side("border")
        .and_then(|d| d.get("bottom"))
        .filter(|v| v.is_truthy())
        .or_else(|| side("padding").and_then(|d| d.get("bottom")))
        .is_some_and(PyValue::is_truthy);
    let mut tail = 0.0;
    if let Some(last) = out.last_mut() {
        tail = match last.remove("trailing_space") {
            Some(t) => number(Some(&t)),
            None => 0.0,
        };
        let keep = match last.get("padding_below") {
            Some(v) => number(Some(v)),
            None => 0.0,
        };
        let below = true_number(last.get("margin_below")).unwrap_or(0.0);
        if !edge && below > keep && has_real_text(text_field(last)) {
            // Nothing at its bottom edge: the space below its last block leaves the box.
            tail = py_max(&[tail, (below - keep) * ems_own(last)]);
            last.set("margin_below", PyValue::Float(keep));
        }
    }
    if truthy(tail) && edge {
        // Spacers after a table that ends the box: below it, inside the box, in its padding.
        let own = ems_own(bx);
        if let Some(PyValue::Dict(d)) = bx.get_mut("box") {
            let mut pad = match d.iter().find(|(k, _)| k.as_str() == Some("padding")) {
                Some((_, PyValue::Dict(p))) if !p.is_empty() => p.clone(),
                _ => BORDER_SIDES
                    .iter()
                    .map(|side| (s(side), PyValue::Float(0.0)))
                    .collect(),
            };
            let was = pad
                .iter()
                .find(|(k, _)| k.as_str() == Some("bottom"))
                .map_or(0.0, |(_, v)| number(Some(v)));
            dict_set(&mut pad, "bottom", PyValue::Float(was + tail / own));
            dict_set(d, "padding", PyValue::Dict(pad));
        }
    } else if truthy(tail) {
        // It meets the box's margin-bottom, where the run around the box meets it.
        let mine = margins.entry(bx.id()).or_insert([0.0; 2]);
        mine[1] = collapse(mine[1], tail);
        bottom = mine[1];
    }
    bx.put_blocks(out);
    Ok(bottom)
}

/// The end of a `_space_between` call: the last block's space below, then the gaps.
fn finish_run(
    run: Run,
    margins: &HashMap<BlockId, [f64; 2]>,
    measured: bool,
    paddings: &HashMap<BlockId, [f64; 4]>,
) -> Vec<Block> {
    let Run {
        mut out,
        gaps,
        above,
        lines,
        fixed,
        on_purpose,
        ..
    } = run;
    if measured
        && on_purpose
        && let Some(last) = out.last_mut()
        && !takes_margin(last)
    {
        // Spacers after a table or a picture that ends the run: their space, for a box.
        last.set("trailing_space", PyValue::Float(fixed + above + lines));
    }
    if measured
        && let Some(last) = out.last_mut()
        && takes_margin(last)
    {
        // The last block keeps its margin-bottom, met by the spacers after it.
        let last_pads = paddings.get(&last.id()).copied().unwrap_or([0.0; 4]);
        let own = ems_own(last);
        let below = if on_purpose {
            (fixed + above) / own + lines
        } else {
            margins.get(&last.id()).map_or(0.0, |m| m[1]) / own
        };
        let mine = true_number(last.get("margin_below")).unwrap_or(0.0);
        last.set(
            "margin_below",
            PyValue::Float(py_max(&[mine, below]) + (last_pads[1] + last_pads[3]) / own),
        );
        last.set("padding_below", PyValue::Float(last_pads[1] / own));
    }
    if !gaps.is_empty() {
        let usual = usual_gap(&gaps);
        let gap_of: HashMap<usize, f64> = gaps.iter().map(|&(i, g, _)| (i, g)).collect();
        if measured {
            mark_tight_runs(&mut out, &gap_of);
        }
        // `ordinary(i)`: no more than the usual gap above `out[i]`, or nothing there.
        let ordinary = |i: Option<usize>| match i.filter(|&i| i < out.len()) {
            None => true,
            Some(i) => gap_of.get(&i).copied().unwrap_or(usual) <= usual + 0.01,
        };
        let mut breaks = Vec::new();
        for &(i, gap, how) in &gaps {
            let Some(how) = how else {
                continue;
            };
            if gap <= usual + 0.01 {
                continue;
            }
            let following = out.get(i + 1);
            if how == How::Margin
                && !(gap >= MARGIN_BREAK_EM
                    && gap >= 2.0 * usual
                    && ordinary(i.checked_sub(1))
                    && ordinary(Some(i + 1))
                    && following.is_none_or(|f| {
                        f.kind().is_none() && !f.get("heading").is_some_and(PyValue::is_truthy)
                    }))
            {
                continue;
            }
            breaks.push((i, gap));
        }
        for (i, gap) in breaks {
            out[i].set("space_above", PyValue::Float(gap));
        }
    }
    out
}

/// `max(Counter(gaps), key=lambda g: (counts[g], -g))`: the most common gap, of two as
/// common the smaller, the first counted when neither is greater. The counter's keys are
/// Python's: `0.0` and `-0.0` are one key (the first seen kept), and each NaN its own.
fn usual_gap(gaps: &[(usize, f64, Option<How>)]) -> f64 {
    let mut counts: Vec<(f64, usize)> = Vec::new();
    let mut at: HashMap<u64, usize> = HashMap::new();
    for &(_, g, _) in gaps {
        if g.is_nan() {
            counts.push((g, 1));
            continue;
        }
        let key = if g == 0.0 { 0 } else { g.to_bits() };
        match at.get(&key) {
            Some(&i) => counts[i].1 += 1,
            None => {
                at.insert(key, counts.len());
                counts.push((g, 1));
            }
        }
    }
    let mut best = counts[0];
    for &(g, n) in &counts[1..] {
        // The tuples compare by count, then by `-g` (false when either is NaN).
        if n > best.1 || (n == best.1 && -g > -best.0) {
            best = (g, n);
        }
    }
    best.0
}

/// `_mark_tight_runs(blocks, gap_of)`: in each run of unindented paragraphs with no gap
/// between them, nine in ten of them short lines, every line but the first is "tight".
fn mark_tight_runs(out: &mut [Block], gap_of: &HashMap<usize, f64>) {
    let line = |b: &Block| {
        let style = b.get("block_style");
        b.kind().is_none()
            && !b.get("heading").is_some_and(PyValue::is_truthy)
            && !b.get("row").is_some_and(PyValue::is_truthy)
            && !style
                .and_then(|st| st.get("indent"))
                .is_some_and(PyValue::is_truthy)
    };
    let mut run: Vec<usize> = Vec::new();
    for i in 0..=out.len() {
        let is_line = out.get(i).is_some_and(line);
        if is_line && !run.is_empty() && gap_of.get(&i) == Some(&0.0) {
            run.push(i);
            continue;
        }
        let short = run
            .iter()
            .filter(|&&r| py::len(text_field(&out[r])) <= SHORT_LINE)
            .count();
        if !run.is_empty() && short as f64 >= SHORT_SHARE * run.len() as f64 {
            for &r in &run[1..] {
                out[r].set("tight", PyValue::Bool(true));
            }
        }
        run.clear();
        if is_line {
            run.push(i);
        }
    }
}

/// `_place_margin`'s arguments past `prev` and `b`.
struct Placing {
    above: f64,
    top: f64,
    lines: f64,
    on_purpose: bool,
    fixed: f64,
    pad_up: f64,
    pad_own: f64,
    outer_up: f64,
    outer_own: f64,
}

/// `_place_margin(prev, b, …)`: the gap above `b`, the larger of the margin waiting above and
/// `b`'s own plus the spacers' lines, on `b` when it has a margin-top of its own, else kept
/// below `prev`.
fn place_margin(prev: Option<&mut Block>, b: &mut Block, p: Placing) {
    let Placing {
        above,
        top,
        lines,
        on_purpose,
        fixed,
        pad_up,
        pad_own,
        outer_up,
        outer_own,
    } = p;
    let float = PyValue::Float;
    let Some(prev) = prev else {
        if takes_margin(b) {
            let space = if truthy(pad_own) {
                collapse(above, outer_own) + pad_own + top
            } else {
                outer_own + collapse(above, top)
            };
            let v = (space + fixed) / ems_own(b) + lines;
            b.set("margin_above", float(v));
        }
        return;
    };
    let (up, own) = (above, top);
    if spaces_itself(b) {
        // The waiting margin meets the rule's own, in its `space_above` (Task 14's rule).
        if let Some(PyValue::Dict(rule)) = b.get_mut("rule") {
            let current = number(
                rule.iter()
                    .find(|(k, _)| k.as_str() == Some("space_above"))
                    .map(|(_, v)| v),
            );
            let gap = if truthy(pad_up) {
                collapse(outer_up, current) + up + pad_up
            } else {
                collapse(up, current)
            };
            dict_set(rule, "space_above", float(gap + fixed));
        }
        return;
    }
    if truthy(pad_up) || truthy(pad_own) {
        let outside = collapse(
            if truthy(pad_up) { outer_up } else { up },
            if truthy(pad_own) { outer_own } else { own },
        );
        let inside_up = if truthy(pad_up) { up + pad_up } else { 0.0 };
        let inside_own = if truthy(pad_own) { pad_own + own } else { 0.0 };
        let gap = outside + inside_up + inside_own + fixed;
        if takes_margin(b)
            && (truthy(pad_own) || truthy(top) || truthy(outer_own) || !takes_margin(prev))
        {
            let v = gap / ems_own(b) + lines;
            b.set("margin_above", float(v));
        } else if takes_margin(prev) && !spaces_itself(b) {
            let v = gap / ems_own(prev);
            prev.set("margin_below", float(v));
            if takes_margin(b) {
                b.set("margin_above", float(lines));
            }
        }
        return;
    }
    let gap = collapse(up, own) + fixed;
    if top != 0.0 || on_purpose || !takes_margin(prev) {
        if takes_margin(b) {
            let v = (gap + pad_up + pad_own) / ems_own(b) + lines;
            b.set("margin_above", float(v));
        }
    } else {
        if !spaces_itself(b) {
            let v = (above + pad_up) / ems_own(prev);
            prev.set("margin_below", float(v));
        }
        if takes_margin(b) {
            let v = pad_own / ems_own(b);
            b.set("margin_above", float(v));
        }
    }
    if !takes_margin(b) && !spaces_itself(b) && takes_margin(prev) && (up > own || truthy(fixed)) {
        // Before a table: the block above keeps it.
        let v = (above + fixed) / ems_own(prev);
        prev.set("margin_below", float(v));
    }
}

/// Expected values from the pinned kfxgen (`converter.py` at `ad38ec8`), its functions run in
/// plain Python on the same inputs, page width 384pt unless named.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::resolver::TextBudget;
    use crate::generate::page_scale::page_width;

    /// A float as Python's `repr` writes it.
    fn show(x: f64) -> String {
        py::repr_float(x)
    }

    fn raised(r: Result<impl std::fmt::Debug, Raised>) -> &'static str {
        match r {
            Err(e) => e.exception,
            Ok(v) => panic!("no raise: {v:?}"),
        }
    }

    #[test]
    fn padding_em_reads_lengths_as_kfxgen() {
        let em = |v: &str, ratio: f64| match padding_em(v, ratio).unwrap() {
            Some(Pad::Em(x)) => show(x),
            Some(Pad::Pct(n)) => format!("%{}", show(n)),
            None => "None".to_owned(),
        };
        assert_eq!(em("1.5em", 2.0), "3.0");
        assert_eq!(em("12px", 1.0), "0.44999999999999996");
        assert_eq!(em("3pt", 1.0), "0.25");
        assert_eq!(em("2rem", 2.0), "2.0");
        assert_eq!(em("10%", 1.0), "%10.0");
        assert_eq!(em("0", 1.0), "0.0");
        assert_eq!(em("5", 1.0), "None");
        assert_eq!(em(" 1.2 em ", 1.0), "1.2");
        assert_eq!(em("\u{661}\u{662}px", 1.0), "0.44999999999999996");
        assert_eq!(em("-1px", 1.0), "None");
        assert_eq!(em("1PX", 1.0), "None");
        assert_eq!(em("1px\u{1f}", 1.0), "0.0375");
        assert_eq!(em("1 2px", 1.0), "None");
        assert_eq!(em(".5em", 3.0), "1.5");
        assert_eq!(em(&format!("{}px", "9".repeat(400)), 1.0), "inf");
        assert_eq!(raised(padding_em(".", 1.0)), "ValueError");
        assert_eq!(raised(padding_em("1.2.3px", 1.0)), "ValueError");
    }

    #[test]
    fn margin_em_keeps_the_sign_python_gives() {
        let budget = TextBudget::new();
        let css = |pairs: &[(&'static str, PyValue)]| Resolved::of(pairs.to_vec(), &budget);
        let m = |css: Option<&Resolved>, side: &str, share: f64| {
            let x = margin_em(css, side, share).unwrap();
            format!("{} {:#x}", show(x), x.to_bits())
        };
        let f = PyValue::Float;
        let neg_zero = format!("-0.0 {:#x}", (-0.0f64).to_bits());
        let c = css(&[("margin-top", s("-0")), ("font-size", f(16.0))]);
        assert_eq!(m(Some(&c), "top", 1.0), neg_zero);
        let c = css(&[("margin-top", s("-0px"))]);
        assert_eq!(m(Some(&c), "top", 1.0), neg_zero);
        let c = css(&[("margin-bottom", s("-5%"))]);
        assert!(m(Some(&c), "bottom", 0.5).starts_with("-0.8 "));
        let c = css(&[("margin-bottom", s("--2em")), ("font-size", f(32.0))]);
        assert!(m(Some(&c), "bottom", 1.0).starts_with("-4.0 "));
        let c = css(&[("margin-top", s("1.5em")), ("font-size", f(13.28))]);
        assert!(m(Some(&c), "top", 1.0).starts_with("1.2449999999999999 "));
        let c = css(&[("margin-top", s("12%"))]);
        assert!(m(Some(&c), "top", 0.37).starts_with("1.4207999999999998 "));
        assert!(m(None, "top", 1.0).starts_with("0.0 "));
    }

    #[test]
    fn collapse_keeps_the_first_of_equal_arguments() {
        let c = |a: f64, b: f64| {
            let x = collapse(a, b);
            format!("{} {}", show(x), x.is_sign_negative())
        };
        assert_eq!(c(-0.0, -0.0), "0.0 false");
        assert_eq!(c(0.0, -0.0), "0.0 false");
        assert_eq!(c(f64::NAN, 1.0), "1.0 false");
        assert_eq!(show(collapse(f64::INFINITY, f64::NEG_INFINITY)), "nan");
        assert_eq!(c(-0.0, 0.5), "0.5 false");
        assert_eq!(c(1.0, -2.0), "-1.0 true");
        assert_eq!(c(-1.0, -3.0), "-3.0 true");
        // `max(above, margin)` with the waiting margin first: its -0.0 survives.
        assert!(py_max(&[-0.0, 0.0]).is_sign_negative());
        assert!(py_max(&[f64::NAN, 1.0]).is_nan());
    }

    #[test]
    fn spacers_are_spaced_as_kfxgen_spaces_them() {
        let budget = TextBudget::new();
        let f = PyValue::Float;
        let spacer = |pairs: Vec<(&'static str, PyValue)>| {
            let css = Resolved::of(pairs, &budget);
            match spacer_em(&css) {
                Ok((a, b, c)) => format!("({}, {}, {})", show(a), show(b), show(c)),
                Err(ItemError::Raised(r)) => r.exception.to_owned(),
                Err(e) => panic!("{e:?}"),
            }
        };
        let base = |size: f64, line: PyValue, declared: PyValue| {
            vec![
                ("font-size", f(size)),
                ("line-height", line),
                ("line-height-declared", declared),
            ]
        };
        let n = PyValue::None;
        assert_eq!(spacer(base(16.0, f(1.2), n.clone())), "(0.0, 0.7125, 0.0)");
        assert_eq!(
            spacer(base(16.0, f(1.2), s("normal"))),
            "(0.0, 0.7875, 0.0)"
        );
        assert_eq!(spacer(base(20.0, f(1.5), s("1.5"))), "(0.0, 1.125, 0.0)");
        assert_eq!(spacer(base(32.0, f(1.2), s("1.2"))), "(0.0, 1.425, 0.0)");
        let mut c = base(16.0, f(2.5), n.clone());
        c.extend([("margin-top", s("1em")), ("margin-bottom", s("6px"))]);
        assert_eq!(spacer(c), "(0.6, 1.5, 0.22499999999999998)");
        assert_eq!(
            spacer(base(10.0, f(2.0), s(" Normal "))),
            "(0.0, 0.4875, 0.0)"
        );
        assert_eq!(spacer(base(f64::NAN, f(1.2), s("normal"))), "ValueError");
        assert_eq!(
            spacer(base(f64::INFINITY, f(1.5), s("1.5"))),
            "OverflowError"
        );
        assert_eq!(spacer(base(16.0, f(f64::NAN), s("x"))), "ValueError");
        let mut c = base(16.0, n.clone(), n.clone());
        c.extend([("margin-top", s("-2pt")), ("margin-bottom", s("50%"))]);
        assert_eq!(spacer(c), "(-0.09999999999999999, 0.7125, 22.2)");
        // A line that rounds to 0px: no height, the margins collapse.
        assert_eq!(spacer(base(16.0, f(0.0001), s("1px"))), "(0.0, 0.0, 0.0)");
        // A number with no unit is 0 before `float()` could refuse it.
        let mut c = base(16.0, f(1.2), n);
        c.push(("margin-top", s(".")));
        assert_eq!(spacer(c), "(0.0, 0.7125, 0.0)");
    }

    #[test]
    fn carried_margins_are_added_as_shares_of_the_page() {
        let len = |m: &str, u: &str| PyValue::Tuple(vec![s(m), s(u)]);
        let add = |own: PyValue, extra: PyValue| match add_margin(&own, &extra).unwrap() {
            PyValue::Tuple(v) => format!("{}, {}", v[0].as_str().unwrap(), v[1].as_str().unwrap()),
            other => format!("{other:?}"),
        };
        let n = PyValue::None;
        assert_eq!(add(len("2", "$308"), len("5", "$314")), "11.25, $314");
        assert_eq!(add(n.clone(), len("1", "$316")), "0.7382, $314");
        assert_eq!(add(len("1.5", "$319"), n.clone()), "1.5, $319");
        assert_eq!(
            add(len("3", "$318"), len("0.33333", "$314")),
            "1.1146, $314"
        );
        assert_eq!(add(n, len("1e309", "$314")), "inf, $314");
        assert_eq!(add(len("-2", "$308"), len("6.25", "$314")), "0, $314");
        let _page = page_width(480.0);
        assert_eq!(add(len("2", "$308"), len("1", "$316")), "5.5906, $314");
    }

    #[test]
    fn the_usual_gap_is_counted_as_python_counts_it() {
        let gaps = |g: &[f64]| g.iter().map(|&g| (0, g, None)).collect::<Vec<_>>();
        assert_eq!(usual_gap(&gaps(&[2.0, 1.0, 3.0, 1.0, 2.0])), 1.0);
        assert!(usual_gap(&gaps(&[f64::NAN, 5.0, 1.0])).is_nan());
        let zero = usual_gap(&gaps(&[-0.0, 0.0, 1.0]));
        assert!(zero == 0.0 && zero.is_sign_negative());
    }

    /// `_space_between` on hand-made blocks.
    mod space_between {
        use super::*;

        const KEYS: [&str; 6] = [
            "margin_above",
            "margin_below",
            "padding_below",
            "space_above",
            "tight",
            "trailing_space",
        ];

        enum B {
            Para(&'static str, Vec<(&'static str, PyValue)>),
            Space(f64, f64, f64, f64),
            Table,
        }

        fn para(text: &'static str) -> B {
            B::Para(text, Vec::new())
        }

        fn style(pairs: Vec<(&str, PyValue)>) -> PyValue {
            PyValue::Dict(pairs.into_iter().map(|(k, v)| (s(k), v)).collect())
        }

        /// The spacing keys of `_space_between(blocks, margins, True, paddings)`'s blocks:
        /// `key=repr` joined by `,`, the blocks by `|`.
        fn run(
            blocks: Vec<B>,
            margins: &[(usize, [f64; 2])],
            paddings: &[(usize, [f64; 4])],
        ) -> String {
            let blocks: Vec<Block> = blocks
                .into_iter()
                .enumerate()
                .map(|(i, b)| {
                    let mut fields = vec![
                        ("text", Field::Py(s(""))),
                        ("spans", Field::Py(PyValue::List(Vec::new()))),
                        ("block_style", Field::Py(PyValue::None)),
                        ("anchor_ids", Field::Py(PyValue::List(Vec::new()))),
                        ("anchor_offsets", Field::Py(PyValue::Dict(Vec::new()))),
                    ];
                    let mut block = |extra: Vec<(&'static str, PyValue)>| {
                        for (k, v) in extra {
                            match fields.iter_mut().find(|(f, _)| *f == k) {
                                Some((_, slot)) => *slot = Field::Py(v),
                                None => fields.push((k, Field::Py(v))),
                            }
                        }
                    };
                    match b {
                        B::Para(text, extra) => {
                            block(vec![("text", s(text))]);
                            block(extra);
                        }
                        B::Space(margin, line, height, after) => block(vec![
                            ("type", s("space")),
                            ("margin", PyValue::Float(margin)),
                            ("line", PyValue::Float(line)),
                            ("height", PyValue::Float(height)),
                            ("margin_after", PyValue::Float(after)),
                        ]),
                        B::Table => block(vec![("type", s("table"))]),
                    }
                    Block {
                        id: BlockId(i),
                        fields,
                    }
                })
                .collect();
            let mut m = margins.iter().map(|&(i, v)| (BlockId(i), v)).collect();
            let mut p = paddings.iter().map(|&(i, v)| (BlockId(i), v)).collect();
            let out = space_between("t.xhtml", blocks, &mut m, true, &mut p).unwrap();
            out.iter()
                .map(|b| {
                    KEYS.iter()
                        .filter_map(|k| {
                            b.get(k).map(|v| match v {
                                PyValue::Float(x) => format!("{k}={}", show(*x)),
                                PyValue::Bool(true) => format!("{k}=True"),
                                other => format!("{k}={other:?}"),
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .collect::<Vec<_>>()
                .join("|")
        }

        #[test]
        fn a_spacer_with_a_height_is_a_break() {
            let got = run(
                vec![
                    para("a"),
                    B::Space(0.6, 0.0, 0.7125, 0.0),
                    para("b"),
                    para("c"),
                    para("d"),
                    para("e"),
                ],
                &[
                    (0, [0.0, 1.0]),
                    (2, [1.0, 1.0]),
                    (3, [1.0, 1.0]),
                    (4, [1.0, 1.0]),
                    (5, [1.0, 1.0]),
                ],
                &[],
            );
            assert_eq!(
                got,
                "margin_above=0.0|margin_above=2.7125,space_above=2.7125|margin_above=1.0|\
                 margin_above=1.0|margin_above=1.0,margin_below=1.0,padding_below=0.0"
            );
        }

        #[test]
        fn a_spacer_after_a_padded_block_meets_the_margin_outside_it() {
            let got = run(
                vec![
                    B::Para("a", vec![("outer_em_ratio", PyValue::Float(2.0))]),
                    B::Space(1.0, 1.2, 0.0, 0.0),
                    para("b"),
                    para("c"),
                ],
                &[(2, [0.5, 0.0])],
                &[(0, [0.0, 0.5, 0.0, 0.8])],
            );
            assert_eq!(
                got,
                "margin_above=0.0|margin_above=2.7,margin_below=0.0,space_above=2.7|\
                 margin_above=0.0,margin_below=0.0,padding_below=0.0,tight=True"
            );
        }

        #[test]
        fn a_margin_in_em_is_read_in_the_blocks_own_size_by_a_spacer() {
            let got = run(
                vec![
                    B::Para(
                        "a",
                        vec![
                            (
                                "block_style",
                                style(vec![("font_size", PyValue::Float(24.0))]),
                            ),
                            ("bottom_in_em", PyValue::Bool(true)),
                            ("own_bottom_em", PyValue::Bool(true)),
                        ],
                    ),
                    B::Space(0.5, 1.2, 0.0, 0.0),
                    para("b"),
                    para("c"),
                ],
                &[(0, [0.0, 1.5]), (2, [0.0, 0.0])],
                &[],
            );
            assert_eq!(
                got,
                "margin_above=0.0|margin_above=2.2,margin_below=0.0,space_above=2.2|\
                 margin_above=0.0,margin_below=0.0,padding_below=0.0,tight=True"
            );
        }

        #[test]
        fn spacers_that_end_the_run_stay_below_its_last_block() {
            let got = run(
                vec![para("a"), B::Table, B::Space(0.0, 1.2, 0.0, 0.0)],
                &[(0, [0.0, 1.0])],
                &[],
            );
            assert_eq!(got, "margin_above=0.0,margin_below=1.0|trailing_space=1.2");
            let got = run(
                vec![
                    para("a"),
                    para("b"),
                    B::Space(0.3, 1.2, 0.0, 0.0),
                    B::Space(0.6, 0.0, 0.7125, 0.2),
                ],
                &[(0, [0.0, 1.0]), (1, [1.0, 0.5])],
                &[(1, [0.0, 0.25, 0.0, 0.5])],
            );
            assert_eq!(
                got,
                "margin_above=0.0|margin_above=1.0,margin_below=2.9625,padding_below=0.25"
            );
        }

        #[test]
        fn short_lines_with_no_gap_are_tight() {
            let indent = style(vec![("indent", PyValue::Tuple(vec![s("2"), s("$308")]))]);
            let long: &'static str = Box::leak("x".repeat(61).into_boxed_str());
            let got = run(
                vec![
                    para("line one"),
                    para("line two"),
                    para("line three"),
                    para(long),
                    para("line five"),
                    B::Para("indented", vec![("block_style", indent)]),
                    para("seven"),
                    B::Para("eight", vec![("heading", PyValue::Bool(true))]),
                    para("nine"),
                    para("ten"),
                ],
                &(0..10).map(|i| (i, [0.0, 0.0])).collect::<Vec<_>>(),
                &[],
            );
            let zero = "margin_above=0.0,margin_below=0.0";
            let want = format!(
                "{}|margin_above=0.0,margin_below=0.0,padding_below=0.0,tight=True",
                [zero; 9].join("|")
            );
            assert_eq!(got, want);
        }

        #[test]
        fn a_gap_that_stands_out_is_a_break_but_not_beside_a_heading() {
            let h = |t| B::Para(t, vec![("heading", PyValue::Bool(true))]);
            let got = run(
                vec![
                    para("a"),
                    para("b"),
                    para("c"),
                    para("d"),
                    para("e"),
                    h("f"),
                    para("g"),
                    para("h"),
                    B::Table,
                    para("i"),
                ],
                &[
                    (0, [1.0, 1.0]),
                    (1, [1.0, 1.0]),
                    (2, [3.0, 1.0]),
                    (3, [1.0, 1.0]),
                    (4, [1.0, 1.0]),
                    (5, [4.0, 4.0]),
                    (6, [1.0, 1.0]),
                    (7, [1.0, 2.5]),
                    (9, [1.0, 1.0]),
                ],
                &[],
            );
            assert_eq!(
                got,
                "margin_above=1.0|margin_above=1.0|margin_above=3.0,space_above=3.0|\
                 margin_above=1.0|margin_above=1.0|margin_above=4.0|margin_above=4.0|\
                 margin_above=1.0,margin_below=2.5||margin_above=1.0,margin_below=1.0,\
                 padding_below=0.0"
            );
        }

        #[test]
        fn padding_keeps_the_margins_on_its_two_sides_apart() {
            let got = run(
                vec![
                    para("a"),
                    para("b"),
                    para("c"),
                    B::Para(
                        "d",
                        vec![(
                            "block_style",
                            style(vec![("font_size", PyValue::Float(8.0))]),
                        )],
                    ),
                ],
                &[
                    (0, [0.5, 0.5]),
                    (1, [0.0, 1.0]),
                    (2, [2.0, 0.0]),
                    (3, [0.25, 0.0]),
                ],
                &[(1, [0.75, 0.0, 1.5, 0.0]), (2, [0.0, 0.5, 0.0, 0.3])],
            );
            assert_eq!(
                got,
                "margin_above=0.5|margin_above=2.25|margin_above=2.0|\
                 margin_above=1.6,margin_below=0.0,padding_below=0.0"
            );
        }

        #[test]
        fn negative_and_signed_zero_margins() {
            let got = run(
                vec![para("a"), para("b"), para("c"), para("d")],
                &[
                    (0, [-0.0, -0.0]),
                    (1, [-1.0, -0.0]),
                    (2, [-0.0, 0.5]),
                    (3, [-2.0, -1.0]),
                ],
                &[],
            );
            assert_eq!(
                got,
                "margin_above=0.0|margin_above=-1.0,margin_below=0.0|margin_above=0.0,\
                 tight=True|margin_above=-1.5,margin_below=0.0,padding_below=0.0"
            );
        }

        #[test]
        fn infinite_margins_give_python_s_nan_and_inf() {
            let (inf, nan) = (f64::INFINITY, f64::NAN);
            let got = run(
                vec![
                    para("a"),
                    para("b"),
                    B::Space(nan, 1.2, 0.0, 0.0),
                    para("c"),
                    para("d"),
                ],
                &[
                    (0, [0.0, inf]),
                    (1, [-inf, 1.0]),
                    (3, [1.0, 1.0]),
                    (4, [inf, 0.0]),
                ],
                &[],
            );
            assert_eq!(
                got,
                "margin_above=0.0|margin_above=nan|margin_above=2.2,space_above=2.2|\
                 margin_above=inf,margin_below=0.0,padding_below=0.0"
            );
        }
    }
}
