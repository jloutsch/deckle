// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's `compute_block_style` (`inline_style.py`, with `_length_percent`,
//! `_line_height_factor` and `_block_typography`) at kfxgen `ad38ec8`: an element's resolved
//! CSS as the `block_style` dict of the model.

use super::font_table::{parse_style, parse_weight};
use super::inline_style::{
    BLACK, Raised, TYPOGRAPHY_KEYS, css_font_size, css_typography, parse_css_length,
    parse_font_family,
};
use super::resolver::{Resolved, Resolver};
use crate::css;
use crate::generate::page_scale::em_percent;
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `ALIGN_MAP`'s keywords.
const ALIGN: [&str; 4] = ["left", "right", "center", "justify"];
/// `BOLD_WEIGHT_THRESHOLD` (`font_table.py`).
const BOLD_WEIGHT: i64 = 600;

/// What Python raises for `(css.get("text-align") or "").strip()` when the computed value is a
/// number other than 0: Calibre's `Style[prop]` reads `text-align: 5px` as points.
const ALIGN_NUMBER: Raised = Raised {
    exception: "AttributeError",
    note: "compute_block_style: a text-align Calibre reads as a number has no strip()",
};
const MAGNITUDE: Raised = Raised {
    exception: "ValueError",
    note: "_length_percent: a magnitude float() refuses",
};

/// The keys whose text `compute_block_style` reads, each a constant number of times.
const READ: [&str; 13] = [
    "text-align",
    "text-indent",
    "margin-left",
    "margin-right",
    "font-family",
    "font-weight",
    "font-style",
    "font-variant",
    "text-transform",
    "letter-spacing",
    "color",
    "text-orientation",
    "text-emphasis-style",
];

/// Why `compute_block_style` gives no block style.
#[derive(Debug)]
pub enum StyleError {
    /// kfxgen raises here: it leaves the spine item out (`extract_chapters_from_oeb`).
    Raised(Raised),
    /// The text read passed the resolver's budget ([`Resolver::charge`]): the file is refused.
    TooLarge(css::Error),
}

impl From<Raised> for StyleError {
    fn from(e: Raised) -> Self {
        StyleError::Raised(e)
    }
}

impl From<css::Error> for StyleError {
    fn from(e: css::Error) -> Self {
        StyleError::TooLarge(e)
    }
}

/// `compute_block_style(css)`: align, indent, margins, font family, line height, emphasis,
/// font size, then each typography value that is set (black left out). Raises where kfxgen
/// does: a length `parse_css_length` raises on, or a text-align that is a number. The text it
/// parses is charged to `resolver`'s budget first.
pub fn compute_block_style(resolver: &Resolver, css: &Resolved) -> Result<PyValue, StyleError> {
    for key in READ {
        if let Some(PyValue::Str(v)) = css.get(key) {
            resolver.charge(v.len())?;
        }
    }
    let raw_align = match css.get("text-align") {
        Some(PyValue::Str(s)) => py::strip(s).to_lowercase(),
        Some(v) if v.is_truthy() => return Err(ALIGN_NUMBER.into()),
        _ => String::new(),
    };
    let raw_align = match raw_align.as_str() {
        "start" => "left",
        "end" => "right",
        "-webkit-center" => "center",
        a => a,
    };
    let align = ALIGN.contains(&raw_align).then(|| raw_align.to_owned());
    let text_indent = text(css.get("text-indent"));
    let mut indent = parse_css_length(text_indent, false)?;
    let margin_left = parse_css_length(text(css.get("margin-left")), false)?;
    if indent.is_none()
        && let Some(margin) = &margin_left
    {
        // A hanging indent within its left margin, as Previewer writes it (#465): one past it
        // is clamped to it. With no margin it would pull the first line off the page.
        let hang = parse_css_length(text_indent, true)?;
        if let (Some(hang_pct), Some(margin_pct)) =
            (length_percent(&hang)?, length_percent(&margin_left)?)
        {
            indent = if -hang_pct <= margin_pct {
                hang
            } else {
                Some((format!("-{}", margin.0), margin.1))
            };
        }
    }
    let margin_right = parse_css_length(text(css.get("margin-right")), false)?;
    let weight = css.get("font-weight");
    let bold = weight.is_some_and(|w| match w {
        PyValue::Str(s) => py::strip(s).to_lowercase() == "bolder",
        _ => false,
    }) || parse_weight(weight) >= BOLD_WEIGHT;
    let s = |k: &str| PyValue::Str(k.to_owned());
    let length = |l: Option<(String, &'static str)>| {
        l.map_or(PyValue::None, |(mag, unit)| {
            PyValue::Tuple(vec![PyValue::Str(mag), s(unit)])
        })
    };
    let mut out = vec![
        (s("align"), align.map_or(PyValue::None, PyValue::Str)),
        (s("indent"), length(indent)),
        (s("margin_left"), length(margin_left)),
        (s("margin_right"), length(margin_right)),
        (s("font_family"), font_family(css.get("font-family"))),
        (s("line_height"), line_height_factor(css.get("line-height"))),
        (s("bold"), PyValue::Bool(bold)),
        (
            s("italic"),
            PyValue::Bool(parse_style(css.get("font-style"))),
        ),
        (
            s("font_size"),
            css_font_size(|k| css.get(k))
                .cloned()
                .unwrap_or(PyValue::None),
        ),
    ];
    // `_block_typography`: each only when set; black, the reader's own, isn't.
    let t = css_typography(|k| css.get(k))?;
    let values = [
        t.small_caps.map(s),
        t.transform.map(s),
        t.spacing.map(|l| length(Some(l))),
        t.colour.filter(|&c| c != BLACK).map(PyValue::Int),
        t.orientation.map(s),
        t.emphasis.map(PyValue::Str),
    ];
    for (key, value) in TYPOGRAPHY_KEYS.into_iter().zip(values) {
        if let Some(v) = value {
            out.push((s(key), v));
        }
    }
    Ok(PyValue::Dict(out))
}

/// `css.get(key) or ""` for a value the resolver gives as a str or None.
fn text(v: Option<&PyValue>) -> &str {
    match v {
        Some(PyValue::Str(s)) => s,
        _ => "",
    }
}

/// `_length_percent`: a parsed length as the % Previewer writes it, or None.
fn length_percent(length: &Option<(String, &'static str)>) -> Result<Option<f64>, Raised> {
    let Some((mag, unit)) = length else {
        return Ok(None);
    };
    let em = em_percent();
    let per = match *unit {
        "$308" | "$505" => em,
        "$318" => em / 12.0,
        "$319" => 0.45 * em / 12.0,
        "$316" => 72.0 / 25.4 * em / 12.0,
        "$314" => 1.0,
        _ => return Ok(None),
    };
    Ok(Some(py::py_float(mag).ok_or(MAGNITUDE)? * per))
}

/// `_parse_font_family(css.get("font-family"))`: the names, lower case; none for no value.
fn font_family(v: Option<&PyValue>) -> PyValue {
    let names = match v {
        Some(PyValue::Str(s)) => parse_font_family(s),
        // Calibre's `Style[prop]` reads `font-family: 12px` as points; Python takes its str().
        Some(PyValue::Float(f)) if *f != 0.0 => parse_font_family(&py::repr_float(*f)),
        _ => Vec::new(),
    };
    PyValue::List(names.into_iter().map(PyValue::Str).collect())
}

/// `_line_height_factor(value)` (`inline_style.py`): a resolved factor above 0, or None.
fn line_height_factor(v: Option<&PyValue>) -> PyValue {
    let factor = match v {
        Some(PyValue::Float(f)) => Some(*f),
        Some(PyValue::Str(s)) => py::py_float(s),
        _ => None,
    };
    factor
        .filter(|&f| f > 0.0)
        .map_or(PyValue::None, PyValue::Float)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::File;
    use crate::css::{ClassNamer, DEFAULT_PROFILE, cascade, flatten, prepare_body};
    use crate::generate::page_scale::{HORIZONTAL_PT, VERTICAL_PT, page_width};

    /// A page with nothing in it, for a resolver's budget.
    fn blank() -> File {
        let mut dom = crate::dom::parse(
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head><body/></html>",
        )
        .unwrap();
        prepare_body(&mut dom);
        let first = cascade(&mut dom, "a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
        let flat = flatten(
            &mut dom,
            "a.xhtml",
            first,
            &mut ClassNamer::default(),
            &DEFAULT_PROFILE,
        )
        .unwrap();
        File {
            href: "a.xhtml".into(),
            dom,
            flat,
            first_pass_rules: Vec::new(),
        }
    }

    /// `compute_block_style(css)`.
    fn compute(css: Vec<(&'static str, PyValue)>) -> Result<PyValue, StyleError> {
        let file = blank();
        let resolver = Resolver::new(&file, &DEFAULT_PROFILE);
        compute_block_style(&resolver, &Resolved::of(css))
    }

    fn s(v: &str) -> PyValue {
        PyValue::Str(v.to_owned())
    }

    fn length(mag: &str, unit: &str) -> PyValue {
        PyValue::Tuple(vec![s(mag), s(unit)])
    }

    /// `key` of `compute_block_style(css)`.
    fn block(css: Vec<(&'static str, PyValue)>, key: &str) -> PyValue {
        compute(css).unwrap().get(key).unwrap().clone()
    }

    /// Expected values from the pinned `inline_style.py`, run under `page_width(pt)`.
    #[test]
    fn a_hanging_indent_is_clamped_to_its_margin_on_the_books_page() {
        let indent = |indent: &str, margin: &str| {
            block(
                vec![("text-indent", s(indent)), ("margin-left", s(margin))],
                "indent",
            )
        };
        {
            let _page = page_width(HORIZONTAL_PT);
            // 2em is 6.25% of a 384pt page: past a 5% margin.
            assert_eq!(indent("-2em", "5%"), length("-5", "$314"));
            assert_eq!(indent("-1em", "2em"), length("-1", "$308"));
            assert_eq!(indent("-3em", "2em"), length("-2", "$308"));
            assert_eq!(indent("-20px", "1em"), length("-20", "$319"));
            assert_eq!(indent("-2em", "1in"), length("-2", "$308"));
        }
        let _page = page_width(VERTICAL_PT);
        // 2em is 5% of a 480pt page.
        assert_eq!(indent("-2em", "5%"), length("-2", "$308"));
        assert_eq!(indent("-3em", "2em"), length("-2", "$308"));
    }

    #[test]
    fn values_of_other_types_as_python_reads_them() {
        let css = || {
            vec![
                ("text-align", s(" END ")),
                ("font-family", PyValue::Float(9.0)),
                ("line-height", PyValue::Float(-1.0)),
                ("font-weight", s("bolder")),
                ("font-style", s("Oblique")),
                ("font-size", PyValue::Float(13.28)),
                ("color", s("#000")),
                ("letter-spacing", s("0.1em")),
                ("font-variant", s("small-caps")),
                ("background-behind", PyValue::Int(0)),
            ]
        };
        let got = compute(css()).unwrap();
        let want = [
            ("align", s("right")),
            ("indent", PyValue::None),
            ("margin_left", PyValue::None),
            ("margin_right", PyValue::None),
            ("font_family", PyValue::List(vec![s("9.0")])),
            ("line_height", PyValue::None),
            ("bold", PyValue::Bool(true)),
            ("italic", PyValue::Bool(true)),
            ("font_size", PyValue::Float(13.28)),
            ("small_caps", s("small-caps")),
            ("letter_spacing", length("0.1", "$308")),
            // Black on a black box, lightened.
            ("colour", PyValue::Int(4285887861)),
        ];
        assert_eq!(
            got,
            PyValue::Dict(want.into_iter().map(|(k, v)| (s(k), v)).collect())
        );
        // A text-align or font family of 0.0 is falsy; another number has no strip().
        let zero = vec![
            ("text-align", PyValue::Float(0.0)),
            ("font-family", PyValue::Float(0.0)),
        ];
        assert_eq!(block(zero.clone(), "align"), PyValue::None);
        assert_eq!(block(zero, "font_family"), PyValue::List(Vec::new()));
        let raised = compute(vec![("text-align", PyValue::Float(3.75))]);
        assert!(
            matches!(&raised, Err(StyleError::Raised(r)) if r.exception == "AttributeError"),
            "{raised:?}"
        );
        // Grey on a dark box keeps its colour key.
        let grey = vec![
            ("color", s("#777")),
            ("background-behind", PyValue::Int(0x333333)),
        ];
        assert_eq!(block(grey, "colour"), PyValue::Int(4288387995));
    }
}
