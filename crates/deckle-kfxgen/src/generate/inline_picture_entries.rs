// SPDX-License-Identifier: GPL-3.0-or-later
//! A picture among a paragraph's words in the storyline (`native_generator.py` at kfxgen
//! `4b81651`): its id (:6683), its `$259` entry (`_inline_picture_entry` :2419) and `s_ipic`
//! style (#529, #565), and the positions of the paragraph holding it (:3383).
use std::collections::BTreeMap;
use std::sync::LazyLock;

use deckle_kfx::ion::Value;
use regex::{Captures, Regex};

use super::chunks::{AmongWords, Chunk};
use super::images::{Dims, Images, SIZE_HINT};
use super::page_scale::{column_px, page_pt};
use super::writing;
use super::{GenerateError, Generator, measure, model_err, py, st, sym};

use GenerateError::Unsupported;

/// A picture among a paragraph's words: `_inline_pictures`' (offset, (resource, alt, size,
/// extra)), with its id (:6683) and its paragraph's size (:7835) once the chunks are made.
pub(crate) struct InlinePicture {
    pub at: i64,
    pub resource: String,
    pub alt: String,
    pub size: String,
    pub extra: String,
    pub eid: i64,
    pub font_size: f64,
}

/// Each picture among words, in reading order, an id of its own after the entries' (:6683),
/// and its paragraph's size (:7835).
pub(crate) fn number_pictures(
    chunks: &mut [Chunk],
    font_size: impl Fn(&Chunk) -> f64,
    next_id: &mut i64,
    step: i64,
) {
    for chunk in chunks.iter_mut().filter(|c| !c.pictures.is_empty()) {
        let size = font_size(chunk);
        for p in &mut chunk.pictures {
            p.eid = *next_id;
            p.font_size = size;
            *next_id += step;
        }
    }
}

/// An `s_ipic` style's key (:2447, :2505): a floated picture's (side, `$56`, its 100% cap,
/// sized, a % height), or another's (`$56`, `$57`, its cap, sized, `$44`, `$31`).
#[derive(PartialEq)]
enum StyleKey {
    Float(String, Option<String>, bool, bool, Option<String>),
    Inline(
        Option<String>,
        String,
        bool,
        bool,
        Option<&'static str>,
        Option<(String, &'static str)>,
    ),
}

/// The `s_ipic` styles of the pictures among words, made as their entries are (:2449), written
/// after the storylines' styles are pruned (:8133).
pub(crate) struct InlineStyles {
    dims: BTreeMap<String, Dims>,
    styles: Vec<(StyleKey, String, Value)>,
}

/// `_INLINE_VALIGN_SYMBOLS` (:245).
fn inline_valign_symbol(va: &str) -> Option<&'static str> {
    Some(match va {
        "middle" => "$320",
        "top" => "$58",
        "bottom" => "$60",
        "text-top" => "$447",
        "text-bottom" => "$449",
        "baseline" => "$350",
        "super" => "$370",
        "sub" => "$371",
        _ => return None,
    })
}

/// `_VALIGN_LENGTH_RE` (:258), its `$` also before a final line break.
static VALIGN_LENGTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A([-+]?(?:\d+(?:\.\d+)?|\.\d+))(px|pt|em|rem|%)?\n?\z").unwrap()
});

/// `float(text)` of a number a regex matched, which may be in other than ASCII digits.
fn number_of(text: &str) -> Result<f64, GenerateError> {
    if !text.is_ascii() {
        return Err(Unsupported("a picture size in other than ASCII digits"));
    }
    Ok(text.parse().unwrap())
}

/// `f"{x:g}"`, which kfxgen writes as a decimal: a finite one.
fn g(x: f64) -> Result<String, GenerateError> {
    if !x.is_finite() {
        return Err(model_err(
            "a picture among words: a size kfxgen can write as a number",
        ));
    }
    Ok(py::format_g(x, 6))
}

/// `f"{round(x, 5):g}"`.
fn g5(x: f64) -> Result<String, GenerateError> {
    g(py::round(x, 5))
}

/// `_valign_shift` (:262): a vertical-align length in its own unit, px at 0.45pt and a bare 0
/// as 0pt (#565).
fn valign_shift(va: &str) -> Result<Option<(String, &'static str)>, GenerateError> {
    let Some(m) = VALIGN_LENGTH.captures(va) else {
        return Ok(None);
    };
    let mut number = number_of(&m[1])?;
    let unit = match m.get(2).map(|u| u.as_str()) {
        None if number != 0.0 => return Ok(None),
        None | Some("pt") => "$318",
        Some("px") => {
            number *= 0.45;
            "$318"
        }
        Some("em") => "$308",
        Some("rem") => "$505",
        _ => "$314",
    };
    Ok(Some((g5(number)?, unit)))
}

/// A size hint's number and unit, px when none (`_SIZE_HINT_RE`).
fn hint(m: &Captures) -> Result<(f64, String), GenerateError> {
    Ok((
        number_of(&m[2])?,
        m.get(3).map_or("px", |u| u.as_str()).to_owned(),
    ))
}

/// `_inline_length_em` (:304): a length in the paragraph's em, em as written, px at 0.45pt and
/// pt with 1em = 12pt.
fn length_em(m: &Captures, font_size: f64) -> Result<f64, GenerateError> {
    let (number, unit) = hint(m)?;
    Ok(match unit.as_str() {
        "em" => number,
        u => number * if u == "px" { 0.45 } else { 1.0 } / 12.0 / or_one(font_size),
    })
}

/// Python's `x or 1`.
fn or_one(x: f64) -> f64 {
    if x == 0.0 { 1.0 } else { x }
}

/// `_inline_float_style` (:279): a float's `$56` as a % of the column (#565), 640px in a
/// vertical book (#754, 0bc705d :285).
fn float_key(
    side: &str,
    m: Option<&Captures>,
    width: f64,
    height: f64,
) -> Result<StyleKey, GenerateError> {
    let side = side.to_owned();
    let Some(m) = m else {
        return Ok(StyleKey::Float(
            side,
            Some(g5(width / column_px() * 100.0)?),
            false,
            false,
            None,
        ));
    };
    let (number, unit) = hint(m)?;
    let mut fill = false;
    let pct = if &m[1] == "h" {
        if unit == "%" {
            return Ok(StyleKey::Float(side, None, false, true, Some(g(number)?)));
        }
        let h = number
            * match unit.as_str() {
                "em" => 16.0,
                "pt" => 4.0 / 3.0,
                _ => 0.6,
            };
        h * width / height / column_px() * 100.0
    } else {
        match unit.as_str() {
            "%" => number,
            "em" => {
                fill = true;
                number * 16.0 / column_px() * 100.0
            }
            u => number * if u == "pt" { 4.0 / 3.0 } else { 1.0 } / column_px() * 100.0,
        }
    };
    Ok(StyleKey::Float(side, Some(g5(pct)?), fill, true, None))
}

/// `_inline_picture_entry`'s size (:2467) of an unfloated picture: its pixels / 16 in the
/// paragraph's em, or the markup's width or height with the other following its shape; a
/// height given with a width is kept too, and a % height stays a %.
fn inline_key(
    m: Option<&Captures>,
    extras: &BTreeMap<&str, &str>,
    (width, height): (f64, f64),
    font_size: f64,
) -> Result<StyleKey, GenerateError> {
    let scale = 16.0 * or_one(font_size);
    let (mut w_em, mut h_em, mut fill) = (width / scale, height / scale, false);
    let (number, unit) = match m {
        Some(m) => hint(m)?,
        None => (0.0, String::new()),
    };
    let axis = m.map(|m| &m[1]);
    let given_text = format!("h={}", extras.get("h").copied().unwrap_or(""));
    let given = SIZE_HINT.captures(&given_text);
    let given_unit = given
        .as_ref()
        .map(|g| g.get(3).map_or("px", |u| u.as_str()));
    let (mut pct_h, mut w_text) = (None, None);
    if axis == Some("h") && unit == "%" {
        pct_h = Some(number);
    } else if axis == Some("w") && given_unit == Some("%") {
        pct_h = Some(number_of(&given.as_ref().unwrap()[2])?);
        w_text = Some(match unit.as_str() {
            "em" => {
                fill = true;
                g5(number)?
            }
            "%" => format!("{}%", g5(number)?),
            u => {
                let pt = number * if u == "px" { 0.45 } else { 1.0 };
                format!("{}%", g5(pt / page_pt() * 100.0)?)
            }
        });
    }
    if pct_h.is_none() && axis == Some("w") {
        match unit.as_str() {
            "em" => (w_em, fill) = (number, true),
            "%" => w_em = number / 100.0 * column_px() / scale,
            u => w_em = number * if u == "px" { 0.45 } else { 1.0 } / 12.0 / or_one(font_size),
        }
        h_em = w_em * height / width;
        if let Some(g) = given.as_ref().filter(|_| given_unit != Some("%")) {
            h_em = length_em(g, font_size)?;
        }
    } else if pct_h.is_none()
        && let Some(m) = m
    {
        h_em = length_em(m, font_size)?;
        w_em = h_em * width / height;
    }
    let va = extras.get("va").copied();
    let valign = va.and_then(inline_valign_symbol);
    let shift = match (valign, va) {
        (None, Some(va)) => valign_shift(va)?,
        _ => None,
    };
    let h_text = match pct_h {
        Some(pct) => format!("{}%", g5(pct)?),
        None => {
            w_text = Some(g5(w_em)?);
            g5(h_em)?
        }
    };
    Ok(StyleKey::Inline(
        w_text,
        h_text,
        fill,
        m.is_some(),
        valign,
        shift,
    ))
}

/// A % or an em of a style: `IonStruct($307, value, $306, $314 or $308)`.
fn pct_or_em(v: &str) -> Value {
    match v.strip_suffix('%') {
        Some(pct) => measure(py::decimal(pct), "$314"),
        None => measure(py::decimal(v), "$308"),
    }
}

fn hundred_pct() -> Value {
    measure(py::decimal("100"), "$314")
}

/// The `$157` an `s_ipic` key writes (:2451, :2510).
fn style_value(key: &StyleKey, name: &str) -> Value {
    let mut v = deckle_kfx::ion::Struct::new();
    match key {
        StyleKey::Float(side, pct, fill, sized, pct_h) => {
            v.insert("$140", sym(if side == "left" { "$59" } else { "$61" }));
            for (k, x) in [("$56", pct), ("$57", pct_h)] {
                if let Some(x) = x {
                    v.insert(k, measure(py::decimal(x), "$314"));
                }
            }
            if *fill {
                v.insert("$65", hundred_pct());
            }
            if *sized {
                v.insert("$546", sym("$377"));
            }
        }
        StyleKey::Inline(w, h, fill, sized, valign, shift) => {
            if let Some(w) = w {
                v.insert("$56", pct_or_em(w));
            }
            v.insert("$57", pct_or_em(h));
            if *fill {
                // Never past the column, as Previewer writes an em width.
                v.insert("$65", hundred_pct());
            }
            if *sized {
                v.insert("$546", sym("$377"));
            }
            if let Some(valign) = valign {
                v.insert("$44", sym(valign));
            }
            if let Some((n, unit)) = shift {
                v.insert("$31", measure(py::decimal(n), unit));
            }
        }
    }
    v.insert("$173", sym(name));
    Value::Struct(v)
}

impl InlineStyles {
    pub(crate) fn new(images: &Images) -> Self {
        InlineStyles {
            dims: images.dims_by_resource(),
            styles: Vec::new(),
        }
    }

    /// `_inline_picture_entry` (:2419): the `$271` of a picture among words, inline, its style
    /// the one its size keys, made the first time (#529, #565).
    fn entry(&mut self, g: &mut Generator, p: &InlinePicture) -> Result<Value, GenerateError> {
        // `dict(e.split("=", 1) ...)`: a later key wins.
        let extras: BTreeMap<&str, &str> = p
            .extra
            .split(';')
            .filter_map(|e| e.split_once('='))
            .collect();
        let (w, h) = self.dims.get(&p.resource).copied().unwrap_or((None, None));
        // Pixels unknown (a picture that won't decode): one em.
        let px = |d: Option<f64>| d.filter(|&d| d != 0.0).unwrap_or(16.0);
        let (width, height) = (px(w), px(h));
        let m = SIZE_HINT.captures(&p.size);
        let key = match extras.get("fl") {
            Some(&side) if side == "left" || side == "right" => {
                float_key(side, m.as_ref(), width, height)?
            }
            _ => inline_key(m.as_ref(), &extras, (width, height), p.font_size)?,
        };
        let name = match self.styles.iter().find(|(k, ..)| *k == key) {
            Some((_, name, _)) => name.clone(),
            None => {
                let name = format!("s_ipic{}", self.styles.len());
                g.local(&name);
                let value = style_value(&key, &name);
                self.styles.push((key, name.clone(), value));
                name
            }
        };
        Ok(st([
            ("$155", Value::Int(p.eid)),
            ("$584", Value::string(&p.alt)),
            ("$601", sym("$283")),
            ("$157", sym(&name)),
            ("$159", sym("$271")),
            ("$175", sym(&p.resource)),
        ]))
    }

    /// A text entry's `$146` (:4400): its words, each picture and 縦中横 among them in its
    /// place.
    pub(crate) fn mixed(
        &mut self,
        g: &mut Generator,
        chunk: &Chunk,
    ) -> Result<Value, GenerateError> {
        let text = &chunk.text;
        let mut out = Vec::new();
        let mut last = 0;
        for item in chunk.among_words() {
            let k = item.at() as usize;
            let words = py::take(py::skip(text, last), k.saturating_sub(last));
            if !words.is_empty() {
                out.push(Value::string(words));
            }
            out.push(match item {
                AmongWords::Picture(p) => self.entry(g, p)?,
                AmongWords::Tcy(t) => writing::tcy_entry(g, t),
            });
            last = k + 1;
        }
        let words = py::skip(text, last);
        if !words.is_empty() {
            out.push(Value::string(words));
        }
        Ok(Value::List(out))
    }

    /// The styles' `$157` fragments, in the order they were made.
    pub(crate) fn into_styles(self) -> Vec<(String, Value)> {
        self.styles.into_iter().map(|(_, n, v)| (n, v)).collect()
    }
}

/// The positions of a text entry holding pictures or 縦中横 (:3383): its words around each,
/// positions at each one's id, as many as it is wide, and the entry's again after it.
/// Pushes `$265`'s (offset, id, offset into the entry, kept beside another at its offset) and
/// the section's (id, length, offset into the entry). Returns the offsets taken beyond its
/// text's, a 縦中横's characters but the first.
pub(crate) fn place_among_words(
    chunk: &Chunk,
    pos: i64,
    char_offset: i64,
    entries: &mut Vec<(i64, i64, i64, bool)>,
    located: &mut Vec<(i64, i64, i64)>,
) -> i64 {
    let len = py::len(&chunk.text) as i64;
    let (mut done, mut extra) = (0, 0);
    for item in chunk.among_words() {
        let k = item.at();
        let base = char_offset + k + extra;
        located.push((pos, k - done, done));
        if k > done {
            // The paragraph reaches the item here; right after another, where that one's
            // resumes it.
            entries.push((base, pos, k, true));
        }
        entries.push((base, item.eid(), 0, true));
        located.push((item.eid(), item.width(), 0));
        let at = base + item.width();
        done = k + 1;
        if done < len {
            entries.push((at, pos, done, true));
        }
        extra += at - base - 1;
    }
    if done < len {
        located.push((pos, len - done, done));
    }
    extra
}
