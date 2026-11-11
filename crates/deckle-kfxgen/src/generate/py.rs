// SPDX-License-Identifier: GPL-3.0-or-later
//! Python's string and number semantics where kfxgen's output depends on them. Strings are
//! indexed by code point (`char`), as Python indexes them. Character classes are Unicode 16.0:
//! the tables below from calibre's Python 3.14 `unicodedata`, categories from the `regex` crate
//! (`DIVERGENCES.md`).
use std::sync::LazyLock;

use deckle_kfx::ion::Decimal;
use regex::Regex;

/// `str.isspace()`: every character it is true for.
const SPACE: [char; 29] = [
    '\u{9}', '\u{A}', '\u{B}', '\u{C}', '\u{D}', '\u{1C}', '\u{1D}', '\u{1E}', '\u{1F}', '\u{20}',
    '\u{85}', '\u{A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
    '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}', '\u{2029}',
    '\u{202F}', '\u{205F}', '\u{3000}',
];

/// `str.isdigit()` beyond category Nd: superscripts, circled digits and the like.
const DIGIT_NOT_ND: [(u32, u32); 20] = [
    (0xB2, 0xB3),
    (0xB9, 0xB9),
    (0x1369, 0x1371),
    (0x19DA, 0x19DA),
    (0x2070, 0x2070),
    (0x2074, 0x2079),
    (0x2080, 0x2089),
    (0x2460, 0x2468),
    (0x2474, 0x247C),
    (0x2488, 0x2490),
    (0x24EA, 0x24EA),
    (0x24F5, 0x24FD),
    (0x24FF, 0x24FF),
    (0x2776, 0x277E),
    (0x2780, 0x2788),
    (0x278A, 0x2792),
    (0x10A40, 0x10A43),
    (0x10E60, 0x10E68),
    (0x11052, 0x1105A),
    (0x1F100, 0x1F10A),
];

/// `unicodedata.east_asian_width(c) in ("W", "F")`.
#[rustfmt::skip]
const EAST_ASIAN_WIDE: [(u32, u32); 122] = [
    (0x1100, 0x115F), (0x231A, 0x231B), (0x2329, 0x232A), (0x23E9, 0x23EC), (0x23F0, 0x23F0),
    (0x23F3, 0x23F3), (0x25FD, 0x25FE), (0x2614, 0x2615), (0x2630, 0x2637), (0x2648, 0x2653),
    (0x267F, 0x267F), (0x268A, 0x268F), (0x2693, 0x2693), (0x26A1, 0x26A1), (0x26AA, 0x26AB),
    (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE), (0x26D4, 0x26D4), (0x26EA, 0x26EA),
    (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD), (0x2705, 0x2705),
    (0x270A, 0x270B), (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E), (0x2753, 0x2755),
    (0x2757, 0x2757), (0x2795, 0x2797), (0x27B0, 0x27B0), (0x27BF, 0x27BF), (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x2E80, 0x2E99), (0x2E9B, 0x2EF3), (0x2F00, 0x2FD5),
    (0x2FF0, 0x303E), (0x3041, 0x3096), (0x3099, 0x30FF), (0x3105, 0x312F), (0x3131, 0x318E),
    (0x3190, 0x31E5), (0x31EF, 0x321E), (0x3220, 0x3247), (0x3250, 0xA48C), (0xA490, 0xA4C6),
    (0xA960, 0xA97C), (0xAC00, 0xD7A3), (0xF900, 0xFAFF), (0xFE10, 0xFE19), (0xFE30, 0xFE52),
    (0xFE54, 0xFE66), (0xFE68, 0xFE6B), (0xFF01, 0xFF60), (0xFFE0, 0xFFE6), (0x16FE0, 0x16FE4),
    (0x16FF0, 0x16FF1), (0x17000, 0x187F7), (0x18800, 0x18CD5), (0x18CFF, 0x18D08),
    (0x1AFF0, 0x1AFF3), (0x1AFF5, 0x1AFFB), (0x1AFFD, 0x1AFFE), (0x1B000, 0x1B122),
    (0x1B132, 0x1B132), (0x1B150, 0x1B152), (0x1B155, 0x1B155), (0x1B164, 0x1B167),
    (0x1B170, 0x1B2FB), (0x1D300, 0x1D356), (0x1D360, 0x1D376), (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF), (0x1F18E, 0x1F18E), (0x1F191, 0x1F19A), (0x1F200, 0x1F202),
    (0x1F210, 0x1F23B), (0x1F240, 0x1F248), (0x1F250, 0x1F251), (0x1F260, 0x1F265),
    (0x1F300, 0x1F320), (0x1F32D, 0x1F335), (0x1F337, 0x1F37C), (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA), (0x1F3CF, 0x1F3D3), (0x1F3E0, 0x1F3F0), (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E), (0x1F440, 0x1F440), (0x1F442, 0x1F4FC), (0x1F4FF, 0x1F53D),
    (0x1F54B, 0x1F54E), (0x1F550, 0x1F567), (0x1F57A, 0x1F57A), (0x1F595, 0x1F596),
    (0x1F5A4, 0x1F5A4), (0x1F5FB, 0x1F64F), (0x1F680, 0x1F6C5), (0x1F6CC, 0x1F6CC),
    (0x1F6D0, 0x1F6D2), (0x1F6D5, 0x1F6D7), (0x1F6DC, 0x1F6DF), (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC), (0x1F7E0, 0x1F7EB), (0x1F7F0, 0x1F7F0), (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945), (0x1F947, 0x1F9FF), (0x1FA70, 0x1FA7C), (0x1FA80, 0x1FA89),
    (0x1FA8F, 0x1FAC6), (0x1FACE, 0x1FADC), (0x1FADF, 0x1FAE9), (0x1FAF0, 0x1FAF8),
    (0x20000, 0x2FFFD), (0x30000, 0x3FFFD),
];

fn in_ranges(c: char, ranges: &[(u32, u32)]) -> bool {
    let c = u32::from(c);
    ranges.iter().any(|&(a, b)| (a..=b).contains(&c))
}

fn matches(re: &Regex, c: char) -> bool {
    re.is_match(c.encode_utf8(&mut [0; 4]))
}

/// Python's `isalpha` is exactly category L, and `isalnum` L or N.
static ALPHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").unwrap());
static ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}]$").unwrap());
static DIGIT_ND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Nd}$").unwrap());

pub(crate) fn is_space(c: char) -> bool {
    SPACE.contains(&c)
}

pub(crate) fn is_alpha(c: char) -> bool {
    matches(&ALPHA, c)
}

pub(crate) fn is_alnum(c: char) -> bool {
    matches(&ALNUM, c)
}

pub(crate) fn is_digit(c: char) -> bool {
    matches(&DIGIT_ND, c) || in_ranges(c, &DIGIT_NOT_ND)
}

pub(crate) fn is_east_asian_wide(c: char) -> bool {
    in_ranges(c, &EAST_ASIAN_WIDE)
}

/// `s.strip()`.
pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.lstrip()`.
pub(crate) fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `"".join(s.split())`.
pub(crate) fn without_spaces(s: &str) -> String {
    s.chars().filter(|&c| !is_space(c)).collect()
}

/// `len(s)`.
pub(crate) fn len(s: &str) -> usize {
    s.chars().count()
}

/// `s[n:]`.
pub(crate) fn skip(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or("", |(i, _)| &s[i..])
}

/// `s[:n]`.
pub(crate) fn take(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// `round(x, digits)`: both round the float's exact value half to even.
pub(crate) fn round(x: f64, digits: usize) -> f64 {
    format!("{x:.digits$}").parse().unwrap_or(x)
}

/// `Decimal(text)` for the decimal text kfxgen writes: `"0.5"`, `"2.0"`, `"1e-05"`.
pub(crate) fn decimal(text: &str) -> Decimal {
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (text, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    Decimal {
        coefficient: format!("{int}{frac}").parse().unwrap_or(0),
        exponent: exponent - frac.len() as i32,
    }
}

/// `f"{x:.{precision}g}"`: `precision` significant digits, no trailing zeros, an exponent
/// below 1e-4 or from 10**precision.
pub(crate) fn format_g(x: f64, precision: usize) -> String {
    let p = precision.max(1);
    let strip = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s
        }
    };
    let sci = format!("{:.*e}", p - 1, x);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if exp < -4 || exp >= p as i32 {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa.to_owned()), exp.abs())
    } else {
        strip(format!("{:.*}", (p as i32 - 1 - exp) as usize, x))
    }
}

/// `Decimal(repr(x)).quantize(Decimal("0.001"), rounding=ROUND_HALF_UP).normalize()`.
pub(crate) fn quantize_milli(x: f64) -> Decimal {
    let d = decimal(&format!("{x}"));
    let mut coefficient = if d.exponent >= -3 {
        d.coefficient * 10i128.pow((d.exponent + 3) as u32)
    } else {
        let div = 10i128.pow((-3 - d.exponent) as u32);
        let (q, r) = (d.coefficient / div, d.coefficient % div);
        // Half up: a tie goes away from zero.
        q + (2 * r.abs() >= div) as i128 * d.coefficient.signum()
    };
    if coefficient == 0 {
        return Decimal {
            coefficient: 0,
            exponent: 0,
        };
    }
    let mut exponent = -3;
    while coefficient % 10 == 0 {
        coefficient /= 10;
        exponent += 1;
    }
    Decimal {
        coefficient,
        exponent,
    }
}

/// `Decimal(str(x))`. Rust prints a float's shortest round-trip digits, as `repr` does, but
/// without an exponent: the same decimal below 1e16, which no style reaches.
pub(crate) fn decimal_of_float(x: f64) -> Decimal {
    let mut text = format!("{x}");
    if !text.contains('.') && x.is_finite() {
        // repr(2.0) is "2.0", and Decimal keeps that zero.
        text.push_str(".0");
    }
    decimal(&text)
}
