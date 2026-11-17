// SPDX-License-Identifier: GPL-3.0-only
//! Pictures and the cover (`native_generator.py` at kfxgen `4b81651`): the `$164` resources and
//! `$417` bytes `generate_full_book` makes from its `images` and `cover_image` (:4679–4849),
//! the picture tokens a paragraph holds (`_token_resource` :1923, `_emit_text_chunks` :5454),
//! their `$157` styles (:7008), the cover's page (:4876), the pictures nothing shows
//! (:4910), and what `$419` and `$585` say about them (:149, :2256).
//!
//! A picture among a paragraph's words, linked, or a float among words is `inline_pictures`'.
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use deckle_kfx::fragment::{Fragment, FragmentList};
use deckle_kfx::ion::Value;
use regex::Regex;

use super::block_styles::Length;
use super::chapters::{Block, Chapter, first_writing_mode};
use super::chunks::Chunk;
use super::inline::{Anchors, Flag, Span};
use super::page_scale::{column_px, em_percent, page_pt};
use super::table_styles::align_symbol;
use super::{GenerateError, Generator, measure, model_err, py, st, sym};
use crate::pyvalue::PyValue;

use GenerateError::Unsupported;

/// `_img_tokens.IMG_TOKEN_RE`: a picture in the text, `(href, alt, size, float, extra)`.
const TOKEN: &str = r"\x00IMG\x01([^\x01]*)\x01([^\x00\x01]*)(?:\x01([^\x00\x01]*))?(?:\x01([^\x00\x01]*))?(?:\x01([^\x00]*))?\x00";
pub(crate) static IMG_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(TOKEN).unwrap());
/// `IMG_TOKEN_RE.fullmatch`.
pub(crate) static WHOLE_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\A(?:{TOKEN})\z")).unwrap());
/// `_SIZE_HINT_RE` (:59), its `$` also before a final line break, as Python's is.
pub(crate) static SIZE_HINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A([wh])=(\d+(?:\.\d+)?)(%|px|em|pt)?\n?\z").unwrap());

pub(crate) const COVER_RESOURCE: &str = "cover_img";
const COVER_LOCATION: &str = "resource/cover_img";
/// The name the cover's page asks for its picture by (:4839).
const COVER_BASENAME: &str = "__kfxgen_cover__";

/// A resource's `$164` and `$417` names.
type Pair = (String, String);
/// A picture's width and height as its style reads them: its pixels, or `display_dims`'.
pub(crate) type Dims = (Option<f64>, Option<f64>);

/// A Python dict: its items in insertion order, and `set` replaces a value where its key
/// stands. `at` finds a key's item and is never iterated.
struct Dict<V> {
    items: Vec<(String, V)>,
    at: HashMap<String, usize>,
}

impl<V> Default for Dict<V> {
    fn default() -> Self {
        Dict {
            items: Vec::new(),
            at: HashMap::new(),
        }
    }
}

impl<V> Dict<V> {
    fn get(&self, key: &str) -> Option<&V> {
        self.at.get(key).map(|&i| &self.items[i].1)
    }

    fn set(&mut self, key: &str, value: V) {
        match self.at.get(key) {
            Some(&i) => self.items[i].1 = value,
            None => {
                self.at.insert(key.to_owned(), self.items.len());
                self.items.push((key.to_owned(), value));
            }
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(&V) -> bool) {
        self.items.retain(|(_, v)| keep(v));
        self.at = (self.items.iter().enumerate())
            .map(|(i, (k, _))| (k.clone(), i))
            .collect();
    }
}

/// `generate_full_book`'s `image_resources` and `_image_dims`.
#[derive(Default)]
pub(crate) struct Images {
    resources: Dict<Pair>,
    dims: Dict<Dims>,
    resolved_refs: bool,
    pub cover: bool,
}

impl Images {
    /// `_token_resource` (:1923): the exact href, its decoded form, then its file name for an
    /// href never resolved. A query string names no other file.
    pub(crate) fn resource(&self, href: &str) -> Option<&Pair> {
        let bare = href.split('#').next().unwrap_or("");
        let bare = bare.split('?').next().unwrap_or("");
        self.resources
            .get(bare)
            .or_else(|| self.resources.get(&unquote(bare)))
            .or_else(|| {
                self.may_fall_back_by_basename(bare)
                    .then(|| self.resources.get(bare.rsplit('/').next().unwrap_or("")))
                    .flatten()
            })
    }

    /// Each resource's dims, from its first href that has them (:2434).
    pub(crate) fn dims_by_resource(&self) -> BTreeMap<String, Dims> {
        let mut out = BTreeMap::new();
        for (key, (res, _)) in &self.resources.items {
            if let Some(d) = self.dims.get(key) {
                out.entry(res.clone()).or_insert(*d);
            }
        }
        out
    }

    /// `_may_fall_back_by_basename` (:1937).
    fn may_fall_back_by_basename(&self, href: &str) -> bool {
        !self.resolved_refs
            || href.starts_with('/')
            || href.replace('\\', "/").split('/').any(|p| p == "..")
    }
}

/// `_img_basename` (:4732).
fn basename(href: &str) -> &str {
    let h = href.split('#').next().unwrap_or("");
    h.rsplit('/').next().unwrap_or("")
}

/// `urllib.parse.unquote`: each run of ASCII characters has its `%XX` escapes decoded as
/// UTF-8, a byte that isn't UTF-8 becoming U+FFFD; other characters stay.
fn unquote(s: &str) -> Cow<'_, str> {
    if !s.contains('%') {
        return Cow::Borrowed(s);
    }
    let mut out = String::new();
    let mut run: Vec<u8> = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if !c.is_ascii() {
            out.push_str(&String::from_utf8_lossy(&std::mem::take(&mut run)));
            out.push(c);
            continue;
        }
        if c == '%' {
            let mut ahead = chars.clone();
            if let (Some(a), Some(b)) = (ahead.next(), ahead.next())
                && let (Some(hi), Some(lo)) = (a.to_digit(16), b.to_digit(16))
            {
                run.push((hi * 16 + lo) as u8);
                chars = ahead;
                continue;
            }
        }
        run.push(c as u8);
    }
    out.push_str(&String::from_utf8_lossy(&run));
    Cow::Owned(out)
}

/// Whether `data` is a JPEG or a PNG, by its first bytes (:4688, :4768).
fn format_of(data: &[u8]) -> Option<bool> {
    if data.starts_with(b"\xff\xd8\xff") {
        Some(true)
    } else if data.starts_with(b"\x89PNG") {
        Some(false)
    } else {
        None
    }
}

/// `_detect_image_dimensions` (:5280): a JPEG's size from its first SOF0–SOF3, a PNG's from
/// its IHDR.
fn detect_image_dimensions(data: &[u8]) -> Option<(i64, i64)> {
    let at = |i: usize| i64::from(data[i]);
    if data.starts_with(b"\xff\xd8\xff") {
        let n = data.len();
        let mut i = 2;
        while i < n - 1 {
            while i < n - 1 && data[i] == 0xFF && data[i + 1] == 0xFF {
                i += 1;
            }
            if i >= n - 1 || data[i] != 0xFF {
                break;
            }
            let marker = data[i + 1];
            if marker == 0x00 {
                i += 2;
                continue;
            }
            if (0xC0..=0xC3).contains(&marker) {
                if i + 9 <= n {
                    return Some((at(i + 7) << 8 | at(i + 8), at(i + 5) << 8 | at(i + 6)));
                }
                break;
            }
            if marker == 0xD9 {
                break;
            }
            if (0xD0..=0xD8).contains(&marker) {
                i += 2;
                continue;
            }
            if i + 4 > n {
                break;
            }
            let length = (at(i + 2) << 8 | at(i + 3)) as usize;
            if length < 2 {
                break;
            }
            i += 2 + length;
        }
    } else if data.starts_with(b"\x89PNG") && data.len() > 24 {
        let int =
            |r: std::ops::Range<usize>| i64::from(u32::from_be_bytes(data[r].try_into().unwrap()));
        return Some((int(16..20), int(20..24)));
    }
    None
}

/// `build_fragment_164` (:2741). The cover's states its MIME type.
fn build_fragment_164(
    g: &mut Generator,
    (resource, location): (&str, &str),
    jpeg: bool,
    size: Option<(i64, i64)>,
    mime: bool,
) -> Fragment {
    g.local(resource);
    g.local(location);
    let Value::Struct(mut v) = st([
        ("$175", sym(resource)),
        ("$161", sym(if jpeg { "$285" } else { "$284" })),
        ("$165", Value::string(location)),
    ]) else {
        unreachable!()
    };
    if let Some((w, h)) = size.filter(|&(w, h)| w != 0 && h != 0) {
        v.insert("$422", Value::Int(w));
        v.insert("$423", Value::Int(h));
    }
    if mime {
        v.insert(
            "$162",
            Value::string(if jpeg { "image/jpg" } else { "image/png" }),
        );
    }
    Fragment::new("$164", Some(resource), Value::Struct(v))
}

/// `build_fragment_417` (:2801): the picture's bytes as they came.
fn build_fragment_417(g: &mut Generator, location: &str, data: &[u8]) -> Fragment {
    g.local(location);
    Fragment::new("$417", Some(location), Value::Blob(data.to_vec()))
}

/// A number of `display_dims`: an int or a float, or None.
fn dim(v: &PyValue) -> Option<Option<f64>> {
    match v {
        PyValue::None => Some(None),
        PyValue::Int(n) => Some(Some(*n as f64)),
        PyValue::Float(f) => Some(Some(*f)),
        _ => None,
    }
}

/// `display_dims.get(href)`, when truthy.
fn display_dims(args: &PyValue, href: &str) -> Result<Option<Dims>, GenerateError> {
    let Some(v) = args
        .get("display_dims")
        .filter(|d| d.is_truthy())
        .and_then(|d| match d {
            PyValue::Dict(pairs) => pairs
                .iter()
                .find(|(k, _)| k.as_str() == Some(href))
                .map(|(_, v)| v),
            _ => None,
        })
    else {
        return Ok(None);
    };
    if !v.is_truthy() {
        return Ok(None);
    }
    match v.as_list() {
        Some([w, h]) => match (dim(w), dim(h)) {
            (Some(w), Some(h)) => Ok(Some((w, h))),
            _ => Err(model_err(format!(
                "args.display_dims[{href:?}]: two numbers"
            ))),
        },
        _ => Err(model_err(format!("args.display_dims[{href:?}]: a pair"))),
    }
}

fn as_dims(size: Option<(i64, i64)>) -> Dims {
    size.map_or((None, None), |(w, h)| (Some(w as f64), Some(h as f64)))
}

/// The cover's and the body's resources, in kfxgen's order (:4679–4849).
pub(crate) fn build_resources(g: &mut Generator, args: &PyValue) -> Result<Images, GenerateError> {
    let mut images = Images {
        resolved_refs: args
            .get("resolved_image_refs")
            .is_some_and(PyValue::is_truthy),
        ..Images::default()
    };
    let cover_image = match args.get("cover_image") {
        None | Some(PyValue::None) => None,
        Some(PyValue::Bytes(b)) => Some(&b[..]).filter(|b| !b.is_empty()),
        Some(_) => return Err(model_err("args.cover_image: bytes or None")),
    };
    let mut cover_dims = (None, None);
    if let Some(data) = cover_image {
        let jpeg = format_of(data)
            .ok_or_else(|| model_err("args.cover_image: the bytes of a JPEG or a PNG"))?;
        let size = detect_image_dimensions(data);
        cover_dims = as_dims(size);
        let f = build_fragment_164(g, (COVER_RESOURCE, COVER_LOCATION), jpeg, size, true);
        g.push(f);
        let f = build_fragment_417(g, COVER_LOCATION, data);
        g.push(f);
        images.cover = true;
    }

    let mut cover_hrefs = Vec::new();
    let mut entries: Vec<(&str, &[u8])> = match args.get("images") {
        None | Some(PyValue::None) => Vec::new(),
        Some(PyValue::Dict(pairs)) => pairs
            .iter()
            .map(|(k, v)| match (k, v) {
                (PyValue::Str(href), PyValue::Bytes(b)) => Ok((href.as_str(), &b[..])),
                (PyValue::Str(href), PyValue::None) => Ok((href.as_str(), &[][..])),
                _ => Err(model_err("args.images: str to bytes")),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(model_err("args.images: a dict")),
    };
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut valid = Vec::new();
    for (href, data) in entries {
        if data.is_empty() {
            continue;
        }
        if cover_image == Some(data) {
            // The cover's picture in the text: shown from the cover's resource (#475).
            cover_hrefs.push(href);
            continue;
        }
        if let Some(jpeg) = format_of(data) {
            valid.push((href, data, jpeg));
        }
    }
    for (idx, (href, data, jpeg)) in valid.into_iter().enumerate() {
        let pair = (format!("img_{idx}"), format!("resource/img_{idx}"));
        let size = detect_image_dimensions(data);
        let f = build_fragment_164(g, (&pair.0, &pair.1), jpeg, size, false);
        g.push(f);
        // The page size is the size to show it at, when the caller gives one (#440).
        let dims = display_dims(args, href)?.unwrap_or(as_dims(size));
        let f = build_fragment_417(g, &pair.1, data);
        g.push(f);
        let base = basename(href);
        // Two pictures of one name: the first keeps it.
        if images.resources.get(base).is_none() {
            images.resources.set(base, pair.clone());
            images.dims.set(base, dims);
        }
        images.resources.set(href, pair.clone());
        images.dims.set(href, dims);
        let decoded = unquote(href);
        if images.resources.get(&decoded).is_none() {
            images.resources.set(&decoded, pair);
        }
    }

    for href in cover_hrefs {
        let dims = display_dims(args, href)?.unwrap_or(cover_dims);
        for key in [basename(href), href, &unquote(href)] {
            if images.resources.get(key).is_none() {
                images.resources.set(key, cover_pair());
                images.dims.set(key, dims);
            }
        }
    }
    // The cover's page asks for its picture by this name (#32).
    if images.cover {
        images.resources.set(COVER_BASENAME, cover_pair());
        images.dims.set(COVER_BASENAME, cover_dims);
    }
    Ok(images)
}

fn cover_pair() -> Pair {
    (COVER_RESOURCE.to_owned(), COVER_LOCATION.to_owned())
}

/// The cover page's text (:4896).
pub(crate) fn cover_token() -> String {
    format!("\x00IMG\x01{COVER_BASENAME}\x01\x00")
}

/// A picture entry: `_emit_text_chunks`' image chunk (:5487).
#[derive(Clone, Debug)]
pub(crate) struct Picture {
    pub resource: String,
    alt: String,
    /// The markup's own size (`w=98%`, `h=50%`, `w=200px`).
    pub size: Option<String>,
    /// `left` or `right` (#243).
    pub float: Option<String>,
    /// In a link's container: inline, the one position its link covers (#355).
    pub inline: bool,
    /// In vertical text or a horizontal block of it: its size only (#247), and a block
    /// picture's alignment there (0bc705d :5898).
    pub plain: bool,
    pub box_align: Option<String>,
    /// The markup's height, the token's `h` (#247, a1ba662).
    pub height: Option<String>,
    /// Directly in vertical-rl text: sized by its height (#247, a1ba662).
    pub upright: bool,
    /// Its own side margins, the token's `ml` and `mr`, and `c` when auto margins centre
    /// it (`r`: the left one alone, setting it right) (#769, f605dd6).
    pub own_margins: [Option<String>; 2],
    pub own_centre: Option<String>,
    /// A block picture's place as its block gives it (#769, f605dd6); None for one placed
    /// as before, centred.
    pub place: Option<Box<Place>>,
}

/// `_emit_paragraph`'s `place` of a block picture (f605dd6): its block's text-align, its
/// first-line indent when the picture opens the block, its side margins, the side margins
/// of the boxes it is in (outermost first), and its block's font size in CSS px. A linked
/// picture's is all None: its container is placed instead.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Place {
    pub align: Option<String>,
    pub indent: Option<Length>,
    pub left: Option<Length>,
    pub right: Option<Length>,
    pub boxes: Vec<[Option<Length>; 2]>,
    pub font_px: Option<f64>,
}

/// Whether `text` holds pictures and nothing else but spaces: `IMG_TOKEN_RE.sub("", text)`
/// strips to nothing while `text` doesn't.
pub(crate) fn only_pictures(text: &str) -> bool {
    !py::strip(text).is_empty() && py::strip(&IMG_TOKEN.replace_all(text, "")).is_empty()
}

pub(crate) fn has_picture(text: &str) -> bool {
    IMG_TOKEN.is_match(text)
}

/// `_emit_text_chunks`' image chunk (:5487) for the token `m`, when the book holds its picture.
pub(crate) fn picture_of(m: &regex::Captures, images: &Images) -> Option<Picture> {
    let (resource, _) = images.resource(&m[1])?;
    let field = |i: usize| m.get(i).map(|g| g.as_str()).filter(|s| !s.is_empty());
    // `dict(e.split("=", 1) for e in extras.split(";") if "=" in e)`: a later key wins.
    let extras: Vec<(&str, &str)> = field(5)
        .unwrap_or("")
        .split(';')
        .filter_map(|e| e.split_once('='))
        .collect();
    let extra = |key: &str| {
        extras
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .filter(|v| !v.is_empty())
    };
    Some(Picture {
        resource: resource.clone(),
        alt: m[2].replace('\x02', " "),
        size: field(3).map(str::to_owned),
        float: field(4).map(str::to_owned),
        inline: false,
        plain: false,
        box_align: None,
        // The markup's height: in vertical text it sizes the picture whenever given (#247,
        // a1ba662).
        height: extra("h").map(str::to_owned),
        upright: false,
        own_margins: [
            extra("ml").map(str::to_owned),
            extra("mr").map(str::to_owned),
        ],
        own_centre: extra("c").map(str::to_owned),
        place: None,
    })
}

/// `inline_style.parse_css_length(value, negative=True)` (ad38ec8): a CSS length as
/// (magnitude, KFX unit), a negative one too; None for none, a zero, or another unit. `in`,
/// `cm` and `ex` are read as pt, mm and em.
fn parse_css_length(value: &str) -> Option<Length> {
    static CSS_LENGTH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A([+-]?[0-9]*\.?[0-9]+)((?i:em|rem|%|pt|px|mm|in|cm|ex))\z").unwrap()
    });
    let m = CSS_LENGTH.captures(value.trim_matches(py::is_space))?;
    let number: f64 = m[1].parse().ok()?;
    if number == 0.0 {
        return None;
    }
    let (mut mag, mut unit) = (m[1].to_owned(), m[2].to_lowercase());
    let factor = match unit.as_str() {
        "in" => Some((72.0, "pt")),
        "cm" => Some((10.0, "mm")),
        "ex" => Some((0.44, "em")),
        _ => None,
    };
    if let Some((factor, to)) = factor {
        mag = format!("{:.4}", number * factor);
        unit = to.to_owned();
    }
    if mag.contains('.') {
        mag = mag.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    let symbol = match unit.as_str() {
        "em" => "$308",
        "rem" => "$505",
        "%" => "$314",
        "pt" => "$318",
        "px" => "$319",
        _ => "$316",
    };
    Some((mag, symbol.to_owned()))
}

/// A length's magnitude, as `float()` reads it.
fn magnitude((mag, _): &Length) -> f64 {
    mag.parse().unwrap_or(0.0)
}

/// `_share` (f605dd6): a length as a % of the page, at Previewer's rates (1em 12pt, 1px
/// 0.45pt); an em at `scale`, its block's font size against the main text's.
fn share(length: Option<&Length>, scale: f64) -> f64 {
    let Some(length) = length else {
        return 0.0;
    };
    let em = em_percent();
    let per = match length.1.as_str() {
        "$308" => em * scale,
        "$505" => em,
        "$319" => em * 0.45 / 12.0,
        "$318" => em / 12.0,
        "$316" => em * 72.0 / 25.4 / 12.0,
        "$314" => 1.0,
        _ => 0.0,
    };
    magnitude(length) * per
}

/// `_sides` (f605dd6): a picture's or its link container's (`$48`, `$50`) in % of the page,
/// as Kindle Previewer 4.0.3 adds them (#769; QA's Q770, Q770E, Q770G): its containers' side
/// margins, outermost first, then its block's, its block's first-line indent and its own.
///
/// A % margin is of the width left when a box outside it has a margin on that side, else of
/// the page; every margin, em ones too, takes its share off the width left. An indent in %
/// is of the width left; one in em or pt is divided by the width the containers leave, not
/// the block's own margins. An em of its block at `scale`.
pub(crate) fn sides(
    boxes: &[[Option<Length>; 2]],
    scale: f64,
    (left, right): (Option<&Length>, Option<&Length>),
    indent: Option<&Length>,
    own: [Option<&Length>; 2],
) -> [f64; 2] {
    let mut width = 100.0;
    let mut out = [0.0, 0.0];
    let mut held = [false, false];
    // (its share of the page, its share off the width left).
    let part = |i: usize, length: &Length, s: f64, width: f64, held: &[bool; 2]| {
        if length.1 == "$314" {
            let frac = magnitude(length) / 100.0;
            (frac * if held[i] { width } else { 100.0 }, frac * width)
        } else {
            let pct = share(Some(length), s);
            (pct, pct)
        }
    };
    let add = |sides: [Option<&Length>; 2],
               at: f64,
               width: &mut f64,
               out: &mut [f64; 2],
               held: &mut [bool; 2]| {
        let mut taken = 0.0;
        for (i, length) in sides.into_iter().enumerate() {
            let Some(length) = length.filter(|l| magnitude(l) != 0.0) else {
                continue;
            };
            let (pct, off) = part(i, length, at, *width, held);
            out[i] += pct;
            taken += off;
            held[i] = true;
        }
        *width = f64::max(*width - taken, 1.0);
    };
    for b in boxes.iter().rev() {
        add(
            [b[0].as_ref(), b[1].as_ref()],
            1.0,
            &mut width,
            &mut out,
            &mut held,
        );
    }
    let left_by_containers = width;
    add([left, right], scale, &mut width, &mut out, &mut held);
    if let Some(indent) = indent {
        if indent.1 == "$314" {
            out[0] += part(0, indent, 1.0, width, &held).0;
        } else {
            out[0] += share(Some(indent), scale) * 100.0 / left_by_containers;
        }
    }
    for (i, length) in own.into_iter().enumerate() {
        if let Some(length) = length {
            out[i] += part(i, length, scale, width, &held).0;
        }
    }
    out
}

/// A block's font size against the main text's: `font_px / main_px` when both are set.
pub(crate) fn place_scale(font_px: Option<f64>, main_px: Option<f64>) -> f64 {
    match (font_px, main_px) {
        (Some(f), Some(m)) if f != 0.0 && m != 0.0 => f / m,
        _ => 1.0,
    }
}

/// `_picture_link` (0bc705d :1835): the link of the first run covering the token at `at`, in
/// characters, that has one; empty targets are none. Of a run's two, the smallest, as the
/// container's `link_target` takes it (#759).
pub(crate) fn picture_link(spans: &[Span], at: i64) -> Option<&String> {
    spans
        .iter()
        .filter(|s| s.start <= at && at < s.start + s.len)
        .find_map(|s| {
            s.flags.iter().find_map(|f| match f {
                Flag::Link(t) if !t.is_empty() => Some(t),
                _ => None,
            })
        })
}

/// `_same_words` (:1143).
fn same_words(text: &str, title: &str) -> bool {
    let words = |s: &str| {
        s.split(py::is_space)
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    words(text) == words(title)
}

/// `_title_after_pictures` (:1148): the chapter opens with pictures and then a paragraph that
/// is its title, which the book prints; kfxgen prints none (#346).
pub(crate) fn title_after_pictures<'a>(
    texts: impl IntoIterator<Item = &'a str>,
    title: &str,
) -> bool {
    let mut seen_picture = false;
    for text in texts {
        // `_is_text_block` (:1127).
        if !py::strip(&IMG_TOKEN.replace_all(text, "")).is_empty() {
            return seen_picture && same_words(text, title);
        }
        if !has_picture(text) {
            return false;
        }
        seen_picture = true;
    }
    false
}

/// The cover's page before the book's (:4876). The first page opening with the cover's
/// picture leaves it to the cover's page (#475), and a contents page's links move one on.
pub(crate) fn add_cover_page(chapters: &mut Vec<Chapter>, images: &Images) {
    if let Some(first) = chapters.first_mut()
        && let Some(blocks) = first.blocks.as_mut()
    {
        without_leading_cover(blocks, images);
        // kfxgen reads the page's writing mode from the blocks left (:4905, then :942).
        first.writing_mode = first_writing_mode(blocks);
    }
    for link in chapters.iter_mut().flat_map(|c| &mut c.toc_links) {
        // Saturating: a target past every chapter stays past them.
        link.target = link.target.saturating_add(1);
    }
    chapters.insert(0, Chapter::cover());
}

/// `_without_leading_cover` (:5372): the cover's picture dropped when it is the first block
/// with text, its ids moving to the next block's start; a box (#238) is looked into. Nothing
/// else printing on the page, it stays, as Kindle Previewer prints it (#524).
fn without_leading_cover(blocks: &mut Vec<Block>, images: &Images) {
    let mut left = blocks.clone();
    // Ids left to place mean no block followed the cover, so nothing below prints.
    if drop_leading_cover(&mut left, images).is_some()
        && left.iter().any(|b| !py::strip(&b.text).is_empty())
    {
        *blocks = left;
    }
}

/// `_with_anchors` (:1911): `source`'s ids at the start of `anchors`.
fn with_anchors(anchors: &mut Anchors, source: Anchors) {
    let mut offsets: BTreeMap<String, i64> = source.keys.iter().map(|k| (k.clone(), 0)).collect();
    offsets.append(&mut anchors.offsets);
    anchors.offsets = offsets;
    anchors.keys.splice(0..0, source.keys);
}

/// `_drop_leading_cover` (:5392): `blocks` less the cover's picture when it is the first
/// block with text, and the ids left to place (those of a picture nothing follows); None,
/// `blocks` as they were, when the first block with text isn't that picture.
fn drop_leading_cover(blocks: &mut Vec<Block>, images: &Images) -> Option<Option<Anchors>> {
    let n = blocks.iter().position(|b| !py::strip(&b.text).is_empty())?;
    let mut pending = blocks[n].anchors.clone();
    if let Some(boxed) = &blocks[n].boxed {
        let mut kids = boxed.blocks.clone();
        let inner = drop_leading_cover(&mut kids, images)?;
        if !kids.is_empty() {
            if let Some(inner) = inner {
                with_anchors(&mut kids[0].anchors, inner);
            }
            let block = &mut blocks[n];
            block.text = kids
                .iter()
                .map(|k| k.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            block.boxed.as_mut().unwrap().blocks = kids;
            return Some(None);
        }
        if let Some(inner) = inner {
            with_anchors(&mut pending, inner);
        }
    } else {
        let m = WHOLE_TOKEN.captures(py::strip(&blocks[n].text))?;
        let (resource, _) = images.resource(&m[1])?;
        if resource != COVER_RESOURCE {
            return None;
        }
    }
    blocks.remove(n);
    match blocks.get_mut(n) {
        Some(next) => {
            with_anchors(&mut next.anchors, pending);
            Some(None)
        }
        None => Some(Some(pending)),
    }
}

/// `_parse_size_hint` (:645): ('w' or 'h', percent), or None to size by pixels. Its column is
/// `column_px()`, as kfxgen 0bc705d passes it (:7132).
fn parse_size_hint(hint: &str) -> Result<Option<(char, f64)>, GenerateError> {
    let Some(m) = SIZE_HINT.captures(hint) else {
        return Ok(None);
    };
    if !m[2].is_ascii() {
        return Err(Unsupported("a picture size in other than ASCII digits"));
    }
    let axis = if &m[1] == "w" { 'w' } else { 'h' };
    let number: f64 = m[2].parse().unwrap();
    let pct = match m.get(3).map_or("px", |u| u.as_str()) {
        "%" => number,
        _ if axis == 'h' => return Ok(None),
        unit => {
            let per_unit = match unit {
                "em" => 16.0,
                "pt" => 4.0 / 3.0,
                _ => 1.0,
            };
            number * per_unit / column_px() * 100.0
        }
    };
    if pct <= 0.0 {
        return Ok(None);
    }
    Ok(Some((axis, py::round(pct.min(100.0), 3))))
}

/// A picture style's size: `_image_size_key` (:7029).
type SizeKey = (char, f64);

/// `_place_key` (f605dd6): a block picture's `$580` or None, and its `$48` and `$50` in %.
type PlaceKey = (Option<&'static str>, f64, f64);

/// `_place_order` (f605dd6): a picture placed as before sorts first.
fn place_order(place: &Option<PlaceKey>) -> (bool, &'static str, f64, f64) {
    match place {
        None => (false, "", 0.0, 0.0),
        Some((align, left, right)) => (true, align.unwrap_or(""), *left, *right),
    }
}

/// `_plain_key` (a1ba662): a plain picture's size, whether the markup set it, a block
/// picture's alignment (#247), and whether a height in em gives it `$64` (QA's Q761 E04).
/// Its clear is never one: the port refuses a clear.
type PlainKey = (SizeKey, bool, Option<String>, bool);

/// `_LENGTH_RE` (a1ba662): a length, as `_img_size_hint` and `_img_inline_extras` pass it.
static LENGTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(\d+(?:\.\d+)?)(%|px|em|pt)?\n?\z").unwrap());

/// `_up` (a1ba662): a picture's percentage, at most 100, to three places rounded half up, as
/// Previewer rounds it (7.813, where `round` gives 7.812; QA's Q761 E32):
/// `Decimal(f"{min(100.0, pct):.9f}").quantize(Decimal("0.001"), ROUND_HALF_UP)`. An infinity,
/// or a result past the context's 28 digits, raises `InvalidOperation` there.
pub(crate) fn up(pct: f64) -> Result<f64, GenerateError> {
    let x = pct.min(100.0);
    let cannot = || {
        model_err(format!(
            "a picture's percentage of {pct}: kfxgen cannot round it"
        ))
    };
    // Before the text: "-inf" has no fraction to slice.
    if !x.is_finite() {
        return Err(cannot());
    }
    let text = format!("{:.9}", x.abs());
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    let digits = format!("{int}{}", &frac[..3]);
    if digits.trim_start_matches('0').len() > 28 {
        return Err(cannot());
    }
    let mut milli: i128 = digits.parse().unwrap();
    if frac.as_bytes()[3] >= b'5' {
        milli += 1;
    }
    // `float()` of the decimal: its text, parsed, is the nearest float.
    let sign = if x.is_sign_negative() { "-" } else { "" };
    Ok(format!("{sign}{}.{:03}", milli / 1000, milli % 1000)
        .parse()
        .unwrap())
}

/// Points per unit for a picture's height in vertical text, as Kindle Previewer 4.0.3
/// counts it (`_HEIGHT_PT`, a1ba662; QA's Q761 E01-E06): px at the picture's own 0.75pt,
/// pt through px at 0.45pt, em at 12pt.
fn height_pt(unit: &str) -> f64 {
    match unit {
        "em" => 12.0,
        "pt" => 0.75 / 0.45,
        _ => 0.75,
    }
}

/// `_WIDTH_PT` (a1ba662): a width in px at 0.45pt, em at 12pt, pt as it is.
fn width_pt(unit: &str) -> f64 {
    match unit {
        "px" => 0.45,
        "em" => 12.0,
        _ => 1.0,
    }
}

/// `_upright_lengths` (a1ba662): a picture's height and width as the markup gives them, each
/// a (number, unit) or None.
fn upright_lengths(p: &Picture) -> Result<[Option<(f64, String)>; 2], GenerateError> {
    let mut raw = [p.height.clone().unwrap_or_default(), String::new()];
    if let Some(size) = SIZE_HINT.captures(p.size.as_deref().unwrap_or(""))
        && !(&size[1] == "h" && !raw[0].is_empty())
    {
        raw[usize::from(&size[1] == "w")] =
            format!("{}{}", &size[2], size.get(3).map_or("", |u| u.as_str()));
    }
    let mut out = [None, None];
    for (slot, text) in out.iter_mut().zip(&raw) {
        let Some(m) = LENGTH.captures(text) else {
            continue;
        };
        if !m[1].is_ascii() {
            return Err(Unsupported("a picture size in other than ASCII digits"));
        }
        let number: f64 = m[1].parse().unwrap();
        if number > 0.0 {
            *slot = Some((number, m.get(2).map_or("px", |u| u.as_str()).to_owned()));
        }
    }
    Ok(out)
}

/// The `$157` styles of the book's pictures (:7008): one per width, then per height, then
/// per floated size and side, then per plain picture's key (#247), each in sorted order (#96,
/// #243).
#[derive(Default)]
pub(crate) struct ImageStyles {
    resource_dims: BTreeMap<String, Dims>,
    main_px: Option<f64>,
    widths: Vec<((f64, Option<PlaceKey>), String)>,
    heights: Vec<((f64, Option<PlaceKey>), String)>,
    floats: Vec<(SizeKey, String, String)>,
    plains: Vec<(PlainKey, String)>,
}

impl ImageStyles {
    pub(crate) fn allocate(
        g: &mut Generator,
        pictures: &[&Picture],
        images: &Images,
        main_px: Option<f64>,
        extra_style_names: &mut Vec<String>,
    ) -> Result<Self, GenerateError> {
        let mut styles = ImageStyles {
            main_px,
            ..ImageStyles::default()
        };
        if pictures.is_empty() {
            return Ok(styles);
        }
        for (base, dims) in &images.dims.items {
            if let Some((resource, _)) = images.resources.get(base) {
                styles.resource_dims.insert(resource.clone(), *dims);
            }
        }
        let (mut widths, mut heights, mut floats) = (Vec::new(), Vec::new(), Vec::new());
        let mut plains = Vec::new();
        for p in pictures {
            if p.plain {
                plains.push(styles.plain_key(p)?);
                continue;
            }
            let key = styles.size_key(p)?;
            match (&p.float, key.0) {
                (Some(side), _) => floats.push((key, side.clone())),
                (None, 'w') => widths.push((key.1, styles.place_key(p)?)),
                (None, _) => heights.push((key.1, styles.place_key(p)?)),
            }
        }
        // By (axis, size, sized, alignment or "", clear or "", `$64`), as kfxgen sorts them
        // (a1ba662).
        plains.sort_by(|a, b| {
            (a.0.0, a.0.1, a.1, a.2.as_deref().unwrap_or(""), a.3)
                .partial_cmp(&(b.0.0, b.0.1, b.1, b.2.as_deref().unwrap_or(""), b.3))
                .unwrap()
        });
        plains.dedup();
        // By (size, a place's order), as kfxgen sorts them (f605dd6).
        for v in [&mut widths, &mut heights] {
            v.sort_by(|a, b| {
                (a.0, place_order(&a.1))
                    .partial_cmp(&(b.0, place_order(&b.1)))
                    .unwrap()
            });
            v.dedup();
        }
        floats.sort_by(|a, b| {
            (a.0.0, a.0.1, &a.1)
                .partial_cmp(&(b.0.0, b.0.1, &b.1))
                .unwrap()
        });
        floats.dedup();
        for (idx, key) in widths.into_iter().enumerate() {
            let name = format!("s_img_w{idx}");
            let f = build_fragment_157_image(g, &name, ('w', key.0), None, key.1.as_ref());
            g.push(f);
            extra_style_names.push(name.clone());
            styles.widths.push((key, name));
        }
        for (idx, key) in heights.into_iter().enumerate() {
            let name = format!("s_img_h{idx}");
            let f = build_fragment_157_image(g, &name, ('h', key.0), None, key.1.as_ref());
            g.push(f);
            extra_style_names.push(name.clone());
            styles.heights.push((key, name));
        }
        for (idx, (key, side)) in floats.into_iter().enumerate() {
            let name = format!("s_img_f{idx}");
            let f = build_fragment_157_image(g, &name, key, Some(&side), None);
            g.push(f);
            extra_style_names.push(name.clone());
            styles.floats.push((key, side, name));
        }
        // After the others, so a book's other pictures keep their names (0bc705d :7222).
        for (idx, key) in plains.into_iter().enumerate() {
            let name = format!("s_img_p{idx}");
            let f = build_plain_image_style_157(g, &name, &key);
            g.push(f);
            extra_style_names.push(name.clone());
            styles.plains.push((key, name));
        }
        Ok(styles)
    }

    /// `_place_key` (f605dd6): a block picture's place, (`$580` or None, `$48` %, `$50` %), or
    /// None for a picture placed as before, centred.
    fn place_key(&self, p: &Picture) -> Result<Option<PlaceKey>, GenerateError> {
        let Some(place) = &p.place else {
            return Ok(None);
        };
        let align = match p.own_centre.as_deref() {
            // display: block with auto margins: both centre it (PBJ J04), the left one alone
            // sets it right (QA's Q770 Y17).
            Some("r") => Some("right"),
            Some(_) => Some("center"),
            None => place.align.as_deref(),
        };
        let symbol = match align {
            Some("left") => Some("$59"),
            Some("center") => Some("$320"),
            Some("right") => Some("$61"),
            _ => None,
        };
        let own = p
            .own_margins
            .each_ref()
            .map(|m| m.as_deref().and_then(parse_css_length));
        let [left, right] = sides(
            &place.boxes,
            place_scale(place.font_px, self.main_px),
            (place.left.as_ref(), place.right.as_ref()),
            place.indent.as_ref(),
            [own[0].as_ref(), own[1].as_ref()],
        );
        Ok(Some((symbol, up(left)?, up(right)?)))
    }

    /// `_plain_key` (a1ba662).
    fn plain_key(&self, p: &Picture) -> Result<PlainKey, GenerateError> {
        if p.upright {
            let [height, width] = upright_lengths(p)?;
            let em_high = height.as_ref().is_some_and(|(_, unit)| unit == "em");
            return Ok((
                self.upright_size_key(p)?,
                height.is_some() || width.is_some(),
                p.box_align.clone(),
                em_high,
            ));
        }
        let sized = parse_size_hint(p.size.as_deref().unwrap_or(""))?.is_some();
        Ok((self.size_key(p)?, sized, p.box_align.clone(), false))
    }

    /// `_upright_size_key` (a1ba662): a picture directly in vertical-rl text is sized by its
    /// height (`$57`), capped at 100%, as Kindle Previewer 4.0.3 writes it (PVA, PVAL; QA's
    /// Q761 E01-E06, E51-E55). A height the markup gives wins, in % or a length; else a width
    /// in % keeps its axis; else a width in a length gives the height at the picture's
    /// proportions; else the picture's own height. Lengths count in points over the page.
    fn upright_size_key(&self, p: &Picture) -> Result<SizeKey, GenerateError> {
        let [height, width] = upright_lengths(p)?;
        match (&height, &width) {
            (Some((n, unit)), _) if unit == "%" => return Ok(('h', up(*n)?)),
            (Some((n, unit)), _) => {
                let pt = n * height_pt(unit);
                return Ok(('h', up(pt / page_pt() * 100.0)?));
            }
            (None, Some((n, unit))) if unit == "%" => return Ok(('w', up(*n)?)),
            _ => {}
        }
        let (Some(w), Some(h)) = self
            .resource_dims
            .get(&p.resource)
            .map_or((None, None), |d| (d.0, d.1))
        else {
            return self.size_key(p);
        };
        if w == 0.0 || h == 0.0 {
            return self.size_key(p);
        }
        let pt = match &width {
            Some((n, unit)) => n * width_pt(unit) * h / w,
            None => h * 0.75,
        };
        Ok(('h', up(pt / page_pt() * 100.0)?))
    }

    /// `_image_size_key` (:7029): the markup's size, else the picture's own width against
    /// the column, `min(100, px / 512 * 100)` (#467), px / 640 in a vertical book (#754).
    fn size_key(&self, p: &Picture) -> Result<SizeKey, GenerateError> {
        if let Some(hint) = parse_size_hint(p.size.as_deref().unwrap_or(""))? {
            return Ok(hint);
        }
        let w = self.resource_dims.get(&p.resource).and_then(|d| d.0);
        Ok(match w {
            Some(w) if w != 0.0 => ('w', py::round((w / column_px() * 100.0).min(100.0), 3)),
            _ => ('w', 100.0),
        })
    }

    /// `_image_style_for` (:7108).
    pub(crate) fn style_for(&self, p: &Picture) -> Result<Option<String>, GenerateError> {
        if p.plain {
            let key = self.plain_key(p)?;
            return Ok(self
                .plains
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, n)| n.clone()));
        }
        let key = self.size_key(p)?;
        let name = match &p.float {
            Some(side) => self
                .floats
                .iter()
                .find(|(k, s, _)| *k == key && s == side)
                .map(|(_, _, n)| n),
            None => {
                let place = self.place_key(p)?;
                if key.0 == 'h' {
                    &self.heights
                } else {
                    &self.widths
                }
                .iter()
                .find(|((pct, at), _)| *pct == key.1 && *at == place)
                .map(|(_, n)| n)
            }
        };
        Ok(name.cloned())
    }
}

/// `build_fragment_157_image` (f605dd6).
fn build_fragment_157_image(
    g: &mut Generator,
    name: &str,
    (axis, pct): SizeKey,
    float_side: Option<&str>,
    place: Option<&PlaceKey>,
) -> Fragment {
    g.local(name);
    let Value::Struct(mut v) = st([
        (
            if axis == 'h' { "$57" } else { "$56" },
            measure(py::decimal_of_float(pct), "$314"),
        ),
        ("$65", measure(py::decimal("100"), "$314")),
        (
            "$785",
            st([("$131", Value::Int(2)), ("$132", Value::Int(2))]),
        ),
        ("$546", sym("$377")),
        ("$42", measure(py::decimal("1"), "$310")),
        ("$580", sym("$320")),
        ("$173", sym(name)),
    ]) else {
        unreachable!()
    };
    if let Some(side) = float_side {
        // Floated: not centred, the text after it wraps beside it (#243).
        v.remove("$580");
        v.insert("$140", sym(if side == "left" { "$59" } else { "$61" }));
    } else if let Some((align, left, right)) = place {
        // Placed as its block places it (#769): aligned only when the block is, and its
        // margins as `$48`/`$50`, in % of the page, a negative one too (QA's Q770 Y12).
        match align {
            Some(align) => v.insert("$580", sym(align)),
            None => {
                v.remove("$580");
            }
        }
        for (key, pct) in [("$48", left), ("$50", right)] {
            if *pct != 0.0 {
                v.insert(key, measure(py::decimal(&py::format_g(*pct, 6)), "$314"));
            }
        }
    }
    Fragment::new("$157", Some(name), Value::Struct(v))
}

/// `build_fragment_157_image` with `plain` (0bc705d :4158): in a horizontal block of vertical
/// text, a picture's size only, `$546` when the markup sizes it, and a block picture's
/// alignment (P247k K01, K08; QA's Q755L).
fn build_plain_image_style_157(
    g: &mut Generator,
    name: &str,
    ((axis, pct), sized, box_align, em_high): &PlainKey,
) -> Fragment {
    g.local(name);
    let Value::Struct(mut v) = st([
        (
            if *axis == 'h' { "$57" } else { "$56" },
            measure(py::decimal_of_float(*pct), "$314"),
        ),
        ("$173", sym(name)),
    ]) else {
        unreachable!()
    };
    if *sized {
        v.insert("$546", sym("$377"));
    }
    if let Some(align) = box_align.as_deref().and_then(align_symbol) {
        v.insert("$580", sym(align));
    }
    if *em_high {
        // A height in em, in vertical text (QA's Q761 E04).
        v.insert("$64", measure(py::decimal("100"), "$314"));
    }
    Fragment::new("$157", Some(name), Value::Struct(v))
}

/// Drops each resource no entry shows, its `$164` and `$417` (:4910, #102). The cover stays:
/// `$490` shows it.
pub(crate) fn drop_unshown(g: &mut Generator, images: &mut Images, shown: &[&str]) {
    let shown: HashSet<&str> = shown.iter().copied().collect();
    let unshown: HashSet<Pair> = (images.resources.items.iter())
        .map(|(_, pair)| pair)
        .filter(|(res, _)| !shown.contains(res.as_str()) && res != COVER_RESOURCE)
        .cloned()
        .collect();
    if unshown.is_empty() {
        return;
    }
    let dead: HashSet<&str> = (unshown.iter())
        .flat_map(|(r, l)| [r.as_str(), l.as_str()])
        .collect();
    g.fragments.retain(|f| {
        !f.fid
            .as_ref()
            .is_some_and(|fid| dead.contains(fid.as_str()))
    });
    images.resources.retain(|pair| !unshown.contains(pair));
}

/// The names `$419` lists for the pictures (:5148): the cover's, then each resource's.
pub(crate) fn entity_names(images: &Images) -> Vec<String> {
    let mut names = Vec::new();
    if images.cover {
        names.extend([COVER_RESOURCE.to_owned(), COVER_LOCATION.to_owned()]);
    }
    for (_, (res, loc)) in &images.resources.items {
        names.extend([res.clone(), loc.clone()]);
    }
    names
}

/// `_DEPENDENCIES` (:146): what a fragment of the first type lists of the second.
const DEPENDENCIES: [(&str, &str); 2] = [("$260", "$164"), ("$164", "$417")];

/// `_entity_dependencies` (:149): `$419`'s `$253`, each section's picture resources and each
/// resource's bytes, by kfxlib's rule: a fragment reaches one it names, by a symbol or by a
/// string under `$165`/`$636`, and on through it; a resource doesn't count resources.
pub(crate) fn entity_dependencies(fragments: &FragmentList) -> Vec<Value> {
    fn names(value: &Value, key: Option<&str>, found: &mut Vec<String>) {
        match value {
            Value::Annotated { value, .. } => names(value, key, found),
            Value::Struct(s) => s
                .iter()
                .for_each(|(k, v)| names(v, Some(k.as_str()), found)),
            Value::List(items) => items.iter().for_each(|v| names(v, key, found)),
            Value::Symbol(s) => found.push(s.0.clone()),
            Value::String(s) if matches!(key, Some("$165" | "$636")) => found.push(s.clone()),
            _ => {}
        }
    }
    let frags: Vec<&Fragment> = fragments.iter().collect();
    // Root fragments have no name to be reached by.
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, f) in frags.iter().enumerate() {
        if let Some(fid) = &f.fid {
            by_name.entry(fid.as_str()).or_default().push(i);
        }
    }
    let refs: Vec<Vec<usize>> = frags
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut found = Vec::new();
            names(&f.value, None, &mut found);
            let mut out: Vec<usize> = found
                .iter()
                .flat_map(|n| by_name.get(n.as_str()).into_iter().flatten().copied())
                .filter(|&j| j != i)
                .filter(|&j| !(f.ftype.as_str() == "$164" && frags[j].ftype.as_str() == "$164"))
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect();
    let reached = |start: usize| {
        let mut seen = vec![false; frags.len()];
        let mut todo = refs[start].clone();
        while let Some(k) = todo.pop() {
            if !seen[k] {
                seen[k] = true;
                todo.extend(&refs[k]);
            }
        }
        seen
    };
    let mut order: Vec<usize> = (0..frags.len()).collect();
    order.sort_by_cached_key(|&i| frags[i].sort_key());
    let mut out = Vec::new();
    for &i in &order {
        for (depends, dependant) in DEPENDENCIES {
            if frags[i].ftype.as_str() != depends {
                continue;
            }
            let seen = reached(i);
            let found: Vec<Value> = order
                .iter()
                .filter(|&&j| seen[j] && frags[j].ftype.as_str() == dependant)
                .map(|&j| sym(frags[j].fid.as_ref().unwrap().as_str()))
                .collect();
            if !found.is_empty() {
                out.push(st([
                    ("$155", sym(frags[i].fid.as_ref().unwrap().as_str())),
                    ("$254", Value::List(found)),
                ]));
            }
        }
    }
    out
}

/// The picture features `_declare_content_features` (:2288) adds: yj_hdv for a resource over
/// 1920 px on a side, yj_jpg_rst_marker_present for a JPEG holding a restart marker.
pub(crate) fn picture_features(fragments: &FragmentList) -> Vec<&'static str> {
    let resources: Vec<&deckle_kfx::ion::Struct> = fragments
        .iter()
        .filter(|f| f.ftype.as_str() == "$164")
        .filter_map(|f| f.value.as_struct())
        .collect();
    let px = |r: &deckle_kfx::ion::Struct, k: &str| r.get(k).and_then(Value::as_int).unwrap_or(0);
    let mut features = Vec::new();
    if resources
        .iter()
        .any(|r| px(r, "$422").max(px(r, "$423")) > 1920)
    {
        features.push("yj_hdv");
    }
    let jpeg_locations: Vec<&str> = resources
        .iter()
        .filter(|r| {
            r.get("$161")
                .and_then(Value::as_sym)
                .is_some_and(|s| s.as_str() == "$285")
        })
        .filter_map(|r| r.get("$165").and_then(Value::as_str))
        .collect();
    let restart = fragments.iter().any(|f| {
        f.ftype.as_str() == "$417"
            && f.fid
                .as_ref()
                .is_some_and(|fid| jpeg_locations.contains(&fid.as_str()))
            && f.value.as_blob().is_some_and(|b| {
                b.windows(2)
                    .any(|w| w[0] == 0xFF && (0xD0..=0xD7).contains(&w[1]))
            })
    });
    if restart {
        features.push("yj_jpg_rst_marker_present");
    }
    features
}

/// `build_fragment_259`'s picture entry (:4352).
pub(crate) fn picture_entry(
    g: &mut Generator,
    p: &Picture,
    position: i64,
    style: &str,
) -> deckle_kfx::ion::Struct {
    g.local(&p.resource);
    let Value::Struct(mut entry) = st([
        ("$155", Value::Int(position)),
        ("$157", sym(style)),
        ("$159", sym("$271")),
        ("$584", Value::string(&p.alt)),
        ("$175", sym(&p.resource)),
    ]) else {
        unreachable!()
    };
    if p.inline {
        entry.insert("$601", sym("$283"));
    }
    entry
}

/// A contents page's pictures the converter replaced (#117), a chunk each.
pub(crate) fn push_preserved(
    tokens: &[String],
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<(), GenerateError> {
    for token in tokens {
        if !py::strip(&IMG_TOKEN.replace_all(token, "")).is_empty() {
            return Err(Unsupported("words kept from a replaced contents page"));
        }
        for m in IMG_TOKEN.captures_iter(token) {
            if let Some(picture) = picture_of(&m, images) {
                chunks.push(Chunk {
                    picture: Some(picture),
                    ..Chunk::text(String::new())
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::up;

    /// `_up` of -inf raises in kfxgen (`Decimal.quantize` of an infinity): an error here, not
    /// a panic. NaN is 100, as `min(100.0, nan)` is in Python.
    #[test]
    fn up_of_a_number_that_is_not_finite() {
        assert!(up(f64::NEG_INFINITY).is_err());
        assert_eq!(up(f64::NAN).unwrap(), 100.0);
        assert_eq!(up(f64::INFINITY).unwrap(), 100.0);
    }
}
