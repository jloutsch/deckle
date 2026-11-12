// SPDX-License-Identifier: GPL-3.0-only
//! Writing direction and language: a block's, a container's and an inline element's
//! direction, unicode-bidi and language as Kindle Previewer 4.0.3 writes them in `$157`
//! (#246, #605, #710, #714), and the runs an inline element's direction lays out
//! (`_direction_layers`, `native_generator.py:402`); ruby (#236); vertical text, and
//! characters set across its lines, 縦中横 (#247).
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::{Struct, Value};

use super::chapters::{Block, Chapter};
use super::chunks::Chunk;
use super::fragments::{MAX_CONTENT_FRAGMENT_SIZE, build_fragment_145};
use super::inline::{Flag, Flags, Run, Span, title_spans};
use super::{GenerateError, Generator, measure, model_err, py, st, sym};
use crate::pyvalue::PyValue;

/// `_BIDI_KEYS` (:71): the converter's names, and the `$157` keys Previewer writes.
const BIDI_KEYS: [(&str, &str); 3] = [
    ("lang", "$10"),
    ("unicode_bidi", "$674"),
    ("direction", "$682"),
];

/// `_BIDI_SYMBOLS` (:72): a direction's or unicode-bidi's symbol.
fn bidi_symbol(value: &str) -> Option<&'static str> {
    Some(match value {
        "rtl" => "$375",
        "ltr" => "$376",
        "embed" => "$675",
        "isolate" => "$676",
        "bidi-override" => "$677",
        "isolate-override" => "$678",
        "plaintext" => "$679",
        "normal" => "$350",
        _ => return None,
    })
}

/// A `bidi` dict: a block's, a container's or an inline element's direction and language.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Bidi {
    /// The value of each of [`BIDI_KEYS`], None where it is unset or empty.
    values: [Option<String>; 3],
    /// `set_here`: the keys the element sets itself, written even where the container
    /// around has the same (#605).
    set_here: Vec<String>,
    /// Not `{}`: `page_bidi` and kfxgen's `or` read the dict for its truth.
    pub set: bool,
}

/// `$157` keys and values, `_bidi_diff`'s (:6927) and an inline element's: (key, value) in
/// [`BIDI_KEYS`] order.
pub(crate) type BidiKeys = Vec<(&'static str, String)>;

impl Bidi {
    /// A `bidi` dict at `at`; None when absent or None. A value kfxgen would write and can't (a
    /// direction `_BIDI_SYMBOLS` doesn't have) is a model error here, read or not.
    pub(crate) fn read(v: Option<&PyValue>, at: &str) -> Result<Option<Bidi>, GenerateError> {
        let pairs = match v {
            None | Some(PyValue::None) => return Ok(None),
            Some(PyValue::Dict(pairs)) => pairs,
            Some(_) => return Err(model_err(format!("{at}: a dict"))),
        };
        let mut bidi = Bidi {
            set: !pairs.is_empty(),
            ..Bidi::default()
        };
        for (k, v) in pairs {
            let key = k.as_str().unwrap_or("");
            if key == "set_here" {
                bidi.set_here = v
                    .as_list()
                    .and_then(|l| l.iter().map(|n| n.as_str().map(str::to_owned)).collect())
                    .ok_or_else(|| model_err(format!("{at}.set_here: names")))?;
                continue;
            }
            bidi.set_value(key, v, at)?;
        }
        Ok(Some(bidi))
    }

    fn set_value(&mut self, key: &str, v: &PyValue, at: &str) -> Result<(), GenerateError> {
        let i = BIDI_KEYS
            .iter()
            .position(|(name, _)| *name == key)
            .ok_or_else(|| model_err(format!("{at}: no key {key:?}")))?;
        let value = match v {
            PyValue::None => return Ok(()),
            PyValue::Str(s) if s.is_empty() => return Ok(()),
            PyValue::Str(s) => s,
            _ => return Err(model_err(format!("{at}.{key}: a string"))),
        };
        if key != "lang" && bidi_symbol(value).is_none() {
            return Err(model_err(format!(
                "{at}.{key}: a direction or unicode-bidi kfxgen knows"
            )));
        }
        self.values[i] = Some(value.clone());
        Ok(())
    }

    /// `make_bidi_flag`'s keys, `((key, value), ...)`.
    fn read_keys(v: &PyValue, at: &str) -> Result<Bidi, GenerateError> {
        let shape = || model_err(format!("{at}: direction keys, (key, value) pairs"));
        let mut bidi = Bidi::default();
        for pair in v.as_list().ok_or_else(shape)? {
            let [k, v] = pair.as_list().ok_or_else(shape)? else {
                return Err(shape());
            };
            bidi.set_value(k.as_str().ok_or_else(shape)?, v, at)?;
        }
        Ok(bidi)
    }

    /// Its keys as written on an inline element's run (:7915): all it sets, `lang` (an
    /// inner element's language alone) under its own.
    fn keys(&self, lang: Option<&str>) -> BidiKeys {
        BIDI_KEYS
            .iter()
            .zip(&self.values)
            .filter_map(|((name, sym), v)| {
                let v = v.as_deref().or(lang.filter(|_| *name == "lang"))?;
                Some((*sym, v.to_owned()))
            })
            .collect()
    }
}

/// `_bidi_diff` (:6927): `own`'s keys, less those the container around already sets the
/// same, unless `own` sets them itself (#605).
pub(crate) fn bidi_diff(own: Option<&Bidi>, around: Option<&Bidi>) -> BidiKeys {
    let Some(own) = own else {
        return BidiKeys::new();
    };
    BIDI_KEYS
        .iter()
        .enumerate()
        .filter_map(|(i, (name, sym))| {
            let v = own.values[i].as_ref()?;
            let here = own.set_here.iter().any(|n| n == name);
            let theirs = around.and_then(|a| a.values[i].as_ref());
            (here || Some(v) != theirs).then(|| (*sym, v.clone()))
        })
        .collect()
}

/// The page's direction and language, for kfxgen's lines with no block of the book's to take
/// them from: its heading, a rebuilt contents page's lines (#714, :6307). The converter's
/// `page_bidi`, else `page_bidi(blocks)` (`inline_style.py:442`): the first block's that is
/// set, of its style, box or table.
pub(crate) fn page_bidi(chapter: &Chapter, blocks: &[Block]) -> Option<Bidi> {
    chapter.page_bidi.clone().filter(|b| b.set).or_else(|| {
        blocks
            .iter()
            .flat_map(|b| {
                let boxed = b.boxed.as_ref().and_then(|x| x.spec.bidi.as_ref());
                let table = b.table.as_ref().and_then(|t| t.bidi.as_ref());
                [b.style.as_ref().and_then(|s| s.bidi.as_ref()), boxed, table]
            })
            .flatten()
            .find(|b| b.set)
            .cloned()
    })
}

/// A `$157` key's value: a language as written, a direction's symbol.
pub(crate) fn bidi_value(key: &str, value: &str) -> Value {
    match bidi_symbol(value).filter(|_| key != "$10") {
        Some(symbol) => sym(symbol),
        None => Value::string(value),
    }
}

/// `make_bidi_flag`'s `("bidi", (serial, depth, keys))`.
pub(crate) fn read_bidi_flag(v: &PyValue, at: &str) -> Result<Flag, GenerateError> {
    let shape = || model_err(format!("{at}: a direction flag, (serial, depth, keys)"));
    let [serial, depth, keys] = v.as_list().ok_or_else(shape)? else {
        return Err(shape());
    };
    let (PyValue::Int(serial), PyValue::Int(depth)) = (serial, depth) else {
        return Err(shape());
    };
    Ok(Flag::Bidi(*serial, *depth, Bidi::read_keys(keys, at)?))
}

/// A run of [`direction_layers`]: where, its flags, an inline element's direction keys, and
/// the flags of each run it is layered on, outermost first.
pub(crate) struct Layer {
    pub start: i64,
    pub len: i64,
    pub flags: Flags,
    pub keys: Option<Bidi>,
    pub under: Vec<Flags>,
}

impl Layer {
    /// Its `$157` direction keys (:7912): its own, over a language alone among its flags,
    /// which leaves them.
    pub(crate) fn bidi_keys(&mut self) -> BidiKeys {
        let lang = self.flags.iter().find_map(|f| match f {
            Flag::Lang(l) if !l.is_empty() => Some(l.clone()),
            _ => None,
        });
        self.flags.retain(|f| !matches!(f, Flag::Lang(_)));
        match &self.keys {
            Some(keys) => keys.keys(lang.as_deref()),
            None => lang.map(|l| vec![("$10", l)]).unwrap_or_default(),
        }
    }
}

fn plain(flags: &Flags) -> Flags {
    flags
        .iter()
        .filter(|f| !matches!(f, Flag::Bidi(..)))
        .cloned()
        .collect()
}

/// For each of `queries`, the `ranges` it lies in, by index, lowest first. Both are
/// (start, end).
fn holders(ranges: &[(i64, i64)], queries: &[(i64, i64)]) -> Vec<Vec<usize>> {
    // The pass below takes a query to end at or after its start; one that doesn't is looked
    // up in every range.
    if queries.iter().any(|(a, b)| b < a) {
        return (queries.iter())
            .map(|&(a, b)| {
                (0..ranges.len())
                    .filter(|&j| ranges[j].0 <= a && b <= ranges[j].1)
                    .collect()
            })
            .collect();
    }
    let mut by_start: Vec<usize> = (0..ranges.len()).collect();
    by_start.sort_by_key(|&j| ranges[j].0);
    let mut by_start = by_start.into_iter().peekable();
    let mut order: Vec<usize> = (0..queries.len()).collect();
    order.sort_by_key(|&q| queries[q].0);
    // The ranges that start at or before the query and end at or after its start, as
    // (end, index).
    let mut open: BTreeSet<(i64, usize)> = BTreeSet::new();
    let mut out = vec![Vec::new(); queries.len()];
    for q in order {
        let (a, b) = queries[q];
        while let Some(j) = by_start.next_if(|&j| ranges[j].0 <= a) {
            open.insert((ranges[j].1, j));
        }
        while open.first().is_some_and(|&(end, _)| end < a) {
            open.pop_first();
        }
        let mut found: Vec<usize> = open.range((b, 0)..).map(|&(_, j)| j).collect();
        found.sort_unstable();
        out[q] = found;
    }
    out
}

/// `_direction_layers` (:402): each inline element with a direction one run over its whole
/// text, and each link (#710, #715), carrying the flags common to all its text no run it is
/// layered on carries; every other run keeps what those don't carry. Outer runs first.
pub(crate) fn direction_layers(spans: &[Span]) -> Vec<Layer> {
    // [start, end, level, keys, flags common to its text]. Levels in quarters: a direction
    // element of depth d at 8d, a link 2 above the elements around it, a flat run 3 above
    // the layers it is in. i128: a depth is the model's.
    struct Range {
        start: i64,
        end: i64,
        level: i128,
        keys: Option<Bidi>,
        flags: Flags,
    }
    let mut dirs: Vec<(i64, Range)> = Vec::new();
    let mut dir_at: HashMap<i64, usize> = HashMap::new();
    let mut links: Vec<Range> = Vec::new();
    for sp in spans {
        let plain = plain(&sp.flags);
        let end = sp.start + sp.len;
        for f in &sp.flags {
            let Flag::Bidi(serial, depth, keys) = f else {
                continue;
            };
            match dir_at.get(serial).map(|&i| &mut dirs[i]) {
                Some((_, r)) => {
                    r.end = r.end.max(end);
                    r.flags = &r.flags & &plain;
                }
                None => {
                    dir_at.insert(*serial, dirs.len());
                    dirs.push((
                        *serial,
                        Range {
                            start: sp.start,
                            end,
                            level: 8 * i128::from(*depth),
                            keys: Some(keys.clone()),
                            flags: plain.clone(),
                        },
                    ));
                }
            }
        }
        // In target order, `Flags`' own (#759, 0bc705d :442); an empty target is no link.
        for f in plain
            .iter()
            .filter(|f| matches!(f, Flag::Link(t) if !t.is_empty()))
        {
            match links.last_mut() {
                // Adjacent runs of one link are one layer.
                Some(last) if last.flags.contains(f) && last.end == sp.start => last.end = end,
                _ => links.push(Range {
                    start: sp.start,
                    end,
                    level: 0,
                    keys: None,
                    flags: Flags::from([f.clone()]),
                }),
            }
        }
    }
    if dirs.is_empty() && links.is_empty() {
        return spans
            .iter()
            .map(|sp| Layer {
                start: sp.start,
                len: sp.len,
                flags: sp.flags.clone(),
                keys: None,
                under: Vec::new(),
            })
            .collect();
    }
    let ends =
        |ranges: &[Range]| -> Vec<(i64, i64)> { ranges.iter().map(|r| (r.start, r.end)).collect() };
    let span_ends: Vec<(i64, i64)> = spans
        .iter()
        .map(|sp| (sp.start, sp.start + sp.len))
        .collect();
    let mut layers: Vec<Range> = dirs.into_iter().map(|(_, r)| r).collect();
    // Each link two levels above the direction elements it is in, with the flags common to
    // the runs in it.
    let around = holders(&ends(&layers), &ends(&links));
    let mut common: Vec<Option<Flags>> = vec![None; links.len()];
    for (sp, links_around) in spans.iter().zip(holders(&ends(&links), &span_ends)) {
        let plain = plain(&sp.flags);
        for i in links_around {
            common[i] = Some(match common[i].take() {
                Some(flags) => &flags & &plain,
                None => plain.clone(),
            });
        }
    }
    for ((mut link, around), common) in links.into_iter().zip(around).zip(common) {
        link.level = around.iter().map(|&j| layers[j].level).max().unwrap_or(0) + 2;
        link.flags = common.unwrap_or_default();
        layers.push(link);
    }
    // The layers each layer is in, itself among them.
    let layer_ends = ends(&layers);
    let around = holders(&layer_ends, &layer_ends);
    // What each layer carries: its flags less those of the layers below it, lowest first.
    let mut by_level: Vec<usize> = (0..layers.len()).collect();
    by_level.sort_by_key(|&i| layers[i].level);
    let mut carried: Vec<Option<Flags>> = vec![None; layers.len()];
    for &i in &by_level {
        let r = &layers[i];
        let mut flags = r.flags.clone();
        for &o in &around[i] {
            if layers[o].level < r.level {
                let c = carried[o].as_ref().expect("a lower layer is carried first");
                flags.retain(|f| !c.contains(f));
            }
        }
        carried[i] = Some(flags);
    }
    let carried: Vec<Flags> = carried.into_iter().map(Option::unwrap_or_default).collect();
    // The flags of `inside`, lowest level first.
    let under = |inside: &[usize]| {
        let mut inside = inside.to_vec();
        inside.sort_by_key(|&i| layers[i].level);
        inside.into_iter().map(|i| carried[i].clone()).collect()
    };
    let mut out: Vec<(Layer, i128)> = Vec::new();
    for (i, r) in layers.iter().enumerate() {
        let inside: Vec<usize> = (around[i].iter().copied())
            .filter(|&j| layers[j].level < r.level)
            .collect();
        out.push((
            Layer {
                start: r.start,
                len: r.end - r.start,
                flags: carried[i].clone(),
                keys: r.keys.clone(),
                under: under(&inside),
            },
            r.level,
        ));
    }
    for (sp, inside) in spans.iter().zip(holders(&layer_ends, &span_ends)) {
        let rest: Flags = plain(&sp.flags)
            .into_iter()
            .filter(|f| !inside.iter().any(|&j| carried[j].contains(f)))
            .collect();
        if !rest.is_empty() {
            let level = inside.iter().map(|&j| layers[j].level).max().unwrap_or(0) + 3;
            out.push((
                Layer {
                    start: sp.start,
                    len: sp.len,
                    flags: rest,
                    keys: None,
                    under: under(&inside),
                },
                level,
            ));
        }
    }
    // Stable, as Python's sort.
    out.sort_by_key(|(l, level)| (l.start, *level));
    out.into_iter().map(|(l, _)| l).collect()
}

/// `RUBY_STYLE_NAME` (:5261): every reading's style.
const RUBY_STYLE_NAME: &str = "s0_rt";

/// `make_ruby_flag`'s `("ruby", (serial, reading))` (#236).
pub(crate) fn read_ruby_flag(v: &PyValue, at: &str) -> Result<Flag, GenerateError> {
    match v.as_list() {
        Some([PyValue::Int(serial), PyValue::Str(reading)]) => {
            Ok(Flag::Ruby(*serial, reading.clone()))
        }
        _ => Err(model_err(format!("{at}: a ruby, (serial, reading)"))),
    }
}

/// `ruby_of`: a run's ruby as (serial, reading).
fn ruby_of(flags: &Flags) -> Option<(i64, &str)> {
    flags.iter().find_map(|f| match f {
        Flag::Ruby(serial, reading) => Some((*serial, reading.as_str())),
        _ => None,
    })
}

/// `without_tcy(without_ruby(...))` (:7889): a run's flags less its ruby, written as an event
/// of its own, and its 縦中横, written as an entry.
pub(crate) fn without_ruby_or_tcy(spans: &[Span]) -> Vec<Span> {
    spans
        .iter()
        .map(|sp| Span {
            flags: sp
                .flags
                .iter()
                .filter(|f| !matches!(f, Flag::Ruby(..) | Flag::Tcy(..)))
                .cloned()
                .collect(),
            ..sp.clone()
        })
        .collect()
}

/// Each ruby's reading is an entry with an id of its own, which takes no position (:6650):
/// ids after the content's, in the order the rubies first appear. Keyed by (chunk, serial).
pub(crate) fn ruby_eids<'a>(
    spans: impl IntoIterator<Item = &'a [Span]>,
    next_eid: &mut i64,
    step: i64,
) -> HashMap<(usize, i64), i64> {
    let mut eids = HashMap::new();
    for (i, spans) in spans.into_iter().enumerate() {
        for sp in spans {
            if let Some((serial, _)) = ruby_of(&sp.flags)
                && let Entry::Vacant(eid) = eids.entry((i, serial))
            {
                eid.insert(*next_eid);
                *next_eid += step;
            }
        }
    }
    eids
}

/// A chunk's rubies (:7847): each one's base, from its first run to its last, and reading,
/// in the order they first appear.
pub(crate) fn rubies(spans: &[Span]) -> Vec<(i64, i64, i64, String)> {
    let mut out: Vec<(i64, i64, i64, String)> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for sp in spans {
        let Some((serial, reading)) = ruby_of(&sp.flags) else {
            continue;
        };
        let end = sp.start + sp.len;
        match at.get(&serial) {
            Some(&i) => out[i].2 = end,
            None => {
                at.insert(serial, out.len());
                out.push((serial, sp.start, end, reading.to_owned()));
            }
        }
    }
    out
}

/// `_ruby_heading_title` (:1237): the base text of the chapter's first block, and its runs,
/// when that block with each reading run in after its base is `title` (#236). A reading
/// may be in parentheses, as `<rp>` gives it; spaces are ignored.
pub(crate) fn ruby_heading_title(
    first: Option<&Block>,
    title: &str,
) -> Option<(String, Vec<Span>)> {
    let first = first.filter(|b| b.is_paragraph())?;
    let mut rubies: Vec<(i64, (i64, &str))> = Vec::new();
    let mut found: HashMap<i64, usize> = HashMap::new();
    for sp in &first.spans {
        if let Some((serial, reading)) = ruby_of(&sp.flags) {
            let at = (sp.start + sp.len, reading);
            match found.get(&serial) {
                Some(&i) => rubies[i].1 = at,
                None => {
                    found.insert(serial, rubies.len());
                    rubies.push((serial, at));
                }
            }
        }
    }
    if rubies.is_empty() {
        return None;
    }
    let mut ends: Vec<(i64, &str)> = rubies.into_iter().map(|(_, at)| at).collect();
    ends.sort_by(|a, b| b.cmp(a));
    let no_space = |s: &str| s.chars().filter(|&c| !py::is_space(c)).collect::<String>();
    let run_in =
        |open: &str, close: &str| no_space(&readings_run_in(&first.text, &ends, open, close));
    let want = no_space(title);
    if ![("", ""), ("(", ")"), ("（", "）")]
        .iter()
        .any(|(open, close)| run_in(open, close) == want)
    {
        return None;
    }
    let base = py::strip(&first.text).to_owned();
    let spans = title_spans(&first.text, &first.spans, py::len(&base));
    Some((base, spans))
}

/// `text` with each reading run in at its end, between `open` and `close`: `ends` from the
/// last to the first, as kfxgen puts them in.
fn readings_run_in(text: &str, ends: &[(i64, &str)], open: &str, close: &str) -> String {
    let n = py::len(text) as i64;
    if ends.iter().any(|&(end, _)| !(0..=n).contains(&end)) {
        return readings_spliced_in(text, ends, open, close);
    }
    // Every end inside the text: each reading stands before the character at its end, and
    // one put in later stands before one put in earlier at the same end.
    let mut out = String::with_capacity(text.len());
    let mut ends = ends.iter().rev().peekable();
    for (i, c) in text.chars().enumerate() {
        while let Some((_, reading)) = ends.next_if(|&&(end, _)| end as usize == i) {
            out.extend([open, reading, close]);
        }
        out.push(c);
    }
    ends.for_each(|(_, reading)| out.extend([open, reading, close]));
    out
}

/// [`readings_run_in`] one reading at a time, for an end outside the text.
fn readings_spliced_in(text: &str, ends: &[(i64, &str)], open: &str, close: &str) -> String {
    let mut text: Vec<char> = text.chars().collect();
    for &(end, reading) in ends {
        // Python's slice: an end past the text is its end, a negative one counts back.
        let n = text.len() as i64;
        let at = if end < 0 {
            (end + n).max(0)
        } else {
            end.min(n)
        } as usize;
        let run: Vec<char> = format!("{open}{reading}{close}").chars().collect();
        text.splice(at..at, run);
    }
    text.into_iter().collect()
}

/// A chunk's rubies put on its runs (:8039): each reading numbered in its chapter's ruby
/// fragment, on the run over exactly its base, or on a run of its own before the first run
/// after its start.
pub(crate) fn place_rubies(
    spans: &[Span],
    runs: &mut Vec<Run>,
    eids: &HashMap<(usize, i64), i64>,
    chunk_idx: usize,
    chapter: usize,
    readings: &mut Vec<(i64, String)>,
) {
    let mut events = Vec::new();
    for (serial, start, end, reading) in rubies(spans) {
        let eid = eids[&(chunk_idx, serial)];
        readings.push((eid, reading));
        let ruby = (format!("ruby{chapter}"), readings.len() as i64);
        events.push((start, end - start, ruby));
    }
    // By start, the longer first: where each event goes among runs in that order.
    let place = |start: i64, len: i64| (start, std::cmp::Reverse(len));
    if runs.is_sorted_by_key(|r| place(r.start, r.len)) {
        place_events_sorted(runs, events, place);
    } else {
        events
            .into_iter()
            .for_each(|(start, len, ruby)| place_event(runs, start, len, ruby));
    }
}

/// A ruby's event on the run it covers, else put in as a run of its own: after a longer run
/// starting with it, such as a link holding it (QA's G53 on #713).
fn place_event(runs: &mut Vec<Run>, start: i64, len: i64, ruby: (String, i64)) {
    let at = runs
        .iter()
        .position(|r| r.start > start || (r.start == start && r.len <= len))
        .unwrap_or(runs.len());
    match runs.get_mut(at) {
        Some(r) if (r.start, r.len) == (start, len) => r.ruby = Some(ruby),
        _ => runs.insert(at, Run::ruby(start, len, ruby)),
    }
}

/// `place_event` for each of `events` in turn, where `runs` are in `place`'s order: one pass
/// over both. Events at one place go on one run, the last of them staying.
fn place_events_sorted<K: Ord + Copy>(
    runs: &mut Vec<Run>,
    mut events: Vec<(i64, i64, (String, i64))>,
    place: impl Fn(i64, i64) -> K,
) {
    // Stable: events at one place stay in their order.
    events.sort_by_key(|&(start, len, _)| place(start, len));
    let mut out: Vec<Run> = Vec::with_capacity(runs.len() + events.len());
    let mut old = std::mem::take(runs).into_iter().peekable();
    for (start, len, ruby) in events {
        let at = place(start, len);
        // An earlier event at this place made the last run, or took it.
        if let Some(last) = out.last_mut().filter(|r| place(r.start, r.len) == at) {
            last.ruby = Some(ruby);
            continue;
        }
        while let Some(r) = old.next_if(|r| place(r.start, r.len) < at) {
            out.push(r);
        }
        match old.next_if(|r| place(r.start, r.len) == at) {
            Some(mut r) => {
                r.ruby = Some(ruby);
                out.push(r);
            }
            None => out.push(Run::ruby(start, len, ruby)),
        }
    }
    out.extend(old);
    *runs = out;
}

/// The readings' style and each chapter's ruby fragment, built after the pruning, which looks
/// only in the storylines (:8126). `readings`: each chapter's (id, reading). Returns the
/// fragments' names.
pub(crate) fn build_ruby_fragments(
    g: &mut Generator,
    readings: &[(usize, Vec<(i64, String)>)],
    extra_style_names: &mut Vec<String>,
) -> Vec<String> {
    if !readings.is_empty() {
        let f = build_ruby_style_157(g);
        g.push(f);
        extra_style_names.push(RUBY_STYLE_NAME.to_owned());
    }
    let mut names = Vec::new();
    for (ch_idx, readings) in readings {
        let name = format!("ruby{ch_idx}");
        let f = build_fragment_756(g, &name, readings);
        g.push(f);
        names.push(name);
    }
    names
}

/// `build_ruby_style_157` (:2328): half the base's size at a normal line height.
fn build_ruby_style_157(g: &mut Generator) -> Fragment {
    g.local(RUBY_STYLE_NAME);
    let value = st([
        ("$16", measure(py::decimal("0.5"), "$308")),
        ("$42", sym("$383")),
        ("$173", sym(RUBY_STYLE_NAME)),
    ]);
    Fragment::new("$157", Some(RUBY_STYLE_NAME), value)
}

/// `build_fragment_756` (:2342): a chapter's ruby fragment, one text entry per reading,
/// numbered from 1 as the bases' events name them.
fn build_fragment_756(g: &mut Generator, name: &str, readings: &[(i64, String)]) -> Fragment {
    g.local(name);
    let entries = readings
        .iter()
        .zip(1..)
        .map(|((eid, reading), number)| {
            st([
                ("$155", Value::Int(*eid)),
                ("$157", sym(RUBY_STYLE_NAME)),
                ("$159", sym("$269")),
                ("$758", Value::Int(number)),
                ("$145", Value::string(reading)),
            ])
        })
        .collect();
    Fragment::new(
        "$756",
        Some(name),
        st([("$757", sym(name)), ("$146", Value::List(entries))]),
    )
}

/// `_WRITING_MODE_SYMBOLS` (:939).
pub(crate) fn writing_mode_symbol(mode: Option<&str>) -> Option<&'static str> {
    match mode? {
        "horizontal-tb" => Some("$557"),
        "vertical-rl" => Some("$559"),
        _ => None,
    }
}

/// `_VERTICAL_KEYS` (:962): in vertical-rl text, the key each of kfxgen's margin and padding
/// keys moves to (#247).
const VERTICAL_KEYS: [(&str, &str); 8] = [
    ("$47", "$50"),
    ("$49", "$48"),
    ("$48", "$47"),
    ("$50", "$49"),
    ("$52", "$55"),
    ("$54", "$53"),
    ("$53", "$52"),
    ("$55", "$54"),
];

/// `_drop_default_left` (0bc705d :992): kfxgen's own 0.5% left margin taken off a `$157`:
/// Previewer writes none in a vertical book, on a horizontal block in it either (P247h, #247).
pub(crate) fn drop_default_left(v: &mut Struct) {
    // Python compares the measure as an OrderedDict, its magnitude by value.
    let default_left = v.get("$48").and_then(Value::as_struct).is_some_and(|m| {
        matches!(&m.0[..], [(k, Value::Decimal(d)), (u, unit)]
        if k.0 == "$307" && u.0 == "$306" && *unit == sym("$314")
            && d.coefficient != 0
            && {
                let (mut c, mut e) = (d.coefficient, d.exponent);
                while c % 10 == 0 {
                    c /= 10;
                    e += 1;
                }
                (c, e) == (5, -1)
            })
    });
    if default_left {
        v.remove("$48");
    }
}

/// `_rotate_margins` (:989): a `$157`'s margins and paddings moved for vertical-rl text, as
/// Kindle Previewer 4.0.3 writes them, kfxgen's own 0.5% left margin dropped.
pub(crate) fn rotate_margins(v: &mut Struct) {
    drop_default_left(v);
    let moved: Vec<(&str, Value)> = VERTICAL_KEYS
        .iter()
        .filter_map(|(old, new)| Some((*new, v.remove(old)?)))
        .collect();
    for (key, value) in moved {
        v.insert(key, value);
    }
}

/// `make_tcy_flag`'s `("tcy", (serial, text))` (#247).
pub(crate) fn read_tcy_flag(v: &PyValue, at: &str) -> Result<Flag, GenerateError> {
    match v.as_list() {
        Some([PyValue::Int(serial), PyValue::Str(text)]) => Ok(Flag::Tcy(*serial, text.clone())),
        _ => Err(model_err(format!("{at}: a tate-chu-yoko, (serial, text)"))),
    }
}

/// 縦中横 in a paragraph's words (#247): where its placeholder is, its id and its text.
pub(crate) struct Tcy {
    pub at: i64,
    pub eid: i64,
    pub text: String,
}

/// Each chunk's 縦中横 (:6675, :7870): an id of its own per (chunk, serial) after the rubies'
/// ids, an item per run carrying one.
pub(crate) fn number_tcy(chunks: &mut [Chunk], next_eid: &mut i64, step: i64) {
    for chunk in chunks {
        let mut eids: Vec<(i64, i64)> = Vec::new();
        for sp in &chunk.spans {
            let Some((serial, text)) = sp.flags.iter().find_map(|f| match f {
                Flag::Tcy(serial, text) => Some((*serial, text)),
                _ => None,
            }) else {
                continue;
            };
            let eid = match eids.iter().find(|(s, _)| *s == serial) {
                Some(&(_, eid)) => eid,
                None => {
                    eids.push((serial, *next_eid));
                    *next_eid += step;
                    *next_eid - step
                }
            };
            chunk.tcys.push(Tcy {
                at: sp.start,
                eid,
                text: text.clone(),
            });
        }
    }
}

/// `TCY_STYLE_NAME` (:2398).
const TCY_STYLE_NAME: &str = "tcy0";

/// The equations' texts and 縦中横's, in `content_math` fragments (:2380); whether a 縦中横
/// was written.
#[derive(Default)]
pub(crate) struct TcyNotes {
    groups: Vec<(String, Vec<String>)>,
    wrote: bool,
}

/// `_math_note` (:2380): a content reference to `text`. A fragment takes strings until it
/// holds `MAX_CONTENT_FRAGMENT_SIZE` bytes, which leaves out its last string.
fn math_note(g: &mut Generator, text: &str) -> Value {
    let full = g.tcy.groups.last().is_none_or(|(_, strings)| {
        strings.iter().map(String::len).sum::<usize>() >= MAX_CONTENT_FRAGMENT_SIZE
    });
    if full {
        let name = format!("content_math{}", g.tcy.groups.len() + 1);
        g.local(&name);
        g.tcy.groups.push((name, Vec::new()));
    }
    let (name, strings) = g.tcy.groups.last_mut().expect("a group was just made");
    strings.push(text.to_owned());
    st([
        ("$4", sym(name)),
        ("$403", Value::Int(strings.len() as i64 - 1)),
    ])
}

/// `_tcy_entry` (:2400): an inline container of the paragraph holding the characters, as
/// Kindle Previewer 4.0.3 writes each `text-combine-upright: all` element.
pub(crate) fn tcy_entry(g: &mut Generator, tcy: &Tcy) -> Value {
    g.tcy.wrote = true;
    g.local(TCY_STYLE_NAME);
    let note = math_note(g, &tcy.text);
    st([
        ("$155", Value::Int(tcy.eid)),
        ("$601", sym("$283")),
        ("$157", sym(TCY_STYLE_NAME)),
        ("$159", sym("$269")),
        ("$145", note),
    ])
}

/// 縦中横's style (:8138), once one is written: horizontal in the vertical line, combined
/// upright, at the width of its characters. Then the texts' `$145` fragments (:8176), named
/// in the order they were made.
pub(crate) fn build_tcy_fragments(
    g: &mut Generator,
    extra_style_names: &mut Vec<String>,
    content_names: &mut Vec<String>,
) {
    if g.tcy.wrote {
        let value = st([
            ("$560", sym("$557")),
            ("$707", sym("$573")),
            ("$708", sym("$383")),
            ("$173", sym(TCY_STYLE_NAME)),
        ]);
        g.push(Fragment::new("$157", Some(TCY_STYLE_NAME), value));
        extra_style_names.push(TCY_STYLE_NAME.to_owned());
    }
    for (name, strings) in std::mem::take(&mut g.tcy.groups) {
        let f = build_fragment_145(g, strings, &name);
        g.push(f);
        content_names.push(name);
    }
}

#[cfg(test)]
mod tests {
    use super::{Run, place_event, place_events_sorted};

    /// Readings run into a text in one pass are those spliced in one at a time.
    #[test]
    fn readings_run_in_one_pass_are_those_spliced_in() {
        let mut seed = 3u64;
        let mut next = |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) % n) as i64
        };
        let readings = ["か", "き", "kana", ""];
        for round in 0..2000 {
            let text: String = "漢字abé".chars().take(next(6) as usize).collect();
            // In every fifth round, ends before and past the text.
            let wide = if round % 5 == 0 { 4 } else { 0 };
            let mut ends: Vec<(i64, &str)> = (0..next(6))
                .map(|_| {
                    (
                        next(text.chars().count() as u64 + 1 + 2 * wide) - wide as i64,
                        readings[next(4) as usize],
                    )
                })
                .collect();
            ends.sort_by(|a, b| b.cmp(a));
            assert_eq!(
                super::readings_run_in(&text, &ends, "(", ")"),
                super::readings_spliced_in(&text, &ends, "(", ")"),
                "round {round}: {text:?} {ends:?}"
            );
        }
    }

    /// The ranges a query lies in are those a look through every range finds.
    #[test]
    fn holders_are_the_ranges_a_query_lies_in() {
        let mut seed = 11u64;
        let mut next = |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) % n) as i64
        };
        for round in 0..2000 {
            // Ranges that end before they start too; queries that do, in every fifth round.
            let ranges: Vec<(i64, i64)> = (0..next(9)).map(|_| (next(8), next(8))).collect();
            let queries: Vec<(i64, i64)> = (0..next(9))
                .map(|_| {
                    let a = next(8);
                    (a, if round % 5 == 0 { next(8) } else { a + next(4) })
                })
                .collect();
            let want: Vec<Vec<usize>> = (queries.iter())
                .map(|&(a, b)| {
                    (0..ranges.len())
                        .filter(|&j| ranges[j].0 <= a && b <= ranges[j].1)
                        .collect()
                })
                .collect();
            assert_eq!(super::holders(&ranges, &queries), want, "round {round}");
        }
    }

    /// Placing every ruby event in one pass gives the runs that placing them one by one does.
    #[test]
    fn events_placed_in_one_pass_are_those_placed_one_by_one() {
        let mut seed = 7u64;
        let mut next = |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) % n) as i64
        };
        let place = |start: i64, len: i64| (start, std::cmp::Reverse(len));
        for round in 0..2000 {
            let mut runs: Vec<Run> = (0..next(8))
                .map(|i| Run::ruby(next(5), next(4), ("run".into(), i)))
                .collect();
            runs.sort_by_key(|r| place(r.start, r.len));
            let events: Vec<_> = (0..next(8))
                .map(|i| (next(5), next(4), ("ruby".to_owned(), i)))
                .collect();
            let mut one_by_one: Vec<Run> = runs
                .iter()
                .map(|r| Run::ruby(r.start, r.len, r.ruby.clone().unwrap()))
                .collect();
            for (start, len, ruby) in events.clone() {
                place_event(&mut one_by_one, start, len, ruby);
            }
            place_events_sorted(&mut runs, events, place);
            let seen = |runs: &[Run]| -> Vec<_> {
                runs.iter()
                    .map(|r| (r.start, r.len, r.ruby.clone()))
                    .collect()
            };
            assert_eq!(seen(&runs), seen(&one_by_one), "round {round}");
        }
    }
}
