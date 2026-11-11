// SPDX-License-Identifier: GPL-3.0-or-later
//! Boxes and rules: a bordered or filled block around paragraphs (#238) and an `<hr>` (#230),
//! read from the model, their chunks (`_emit_box_chunks` `native_generator.py:5946`,
//! `_rule_chunk` :1105) and `$157` styles (`build_box_style_157` :3853,
//! `build_rule_style_157` :3915).
use std::borrow::Borrow;

use deckle_kfx::ion::Value;

use super::block_styles::{keep_space, number, read_bidi};
use super::chapters::{Block, read_block};
use super::chunks::{Chunk, Marker, Node, emit_paragraph};
use super::images::{IMG_TOKEN, Images};
use super::inline;
use super::styles::{LINE_EM, g6};
use super::table_chunks::{emit_table_chunks, link_target};
use super::table_styles::{apply_border, border_style_symbol, padding_values, side_keys};
use super::tables::{self, Border, Padding, read_border, read_padding, read_sides};
use super::title::opens_with_title;
use super::{GenerateError, Generator, model_err, py, st, sym};
use crate::pyvalue::{PyValue, truthy};

use GenerateError::Unsupported;

/// A rule's border width: points, or ems kept in ems (QA on #724).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RuleWidth {
    Pt(f64),
    Em(f64),
}

/// A rule's border, as `_border_key` (:1986) keys it: `(side, (style, width, ARGB or None))`
/// in side order, never empty.
pub(crate) type RuleBorder = Vec<(String, (String, RuleWidth, Option<i64>))>;

fn read_rule_border(v: Option<&PyValue>, at: &str) -> Result<Option<RuleBorder>, GenerateError> {
    read_sides(
        v,
        at,
        "{side: (style, width or (n, \"em\"), colour)}",
        |spec| {
            let [kind, width, colour] = spec.as_list()? else {
                return None;
            };
            let width = match width {
                PyValue::Tuple(_) | PyValue::List(_) => match measure_of(width)? {
                    (n, unit) if unit == "em" => RuleWidth::Em(n),
                    _ => return None,
                },
                w => RuleWidth::Pt(number(w)?),
            };
            let colour = match colour {
                PyValue::None => None,
                c => Some(c.as_int()?),
            };
            Some((kind.as_str()?.to_owned(), width, colour))
        },
    )
}

/// A rule block's `rule` (#230): how `build_rule_style_157` draws it.
#[derive(Clone, PartialEq)]
pub(crate) struct RuleStyle {
    /// In ems.
    pub space_above: Option<f64>,
    pub space_below: Option<f64>,
    /// `(n, "%" or "em")`.
    pub width: Option<(f64, String)>,
    pub align: Option<String>,
    /// In % of the page.
    pub margin_left: Option<f64>,
    pub margin_right: Option<f64>,
    pub border: Option<RuleBorder>,
    pub fill: Option<i64>,
    /// `(n, "pt" or "em")`.
    pub height: Option<(f64, String)>,
}

fn opt<T>(
    d: &PyValue,
    key: &str,
    at: &str,
    expected: &str,
    read: impl FnOnce(&PyValue) -> Option<T>,
) -> Result<Option<T>, GenerateError> {
    match d.get(key) {
        None | Some(PyValue::None) => Ok(None),
        Some(v) => read(v)
            .map(Some)
            .ok_or_else(|| model_err(format!("{at}.{key}: {expected}"))),
    }
}

/// `(n, unit)`.
fn measure_of(v: &PyValue) -> Option<(f64, String)> {
    let [n, unit] = v.as_list()? else {
        return None;
    };
    Some((number(n)?, unit.as_str()?.to_owned()))
}

/// A rule block's `rule`.
pub(crate) fn read_rule(b: &PyValue, at: &str) -> Result<RuleStyle, GenerateError> {
    let at = format!("{at}.rule");
    let r = match b.get("rule") {
        Some(r @ PyValue::Dict(_)) => r,
        _ => return Err(model_err(format!("{at}: a dict"))),
    };
    let PyValue::Dict(pairs) = r else {
        unreachable!()
    };
    for (k, _) in pairs {
        match k.as_str() {
            Some(
                "space_above" | "space_below" | "width" | "align" | "margin_left" | "margin_right"
                | "border" | "fill" | "height",
            ) => {}
            Some("clear") => return Err(Unsupported("clearing floats")),
            _ => return Err(Unsupported("a rule's look")),
        }
    }
    // `rule["space_above"]`: kfxgen reads both as there.
    for key in ["space_above", "space_below"] {
        if r.get(key).is_none() {
            return Err(model_err(format!("{at}.{key}: a number or None")));
        }
    }
    let border = read_rule_border(r.get("border"), &format!("{at}.border"))?;
    Ok(RuleStyle {
        space_above: opt(r, "space_above", &at, "a number", number)?,
        space_below: opt(r, "space_below", &at, "a number", number)?,
        width: opt(r, "width", &at, "(n, unit)", measure_of)?,
        align: opt(r, "align", &at, "a string", |v| {
            v.as_str().map(str::to_owned)
        })?,
        margin_left: opt(r, "margin_left", &at, "a number", number)?,
        margin_right: opt(r, "margin_right", &at, "a number", number)?,
        border,
        fill: opt(r, "fill", &at, "an ARGB int", PyValue::as_int)?,
        height: opt(r, "height", &at, "(n, unit)", measure_of)?,
    })
}

/// A box block's `box`: what its `$157` is made from.
#[derive(Clone, PartialEq)]
pub(crate) struct BoxSpec {
    pub border: Option<Border>,
    pub background: Option<i64>,
    pub padding: Option<Padding>,
    /// Its own font size, in CSS px: its margins and padding are in its ems (#647).
    pub font_px: Option<f64>,
    /// In % of the page.
    pub margin_left: Option<f64>,
    pub margin_right: Option<f64>,
    /// `bidi`: None when absent; `Some(None)` for `{}`.
    pub bidi: Option<Option<String>>,
}

/// A box block (#238): its look and the blocks in it.
#[derive(Clone)]
pub(crate) struct BoxBlock {
    pub spec: BoxSpec,
    pub blocks: Vec<Block>,
    /// Opened before kfxgen's heading, which it draws round (#646).
    pub opened: bool,
}

/// A box block's `box` and `blocks`.
pub(crate) fn read_box(b: &PyValue, at: &str) -> Result<BoxBlock, GenerateError> {
    let at_box = format!("{at}.box");
    let d = match b.get("box") {
        Some(d @ PyValue::Dict(_)) => d,
        _ => return Err(model_err(format!("{at_box}: a dict"))),
    };
    // A sized, floated, centred or horizontal box is a wrapper Previewer writes otherwise
    // (#382, #247), not ported.
    for key in [
        "width",
        "width_px",
        "float",
        "centred",
        "horizontal",
        "float_wrapper",
    ] {
        if truthy(d.get(key)) {
            return Err(Unsupported("a sized, floated or centred box"));
        }
    }
    let spec = BoxSpec {
        border: read_border(d.get("border"), &format!("{at_box}.border"))?,
        background: opt(d, "background", &at_box, "an ARGB int", PyValue::as_int)?,
        padding: read_padding(d.get("padding"), &format!("{at_box}.padding"))?,
        font_px: opt(d, "font_px", &at_box, "a number", number)?,
        margin_left: opt(d, "margin_left", &at_box, "a number", number)?,
        margin_right: opt(d, "margin_right", &at_box, "a number", number)?,
        bidi: read_bidi(d.get("bidi"), &at_box)?,
    };
    let blocks = b
        .get("blocks")
        .and_then(PyValue::as_list)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| model_err(format!("{at}.blocks: a list of blocks")))?
        .iter()
        .enumerate()
        .map(|(j, kid)| read_block(kid, &format!("{at}.blocks[{j}]")))
        .collect::<Result<_, _>>()?;
    Ok(BoxBlock {
        spec,
        blocks,
        opened: false,
    })
}

/// A box's `open` (:5952): its look, and its margins spaced as a paragraph's, in its ems.
#[derive(Clone)]
pub(crate) struct BoxOpen {
    pub spec: BoxSpec,
    pub space_above: f64,
    pub space_below: f64,
}

impl BoxOpen {
    fn of(block: &Block, b: &BoxBlock) -> Self {
        BoxOpen {
            spec: b.spec.clone(),
            space_above: block.margin_above.flatten().unwrap_or(0.0),
            space_below: block.margin_below,
        }
    }
}

/// `_drop_edge_rules` (:1091): a chapter's blocks without the rules at its start or end.
pub(crate) fn drop_edge_rules<B: Borrow<Block>>(blocks: &[B]) -> &[B] {
    let start = blocks.iter().position(|b| b.borrow().rule.is_none());
    let end = blocks.iter().rposition(|b| b.borrow().rule.is_none());
    match (start, end) {
        (Some(s), Some(e)) => &blocks[s..=e],
        _ => &[],
    }
}

/// `_unboxed` (:1294): each box replaced by its blocks; `_leading_blocks` (:1134) too.
pub(crate) fn unboxed(blocks: &[Block]) -> Vec<&Block> {
    let mut out = Vec::new();
    for b in blocks {
        match &b.boxed {
            Some(bx) => out.extend(unboxed(&bx.blocks)),
            None => out.push(b),
        }
    }
    out
}

/// `_rule_chunk` (:1105).
pub(crate) fn rule_chunk(rule: &RuleStyle) -> Chunk {
    Chunk {
        rule: Some(rule.clone()),
        ..Chunk::text(String::new())
    }
}

/// `_emit_box_chunks` (:5946): the box's `open`, its blocks, a `close`. Its own ids go on its
/// first paragraph, a link target as the container isn't. Returns the ids it could not place.
pub(crate) fn emit_box_chunks(
    block: &Block,
    b: &BoxBlock,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    let inner: Vec<String> = b.blocks.iter().flat_map(block_keys).collect();
    let own: Vec<String> = block
        .anchors
        .keys
        .iter()
        .filter(|k| !inner.contains(k))
        .cloned()
        .collect();
    if !b.opened {
        chunks.push(Chunk::marker(Marker::Open(Node::Box(BoxOpen::of(
            block, b,
        )))));
    }
    let mut pending = own;
    for kid in &b.blocks {
        let first = chunks.len();
        // A table or a box inside is nested in it, as Kindle Previewer nests them (#310).
        let unplaced = if let Some(t) = &kid.table {
            emit_table_chunks(kid, t, chunks);
            Vec::new()
        } else if let Some(inner) = &kid.boxed {
            emit_box_chunks(kid, inner, images, chunks)?
        } else if kid.rule.is_some() {
            // `_emit_paragraph` reads the text a rule block has none of.
            return Err(Unsupported("a rule in a box"));
        } else {
            emit_paragraph(kid, false, images, chunks)?
        };
        if !pending.is_empty() && chunks[first..].iter().any(Chunk::is_content) {
            let at = link_target(chunks, first);
            chunks[at].anchors.attach(&pending);
            pending.clear();
        }
        pending.extend(unplaced);
    }
    chunks.push(Chunk::marker(Marker::Close));
    Ok(pending)
}

/// `_block_keys` (:1110): every anchor key a block holds, in a box or a table included.
pub(crate) fn block_keys(b: &Block) -> Vec<String> {
    let mut keys = super::tables::block_keys(b);
    if let Some(bx) = &b.boxed {
        for kid in &bx.blocks {
            keys.extend(block_keys(kid));
        }
    }
    keys
}

/// `build_box_style_157`'s arguments (:7520): the box's look, its padding and margins
/// scaled from its ems to the main size's.
#[derive(Clone, PartialEq)]
pub(crate) struct BoxStyle {
    pub border: Option<Border>,
    pub background: Option<i64>,
    pub padding: Option<Padding>,
    pub margin_left: Option<f64>,
    pub margin_right: Option<f64>,
    pub space_above: f64,
    pub space_below: f64,
}

impl BoxStyle {
    /// `own`: the box's font size against the main text's, `_rem(box["font_px"])`.
    pub(crate) fn of(open: &BoxOpen, own: f64) -> Self {
        use super::tables::Pad;
        let padding = open.spec.padding.clone().map(|p| {
            if own == 1.0 {
                return p;
            }
            p.into_iter()
                .map(|(side, v)| match v {
                    Pad::Em(em) => (side, Pad::Em(py::round(em * own, 6))),
                    pct => (side, pct),
                })
                .collect()
        });
        BoxStyle {
            border: open.spec.border.clone(),
            background: open.spec.background,
            padding,
            margin_left: open.spec.margin_left,
            margin_right: open.spec.margin_right,
            space_above: open.space_above * own,
            space_below: open.space_below * own,
        }
    }
}

/// `build_box_style_157` (:3853): a bordered or filled container as Kindle Previewer 4.0.1
/// writes it.
pub(crate) fn build_box_style_157(g: &mut Generator, name: &str, s: &BoxStyle) -> Value {
    g.local(name);
    let Value::Struct(mut v) = st([("$173", sym(name))]) else {
        unreachable!()
    };
    apply_border(&mut v, s.border.as_ref(), false);
    if let Some(fill) = s.background {
        v.insert("$70", Value::Int(fill));
    }
    if let Some(p) = &s.padding {
        let values =
            padding_values(p, None).expect("checked before it was allocated (BoxStyle::check)");
        for (key, value) in values {
            v.insert(key, value);
        }
    }
    for (key, pct) in [("$48", s.margin_left), ("$50", s.margin_right)] {
        if let Some(pct) = pct.filter(|&p| p != 0.0) {
            v.insert(key, g6(pct, "$314"));
        }
    }
    for (key, em) in [("$47", s.space_above), ("$49", s.space_below)] {
        if em != 0.0 {
            v.insert(key, g6(em / LINE_EM, "$310"));
        }
    }
    Value::Struct(v)
}

impl BoxStyle {
    /// Whether kfxgen can write its padding: `quantize` raises past 28 digits (:2071).
    pub(crate) fn check(&self) -> Result<(), GenerateError> {
        match &self.padding {
            Some(p) if padding_values(p, None).is_none() => Err(model_err(
                "a box's padding: a side kfxgen can write in 28 digits",
            )),
            _ => Ok(()),
        }
    }
}

/// `build_rule_style_157` (:3915): an `<hr>` as Kindle Previewer 4.0.3 writes it (#722).
pub(crate) fn build_rule_style_157(g: &mut Generator, name: &str, s: &RuleStyle) -> Value {
    g.local(name);
    let Value::Struct(mut v) = st([("$173", sym(name))]) else {
        unreachable!()
    };
    for (key, em) in [("$47", s.space_above), ("$49", s.space_below)] {
        if let Some(em) = em.filter(|&e| e != 0.0) {
            v.insert(key, g6(em / LINE_EM, "$310"));
        }
    }
    if let Some((n, unit)) = &s.width {
        v.insert("$56", g6(*n, if unit == "%" { "$314" } else { "$308" }));
        if unit == "em" {
            v.insert("$65", g6(100.0, "$314"));
        }
        v.insert("$546", sym("$377"));
    }
    if let Some(align) = s.align.as_deref().filter(|a| !a.is_empty()) {
        let at = match align {
            "left" => "$59",
            "right" => "$61",
            _ => "$320",
        };
        v.insert("$580", sym(at));
    }
    for (key, pct) in [("$48", s.margin_left), ("$50", s.margin_right)] {
        if let Some(pct) = pct.filter(|&p| p != 0.0) {
            v.insert(key, g6(pct, "$314"));
        }
    }
    let sides: Vec<_> = s
        .border
        .iter()
        .flatten()
        .filter(|(_, (kind, ..))| border_style_symbol(kind).is_some())
        .collect();
    let colour_of = |c: &Option<i64>| c.filter(|&c| c != 0);
    let border_width = |w: &RuleWidth| match *w {
        RuleWidth::Pt(pt) => g6(pt, "$318"),
        RuleWidth::Em(em) => g6(em, "$308"),
    };
    if sides.len() == 4 && sides.iter().all(|(_, spec)| *spec == sides[0].1) {
        let (kind, width, colour) = &sides[0].1;
        v.insert("$93", border_width(width));
        if !(kind == "inset" && colour.is_none()) {
            v.insert("$88", sym(border_style_symbol(kind).unwrap()));
        }
        if let Some(c) = colour_of(colour) {
            v.insert("$83", Value::Int(c));
        }
    } else if !sides.is_empty() {
        for side in ["top", "right", "bottom", "left"] {
            let [w, st_key, c] = side_keys(side);
            let Some((_, (kind, width, colour))) = sides.iter().find(|(s, _)| s == side) else {
                v.insert(w, g6(0.0, "$318"));
                continue;
            };
            v.insert(w, border_width(width));
            v.insert(st_key, sym(border_style_symbol(kind).unwrap()));
            if let Some(colour) = colour_of(colour) {
                v.insert(c, Value::Int(colour));
            }
        }
    }
    if let Some(fill) = s.fill {
        v.insert("$70", Value::Int(fill));
        if s.border.is_none() {
            v.insert("$93", g6(0.0, "$318"));
        }
    }
    if let Some((n, unit)) = &s.height {
        v.insert("$62", g6(*n, if unit == "pt" { "$318" } else { "$308" }));
    }
    Value::Struct(v)
}

/// `_is_text_block` (:1127): a paragraph of words, not a table, a box or only pictures.
fn is_text_block(b: &Block) -> bool {
    b.table.is_none()
        && b.boxed.is_none()
        && !py::strip(&IMG_TOKEN.replace_all(&b.text, "")).is_empty()
}

/// `_cut_title_from_box` (:1379): the box block with `title` cut from its first paragraph,
/// through a box it opens with (#238, #310), and the ids it carries; None when it doesn't
/// open with the title. The box is None when nothing is left in it.
pub(crate) fn cut_title_from_box(
    block: &Block,
    title: &str,
) -> Option<(Option<Block>, Vec<String>)> {
    let bx = block.boxed.as_deref()?;
    let first = bx.blocks.first()?;
    let (mut kids, mut carried): (Vec<Block>, Vec<String>);
    if first.boxed.is_some() {
        let (inner, more) = cut_title_from_box(first, title)?;
        kids = inner.into_iter().collect();
        kids.extend_from_slice(&bx.blocks[1..]);
        carried = more;
    } else if !is_text_block(first) {
        return None;
    } else {
        let stripped = py::lstrip(&first.text);
        if !opens_with_title(stripped, title) {
            return None;
        }
        let remainder = py::lstrip(py::skip(stripped, py::len(title)));
        if remainder.is_empty() {
            kids = bx.blocks[1..].to_vec();
            carried = first.anchors.keys.clone();
        } else {
            // `_trim_text` (:909): runs and offsets move with the cut.
            let rest = py::len(remainder) as i64;
            let mut cut = first.clone();
            inline::trim(
                &mut cut.spans,
                &mut cut.anchors,
                py::len(&first.text) as i64 - rest,
                rest,
            );
            cut.text = remainder.to_owned();
            kids = vec![cut];
            kids.extend_from_slice(&bx.blocks[1..]);
            carried = Vec::new();
        }
    }
    if kids.is_empty() {
        carried.extend(block.anchors.keys.iter().cloned());
        return Some((None, carried));
    }
    Some((Some(with_blocks(block, bx, kids)), carried))
}

/// `_box_heading` (:5921): the box `block`, which opened with the chapter's title, opened
/// before kfxgen's heading at `heading`, as Kindle Previewer 4.0.3 draws it round the
/// book's heading (#646). With `close` it holds the heading alone. Returns where the heading
/// is now.
pub(crate) fn box_heading(
    block: &Block,
    chunks: &mut Vec<Chunk>,
    heading: usize,
    close: bool,
) -> usize {
    let bx = block.boxed.as_deref().expect("a box block");
    chunks.insert(
        heading,
        Chunk::marker(Marker::Open(Node::Box(BoxOpen::of(block, bx)))),
    );
    if close {
        chunks.push(Chunk::marker(Marker::Close));
    }
    heading + 1
}

/// Where a title line is in a block: the block whole, a table's row, or a box's line.
pub(crate) enum Line {
    Whole,
    Row(usize),
    /// The blocks' indices through the boxes it opens with, and a row of a table there.
    Boxed(Vec<usize>, Option<usize>),
}

/// A box's line as the split-title cut reads it: its path through the boxes, a table's row
/// there, its text.
pub(crate) type BoxLine = (Vec<usize>, Option<usize>, String);

/// `_box_title_lines` (:1413): the paragraphs a box opens with, through the boxes it opens
/// with, as the split-title cut reads them, a table's rows with text as lines too; and
/// whether no picture or rule stopped them.
pub(crate) fn box_title_lines(bx: &BoxBlock, path: &[usize]) -> (Vec<BoxLine>, bool) {
    let mut out = Vec::new();
    for (j, kid) in bx.blocks.iter().enumerate() {
        let here = [path, &[j]].concat();
        if let Some(inner) = &kid.boxed {
            let (lines, whole) = box_title_lines(inner, &here);
            out.extend(lines);
            if !whole {
                return (out, false);
            }
        } else if let Some(t) = &kid.table {
            for r in tables::source_order(&t.rows) {
                let text = tables::row_text(&t.rows[r]);
                if !text.is_empty() {
                    out.push((here.clone(), Some(r), text));
                }
            }
        } else if is_text_block(kid) {
            out.push((here, None, kid.text.clone()));
        } else {
            return (out, false);
        }
    }
    (out, true)
}

/// `_drop_box_lines` (:1441): the box block without its blocks up to and including the line
/// at `path`, a table's rows up to and including `row` (#310, #344); None when nothing is
/// left. And the ids of what it dropped.
pub(crate) fn drop_box_lines(
    block: &Block,
    path: &[usize],
    row: Option<usize>,
) -> (Option<Block>, Vec<String>) {
    let bx = block.boxed.as_deref().expect("a box block");
    let kids = &bx.blocks;
    let j = path[0];
    let mut carried: Vec<String> = kids[..j].iter().flat_map(block_keys).collect();
    let mut rest: Vec<Block> = Vec::new();
    match (path.len(), row) {
        (1, None) => carried.extend(kids[j].anchors.keys.iter().cloned()),
        (1, Some(r)) => {
            let t = kids[j].table.as_deref().expect("a table line");
            let order = tables::source_order(&t.rows);
            let upto = order.iter().position(|&o| o == r).unwrap_or(0);
            match tables::drop_rows(&kids[j], &order[..=upto]) {
                Some(kept) => rest.push(kept),
                None => carried.extend(kids[j].anchors.keys.iter().cloned()),
            }
        }
        _ => {
            let (inner, more) = drop_box_lines(&kids[j], &path[1..], row);
            carried.extend(more);
            rest.extend(inner);
        }
    }
    rest.extend_from_slice(&kids[j + 1..]);
    if rest.is_empty() {
        carried.extend(block.anchors.keys.iter().cloned());
        return (None, carried);
    }
    (Some(with_blocks(block, bx, rest)), carried)
}

/// `{**box, "blocks": kids, "text": "\n\n".join(...)}`.
fn with_blocks(block: &Block, bx: &BoxBlock, kids: Vec<Block>) -> Block {
    let text = kids
        .iter()
        .map(|k| k.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    Block {
        text,
        boxed: Some(Box::new(BoxBlock {
            blocks: kids,
            ..bx.clone()
        })),
        ..block.clone()
    }
}

/// The chapter title cut from the box `iter_blocks` opens with (:6392): the box kept without
/// it, its ids carried, and the box drawn round kfxgen's heading when the title was a block
/// of its own (#646). False when the box doesn't open with the title.
pub(crate) fn cut_title_in_box(
    iter_blocks: &mut Vec<Block>,
    title: &str,
    chunks: &mut Vec<Chunk>,
    heading_chunk: &mut Option<usize>,
    carried: &mut Vec<String>,
) -> bool {
    let Some((mut kept, keys)) = cut_title_from_box(&iter_blocks[0], title) else {
        return false;
    };
    let first = iter_blocks.remove(0);
    carried.extend(keys);
    let first_box = first.boxed.as_deref().expect("a box");
    let kept_fewer = kept.as_ref().is_none_or(|k| {
        let kids = &k.boxed.as_deref().expect("a box").blocks;
        kids.len() < first_box.blocks.len()
    });
    // A paragraph running the title in keeps the rest in the box, the heading above.
    if let Some(h) = *heading_chunk
        && h + 1 == chunks.len()
        && first_box.blocks[0].boxed.is_none()
        && kept_fewer
    {
        let at = box_heading(&first, chunks, h, kept.is_none());
        *heading_chunk = Some(at);
        let kid = &first_box.blocks[0];
        keep_space(
            &mut chunks[at].heading,
            kid.style.as_ref(),
            None,
            kid.margin_below,
        );
        if let Some(k) = kept.as_mut().and_then(|k| k.boxed.as_mut()) {
            k.opened = true;
        }
    }
    iter_blocks.splice(0..0, kept);
    true
}
