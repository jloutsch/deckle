// SPDX-License-Identifier: GPL-3.0-or-later
//! The `$157` paragraph styles: [`Style157`] holds `build_fragment_157`'s arguments, [`Styles`]
//! allocates one fragment per distinct set.
use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::Value;

use super::block_styles::Length;
use super::boxes::{BoxStyle, RuleStyle, build_box_style_157, build_rule_style_157};
use super::table_styles::{
    CellStyle, PartStyle, TableStyle, align_symbol, build_cell_style_157, build_part_style_157,
    build_table_style_157,
};
use super::typography::{Typography, on_page};
use super::writing::{BidiKeys, bidi_value, rotate_margins};
use super::{Generator, measure, py, st, sym};

/// `_LINE_EM` (:1055): one line of space, in ems.
pub(crate) const LINE_EM: f64 = 1.2;

/// `build_fragment_157`'s arguments that the ported paths set, defaulting as it does. Two
/// styles are one when their arguments are, as `_allocate_style` keys them (:6950).
#[derive(Clone, PartialEq)]
pub(crate) struct Style157 {
    pub font_size: f64,
    pub line_height: f64,
    pub bold: bool,
    pub italic: bool,
    pub upright: bool,
    pub underline: bool,
    pub strike: bool,
    pub margin_top: Option<f64>,
    pub align: Option<String>,
    pub text_indent: Option<Length>,
    pub margin_left: Option<Length>,
    pub margin_right: Option<Length>,
    pub font_family: Option<String>,
    pub baseline_style: Option<&'static str>,
    pub is_heading: bool,
    /// A run in a table cell: a span can't carry text-align (#281).
    pub no_align: bool,
    /// In ems: none written for 0, kfxgen's own 1lh for None (#230, #331).
    pub space_above: Option<f64>,
    pub space_below: Option<f64>,
    /// A size of 1 written, not left to the container's (#241, #681). None: not given, a
    /// style apart from one given False, as `_allocate_style` keys them.
    pub explicit_size: Option<bool>,
    /// The lines a drop cap drops through and its characters (#240).
    pub drop_cap: Option<(i64, i64)>,
    pub typography: Typography,
    /// ARGB: a link's own colour (#728).
    pub link_colour: Option<i64>,
}

impl Default for Style157 {
    fn default() -> Self {
        Style157 {
            font_size: 1.0,
            line_height: 1.0,
            bold: false,
            italic: false,
            upright: false,
            underline: false,
            strike: false,
            margin_top: None,
            align: None,
            text_indent: None,
            margin_left: None,
            margin_right: None,
            font_family: None,
            baseline_style: None,
            is_heading: false,
            no_align: false,
            space_above: None,
            space_below: None,
            explicit_size: None,
            drop_cap: None,
            typography: Typography::default(),
            link_colour: None,
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
    Box(BoxStyle),
    Rule(RuleStyle),
    /// A run's direction keys alone (#710).
    Direction,
}

impl From<Style157> for StyleArgs {
    fn from(s: Style157) -> Self {
        StyleArgs::Text(s)
    }
}

/// `_allocate_style` (:6941): one `$157` per kind, arguments, direction keys and rotation,
/// named `s<n><kind>`.
#[derive(Default)]
pub(crate) struct Styles {
    pub cache: Vec<(&'static str, StyleArgs, BidiKeys, bool, String)>,
    counts: Vec<(&'static str, usize)>,
    /// `rotation`: styles are written for a vertical-rl chapter, their margins moved (#247).
    pub vertical: bool,
    /// `open_fills`: the fill of each box and cell open around the styles written now (#383).
    pub fills: Vec<Option<i64>>,
}

impl Styles {
    pub(crate) fn allocate(
        &mut self,
        g: &mut Generator,
        kind: &'static str,
        style: impl Into<StyleArgs>,
    ) -> String {
        self.allocate_in(g, kind, style, &[])
    }

    /// With the direction and language `_bidi_diff` (:6927) gives it, written last.
    pub(crate) fn allocate_in(
        &mut self,
        g: &mut Generator,
        kind: &'static str,
        style: impl Into<StyleArgs>,
        bidi: &[(&'static str, String)],
    ) -> String {
        let mut style = style.into();
        let colour = match &mut style {
            StyleArgs::Text(s) => Some(&mut s.typography.colour),
            StyleArgs::Cell(c) if c.background.is_none() => Some(&mut c.typography.colour),
            _ => None,
        };
        if let Some(Some(colour)) = colour
            && !self.fills.iter().any(|f| f.is_some_and(|f| f != 0))
        {
            // Text written on the page is measured against it: a colour already readable
            // there is unchanged (#383).
            *colour = on_page(*colour);
        }
        if let Some((.., name)) = self.cache.iter().find(|(k, s, b, v, _)| {
            *k == kind && *s == style && b[..] == *bidi && *v == self.vertical
        }) {
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
        let mut value = match &style {
            StyleArgs::Text(s) => build_fragment_157(g, &name, s),
            StyleArgs::Table(s) => build_table_style_157(g, &name, s),
            StyleArgs::Cell(s) => build_cell_style_157(g, &name, s),
            StyleArgs::Part(s) => build_part_style_157(g, &name, s),
            StyleArgs::Box(s) => build_box_style_157(g, &name, s),
            StyleArgs::Rule(s) => build_rule_style_157(g, &name, s),
            // `build_direction_style_157` (:2321).
            StyleArgs::Direction => {
                g.local(&name);
                st([("$173", sym(&name))])
            }
        };
        if let Value::Struct(v) = &mut value {
            if self.vertical {
                rotate_margins(v);
            }
            for (key, text) in bidi {
                v.insert(*key, bidi_value(key, text));
            }
        }
        g.push(Fragment::new("$157", Some(&name), value));
        self.cache
            .push((kind, style, bidi.to_vec(), self.vertical, name.clone()));
        name
    }
}

/// `measure(Decimal(f"{x:.6g}"), unit)`.
pub(crate) fn g6(x: f64, unit: &str) -> Value {
    measure(py::decimal(&py::format_g(x, 6)), unit)
}

fn length((magnitude, unit): &Length) -> Value {
    measure(py::decimal(magnitude), unit)
}

/// `build_fragment_157` (:3568), for the arguments [`Style157`] holds.
fn build_fragment_157(g: &mut Generator, name: &str, style: &Style157) -> Value {
    g.local(name);
    let text_align = style
        .align
        .as_deref()
        .and_then(align_symbol)
        .unwrap_or("$321");
    let zero_pct = || measure(py::decimal("0"), "$314");
    let Value::Struct(mut v) = st([
        (
            "$48",
            style
                .margin_left
                .as_ref()
                .map_or_else(|| measure(py::decimal("0.5"), "$314"), length),
        ),
        ("$34", sym(text_align)),
        (
            "$36",
            style.text_indent.as_ref().map_or_else(zero_pct, length),
        ),
        (
            "$42",
            measure(py::decimal_of_float(style.line_height), "$310"),
        ),
        ("$173", sym(name)),
        ("$13", sym(if style.bold { "$361" } else { "$350" })),
    ]) else {
        unreachable!()
    };
    let lh = style.line_height;
    if let Some(below) = style.space_below.filter(|&b| b != 0.0) {
        v.insert("$49", g6(below / LINE_EM / lh, "$310"));
    }
    match style.space_above {
        // 0: a line of verse set tight under the one before (#331).
        Some(above) => {
            if above != 0.0 {
                v.insert("$47", g6(above / LINE_EM / lh, "$310"));
            }
        }
        // A first-line indent replaces the space; a hanging one doesn't (#465).
        None if !style.is_heading
            && style
                .text_indent
                .as_ref()
                .is_none_or(|(m, _)| m.parse::<f64>().is_ok_and(|x| x < 0.0)) =>
        {
            v.insert("$47", g6(1.0 / lh, "$310"));
        }
        None => {}
    }
    if style.font_size != 1.0 || style.explicit_size == Some(true) {
        v.insert(
            "$16",
            measure(py::decimal_of_float(style.font_size), "$505"),
        );
    }
    if let Some((lines, chars)) = style.drop_cap {
        // The lines its initial drops through and the initial's characters, as Kindle
        // Previewer writes them (#240).
        v.insert("$125", Value::Int(lines));
        v.insert("$126", Value::Int(chars));
    }
    style.typography.write(&mut v);
    if let Some(link) = style.link_colour {
        // A link's own colour, as Previewer writes it: the colour of the link and of a
        // visited link (#728).
        for key in ["$576", "$577"] {
            v.insert(key, st([("$19", Value::Int(link))]));
        }
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
    if let Some(family) = &style.font_family {
        v.insert("$11", Value::string(family));
    }
    if let Some(top) = style.margin_top {
        v.insert("$46", measure(py::decimal_of_float(top), "$310"));
    }
    if let Some(right) = &style.margin_right {
        v.insert("$50", length(right));
    }
    if let Some(baseline) = style.baseline_style {
        v.insert("$44", sym(baseline));
    }
    if style.no_align {
        v.remove("$34");
    }
    Value::Struct(v)
}
