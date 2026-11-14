// SPDX-License-Identifier: GPL-3.0-only
//! Style values: what Calibre 9.18's `Style` gives kfxgen for each element of a flattened file
//! (phase 2 plan, Task 10). Ported from `ebooks/oeb/stylizer.py` (`Style`, lines 518-944) and
//! `ebooks/__init__.py` (`unit_convert`); GPL v3, `NOTICE`.
//!
//! kfxgen reads an element's style as `style[prop]`, a length in points or the value as text,
//! and as `style._get(prop)`, the value with inheritance. [`Styles`] answers both from the
//! declarations kfxgen's `Stylizer` holds for each element ([`Flat::specified`]).
//!
//! Hostile input: no answer recurses per tree level. Calibre asks the parent's `Style`, which
//! asks its parent; the port walks up in a loop to the nearest element already answered, then
//! fills each element on the way down, so a tree tens of thousands deep costs one pass per
//! property. Every answer is kept per element: the font size, width, height and line height in
//! one slot each, an inherited value as the element that declares it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::defaults::default_value;
use super::flatten::Flat;
use super::normalize::{is_color, primitives};
use super::{Decls, Error, INHERITED, Profile};
use crate::dom::{Dom, NodeId, py_space};

/// A style value as Calibre's `Style[prop]` gives it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A length in points, or a number `unit_convert` read as one; NaN and infinity included,
    /// as Python reads `nan` and `1e999`.
    Pt(f64),
    /// The value as written, where `unit_convert` does not read it (`bold`, `initial`).
    Text(String),
    /// Python's `None`: `background-color` where the element sets no colour.
    Absent,
}

/// A value as `unit_convert` gives it, the text borrowed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Num<'a> {
    Pt(f64),
    Text(&'a str),
}

impl From<Num<'_>> for Value {
    fn from(n: Num<'_>) -> Value {
        match n {
            Num::Pt(v) => Value::Pt(v),
            Num::Text(t) => Value::Text(t.to_owned()),
        }
    }
}

/// An inherited value's slot ([`Styles::source`]): not worked out yet.
const UNKNOWN: u32 = u32::MAX;
/// An inherited value's slot: Calibre's default, or none.
const DEFAULT: u32 = u32::MAX - 1;

/// For each element (by [`NodeId`] index), the element whose declaration `_get` gives, or
/// [`DEFAULT`]; [`UNKNOWN`] where not worked out yet.
enum Sources {
    /// An inherited property: one slot per element, since any element may ask its parent.
    Every(Box<[u32]>),
    /// Any other property, which only `inherit` takes from the parent: the elements that
    /// declare `inherit` and those their walks reached, so no more than the declarations.
    Some(HashMap<u32, u32>),
}

impl Sources {
    fn get(&self, i: usize) -> u32 {
        match self {
            Sources::Every(all) => all[i],
            Sources::Some(map) => map.get(&(i as u32)).copied().unwrap_or(UNKNOWN),
        }
    }

    fn set(&mut self, i: usize, at: u32) {
        match self {
            Sources::Every(all) => all[i] = at,
            Sources::Some(map) => {
                map.insert(i as u32, at);
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        match self {
            Sources::Every(all) => all.len(),
            Sources::Some(map) => map.len(),
        }
    }
}

/// The style values of one flattened file, worked out as asked and kept.
pub struct Styles<'a> {
    dom: &'a Dom,
    flat: &'a Flat,
    profile: &'a Profile,
    font_size: Box<[Cell<Option<f64>>]>,
    width: Box<[Cell<Option<f64>>]>,
    height: Box<[Cell<Option<f64>>]>,
    line_height: Box<[Cell<Option<Num<'a>>>]>,
    /// Property → where each element's `_get` value is.
    found: RefCell<HashMap<Box<str>, Sources>>,
}

fn slots<T: Copy>(n: usize) -> Box<[Cell<Option<T>>]> {
    (0..n).map(|_| Cell::new(None)).collect()
}

impl<'a> Styles<'a> {
    /// The styles of `dom`'s elements, `flat` being what [`super::flatten`] (and
    /// [`super::remove_fake_margins`]) made of it. Calibre's `Stylizer` uses the `default`
    /// output profile ([`super::DEFAULT_PROFILE`]).
    pub fn new(dom: &'a Dom, flat: &'a Flat, profile: &'a Profile) -> Styles<'a> {
        let n = dom.node_count();
        Styles {
            dom,
            flat,
            profile,
            font_size: slots(n),
            width: slots(n),
            height: slots(n),
            line_height: slots(n),
            found: RefCell::new(HashMap::new()),
        }
    }

    /// `Style.get(prop)`: the element's own declared value.
    pub fn get(&self, n: NodeId, prop: &str) -> Option<&'a str> {
        own(self.flat, n.index(), prop)
    }

    /// `Style._get(prop)`: the element's value, `inherit` and an inherited property it does not
    /// declare taken from its parent, else Calibre's default. [`Error::NoDefault`] where
    /// Calibre raises `KeyError`: no value and no default.
    pub fn inherited(&self, n: NodeId, prop: &str) -> Result<&'a str, Error> {
        match self.source(n, prop) {
            DEFAULT => default_value(prop).ok_or_else(|| Error::NoDefault {
                prop: prop.to_owned(),
            }),
            at => Ok(own(self.flat, at as usize, prop).unwrap_or_default()),
        }
    }

    /// `Style[prop]` for `prop` as CSS writes it. `font-size`, `width`, `height` and
    /// `line-height` have their own rules; `color` falls back to `black` and
    /// `background-color` is the element's own, not inherited; margins and padding are
    /// lengths against the parent's width, any other property against the element's own.
    pub fn computed(&self, n: NodeId, prop: &str) -> Result<Value, Error> {
        Ok(match prop {
            "color" => Value::Text(self.color(n).to_owned()),
            "background-color" => self
                .background_color(n)
                .map_or(Value::Absent, |c| Value::Text(c.to_owned())),
            "font-size" => Value::Pt(self.font_size(n)),
            "width" => Value::Pt(self.width(n)),
            "height" => Value::Pt(self.height(n)),
            "line-height" => self.line_height(n).into(),
            "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding-top"
            | "padding-right" | "padding-bottom" | "padding-left" => {
                let v = self.inherited(n, prop)?;
                self.convert(v, || self.parent_width(n), || self.font_size(n))
                    .into()
            }
            _ => {
                let v = self.inherited(n, prop)?;
                self.convert(v, || self.width(n), || self.font_size(n))
                    .into()
            }
        })
    }

    /// `Style._pseudo_classes[name]`: the element's declarations under pseudo-class `name`.
    pub fn pseudo(&self, n: NodeId, name: &str) -> Option<&'a Decls> {
        self.flat
            .pseudo
            .get(n.index())?
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, d)| d)
    }

    /// `_unit_convert`: `value` in points against `base`, `em` against `font`. Each is worked
    /// out only where the unit needs it (Calibre works out both first; they change nothing).
    fn convert(
        &self,
        value: &'a str,
        base: impl FnOnce() -> f64,
        font: impl FnOnce() -> f64,
    ) -> Num<'a> {
        unit_convert(value, base, font, self.profile)
    }

    /// Where `_get(prop)` finds the element's value: the element that declares it, or
    /// [`DEFAULT`]. Walks up to the nearest element already answered, then gives each element
    /// on the way the answer. A property that is not inherited, asked of an element that does
    /// not declare it `inherit`, is answered from the element alone and keeps nothing.
    fn source(&self, n: NodeId, prop: &str) -> u32 {
        let inherited = INHERITED.contains(&prop);
        if !inherited {
            match own(self.flat, n.index(), prop) {
                None => return DEFAULT,
                Some(v) if v != "inherit" => return n.index() as u32,
                Some(_) => {}
            }
        }
        let mut found = self.found.borrow_mut();
        if !found.contains_key(prop) {
            let fresh = if inherited {
                Sources::Every(vec![UNKNOWN; self.font_size.len()].into_boxed_slice())
            } else {
                Sources::Some(HashMap::new())
            };
            found.insert(prop.into(), fresh);
        }
        let Some(slots) = found.get_mut(prop) else {
            return DEFAULT;
        };
        let mut chain = Vec::new();
        let mut cur = n;
        let at = loop {
            let known = slots.get(cur.index());
            if known != UNKNOWN {
                break known;
            }
            let value = own(self.flat, cur.index(), prop);
            if value.is_some_and(|v| v != "inherit") {
                break cur.index() as u32;
            }
            chain.push(cur);
            // `inherit`, or an inherited property the element does not set: the parent's.
            // The root's parent is no element, whose `_get` gives the default.
            match self.dom.parent(cur) {
                Some(p) if value.is_some() || inherited => cur = p,
                _ => break DEFAULT,
            }
        };
        slots.set(cur.index(), at);
        for c in chain {
            slots.set(c.index(), at);
        }
        at
    }

    /// How many answers of `_get` are kept, over all properties.
    #[cfg(test)]
    fn kept(&self) -> usize {
        self.found.borrow().values().map(Sources::len).sum()
    }

    /// A value worked out from the parent's: the cached one, else each uncached element from
    /// the topmost down, `f` given the parent's value (None at the root).
    fn down<T: Copy>(
        &self,
        cache: &[Cell<Option<T>>],
        n: NodeId,
        mut f: impl FnMut(NodeId, Option<T>) -> T,
    ) -> T {
        if let Some(v) = cache[n.index()].get() {
            return v;
        }
        // The uncached ancestors, nearest first.
        let mut chain = Vec::new();
        let mut parent = None;
        let mut cur = n;
        while let Some(p) = self.dom.parent(cur) {
            if let Some(v) = cache[p.index()].get() {
                parent = Some(v);
                break;
            }
            chain.push(p);
            cur = p;
        }
        for &m in chain.iter().rev() {
            let v = f(m, parent);
            cache[m.index()].set(Some(v));
            parent = Some(v);
        }
        let v = f(n, parent);
        cache[n.index()].set(Some(v));
        v
    }

    /// `Style.fontSize`: the declared `font-size` against the parent's, the root's against
    /// the profile's base size.
    fn font_size(&self, n: NodeId) -> f64 {
        self.down(&self.font_size, n, |m, parent| {
            let base = parent.unwrap_or(self.profile.fbase);
            match self.get(m, "font-size") {
                Some(size) => self.normalize_font_size(size, base),
                None => base,
            }
        })
    }

    /// `normalize_fontsize`.
    fn normalize_font_size(&self, value: &str, base: f64) -> f64 {
        let value = value.replace(['"', '\''], "");
        if value == "inherit" {
            // `value = base`, a number `unit_convert` gives back as it is.
            return if base < 0.0 {
                self.relative_size(false, base)
            } else {
                base
            };
        }
        if let Some(&(_, size)) = self.profile.fnames.iter().find(|(k, _)| *k == value) {
            return size;
        }
        match value.as_str() {
            "smaller" => self.relative_size(false, base),
            "larger" => self.relative_size(true, base),
            _ => match unit_convert(&value, || base, || base, self.profile) {
                Num::Pt(v) if v < 0.0 => self.relative_size(false, base),
                Num::Pt(v) => v,
                Num::Text(_) => base,
            },
        }
    }

    /// `normalize_fontsize('smaller' | 'larger', base)`: the next size of the profile's table,
    /// or `base` ÷ 1.2 (× 1.2) past its end.
    fn relative_size(&self, larger: bool, base: f64) -> f64 {
        // `profile.fsizes`: the keywords' sizes, then `<font size="7">`'s.
        let p = self.profile;
        let sizes: [f64; 8] = std::array::from_fn(|i| p.fnames.get(i).map_or(p.fnums[6], |f| f.1));
        let mut factor = Some(if larger { 1.2 } else { 1.0 / 1.2 });
        let mut result = base;
        if larger {
            for &size in sizes.iter().rev() {
                if base >= size {
                    break;
                }
                factor = None;
                result = size;
            }
        } else {
            for &size in &sizes {
                if base <= size {
                    break;
                }
                factor = None;
                result = size;
            }
        }
        match factor {
            Some(f) => f * base,
            None => result,
        }
    }

    /// `Style.width`.
    fn width(&self, n: NodeId) -> f64 {
        self.down(&self.width, n, |m, parent| {
            self.dimension(
                m,
                "width",
                parent.unwrap_or(self.profile.width_pts),
                self.profile.width_px,
            )
        })
    }

    /// `Style.height`.
    fn height(&self, n: NodeId) -> f64 {
        self.down(&self.height, n, |m, parent| {
            self.dimension(
                m,
                "height",
                parent.unwrap_or(self.profile.height_pts),
                self.profile.height_px,
            )
        })
    }

    /// `Style.width` or `Style.height` of `n` against the parent's, `base`: the attribute
    /// before CSS, `auto` or empty the parent's, one `unit_convert` does not read
    /// `unreadable`; then no more than `max-width` (`max-height`).
    fn dimension(&self, n: NodeId, attr: &str, base: f64, unreadable: f64) -> f64 {
        let size = self.dom.attr(n, attr).or_else(|| self.get(n, attr));
        let mut result = match size {
            None | Some("" | "auto") => base,
            Some(v) => match unit_convert(v, || base, || self.font_size(n), self.profile) {
                Num::Pt(v) => v,
                Num::Text(_) => unreadable,
            },
        };
        let max = if attr == "width" {
            "max-width"
        } else {
            "max-height"
        };
        if let Some(m) = self.get(n, max) {
            let limit = match unit_convert(m, || base, || self.font_size(n), self.profile) {
                Num::Pt(v) => v,
                Num::Text(_) => result,
            };
            // Python's `min(result, limit)`: `limit` only where it is less.
            if limit < result {
                result = limit;
            }
        }
        result
    }

    /// `Style.parent_width`: the parent's width, the root's own.
    fn parent_width(&self, n: NodeId) -> f64 {
        match self.dom.parent(n) {
            Some(p) => self.width(p),
            None => self.width(n),
        }
    }

    /// `Style.lineHeight`: the declared `line-height` (`normal` as `1.2`) against the font
    /// size, else the parent's line height as it is, a number of points or text; the root's
    /// 1.2 × its font size.
    fn line_height(&self, n: NodeId) -> Num<'a> {
        self.down(&self.line_height, n, |m, parent| {
            match self.get(m, "line-height") {
                Some(l) => {
                    let l = if l == "normal" { "1.2" } else { l };
                    match py_float(l) {
                        Some(v) => Num::Pt(v * self.font_size(m)),
                        None => self.convert(l, || self.font_size(m), || self.font_size(m)),
                    }
                }
                None => parent.unwrap_or_else(|| Num::Pt(1.2 * self.font_size(m))),
            }
        })
    }

    /// `Style.color`: `_get('color')` where css_parser reads it as a colour, else `black`.
    fn color(&self, n: NodeId) -> &'a str {
        match self.inherited(n, "color") {
            Ok(v) if !v.is_empty() && is_color(v) => v,
            _ => default_value("color").unwrap_or("black"),
        }
    }

    /// `Style.backgroundColor`: the element's own `background-color` where it is a colour,
    /// else the first colour among the items of its own `background`, else none.
    fn background_color(&self, n: NodeId) -> Option<&'a str> {
        if let Some(v) = self.get(n, "background-color")
            && !v.is_empty()
            && is_color(v)
        {
            return Some(v);
        }
        primitives(self.get(n, "background")?)
            .into_iter()
            .find(|c| is_color(c))
    }
}

/// Element `i`'s own declared value of `prop`.
fn own<'f>(flat: &'f Flat, i: usize, prop: &str) -> Option<&'f str> {
    flat.specified
        .get(i)?
        .iter()
        .find(|(k, _)| k == prop)
        .map(|(_, d)| d.value.as_str())
}

/// `unit_convert(value, base, font, dpi, body_font_size)`: `value` in points. A number is
/// pixels; a number and a unit `UNIT_RE` reads is converted (`%` of `base`, `em` of `font`,
/// `rem` of the profile's base size); anything else comes back as it is. `base` and `font` are
/// asked only where the unit needs them. Calibre's order of operations is kept.
fn unit_convert<'v>(
    value: &'v str,
    base: impl FnOnce() -> f64,
    font: impl FnOnce() -> f64,
    profile: &Profile,
) -> Num<'v> {
    if let Some(v) = py_float(value) {
        return Num::Pt(v * 72.0 / profile.dpi);
    }
    let Some((v, unit)) = unit_number(value) else {
        return Num::Text(value);
    };
    Num::Pt(match unit {
        "%" => (v / 100.0) * base(),
        "px" => v * 72.0 / profile.dpi,
        "in" => v * 72.0,
        "pt" => v,
        "em" => v * font(),
        // "This is a hack for ex since we have no way to know the x-height of the font".
        "ex" | "en" => v * font() * 0.5,
        "pc" => v * 12.0,
        "mm" => v * 2.8346456693,
        "cm" => v * 28.346456693,
        "rem" => v * profile.fbase,
        "q" => v * 0.708661417325,
        _ => return Num::Text(value),
    })
}

/// `UNIT_RE = r'^(-*[0-9]*[.]?[0-9]*)\s*(%|em|ex|en|px|mm|cm|in|pt|pc|rem|q)$'` with its number
/// not empty: (`float()` of the number, 0 where it does not read, as `--5`; the unit).
pub(crate) fn unit_number(value: &str) -> Option<(f64, &str)> {
    let b = value.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] == b'-' {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let (num, rest) = value.split_at(i);
    let unit = rest.trim_start_matches(py_space);
    // `$` also matches before a final line feed.
    let unit = unit.strip_suffix('\n').unwrap_or(unit);
    const UNITS: [&str; 12] = [
        "%", "em", "ex", "en", "px", "mm", "cm", "in", "pt", "pc", "rem", "q",
    ];
    let unit = UNITS.iter().find(|u| **u == unit)?;
    if num.is_empty() {
        return None;
    }
    Some((py_float(num).unwrap_or(0.0), unit))
}

/// Python's `float()` of a string, but for digits outside ASCII and `_` between digits.
pub(crate) fn py_float(s: &str) -> Option<f64> {
    let t = s.trim_matches(py_space);
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    // Rust reads `inf`, `infinity` and `nan` in any case, as Python does; it also reads
    // forms Python does not.
    let ok = unsigned.eq_ignore_ascii_case("inf")
        || unsigned.eq_ignore_ascii_case("infinity")
        || unsigned.eq_ignore_ascii_case("nan")
        || (!unsigned.is_empty()
            && unsigned
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
            && unsigned.bytes().any(|b| b.is_ascii_digit()));
    if !ok {
        return None;
    }
    t.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::css::{DEFAULT_PROFILE, Decl};

    #[test]
    fn names_not_inherited_keep_nothing_per_element() {
        // 100,000 paragraphs under <body>, which declares `x-own: 3px`; every 1,000th
        // paragraph declares `x-own: inherit`.
        let mut dom = crate::dom::parse(
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head/><body/></html>",
        )
        .unwrap();
        let body = dom.children(dom.root())[1];
        let ps: Vec<NodeId> = (0..100_000).map(|_| dom.append_child(body, "p")).collect();
        let n = dom.node_count();
        let decl = |k: &str, v: &str| {
            Arc::new(vec![(
                k.to_owned(),
                Decl {
                    value: v.to_owned(),
                    important: false,
                },
            )])
        };
        let none = Arc::new(Decls::new());
        let mut specified = vec![none; n];
        specified[body.index()] = decl("x-own", "3px");
        let inherit = decl("x-own", "inherit");
        for p in ps.iter().step_by(1_000) {
            specified[p.index()] = Arc::clone(&inherit);
        }
        let flat = Flat {
            specified,
            pseudo: vec![Vec::new(); n],
            class: vec![None; n],
            ua: Vec::new(),
            main: Vec::new(),
        };
        let styles = Styles::new(&dom, &flat, &DEFAULT_PROFILE);
        let last = ps[99_999];
        for i in 0..2_000 {
            let prop = format!("x-unknown-{i}");
            assert!(matches!(
                styles.computed(last, &prop),
                Err(Error::NoDefault { .. })
            ));
        }
        assert_eq!(styles.kept(), 0);
        // Declared or not, a name that is not inherited is answered from the element alone.
        assert_eq!(styles.inherited(body, "x-own").unwrap(), "3px");
        assert!(styles.inherited(last, "x-own").is_err());
        assert_eq!(styles.kept(), 0);
        // `inherit` walks to <body>, keeping the paragraph's answer and <body>'s.
        assert_eq!(styles.inherited(ps[1_000], "x-own").unwrap(), "3px");
        assert_eq!(styles.kept(), 2);
        // An inherited property keeps one answer per element.
        assert_eq!(styles.inherited(last, "font-family").unwrap(), "serif");
        assert_eq!(styles.kept(), 2 + n);
    }

    fn uc(value: &str) -> Num<'_> {
        unit_convert(value, || 200.0, || 10.0, &DEFAULT_PROFILE)
    }

    #[test]
    fn units_convert_as_calibre_converts_them() {
        let cases: [(&str, f64); 16] = [
            ("12", 8.64),
            ("12px", 8.64),
            ("1in", 72.0),
            ("7pt", 7.0),
            ("2em", 20.0),
            ("2ex", 10.0),
            ("2en", 10.0),
            ("1pc", 12.0),
            ("10mm", 28.346456693),
            ("1cm", 28.346456693),
            ("1.5rem", 18.0),
            ("10%", 20.0),
            ("4q", 4.0 * 0.708661417325),
            ("-5px", -3.6),
            ("5 px", 3.6),
            ("--5px", 0.0),
        ];
        for (value, want) in cases {
            assert_eq!(uc(value), Num::Pt(want), "{value}");
        }
        for text in ["foo", "5PX", "px", "1e3px", "auto", "initial", "5px;"] {
            assert_eq!(uc(text), Num::Text(text), "{text}");
        }
        assert!(matches!(uc("nan"), Num::Pt(v) if v.is_nan()));
        assert_eq!(uc("1e999"), Num::Pt(f64::INFINITY));
        // `$` before a final line feed.
        assert_eq!(uc("5px\n"), Num::Pt(3.6));
    }

    #[test]
    fn colours_are_read_as_calibres_validate_color_reads_them() {
        // Each answer read from Calibre 9.18's `validate_color`.
        let cases = [
            ("red", true),
            ("RED", true),
            (" red", false),
            ("red ", false),
            ("#FFF", true),
            ("#ffff", false),
            ("inherit", true),
            ("", false),
            ("rgb(1,2,3)", true),
            ("rgb( 1 , 2 , 3 )", true),
            ("red\n", true),
            ("currentColor", true),
            ("transparent", true),
            ("rgba(1,2,3,0.5)", true),
            ("hsl(1,2%,3%)", true),
            ("0", false),
            ("none", false),
            ("initial", false),
        ];
        for (value, want) in cases {
            assert_eq!(is_color(value), want, "{value:?}");
        }
    }
}
