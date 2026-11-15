// SPDX-License-Identifier: GPL-3.0-only
//! A block's writing direction, unicode-bidi and language: kfxgen's `_bidi`, `_with_bidi` and
//! `_start_end_align` (`converter.py`, at kfxgen `ad38ec8`).
//!
//! `_bidi` walks up from the block for each of its three values and stops at the nearest
//! element that sets it; `_first_strong` reads the text of every `dir="auto"` element on the
//! way. Asked for each block, that is the depth times the blocks, and a `dir="auto"` section's
//! text read again for every block in it. Here each element's inherited values are worked out
//! once, from its parent's ([`BlockBidi`]), and each `_first_strong` answer is kept, so a file
//! costs one pass of each.

use super::ItemError;
use super::bidi::first_strong;
use super::inline::keyword;
use super::resolver::Resolver;
use crate::dom::{Dom, NodeId};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// The namespace of the `xml:` prefix.
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// `_UNICODE_BIDI`.
fn unicode_bidi(value: &str) -> Option<&'static str> {
    Some(match value {
        "embed" => "embed",
        "isolate" => "isolate",
        "bidi-override" => "bidi-override",
        "isolate-override" => "isolate-override",
        "plaintext" => "plaintext",
        "normal" => "normal",
        _ => return None,
    })
}

/// What an element has from itself or its nearest ancestor setting each: its direction,
/// its unicode-bidi, and the element whose `xml:lang` or `lang` it takes.
#[derive(Clone, Copy, Default)]
struct Inherited {
    direction: Option<&'static str>,
    bidi: Option<&'static str>,
    lang: Option<NodeId>,
}

/// What one element sets itself, of the three.
struct Own {
    direction: Option<&'static str>,
    bidi: Option<&'static str>,
    lang: bool,
}

/// `_bidi` over one file: each element's inherited values, and each `_first_strong`, kept.
pub(super) struct BlockBidi {
    inherited: Vec<Option<Inherited>>,
    strong: Vec<Option<&'static str>>,
}

/// `(e.get(_XML_LANG) or e.get("lang"))`, when it is not blank after `strip()`.
fn lang_attr(dom: &Dom, e: NodeId) -> Option<&str> {
    dom.attr_ns(e, XML_NS, "lang")
        .filter(|l| !l.is_empty())
        .or_else(|| dom.attr(e, "lang"))
        .filter(|l| !py::strip(l).is_empty())
}

impl BlockBidi {
    pub(super) fn new(dom: &Dom) -> BlockBidi {
        BlockBidi {
            inherited: vec![None; dom.node_count()],
            strong: vec![None; dom.node_count()],
        }
    }

    fn first_strong(&mut self, dom: &Dom, e: NodeId) -> &'static str {
        self.strong[e.index()].get_or_insert_with(|| first_strong(dom, e))
    }

    /// One turn of `_bidi`'s loop on `e`: what it sets of the three. Its style is resolved
    /// whether or not kfxgen's loop still needs it there (resolving reads nothing it keeps).
    fn own(&mut self, dom: &Dom, resolver: &Resolver, e: NodeId) -> Result<Own, ItemError> {
        // `(e.get("dir") or "").strip().lower()`, compared with three ASCII words: no
        // character outside ASCII lowers to one of their letters.
        let attr = py::strip(dom.attr(e, "dir").unwrap_or(""));
        let is = |w: &str| attr.eq_ignore_ascii_case(w);
        let has_dir = is("rtl") || is("ltr") || is("auto");
        let css = resolver.resolve(e)?;
        let own = keyword(css.as_ref(), "own-direction")?;
        let direction = match own.as_str() {
            "rtl" => Some("rtl"),
            "ltr" => Some("ltr"),
            _ if is("auto") => Some(self.first_strong(dom, e)),
            _ if is("rtl") => Some("rtl"),
            _ if is("ltr") => Some("ltr"),
            _ => None,
        };
        let bidi = unicode_bidi(&keyword(css.as_ref(), "own-unicode-bidi")?)
            .or(has_dir.then_some("isolate"));
        Ok(Own {
            direction,
            bidi,
            lang: lang_attr(dom, e).is_some(),
        })
    }

    /// The three values `n` has, from itself or the nearest ancestor setting each. Worked out
    /// top-down from the nearest ancestor already known, so each element is read once.
    fn inherited(
        &mut self,
        dom: &Dom,
        resolver: &Resolver,
        n: NodeId,
    ) -> Result<Inherited, ItemError> {
        if let Some(known) = self.inherited[n.index()] {
            return Ok(known);
        }
        let mut chain = vec![n];
        let mut above = Inherited::default();
        while let Some(p) = dom.parent(chain[chain.len() - 1]) {
            if let Some(known) = self.inherited[p.index()] {
                above = known;
                break;
            }
            chain.push(p);
        }
        for &e in chain.iter().rev() {
            let own = self.own(dom, resolver, e)?;
            above = Inherited {
                direction: own.direction.or(above.direction),
                bidi: own.bidi.or(above.bidi),
                lang: if own.lang { Some(e) } else { above.lang },
            };
            self.inherited[e.index()] = Some(above);
        }
        Ok(above)
    }

    /// `_bidi(elem, style_resolver)`: `{"lang", "unicode_bidi", "direction", "set_here"}`,
    /// each only when set, in that order (kfxgen's insertion order); empty for none. The
    /// language copied is charged.
    pub(super) fn bidi(
        &mut self,
        dom: &Dom,
        resolver: &Resolver,
        elem: NodeId,
    ) -> Result<Vec<(PyValue, PyValue)>, ItemError> {
        let all = self.inherited(dom, resolver, elem)?;
        let own = self.own(dom, resolver, elem)?;
        let mut out = Vec::new();
        if let Some(e) = all.lang {
            let lang = py::strip(lang_attr(dom, e).unwrap_or(""));
            resolver.charge(lang.len())?;
            out.push((s("lang"), s(&lang.to_lowercase())));
        }
        if let Some(b) = all.bidi {
            out.push((s("unicode_bidi"), s(b)));
        }
        if let Some(d) = all.direction {
            out.push((s("direction"), s(d)));
        }
        // `here`, in the order kfxgen's first turn appends them.
        let here: Vec<PyValue> = [
            (own.direction.is_some(), "direction"),
            (own.bidi.is_some(), "unicode_bidi"),
            (own.lang, "lang"),
        ]
        .into_iter()
        .filter(|(set, _)| *set)
        .map(|(_, k)| s(k))
        .collect();
        if !here.is_empty() {
            out.push((s("set_here"), PyValue::Tuple(here)));
        }
        Ok(out)
    }

    /// `_with_bidi(block_style, elem, style_resolver, own)`.
    pub(super) fn with_bidi(
        &mut self,
        dom: &Dom,
        resolver: &Resolver,
        block_style: PyValue,
        elem: NodeId,
        own: bool,
    ) -> Result<PyValue, ItemError> {
        let mut bidi = self.bidi(dom, resolver, elem)?;
        if !own {
            bidi.retain(|(k, _)| k.as_str() != Some("set_here"));
        }
        if bidi.is_empty() {
            return Ok(block_style);
        }
        let mut style = match block_style {
            PyValue::Dict(pairs) => pairs,
            _ => Vec::new(),
        };
        dict_set(&mut style, "bidi", PyValue::Dict(bidi));
        Ok(PyValue::Dict(style))
    }
}

fn s(v: &str) -> PyValue {
    PyValue::Str(v.to_owned())
}

/// `d[key] = value` on a dict's pairs: in place when the key is there, else at the end.
pub(super) fn dict_set(pairs: &mut Vec<(PyValue, PyValue)>, key: &str, value: PyValue) {
    match pairs.iter_mut().find(|(k, _)| k.as_str() == Some(key)) {
        Some((_, v)) => *v = value,
        None => pairs.push((s(key), value)),
    }
}

/// `_start_end_align(block_style, css)`, given `str(css.get("text-align") or
/// "").strip().lower()`: `start` is right and `end` left in a right-to-left paragraph.
pub(super) fn start_end_align(block_style: PyValue, raw: &str) -> PyValue {
    if !matches!(raw, "start" | "end") {
        return block_style;
    }
    let PyValue::Dict(mut style) = block_style else {
        // None: `not block_style`.
        return block_style;
    };
    if style.is_empty() {
        return PyValue::Dict(style);
    }
    let rtl = style.iter().any(|(k, v)| {
        k.as_str() == Some("bidi") && v.get("direction").and_then(PyValue::as_str) == Some("rtl")
    });
    if rtl {
        dict_set(
            &mut style,
            "align",
            s(if raw == "start" { "right" } else { "left" }),
        );
    }
    PyValue::Dict(style)
}
