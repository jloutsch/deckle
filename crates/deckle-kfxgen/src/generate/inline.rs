// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs inside a paragraph: a block's spans and anchors as kfxgen moves them onto its chunks,
//! the `$266` anchors links land on, and the `$142` runs of a `$259` entry with their `$157`
//! styles (`_build_chapter_content`, `native_generator.py:5396`; `inline_style.py`).
use std::collections::{BTreeMap, BTreeSet};

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::{Struct, Value};

use super::text::{Style157, Styles};
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
    Link(String),
}

/// A run's flags: a Python frozenset, kept sorted so nothing depends on set order.
pub(crate) type Flags = BTreeSet<Flag>;

/// `(start, length, flags)`, in characters.
#[derive(Clone, Debug)]
pub(crate) struct Span {
    pub start: i64,
    pub len: i64,
    pub flags: Flags,
}

fn flag_of(f: &PyValue) -> Result<Flag, GenerateError> {
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
            "noteref" => return Err(Unsupported("pop-up notes")),
            s if s.contains('@') => return Err(Unsupported("a shift inside another")),
            _ => return Err(Unsupported("a span flag")),
        },
        PyValue::Tuple(kv) if kv.len() == 2 => match (kv[0].as_str(), &kv[1]) {
            (Some("link"), PyValue::Str(target)) => Flag::Link(target.clone()),
            (Some("ruby"), _) => return Err(Unsupported("ruby")),
            (Some("tcy"), _) => return Err(Unsupported("tate-chu-yoko")),
            (Some("bidi" | "lang"), _) => return Err(Unsupported("text direction")),
            (Some("mono"), _) => return Err(Unsupported("monospace runs")),
            (Some("size"), _) => return Err(Unsupported("sized runs")),
            (Some("link-colour"), _) => return Err(Unsupported("link colours")),
            (Some("picture"), _) => return Err(Unsupported("images")),
            (
                Some(
                    "small-caps" | "transform" | "spacing" | "colour" | "orientation"
                    | "emphasis-marks",
                ),
                _,
            ) => return Err(Unsupported("small caps, case, spacing and colour in runs")),
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
        parts.push((*start, *len, flags));
    }
    let mut out = Vec::with_capacity(parts.len());
    for (start, len, flags) in parts {
        let flags: Flags = flags.iter().map(flag_of).collect::<Result<_, _>>()?;
        // `_direction_layers` would take the links in set order.
        if flags.iter().filter(|f| matches!(f, Flag::Link(_))).count() > 1 {
            return Err(Unsupported("a run in two links"));
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

/// `_append_text_with_spans` (:5507) and the keys `_emit_paragraph` (:5847) gives its first
/// chunk, for a paragraph that is one chunk of `len` characters at offset 0 of its text: the
/// runs inside it, and the anchors, each at its offset when it is in the text, else at 0.
pub(crate) fn place(spans: &[Span], anchors: &Anchors, len: i64) -> (Vec<Span>, Anchors) {
    let runs = spans
        .iter()
        .filter_map(|sp| {
            let a = sp.start.max(0);
            let b = (sp.start + sp.len).min(len);
            (b > a).then(|| Span {
                start: a,
                len: b - a,
                flags: sp.flags.clone(),
            })
        })
        .collect();
    // One after the last word lands on the last character (#487).
    let last = (len - 1).max(0);
    let (mine, rest): (Vec<&String>, Vec<&String>) = anchors
        .keys
        .iter()
        .partition(|k| (0..=len).contains(&anchors.offset(k)));
    let mut placed = Anchors {
        keys: mine.iter().map(|k| (*k).clone()).collect(),
        offsets: mine
            .iter()
            .map(|k| ((*k).clone(), anchors.offset(k).min(last)))
            .collect(),
    };
    let rest: Vec<String> = rest.into_iter().cloned().collect();
    if !rest.is_empty() {
        placed.attach(&rest);
    }
    (runs, placed)
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
}

/// The body's `$266` anchors (:6728, :6783): one per link target something in the book
/// declares, named in the order of the sorted targets. Returns target → anchor name.
pub(crate) fn build_body_anchors(
    g: &mut Generator,
    chunks: &[ChunkLinks],
    anchor_names: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut key_to_chunk: BTreeMap<&str, (usize, i64)> = BTreeMap::new();
    let mut file_to_chunk: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, chunk) in chunks.iter().enumerate() {
        for key in &chunk.anchors.keys {
            key_to_chunk
                .entry(key)
                .or_insert((i, chunk.anchors.offset(key)));
            let file = key.split_once('#').map_or(key.as_str(), |(f, _)| f);
            file_to_chunk.entry(file).or_insert(i);
        }
    }
    let targets: BTreeSet<&str> = chunks
        .iter()
        .flat_map(|c| c.spans)
        .flat_map(|sp| &sp.flags)
        .filter_map(|f| match f {
            Flag::Link(t) => Some(t.as_str()),
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
        body_anchor_map.insert(target.to_owned(), name.clone());
        anchor_names.push(name);
    }
    body_anchor_map
}

/// A contents line's `$266` (:6765): the start of the chapter it names.
pub(crate) fn build_toc_anchor(
    g: &mut Generator,
    anchor_names: &mut Vec<String>,
    chapter_start_positions: &[i64],
    target: i64,
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
    let name = format!("toc_anchor_{}", anchor_names.len());
    let f = build_fragment_266(g, &name, position, 0);
    g.push(f);
    anchor_names.push(name.clone());
    Ok(name)
}

/// `_direction_layers` (:402) for runs with no direction element: each link one run over its
/// whole text, carrying the flags common to it, and the runs inside it layered on it with
/// what it doesn't carry. `(start, length, flags, under)`, outer runs first.
fn link_layers(spans: &[Span]) -> Vec<(i64, i64, Flags, Vec<Flags>)> {
    // [start, end, the link flag]; adjacent runs of one link are one layer.
    let mut links: Vec<(i64, i64, &Flag)> = Vec::new();
    for sp in spans {
        for f in sp.flags.iter().filter(|f| matches!(f, Flag::Link(_))) {
            match links.last_mut() {
                Some(last) if last.2 == f && last.1 == sp.start => last.1 = sp.start + sp.len,
                _ => links.push((sp.start, sp.start + sp.len, f)),
            }
        }
    }
    if links.is_empty() {
        return spans
            .iter()
            .map(|sp| (sp.start, sp.len, sp.flags.clone(), Vec::new()))
            .collect();
    }
    // Levels in quarters: links at 0.5, a run 0.75 above the layers it's in. Every link is
    // at one level, so none carries less for another above it.
    let carried: Vec<Flags> = links
        .iter()
        .map(|&(a, b, _)| {
            let mut inside = spans
                .iter()
                .filter(|sp| a <= sp.start && sp.start + sp.len <= b);
            let first = inside.next().map(|sp| sp.flags.clone()).unwrap_or_default();
            inside.fold(first, |acc, sp| &acc & &sp.flags)
        })
        .collect();
    let mut out: Vec<(i64, i64, Flags, Vec<Flags>, i64)> = links
        .iter()
        .zip(&carried)
        .map(|(&(a, b, _), c)| (a, b - a, c.clone(), Vec::new(), 2))
        .collect();
    for sp in spans {
        let inside: Vec<&Flags> = links
            .iter()
            .zip(&carried)
            .filter(|((a, b, _), _)| *a <= sp.start && sp.start + sp.len <= *b)
            .map(|(_, c)| c)
            .collect();
        let rest: Flags = sp
            .flags
            .iter()
            .filter(|f| !inside.iter().any(|c| c.contains(f)))
            .cloned()
            .collect();
        if !rest.is_empty() {
            let level = if inside.is_empty() { 3 } else { 5 };
            let under = inside.into_iter().cloned().collect();
            out.push((sp.start, sp.len, rest, under, level));
        }
    }
    out.sort_by_key(|x| (x.0, x.4));
    out.into_iter()
        .map(|(s, n, f, u, _)| (s, n, f, u))
        .collect()
}

/// `_emphasis_style` (:7245) with no font, size, typography or cell: a run's `$157`.
fn emphasis_style(flags: &Flags, blk_bold: bool, blk_italic: bool) -> Style157 {
    let has = |f: Flag| flags.contains(&f);
    let bold = has(Flag::Bold) || (blk_bold && !has(Flag::NotBold));
    let italic = has(Flag::Italic) || (blk_italic && !has(Flag::NotItalic));
    let mut style = Style157 {
        bold,
        italic,
        // Set back to upright inside an italic paragraph (#367 review).
        upright: blk_italic && !italic,
        underline: has(Flag::Underline),
        strike: has(Flag::Strike),
        ..Style157::default()
    };
    // Superscript wins if a run is marked both (#52, #123). kfxgen's
    // KFXGEN_SUPERSCRIPT_FONT_SIZE override isn't read (DIVERGENCES.md).
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
    start: i64,
    len: i64,
    style: Option<String>,
    anchor: Option<String>,
}

/// The runs of a chunk's entry (:7851–8037): each layered span's style, and its link's
/// anchor when the book declares the target. A run that is only a found link has no style.
/// `blk_bold`: the entry's own text is bold (a chapter heading).
pub(crate) fn entry_runs(
    g: &mut Generator,
    styles: &mut Styles,
    extra_style_names: &mut Vec<String>,
    spans: &[Span],
    blk_bold: bool,
    body_anchor_map: &BTreeMap<String, String>,
) -> Vec<Run> {
    let mut runs = Vec::new();
    for (start, len, flags, under) in link_layers(spans) {
        // A run layered on others starts from their weight and slant (#715).
        let (mut run_b, mut run_i) = (blk_bold, false);
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
        }
        if flags.is_empty() {
            continue;
        }
        let anchor = flags.iter().find_map(|f| match f {
            Flag::Link(t) => body_anchor_map.get(t).cloned(),
            _ => None,
        });
        let has_emphasis = flags
            .iter()
            .any(|f| !matches!(f, Flag::Link(_) | Flag::Baseline));
        let style = (has_emphasis || anchor.is_none()).then(|| {
            let name = styles.allocate(g, "_em", emphasis_style(&flags, run_b, run_i));
            if !extra_style_names.contains(&name) {
                extra_style_names.push(name.clone());
            }
            name
        });
        runs.push(Run {
            start,
            len,
            style,
            anchor,
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
        if run.style.is_none() && run.anchor.is_none() {
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
            span.insert("$179", sym(anchor));
        }
        if let Some(style) = &run.style {
            g.local(style);
            span.insert("$157", sym(style));
        }
        out.push(Value::Struct(span));
    }
    if link.is_some() || !runs.is_empty() {
        entry.insert("$142", Value::List(out));
    }
}
