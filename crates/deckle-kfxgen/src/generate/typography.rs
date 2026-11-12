// SPDX-License-Identifier: GPL-3.0-only
//! A paragraph's and a run's typography in a `$157`: small caps, case, letter spacing and
//! colour (#239), text-orientation and emphasis marks (#247), a link's own colour (#728),
//! a text colour measured against the page it is drawn on (#383), and drop caps (#240).
use deckle_kfx::ion::{Struct, Value};

use super::block_styles::{Length, Sizes, read_length};
use super::chunks::Chunk;
use super::inline::{Flag, run_font_size};
use super::{GenerateError, measure, model_err, py, sym};
use crate::pyvalue::PyValue;

/// `_write_typography`'s arguments (`native_generator.py:1023`): a block style's
/// `TYPOGRAPHY_KEYS` or a run's `TYPOGRAPHY_FLAGS` (`inline_style.py:551`), each as given.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Typography {
    pub small_caps: Option<String>,
    pub text_transform: Option<String>,
    pub letter_spacing: Option<Length>,
    /// ARGB.
    pub colour: Option<i64>,
    pub text_orientation: Option<String>,
    pub text_emphasis: Option<String>,
}

impl Typography {
    /// One of a block style's `TYPOGRAPHY_KEYS` (`inline_style.py:560`), not None: false
    /// when `key` is none of them.
    pub(crate) fn read_key(
        &mut self,
        key: &str,
        v: &PyValue,
        at: &str,
    ) -> Result<bool, GenerateError> {
        let string = || {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| model_err(format!("{at}.{key}: a string")))
        };
        match key {
            "small_caps" => self.small_caps = Some(string()?),
            "text_transform" => self.text_transform = Some(string()?),
            "text_orientation" => self.text_orientation = Some(string()?),
            "text_emphasis" => self.text_emphasis = Some(string()?),
            "letter_spacing" => {
                self.letter_spacing = Some(read_length(v).ok_or_else(|| {
                    model_err(format!(
                        "{at}.{key}: (a decimal of at most 38 digits, unit)"
                    ))
                })?);
            }
            "colour" => {
                self.colour = Some(
                    v.as_int()
                        .ok_or_else(|| model_err(format!("{at}.{key}: an ARGB int")))?,
                );
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// A run's typography flags (`_emphasis_style`, :7308).
    pub(crate) fn of_flags<'a>(flags: impl IntoIterator<Item = &'a Flag>) -> Self {
        let mut t = Typography::default();
        for f in flags {
            match f {
                Flag::SmallCaps(v) => t.small_caps = Some(v.clone()),
                Flag::Transform(v) => t.text_transform = Some(v.clone()),
                Flag::Spacing(v) => t.letter_spacing = Some(v.clone()),
                Flag::Colour(v) => t.colour = Some(*v),
                Flag::Orientation(v) => t.text_orientation = Some(v.clone()),
                Flag::EmphasisMarks(v) => t.text_emphasis = Some(v.clone()),
                _ => {}
            }
        }
        t
    }

    /// `_write_typography` (:1023): as Kindle Previewer writes them.
    pub(crate) fn write(&self, v: &mut Struct) {
        if let Some(caps) = self.small_caps.as_deref().and_then(small_caps_symbol) {
            v.insert("$583", sym(caps));
        }
        if let Some(t) = self.text_transform.as_deref().and_then(transform_symbol) {
            v.insert("$41", sym(t));
        }
        if let Some((magnitude, unit)) = &self.letter_spacing {
            v.insert("$32", measure(py::decimal(magnitude), unit));
        }
        if let Some(colour) = self.colour {
            v.insert("$19", Value::Int(colour));
        }
        if let Some(o) = self
            .text_orientation
            .as_deref()
            .and_then(orientation_symbol)
        {
            v.insert("$706", sym(o));
        }
        if let Some(e) = self.text_emphasis.as_deref() {
            if let Some(mark) = emphasis_symbol(e) {
                v.insert("$717", sym(mark));
            } else if e.starts_with('"') {
                // A mark of the book's own, as the string (QA's P748).
                let inner: String = e
                    .chars()
                    .skip(1)
                    .take(py::len(e).saturating_sub(2))
                    .collect();
                v.insert("$717", Value::string(&inner));
            }
        }
    }
}

/// `_SMALL_CAPS_SYMBOLS` (:927).
fn small_caps_symbol(value: &str) -> Option<&'static str> {
    match value {
        "small-caps" => Some("$369"),
        "normal" => Some("$349"),
        _ => None,
    }
}

/// `_TRANSFORM_SYMBOLS` (:928).
fn transform_symbol(value: &str) -> Option<&'static str> {
    match value {
        "uppercase" => Some("$372"),
        "lowercase" => Some("$373"),
        "capitalize" => Some("$374"),
        "none" => Some("$349"),
        _ => None,
    }
}

/// `_ORIENTATION_SYMBOLS` (:937).
fn orientation_symbol(value: &str) -> Option<&'static str> {
    match value {
        "upright" => Some("$779"),
        "sideways" => Some("$778"),
        "mixed" => Some("$383"),
        _ => None,
    }
}

/// `_EMPHASIS_SYMBOLS` (:1006): text-emphasis-style as kfxlib maps it.
fn emphasis_symbol(value: &str) -> Option<&'static str> {
    Some(match value {
        "filled" => "$724",
        "open" => "$725",
        "filled dot" => "$726",
        "open dot" => "$727",
        "filled circle" => "$728",
        "open circle" => "$729",
        "filled double-circle" => "$730",
        "open double-circle" => "$731",
        "filled triangle" => "$732",
        "open triangle" => "$733",
        "filled sesame" => "$734",
        "open sesame" => "$735",
        _ => return None,
    })
}

/// A run's `("small-caps" | "transform" | "spacing" | "colour", value)` (#239): a CSS
/// keyword, a KFX length or an ARGB int.
pub(crate) fn read_typography_flag(
    kind: &str,
    v: &PyValue,
    at: &str,
) -> Result<Flag, GenerateError> {
    let err = |what: &str| model_err(format!("{at}: a run's {kind}, {what}"));
    Ok(match kind {
        "small-caps" => Flag::SmallCaps(v.as_str().ok_or_else(|| err("a string"))?.to_owned()),
        "transform" => Flag::Transform(v.as_str().ok_or_else(|| err("a string"))?.to_owned()),
        "spacing" => Flag::Spacing(
            read_length(v).ok_or_else(|| err("(a decimal of at most 38 digits, unit)"))?,
        ),
        _ => Flag::Colour(v.as_int().ok_or_else(|| err("an ARGB int"))?),
    })
}

/// `LINK_COLOUR_FLAG`'s value (`inline_style.py:573`): `(its ARGB, or None when Previewer
/// writes none; its ARGB as text, for a link whose target isn't found)`.
pub(crate) fn read_link_colour(v: &PyValue, at: &str) -> Result<Flag, GenerateError> {
    match v.as_list() {
        Some([PyValue::None, PyValue::Int(text)]) => Ok(Flag::LinkColour(None, *text)),
        Some([PyValue::Int(link), PyValue::Int(text)]) => Ok(Flag::LinkColour(Some(*link), *text)),
        _ => Err(model_err(format!(
            "{at}: a link colour, (an ARGB int or None, an ARGB int)"
        ))),
    }
}

/// `inline_style._luminance` (:735): WCAG relative luminance of channels 0 to 255.
fn luminance([r, g, b]: [f64; 3]) -> f64 {
    let lin = |c: f64| {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// `_on_page` (`native_generator.py:1074`): a text colour as it is drawn on the white page,
/// unchanged when it has 4.5:1 contrast there, else darkened to it keeping its hue
/// (`css_text_colour` and `_scaled`'s darkening, `inline_style.py:752`, :782). The alpha is
/// written opaque.
pub(crate) fn on_page(argb: i64) -> i64 {
    // `_MAX_LUMINANCE` (:749): the luminance of 4.5:1 against white.
    let target = 1.05 / 4.5 - 0.05;
    let ch = [16, 8, 0].map(|shift| ((argb >> shift) & 0xFF) as f64);
    let ch = if luminance(ch) > target {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            if luminance(ch.map(|c| c * mid)) > target {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        ch.map(|c| (c * lo).trunc())
    } else {
        ch
    };
    0xFF00_0000 | ((ch[0] as i64) << 16) | ((ch[1] as i64) << 8) | ch[2] as i64
}

/// `_drop_cap` (:7213): (lines, characters) for a paragraph opening on a drop cap, or None.
/// Kindle Previewer's lines follow the initial's size against the main text; below 1.5 it
/// makes none (#240).
pub(crate) fn drop_cap(sizes: &Sizes, chunk: &Chunk) -> Result<Option<(i64, i64)>, GenerateError> {
    let Some(n) = chunk
        .style
        .as_ref()
        .and_then(|b| b.drop_cap)
        .filter(|&n| n != 0)
    else {
        return Ok(None);
    };
    // `text[n:]`, counting from the end for a negative `n`.
    let len = py::len(&chunk.text) as i64;
    let from = if n < 0 { (len + n).max(0) } else { n.min(len) };
    if py::strip(py::skip(&chunk.text, from as usize)).is_empty() {
        return Ok(None);
    }
    let mut px = None;
    for sp in &chunk.spans {
        // `s <= n - 1 < s + length`.
        if sp.start < n && n <= sp.start + sp.len {
            px = run_font_size(&sp.flags);
        }
    }
    let size = px.map_or(0.0, |px| sizes.rem(Some(px)));
    if size < 1.5 {
        return Ok(None);
    }
    let lines = 2.0 + 0.55 * size;
    // `int(...)`: Python's has no bound, and raises on infinity and NaN.
    if lines.is_nan() || lines >= 2f64.powi(63) {
        return Err(model_err(
            "a drop cap's initial at a size of fewer than 2^63 lines",
        ));
    }
    Ok(Some((lines as i64, n)))
}
