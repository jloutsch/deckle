// SPDX-License-Identifier: GPL-3.0-or-later
//! The `$157` paragraph styles: [`Style157`] holds `build_fragment_157`'s arguments, [`Styles`]
//! allocates one fragment per distinct set.
use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::{Decimal, Value};

use super::table_styles::{
    CellStyle, PartStyle, TableStyle, build_cell_style_157, build_part_style_157,
    build_table_style_157,
};
use super::{Generator, measure, py, st, sym};

/// `build_fragment_157`'s arguments that the ported paths set, defaulting as it does. Two
/// styles are one when their arguments are, as `_allocate_style` keys them (:6950).
#[derive(Clone, PartialEq)]
pub(crate) struct Style157 {
    pub font_size: f64,
    pub bold: bool,
    pub italic: bool,
    pub upright: bool,
    pub underline: bool,
    pub strike: bool,
    pub margin_top: Option<f64>,
    /// A contents line's indent in %, as `f"{6.25 * level:g}"` writes it.
    pub margin_left: Option<String>,
    pub baseline_style: Option<&'static str>,
    pub is_heading: bool,
    /// A run in a table cell: a span can't carry text-align (#281).
    pub no_align: bool,
}

impl Default for Style157 {
    fn default() -> Self {
        Style157 {
            font_size: 1.0,
            bold: false,
            italic: false,
            upright: false,
            underline: false,
            strike: false,
            margin_top: None,
            margin_left: None,
            baseline_style: None,
            is_heading: false,
            no_align: false,
        }
    }
}

impl Style157 {
    pub(crate) fn body(font_size: f64) -> Self {
        Style157 {
            font_size,
            ..Style157::default()
        }
    }
}

/// The arguments of one of the builders `_allocate_style` calls.
#[derive(Clone, PartialEq)]
pub(crate) enum StyleArgs {
    Text(Style157),
    Table(TableStyle),
    Cell(CellStyle),
    Part(PartStyle),
}

impl From<Style157> for StyleArgs {
    fn from(s: Style157) -> Self {
        StyleArgs::Text(s)
    }
}

/// `_allocate_style` (:6941): one `$157` per kind and arguments, named `s<n><kind>`.
#[derive(Default)]
pub(crate) struct Styles {
    pub cache: Vec<(&'static str, StyleArgs, String)>,
    counts: Vec<(&'static str, usize)>,
}

impl Styles {
    pub(crate) fn allocate(
        &mut self,
        g: &mut Generator,
        kind: &'static str,
        style: impl Into<StyleArgs>,
    ) -> String {
        let style = style.into();
        if let Some((_, _, name)) = self
            .cache
            .iter()
            .find(|(k, s, _)| *k == kind && *s == style)
        {
            return name.clone();
        }
        let idx = match self.counts.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => {
                *n += 1;
                *n - 1
            }
            None => {
                self.counts.push((kind, 1));
                0
            }
        };
        let name = format!("s{idx}{kind}");
        let value = match &style {
            StyleArgs::Text(s) => build_fragment_157(g, &name, s),
            StyleArgs::Table(s) => build_table_style_157(g, &name, s),
            StyleArgs::Cell(s) => build_cell_style_157(g, &name, s),
            StyleArgs::Part(s) => build_part_style_157(g, &name, s),
        };
        g.push(Fragment::new("$157", Some(&name), value));
        self.cache.push((kind, style, name.clone()));
        name
    }
}

/// `build_fragment_157` (:3568), for the arguments [`Style157`] holds.
fn build_fragment_157(g: &mut Generator, name: &str, style: &Style157) -> Value {
    g.local(name);
    let margin_left = style.margin_left.as_deref().unwrap_or("0.5");
    let Value::Struct(mut v) = st([
        ("$48", measure(py::decimal(margin_left), "$314")),
        ("$34", sym("$321")),
        ("$36", measure(py::decimal("0"), "$314")),
        // IonDecimal(str(line_height)), line_height 1.0
        ("$42", measure(py::decimal("1.0"), "$310")),
        ("$173", sym(name)),
        ("$13", sym(if style.bold { "$361" } else { "$350" })),
    ]) else {
        unreachable!()
    };
    if !style.is_heading {
        // f"{1 / line_height:.6g}", line_height 1.0
        v.insert(
            "$47",
            measure(
                Decimal {
                    coefficient: 1,
                    exponent: 0,
                },
                "$310",
            ),
        );
    }
    if style.font_size != 1.0 {
        v.insert(
            "$16",
            measure(py::decimal_of_float(style.font_size), "$505"),
        );
    }
    if style.underline {
        v.insert("$23", sym("$328"));
    }
    if style.strike {
        v.insert("$27", sym("$328"));
    }
    if style.italic {
        v.insert("$12", sym("$382"));
    } else if style.upright {
        v.insert("$12", sym("$350"));
    }
    if let Some(top) = style.margin_top {
        v.insert("$46", measure(py::decimal_of_float(top), "$310"));
    }
    if let Some(baseline) = style.baseline_style {
        v.insert("$44", sym(baseline));
    }
    if style.no_align {
        v.remove("$34");
    }
    Value::Struct(v)
}
