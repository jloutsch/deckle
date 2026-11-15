// SPDX-License-Identifier: GPL-3.0-only
//! Boxes, at kfxgen `ad38ec8`: `_make_box`, which replaces the blocks a bordered, filled,
//! sized or turned container made with one box block holding them, and the module helpers it
//! calls (`_box_width`, `_box_width_px`, `_box_padding`, `_BOX_TAGS`, `_SIZED_BOX_TAGS`,
//! `_CSS_PX`). The box block itself is the one `_list_box` writes too ([`Walker::box_block`]).
//!
//! What a box takes part in once made is the spacing code's (`spacing.rs`:
//! `_note_box_margins`, the box branches of `_carry_container_margins`, `_note_padding` and
//! `_note_margins`, the box part of `_space_between`) and the anchor code's (`anchors.rs`).
//!
//! Every box block carries a `box` dict and a `blocks` list; the spacing code relies on it
//! where kfxgen would raise `KeyError` without them.
//!
//! Boxes nest no deeper than the elements that make them: each is made at its element's end,
//! from the blocks already made, so a box's kids are moved into it, never walked again. Each
//! box copies its kids' texts and ids (kfxgen's model holds them so), charged to the file's
//! budget.

use super::rules::{BORDER_SIDES, BorderSide, css_border};
use super::spacing::{Pad, font_ratio, length_py, margin_em, padding_em, page_share, truthy};
use super::walk::{HORIZONTAL, VERTICAL, Walker, has_words, text_field};
use super::{Block, Field, dedupe, dict_set, s, strs};
use crate::convert::inline::text_of;
use crate::convert::inline_style::parse_css_length;
use crate::convert::pictures::float_side;
use crate::convert::resolver::Resolved;
use crate::convert::{ItemError, Raised};
use crate::css::TEXT_UNIT;
use crate::dom::NodeId;
use crate::generate::images::{IMG_TOKEN, WHOLE_TOKEN};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `em / ratio` in `_box_padding` with a font size so small its ratio is 0. Not reached: the
/// resolver rounds a font size to three places, so a positive one is at least 0.001.
const ZERO_RATIO: Raised = Raised {
    exception: "ZeroDivisionError",
    note: "_box_padding: a padding divided by a font ratio of 0",
};

/// What `_make_box` reads in a block's text, worked out the first time it is asked: whether it
/// has words (`text.strip()`), a picture token (`_IMG_TOKEN_RE.search`), and whether it is
/// one floated picture (`_IMG_TOKEN_RE.fullmatch` with a float). A block's text never changes
/// once it is made.
#[derive(Clone, Copy)]
pub(super) struct Seen {
    words: bool,
    picture: bool,
    floated: bool,
}

/// `_BOX_TAGS`: containers drawn as a box when they have a border or a fill.
pub(super) fn is_box_tag(local: &str) -> bool {
    matches!(
        local,
        "div" | "aside" | "section" | "blockquote" | "article" | "p" | "pre"
    )
}

/// `_CSS_PX`: CSS px per unit, for a wrapper's width in a length.
pub(super) fn css_px(unit: &str) -> Option<f64> {
    match unit {
        "$319" => Some(1.0),
        "$308" | "$505" => Some(16.0),
        "$318" => Some(4.0 / 3.0),
        "$316" => Some(96.0 / 25.4),
        _ => None,
    }
}

/// The element's `width` as `parse_css_length(str(css.get("width") or ""))` reads it.
fn width_length(css: Option<&Resolved>) -> Result<Option<(String, &'static str)>, ItemError> {
    let raw = match css {
        Some(css) => text_of(css.get("width")?).into_owned(),
        None => String::new(),
    };
    Ok(parse_css_length(&raw, false)?)
}

/// `_box_width(css)`: a wrapper's CSS width as a share of the page, in %, or None for none,
/// `auto`, the full width or anything unreadable.
fn box_width(css: Option<&Resolved>) -> Result<Option<f64>, ItemError> {
    let share = match width_length(css)? {
        Some(width) => page_share(&length_py(&Some(width)))?,
        None => 0.0,
    };
    Ok((share > 0.0 && share < 100.0).then_some(share))
}

/// `_box_width_px(css)`: a wrapper's CSS width in CSS px when it is a length, else None.
fn box_width_px(css: Option<&Resolved>) -> Result<Option<f64>, ItemError> {
    let Some((mag, unit)) = width_length(css)? else {
        return Ok(None);
    };
    let Some(px) = css_px(unit) else {
        return Ok(None);
    };
    // `float(width[0])` of a magnitude `parse_css_length` wrote: always a number.
    Ok(Some(py::py_float(&mag).unwrap_or(0.0) * px))
}

/// `_box_padding(css)`: a box's padding per side, in its own ems or `("%", n)`, or None when
/// it has none.
fn box_padding(css: Option<&Resolved>) -> Result<PyValue, ItemError> {
    let Some(css) = css else {
        return Ok(PyValue::None);
    };
    // `if not sides`; the resolver always writes a tuple of four.
    let sides = match css.get("padding-sides")? {
        Some(PyValue::Tuple(v) | PyValue::List(v)) if !v.is_empty() => v,
        _ => return Ok(PyValue::None),
    };
    let ratio = font_ratio(Some(css));
    let mut out = Vec::with_capacity(4);
    for (side, value) in BORDER_SIDES.iter().zip(sides) {
        // `em = _padding_em(value, ratio) or 0.0`, then `em / ratio` unless a percentage.
        let em = match padding_em(&text_of(Some(value)), ratio)? {
            Some(Pad::Pct(n)) => {
                out.push((s(side), PyValue::Tuple(vec![s("%"), PyValue::Float(n)])));
                continue;
            }
            Some(Pad::Em(em)) if truthy(em) => em,
            _ => 0.0,
        };
        if ratio == 0.0 {
            return Err(ZERO_RATIO.into());
        }
        out.push((s(side), PyValue::Float(em / ratio)));
    }
    // `all(v == 0 for v in out.values())`: a percentage is never 0.
    if out
        .iter()
        .all(|(_, v)| matches!(v, PyValue::Float(f) if *f == 0.0))
    {
        return Ok(PyValue::None);
    }
    Ok(PyValue::Dict(out))
}

/// `_css_border`'s dict as the model holds it: `{side: (style, pt, colour)}`, or None.
pub(super) fn border_py(border: &Option<Vec<BorderSide>>) -> PyValue {
    match border {
        Some(sides) => PyValue::Dict(
            sides
                .iter()
                .map(|b| {
                    (
                        s(b.side),
                        PyValue::Tuple(vec![
                            s(&b.kind),
                            PyValue::Float(b.pt),
                            b.colour.map_or(PyValue::None, PyValue::Int),
                        ]),
                    )
                })
                .collect(),
        ),
        None => PyValue::None,
    }
}

/// `x or None` of a float.
fn share_or_none(x: f64) -> PyValue {
    if truthy(x) {
        PyValue::Float(x)
    } else {
        PyValue::None
    }
}

impl Walker<'_, '_, '_, '_> {
    /// One unit charged for each of `n` kids looked at.
    fn step(&self, n: usize) -> Result<(), ItemError> {
        Ok(self.resolver.charge(TEXT_UNIT.saturating_mul(n))?)
    }

    /// What `_make_box` reads in `self.blocks[i]`'s text, read once per block.
    fn seen(&mut self, i: usize) -> Seen {
        let b = &self.blocks[i];
        *self.seen.entry(b.id()).or_insert_with(|| {
            let text = text_field(b);
            Seen {
                words: has_words(text),
                picture: IMG_TOKEN.is_match(text),
                floated: WHOLE_TOKEN
                    .captures(text)
                    .and_then(|m| m.get(4))
                    .is_some_and(|g| !g.as_str().is_empty()),
            }
        })
    }

    /// A box block holding `kids`, kfxgen's dict for `_make_box` and `_list_box` alike:
    /// `break_before` if any kid has it, `clear` when given, the kids' texts joined, their
    /// ids, and `bx` as its `box`. Each box copies its kids' texts and ids once more, so a box
    /// nested `d` deep copies them `d` times, as kfxgen's model holds them: each copy is
    /// charged.
    pub(super) fn box_block(
        &mut self,
        kids: Vec<Block>,
        clear: Option<PyValue>,
        font_px: PyValue,
        bx: Vec<(PyValue, PyValue)>,
    ) -> Result<Block, ItemError> {
        let mut ids = Vec::new();
        for b in &kids {
            if let Some(PyValue::List(items)) = b.get("anchor_ids") {
                ids.extend(items.iter().filter_map(|i| i.as_str().map(str::to_owned)));
            }
        }
        let ids = dedupe(ids);
        let texts: Vec<&str> = kids
            .iter()
            .filter_map(|b| b.get("text").and_then(PyValue::as_str))
            .filter(|t| !t.is_empty())
            .collect();
        let bytes = texts.iter().map(|t| t.len() + 2).sum::<usize>()
            + 2 * ids.iter().map(String::len).sum::<usize>();
        self.resolver.charge(bytes)?;
        let text = texts.join("\n\n");
        let break_before = kids
            .iter()
            .any(|b| b.get("break_before").is_some_and(PyValue::is_truthy));
        let offsets = ids.iter().map(|i| (s(i), PyValue::Int(0))).collect();
        let mut fields = vec![
            ("type", Field::Py(s("box"))),
            ("break_before", Field::Py(PyValue::Bool(break_before))),
        ];
        if let Some(clear) = clear {
            fields.push(("clear", Field::Py(clear)));
        }
        fields.extend([
            ("text", Field::Py(PyValue::Str(text))),
            ("spans", Field::Py(PyValue::List(Vec::new()))),
            ("block_style", Field::Py(PyValue::None)),
            ("font_px", Field::Py(font_px)),
            ("blocks", Field::Blocks(kids)),
            ("anchor_ids", Field::Py(strs(&ids))),
            ("anchor_offsets", Field::Py(PyValue::Dict(offsets))),
            ("box", Field::Py(PyValue::Dict(bx))),
        ]);
        Ok(self.new_block(fields))
    }

    /// `_make_box(elem, start, leaf)`: replaces `blocks[start:]`, what `elem` made, with one
    /// box block holding them when `elem` is drawn as a box (a border or fill on a box tag, a
    /// width round a picture, or a block turned against the text round it), and notes the
    /// box's margins. `leaf`: `elem` is itself the one paragraph; its side margins move to
    /// the box. Returns whether it made a box.
    pub(super) fn make_box(
        &mut self,
        elem: NodeId,
        start: usize,
        leaf: bool,
    ) -> Result<bool, ItemError> {
        let dom = self.dom;
        let tag = dom.tag(elem);
        let horizontal = dom.attr(elem, HORIZONTAL).is_some();
        let vertical = dom.attr(elem, VERTICAL).is_some();
        let turned = horizontal || vertical;
        if !(is_box_tag(tag) || tag == "figure") && !turned {
            return Ok(false);
        }
        // `style_resolver(elem) or {}`: every read below treats no style as `{}`.
        let css = self.resolve(elem)?;
        let css = css.as_ref();
        let border = css_border(css)?;
        // `_box_background(css)`: the resolved style's background values are the element's
        // own (`st.get`), which the resolver's fill reads.
        let fill = match css {
            Some(_) => self
                .resolver
                .box_background(elem)?
                .map_err(ItemError::Raised)?,
            None => None,
        };
        let (border, fill) = if is_box_tag(tag) {
            (border, fill)
        } else {
            (None, None)
        };
        let start = start.min(self.blocks.len());
        let end = self.blocks.len();
        let mut width = box_width(css)?;
        let side = float_side(css)?;
        // Kids left flat (no box made round them) are looked at again by each container
        // round them that gets this far: each look costs a unit, and what is read in a kid's
        // text is worked out once per block (`self.seen`).
        if width.is_some() {
            self.step(end - start)?;
            if !(start..end).any(|i| self.seen(i).picture) {
                width = None;
            }
        }
        if width.is_some() && side.is_some() && end - start == 1 && self.seen(start).floated {
            // Only a floated picture: the floated picture, not a box.
            return Ok(false);
        }
        if border.is_none() && fill.is_none() && width.is_none() && !turned {
            return Ok(false);
        }
        // `any(...)` stops at the first kid with content: charge the kids it visits.
        let visited = (start..end)
            .position(|i| {
                matches!(self.blocks[i].kind(), Some("table" | "box")) || self.seen(i).words
            })
            .map(|at| at + 1);
        self.step(visited.unwrap_or(end - start))?;
        if visited.is_none() {
            return Ok(false);
        }
        if leaf {
            // The paragraph's side margins are the box's now, and its own `dir` and `lang`
            // are set on the box only. kfxgen copies the style first; the block owns it here.
            let b = &mut self.blocks[start];
            match b.get_mut("block_style") {
                Some(PyValue::Dict(style)) => {
                    dict_set(style, "margin_left", PyValue::None);
                    dict_set(style, "margin_right", PyValue::None);
                    if let Some((_, PyValue::Dict(bidi))) =
                        style.iter_mut().find(|(k, _)| k.as_str() == Some("bidi"))
                    {
                        bidi.retain(|(k, _)| k.as_str() != Some("set_here"));
                    }
                }
                _ => b.set(
                    "block_style",
                    PyValue::Dict(vec![
                        (s("margin_left"), PyValue::None),
                        (s("margin_right"), PyValue::None),
                    ]),
                ),
            }
        }
        // A contents entry naming a block past the first keeps the blocks flat. A leaf's box
        // holds one block, so this never refuses it. Kids left flat are looked at again by
        // each container round them, so each kid this loop visits costs a unit, and the ids
        // it reads cost their bytes, charged once after the loop.
        let toc = self.opts.toc_targets;
        if !toc.is_empty() {
            let (mut looked, mut bytes, mut named) = (0usize, 0usize, false);
            for b in self.blocks[start..].iter().skip(1) {
                looked += 1;
                let ids = b
                    .get("anchor_ids")
                    .and_then(PyValue::as_list)
                    .unwrap_or(&[]);
                for a in ids.iter().filter_map(PyValue::as_str) {
                    bytes = bytes.saturating_add(a.len());
                }
                if ids
                    .iter()
                    .filter_map(PyValue::as_str)
                    .any(|a| toc.contains(a))
                {
                    named = true;
                    break;
                }
            }
            self.resolver
                .charge(TEXT_UNIT.saturating_mul(looked).saturating_add(bytes))?;
            if named {
                return Ok(false);
            }
        }
        let side_margin = |key: &str| -> Result<Option<(String, &'static str)>, ItemError> {
            let raw = match css {
                Some(css) => text_of(css.get(key)?).into_owned(),
                None => String::new(),
            };
            Ok(parse_css_length(&raw, false)?)
        };
        let (left, right) = (side_margin("margin-left")?, side_margin("margin-right")?);
        let auto = |key: &str| -> Result<bool, ItemError> {
            // `str(css.get(key)).strip() == "auto"`: `str(None)` is not "auto" either.
            Ok(match css {
                Some(css) => py::strip(&text_of(css.get(key)?)) == "auto",
                None => false,
            })
        };
        let font_px = css
            .and_then(Resolved::font_size)
            .cloned()
            .unwrap_or(PyValue::None);
        // The box's values, in kfxgen's order: what can raise raises before the kids' texts
        // are copied (and charged).
        let padding = box_padding(css)?;
        let (margin_left, margin_right) = if vertical {
            (PyValue::None, PyValue::None)
        } else {
            (
                share_or_none(page_share(&length_py(&left))?),
                share_or_none(page_share(&length_py(&right))?),
            )
        };
        let width_px = match width {
            Some(_) => box_width_px(css)?.map_or(PyValue::None, PyValue::Float),
            None => PyValue::None,
        };
        let centred = width.is_some() && auto("margin-left")? && auto("margin-right")?;
        let bidi = self.bidi.bidi(dom, self.resolver, elem)?;
        let mut bx = vec![
            (s("border"), border_py(&border)),
            (s("background"), fill.map_or(PyValue::None, PyValue::Int)),
            (s("padding"), padding),
            (s("font_px"), font_px.clone()),
            (s("margin_left"), margin_left),
            (s("margin_right"), margin_right),
            (s("width"), width.map_or(PyValue::None, PyValue::Float)),
            (s("width_px"), width_px),
            (
                s("float"),
                match (width, side) {
                    (Some(_), Some(side)) => s(side),
                    _ => PyValue::None,
                },
            ),
            (s("centred"), PyValue::Bool(centred)),
            (s("bidi"), PyValue::Dict(bidi)),
        ];
        if horizontal {
            bx.push((s("horizontal"), PyValue::Bool(true)));
        }
        if vertical {
            // A vertical block's right and left are its space before and after, in ems.
            bx.push((s("vertical"), PyValue::Bool(true)));
            bx.push((s("start"), PyValue::Float(margin_em(css, "right", 1.0)?)));
            bx.push((s("end"), PyValue::Float(margin_em(css, "left", 1.0)?)));
        }
        let mut kids = self.blocks.split_off(start);
        // A clear set on the container reached its first block; it is the box's.
        let clear = kids
            .iter_mut()
            .find(|b| b.kind() != Some("space"))
            .and_then(|b| b.remove("clear"))
            .filter(PyValue::is_truthy);
        let block = self.box_block(kids, clear, font_px, bx)?;
        self.blocks.push(block);
        self.note_box_margins(start, css)?;
        Ok(true)
    }
}

/// Expected values printed by the pinned functions (`_box_width`, `_box_width_px`,
/// `_box_padding`, `_margin_em`, copied out of `converter.py` at `ad38ec8` with `ast` and run
/// under Python 3 with its `inline_style.py` and `page_scale.py`; no Calibre).
#[cfg(test)]
mod tests {
    use super::super::rules::tests::repr;
    use super::*;
    use crate::convert::resolver::TextBudget;
    use crate::generate::page_scale::{HORIZONTAL_PT, VERTICAL_PT, page_width};

    fn opt(x: Option<f64>) -> String {
        x.map_or("None".into(), py::repr_float)
    }

    #[test]
    fn css_px_is_kfxgen_s_table() {
        assert_eq!(css_px("$319"), Some(1.0));
        assert_eq!(css_px("$308"), Some(16.0));
        assert_eq!(css_px("$505"), Some(16.0));
        // `4 / 3` and `96 / 25.4`.
        assert_eq!(
            py::repr_float(css_px("$318").unwrap()),
            "1.3333333333333333"
        );
        assert_eq!(
            py::repr_float(css_px("$316").unwrap()),
            "3.7795275590551185"
        );
        assert_eq!(css_px("$314"), None);
    }

    /// `_box_width(css)` and `_box_width_px(css)` on a width, on both pages.
    #[test]
    fn widths_are_read_as_kfxgen_reads_them() {
        let budget = TextBudget::new();
        let cases: [(&str, [&str; 2], [&str; 2]); 19] = [
            ("", ["None", "None"], ["None", "None"]),
            ("auto", ["None", "None"], ["None", "None"]),
            ("50%", ["50.0", "50.0"], ["None", "None"]),
            ("100%", ["None", "None"], ["None", "None"]),
            ("0%", ["None", "None"], ["None", "None"]),
            ("99.9%", ["99.9", "99.9"], ["None", "None"]),
            ("300px", ["35.15625", "28.125"], ["300.0", "300.0"]),
            ("1000px", ["None", "93.75"], ["1000.0", "1000.0"]),
            ("10em", ["31.25", "25.0"], ["160.0", "160.0"]),
            ("2.5in", ["46.875", "37.5"], ["240.0", "240.0"]),
            (
                "3cm",
                ["22.145669291338585", "17.716535433070867"],
                ["113.38582677165356", "113.38582677165356"],
            ),
            ("12pt", ["3.125", "2.5"], ["16.0", "16.0"]),
            ("-5%", ["None", "None"], ["None", "None"]),
            ("1e2%", ["None", "None"], ["None", "None"]),
            ("20rem", ["62.5", "50.0"], ["320.0", "320.0"]),
            ("5ex", ["6.875000000000001", "5.5"], ["35.2", "35.2"]),
            (
                "40mm",
                ["29.52755905511811", "23.62204724409449"],
                ["151.18110236220474", "151.18110236220474"],
            ),
            ("7vw", ["None", "None"], ["None", "None"]),
            (".5%", ["0.5", "0.5"], ["None", "None"]),
        ];
        for (width, shares, px) in cases {
            let css = Resolved::of(vec![("width", s(width))], &budget);
            for (i, pt) in [HORIZONTAL_PT, VERTICAL_PT].into_iter().enumerate() {
                let _page = page_width(pt);
                assert_eq!(
                    opt(box_width(Some(&css)).unwrap()),
                    shares[i],
                    "{width} at {pt}"
                );
                assert_eq!(
                    opt(box_width_px(Some(&css)).unwrap()),
                    px[i],
                    "{width} at {pt}"
                );
            }
        }
        assert_eq!(box_width(None).unwrap(), None);
        assert_eq!(box_width_px(None).unwrap(), None);
    }

    /// Padding sides (None for a side calibre leaves unset), a font size, and the `repr` of
    /// what `_box_padding` gives.
    type PaddingCase = (&'static [Option<&'static str>], Option<f64>, &'static str);

    /// `repr(_box_padding(css))` on padding sides and a font size.
    #[test]
    fn paddings_are_read_as_kfxgen_reads_them() {
        let budget = TextBudget::new();
        let side = |v: Option<&str>| v.map_or(PyValue::None, s);
        let cases: [PaddingCase; 9] = [
            (
                &[Some("1em"), Some("2px"), Some("3pt"), Some("4%")],
                Some(16.0),
                "{'top': 1.0, 'right': 0.075, 'bottom': 0.25, 'left': ('%', 4.0)}",
            ),
            (
                &[Some("1em"), Some("2px"), Some("3pt"), Some("4%")],
                Some(13.28),
                "{'top': 1.0, 'right': 0.09036144578313253, 'bottom': 0.30120481927710846, \
                 'left': ('%', 4.0)}",
            ),
            (
                &[Some("0"), Some("0px"), Some("0em"), None],
                Some(16.0),
                "None",
            ),
            (
                &[Some("0"), Some("0"), Some("0"), Some("0")],
                Some(20.0),
                "None",
            ),
            (
                &[Some("1.5em"), Some(""), Some("-1px"), Some("auto")],
                Some(24.0),
                "{'top': 1.5, 'right': 0.0, 'bottom': 0.0, 'left': 0.0}",
            ),
            (
                &[Some("0"), Some("0"), Some("0"), Some("5%")],
                Some(16.0),
                "{'top': 0.0, 'right': 0.0, 'bottom': 0.0, 'left': ('%', 5.0)}",
            ),
            (
                &[Some("2rem"), Some("1PX"), Some("0.5em"), Some("10px")],
                None,
                "{'top': 2.0, 'right': 0.0, 'bottom': 0.5, 'left': 0.375}",
            ),
            (&[Some("12px")], Some(16.0), "{'top': 0.44999999999999996}"),
            (&[], Some(16.0), "None"),
        ];
        for (sides, size, want) in cases {
            let mut pairs = vec![(
                "padding-sides",
                PyValue::Tuple(sides.iter().map(|v| side(*v)).collect()),
            )];
            if let Some(size) = size {
                pairs.push(("font-size", PyValue::Float(size)));
            }
            let css = Resolved::of(pairs, &budget);
            assert_eq!(repr(&box_padding(Some(&css)).unwrap()), want, "{sides:?}");
        }
        assert_eq!(repr(&box_padding(None).unwrap()), "None");
    }

    /// `_margin_em(css, side)` on a side margin, as a vertical box's `start` and `end` read it.
    #[test]
    fn side_margins_in_ems_are_kfxgen_s() {
        let budget = TextBudget::new();
        let cases = [
            ("left", "2em", 16.0, ["2.0", "2.0"]),
            ("right", "-1.5em", 20.0, ["-1.875", "-1.875"]),
            ("left", "10%", 16.0, ["3.2", "4.0"]),
            (
                "right",
                "3px",
                16.0,
                ["0.11249999999999999", "0.11249999999999999"],
            ),
            ("left", "-0", 16.0, ["-0.0", "-0.0"]),
            ("right", "auto", 16.0, ["0.0", "0.0"]),
        ];
        for (side, value, size, want) in cases {
            let key = if side == "left" {
                "margin-left"
            } else {
                "margin-right"
            };
            let css = Resolved::of(
                vec![(key, s(value)), ("font-size", PyValue::Float(size))],
                &budget,
            );
            for (i, pt) in [HORIZONTAL_PT, VERTICAL_PT].into_iter().enumerate() {
                let _page = page_width(pt);
                let x = margin_em(Some(&css), side, 1.0).unwrap();
                assert_eq!(py::repr_float(x), want[i], "{side} {value} at {pt}");
            }
        }
    }
}
