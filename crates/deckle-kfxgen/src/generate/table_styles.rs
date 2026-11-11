// SPDX-License-Identifier: GPL-3.0-or-later
//! A table's `$157` styles, as Kindle Previewer writes them: the table's
//! (`build_table_style_157`, `native_generator.py:3808`), a cell's (`build_cell_style_157`
//! :4015, from `_cell_style` :7326) and a row's or row group's border (`build_part_style_157`
//! :4007), and the table's `$152` columns.
use deckle_kfx::ion::{Decimal, Struct, Value};

use super::block_styles::{BlockStyle, family};
use super::chunks::{Marker, Node};
use super::styles::{StyleArgs, Styles, g6};
use super::table_chunks::CellInfo;
use super::tables::{Border, ColumnWidths, Pad, Padding};
use super::{Generator, measure, py, st, sym};

/// The arguments of `build_table_style_157` the ported paths set.
#[derive(Clone, PartialEq)]
pub(crate) struct TableStyle {
    pub border: Option<Border>,
    pub width: Option<f64>,
}

/// The arguments of `build_part_style_157`.
#[derive(Clone, PartialEq)]
pub(crate) struct PartStyle {
    pub border: Option<Border>,
}

/// The arguments `_cell_style` (:7326) gives `build_cell_style_157`: no typography.
#[derive(Clone, PartialEq)]
pub(crate) struct CellStyle {
    pub align: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub font_family: Option<&'static str>,
    pub colspan: i64,
    pub rowspan: i64,
    pub font_size: f64,
    pub valign: String,
    pub border: Option<Border>,
    pub border_box: bool,
    pub padding: Option<Padding>,
    pub table_width: Option<f64>,
    pub background: Option<i64>,
}

impl CellStyle {
    /// `_cell_style` (:7326): a header is bold and centred on its own.
    pub(crate) fn of(cell: &CellInfo, bs: Option<&BlockStyle>, font_size: f64) -> Self {
        let (blk_bold, blk_italic) = bs.map_or((false, false), |b| (b.bold, b.italic));
        let align = bs.and_then(|b| b.align.clone());
        CellStyle {
            align: align.or_else(|| cell.header.then(|| "center".to_owned())),
            bold: (cell.header && !cell.not_bold) || blk_bold,
            italic: blk_italic,
            font_family: bs.and_then(|b| family(&b.family)),
            colspan: cell.colspan,
            rowspan: cell.rowspan,
            font_size,
            // Unset is middle, as calibre's UA sheet makes it, so the two share one style.
            valign: match cell.valign.as_deref() {
                None | Some("") => "middle".to_owned(),
                Some(v) => v.to_owned(),
            },
            border: cell.border.clone(),
            border_box: cell.width_set,
            padding: cell.padding.clone(),
            table_width: cell.padding.as_ref().and(cell.table_width),
            background: cell.background,
        }
    }
}

/// A table container's `$157` (:7441), and a row's or group's own for its border (:7556):
/// (the entry's style, its part style). `bs`, `font_size`, `lang`: a cell's.
pub(crate) fn marker_styles(
    g: &mut Generator,
    styles: &mut Styles,
    marker: &Marker,
    bs: Option<&BlockStyle>,
    font_size: f64,
    lang: Option<&str>,
) -> (Option<String>, Option<String>) {
    match marker {
        Marker::Open(Node::Table(t)) => {
            let table = TableStyle {
                border: t.border.clone(),
                width: t.width,
            };
            (
                Some(styles.allocate(g, "_tbl", StyleArgs::Table(table))),
                None,
            )
        }
        Marker::Open(Node::Cell(cell)) => {
            let cell = CellStyle::of(cell, bs, font_size);
            let name = styles.allocate_in(g, "_td", StyleArgs::Cell(cell), lang);
            (Some(name), None)
        }
        Marker::Open(Node::Group(_, Some(border)) | Node::Row(Some(border))) => {
            let part = PartStyle {
                border: Some(border.clone()),
            };
            (None, Some(styles.allocate(g, "_tp", StyleArgs::Part(part))))
        }
        _ => (None, None),
    }
}

/// `inline_style.ALIGN_MAP` (:934).
pub(crate) fn align_symbol(align: &str) -> Option<&'static str> {
    match align {
        "left" => Some("$59"),
        "right" => Some("$61"),
        "center" => Some("$320"),
        "justify" => Some("$321"),
        _ => None,
    }
}

/// `inline_style.VALIGN_MAP` (:939).
fn valign_symbol(valign: &str) -> Option<&'static str> {
    match valign {
        "top" => Some("$58"),
        "middle" => Some("$320"),
        "bottom" => Some("$60"),
        _ => None,
    }
}

/// `_BORDER_STYLE_SYMBOLS` (:1967).
pub(crate) fn border_style_symbol(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "solid" => "$328",
        "double" => "$329",
        "dashed" => "$330",
        "dotted" => "$331",
        "groove" => "$334",
        "ridge" => "$335",
        "inset" => "$336",
        "outset" => "$337",
        _ => return None,
    })
}

/// `_BORDER_SIDE_KEYS` (:1978): a side's width, style and colour keys.
pub(crate) fn side_keys(side: &str) -> [&'static str; 3] {
    match side {
        "top" => ["$94", "$89", "$84"],
        "right" => ["$97", "$92", "$87"],
        "bottom" => ["$96", "$91", "$86"],
        _ => ["$95", "$90", "$85"],
    }
}

/// `_apply_border` (:1991): equal sides as `$93`/`$88` (and `$83` unless black), else each
/// drawn side's own. A table's default grey `$83` goes when its border is black.
pub(crate) fn apply_border(value: &mut Struct, border: Option<&Border>, is_table: bool) {
    let sides: Vec<_> = border
        .into_iter()
        .flatten()
        .filter(|(_, (kind, width, _))| border_style_symbol(kind).is_some() && *width > 0.0)
        .collect();
    let Some((_, first)) = sides.first() else {
        return;
    };
    let colour_of = |c: &Option<i64>| c.filter(|&c| c != 0);
    if sides.len() == 4 && sides.iter().all(|(_, spec)| spec == first) {
        let (kind, width, colour) = first;
        value.insert("$93", g6(*width, "$318"));
        value.insert("$88", sym(border_style_symbol(kind).unwrap()));
        match colour_of(colour) {
            Some(c) => value.insert("$83", Value::Int(c)),
            None if is_table => {
                value.remove("$83");
            }
            None => {}
        }
        return;
    }
    for (side, (kind, width, colour)) in sides {
        let [w, s, c] = side_keys(side);
        value.insert(w, g6(*width, "$318"));
        value.insert(s, sym(border_style_symbol(kind).unwrap()));
        if let Some(colour) = colour_of(colour) {
            value.insert(c, Value::Int(colour));
        }
    }
    if is_table {
        value.remove("$83");
    }
}

/// `_column_widths_152` (:2028): a table's `$152` columns, or None. From `<col>`, one entry
/// each; from cells, adjacent equal widths merged by a `$118` count. Trailing unset ones go.
pub(crate) fn column_widths_152(widths: Option<&ColumnWidths>) -> Option<Value> {
    let (source, widths) = widths?;
    let mut widths = widths.clone();
    while widths.last() == Some(&None) {
        widths.pop();
    }
    if widths.is_empty() {
        return None;
    }
    let pct = |w: f64| g6(w, "$314");
    let mut out = Vec::new();
    if source == "col" {
        for w in widths {
            out.push(match w {
                None => Value::Struct(Struct::new()),
                Some(w) => st([("$56", pct(w)), ("$546", sym("$377"))]),
            });
        }
        return Some(Value::List(out));
    }
    let mut i = 0;
    while i < widths.len() {
        let w = widths[i];
        let mut n = 1;
        while i + n < widths.len() && widths[i + n] == w && w.is_some() {
            n += 1;
        }
        out.push(match w {
            None => Value::Struct(Struct::new()),
            Some(w) => {
                let Value::Struct(mut entry) = st([("$56", pct(w))]) else {
                    unreachable!()
                };
                if n > 1 {
                    entry.insert("$118", Value::Int(n as i64));
                }
                Value::Struct(entry)
            }
        });
        i += n;
    }
    Some(Value::List(out))
}

/// `_padding_values` (:2071): top and bottom in lh, em / 1.2; left and right in %, em x 3.125
/// over the table's width; a percentage p is p x 32 / 100 em and p %.
/// None where kfxgen's `quantize` raises (`py::quantize_milli`).
pub(crate) fn padding_values(
    padding: &Padding,
    table_width: Option<f64>,
) -> Option<Vec<(&'static str, Value)>> {
    let frac = table_width.filter(|&w| w != 0.0).map_or(1.0, |w| w / 100.0);
    let mut out = Vec::new();
    for (side, value) in padding {
        let (em, pct) = match *value {
            Pad::Pct(n) => (n * 32.0 / 100.0, n / frac),
            Pad::Em(v) => (v, v * 3.125 / frac),
        };
        if em == 0.0 {
            continue;
        }
        let key = match side.as_str() {
            "top" => "$52",
            "right" => "$55",
            "bottom" => "$54",
            _ => "$53",
        };
        let v = if matches!(side.as_str(), "top" | "bottom") {
            g6(em / 1.2, "$310")
        } else {
            measure(py::quantize_milli(pct)?, "$314")
        };
        out.push((key, v));
    }
    Some(out)
}

fn decimal(text: &str) -> Decimal {
    py::decimal(text)
}

/// `build_table_style_157` (:3808).
pub(crate) fn build_table_style_157(g: &mut Generator, name: &str, s: &TableStyle) -> Value {
    g.local(name);
    let Value::Struct(mut v) = st([
        ("$16", measure(decimal("1"), "$505")),
        ("$65", measure(decimal("100"), "$314")),
        ("$42", measure(decimal("1"), "$310")),
        ("$173", sym(name)),
        ("$83", Value::Int(4286611584)),
    ]) else {
        unreachable!()
    };
    apply_border(&mut v, s.border.as_ref(), true);
    if let Some(width) = s.width {
        // A min-width, and at 100% the width too (#264).
        let pct = g6(width, "$314");
        if width == 100.0 {
            v.insert("$56", pct.clone());
        }
        v.insert("$63", pct);
        v.insert("$546", sym("$377"));
    }
    Value::Struct(v)
}

/// `build_part_style_157` (:4007).
pub(crate) fn build_part_style_157(g: &mut Generator, name: &str, s: &PartStyle) -> Value {
    g.local(name);
    let Value::Struct(mut v) = st([("$173", sym(name))]) else {
        unreachable!()
    };
    apply_border(&mut v, s.border.as_ref(), false);
    Value::Struct(v)
}

/// `build_cell_style_157` (:4015).
pub(crate) fn build_cell_style_157(g: &mut Generator, name: &str, s: &CellStyle) -> Value {
    g.local(name);
    let lh = |v: &str| measure(decimal(v), "$310");
    let pct = |v: &str| measure(decimal(v), "$314");
    let Value::Struct(mut v) = st([
        ("$52", lh("0.03125")),
        ("$53", pct("0.117")),
        ("$54", lh("0.03125")),
        ("$55", pct("0.117")),
        ("$173", sym(name)),
    ]) else {
        unreachable!()
    };
    if let Some(valign) = valign_symbol(&s.valign) {
        // First, where it has always been written.
        v.0.insert(0, ("$633".into(), sym(valign)));
    }
    if let Some(align) = s.align.as_deref().and_then(align_symbol) {
        v.insert("$34", sym(align));
    }
    if s.bold {
        v.insert("$13", sym("$361"));
    }
    if s.italic {
        v.insert("$12", sym("$382"));
    }
    if s.colspan > 1 {
        v.insert("$148", Value::Int(s.colspan));
    }
    if s.rowspan > 1 {
        v.insert("$149", Value::Int(s.rowspan));
    }
    if let Some(family) = s.font_family {
        v.insert("$11", Value::string(family));
    }
    if s.font_size != 1.0 {
        v.insert("$16", measure(py::decimal_of_float(s.font_size), "$505"));
    }
    apply_border(&mut v, s.border.as_ref(), false);
    if let Some(padding) = &s.padding {
        // The cell's own padding replaces Previewer's 1px default (#264).
        for key in ["$52", "$53", "$54", "$55"] {
            v.remove(key);
        }
        let values = padding_values(padding, s.table_width)
            .expect("checked when the table was read (tables::read_table_block)");
        for (key, value) in values {
            v.insert(key, value);
        }
    }
    if s.border_box {
        v.insert("$546", sym("$377"));
    }
    if let Some(fill) = s.background {
        v.insert("$70", Value::Int(fill));
    }
    Value::Struct(v)
}
