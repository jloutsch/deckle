// SPDX-License-Identifier: GPL-3.0-only
//! The pure helpers of kfxgen's `inline_style.py`: a paragraph's (text, flags) segments made
//! into one string with spans and anchor offsets, the run flags and their makers, and the CSS
//! values runs read (lengths, vertical-align, colours, typography).
//!
//! Every string comes from the book. Python's rules are kept: offsets count code points, the
//! white space HTML collapses is ASCII, `\s` and `strip()` are `isspace`, `\d` and `int()` read
//! any decimal digit, `round()` goes half to even. Where kfxgen raises on an input, the port
//! returns [`Raised`].
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;

use crate::generate::py;
use crate::generate::typography::luminance;
use crate::pyvalue::PyValue;

pub const FLAG_ITALIC: &str = "italic";
pub const FLAG_BOLD: &str = "bold";
pub const FLAG_SUPER: &str = "super";
pub const FLAG_SUB: &str = "sub";
pub const FLAG_TEXT_TOP: &str = "text-top";
pub const FLAG_TEXT_BOTTOM: &str = "text-bottom";
/// The vertical shifts, superscript first: it wins on one run.
pub const SHIFT_FLAGS: [&str; 4] = [FLAG_SUPER, FLAG_SUB, FLAG_TEXT_TOP, FLAG_TEXT_BOTTOM];
/// A shift back to the baseline, written only inside another (#638).
pub const BASELINE_SHIFT: &str = "baseline";
pub const FLAG_UNDERLINE: &str = "underline";
pub const FLAG_STRIKE: &str = "line-through";
pub const FLAG_NOT_BOLD: &str = "not-bold";
pub const FLAG_NOT_ITALIC: &str = "not-italic";
/// White-space modes: `normalize_runs_with_anchors` reads them and takes them off (#202).
pub const FLAG_PRE: &str = "ws-pre";
pub const FLAG_PRE_LINE: &str = "ws-pre-line";
const WS_FLAGS: [&str; 2] = [FLAG_PRE, FLAG_PRE_LINE];
/// A forced line break (`<br>`) in a segment stream (#202).
pub const HARD_BREAK: char = '\u{2028}';
/// A segment of its own, ("", {LINE_START}): what follows starts a new line (#233).
pub const LINE_START: &str = "line-start";
pub const NBSP: char = '\u{a0}';
/// The white space HTML collapses: ASCII only (#244).
const COLLAPSIBLE: [char; 5] = [' ', '\t', '\n', '\r', '\u{c}'];
const TAB_STOP: usize = 8;
/// Marks where an anchor is declared in a segment stream (#79).
pub const ANCHOR_MARK: &str = "anchor-mark";
pub const LINK_FLAG: &str = "link";
/// A link target leaving the book: the address after this prefix (#474).
pub const EXTERNAL_LINK_PREFIX: &str = "url:";
pub const NOTEREF_FLAG: &str = "noteref";
pub const RUBY_FLAG: &str = "ruby";
pub const TCY_FLAG: &str = "tcy";
pub const TURN_FLAG: &str = "turn";
pub const BIDI_FLAG: &str = "bidi";
pub const LANG_FLAG: &str = "lang";
pub const MONO_FLAG: &str = "mono";
pub const SIZE_FLAG: &str = "size";
pub const SMALL_CAPS_FLAG: &str = "small-caps";
pub const TRANSFORM_FLAG: &str = "transform";
pub const SPACING_FLAG: &str = "spacing";
pub const COLOUR_FLAG: &str = "colour";
pub const ORIENTATION_FLAG: &str = "orientation";
pub const EMPHASIS_MARKS_FLAG: &str = "emphasis-marks";
pub const TYPOGRAPHY_FLAGS: [&str; 6] = [
    SMALL_CAPS_FLAG,
    TRANSFORM_FLAG,
    SPACING_FLAG,
    COLOUR_FLAG,
    ORIENTATION_FLAG,
    EMPHASIS_MARKS_FLAG,
];
/// Their block_style keys, in the same order.
pub const TYPOGRAPHY_KEYS: [&str; 6] = [
    "small_caps",
    "text_transform",
    "letter_spacing",
    "colour",
    "text_orientation",
    "text_emphasis",
];
pub const SPACING_NORMAL: (&str, &str) = ("0", "$308");
pub const LINK_COLOUR_FLAG: &str = "link-colour";
pub const BLACK: i64 = 0xFF00_0000;

/// An exception kfxgen's helper raises on this input instead of returning. Where it lands
/// depends on the caller, and the caller in the port must do as kfxgen's does: raised in the
/// style resolver's `resolve` (a box's or cell's fill), that element resolves to None; raised
/// anywhere else in a spine item's extraction, kfxgen logs "Spine item parse failed" and
/// leaves the item out (`extract_chapters_from_oeb`). Python's exception, and which helper
/// raised it on what kind of input (never the input itself, which can be any size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raised {
    pub exception: &'static str,
    pub note: &'static str,
}

const UNIT_I: Raised = Raised {
    exception: "KeyError",
    note: "parse_css_length: a unit whose i is U+0130 or U+0131, matched by re.I as \"in\"",
};
const COLOUR_NUMBER: Raised = Raised {
    exception: "ValueError",
    note: "_rgb_alpha: float() of a colour's number of two points or no digit",
};
const PERCENT_INF: Raised = Raised {
    exception: "ValueError",
    note: "_rgb_alpha: int() of NaN, from a percentage channel too big for a float",
};
const CHANNEL_DIGITS: Raised = Raised {
    exception: "ValueError",
    note: "_rgb_alpha: int() of a channel of more than 4300 digits",
};

/// A member of a run's flag set: a string, or a (kind, value) pair.
#[derive(Clone, Debug)]
pub enum Flag {
    Str(String),
    Pair(String, PyValue),
}

impl Flag {
    pub fn to_py(&self) -> PyValue {
        match self {
            Flag::Str(s) => PyValue::Str(s.clone()),
            Flag::Pair(kind, value) => {
                PyValue::Tuple(vec![PyValue::Str(kind.clone()), value.clone()])
            }
        }
    }
}

/// A total order on values, so a flag set has one form: Python compares its frozensets by
/// equality alone. Floats compare by bits (`-0.0` is not `0.0`, a NaN is itself), as
/// `model_diff` does; no flag kfxgen makes holds either.
fn cmp_py(a: &PyValue, b: &PyValue) -> Ordering {
    use PyValue as P;
    let rank = |v: &PyValue| match v {
        P::None => 0,
        P::Bool(_) => 1,
        P::Int(_) => 2,
        P::Float(_) => 3,
        P::Str(_) => 4,
        P::Bytes(_) => 5,
        P::Tuple(_) => 6,
        P::List(_) => 7,
        P::Set(_) => 8,
        P::Dict(_) => 9,
        P::Obj { .. } => 10,
    };
    let seq = |x: &[PyValue], y: &[PyValue]| {
        x.iter()
            .zip(y)
            .map(|(p, q)| cmp_py(p, q))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| x.len().cmp(&y.len()))
    };
    match (a, b) {
        (P::Bool(x), P::Bool(y)) => x.cmp(y),
        (P::Int(x), P::Int(y)) => x.cmp(y),
        (P::Float(x), P::Float(y)) => x.total_cmp(y),
        (P::Str(x), P::Str(y)) => x.cmp(y),
        (P::Bytes(x), P::Bytes(y)) => x[..].cmp(&y[..]),
        (P::Tuple(x), P::Tuple(y)) | (P::List(x), P::List(y)) | (P::Set(x), P::Set(y)) => seq(x, y),
        (P::Dict(x), P::Dict(y)) => x
            .iter()
            .zip(y)
            .map(|((k, v), (l, w))| cmp_py(k, l).then_with(|| cmp_py(v, w)))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| x.len().cmp(&y.len())),
        (
            P::Obj {
                class: ca,
                state: sa,
            },
            P::Obj {
                class: cb,
                state: sb,
            },
        ) => ca.cmp(cb).then_with(|| cmp_py(sa, sb)),
        _ => rank(a).cmp(&rank(b)),
    }
}

impl Ord for Flag {
    fn cmp(&self, other: &Flag) -> Ordering {
        match (self, other) {
            (Flag::Str(a), Flag::Str(b)) => a.cmp(b),
            (Flag::Str(_), Flag::Pair(..)) => Ordering::Less,
            (Flag::Pair(..), Flag::Str(_)) => Ordering::Greater,
            (Flag::Pair(ka, va), Flag::Pair(kb, vb)) => ka.cmp(kb).then_with(|| cmp_py(va, vb)),
        }
    }
}

impl PartialOrd for Flag {
    fn partial_cmp(&self, other: &Flag) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Flag {
    fn eq(&self, other: &Flag) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Flag {}

/// A run's flags, the frozenset kfxgen carries: kept sorted, each flag once, so two sets
/// are equal when their vectors are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flags(Vec<Flag>);

impl FromIterator<Flag> for Flags {
    fn from_iter<I: IntoIterator<Item = Flag>>(iter: I) -> Flags {
        let mut v: Vec<Flag> = iter.into_iter().collect();
        v.sort();
        v.dedup();
        Flags(v)
    }
}

impl Flags {
    pub fn insert(&mut self, flag: Flag) {
        if let Err(i) = self.0.binary_search(&flag) {
            self.0.insert(i, flag);
        }
    }

    /// `name in flags` for a string flag.
    pub fn has(&self, name: &str) -> bool {
        self.0
            .binary_search_by(|f| match f {
                Flag::Str(s) => s.as_str().cmp(name),
                Flag::Pair(..) => Ordering::Greater,
            })
            .is_ok()
    }

    /// `flags - {names}` for string flags.
    pub fn without(&self, names: &[&str]) -> Flags {
        Flags(
            self.0
                .iter()
                .filter(|f| !matches!(f, Flag::Str(s) if names.contains(&s.as_str())))
                .cloned()
                .collect(),
        )
    }

    /// `flag_value(flags, kind)`: the value of a (kind, value) flag. Of two of one kind,
    /// kfxgen's is whichever its set's hash order gives first; the port's the smaller.
    pub fn value(&self, kind: &str) -> Option<&PyValue> {
        self.0.iter().find_map(|f| match f {
            Flag::Pair(k, v) if k == kind => Some(v),
            _ => None,
        })
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Flag> {
        self.0.iter()
    }

    /// `flag in flags`.
    pub fn contains(&self, flag: &Flag) -> bool {
        self.0.binary_search(flag).is_ok()
    }

    /// `{f for f in flags if keep(f)}`.
    pub fn retain(&mut self, keep: impl FnMut(&Flag) -> bool) {
        self.0.retain(keep);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The frozenset as the model holds it.
    pub fn to_py(&self) -> PyValue {
        PyValue::Set(self.0.iter().map(Flag::to_py).collect())
    }
}

/// One (text, flags) piece of a paragraph, as the inline walk gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub flags: Flags,
}

/// A run of characters sharing one non-empty flag set: (start, length, flags), in code
/// points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub len: usize,
    pub flags: Flags,
}

/// A count as Python's int; no string has more than `i64::MAX` code points.
fn int(n: usize) -> PyValue {
    PyValue::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

impl Span {
    pub fn to_py(&self) -> PyValue {
        PyValue::Tuple(vec![int(self.start), int(self.len), self.flags.to_py()])
    }
}

fn pair(kind: &str, value: PyValue) -> Flag {
    Flag::Pair(kind.to_owned(), value)
}

fn text(s: &str) -> PyValue {
    PyValue::Str(s.to_owned())
}

/// `nested_shift`: (shift, depth, keeps_size) of a shift inside another, "sub@2", with "="
/// after the depth when the element keeps its parent's size (#536, #638).
pub fn nested_shift(flag: &Flag) -> Option<(&str, i64, bool)> {
    let Flag::Str(flag) = flag else {
        return None;
    };
    let (shift, depth) = flag.split_once('@')?;
    let keeps = depth.ends_with('=');
    let depth = depth.trim_end_matches('=');
    let shifts = SHIFT_FLAGS.contains(&shift) || shift == BASELINE_SHIFT;
    if shifts && !depth.is_empty() && depth.chars().all(py::is_digit) {
        // `int(depth)` raises on a digit that is no decimal one ("²"); kfxgen writes the
        // depth itself, in ASCII, so that is never reached, and the port says no shift.
        return Some((shift, py::py_int(depth)?, keeps));
    }
    None
}

/// `make_anchor_mark`: the zero-length segment marking where `anchor_id` is declared.
pub fn make_anchor_mark(anchor_id: &str) -> Segment {
    Segment {
        text: String::new(),
        flags: Flags(vec![pair(ANCHOR_MARK, text(anchor_id))]),
    }
}

/// `anchor_mark_id`: the id an anchor-mark segment carries. kfxgen takes any value but None
/// as an id; it only ever makes string ones (`make_anchor_mark`), the only kind read here.
fn anchor_mark_id(flags: &Flags) -> Option<&str> {
    flags.iter().find_map(|f| match f {
        Flag::Pair(k, PyValue::Str(id)) if k == ANCHOR_MARK => Some(id.as_str()),
        _ => None,
    })
}

pub fn make_link_flag(target: &str) -> Flag {
    pair(LINK_FLAG, text(target))
}

/// ("ruby", (serial, reading)); the serial counts rubies across the book (#236).
pub fn make_ruby_flag(serial: i64, reading: &str) -> Flag {
    pair(
        RUBY_FLAG,
        PyValue::Tuple(vec![PyValue::Int(serial), text(reading)]),
    )
}

/// ("tcy", (serial, text)) (#247).
pub fn make_tcy_flag(serial: i64, tcy: &str) -> Flag {
    pair(
        TCY_FLAG,
        PyValue::Tuple(vec![PyValue::Int(serial), text(tcy)]),
    )
}

/// ("turn", (serial, mode, text, spans)) (#247).
pub fn make_turn_flag(serial: i64, mode: &str, turned: &str, spans: &[Span]) -> Flag {
    pair(
        TURN_FLAG,
        PyValue::Tuple(vec![
            PyValue::Int(serial),
            text(mode),
            text(turned),
            PyValue::Tuple(spans.iter().map(Span::to_py).collect()),
        ]),
    )
}

/// ("bidi", (serial, depth, ((key, value), ...))), the keys sorted (#710).
pub fn make_bidi_flag(serial: i64, depth: i64, keys: &[(&str, &str)]) -> Flag {
    let mut keys = keys.to_vec();
    keys.sort();
    pair(
        BIDI_FLAG,
        PyValue::Tuple(vec![
            PyValue::Int(serial),
            PyValue::Int(depth),
            PyValue::Tuple(
                keys.iter()
                    .map(|(k, v)| PyValue::Tuple(vec![text(k), text(v)]))
                    .collect(),
            ),
        ]),
    )
}

/// ("mono", families) (#242).
pub fn make_mono_flag(families: &[String]) -> Flag {
    pair(
        MONO_FLAG,
        PyValue::Tuple(families.iter().map(|f| text(f)).collect()),
    )
}

/// ("size", size), in CSS px as the resolver gives it, a float (#241).
pub fn make_size_flag(size: PyValue) -> Flag {
    pair(SIZE_FLAG, size)
}

/// The characters `normalize_runs_with_anchors` has kept so far.
struct Out {
    chars: Vec<char>,
    /// Per character, its flags: an index into the caller's sets, whose 0 is the empty set.
    flags_of: Vec<usize>,
    /// Indexes of spaces kept in preformatted text, ascending.
    pre_spaces: Vec<usize>,
    /// The last "\n" in `chars`: only `brk` adds one, and nothing takes one away before the
    /// end, so it replaces kfxgen's walk back to the line start for each tab.
    last_newline: Option<usize>,
}

impl Out {
    fn push(&mut self, c: char, set: usize) {
        self.chars.push(c);
        self.flags_of.push(set);
    }

    /// `_drop_last`: a dropped kept space loses its mark too.
    fn drop_last(&mut self) {
        self.chars.pop();
        self.flags_of.pop();
        while self
            .pre_spaces
            .last()
            .is_some_and(|&i| i >= self.chars.len())
        {
            self.pre_spaces.pop();
        }
    }

    /// `_break`: spaces before a line break are dropped, and a break before any text.
    fn brk(&mut self, set: usize) {
        while self.chars.last() == Some(&' ') {
            self.drop_last();
        }
        if !self.chars.is_empty() {
            self.last_newline = Some(self.chars.len());
            self.push('\n', set);
        }
    }
}

/// `normalize_runs_with_anchors`: collapse white space across the segments and return
/// (text, spans, anchors). Spans are maximal runs of one non-empty flag set; anchors are
/// (id, offset) in first-declared order, each at its first declaration, clamped to the
/// text's end. Linear in the text and the flags: each character is looked at a bounded
/// number of times, and flag sets are compared only where the segment changes. (kfxgen's
/// own walks back to the line start for every tab of preformatted text.)
pub fn normalize_runs_with_anchors(
    segments: &[Segment],
) -> (String, Vec<Span>, Vec<(String, usize)>) {
    let mut out = Out {
        chars: Vec::new(),
        flags_of: Vec::new(),
        pre_spaces: Vec::new(),
        last_newline: None,
    };
    let mut sets: Vec<Cow<'_, Flags>> = vec![Cow::Owned(Flags::default())];
    let mut anchors: Vec<(String, usize)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut prev_space = true;
    let mut line_started = false;
    for seg in segments {
        if let Some(id) = anchor_mark_id(&seg.flags) {
            if seen.insert(id) {
                anchors.push((id.to_owned(), out.chars.len()));
            }
            continue;
        }
        if seg.flags.has(LINE_START) {
            if out.chars.last().is_some_and(|&c| c != '\n') {
                out.brk(0);
            }
            prev_space = true;
            line_started = true;
            continue;
        }
        let pre = seg.flags.has(FLAG_PRE);
        let pre_line = seg.flags.has(FLAG_PRE_LINE);
        let set = sets.len();
        sets.push(if pre || pre_line {
            Cow::Owned(seg.flags.without(&WS_FLAGS))
        } else {
            Cow::Borrowed(&seg.flags)
        });
        for ch in seg.text.chars() {
            if ch == HARD_BREAK || ((pre || pre_line) && ch == '\n') {
                if !line_started {
                    out.brk(set);
                }
                line_started = false;
                prev_space = true;
            } else if pre && (ch == ' ' || ch == '\t') {
                let width = if ch == '\t' {
                    let column = out.chars.len() - out.last_newline.map_or(0, |i| i + 1);
                    TAB_STOP - column % TAB_STOP
                } else {
                    1
                };
                for _ in 0..width {
                    out.pre_spaces.push(out.chars.len());
                    out.push(' ', set);
                }
                prev_space = false;
            } else if pre && ch == '\r' {
                continue;
            } else if COLLAPSIBLE.contains(&ch) {
                if !prev_space {
                    // A collapsed space carries its own segment's flags.
                    out.push(' ', set);
                    prev_space = true;
                }
            } else {
                out.push(ch, set);
                prev_space = false;
                line_started = false;
            }
        }
    }
    while out.chars.last().is_some_and(|&c| c == ' ' || c == '\n') {
        out.drop_last();
    }
    let Out {
        mut chars,
        flags_of,
        pre_spaces,
        ..
    } = out;

    // A kept space starting a line or beside another kept one becomes a non-breaking
    // space, in order: one just changed counts as the NBSP before the next (#202).
    // `pre_spaces` ascends, so its neighbours say whether i - 1 and i + 1 are kept.
    for (k, &i) in pre_spaces.iter().enumerate() {
        if i >= chars.len() {
            continue;
        }
        let at_line_start = i == 0 || chars[i - 1] == '\n' || chars[i - 1] == NBSP;
        let beside_space = (k > 0 && i > 0 && pre_spaces[k - 1] == i - 1)
            || pre_spaces.get(k + 1) == Some(&(i + 1));
        if at_line_start || beside_space {
            chars[i] = NBSP;
        }
    }

    let mut spans = Vec::new();
    let n = flags_of.len();
    let mut i = 0;
    while i < n {
        let f = flags_of[i];
        if sets[f].is_empty() {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        // Equal to the character before is equal to `f`; sets are compared only where the
        // index changes.
        while j < n && (flags_of[j] == flags_of[j - 1] || sets[flags_of[j]] == sets[f]) {
            j += 1;
        }
        spans.push(Span {
            start: i,
            len: j - i,
            flags: sets[f].clone().into_owned(),
        });
        i = j;
    }
    let limit = chars.len();
    for (_, at) in &mut anchors {
        *at = (*at).min(limit);
    }
    (chars.into_iter().collect(), spans, anchors)
}

/// CSS length unit to KFX unit symbol.
fn kfx_unit(unit: &str) -> Option<&'static str> {
    Some(match unit {
        "em" => "$308",
        "rem" => "$505",
        "%" => "$314",
        "pt" => "$318",
        "px" => "$319",
        "mm" => "$316",
        _ => return None,
    })
}

/// The length of `unit` at the start of `s`, matched as `re.I` does: ASCII letters either
/// case, and the "i" also by U+0130 and U+0131.
fn unit_prefix(s: &str, unit: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    for u in unit.chars() {
        let (_, c) = chars.next()?;
        let same = c == u
            || c == u.to_ascii_uppercase()
            || (u == 'i' && (c == '\u{130}' || c == '\u{131}'));
        if !same {
            return None;
        }
    }
    Some(chars.next().map_or(s.len(), |(i, _)| i))
}

/// `^\s*([+-]?[0-9]*\.?[0-9]+)` + (`\s*` when `spaced`) + `(units)\s*$`, `re.I`, matched
/// from the start: (number, unit as written). The number is all the digits and points
/// after the sign: the unit cannot start with either, so no other split matches.
fn number_unit<'a>(value: &'a str, spaced: bool, units: &[&str]) -> Option<(&'a str, &'a str)> {
    let s = value.trim_start_matches(py::is_space);
    let sign = usize::from(s.starts_with(['+', '-']));
    let body = &s[sign..];
    let end = sign
        + body
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(body.len());
    let digits = &s[sign..end];
    if digits.is_empty() || digits.matches('.').count() > 1 || digits.ends_with('.') {
        return None;
    }
    let mut rest = &s[end..];
    if spaced {
        rest = rest.trim_start_matches(py::is_space);
    }
    units.iter().find_map(|u| {
        let len = unit_prefix(rest, u)?;
        rest[len..]
            .chars()
            .all(py::is_space)
            .then(|| (&s[..end], &rest[..len]))
    })
}

/// `parse_css_length`: a CSS length as (magnitude text, KFX unit), or None for none, a unit
/// it doesn't read, zero, or a negative magnitude unless `negative` (#465). in, cm and ex
/// become pt, mm and em (#486).
pub fn parse_css_length(
    value: &str,
    negative: bool,
) -> Result<Option<(String, &'static str)>, Raised> {
    const UNITS: [&str; 9] = ["em", "rem", "%", "pt", "px", "mm", "in", "cm", "ex"];
    if value.is_empty() {
        return Ok(None);
    }
    let Some((mag, unit)) = number_unit(value, false, &UNITS) else {
        return Ok(None);
    };
    // Python's .lower(): "İn" becomes "i̇n" and "ın" stays, neither a unit kfxgen knows.
    let unit = unit.to_lowercase();
    let Ok(x) = mag.parse::<f64>() else {
        return Ok(None);
    };
    if x == 0.0 || (x < 0.0 && !negative) {
        return Ok(None);
    }
    let factor = match unit.as_str() {
        "in" => Some((72.0, "pt")),
        "cm" => Some((10.0, "mm")),
        "ex" => Some((0.44, "em")),
        _ => None,
    };
    let (mut mag, unit) = match factor {
        // Both round the exact value half to even; inf is "inf", a negative one rounded to
        // nothing keeps its sign ("-0.0000").
        Some((factor, unit)) => (format!("{:.4}", x * factor), unit),
        None => (mag.to_owned(), unit.as_str()),
    };
    if mag.contains('.') {
        mag = mag.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    match kfx_unit(unit) {
        Some(sym) => Ok(Some((mag, sym))),
        None => Err(UNIT_I),
    }
}

/// `parse_vertical_align`: "super", "sub", "text-top", "text-bottom" or None. A length
/// raises above zero and lowers below it (#52); the unit may follow a space.
pub fn parse_vertical_align(value: &str) -> Option<&'static str> {
    if value.is_empty() {
        return None;
    }
    let raw = py::strip(value).to_lowercase();
    match raw.as_str() {
        "super" => return Some("super"),
        "sub" => return Some("sub"),
        "text-top" => return Some("text-top"),
        "text-bottom" => return Some("text-bottom"),
        _ => {}
    }
    let (mag, _) = number_unit(&raw, true, &["em", "rem", "%", "pt", "px", "mm"])?;
    let x = mag.parse::<f64>().ok()?;
    if x > 0.0 {
        Some("super")
    } else if x < 0.0 {
        Some("sub")
    } else {
        None
    }
}

/// The CSS named colours, all 148 (CSS Color 4), `rebeccapurple` too (#383).
#[rustfmt::skip]
const CSS_COLOURS: [(&str, i64); 148] = [
    ("aliceblue", 0xF0F8FF), ("antiquewhite", 0xFAEBD7), ("aqua", 0x00FFFF),
    ("aquamarine", 0x7FFFD4), ("azure", 0xF0FFFF), ("beige", 0xF5F5DC), ("bisque", 0xFFE4C4),
    ("black", 0x000000), ("blanchedalmond", 0xFFEBCD), ("blue", 0x0000FF),
    ("blueviolet", 0x8A2BE2), ("brown", 0xA52A2A), ("burlywood", 0xDEB887),
    ("cadetblue", 0x5F9EA0), ("chartreuse", 0x7FFF00), ("chocolate", 0xD2691E),
    ("coral", 0xFF7F50), ("cornflowerblue", 0x6495ED), ("cornsilk", 0xFFF8DC),
    ("crimson", 0xDC143C), ("cyan", 0x00FFFF), ("darkblue", 0x00008B), ("darkcyan", 0x008B8B),
    ("darkgoldenrod", 0xB8860B), ("darkgray", 0xA9A9A9), ("darkgreen", 0x006400),
    ("darkgrey", 0xA9A9A9), ("darkkhaki", 0xBDB76B), ("darkmagenta", 0x8B008B),
    ("darkolivegreen", 0x556B2F), ("darkorange", 0xFF8C00), ("darkorchid", 0x9932CC),
    ("darkred", 0x8B0000), ("darksalmon", 0xE9967A), ("darkseagreen", 0x8FBC8F),
    ("darkslateblue", 0x483D8B), ("darkslategray", 0x2F4F4F), ("darkslategrey", 0x2F4F4F),
    ("darkturquoise", 0x00CED1), ("darkviolet", 0x9400D3), ("deeppink", 0xFF1493),
    ("deepskyblue", 0x00BFFF), ("dimgray", 0x696969), ("dimgrey", 0x696969),
    ("dodgerblue", 0x1E90FF), ("firebrick", 0xB22222), ("floralwhite", 0xFFFAF0),
    ("forestgreen", 0x228B22), ("fuchsia", 0xFF00FF), ("gainsboro", 0xDCDCDC),
    ("ghostwhite", 0xF8F8FF), ("gold", 0xFFD700), ("goldenrod", 0xDAA520), ("gray", 0x808080),
    ("green", 0x008000), ("greenyellow", 0xADFF2F), ("grey", 0x808080), ("honeydew", 0xF0FFF0),
    ("hotpink", 0xFF69B4), ("indianred", 0xCD5C5C), ("indigo", 0x4B0082), ("ivory", 0xFFFFF0),
    ("khaki", 0xF0E68C), ("lavender", 0xE6E6FA), ("lavenderblush", 0xFFF0F5),
    ("lawngreen", 0x7CFC00), ("lemonchiffon", 0xFFFACD), ("lightblue", 0xADD8E6),
    ("lightcoral", 0xF08080), ("lightcyan", 0xE0FFFF), ("lightgoldenrodyellow", 0xFAFAD2),
    ("lightgray", 0xD3D3D3), ("lightgreen", 0x90EE90), ("lightgrey", 0xD3D3D3),
    ("lightpink", 0xFFB6C1), ("lightsalmon", 0xFFA07A), ("lightseagreen", 0x20B2AA),
    ("lightskyblue", 0x87CEFA), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xB0C4DE), ("lightyellow", 0xFFFFE0), ("lime", 0x00FF00),
    ("limegreen", 0x32CD32), ("linen", 0xFAF0E6), ("magenta", 0xFF00FF), ("maroon", 0x800000),
    ("mediumaquamarine", 0x66CDAA), ("mediumblue", 0x0000CD), ("mediumorchid", 0xBA55D3),
    ("mediumpurple", 0x9370DB), ("mediumseagreen", 0x3CB371), ("mediumslateblue", 0x7B68EE),
    ("mediumspringgreen", 0x00FA9A), ("mediumturquoise", 0x48D1CC),
    ("mediumvioletred", 0xC71585), ("midnightblue", 0x191970), ("mintcream", 0xF5FFFA),
    ("mistyrose", 0xFFE4E1), ("moccasin", 0xFFE4B5), ("navajowhite", 0xFFDEAD),
    ("navy", 0x000080), ("oldlace", 0xFDF5E6), ("olive", 0x808000), ("olivedrab", 0x6B8E23),
    ("orange", 0xFFA500), ("orangered", 0xFF4500), ("orchid", 0xDA70D6),
    ("palegoldenrod", 0xEEE8AA), ("palegreen", 0x98FB98), ("paleturquoise", 0xAFEEEE),
    ("palevioletred", 0xDB7093), ("papayawhip", 0xFFEFD5), ("peachpuff", 0xFFDAB9),
    ("peru", 0xCD853F), ("pink", 0xFFC0CB), ("plum", 0xDDA0DD), ("powderblue", 0xB0E0E6),
    ("purple", 0x800080), ("rebeccapurple", 0x663399), ("red", 0xFF0000),
    ("rosybrown", 0xBC8F8F), ("royalblue", 0x4169E1), ("saddlebrown", 0x8B4513),
    ("salmon", 0xFA8072), ("sandybrown", 0xF4A460), ("seagreen", 0x2E8B57),
    ("seashell", 0xFFF5EE), ("sienna", 0xA0522D), ("silver", 0xC0C0C0), ("skyblue", 0x87CEEB),
    ("slateblue", 0x6A5ACD), ("slategray", 0x708090), ("slategrey", 0x708090),
    ("snow", 0xFFFAFA), ("springgreen", 0x00FF7F), ("steelblue", 0x4682B4), ("tan", 0xD2B48C),
    ("teal", 0x008080), ("thistle", 0xD8BFD8), ("tomato", 0xFF6347), ("turquoise", 0x40E0D0),
    ("violet", 0xEE82EE), ("wheat", 0xF5DEB3), ("white", 0xFFFFFF), ("whitesmoke", 0xF5F5F5),
    ("yellow", 0xFFFF00), ("yellowgreen", 0x9ACD32),
];

/// Reads a colour function's pieces off the front of a string, as `re.fullmatch` would.
struct Scan<'a> {
    rest: &'a str,
}

impl<'a> Scan<'a> {
    /// `\s*`: Python's `\s` is `isspace`.
    fn spaces(&mut self) {
        self.rest = self.rest.trim_start_matches(py::is_space);
    }

    fn eat(&mut self, lit: &str) -> bool {
        match self.rest.strip_prefix(lit) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    /// The longest run of characters `keep` takes. Each pattern's next piece starts with
    /// none of them, so the longest is the only one that can match.
    fn run(&mut self, keep: impl Fn(char) -> bool) -> &'a str {
        let end = self.rest.find(|c| !keep(c)).unwrap_or(self.rest.len());
        let (run, rest) = self.rest.split_at(end);
        self.rest = rest;
        run
    }

    /// `\s*(\d+(?:\.\d+)?%|\d+)\s*`: (digits, percent).
    fn channel(&mut self) -> Option<(&'a str, bool)> {
        self.spaces();
        let start = self.rest;
        let int = self.run(is_decimal);
        if int.is_empty() {
            return None;
        }
        let after = self.rest;
        let percent = if self.eat("%") {
            Some(int)
        } else if self.eat(".") && !self.run(is_decimal).is_empty() && self.rest.starts_with('%') {
            let number = &start[..start.len() - self.rest.len()];
            self.eat("%");
            Some(number)
        } else {
            self.rest = after;
            None
        };
        self.spaces();
        Some(percent.map_or((int, false), |p| (p, true)))
    }

    /// `[\d.]+`.
    fn number(&mut self) -> Option<&'a str> {
        Some(self.run(|c| c == '.' || is_decimal(c))).filter(|n| !n.is_empty())
    }

    /// `(?:,\s*([\d.]+)\s*)?\)` and the end of the string.
    fn alpha_and_close(&mut self) -> Option<Option<&'a str>> {
        let alpha = if self.eat(",") {
            self.spaces();
            let alpha = self.number()?;
            self.spaces();
            Some(alpha)
        } else {
            None
        };
        (self.eat(")") && self.rest.is_empty()).then_some(alpha)
    }
}

/// `\d`: a decimal digit of any script.
fn is_decimal(c: char) -> bool {
    py::digit_value(c).is_some()
}

/// `float(x)` of decimal digits of any script and points: one point at most, a digit at
/// least, else `ValueError`.
fn digits_float(x: &str) -> Result<f64, Raised> {
    let ascii: String = x
        .chars()
        .map(|c| match py::digit_value(c) {
            Some(d) => char::from(b'0' + d as u8),
            None => c,
        })
        .collect();
    if ascii.matches('.').count() > 1 || !ascii.bytes().any(|b| b.is_ascii_digit()) {
        return Err(COLOUR_NUMBER);
    }
    ascii.parse().map_err(|_| COLOUR_NUMBER)
}

/// Python's `a % b` for floats: the sign of `b` (CPython `float_rem`).
fn py_mod(a: f64, b: f64) -> f64 {
    let m = a % b;
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) { m + b } else { m }
    } else {
        0.0f64.copysign(b)
    }
}

/// Python's `a // b` for floats (CPython `_float_div_mod`); NaN when `a` is inf.
fn py_floordiv(a: f64, b: f64) -> f64 {
    let m = a % b;
    let mut div = (a - m) / b;
    if m != 0.0 && (b < 0.0) != (m < 0.0) {
        div -= 1.0;
    }
    if div != 0.0 {
        let floor = div.floor();
        if div - floor > 0.5 {
            floor + 1.0
        } else {
            floor
        }
    } else {
        0.0f64.copysign(a / b)
    }
}

/// `colorsys.hls_to_rgb` (Python 3.14).
fn hls_to_rgb(h: f64, l: f64, s: f64) -> [f64; 3] {
    const ONE_THIRD: f64 = 1.0 / 3.0;
    const ONE_SIXTH: f64 = 1.0 / 6.0;
    const TWO_THIRD: f64 = 2.0 / 3.0;
    if s == 0.0 {
        return [l, l, l];
    }
    let m2 = if l <= 0.5 {
        l * (1.0 + s)
    } else {
        l + s - (l * s)
    };
    let m1 = 2.0 * l - m2;
    // A NaN hue fails every test and is m1.
    let v = |hue: f64| {
        let hue = py_mod(hue, 1.0);
        if hue < ONE_SIXTH {
            m1 + (m2 - m1) * hue * 6.0
        } else if hue < 0.5 {
            m2
        } else if hue < TWO_THIRD {
            m1 + (m2 - m1) * (TWO_THIRD - hue) * 6.0
        } else {
            m1
        }
    };
    [v(h + ONE_THIRD), v(h), v(h - ONE_THIRD)]
}

/// `round(x)` to an int: half to even. `x` is a channel, 0 to 255, never NaN here.
fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// `_rgb_alpha`: a colour as (0xRRGGBB, alpha), or None when it can't be read.
fn rgb_alpha(value: &str) -> Result<Option<(i64, f64)>, Raised> {
    let v = py::strip(value).to_lowercase();
    if let Some(&(_, rgb)) = CSS_COLOURS.iter().find(|(name, _)| *name == v) {
        return Ok(Some((rgb, 1.0)));
    }
    let hex = v
        .strip_prefix('#')
        .filter(|h| h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    match hex.map(str::len) {
        Some(3) => {
            let doubled: String = hex.unwrap_or("").chars().flat_map(|c| [c, c]).collect();
            return Ok(i64::from_str_radix(&doubled, 16).ok().map(|rgb| (rgb, 1.0)));
        }
        Some(6) => {
            return Ok(i64::from_str_radix(hex.unwrap_or(""), 16)
                .ok()
                .map(|rgb| (rgb, 1.0)));
        }
        _ => {}
    }
    let mut sc = Scan { rest: &v };
    let (rgb, alpha) = if sc.eat("rgba(") || sc.eat("rgb(") {
        let mut channels = Vec::with_capacity(3);
        for i in 0..3 {
            if i > 0 && !sc.eat(",") {
                return Ok(None);
            }
            let Some(channel) = sc.channel() else {
                return Ok(None);
            };
            channels.push(channel);
        }
        let Some(alpha) = sc.alpha_and_close() else {
            return Ok(None);
        };
        let mut rgb = 0;
        for (x, percent) in channels {
            let c = if percent {
                // floor(p x 2.56), capped at 255 (#394). Too big for a float, p is inf and
                // inf // 100 is NaN, which int() refuses.
                let c = py_floordiv(digits_float(x)? * 256.0, 100.0);
                if c.is_nan() {
                    return Err(PERCENT_INF);
                }
                c.min(255.0) as i64
            } else {
                py::py_int(x).ok_or(CHANNEL_DIGITS)?.min(255)
            };
            rgb = (rgb << 8) | c;
        }
        (rgb, alpha)
    } else if sc.eat("hsla(") || sc.eat("hsl(") {
        sc.spaces();
        let Some(h) = sc.number() else {
            return Ok(None);
        };
        sc.eat("deg");
        let mut parts = vec![h];
        for _ in 0..2 {
            sc.spaces();
            if !sc.eat(",") {
                return Ok(None);
            }
            sc.spaces();
            let Some(part) = sc.number() else {
                return Ok(None);
            };
            if !sc.eat("%") {
                return Ok(None);
            }
            parts.push(part);
        }
        sc.spaces();
        let Some(alpha) = sc.alpha_and_close() else {
            return Ok(None);
        };
        let (h, sat, light) = (
            digits_float(parts[0])?,
            digits_float(parts[1])?,
            digits_float(parts[2])?,
        );
        let [r, g, b] = hls_to_rgb(
            py_mod(h, 360.0) / 360.0,
            light.min(100.0) / 100.0,
            sat.min(100.0) / 100.0,
        )
        .map(|c| round_int(c * 255.0));
        ((r << 16) | (g << 8) | b, alpha)
    } else {
        return Ok(None);
    };
    let alpha = match alpha {
        None => 1.0,
        Some(a) => digits_float(a)?,
    };
    Ok(Some((rgb, alpha)))
}

/// `css_rgb`: a colour as 0xRRGGBB, or None when it can't be read. A translucent one is
/// blended onto the white page, each channel rounded half to even (#383).
pub fn css_rgb(value: &str) -> Result<Option<i64>, Raised> {
    let Some((rgb, alpha)) = rgb_alpha(value)? else {
        return Ok(None);
    };
    if alpha >= 1.0 {
        return Ok(Some(rgb));
    }
    let mut out = 0;
    for shift in [16, 8, 0] {
        let c = ((rgb >> shift) & 0xFF) as f64;
        out |= round_int(c * alpha + 255.0 * (1.0 - alpha)) << shift;
    }
    Ok(Some(out))
}

/// `is_transparent`: `transparent`, or an alpha of 0 or less.
pub fn is_transparent(value: &str) -> Result<bool, Raised> {
    let v = py::strip(value).to_lowercase();
    if v == "transparent" {
        return Ok(true);
    }
    Ok(rgb_alpha(&v)?.is_some_and(|(_, alpha)| alpha <= 0.0))
}

fn channels(rgb: i64) -> [i64; 3] {
    [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF]
}

fn floats(ch: [i64; 3]) -> [f64; 3] {
    ch.map(|c| c as f64)
}

/// `_scaled`: `ch` scaled, keeping its hue, to the luminance `target`: down when `darken`,
/// else up. A colour whose form with its top channel at 254 is still below `target` is
/// kept (P734, P739b, #734).
fn scaled(ch: [i64; 3], target: f64, darken: bool) -> [i64; 3] {
    let f = floats(ch);
    if darken {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            if luminance(f.map(|c| c * mid)) > target {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        // int() truncates.
        return f.map(|c| (c * lo).trunc() as i64);
    }
    let top = ch.into_iter().max().unwrap_or(0);
    // Black lightens as grey.
    let (base, max_base) = if top != 0 {
        (f, top as f64)
    } else {
        ([1.0; 3], 1.0)
    };
    let (mut lo, mut hi) = (if top != 0 { 1.0 } else { 0.0 }, 255.0 / max_base);
    if luminance(base.map(|c| c * 254.0 / max_base)) < target {
        return ch;
    }
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if luminance(base.map(|c| c * mid)) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    base.map(|c| ((c * hi - 1e-9).ceil() as i64).min(255))
}

/// `css_text_colour`: a text colour as the ARGB Previewer writes, or None when it draws
/// nothing or can't be read (#239). Under 4.5:1 contrast with `background` (0xRRGGBB, or
/// None for the white page) it moves toward black or white, keeping its hue (#383).
pub fn css_text_colour(value: &str, background: Option<i64>) -> Result<Option<i64>, Raised> {
    if is_transparent(value)? {
        return Ok(None);
    }
    let Some(rgb) = css_rgb(value)? else {
        return Ok(None);
    };
    let mut ch = channels(rgb);
    let lum = luminance(floats(channels(background.unwrap_or(0xFF_FFFF))));
    // The luminance of 4.5:1 against white.
    let max_luminance = 1.05 / 4.5 - 0.05;
    if background.is_none() {
        if luminance(floats(ch)) > max_luminance {
            ch = scaled(ch, max_luminance, true);
        }
    } else {
        let mine = luminance(floats(ch));
        if (mine.max(lum) + 0.05) / (mine.min(lum) + 0.05) < 4.5 {
            // Black contrasts more with a light background, white with a dark one.
            let darken = (lum + 0.05) / 0.05 >= 1.05 / (lum + 0.05);
            let target = if darken {
                (lum + 0.05) / 4.5 - 0.05
            } else {
                4.5 * (lum + 0.05) - 0.05
            };
            ch = scaled(ch, target, darken);
        }
    }
    Ok(Some(BLACK | (ch[0] << 16) | (ch[1] << 8) | ch[2]))
}

/// `css_link_colour`: a link's own colour as given, opaque, or None (#728).
pub fn css_link_colour(value: &str) -> Result<Option<i64>, Raised> {
    if is_transparent(value)? {
        return Ok(None);
    }
    Ok(css_rgb(value)?.map(|rgb| BLACK | rgb))
}

/// `str.lower().split()`.
fn lower_words(value: &str) -> Vec<String> {
    value
        .to_lowercase()
        .split(py::is_space)
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `css_small_caps`: "small-caps" for a `font-variant` naming it (#239).
pub fn css_small_caps(value: &str) -> Option<&'static str> {
    lower_words(value)
        .iter()
        .any(|w| w == "small-caps")
        .then_some("small-caps")
}

/// `value.strip().lower()` when it is one of `keywords`.
fn keyword(value: &str, keywords: &[&'static str]) -> Option<&'static str> {
    let v = py::strip(value).to_lowercase();
    keywords.iter().copied().find(|k| *k == v)
}

/// `css_text_transform`: uppercase, lowercase or capitalize.
pub fn css_text_transform(value: &str) -> Option<&'static str> {
    keyword(value, &["uppercase", "lowercase", "capitalize"])
}

/// `css_letter_spacing`: a widening letter-spacing as the KFX length Previewer writes: em,
/// rem and pt as written, px as 0.45pt each (#239); not a negative one, a % or mm.
pub fn css_letter_spacing(value: &str) -> Result<Option<(String, &'static str)>, Raised> {
    let Some((mag, unit)) = parse_css_length(value, false)? else {
        return Ok(None);
    };
    match unit {
        "$314" | "$316" => Ok(None),
        "$319" => {
            // The length pattern's number ([+-]?[0-9]*\.?[0-9]+), which float() and Rust
            // both always read.
            let px: f64 = mag.parse().unwrap_or(0.0);
            Ok(Some((py::format_g(px * 0.45, 4), "$318")))
        }
        _ => Ok(Some((mag, unit))),
    }
}

/// `css_text_orientation`: upright, sideways or mixed (#247).
pub fn css_text_orientation(value: &str) -> Option<&'static str> {
    keyword(value, &["upright", "sideways", "mixed"])
}

const EMPHASIS_SHAPES: [&str; 5] = ["dot", "circle", "double-circle", "triangle", "sesame"];

/// `css_text_emphasis`: "filled sesame", "open dot", "filled" and so on, a mark of the
/// book's own in double quotes, "none", or None (#247).
pub fn css_text_emphasis(value: &str) -> Option<String> {
    let raw = py::strip(value);
    let mut ends = raw.chars();
    let (first, last) = (ends.next(), ends.next_back());
    if let (Some(q @ ('"' | '\'')), Some(l)) = (first, last)
        && q == l
    {
        // Both ends are one byte.
        let inner = &raw[1..raw.len() - 1];
        return (!inner.is_empty()).then(|| format!("\"{inner}\""));
    }
    let words = lower_words(raw);
    if words == ["none"] {
        return Some("none".into());
    }
    let fill = if words.iter().any(|w| w == "open") {
        "open"
    } else {
        "filled"
    };
    let shapes: Vec<&String> = words
        .iter()
        .filter(|w| EMPHASIS_SHAPES.contains(&w.as_str()))
        .collect();
    let known = |w: &String| w == "open" || w == "filled" || EMPHASIS_SHAPES.contains(&w.as_str());
    if words.is_empty() || !words.iter().all(known) || shapes.len() > 1 {
        return None;
    }
    Some(match shapes.first() {
        Some(shape) => format!("{fill} {shape}"),
        None => fill.to_owned(),
    })
}

/// A run's typography from its CSS, each as its flag carries it, or None (#239, #247).
#[derive(Clone, Debug, PartialEq)]
pub struct Typography {
    pub small_caps: Option<&'static str>,
    pub transform: Option<&'static str>,
    pub spacing: Option<(String, &'static str)>,
    pub colour: Option<i64>,
    pub orientation: Option<&'static str>,
    pub emphasis: Option<String>,
}

/// `str(value or "")` of a style value. The resolver gives a str or None (R-inline §1).
/// Python would give anything else's repr, which none of the helpers here reads as a
/// keyword, a length or a colour; "?" stands for it.
fn style_text(v: Option<&PyValue>) -> &str {
    match v {
        Some(PyValue::Str(s)) => s,
        Some(v) if v.is_truthy() => "?",
        _ => "",
    }
}

/// `css_typography`: from an element's computed CSS, `css(key)` being `css.get(key)`.
pub fn css_typography<'a>(css: impl Fn(&str) -> Option<&'a PyValue>) -> Result<Typography, Raised> {
    let small_caps = css_small_caps(style_text(css("font-variant")));
    let transform = css_text_transform(style_text(css("text-transform")));
    let spacing = css_letter_spacing(style_text(css("letter-spacing")))?;
    // Unset is black, measured against the box behind the text or the page (#383). The
    // resolver gives the box's fill as an int or None.
    let colour = match style_text(css("color")) {
        "" => "black",
        c => c,
    };
    let behind = match css("background-behind") {
        Some(PyValue::Int(bg)) => Some(*bg),
        _ => None,
    };
    let colour = css_text_colour(colour, behind)?;
    Ok(Typography {
        small_caps,
        transform,
        spacing,
        colour,
        orientation: css_text_orientation(style_text(css("text-orientation"))),
        emphasis: css_text_emphasis(style_text(css("text-emphasis-style"))),
    })
}

/// `css_font_size`: an element's font size in CSS px as the resolver gives it (an int or a
/// float above 0; a NaN passes, as `size <= 0` is false for it), else None (#241).
pub fn css_font_size<'a>(css: impl Fn(&str) -> Option<&'a PyValue>) -> Option<&'a PyValue> {
    match css("font-size") {
        Some(v @ PyValue::Int(i)) if *i > 0 => Some(v),
        Some(v @ PyValue::Float(f)) if *f > 0.0 || f.is_nan() => Some(v),
        _ => None,
    }
}

/// Monospace families a book names without the generic `monospace` after them.
const MONOSPACE_NAMES: [&str; 20] = [
    "courier",
    "courier new",
    "consolas",
    "menlo",
    "monaco",
    "lucida console",
    "lucida sans typewriter",
    "andale mono",
    "dejavu sans mono",
    "liberation mono",
    "source code pro",
    "inconsolata",
    "fira mono",
    "fira code",
    "ubuntu mono",
    "roboto mono",
    "noto sans mono",
    "droid sans mono",
    "sf mono",
    "pt mono",
];

/// `is_monospace`: a parsed font-family list names `monospace`, or opens on a known
/// monospace face.
pub fn is_monospace(families: &[String]) -> bool {
    families.iter().any(|f| f == "monospace")
        || families
            .first()
            .is_some_and(|f| MONOSPACE_NAMES.contains(&f.as_str()))
}

/// `_parse_font_family`: a font-family value as stripped, lowercased names, a comma inside
/// quotes part of a name (#487): `re.findall(r'"([^"]*)"|\'([^\']*)\'|([^,"\']+)')`. A quote
/// with no closing one matches nothing and is passed over. Linear: only the last quote of
/// each kind can fail to close.
pub fn parse_font_family(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(c) = rest.chars().next() {
        let (name, len) = match c {
            '"' | '\'' => match rest[1..].find(c) {
                Some(end) => (&rest[1..1 + end], end + 2),
                None => ("", 1),
            },
            ',' => ("", 1),
            _ => {
                let end = rest.find([',', '"', '\'']).unwrap_or(rest.len());
                (&rest[..end], end)
            }
        };
        let name = py::strip(name).to_lowercase();
        if !name.is_empty() {
            out.push(name);
        }
        rest = &rest[len..];
    }
    out
}

/// `page_bidi`: the `bidi` of the first block whose block style, box or table has one,
/// else None (#714).
pub fn page_bidi(blocks: &[PyValue]) -> Option<&PyValue> {
    blocks.iter().find_map(|b| {
        ["block_style", "box", "table"]
            .iter()
            .filter_map(|key| b.get(key)?.get("bidi"))
            .find(|v| v.is_truthy())
    })
}

#[cfg(test)]
mod tests {
    //! The helpers the unit table has no rows for, on odd input, against what the pinned
    //! Python gives.
    use super::*;

    fn s(x: &str) -> PyValue {
        PyValue::Str(x.into())
    }

    fn dict(pairs: Vec<(&str, PyValue)>) -> PyValue {
        PyValue::Dict(pairs.into_iter().map(|(k, v)| (s(k), v)).collect())
    }

    /// Each kind of input kfxgen raises on, through every helper that reaches it, and the
    /// zero and sign checks that come before the unit's `KeyError`.
    #[test]
    fn inputs_kfxgen_raises_on_are_raised() {
        let big = "9".repeat(400);
        let long = "1".repeat(4301);
        assert_eq!(parse_css_length("1\u{130}n", true), Err(UNIT_I));
        assert_eq!(parse_css_length("1\u{131}N", false), Err(UNIT_I));
        assert_eq!(parse_css_length("0\u{131}n", false), Ok(None));
        assert_eq!(parse_css_length("-1\u{131}n", false), Ok(None));
        assert_eq!(css_letter_spacing("2\u{131}n"), Err(UNIT_I));
        for colour in [
            "rgba(0,0,0,1.2.3)",
            "rgba(0,0,0,.)",
            "hsl(1.2.3,10%,10%)",
            "hsl(0,.%,0%)",
            "hsla(0,0%,0%,..)",
        ] {
            assert_eq!(css_rgb(colour), Err(COLOUR_NUMBER), "{colour}");
            assert_eq!(is_transparent(colour), Err(COLOUR_NUMBER), "{colour}");
            assert_eq!(css_text_colour(colour, Some(0x333333)), Err(COLOUR_NUMBER));
            assert_eq!(css_link_colour(colour), Err(COLOUR_NUMBER), "{colour}");
        }
        assert_eq!(css_rgb(&format!("rgb(1,2,{big}.5%)")), Err(PERCENT_INF));
        assert_eq!(
            is_transparent(&format!("rgb({big}%,0,0)")),
            Err(PERCENT_INF)
        );
        assert_eq!(
            css_text_colour(&format!("rgb({long},0,0)"), None),
            Err(CHANNEL_DIGITS)
        );
        // A percentage alpha is no colour at all, not an error.
        assert_eq!(css_link_colour(&format!("rgba(0,0,0,{big}%)")), Ok(None));
        // The first to raise wins: letter spacing comes before colour.
        let css = dict(vec![
            ("letter-spacing", s("1\u{131}n")),
            ("color", s("rgba(0,0,0,.)")),
        ]);
        assert_eq!(css_typography(|k| css.get(k)), Err(UNIT_I));
        let css = dict(vec![("color", s("rgba(0,0,0,.)"))]);
        assert_eq!(css_typography(|k| css.get(k)), Err(COLOUR_NUMBER));
    }

    #[test]
    fn emphasis_marks_quotes_and_words() {
        for (value, want) in [
            ("\"\u{2605}\"", Some("\"\u{2605}\"")),
            ("'é'", Some("\"é\"")),
            ("\"\"", None),
            ("\"", None),
            ("' x '", Some("\" x \"")),
            ("\u{1c} open  SESAME\u{3000}", Some("open sesame")),
            ("dot dot", None),
            ("open filled", Some("open")),
            ("filled", Some("filled")),
            ("none", Some("none")),
            ("NONE dot", None),
            ("", None),
            ("\u{a0}'*'\u{a0}", Some("\"*\"")),
            ("triangle open", Some("open triangle")),
        ] {
            assert_eq!(css_text_emphasis(value).as_deref(), want, "{value:?}");
        }
    }

    #[test]
    fn font_families_with_odd_quotes() {
        for (value, want) in [
            ("\"Courier New\", serif", vec!["courier new", "serif"]),
            ("'a,b' , c", vec!["a,b", "c"]),
            ("\"unclosed, mono", vec!["unclosed", "mono"]),
            ("x'y", vec!["x", "y"]),
            (" , ,", vec![]),
            ("A\u{3000}\"B\"\u{1c}", vec!["a", "b"]),
            ("''", vec![]),
            ("\"a\"b\"c", vec!["a", "b", "c"]),
        ] {
            assert_eq!(parse_font_family(value), want, "{value:?}");
        }
        let names = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(is_monospace(&names(&["menlo", "serif"])));
        assert!(is_monospace(&names(&["serif", "monospace"])));
        assert!(!is_monospace(&[]));
        assert!(!is_monospace(&names(&["serif", "menlo"])));
    }

    #[test]
    fn typography_from_css() {
        assert_eq!(css_small_caps("ALL\u{3000}SMALL-CAPS"), Some("small-caps"));
        assert_eq!(css_small_caps("small-capsx"), None);
        assert_eq!(
            css_text_transform("\u{1c}UPPERCASE\u{a0}"),
            Some("uppercase")
        );
        assert_eq!(css_text_orientation(" Upright "), Some("upright"));
        let css = dict(vec![
            ("font-variant", s("small-caps")),
            ("color", s("")),
            ("letter-spacing", s("2px")),
            ("background-behind", PyValue::Int(0x333333)),
            ("text-emphasis-style", s("'x'")),
        ]);
        assert_eq!(
            css_typography(|k| css.get(k)),
            Ok(Typography {
                small_caps: Some("small-caps"),
                transform: None,
                spacing: Some(("0.9".into(), "$318")),
                colour: Some(4288387995),
                orientation: None,
                emphasis: Some("\"x\"".into()),
            })
        );
        let white = dict(vec![
            ("color", s("white")),
            ("background-behind", PyValue::None),
        ]);
        assert_eq!(
            css_typography(|k| white.get(k)).unwrap().colour,
            Some(4285953654)
        );
        assert_eq!(css_typography(|_| None).unwrap().colour, Some(BLACK));
        assert!(is_transparent(" RGBA(1,2,3,0) ").unwrap());
        assert!(is_transparent("hsla(0,0%,0%,0.0)").unwrap());
        assert!(!is_transparent("rgba(1,2,3,0.1)").unwrap());
    }

    #[test]
    fn nested_shifts_and_flag_makers() {
        let f = |x: &str| Flag::Str(x.into());
        assert_eq!(nested_shift(&f("sub@2")), Some(("sub", 2, false)));
        assert_eq!(nested_shift(&f("super@3=")), Some(("super", 3, true)));
        assert_eq!(nested_shift(&f("baseline@2")), Some(("baseline", 2, false)));
        for no in ["bold@2", "sub@", "sub@2x", "sub@2@3"] {
            assert_eq!(nested_shift(&f(no)), None, "{no}");
        }
        // isdigit() and int() read any decimal digit.
        assert_eq!(nested_shift(&f("sub@\u{661}")), Some(("sub", 1, false)));
        assert_eq!(
            make_bidi_flag(3, 1, &[("lang", "fr"), ("direction", "rtl")]).to_py(),
            PyValue::Tuple(vec![
                s("bidi"),
                PyValue::Tuple(vec![
                    PyValue::Int(3),
                    PyValue::Int(1),
                    PyValue::Tuple(vec![
                        PyValue::Tuple(vec![s("direction"), s("rtl")]),
                        PyValue::Tuple(vec![s("lang"), s("fr")]),
                    ]),
                ]),
            ])
        );
    }

    /// The tuples the makers give, as the pinned Python's do (its serials from its own
    /// counters; the port's from the caller).
    #[test]
    fn flag_makers_give_kfxgens_tuples() {
        let t = PyValue::Tuple;
        let i = PyValue::Int;
        let flag = |kind: &str, v: PyValue| t(vec![s(kind), v]);
        assert_eq!(
            make_link_flag("c.xhtml#n").to_py(),
            flag("link", s("c.xhtml#n"))
        );
        assert_eq!(
            make_ruby_flag(1, "ふ").to_py(),
            flag("ruby", t(vec![i(1), s("ふ")]))
        );
        assert_eq!(
            make_tcy_flag(0, "12").to_py(),
            flag("tcy", t(vec![i(0), s("12")]))
        );
        let bold = Span {
            start: 0,
            len: 1,
            flags: Flags::from_iter([Flag::Str("bold".into())]),
        };
        assert_eq!(
            make_turn_flag(0, "horizontal", "AB", &[bold]).to_py(),
            flag(
                "turn",
                t(vec![
                    i(0),
                    s("horizontal"),
                    s("AB"),
                    t(vec![t(vec![i(0), i(1), PyValue::Set(vec![s("bold")])])]),
                ])
            )
        );
        assert_eq!(
            make_mono_flag(&["menlo".into(), "monospace".into()]).to_py(),
            flag("mono", t(vec![s("menlo"), s("monospace")]))
        );
        assert_eq!(
            make_size_flag(PyValue::Float(24.0)).to_py(),
            flag("size", PyValue::Float(24.0))
        );
    }

    /// An int or a float above 0, NaN and inf too; not a bool, a string or 0.
    #[test]
    fn font_size_as_the_resolver_gives_it() {
        let size = |v: PyValue| {
            let css = dict(vec![("font-size", v)]);
            css_font_size(|k| css.get(k)).cloned()
        };
        assert_eq!(css_font_size(|_| None), None);
        assert_eq!(size(PyValue::Float(24.0)), Some(PyValue::Float(24.0)));
        assert_eq!(size(PyValue::Int(12)), Some(PyValue::Int(12)));
        assert_eq!(
            size(PyValue::Float(f64::INFINITY)),
            Some(PyValue::Float(f64::INFINITY))
        );
        assert!(matches!(size(PyValue::Float(f64::NAN)), Some(PyValue::Float(x)) if x.is_nan()));
        for none in [
            PyValue::Float(0.0),
            PyValue::Float(-1.5),
            PyValue::Bool(true),
            PyValue::Int(0),
            s("12px"),
            PyValue::None,
        ] {
            assert_eq!(size(none.clone()), None, "{none:?}");
        }
    }

    #[test]
    fn page_direction_from_the_first_block_that_has_one() {
        let rtl = dict(vec![("direction", s("rtl"))]);
        let blocks = [
            dict(vec![
                ("block_style", dict(vec![("bidi", dict(vec![]))])),
                ("box", dict(vec![("bidi", rtl.clone())])),
            ]),
            dict(vec![(
                "table",
                dict(vec![("bidi", dict(vec![("x", PyValue::Int(1))]))]),
            )]),
        ];
        assert_eq!(page_bidi(&blocks), Some(&rtl));
        assert_eq!(
            page_bidi(&[dict(vec![("block_style", PyValue::None)]), dict(vec![])]),
            None
        );
    }
}
