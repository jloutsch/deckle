// SPDX-License-Identifier: GPL-3.0-or-later
//! Native tables (#219): a table block read from the model, and what the chapter title cut
//! (`native_generator.py:1187` and on) reads of its rows. `table_chunks` makes its
//! storyline entries, `table_styles` their `$157` styles.

use std::collections::BTreeSet;

use super::block_styles::{BlockStyle, number, read_block_style};
use super::chapters::Block;
use super::images;
use super::inline::{Anchors, Span, read_spans};
use super::inline_pictures::cut_after_picture_lines;
use super::table_styles::padding_values;
use super::writing::Bidi;
use super::{GenerateError, model_err, py};
use crate::pyvalue::{PyValue, truthy};

use GenerateError::Unsupported;

/// A border as `_border_key` (:1986) keys it: `(side, (style, width in pt, ARGB or None))`
/// in side order, never empty.
pub(crate) type Border = Vec<(String, (String, f64, Option<i64>))>;

/// A cell's padding side: ems, or `("%", n)`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Pad {
    Em(f64),
    Pct(f64),
}

/// A padding as `_border_key` keys it: `(side, value)` in side order, never empty.
pub(crate) type Padding = Vec<(String, Pad)>;

/// `("col" or "cell", [percent or None per column])` (converter `_column_widths`).
pub(crate) type ColumnWidths = (String, Vec<Option<f64>>);

#[derive(Clone)]
pub(crate) struct Table {
    pub rows: Vec<Row>,
    pub anchor_keys: Vec<String>,
    pub border: Option<Border>,
    pub column_widths: Option<ColumnWidths>,
    pub width: Option<f64>,
    /// `bidi`: `{}` sets none inside it, where None takes the container's (#246).
    pub bidi: Option<Bidi>,
}

#[derive(Clone)]
pub(crate) struct Row {
    /// "head", "body" or "foot".
    pub group: &'static str,
    pub border: Option<Border>,
    pub group_border: Option<Border>,
    /// Its own direction and its group's (#605).
    pub bidi: Option<Bidi>,
    pub group_bidi: Option<Bidi>,
    pub anchor_keys: Vec<String>,
    pub cells: Vec<Cell>,
    /// Where the row sat in the source (:872), else its index.
    pub source_order: Option<i64>,
}

#[derive(Clone)]
pub(crate) struct Cell {
    pub text: String,
    pub spans: Vec<Span>,
    pub anchors: Anchors,
    pub header: bool,
    pub not_bold: bool,
    pub heading: bool,
    pub colspan: i64,
    pub rowspan: i64,
    pub valign: Option<String>,
    pub border: Option<Border>,
    pub width_set: bool,
    pub padding: Option<Padding>,
    pub background: Option<i64>,
    /// A cell holding several blocks (#261).
    pub paragraphs: Vec<Paragraph>,
    pub style: Option<BlockStyle>,
}

#[derive(Clone)]
pub(crate) struct Paragraph {
    pub text: String,
    pub spans: Vec<Span>,
    pub anchors: Anchors,
    pub heading: bool,
}

fn opt<T>(
    v: Option<&PyValue>,
    read: impl FnOnce(&PyValue) -> Option<T>,
    err: impl FnOnce() -> GenerateError,
) -> Result<Option<T>, GenerateError> {
    match v {
        None | Some(PyValue::None) => Ok(None),
        Some(v) => read(v).map(Some).ok_or_else(err),
    }
}

const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];

/// A dict of sides, sorted, as `_border_key` (:1986) keys it; None when empty.
pub(crate) fn read_sides<T>(
    v: Option<&PyValue>,
    at: &str,
    expected: &str,
    side: impl Fn(&PyValue) -> Option<T>,
) -> Result<Option<Vec<(String, T)>>, GenerateError> {
    let err = || model_err(format!("{at}: {expected}"));
    let pairs = match v {
        None | Some(PyValue::None) => return Ok(None),
        Some(PyValue::Dict(pairs)) => pairs,
        Some(_) => return Err(err()),
    };
    let mut out = pairs
        .iter()
        .map(|(k, v)| {
            let name = k.as_str().filter(|s| SIDES.contains(s))?;
            Some((name.to_owned(), side(v)?))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(err)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((!out.is_empty()).then_some(out))
}

pub(crate) fn read_border(v: Option<&PyValue>, at: &str) -> Result<Option<Border>, GenerateError> {
    read_sides(v, at, "{side: (style, width, colour)}", |spec| {
        let [kind, width, colour] = spec.as_list()? else {
            return None;
        };
        let colour = match colour {
            PyValue::None => None,
            c => Some(c.as_int()?),
        };
        Some((kind.as_str()?.to_owned(), number(width)?, colour))
    })
}

pub(crate) fn read_padding(
    v: Option<&PyValue>,
    at: &str,
) -> Result<Option<Padding>, GenerateError> {
    read_sides(v, at, "{side: ems or (\"%\", n)}", |p| match p {
        PyValue::Tuple(_) | PyValue::List(_) => Some(Pad::Pct(number(p.as_list()?.get(1)?)?)),
        p => Some(Pad::Em(number(p)?)),
    })
}

fn read_keys(d: &PyValue, at: &str) -> Result<Vec<String>, GenerateError> {
    Ok(Anchors::read(d, at)?.keys)
}

/// A block of type "table".
pub(crate) fn read_table_block(b: &PyValue, at: &str) -> Result<Block, GenerateError> {
    // Read by `_emit_table_chunks` (#381, #680) and `page_bidi`, and not ported.
    if truthy(b.get("clear")) {
        return Err(Unsupported("clearing floats"));
    }
    if truthy(b.get("trailing_space")) {
        return Err(Unsupported("spacers"));
    }
    if truthy(b.get("block_style")) {
        return Err(Unsupported("block styles"));
    }
    let text = match b.get("text") {
        Some(PyValue::Str(s)) => s.clone(),
        _ => return Err(model_err(format!("{at}.text: a string"))),
    };
    let t = b
        .get("table")
        .ok_or_else(|| model_err(format!("{at}.table: a dict")))?;
    let at_t = format!("{at}.table");
    let bidi = Bidi::read(t.get("bidi"), &format!("{at_t}.bidi"))?;
    if truthy(t.get("horizontal")) {
        return Err(Unsupported("vertical text"));
    }
    let rows = t
        .get("rows")
        .and_then(PyValue::as_list)
        .ok_or_else(|| model_err(format!("{at_t}.rows: a list")))?
        .iter()
        .enumerate()
        .map(|(j, r)| read_row(r, &format!("{at_t}.rows[{j}]")))
        .collect::<Result<_, _>>()?;
    let column_widths = opt(
        t.get("column_widths"),
        |v| {
            let [source, widths] = v.as_list()? else {
                return None;
            };
            let widths = widths
                .as_list()?
                .iter()
                .map(|w| match w {
                    PyValue::None => Some(None),
                    w => number(w).map(Some),
                })
                .collect::<Option<_>>()?;
            Some((source.as_str()?.to_owned(), widths))
        },
        || model_err(format!("{at_t}.column_widths: (source, [percent or None])")),
    )?;
    let table = Table {
        rows,
        anchor_keys: read_keys(t, &at_t)?,
        border: read_border(t.get("border"), &format!("{at_t}.border"))?,
        column_widths,
        width: opt(t.get("width"), number, || {
            model_err(format!("{at_t}.width: a number"))
        })?,
        bidi,
    };
    // kfxgen's `quantize` raises on a side it can't write in 28 digits (:2071).
    for (j, row) in table.rows.iter().enumerate() {
        for (k, cell) in row.cells.iter().enumerate() {
            if let Some(p) = &cell.padding
                && padding_values(p, table.width).is_none()
            {
                return Err(model_err(format!(
                    "{at_t}.rows[{j}].cells[{k}].padding: a side kfxgen can write in 28 digits"
                )));
            }
        }
    }
    Ok(Block {
        anchors: Anchors::read(b, at)?,
        table: Some(Box::new(table)),
        ..Block::plain(text)
    })
}

fn read_row(r: &PyValue, at: &str) -> Result<Row, GenerateError> {
    let group = match r.get("group").and_then(PyValue::as_str) {
        Some("head") => "head",
        Some("body") => "body",
        Some("foot") => "foot",
        _ => return Err(model_err(format!("{at}.group: head, body or foot"))),
    };
    let cells = r
        .get("cells")
        .and_then(PyValue::as_list)
        .ok_or_else(|| model_err(format!("{at}.cells: a list")))?
        .iter()
        .enumerate()
        .map(|(k, c)| read_cell(c, &format!("{at}.cells[{k}]")))
        .collect::<Result<_, _>>()?;
    Ok(Row {
        group,
        border: read_border(r.get("border"), &format!("{at}.border"))?,
        group_border: read_border(r.get("group_border"), &format!("{at}.group_border"))?,
        bidi: Bidi::read(r.get("bidi"), &format!("{at}.bidi"))?,
        group_bidi: Bidi::read(r.get("group_bidi"), &format!("{at}.group_bidi"))?,
        anchor_keys: read_keys(r, at)?,
        cells,
        source_order: opt(r.get("source_order"), PyValue::as_int, || {
            model_err(format!("{at}.source_order: an int"))
        })?,
    })
}

fn read_cell(c: &PyValue, at: &str) -> Result<Cell, GenerateError> {
    let text = match c.get("text") {
        Some(PyValue::None) => String::new(),
        Some(PyValue::Str(s)) => s.clone(),
        _ => return Err(model_err(format!("{at}.text: a string or None"))),
    };
    let span = |key: &str| match c.get(key) {
        None => Ok(1),
        Some(PyValue::Int(n)) => Ok(*n),
        Some(_) => Err(model_err(format!("{at}.{key}: an int"))),
    };
    let paragraphs: Vec<Paragraph> = match c.get("paragraphs") {
        None | Some(PyValue::None) => Vec::new(),
        Some(v) => v
            .as_list()
            .ok_or_else(|| model_err(format!("{at}.paragraphs: a list")))?
            .iter()
            .enumerate()
            .map(|(i, p)| read_paragraph(p, &format!("{at}.paragraphs[{i}]")))
            .collect::<Result<_, _>>()?,
    };
    if paragraphs.is_empty() && !text.is_empty() {
        // `_cut_after_picture_lines` would make it a container of its parts (#624).
        if images::has_picture(&text) {
            return Err(Unsupported("pictures in a table cell"));
        }
        if cut_after_picture_lines(&Block::plain(text.clone())).is_some() {
            return Err(Unsupported("a cell cut at an empty line"));
        }
    }
    Ok(Cell {
        text,
        spans: read_spans(c.get("spans"), at)?,
        anchors: Anchors::read(c, at)?,
        header: truthy(c.get("header")),
        not_bold: truthy(c.get("not_bold")),
        heading: truthy(c.get("heading")),
        colspan: span("colspan")?,
        rowspan: span("rowspan")?,
        valign: opt(
            c.get("valign"),
            |v| v.as_str().map(str::to_owned),
            || model_err(format!("{at}.valign: a string")),
        )?,
        border: read_border(c.get("border"), &format!("{at}.border"))?,
        width_set: truthy(c.get("width_set")),
        padding: read_padding(c.get("padding"), &format!("{at}.padding"))?,
        background: opt(c.get("background"), PyValue::as_int, || {
            model_err(format!("{at}.background: an ARGB int"))
        })?,
        paragraphs,
        style: read_block_style(c.get("block_style"), at)?,
    })
}

fn read_paragraph(p: &PyValue, at: &str) -> Result<Paragraph, GenerateError> {
    if p.get("type").and_then(PyValue::as_str) == Some("box") {
        return Err(Unsupported("boxes"));
    }
    if truthy(p.get("block_style")) {
        return Err(Unsupported("block styles"));
    }
    // Its margins, written on its entry (#655).
    let above = p.get("margin_above").or(p.get("space_above"));
    if above.is_some_and(|v| !matches!(v, PyValue::None)) || truthy(p.get("margin_below")) {
        return Err(Unsupported("block margins"));
    }
    let text = match p.get("text") {
        Some(PyValue::Str(s)) => s.clone(),
        _ => return Err(model_err(format!("{at}.text: a string"))),
    };
    if images::has_picture(&text) {
        return Err(Unsupported("pictures in a table cell"));
    }
    Ok(Paragraph {
        spans: read_spans(p.get("spans"), at)?,
        anchors: Anchors::read(p, at)?,
        heading: truthy(p.get("heading")),
        text,
    })
}

/// `_dedupe_keys` (:1571).
pub(crate) fn dedupe(keys: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    keys.into_iter()
        .filter(|k| seen.insert(k.clone()))
        .collect()
}

/// `_block_keys` (:1110): every anchor key a block holds, its table's included.
pub(crate) fn block_keys(b: &Block) -> Vec<String> {
    let mut keys = b.anchors.keys.clone();
    if let Some(t) = &b.table {
        keys.extend(t.anchor_keys.iter().cloned());
        for r in &t.rows {
            keys.extend(r.anchor_keys.iter().cloned());
            for c in &r.cells {
                keys.extend(c.anchors.keys.iter().cloned());
                for p in &c.paragraphs {
                    keys.extend(p.anchors.keys.iter().cloned());
                }
            }
        }
    }
    keys
}

/// `_table_feature_version` (:1953): `yj_table` 3 when a table uses rowspan, else 1.
pub(crate) fn feature_version<'a>(blocks: impl IntoIterator<Item = &'a Block>) -> i64 {
    let rowspan = blocks
        .into_iter()
        .filter_map(|b| b.table.as_deref())
        .flat_map(|t| &t.rows)
        .flat_map(|r| &r.cells)
        .any(|c| c.rowspan > 1);
    if rowspan { 3 } else { 1 }
}

/// `_row_text` (:857): a row's non-empty cells joined by a space.
pub(crate) fn row_text(row: &Row) -> String {
    row.cells
        .iter()
        .filter(|c| !c.text.is_empty())
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `_source_order` (:872): row indices in the order the rows build wrote them.
pub(crate) fn source_order(rows: &[Row]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&j| rows[j].source_order.unwrap_or(j as i64));
    order
}

/// `_table_opens_with_title` (:1469): the first row with text opens with `title`.
pub(crate) fn opens_with_title(t: &Table, title: &str) -> bool {
    source_order(&t.rows)
        .into_iter()
        .map(|j| row_text(&t.rows[j]))
        .find(|text| !text.is_empty())
        .is_some_and(|text| super::title::opens_with_title(py::lstrip(&text), title))
}

/// `_drop_table_rows` (:879): the table without the rows at `dropped`, their ids joining
/// the table's own; None when no row with text is left.
pub(crate) fn drop_rows(block: &Block, dropped: &[usize]) -> Option<Block> {
    let t = block.table.as_deref()?;
    let mut sorted = dropped.to_vec();
    sorted.sort_unstable();
    let moved = sorted.iter().flat_map(|&j| {
        let r = &t.rows[j];
        r.anchor_keys
            .iter()
            .cloned()
            .chain(r.cells.iter().flat_map(|c| {
                c.anchors
                    .keys
                    .iter()
                    .chain(c.paragraphs.iter().flat_map(|p| &p.anchors.keys))
                    .cloned()
            }))
    });
    let anchor_keys = dedupe(t.anchor_keys.iter().cloned().chain(moved));
    let rows: Vec<Row> = (0..t.rows.len())
        .filter(|j| !dropped.contains(j))
        .map(|j| t.rows[j].clone())
        .collect();
    if rows.iter().all(|r| row_text(r).is_empty()) {
        return None;
    }
    // `_with_rows` (:863).
    let text = rows
        .iter()
        .map(row_text)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Some(Block {
        text,
        table: Some(Box::new(Table {
            rows,
            anchor_keys,
            ..(*t).clone()
        })),
        ..block.clone()
    })
}
