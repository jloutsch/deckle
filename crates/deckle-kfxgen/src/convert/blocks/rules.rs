// SPDX-License-Identifier: GPL-3.0-only
//! Rules and the borders a rule or a box draws, at kfxgen `ad38ec8`: `_emit_rule` (an `<hr>`),
//! `_rule_look`, `_float_or_none`, and `_css_border` with `_border_colour`, which `_make_box`
//! reads too.
//!
//! `_box_background` and `_background_parts` are the resolver's
//! ([`Resolver::box_background`](crate::convert::resolver::Resolver::box_background)): its
//! `behind` needs a box's fill while it resolves, so both live there, and a rule's fill is read
//! through it.
//!
//! Work is linear in the lengths of the values read: four border sides, two margins, a width,
//! a height and four declared border widths, each parsed once by Task 7's parsers, and the
//! background through the resolver's charging accessor.

use super::spacing::{font_ratio, hidden_rule_em, margin_em};
use super::walk::{Walker, unit_percent};
use super::{Field, s};
use crate::convert::ItemError;
use crate::convert::inline::{own_anchor_ids, text_of};
use crate::convert::inline_style::{css_rgb, is_transparent, parse_css_length};
use crate::convert::resolver::Resolved;
use crate::dom::NodeId;
use crate::generate::py;
use crate::pyvalue::{PyValue, truthy};

/// `_BORDER_SIDES`.
pub(super) const BORDER_SIDES: [&str; 4] = ["top", "right", "bottom", "left"];
/// `_PREVIEWER_PT_PER_CALIBRE_PT`: Previewer writes a border at 0.45pt a pixel, calibre
/// computes it at 0.72pt.
const PREVIEWER_PT_PER_CALIBRE_PT: f64 = 0.625;
/// `_RULE_SPACE_EM`: an `<hr>`'s margins in calibre's own sheet, above and below.
const RULE_SPACE_EM: f64 = 0.5;
/// `_UA_RULE_SIDE`: calibre's own `<hr>` border side, 1px inset in the text colour.
const UA_RULE_SIDE: (&str, f64, &str) = ("inset", 0.72, "currentcolor");
/// `_UNIT_TO_PCT`: the units a side margin is turned into a share of the page from.
const UNIT_TO_PCT: [&str; 6] = ["$308", "$505", "$319", "$318", "$316", "$314"];

/// One drawn side of `_css_border`'s dict: `(style, width in Previewer's points, ARGB colour
/// or None)`, keyed by its side.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BorderSide {
    pub(super) side: &'static str,
    pub(super) kind: String,
    pub(super) pt: f64,
    pub(super) colour: Option<i64>,
}

/// `_css_border(css)`: the drawn sides of the element's computed border, in `_BORDER_SIDES`
/// order, or None when no side is drawn. A colour is read as `_border_colour` reads it, which
/// can raise.
pub(super) fn css_border(css: Option<&Resolved>) -> Result<Option<Vec<BorderSide>>, ItemError> {
    let Some(css) = css else {
        return Ok(None);
    };
    let Some(PyValue::Tuple(sides)) = css.get("border-sides")? else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for (side, value) in BORDER_SIDES.into_iter().zip(sides.iter()) {
        let PyValue::Tuple(parts) = value else {
            continue;
        };
        let get = |i: usize| parts.get(i).unwrap_or(&PyValue::None);
        let kind = text_of(Some(get(0)));
        let kind = if kind.is_empty() {
            "none".to_owned()
        } else {
            kind.to_lowercase()
        };
        let colour = text_of(Some(get(2)));
        if matches!(kind.as_str(), "none" | "hidden") || is_transparent(&colour)? {
            continue;
        }
        let pt = match get(1) {
            PyValue::Float(f) => Some(*f),
            PyValue::Int(i) => Some(*i as f64),
            PyValue::Str(v) => py::py_float(v),
            _ => None,
        };
        let pt = match pt {
            Some(pt) => pt,
            None => match text_of(Some(get(1))).to_lowercase().as_str() {
                "thin" => 0.72,
                "medium" => 3.0 * 0.72,
                "thick" => 5.0 * 0.72,
                _ => continue,
            },
        };
        // False for NaN, as Python's: such a side is drawn.
        if pt <= 0.0 {
            continue;
        }
        // `_border_colour`: the ARGB integer, None for black or unreadable.
        let colour = css_rgb(&colour)?
            .filter(|rgb| *rgb != 0)
            .map(|rgb| 0xFF00_0000 | rgb);
        out.push(BorderSide {
            side,
            kind,
            pt: py::round(pt * PREVIEWER_PT_PER_CALIBRE_PT, 4),
            colour,
        });
    }
    Ok((!out.is_empty()).then_some(out))
}

/// `_float_or_none(value)`: `float(value)`, None where it raises.
fn float_or_none(v: &PyValue) -> Option<f64> {
    match v {
        PyValue::Float(f) => Some(*f),
        PyValue::Int(i) => Some(*i as f64),
        PyValue::Bool(b) => Some(f64::from(u8::from(*b))),
        PyValue::Str(v) => py::py_float(v),
        _ => None,
    }
}

/// `float(length[0])` of a magnitude `parse_css_length` wrote: always a number.
fn magnitude(mag: &str) -> f64 {
    py::py_float(mag).unwrap_or(0.0)
}

/// `(n, unit)`, a length as `_rule_look` writes it.
fn pair(n: f64, unit: &str) -> PyValue {
    PyValue::Tuple(vec![PyValue::Float(n), s(unit)])
}

/// `_rule_look(css)`: how an `<hr>` is drawn, as Previewer writes it, in kfxgen's key order:
/// `width`, `margin_left`, `margin_right`, `align`, `border`, `height`, `fill`; empty for
/// calibre's plain rule. `fill` is `_box_background(css)`, read last, as kfxgen reads it.
fn rule_look(
    css: Option<&Resolved>,
    fill: impl FnOnce() -> Result<Option<i64>, ItemError>,
) -> Result<Vec<(&'static str, PyValue)>, ItemError> {
    let mut look: Vec<(&'static str, PyValue)> = Vec::new();
    // `css = css or {}`: every value then reads as None, and the look is empty.
    let Some(css) = css else {
        return Ok(look);
    };
    let width = parse_css_length(&text_of(css.get("width")?), false)?;
    match &width {
        Some((mag, unit @ ("$314" | "$319"))) => {
            // `_page_share(width)`.
            look.push(("width", pair(magnitude(mag) * unit_percent(unit), "%")));
        }
        Some((mag, "$308")) => look.push(("width", pair(magnitude(mag), "em"))),
        _ => {}
    }
    let mut margins = [String::new(), String::new()];
    for (slot, (side, key)) in [
        ("margin_left", "margin-left"),
        ("margin_right", "margin-right"),
    ]
    .into_iter()
    .enumerate()
    {
        let value = py::strip(&text_of(css.get(key)?)).to_lowercase();
        if let Some((mag, unit)) = parse_css_length(&value, false)?
            && magnitude(&mag) > 0.0
            && UNIT_TO_PCT.contains(&unit)
        {
            look.push((side, PyValue::Float(magnitude(&mag) * unit_percent(unit))));
        }
        margins[slot] = value;
    }
    if look.iter().any(|(k, _)| *k == "width") {
        // Placed from one side: only that side's margin is written.
        let align = if !matches!(margins[0].as_str(), "" | "auto") {
            look.retain(|(k, _)| *k != "margin_right");
            "left"
        } else if !matches!(margins[1].as_str(), "" | "auto") {
            "right"
        } else {
            "centre"
        };
        look.push(("align", s(align)));
    }
    let sides = match css.get("border-sides")? {
        Some(v) => v.as_list().unwrap_or(&[]),
        None => &[],
    };
    // `str(k).lower()` and `str(c).lower()` differ from `text_of` only for None, False and 0,
    // none of which is "inset" or "currentcolor". The resolver gives each side three parts;
    // kfxgen would raise unpacking any other count.
    let ua = sides.len() == 4
        && sides.iter().all(|side| match side.as_list() {
            Some([k, w, c]) => {
                text_of(Some(k)).to_lowercase() == UA_RULE_SIDE.0
                    && float_or_none(w) == Some(UA_RULE_SIDE.1)
                    && text_of(Some(c)).to_lowercase() == UA_RULE_SIDE.2
            }
            _ => false,
        });
    let border = if ua { None } else { css_border(Some(css))? };
    if let Some(border) = border {
        // A black the book sets is written; the text colour (`currentColor`) is not.
        let raw = |side: &str| {
            let at = BORDER_SIDES.iter().position(|s| *s == side);
            at.and_then(|i| sides.get(i))
                .and_then(PyValue::as_list)
                .and_then(|parts| parts.get(2))
        };
        // A width in em stays in em, in the rule's own size.
        let declared = match css.get("border-widths")? {
            Some(v) => v.as_list().unwrap_or(&[]),
            None => &[],
        };
        let ratio = font_ratio(Some(css));
        let mut drawn = Vec::with_capacity(border.len());
        for BorderSide {
            side,
            kind,
            pt,
            colour,
        } in border
        {
            let at = BORDER_SIDES.iter().position(|s| *s == side);
            let declared = at.and_then(|i| declared.get(i));
            let width = match parse_css_length(&text_of(declared), false)? {
                Some((mag, "$308")) => pair(py::round(magnitude(&mag) * ratio, 4), "em"),
                _ => PyValue::Float(pt),
            };
            let colour = if css_rgb(&text_of(raw(side)))? == Some(0) {
                Some(0xFF00_0000)
            } else {
                colour
            };
            drawn.push((
                s(side),
                PyValue::Tuple(vec![
                    s(&kind),
                    width,
                    colour.map_or(PyValue::None, PyValue::Int),
                ]),
            ));
        }
        look.push(("border", PyValue::Dict(drawn)));
    }
    let height = py::strip(&text_of(css.get("height")?)).to_owned();
    let length = parse_css_length(&height, false)?;
    let mut tall = 0.0;
    if height == "0" {
        look.push(("height", pair(0.0, "pt")));
    } else if let Some((mag, unit @ ("$319" | "$308"))) = &length {
        let (n, unit) = if *unit == "$319" {
            (py::round(magnitude(mag) * 0.45, 4), "pt")
        } else {
            (magnitude(mag), "em")
        };
        tall = n;
        look.push(("height", pair(n, unit)));
    }
    // `look.get("height", (0,))[0] > 0`: false for NaN.
    if let Some(fill) = fill()?
        && tall > 0.0
    {
        look.push(("fill", PyValue::Int(fill)));
    }
    Ok(look)
}

impl Walker<'_, '_, '_, '_> {
    /// `_emit_rule(elem)`: an `<hr>`, a rule block with its margins as space above and below
    /// and its look, or only space when its CSS draws no border or filled bar. Its own ids name
    /// the block after it.
    pub(super) fn emit_rule(&mut self, elem: NodeId) -> Result<(), ItemError> {
        for id in own_anchor_ids(self.dom, elem) {
            self.rule_ids.insert(id.to_owned());
            self.pending_ids.push(id.to_owned());
        }
        let css = self.resolve(elem)?;
        let resolver = self.resolver;
        let look = rule_look(css.as_ref(), || -> Result<Option<i64>, ItemError> {
            resolver.box_background(elem)?.map_err(ItemError::Raised)
        })?;
        if let Some(css) = &css
            && css_border(Some(css))?.is_none()
            && !look.iter().any(|(k, _)| *k == "fill")
        {
            let (top, height, bottom) = hidden_rule_em(
                &text_of(css.get("margin-top")?),
                &text_of(css.get("margin-bottom")?),
                &text_of(css.get("height")?),
                font_ratio(Some(css)),
            )?;
            self.emit_space(top, 0.0, height, bottom);
            return Ok(());
        }
        let space = |side: &str, key: &str| -> Result<f64, ItemError> {
            let set = match &css {
                Some(css) => truthy(css.get(key)?),
                None => false,
            };
            if set {
                margin_em(css.as_ref(), side, 1.0)
            } else {
                Ok(RULE_SPACE_EM)
            }
        };
        // `space_above` is always a float: `_space_between` rewrites it in place.
        let mut rule = vec![
            (
                s("space_above"),
                PyValue::Float(space("top", "margin-top")?),
            ),
            (
                s("space_below"),
                PyValue::Float(space("bottom", "margin-bottom")?),
            ),
        ];
        rule.extend(look.into_iter().map(|(k, v)| (s(k), v)));
        let block = self.new_block(vec![
            ("text", Field::Py(s(""))),
            ("spans", Field::Py(PyValue::List(Vec::new()))),
            ("block_style", Field::Py(PyValue::None)),
            ("anchor_ids", Field::Py(PyValue::List(Vec::new()))),
            ("anchor_offsets", Field::Py(PyValue::Dict(Vec::new()))),
            ("type", Field::Py(s("rule"))),
            ("rule", Field::Py(PyValue::Dict(rule))),
        ]);
        self.blocks.push(block);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::resolver::TextBudget;

    /// A resolved style's pairs, as a case gives them.
    type Css = Vec<(&'static str, PyValue)>;

    /// `repr(v)` for the values a rule's look holds.
    fn repr(v: &PyValue) -> String {
        match v {
            PyValue::None => "None".into(),
            PyValue::Int(i) => i.to_string(),
            PyValue::Float(f) => py::repr_float(*f),
            PyValue::Str(s) => format!("'{s}'"),
            PyValue::Tuple(items) => {
                let inner: Vec<String> = items.iter().map(repr).collect();
                format!("({})", inner.join(", "))
            }
            PyValue::Dict(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", repr(k), repr(v)))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            other => format!("{other:?}"),
        }
    }

    fn side(kind: &str, width: PyValue, colour: &str) -> PyValue {
        PyValue::Tuple(vec![s(kind), width, s(colour)])
    }

    fn sides(given: Vec<PyValue>) -> PyValue {
        let none = side("none", PyValue::Float(2.16), "currentColor");
        let mut all = given;
        all.resize(4, none);
        PyValue::Tuple(all)
    }

    fn widths(top: &str) -> PyValue {
        PyValue::Tuple(vec![s(top), PyValue::None, PyValue::None, PyValue::None])
    }

    /// Each case's css, the fill `_box_background(css)` gives for it, and `repr(_rule_look(css))`,
    /// both printed by the pinned functions (copied out of `converter.py`, run under Python 3
    /// with the pinned `inline_style.py` and `page_scale.py`, no calibre).
    #[test]
    fn rule_looks_match_kfxgen() {
        let f = PyValue::Float;
        let ua = || side("inset", f(0.72), "currentColor");
        let cases: Vec<(Css, Option<i64>, &str)> = vec![
            (vec![], None, "{}"),
            (
                vec![
                    ("width", s("50%")),
                    ("margin-left", s("auto")),
                    ("margin-right", s("auto")),
                ],
                None,
                "{'width': (50.0, '%'), 'align': 'centre'}",
            ),
            (
                vec![
                    ("width", s("300px")),
                    ("margin-left", s("10%")),
                    ("margin-right", s("auto")),
                ],
                None,
                "{'width': (35.15625, '%'), 'margin_left': 10.0, 'align': 'left'}",
            ),
            (
                vec![
                    ("width", s("10em")),
                    ("margin-left", s("auto")),
                    ("margin-right", s("5%")),
                ],
                None,
                "{'width': (10.0, 'em'), 'margin_right': 5.0, 'align': 'right'}",
            ),
            (
                vec![("margin-left", s("10%")), ("margin-right", s("2em"))],
                None,
                "{'margin_left': 10.0, 'margin_right': 6.25}",
            ),
            (
                vec![("width", s("2rem")), ("margin-left", s("1pt"))],
                None,
                "{'margin_left': 0.2604166666666667}",
            ),
            (
                vec![
                    ("width", s("30%")),
                    ("margin-left", s(" AUTO ")),
                    ("margin-right", s("0")),
                ],
                None,
                "{'width': (30.0, '%'), 'align': 'right'}",
            ),
            (
                vec![
                    ("width", s("30%")),
                    ("margin-left", s("-5%")),
                    ("margin-right", s("3mm")),
                ],
                None,
                "{'width': (30.0, '%'), 'align': 'left'}",
            ),
            (
                vec![
                    ("width", s("30%")),
                    ("margin-left", s("1in")),
                    ("margin-right", s("2ex")),
                ],
                None,
                "{'width': (30.0, '%'), 'margin_left': 18.75, 'align': 'left'}",
            ),
            (
                vec![("border-sides", PyValue::Tuple(vec![ua(), ua(), ua(), ua()]))],
                None,
                "{}",
            ),
            (
                vec![(
                    "border-sides",
                    PyValue::Tuple(vec![side("INSET", s("0.72"), "CurrentColor"); 4]),
                )],
                None,
                "{}",
            ),
            (
                vec![(
                    "border-sides",
                    PyValue::Tuple(vec![ua(), ua(), ua(), side("inset", f(0.72), "red")]),
                )],
                None,
                "{'border': {'top': ('inset', 0.45, None), 'right': ('inset', 0.45, None), \
                 'bottom': ('inset', 0.45, None), 'left': ('inset', 0.45, 4294901760)}}",
            ),
            (
                vec![("border-sides", PyValue::Tuple(vec![ua(), ua(), ua()]))],
                None,
                "{'border': {'top': ('inset', 0.45, None), 'right': ('inset', 0.45, None), \
                 'bottom': ('inset', 0.45, None)}}",
            ),
            (
                vec![
                    ("border-sides", sides(vec![side("solid", f(1.5), "black")])),
                    ("border-widths", widths("0.1em")),
                    ("font-size", f(24.0)),
                ],
                None,
                "{'border': {'top': ('solid', (0.15, 'em'), 4278190080)}}",
            ),
            (
                vec![(
                    "border-sides",
                    PyValue::Tuple(vec![side("dashed", f(1.44), "red"); 4]),
                )],
                None,
                "{'border': {'top': ('dashed', 0.9, 4294901760), 'right': ('dashed', 0.9, \
                 4294901760), 'bottom': ('dashed', 0.9, 4294901760), 'left': ('dashed', 0.9, \
                 4294901760)}}",
            ),
            (
                vec![
                    (
                        "border-sides",
                        sides(vec![side("solid", s("thin"), "#000")]),
                    ),
                    ("border-widths", widths("thin")),
                ],
                None,
                "{'border': {'top': ('solid', 0.45, 4278190080)}}",
            ),
            (
                vec![(
                    "border-sides",
                    sides(vec![
                        side("solid", s("medium"), "rgb(0,0,0)"),
                        side("double", s("thick"), "blue"),
                    ]),
                )],
                None,
                "{'border': {'top': ('solid', 1.35, 4278190080), 'right': ('double', 2.25, \
                 4278190335)}}",
            ),
            (
                vec![(
                    "border-sides",
                    sides(vec![
                        side("solid", f(1.0), "transparent"),
                        side("solid", f(0.0), "red"),
                        side("hidden", f(3.0), "red"),
                    ]),
                )],
                None,
                "{}",
            ),
            (
                vec![(
                    "border-sides",
                    sides(vec![side("solid", f(1.0), "rgba(0,0,0,0.5)")]),
                )],
                None,
                "{'border': {'top': ('solid', 0.625, 4286611584)}}",
            ),
            (
                vec![
                    (
                        "border-sides",
                        sides(vec![side("Solid", s("nan"), "currentColor")]),
                    ),
                    ("border-widths", widths("2ex")),
                ],
                None,
                "{'border': {'top': ('solid', (0.88, 'em'), None)}}",
            ),
            (
                vec![
                    ("border-sides", sides(vec![side("solid", f(0.72), "#123")])),
                    ("border-widths", widths("1e3em")),
                    ("font-size", f(13.28)),
                ],
                None,
                "{'border': {'top': ('solid', 0.45, 4279312947)}}",
            ),
            (vec![("height", s("0"))], None, "{'height': (0.0, 'pt')}"),
            (
                vec![("height", s("3px"))],
                Some(4287137928),
                "{'height': (1.35, 'pt'), 'fill': 4287137928}",
            ),
            (
                vec![("height", s("0.5em"))],
                Some(4278850590),
                "{'height': (0.5, 'em'), 'fill': 4278850590}",
            ),
            (vec![("height", s("2pt"))], Some(4294901760), "{}"),
            (
                vec![("height", s("0"))],
                Some(4294901760),
                "{'height': (0.0, 'pt')}",
            ),
            (vec![("height", s(" 0px "))], Some(4294901760), "{}"),
            (
                vec![("height", s("1px"))],
                Some(4278190080),
                "{'height': (0.45, 'pt'), 'fill': 4278190080}",
            ),
            (
                vec![
                    ("width", s("99999999999999999999999999999999999999999999%")),
                    ("height", s("1e2px")),
                ],
                None,
                "{'width': (1e+44, '%'), 'align': 'centre'}",
            ),
            (
                vec![
                    ("width", s("50%")),
                    ("margin-left", s("10%")),
                    ("margin-right", s("10%")),
                    ("border-sides", sides(vec![side("solid", f(0.72), "black")])),
                    ("height", s("2px")),
                ],
                Some(4286611584),
                "{'width': (50.0, '%'), 'margin_left': 10.0, 'align': 'left', 'border': \
                 {'top': ('solid', 0.45, 4278190080)}, 'height': (0.9, 'pt'), 'fill': \
                 4286611584}",
            ),
            (
                vec![
                    ("width", s("100%")),
                    ("border-sides", sides(vec![side("none", f(1.0), "red")])),
                ],
                None,
                "{'width': (100.0, '%'), 'align': 'centre'}",
            ),
        ];
        let budget = TextBudget::new();
        for (pairs, fill, want) in cases {
            let css = Resolved::of(pairs.clone(), &budget);
            let look = rule_look(Some(&css), || Ok(fill)).unwrap();
            let got = PyValue::Dict(look.into_iter().map(|(k, v)| (s(k), v)).collect());
            assert_eq!(repr(&got), want, "{pairs:?}");
        }
        assert!(rule_look(None, || Ok(Some(1))).unwrap().is_empty());
    }

    /// A stranger's values: no panic, and each look is kfxgen's, empty (printed by the pinned
    /// functions as above).
    #[test]
    fn odd_rule_values_do_not_panic() {
        let budget = TextBudget::new();
        let many = "1px ".repeat(10_000);
        for v in [
            "",
            "(",
            ")",
            "url(",
            "nan%",
            "infpx",
            "-inf%",
            "1e309px",
            many.as_str(),
        ] {
            let css = Resolved::of(
                vec![
                    ("width", s(v)),
                    ("margin-left", s(v)),
                    ("margin-right", s(v)),
                    ("height", s(v)),
                    (
                        "border-sides",
                        PyValue::Tuple(vec![side("solid", s(v), v); 4]),
                    ),
                    ("border-widths", PyValue::Tuple(vec![s(v); 4])),
                ],
                &budget,
            );
            let look = rule_look(Some(&css), || Ok(Some(0xFF00_0000))).unwrap();
            assert!(look.is_empty(), "{v:?}: {look:?}");
        }
    }

    /// Where the pinned `_rule_look` raises (run as above), the port raises the same exception;
    /// `2İn` lowers to `2i̇n`, which reads as no length, as in Python.
    #[test]
    fn what_kfxgen_raises_on_in_a_rule_is_raised() {
        let budget = TextBudget::new();
        let none = || side("none", PyValue::Float(1.0), "red");
        let cases: Vec<(Css, Option<&str>)> = vec![
            (
                vec![(
                    "border-sides",
                    PyValue::Tuple(vec![
                        side("solid", PyValue::Float(1.0), "rgba(0,0,0,1.2.3)"),
                        none(),
                        none(),
                        none(),
                    ]),
                )],
                Some("ValueError"),
            ),
            (vec![("width", s("2ın"))], Some("KeyError")),
            (vec![("margin-left", s("2İn"))], None),
            (vec![("height", s("2ın"))], Some("KeyError")),
            (
                vec![
                    (
                        "border-sides",
                        PyValue::Tuple(vec![
                            side("solid", PyValue::Float(1.0), "red"),
                            none(),
                            none(),
                            none(),
                        ]),
                    ),
                    ("border-widths", widths("2ın")),
                ],
                Some("KeyError"),
            ),
        ];
        for (pairs, want) in cases {
            let css = Resolved::of(pairs.clone(), &budget);
            match (rule_look(Some(&css), || Ok(None)), want) {
                (Err(ItemError::Raised(r)), Some(want)) => assert_eq!(r.exception, want),
                (Ok(look), None) => assert!(look.is_empty()),
                (got, _) => panic!("{pairs:?}: {got:?}"),
            }
        }
    }
}
