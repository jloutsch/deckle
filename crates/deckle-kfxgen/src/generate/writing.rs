// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing direction and language: a block's, a container's and an inline element's
//! direction, unicode-bidi and language as Kindle Previewer 4.0.3 writes them in `$157`
//! (#246, #605, #710, #714), and the runs an inline element's direction lays out
//! (`_direction_layers`, `native_generator.py:402`).
use super::inline::{Flag, Flags, Span};
use super::{GenerateError, model_err};
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

/// `page_bidi` (`inline_style.py:442`): the first of `bidis` that is set.
pub(crate) fn page_bidi<'a>(bidis: impl IntoIterator<Item = Option<&'a Bidi>>) -> Option<Bidi> {
    bidis.into_iter().flatten().find(|b| b.set).cloned()
}

/// A `$157` key's value: a language as written, a direction's symbol.
pub(crate) fn bidi_value(key: &str, value: &str) -> deckle_kfx::ion::Value {
    match bidi_symbol(value).filter(|_| key != "$10") {
        Some(symbol) => super::sym(symbol),
        None => deckle_kfx::ion::Value::string(value),
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
    let mut links: Vec<Range> = Vec::new();
    for sp in spans {
        let plain = plain(&sp.flags);
        let end = sp.start + sp.len;
        for f in &sp.flags {
            let Flag::Bidi(serial, depth, keys) = f else {
                continue;
            };
            match dirs.iter_mut().find(|(s, _)| s == serial) {
                Some((_, r)) => {
                    r.end = r.end.max(end);
                    r.flags = &r.flags & &plain;
                }
                None => dirs.push((
                    *serial,
                    Range {
                        start: sp.start,
                        end,
                        level: 8 * i128::from(*depth),
                        keys: Some(keys.clone()),
                        flags: plain.clone(),
                    },
                )),
            }
        }
        for f in plain.iter().filter(|f| matches!(f, Flag::Link(_))) {
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
    let mut layers: Vec<Range> = dirs.into_iter().map(|(_, r)| r).collect();
    let n_dirs = layers.len();
    for mut link in links {
        let (a, b) = (link.start, link.end);
        link.level = layers[..n_dirs]
            .iter()
            .filter(|r| r.start <= a && b <= r.end)
            .map(|r| r.level)
            .max()
            .unwrap_or(0)
            + 2;
        let mut inside = spans
            .iter()
            .filter(|sp| a <= sp.start && sp.start + sp.len <= b)
            .map(|sp| plain(&sp.flags));
        let first = inside.next().unwrap_or_default();
        link.flags = inside.fold(first, |acc, f| &acc & &f);
        layers.push(link);
    }
    let contains = |o: &Range, a: i64, b: i64| o.start <= a && b <= o.end;
    // What each layer carries: its flags less those of the layers below it, lowest first.
    let mut by_level: Vec<usize> = (0..layers.len()).collect();
    by_level.sort_by_key(|&i| layers[i].level);
    let mut carried: Vec<Option<Flags>> = vec![None; layers.len()];
    for &i in &by_level {
        let r = &layers[i];
        let mut flags = r.flags.clone();
        for (o, c) in layers.iter().zip(&carried) {
            if o.level < r.level && contains(o, r.start, r.end) {
                let c = c.as_ref().expect("a lower layer is carried first");
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
        let inside: Vec<usize> = (0..layers.len())
            .filter(|&j| layers[j].level < r.level && contains(&layers[j], r.start, r.end))
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
    for sp in spans {
        let inside: Vec<usize> = (0..layers.len())
            .filter(|&j| contains(&layers[j], sp.start, sp.start + sp.len))
            .collect();
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
