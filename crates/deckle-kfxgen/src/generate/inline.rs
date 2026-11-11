// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs inside a paragraph: a block's spans and anchors as kfxgen moves them onto its chunks,
//! the `$266` anchors links land on, and the `$142` runs of a `$259` entry with their `$157`
//! styles (`_build_chapter_content`, `native_generator.py:5396`; `inline_style.py`).
use std::collections::{BTreeMap, BTreeSet};

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::{Struct, Value};

use super::block_styles::{Length, Sizes, number};
use super::fonts::FontTable;
use super::notes::Popups;
use super::styles::{Style157, StyleArgs, Styles};
use super::typography::{Typography, read_link_colour, read_typography_flag};
use super::writing::{
    Bidi, direction_layers, read_bidi_flag, read_ruby_flag, read_tcy_flag, without_ruby_or_tcy,
};
use super::{GenerateError, Generator, model_err, py, st, sym};
use crate::pyvalue::{PyValue, truthy};

use GenerateError::Unsupported;

/// `inline_style.EXTERNAL_LINK_PREFIX` (:307): a link leaving the book.
const EXTERNAL_LINK_PREFIX: &str = "url:";
/// `SUPERSCRIPT_FONT_SIZE`, `SUBSCRIPT_FONT_SIZE` (:691, :695).
const SHIFT_FONT_SIZE: f64 = 0.75;

/// A run flag `_emphasis_style` (:7245) writes, or a link (`inline_style.py`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Flag {
    Bold,
    Italic,
    NotBold,
    NotItalic,
    Super,
    Sub,
    TextTop,
    TextBottom,
    Baseline,
    Underline,
    Strike,
    /// A link to a note (#445).
    NoteRef,
    Link(String),
    /// `make_mono_flag` (#242): monospace, in these families.
    Mono(Vec<String>),
    /// `make_size_flag` (#241): a size of its own, in CSS px.
    Size(Px),
    /// `make_bidi_flag` (#710): an inline element's own direction, as (serial, depth, keys).
    Bidi(i64, i64, Bidi),
    /// An inline element's language when it sets no direction (#710).
    Lang(String),
    /// `make_ruby_flag` (#236): the base of a reading, as (serial, reading).
    Ruby(i64, String),
    /// `ORIENTATION_FLAG` (#247): text-orientation in vertical text.
    Orientation(String),
    /// `EMPHASIS_MARKS_FLAG` (#247): text-emphasis-style, marks beside vertical text.
    EmphasisMarks(String),
    /// `make_tcy_flag` (#247): characters set across a vertical line, as (serial, text), on
    /// the one placeholder character they take.
    Tcy(i64, String),
    /// `LINK_COLOUR_FLAG` (#728): a link's own colour, or None, and its colour as text.
    LinkColour(Option<i64>, i64),
    /// `TYPOGRAPHY_FLAGS` (#239): a run's own small caps and text-transform keywords, letter
    /// spacing and ARGB colour.
    SmallCaps(String),
    Transform(String),
    Spacing(Length),
    Colour(i64),
}

/// A size in CSS px, ordered by `total_cmp` so a run's flags can be a set; read with no sign
/// on 0, which Python's `==` ignores.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Px(f64);

impl PartialEq for Px {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Px {}

impl PartialOrd for Px {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Px {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// A run's flags: a Python frozenset, kept sorted so nothing depends on set order.
pub(crate) type Flags = BTreeSet<Flag>;

/// `run_font_size`: the size a run's size flag carries.
pub(crate) fn run_font_size(flags: &Flags) -> Option<f64> {
    flags.iter().find_map(|f| match f {
        Flag::Size(px) => Some(px.0),
        _ => None,
    })
}

/// `mono_families`: the families a run's monospace flag carries.
fn mono_families(flags: &Flags) -> Option<&[String]> {
    flags.iter().find_map(|f| match f {
        Flag::Mono(families) => Some(&families[..]),
        _ => None,
    })
}

/// `(start, length, flags)`, in characters.
#[derive(Clone, Debug)]
pub(crate) struct Span {
    pub start: i64,
    pub len: i64,
    pub flags: Flags,
}

fn flag_of(f: &PyValue, at: &str) -> Result<Flag, GenerateError> {
    Ok(match f {
        PyValue::Str(s) => match s.as_str() {
            "bold" => Flag::Bold,
            "italic" => Flag::Italic,
            "not-bold" => Flag::NotBold,
            "not-italic" => Flag::NotItalic,
            "super" => Flag::Super,
            "sub" => Flag::Sub,
            "text-top" => Flag::TextTop,
            "text-bottom" => Flag::TextBottom,
            "baseline" => Flag::Baseline,
            "underline" => Flag::Underline,
            "line-through" => Flag::Strike,
            "noteref" => Flag::NoteRef,
            s if s.contains('@') => return Err(Unsupported("a shift inside another")),
            _ => return Err(Unsupported("a span flag")),
        },
        PyValue::Tuple(kv) if kv.len() == 2 => match (kv[0].as_str(), &kv[1]) {
            (Some("link"), PyValue::Str(target)) => Flag::Link(target.clone()),
            (Some("ruby"), v) => read_ruby_flag(v, &format!("{at}.spans"))?,
            (Some("tcy"), v) => read_tcy_flag(v, &format!("{at}.spans"))?,
            (Some("bidi"), v) => read_bidi_flag(v, &format!("{at}.spans"))?,
            (Some("lang"), PyValue::Str(lang)) => Flag::Lang(lang.clone()),
            (Some("lang"), _) => {
                return Err(model_err(format!("{at}.spans: a language, a string")));
            }
            (Some("mono"), families) => Flag::Mono(
                families
                    .as_list()
                    .and_then(|l| l.iter().map(|f| f.as_str().map(str::to_owned)).collect())
                    .ok_or_else(|| model_err(format!("{at}.spans: monospace families, strings")))?,
            ),
            (Some("size"), px) => {
                let px = number(px)
                    .ok_or_else(|| model_err(format!("{at}.spans: a run's size, a number")))?;
                Flag::Size(Px(if px == 0.0 { 0.0 } else { px }))
            }
            (Some("link-colour"), v) => read_link_colour(v, &format!("{at}.spans"))?,
            (Some("picture"), _) => return Err(Unsupported("images")),
            (Some("orientation"), PyValue::Str(v)) => Flag::Orientation(v.clone()),
            (Some("emphasis-marks"), PyValue::Str(v)) => Flag::EmphasisMarks(v.clone()),
            (Some("orientation" | "emphasis-marks"), _) => {
                return Err(model_err(format!(
                    "{at}.spans: a typography value, a string"
                )));
            }
            (Some(kind @ ("small-caps" | "transform" | "spacing" | "colour")), v) => {
                read_typography_flag(kind, v, &format!("{at}.spans"))?
            }
            _ => return Err(Unsupported("a span flag")),
        },
        _ => return Err(Unsupported("a span flag")),
    })
}

/// `make_math_flag`'s `("math", ...)`.
fn is_math(f: &PyValue) -> bool {
    matches!(f, PyValue::Tuple(kv) if kv.first().and_then(PyValue::as_str) == Some("math"))
}

/// A block's `spans`. An equation in any of them is reported before any other flag: math is
/// out of the port's scope.
pub(crate) fn read_spans(v: Option<&PyValue>, at: &str) -> Result<Vec<Span>, GenerateError> {
    let Some(v) = v.filter(|v| !matches!(v, PyValue::None)) else {
        return Ok(Vec::new());
    };
    let spans = v
        .as_list()
        .ok_or_else(|| model_err(format!("{at}.spans: a list")))?;
    let shape = || model_err(format!("{at}.spans: (start, length, flags) tuples"));
    let mut parts = Vec::with_capacity(spans.len());
    for span in spans {
        let [start, len, flags] = span.as_list().ok_or_else(shape)? else {
            return Err(shape());
        };
        let (PyValue::Int(start), PyValue::Int(len), PyValue::Set(flags)) = (start, len, flags)
        else {
            return Err(shape());
        };
        if flags.iter().any(is_math) {
            return Err(Unsupported("math"));
        }
        // The port takes text lengths from a run's ends and subtracts one end from another,
        // in an i64 where kfxgen's ints have no bound (DIVERGENCES.md).
        let bound = 1i64 << 61;
        if !start
            .checked_add(*len)
            .is_some_and(|end| [*start, end].iter().all(|x| (-bound..=bound).contains(x)))
        {
            return Err(model_err(format!(
                "{at}.spans: a run whose start and end are within ±2^61"
            )));
        }
        parts.push((*start, *len, flags));
    }
    let mut out = Vec::with_capacity(parts.len());
    for (start, len, flags) in parts {
        let flags: Flags = flags
            .iter()
            .map(|f| flag_of(f, at))
            .collect::<Result<_, _>>()?;
        // `_direction_layers` would take the links in set order, `run_font_size`,
        // `mono_families` and the run's language the first size, family or language.
        let count = |kind: fn(&Flag) -> bool| flags.iter().filter(|f| kind(f)).count();
        if count(|f| matches!(f, Flag::Link(_))) > 1 {
            return Err(Unsupported("a run in two links"));
        }
        if count(|f| matches!(f, Flag::Lang(_))) > 1 {
            return Err(Unsupported("a run in two languages"));
        }
        if count(|f| matches!(f, Flag::Ruby(..))) > 1 {
            return Err(Unsupported("a run under two rubies"));
        }
        if count(|f| matches!(f, Flag::Tcy(..))) > 1 {
            return Err(Unsupported("a run of two tate-chu-yoko"));
        }
        if count(|f| matches!(f, Flag::Orientation(_))) > 1
            || count(|f| matches!(f, Flag::EmphasisMarks(_))) > 1
            || count(|f| matches!(f, Flag::SmallCaps(_))) > 1
            || count(|f| matches!(f, Flag::Transform(_))) > 1
            || count(|f| matches!(f, Flag::Spacing(_))) > 1
            || count(|f| matches!(f, Flag::Colour(_))) > 1
        {
            return Err(Unsupported("a run of two values of one kind of typography"));
        }
        if count(|f| matches!(f, Flag::LinkColour(..))) > 1 {
            return Err(Unsupported("a run of two link colours"));
        }
        if count(|f| matches!(f, Flag::Size(_))) > 1 || count(|f| matches!(f, Flag::Mono(_))) > 1 {
            return Err(Unsupported("a run of two sizes or two monospace families"));
        }
        out.push(Span { start, len, flags });
    }
    Ok(out)
}

/// A line of a rebuilt contents page: a link to a chapter's start.
pub(crate) struct TocLink {
    pub text: String,
    pub target: i64,
    pub level: i64,
    /// A place in the chapter, landed on when a contents entry names it (#363).
    pub key: Option<String>,
}

impl TocLink {
    pub(crate) fn read(link: &PyValue, at: &str) -> Result<Self, GenerateError> {
        // A line with no target is one of the page's own headings (#585); a heading the
        // contents lists prints as that heading (#595).
        let Some(target) = link.get("target_chapter_idx") else {
            return Err(Unsupported("a contents page's own headings"));
        };
        if truthy(link.get("heading_level")) {
            return Err(Unsupported("a heading on a contents page"));
        }
        let int = |v: Option<&PyValue>, what: &str| {
            v.and_then(PyValue::as_int)
                .ok_or_else(|| model_err(format!("{at}.{what}: an int")))
        };
        let level = match link.get("level") {
            None | Some(PyValue::None) => 0,
            v => int(v, "level")?,
        };
        // Its indent is `f"{6.25 * level:g}"`: Rust prints the same while it has 6 digits.
        if level.abs() >= 1600 {
            return Err(Unsupported("a contents line nested 1600 levels deep"));
        }
        Ok(TocLink {
            text: match link.get("text") {
                Some(PyValue::Str(s)) => s.clone(),
                _ => return Err(model_err(format!("{at}.text: a string"))),
            },
            target: int(Some(target), "target_chapter_idx")?,
            level,
            key: match link.get("target_key") {
                None | Some(PyValue::None) => None,
                Some(PyValue::Str(k)) => Some(k.clone()),
                Some(_) => return Err(model_err(format!("{at}.target_key: a string or None"))),
            },
        })
    }
}

/// A block's or chunk's `anchor_keys` and `anchor_offsets`. Offsets are only looked up, so a
/// map's order can't reach the output.
#[derive(Clone, Default)]
pub(crate) struct Anchors {
    pub keys: Vec<String>,
    pub offsets: BTreeMap<String, i64>,
}

impl Anchors {
    pub(crate) fn read(b: &PyValue, at: &str) -> Result<Self, GenerateError> {
        let keys = match b.get("anchor_keys") {
            None | Some(PyValue::None) => Vec::new(),
            Some(v) => v
                .as_list()
                .and_then(|l| l.iter().map(|k| k.as_str().map(str::to_owned)).collect())
                .ok_or_else(|| model_err(format!("{at}.anchor_keys: a list of strings")))?,
        };
        let offsets = match b.get("anchor_offsets") {
            None | Some(PyValue::None) => BTreeMap::new(),
            Some(PyValue::Dict(pairs)) => pairs
                .iter()
                .map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_int()?)))
                .collect::<Option<_>>()
                .ok_or_else(|| model_err(format!("{at}.anchor_offsets: str to int")))?,
            Some(_) => return Err(model_err(format!("{at}.anchor_offsets: a dict"))),
        };
        Ok(Anchors { keys, offsets })
    }

    fn offset(&self, key: &str) -> i64 {
        self.offsets.get(key).copied().unwrap_or(0)
    }

    /// `_attach_keys` (:5899): `keys` added at offset 0.
    pub(crate) fn attach(&mut self, keys: &[String]) {
        self.attach_at(keys, &BTreeMap::new());
    }

    /// `keys` added at their offsets in `offsets`, else 0 (:6622).
    pub(crate) fn attach_at(&mut self, keys: &[String], offsets: &BTreeMap<String, i64>) {
        self.keys.extend(keys.iter().cloned());
        dedupe_keys(&mut self.keys);
        for k in keys {
            self.offsets
                .insert(k.clone(), offsets.get(k).copied().unwrap_or(0));
        }
    }
}

/// Where each declared id first lives, as (chunk, its offset), and where each file first
/// appears (:6728).
pub(crate) type IdMaps<'a> = (BTreeMap<&'a str, (usize, i64)>, BTreeMap<&'a str, usize>);

pub(crate) fn id_maps<'a>(anchors: impl IntoIterator<Item = &'a Anchors>) -> IdMaps<'a> {
    let mut key_to_chunk = BTreeMap::new();
    let mut file_to_chunk = BTreeMap::new();
    for (i, anchors) in anchors.into_iter().enumerate() {
        for key in &anchors.keys {
            key_to_chunk
                .entry(key.as_str())
                .or_insert((i, anchors.offset(key)));
            let file = key.split_once('#').map_or(key.as_str(), |(f, _)| f);
            file_to_chunk.entry(file).or_insert(i);
        }
    }
    (key_to_chunk, file_to_chunk)
}

/// `_dedupe_keys` (:1571).
fn dedupe_keys(keys: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    keys.retain(|k| seen.insert(k.clone()));
}

/// The length of the space `lstrip` takes off `text`.
fn lead(text: &str) -> i64 {
    (py::len(text) - py::len(py::lstrip(text))) as i64
}

/// `_title_spans` (:1343): a block's runs moved onto a heading printing its first `length`
/// characters after any leading space.
pub(crate) fn title_spans(text: &str, spans: &[Span], length: usize) -> Vec<Span> {
    let lead = lead(text);
    spans
        .iter()
        .filter_map(|sp| {
            let s = (sp.start - lead).max(0);
            let e = (sp.start + sp.len - lead).min(length as i64);
            (e > s).then(|| Span {
                start: s,
                len: e - s,
                flags: sp.flags.clone(),
            })
        })
        .collect()
}

/// `_heading_offsets` (:1332): a block's anchor offsets in a heading printing `title`.
pub(crate) fn heading_offsets(text: &str, anchors: &Anchors, title: &str) -> BTreeMap<String, i64> {
    let lead = lead(text);
    let last = (py::len(title) as i64 - 1).max(0);
    anchors
        .offsets
        .iter()
        .map(|(k, &off)| (k.clone(), (off - lead).max(0).min(last)))
        .collect()
}

/// `_trim_text` (:909): the runs and anchor offsets of a paragraph whose first `cut`
/// characters are cut, leaving `rest` characters.
pub(crate) fn trim(spans: &mut Vec<Span>, anchors: &mut Anchors, cut: i64, rest: i64) {
    *spans = spans
        .iter()
        .filter_map(|sp| {
            let start = (sp.start - cut).max(0);
            let end = (sp.start + sp.len - cut).min(rest);
            (end > start).then(|| Span {
                start,
                len: end - start,
                flags: sp.flags.clone(),
            })
        })
        .collect();
    for v in anchors.offsets.values_mut() {
        *v = (*v - cut).max(0).min(rest);
    }
}

/// `build_fragment_266` (:4155).
fn build_fragment_266(g: &mut Generator, name: &str, position: i64, offset: i64) -> Fragment {
    g.local(name);
    let Value::Struct(mut target) = st([("$155", Value::Int(position))]) else {
        unreachable!()
    };
    if offset != 0 {
        target.insert("$143", Value::Int(offset));
    }
    let value = st([("$180", sym(name)), ("$183", Value::Struct(target))]);
    Fragment::new("$266", Some(name), value)
}

/// `build_fragment_266_uri` (:4144).
fn build_fragment_266_uri(g: &mut Generator, name: &str, uri: &str) -> Fragment {
    g.local(name);
    let value = st([("$180", sym(name)), ("$186", Value::string(uri))]);
    Fragment::new("$266", Some(name), value)
}

/// What a chunk carries that the anchors read.
pub(crate) struct ChunkLinks<'a> {
    pub spans: &'a [Span],
    pub anchors: &'a Anchors,
    pub position: i64,
    /// A cell's text or a paragraph in one (:6846).
    pub in_cell: bool,
}

/// The body's `$266` anchors (:6728, :6783): one per link target something in the book
/// declares, named in the order of the sorted targets. Returns target → anchor name.
pub(crate) fn build_body_anchors(
    g: &mut Generator,
    chunks: &[ChunkLinks],
    anchor_names: &mut Vec<String>,
    popups: &mut Popups,
) -> BTreeMap<String, String> {
    let (key_to_chunk, file_to_chunk) = id_maps(chunks.iter().map(|c| c.anchors));
    let targets: BTreeSet<&str> = chunks
        .iter()
        .flat_map(|c| c.spans)
        .flat_map(|sp| &sp.flags)
        .filter_map(|f| match f {
            // An empty target is none (`if target`, :6790).
            Flag::Link(t) if !t.is_empty() => Some(t.as_str()),
            _ => None,
        })
        .collect();
    let mut body_anchor_map = BTreeMap::new();
    if targets.is_empty() {
        return body_anchor_map;
    }
    // A link in other capitals still lands; an exact match comes first (#476).
    let mut folded_keys: BTreeMap<String, &str> = BTreeMap::new();
    for &k in key_to_chunk.keys() {
        folded_keys.entry(k.to_lowercase()).or_insert(k);
    }
    let mut folded_files: BTreeMap<String, &str> = BTreeMap::new();
    for &f in file_to_chunk.keys() {
        folded_files.entry(f.to_lowercase()).or_insert(f);
    }
    for target in targets {
        if let Some(uri) = target.strip_prefix(EXTERNAL_LINK_PREFIX) {
            let name = format!("ext_anchor_{}", body_anchor_map.len());
            let f = build_fragment_266_uri(g, &name, uri);
            g.push(f);
            body_anchor_map.insert(target.to_owned(), name.clone());
            anchor_names.push(name);
            continue;
        }
        let key = if key_to_chunk.contains_key(target) {
            Some(target)
        } else {
            folded_keys.get(&target.to_lowercase()).copied()
        };
        let mut found = key.and_then(|k| key_to_chunk.get(k).copied());
        if found.is_none() && !target.contains('#') {
            // A whole-file target names the file's start.
            let file = if file_to_chunk.contains_key(target) {
                Some(target)
            } else {
                folded_files.get(&target.to_lowercase()).copied()
            };
            found = file.and_then(|f| file_to_chunk.get(f)).map(|&i| (i, 0));
        }
        // Nothing declares it: the run stays unlinked.
        let Some((idx, offset)) = found else {
            continue;
        };
        let name = format!("body_anchor_{}", body_anchor_map.len());
        let f = build_fragment_266(g, &name, chunks[idx].position, offset);
        g.push(f);
        popups.landed(target, &name, chunks[idx].position, chunks[idx].in_cell);
        body_anchor_map.insert(target.to_owned(), name.clone());
        anchor_names.push(name);
    }
    body_anchor_map
}

/// A contents line's `$266` (:6765): the start of the chapter it names, or the place in it
/// its key names (#363).
pub(crate) fn build_toc_anchor(
    g: &mut Generator,
    anchor_names: &mut Vec<String>,
    chapter_start_positions: &[i64],
    target: i64,
    place: Option<(i64, i64)>,
) -> Result<String, GenerateError> {
    let n = chapter_start_positions.len() as i64;
    // `target < len(...)`, then Python's indexing: a negative index counts from the end.
    let i = if target >= n {
        0
    } else if target < 0 {
        target + n
    } else {
        target
    };
    let position = *usize::try_from(i)
        .ok()
        .and_then(|i| chapter_start_positions.get(i))
        .ok_or_else(|| model_err("toc_links: a target_chapter_idx out of range"))?;
    let (position, offset) = place.unwrap_or((position, 0));
    let name = format!("toc_anchor_{}", anchor_names.len());
    let f = build_fragment_266(g, &name, position, offset);
    g.push(f);
    anchor_names.push(name.clone());
    Ok(name)
}

/// `_emphasis_style` (:7245) with no typography: a run's `$157`, from its weight and slant
/// and those it starts from (`run_b`, `run_i`), its family (#50) and its own size (#241).
fn emphasis_style(
    fonts: &FontTable,
    sizes: &Sizes,
    flags: &Flags,
    (blk_bold, blk_italic): (bool, bool),
    families: &[String],
    base: &RunBase,
    link_found: bool,
) -> Style157 {
    let has = |f: Flag| flags.contains(&f);
    let bold = has(Flag::Bold) || (blk_bold && !has(Flag::NotBold));
    let italic = has(Flag::Italic) || (blk_italic && !has(Flag::NotItalic));
    let mono = mono_families(flags).filter(|m| !m.is_empty());
    let mut style = Style157 {
        bold,
        italic,
        // Set back to upright inside an italic paragraph (#367 review).
        upright: blk_italic && !italic,
        underline: has(Flag::Underline),
        strike: has(Flag::Strike),
        font_family: fonts.family(mono.unwrap_or(families), bold, italic),
        no_align: base.in_cell,
        ..Style157::default()
    };
    if let Some(px) = run_font_size(flags).filter(|_| base.sized) {
        let size = sizes.rem(Some(px));
        if size != base.blk_fs {
            // 1 is written too, when the paragraph's isn't (#375 review).
            style.font_size = size;
            style.explicit_size = Some(size == 1.0);
        }
    }
    // Superscript wins if a run is marked both (#52, #123). kfxgen's
    // KFXGEN_SUPERSCRIPT_FONT_SIZE override isn't read (DIVERGENCES.md).
    style.typography = Typography::of_flags(flags);
    for f in flags {
        match f {
            Flag::LinkColour(link, _) if link_found => style.link_colour = *link,
            // A link whose target isn't found is written as text: its colour as the
            // text's (QA on #732, P732b).
            Flag::LinkColour(_, text) if style.typography.colour.is_none() => {
                style.typography.colour = Some(*text);
            }
            _ => {}
        }
    }
    if has(Flag::Super) {
        (style.font_size, style.baseline_style) = (SHIFT_FONT_SIZE, Some("$370"));
    } else if has(Flag::Sub) {
        (style.font_size, style.baseline_style) = (SHIFT_FONT_SIZE, Some("$371"));
    } else if has(Flag::TextTop) {
        style.baseline_style = Some("$447");
    } else if has(Flag::TextBottom) {
        style.baseline_style = Some("$449");
    } else if has(Flag::Baseline) {
        style.baseline_style = Some("$350");
    }
    style
}

/// A `$142` run of a `$259` entry: where, its `$157` and the `$266` it links to.
pub(crate) struct Run {
    pub start: i64,
    pub len: i64,
    style: Option<String>,
    anchor: Option<String>,
    /// Its anchor is a pop-up note's (#445).
    popup: bool,
    /// The event of a ruby over it: its chapter's ruby fragment and its number there (#236).
    pub ruby: Option<(String, i64)>,
}

impl Run {
    /// A ruby's event alone.
    pub(crate) fn ruby(start: i64, len: i64, ruby: (String, i64)) -> Self {
        Run {
            start,
            len,
            style: None,
            anchor: None,
            popup: false,
            ruby: Some(ruby),
        }
    }
}

/// What a chunk's runs start from (:7820): its paragraph's weight, slant, families and size,
/// and whether it is in a table cell. `bold`: a chapter heading, a header cell, a bold block.
pub(crate) struct RunBase<'a> {
    pub bold: bool,
    pub italic: bool,
    pub family: &'a [String],
    pub in_cell: bool,
    /// `_block_font_size`, or the size of the book's heading printing as kfxgen's (#721).
    pub blk_fs: f64,
    /// Runs' own sizes are written: not in kfxgen's own heading (#241).
    pub sized: bool,
    /// The characters of its drop cap's initial (#240).
    pub drop_cap: Option<i64>,
}

/// The runs of a chunk's entry (:7851–8037): each layered span's style, and its link's
/// anchor when the book declares the target. A run that is only a found link has no style.
#[allow(clippy::too_many_arguments)]
pub(crate) fn entry_runs(
    g: &mut Generator,
    styles: &mut Styles,
    extra_style_names: &mut Vec<String>,
    sizes: &Sizes,
    spans: &[Span],
    base: &RunBase,
    body_anchor_map: &BTreeMap<String, String>,
    popups: &Popups,
) -> Vec<Run> {
    let mut runs = Vec::new();
    for mut layer in direction_layers(&without_ruby_or_tcy(spans)) {
        // A run layered on others starts from their weight, slant and font (#715).
        let (mut run_b, mut run_i, mut run_fam) = (base.bold, base.italic, base.family);
        let under = std::mem::take(&mut layer.under);
        for u in &under {
            if u.contains(&Flag::Bold) {
                run_b = true;
            } else if u.contains(&Flag::NotBold) {
                run_b = false;
            }
            if u.contains(&Flag::Italic) {
                run_i = true;
            } else if u.contains(&Flag::NotItalic) {
                run_i = false;
            }
            if let Some(mono) = mono_families(u).filter(|m| !m.is_empty()) {
                run_fam = mono;
            }
        }
        // A language alone is a flag of the run's (#710's P01); its keys are written as they
        // are, not against the paragraph's (:7912).
        let bidi = layer.bidi_keys();
        let (start, len, mut flags) = (layer.start, layer.len, layer.flags);
        if flags.is_empty() && bidi.is_empty() {
            continue;
        }
        if base.drop_cap.is_some_and(|n| start + len <= n) {
            // The Kindle sizes a drop cap to its lines: Previewer gives the initial no size
            // of its own (#240).
            flags.retain(|f| !matches!(f, Flag::Size(_)));
            if flags.is_empty() && bidi.is_empty() {
                continue;
            }
        }
        let anchor = flags.iter().find_map(|f| match f {
            Flag::Link(t) => body_anchor_map.get(t).cloned(),
            _ => None,
        });
        // The link this run is in, its own or one it's layered on, has a target in the book
        // (#728); an empty target is none (`if t`, :7944).
        let link_found = [&flags].into_iter().chain(&under).flatten().all(
            |f| !matches!(f, Flag::Link(t) if !t.is_empty() && !body_anchor_map.contains_key(t)),
        );
        let mut has_emphasis = flags.iter().any(|f| match f {
            Flag::Link(_)
            | Flag::Baseline
            | Flag::NoteRef
            | Flag::Size(_)
            | Flag::Bidi(..)
            | Flag::Lang(_) => false,
            Flag::LinkColour(link, _) => link.is_some() || !link_found,
            Flag::Mono(families) => !families.is_empty(),
            _ => true,
        });
        let run_px = run_font_size(&flags);
        if base.sized && run_px.is_some_and(|px| sizes.rem(Some(px)) != base.blk_fs) {
            has_emphasis = true;
        } else if bidi.is_empty()
            && !flags.is_empty()
            && flags.iter().all(|f| matches!(f, Flag::Size(_)))
        {
            // Only a size, and the paragraph's: no run, as a paragraph wholly in a sized
            // span has none (#375). kfxgen also asks for no anchor, which only a link has.
            continue;
        }
        let style = if !bidi.is_empty() && !has_emphasis {
            // Only its direction: a style of those keys alone (#710).
            Some(styles.allocate_in(g, "_dir", StyleArgs::Direction, &bidi))
        } else {
            (has_emphasis || anchor.is_none()).then(|| {
                let style = emphasis_style(
                    &g.fonts,
                    sizes,
                    &flags,
                    (run_b, run_i),
                    run_fam,
                    base,
                    link_found,
                );
                let name = styles.allocate_in(g, "_em", style, &bidi);
                if !extra_style_names.contains(&name) {
                    extra_style_names.push(name.clone());
                }
                name
            })
        };
        runs.push(Run {
            start,
            len,
            style,
            popup: anchor.as_ref().is_some_and(|a| popups.anchors.contains(a)),
            anchor,
            ruby: None,
        });
    }
    runs
}

/// A contents line's link: its anchor, its style and its text's length.
pub(crate) type EntryLink = (String, String, i64);

/// An entry's `$142` (`build_fragment_259`, :4426–4485): a contents line's link over its
/// whole text, then its runs.
pub(crate) fn write_runs(
    g: &mut Generator,
    entry: &mut Struct,
    link: Option<&EntryLink>,
    runs: &[Run],
) {
    let mut out = Vec::new();
    if let Some((anchor, style, len)) = link {
        g.local(anchor);
        g.local(style);
        out.push(st([
            ("$143", Value::Int(0)),
            ("$144", Value::Int(*len)),
            ("$179", sym(anchor)),
            ("$157", sym(style)),
        ]));
    }
    for run in runs {
        if run.style.is_none() && run.anchor.is_none() && run.ruby.is_none() {
            continue;
        }
        let Value::Struct(mut span) = st([
            ("$143", Value::Int(run.start)),
            ("$144", Value::Int(run.len)),
        ]) else {
            unreachable!()
        };
        if let Some(anchor) = &run.anchor {
            g.local(anchor);
            if run.popup {
                span.insert("$616", sym("$617"));
            }
            span.insert("$179", sym(anchor));
        }
        if let Some(style) = &run.style {
            g.local(style);
            span.insert("$157", sym(style));
        }
        if let Some((fragment, number)) = &run.ruby {
            g.local(fragment);
            span.insert("$757", sym(fragment));
            span.insert("$758", Value::Int(*number));
        }
        out.push(Value::Struct(span));
    }
    if link.is_some() || !runs.is_empty() {
        entry.insert("$142", Value::List(out));
    }
}
