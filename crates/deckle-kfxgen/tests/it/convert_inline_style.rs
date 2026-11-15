// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's pure helpers of `inline_style.py` (and `parse_weight`, `parse_style` of
//! `font_table.py`) against the table of their inputs and outputs recorded at the pin
//! (`fixtures/kfxgen/convert/units/inline_style.json`), then on input the table cannot hold
//! (plan Task 7).

use deckle_kfxgen::convert::{
    Flag, Flags, Raised, Segment, Span, css_letter_spacing, css_link_colour, css_rgb,
    css_text_colour, normalize_runs_with_anchors, parse_css_length, parse_style,
    parse_vertical_align, parse_weight,
};
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_support::{dump, model_diff};

/// Helpers of the table another task ports, with why.
const SKIP: &[(&str, &str)] = &[("emitted_name", "font_table.py's naming, Task 19")];

fn s(x: &str) -> PyValue {
    PyValue::Str(x.into())
}

/// `str` or `None`, as the table writes an argument the helpers read as `value or ""`.
fn text(v: &PyValue) -> &str {
    match v {
        PyValue::None => "",
        PyValue::Str(x) => x,
        other => panic!("not a str or None: {other:?}"),
    }
}

fn opt_int(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn length(v: Option<(String, &str)>) -> PyValue {
    v.map_or(PyValue::None, |(mag, unit)| {
        PyValue::Tuple(vec![PyValue::Str(mag), s(unit)])
    })
}

fn ok<T>(v: Result<T, Raised>, row: &PyValue) -> T {
    v.unwrap_or_else(|e| panic!("{row:?} raised {e:?}"))
}

/// A flag of the table: a str, or a (kind, value) tuple.
fn flag(v: &PyValue) -> Flag {
    match v {
        PyValue::Str(x) => Flag::Str(x.clone()),
        PyValue::Tuple(kv) => match &kv[..] {
            [PyValue::Str(kind), value] => Flag::Pair(kind.clone(), value.clone()),
            _ => panic!("not a flag: {v:?}"),
        },
        _ => panic!("not a flag: {v:?}"),
    }
}

fn segments(v: &PyValue) -> Vec<Segment> {
    v.as_list()
        .unwrap()
        .iter()
        .map(|seg| {
            let [PyValue::Str(text), PyValue::Set(flags)] = seg.as_list().unwrap() else {
                panic!("not a segment: {seg:?}");
            };
            Segment {
                text: text.clone(),
                flags: flags.iter().map(flag).collect(),
            }
        })
        .collect()
}

fn normalized((text, spans, anchors): (String, Vec<Span>, Vec<(String, usize)>)) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Str(text),
        PyValue::List(spans.iter().map(Span::to_py).collect()),
        PyValue::Dict(
            anchors
                .into_iter()
                .map(|(id, at)| (PyValue::Str(id), PyValue::Int(at as i64)))
                .collect(),
        ),
    ])
}

/// What the port gives for one row's arguments, as the table writes it.
fn ours(name: &str, args: &[PyValue], row: &PyValue) -> PyValue {
    match name {
        "parse_css_length" => {
            let PyValue::Bool(negative) = args[1] else {
                panic!("{row:?}")
            };
            length(ok(parse_css_length(text(&args[0]), negative), row))
        }
        "parse_vertical_align" => parse_vertical_align(text(&args[0])).map_or(PyValue::None, s),
        "css_rgb" => opt_int(ok(css_rgb(text(&args[0])), row)),
        "css_text_colour" => {
            let background = match &args[1] {
                PyValue::None => None,
                PyValue::Int(bg) => Some(*bg),
                other => panic!("{other:?}"),
            };
            opt_int(ok(css_text_colour(text(&args[0]), background), row))
        }
        "css_link_colour" => opt_int(ok(css_link_colour(text(&args[0])), row)),
        "css_letter_spacing" => length(ok(css_letter_spacing(text(&args[0])), row)),
        "normalize_runs_with_anchors" => {
            normalized(normalize_runs_with_anchors(&segments(&args[0])))
        }
        "parse_weight" => PyValue::Int(parse_weight(match &args[0] {
            PyValue::None => None,
            v => Some(v),
        })),
        _ => panic!("no port of {name}"),
    }
}

/// Each input of the table gives the output kfxgen gave for it.
#[test]
fn every_row_of_the_unit_table_matches() {
    let PyValue::Dict(table) = dump("units", "inline_style.json") else {
        panic!("the table is no dict");
    };
    let mut failures = Vec::new();
    let mut rows = 0;
    for (name, cases) in &table {
        let name = name.as_str().unwrap();
        if SKIP.iter().any(|(n, _)| *n == name) {
            continue;
        }
        for row in cases.as_list().unwrap() {
            let [args, want] = row.as_list().unwrap() else {
                panic!("{name}: not [args, output]: {row:?}");
            };
            let got = ours(name, args.as_list().unwrap(), row);
            for d in model_diff(&got, want) {
                failures.push(format!("{name}{args:?}{d}"));
            }
            rows += 1;
        }
    }
    assert_eq!(rows, 349, "rows read");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn plain(text: &str) -> Segment {
    Segment {
        text: text.into(),
        flags: Flags::default(),
    }
}

/// Numbers past every bound, inf and NaN as text, and no number at all.
#[test]
fn numbers_past_every_bound_give_pythons_answer() {
    let digits = "9".repeat(400);
    // float("9" * 400) is inf: the in, cm and ex forms format it as "inf".
    assert_eq!(
        parse_css_length(&format!("{digits}in"), false),
        Ok(Some(("inf".into(), "$318")))
    );
    assert_eq!(
        parse_css_length(&format!("-{digits}cm"), true),
        Ok(Some(("-inf".into(), "$316")))
    );
    // A unit read as written keeps its digits, all of them.
    assert_eq!(
        parse_css_length(&format!("{digits}px"), false),
        Ok(Some((digits.clone(), "$319")))
    );
    assert_eq!(
        css_letter_spacing(&format!("{digits}px")),
        Ok(Some(("inf".into(), "$318")))
    );
    assert_eq!(parse_vertical_align(&format!("-{digits}em")), Some("sub"));
    // Python's float() reads these words; the length pattern does not.
    for word in [
        "inf", "nan", "infinity", "-inf", "1e3em", "1_0em", "100.", ".",
    ] {
        assert_eq!(parse_css_length(word, true), Ok(None), "{word}");
        assert_eq!(parse_vertical_align(word), None, "{word}");
    }
    // Rounded to four places, a tiny length is no longer zero to the zero test, which
    // came first, and the sign of a negative one stays: f"{-7.2e-06:.4f}" is "-0.0000",
    // stripped to "-0".
    assert_eq!(
        parse_css_length("0.0000001in", false),
        Ok(Some(("0".into(), "$318")))
    );
    assert_eq!(
        parse_css_length("-0.0000001in", true),
        Ok(Some(("-0".into(), "$318")))
    );
    // A channel of 300 digits is 255; a percentage too big for a float is inf, and
    // int(inf * 256 // 100) raises.
    assert_eq!(css_rgb(&format!("rgb({digits},0,0)")), Ok(Some(0xFF0000)));
    assert!(css_rgb(&format!("rgb({digits}%,0,0)")).is_err());
    // int() refuses more than 4300 digits.
    assert!(css_rgb(&format!("rgb({},0,0)", "1".repeat(4301))).is_err());
    assert_eq!(
        css_rgb(&format!("rgb({},0,0)", "1".repeat(4300))),
        Ok(Some(0xFF0000))
    );
    // A hue too big for a float is inf: its hue % 360 is NaN, which colorsys draws as m1.
    assert_eq!(css_rgb(&format!("hsl({digits}, 100%, 50%)")), Ok(Some(0)));
    // float() of "1.2.3" or "." raises, inside a colour the pattern accepted.
    for colour in ["rgba(0,0,0,1.2.3)", "rgba(0,0,0,.)", "hsl(1.2.3,10%,10%)"] {
        assert!(css_rgb(colour).is_err(), "{colour}");
        assert!(css_text_colour(colour, None).is_err(), "{colour}");
        assert!(css_link_colour(colour).is_err(), "{colour}");
    }
    // int(" +7_00 ") is 700; " 7 0" and "7__0" are no int.
    assert_eq!(parse_weight(Some(&s(" +7_00 "))), 700);
    assert_eq!(parse_weight(Some(&s("7__0"))), 400);
    assert_eq!(parse_weight(Some(&s(" 7 0"))), 400);
    assert_eq!(parse_weight(Some(&s(&"9".repeat(4300)))), i64::MAX);
    assert_eq!(parse_weight(Some(&s(&"9".repeat(4301)))), 400);
    assert_eq!(
        parse_weight(Some(&s(&format!("-{}", "9".repeat(30))))),
        i64::MIN
    );
    assert_eq!(parse_weight(Some(&PyValue::Float(f64::NAN))), 400);
    assert_eq!(parse_weight(Some(&PyValue::Bool(true))), 400);
}

/// Digits and spaces outside ASCII: Python's `\d` and `int()` read any decimal digit, its
/// `\s` and `strip()` any `isspace` character, but `int()` and `float()` not U+001C to U+001F.
#[test]
fn digits_and_spaces_outside_ascii_are_pythons() {
    // ٣ is 3 and ٤ is 4 (Arabic-Indic): \d and int() read them; the length pattern's
    // [0-9] does not.
    assert_eq!(css_rgb("rgb(٢٥٥,0,0)"), Ok(Some(0xFF0000)));
    assert_eq!(css_rgb("rgb(٥٠%,0,0)"), Ok(Some(0x800000)));
    assert_eq!(parse_css_length("٣em", false), Ok(None));
    assert_eq!(parse_weight(Some(&s("\u{3000}٧٠٠\u{a0}"))), 700);
    assert_eq!(parse_weight(Some(&s("٧_٠٠"))), 700);
    // str.strip() takes U+001C; then int() reads "700".
    assert_eq!(parse_weight(Some(&s("\u{1c}700"))), 700);
    // A superscript two is a digit to isdigit() but not to int().
    assert_eq!(parse_weight(Some(&s("²"))), 400);
    // Python's \s around a length or a colour is isspace: U+001C and the ideographic space
    // too.
    assert_eq!(
        parse_css_length("\u{1c}\u{3000}2em\u{2028}", false),
        Ok(Some(("2".into(), "$308")))
    );
    assert_eq!(
        css_rgb("\u{3000}rgb(\u{1c}1 ,\u{a0}2,3\u{85})\u{1f}"),
        Ok(Some(0x010203))
    );
    // A non-breaking space between number and unit is no length, and is in vertical-align.
    assert_eq!(parse_css_length("2\u{a0}em", false), Ok(None));
    assert_eq!(parse_vertical_align("2\u{a0}em"), Some("super"));
    // A dotted or dotless i is the i of "in" to re.I, and .lower() leaves no unit kfxgen
    // knows: KeyError.
    assert!(parse_css_length("1\u{130}n", false).is_err());
    assert!(parse_css_length("1\u{131}N", false).is_err());
    assert!(css_letter_spacing("2\u{131}n").is_err());
    // The Kelvin sign lowers to k: "blac\u{212a}" is black.
    assert_eq!(css_rgb("BLAC\u{212a}"), Ok(Some(0)));
    assert!(parse_style(Some(&s("\u{1c} ITALIC\u{3000}"))));
    assert!(!parse_style(Some(&s("italics"))));
}

/// Empty, blank and odd text through the run normalizer: collapsible white space is ASCII
/// only; offsets count code points; an anchor at the end clamps to it.
#[test]
fn runs_count_code_points_and_collapse_ascii_space_only() {
    assert_eq!(
        normalize_runs_with_anchors(&[]),
        (String::new(), vec![], vec![])
    );
    let (text, spans, anchors) = normalize_runs_with_anchors(&[plain(" \t\n\r\u{c} ")]);
    assert_eq!((text.as_str(), spans.len(), anchors.len()), ("", 0, 0));
    // \v, U+001C and U+0085 are the book's own, kept.
    let (text, _, _) = normalize_runs_with_anchors(&[plain(" a\u{b}\u{1c}\u{85} b ")]);
    assert_eq!(text, "a\u{b}\u{1c}\u{85} b");
    let link = Flags::from_iter([Flag::Pair("link".into(), s("x#y"))]);
    let anchor = Segment {
        text: "ignored".into(),
        flags: Flags::from_iter([Flag::Pair("anchor-mark".into(), s("end"))]),
    };
    let (text, spans, anchors) = normalize_runs_with_anchors(&[
        plain("😀𝔘 "),
        Segment {
            text: "é ".into(),
            flags: link.clone(),
        },
        plain("  "),
        anchor,
    ]);
    assert_eq!(text, "😀𝔘 é");
    assert_eq!(
        spans,
        vec![Span {
            start: 3,
            len: 1,
            flags: link
        }]
    );
    assert_eq!(anchors, vec![("end".to_owned(), 4)]);
    // Kept spaces dropped before a break lose their marks: the next line keeps its letters.
    let pre = Segment {
        text: "code  \nnext  x".into(),
        flags: Flags::from_iter([Flag::Str("ws-pre".into())]),
    };
    assert_eq!(
        normalize_runs_with_anchors(&[pre]).0,
        "code\nnext\u{a0}\u{a0}x"
    );
    let start = Segment {
        text: String::new(),
        flags: Flags::from_iter([Flag::Str("line-start".into())]),
    };
    let (text, _, _) =
        normalize_runs_with_anchors(&[plain("a"), plain("\u{2028} "), start, plain("b")]);
    assert_eq!(text, "a\nb");
}

/// A paragraph of a million characters in 200,000 segments, a line of 100,000 tabs in
/// preformatted text, and 100,000 spans: each in time linear in its size.
#[test]
fn long_runs_take_linear_time() {
    let start = std::time::Instant::now();
    let italic = Flags::from_iter([Flag::Str("italic".into())]);
    let bold = Flags::from_iter([Flag::Str("bold".into())]);
    let segs: Vec<Segment> = (0..200_000)
        .map(|i| Segment {
            text: "word ".into(),
            flags: if i % 2 == 0 {
                italic.clone()
            } else {
                bold.clone()
            },
        })
        .collect();
    let (text, spans, _) = normalize_runs_with_anchors(&segs);
    assert_eq!(text.chars().count(), 999_999);
    assert_eq!(spans.len(), 200_000);
    // One span over many segments of equal flags, each its own set.
    let same: Vec<Segment> = (0..200_000)
        .map(|_| Segment {
            text: "word ".into(),
            flags: Flags::from_iter((0..50).map(|k| Flag::Pair("bidi".into(), PyValue::Int(k)))),
        })
        .collect();
    let (_, spans, _) = normalize_runs_with_anchors(&same);
    assert_eq!(spans.len(), 1);
    let pre = Segment {
        text: "\t".repeat(100_000),
        flags: Flags::from_iter([Flag::Str("ws-pre".into())]),
    };
    let (text, _, _) = normalize_runs_with_anchors(&[plain("x"), pre, plain("y")]);
    assert_eq!(text.chars().count(), 800_001);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}
