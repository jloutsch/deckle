// SPDX-License-Identifier: GPL-3.0-only
//! The `font` shorthand as Calibre expands it: `normalize_css.py` `normalize_font`, which reads
//! the value with `tinycss/fonts3.py` `parse_font` and writes the families back with
//! `serialize_font_family`.

use cssparser::{Parser, ParserInput, Token};

use super::defaults::default_of;

/// `normalize_css.py` `font_composition`: the properties `font` sets, in its order.
const FONT_COMPOSITION: [&str; 6] = [
    "font-style",
    "font-variant",
    "font-weight",
    "font-size",
    "line-height",
    "font-family",
];

// `fonts3.py`'s keyword sets.
const GLOBAL_IDENTS: [&str; 4] = ["inherit", "initial", "unset", "normal"];
const STYLE_IDENTS: [&str; 2] = ["italic", "oblique"];
const VARIANT_IDENTS: [&str; 1] = ["small-caps"];
const WEIGHT_IDENTS: [&str; 3] = ["bold", "bolder", "lighter"];
const STRETCH_IDENTS: [&str; 8] = [
    "ultra-condensed",
    "extra-condensed",
    "condensed",
    "semi-condensed",
    "semi-expanded",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
];
const SIZE_IDENTS: [&str; 9] = [
    "xx-small", "x-small", "small", "medium", "large", "x-large", "xx-large", "larger", "smaller",
];
const WEIGHT_SIZES: [i64; 9] = [100, 200, 300, 400, 500, 600, 700, 800, 900];
const LEGACY_FONT_SPEC: [&str; 6] = [
    "caption",
    "icon",
    "menu",
    "message-box",
    "small-caption",
    "status-bar",
];
const GENERIC_FAMILIES: [&str; 6] = [
    "serif",
    "sans-serif",
    "sansserif",
    "cursive",
    "fantasy",
    "monospace",
];

/// A token as tinycss's tokenizer types it, white space dropped (no branch of `parse_font`
/// reads it). `Dim` is a `DIMENSION` or `PERCENTAGE`, kept as written (`as_css`).
#[derive(Debug, Clone, PartialEq)]
enum FTok {
    Ident(String),
    Str(String),
    /// An `INTEGER`: as written, and its value when it fits.
    Integer(String, Option<i64>),
    Number(String),
    Dim(String),
    Delim(char),
    /// Anything else (a function, a URL, a hash): `parse_font` passes over it.
    Other,
}

impl FTok {
    /// tinycss's `token.value`, for the legacy keyword test.
    fn value(&self) -> Option<&str> {
        match self {
            FTok::Ident(v) | FTok::Str(v) => Some(v),
            _ => None,
        }
    }
}

/// tinycss's `tokenize_grouped`, at the top level, of a value css_parser wrote. A number is
/// typed by its text as tinycss's CSS 2.1 tokenizer types it: `1e3` is a dimension there.
fn tokenize(css: &str) -> Vec<FTok> {
    let mut input = ParserInput::new(css);
    let mut p = Parser::new(&mut input);
    let mut out = Vec::new();
    loop {
        let start = p.position();
        let Ok(tok) = p.next() else { break };
        let tok = tok.clone();
        let raw = p.slice_from(start).trim().to_owned();
        out.push(match tok {
            Token::Ident(v) => FTok::Ident(v.to_string()),
            Token::QuotedString(v) => FTok::Str(v.to_string()),
            Token::Number { .. } => {
                let digits = raw.strip_prefix(['+', '-']).unwrap_or(&raw);
                if !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) {
                    FTok::Integer(raw.clone(), raw.parse().ok())
                } else if digits.split_once('.').is_some_and(|(a, b)| {
                    a.bytes().all(|c| c.is_ascii_digit())
                        && !b.is_empty()
                        && b.bytes().all(|c| c.is_ascii_digit())
                }) {
                    FTok::Number(raw)
                } else {
                    FTok::Dim(raw)
                }
            }
            Token::Dimension { .. } | Token::Percentage { .. } => FTok::Dim(raw),
            Token::Delim(c) => FTok::Delim(c),
            Token::Comma => FTok::Delim(','),
            _ => FTok::Other,
        });
    }
    out
}

/// What `parse_font` reads from a `font` value.
#[derive(Debug, Default, PartialEq)]
struct FontParts {
    style: Option<String>,
    variant: Option<String>,
    weight: Option<String>,
    stretch: Option<String>,
    size: Option<String>,
    height: Option<String>,
    families: Vec<String>,
}

/// `fonts3.py` `parse_font`, line by line: the tokens are read from the start; where Python
/// puts a token back (`tokens.append(tok)`) the index steps back, and where it breaks the
/// token read is dropped and the rest are the families.
fn parse_font(css: &str) -> FontParts {
    let toks = tokenize(css.trim());
    if toks
        .first()
        .and_then(FTok::value)
        .is_some_and(|v| LEGACY_FONT_SPEC.contains(&v))
    {
        return FontParts {
            families: vec!["sans-serif".to_owned()],
            ..FontParts::default()
        };
    }
    let mut f = FontParts::default();
    let mut i = 0;
    while i < toks.len() {
        let tok = &toks[i];
        i += 1;
        match tok {
            FTok::Str(_) => {
                i -= 1;
                break;
            }
            FTok::Integer(raw, v) => {
                if f.size.is_none() {
                    if f.weight.is_none() && v.is_some_and(|v| WEIGHT_SIZES.contains(&v)) {
                        f.weight = Some(raw.clone());
                        continue;
                    }
                    break;
                }
                if f.height.is_none() {
                    f.height = Some(raw.clone());
                }
                break;
            }
            FTok::Number(raw) => {
                if f.size.is_some() && f.height.is_none() {
                    f.height = Some(raw.clone());
                }
                break;
            }
            FTok::Delim(c) => {
                if *c == '/' && f.size.is_some() && f.height.is_none() {
                    continue;
                }
                break;
            }
            FTok::Dim(raw) => {
                if f.size.is_none() {
                    f.size = Some(raw.clone());
                    continue;
                }
                if f.height.is_none() {
                    f.height = Some(raw.clone());
                }
                break;
            }
            FTok::Ident(v) => {
                let v = v.as_str();
                if GLOBAL_IDENTS.contains(&v) {
                    if f.size.is_some() {
                        if f.height.is_none() {
                            f.height = Some(v.to_owned());
                        } else {
                            i -= 1;
                        }
                        break;
                    }
                    let slot = [
                        &mut f.style,
                        &mut f.variant,
                        &mut f.weight,
                        &mut f.stretch,
                        &mut f.size,
                    ]
                    .into_iter()
                    .find(|s| s.is_none());
                    match slot {
                        Some(s) => *s = Some(v.to_owned()),
                        None if f.height.is_none() => {
                            f.height = Some(v.to_owned());
                            break;
                        }
                        None => {
                            i -= 1;
                            break;
                        }
                    }
                    continue;
                }
                if STYLE_IDENTS.contains(&v)
                    || VARIANT_IDENTS.contains(&v)
                    || WEIGHT_IDENTS.contains(&v)
                    || STRETCH_IDENTS.contains(&v)
                {
                    if f.size.is_some() {
                        break;
                    }
                    let slot = if STYLE_IDENTS.contains(&v) {
                        &mut f.style
                    } else if VARIANT_IDENTS.contains(&v) {
                        &mut f.variant
                    } else if WEIGHT_IDENTS.contains(&v) {
                        &mut f.weight
                    } else {
                        &mut f.stretch
                    };
                    *slot = Some(v.to_owned());
                } else if SIZE_IDENTS.contains(&v) {
                    f.size = Some(v.to_owned());
                } else {
                    i -= 1;
                    break;
                }
            }
            FTok::Other => {}
        }
    }
    f.families = family_tokens(&toks[i..]);
    f
}

/// `fonts3.py` `parse_font_family_tokens`: a string starts a family, identifiers join the
/// current one with spaces, a comma ends it.
fn family_tokens(toks: &[FTok]) -> Vec<String> {
    let mut families = Vec::new();
    let mut current = String::new();
    let commit = |current: &str, families: &mut Vec<String>| {
        let val = current.trim();
        if !val.is_empty() {
            families.push(val.to_owned());
        }
    };
    for tok in toks {
        match tok {
            FTok::Str(v) => {
                if !current.is_empty() {
                    commit(&current, &mut families);
                }
                current = v.clone();
            }
            FTok::Delim(',') => {
                if !current.is_empty() {
                    commit(&current, &mut families);
                }
                current.clear();
            }
            FTok::Ident(v) => {
                current.push(' ');
                current.push_str(v);
            }
            _ => {}
        }
    }
    if !current.is_empty() {
        commit(&current, &mut families);
    }
    families
}

/// `fonts3.py` `serialize_single_font_family`: a generic family in lower case
/// (`sansserif` as `sans-serif`), a plain name (`[a-zA-Z][a-zA-Z0-9_-]*`, not starting with
/// `and`, which css_parser cannot read) as it is, any other in double quotes.
fn serialize_family(x: &str) -> String {
    let xl = x.to_lowercase();
    if GENERIC_FAMILIES.contains(&xl.as_str()) {
        return if xl == "sansserif" {
            "sans-serif".to_owned()
        } else {
            xl
        };
    }
    let b = x.as_bytes();
    let simple = b.first().is_some_and(u8::is_ascii_alphabetic)
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-');
    if simple && !xl.starts_with("and") {
        x.to_owned()
    } else {
        format!("\"{}\"", x.replace('"', "\\\""))
    }
}

/// `normalize_css.py` `normalize_font`: the six `font_composition` properties, from their
/// defaults and what `parse_font` reads, then `font-stretch` when the value names one.
pub(crate) fn normalize_font(value: &str) -> Vec<(String, String)> {
    if value == "inherit" {
        return FONT_COMPOSITION
            .iter()
            .map(|k| ((*k).to_owned(), "inherit".to_owned()))
            .collect();
    }
    let mut out: Vec<(String, String)> = FONT_COMPOSITION
        .iter()
        .map(|k| ((*k).to_owned(), default_of(k).to_owned()))
        .collect();
    if LEGACY_FONT_SPEC.contains(&value) {
        return out;
    }
    let f = parse_font(value);
    let family = (!f.families.is_empty()).then(|| {
        f.families
            .iter()
            .map(|x| serialize_family(x))
            .collect::<Vec<_>>()
            .join(", ")
    });
    for (k, v) in [
        ("font-style", f.style),
        ("font-variant", f.variant),
        ("font-weight", f.weight),
        ("font-size", f.size),
        ("line-height", f.height),
        ("font-family", family),
    ] {
        if let Some(v) = v
            && let Some(slot) = out.iter_mut().find(|(n, _)| n == k)
        {
            slot.1 = v;
        }
    }
    if let Some(stretch) = f.stretch {
        out.push(("font-stretch".to_owned(), stretch));
    }
    out
}
