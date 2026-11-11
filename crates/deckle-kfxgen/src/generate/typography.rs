// SPDX-License-Identifier: GPL-3.0-or-later
//! Colour in a `$157`: a link's own colour (#728) and a text colour measured against the page
//! it is drawn on (#383).
use super::inline::Flag;
use super::{GenerateError, model_err};
use crate::pyvalue::PyValue;

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
