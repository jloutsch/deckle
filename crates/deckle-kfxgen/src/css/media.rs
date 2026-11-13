// SPDX-License-Identifier: GPL-3.0-only
//! Media queries: which `@media` rules, `@import`s and `media` attributes Calibre keeps
//! (`stylizer.py` `media_ok`, with `tinycss/media3.py` `CSSMedia3Parser.parse_media`).

use cssparser::{Parser, ParserInput, Token};

/// `stylizer.py` `ALLOWED_MEDIA_TYPES`.
const ALLOWED_MEDIA_TYPES: [&str; 4] = ["screen", "all", "aural", "amzn-kf8"];

/// `stylizer.py` `IGNORED_MEDIA_FEATURES`: features that make a query fail.
const IGNORED_MEDIA_FEATURES: [&str; 33] = [
    "width",
    "min-width",
    "max-width",
    "height",
    "min-height",
    "max-height",
    "device-width",
    "min-device-width",
    "max-device-width",
    "device-height",
    "min-device-height",
    "max-device-height",
    "aspect-ratio",
    "min-aspect-ratio",
    "max-aspect-ratio",
    "device-aspect-ratio",
    "min-device-aspect-ratio",
    "max-device-aspect-ratio",
    "color",
    "min-color",
    "max-color",
    "color-index",
    "min-color-index",
    "max-color-index",
    "monochrome",
    "min-monochrome",
    "max-monochrome",
    "-webkit-min-device-pixel-ratio",
    "resolution",
    "min-resolution",
    "max-resolution",
    "scan",
    "grid",
];

/// Whether Calibre applies CSS under this media query (`stylizer.py` `media_ok`): an empty
/// query does, `amzn-mobi` does not, and otherwise any comma-separated query whose media type
/// is `screen`, `all`, `aural` or `amzn-kf8` (as written: `SCREEN` is not) and that tests none
/// of the size, colour or resolution features, or a `not` query that fails those. A malformed
/// query is read as `not all`, as tinycss reads it.
pub fn media_ok(query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if query == "amzn-mobi" {
        return false;
    }
    match parse_media(query) {
        Some(queries) => queries.iter().any(MediaQuery::ok),
        // Calibre's `media_ok` returns True when tinycss raises.
        None => true,
    }
}

struct MediaQuery {
    media_type: String,
    features: Vec<String>,
    negated: bool,
}

impl MediaQuery {
    /// `media_ok`'s `query_ok`.
    fn ok(&self) -> bool {
        let matched = ALLOWED_MEDIA_TYPES.contains(&self.media_type.as_str())
            && !self
                .features
                .iter()
                .any(|f| IGNORED_MEDIA_FEATURES.contains(&f.as_str()));
        self.negated ^ matched
    }
}

/// A token of a media query as tinycss sees it, white space removed.
#[derive(Debug, Clone, PartialEq)]
enum MTok {
    Ident(String),
    /// `( … )`, with the tokens inside it, one level deep.
    Paren(Vec<MTok>),
    /// Another block or a function: a container tinycss rejects here.
    Container,
    Colon,
    Comma,
    Integer,
    Delim(char),
    Other,
}

fn mtok(t: &Token<'_>) -> MTok {
    match t {
        Token::Ident(s) => MTok::Ident(s.to_string()),
        Token::Colon => MTok::Colon,
        Token::Comma => MTok::Comma,
        Token::Number {
            int_value: Some(_), ..
        } => MTok::Integer,
        Token::Delim(c) => MTok::Delim(*c),
        Token::Function(_) | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
            MTok::Container
        }
        _ => MTok::Other,
    }
}

/// The query's tokens, as tinycss reads the head of `@media {raw} {}`: up to the first `{`.
/// `None` when a `;` comes first, which ends that at-rule without a block, so Calibre's
/// `rules[0]` raises.
fn tokens(query: &str) -> Option<Vec<MTok>> {
    let mut input = ParserInput::new(query);
    let mut p = Parser::new(&mut input);
    let mut out = Vec::new();
    loop {
        let Ok(t) = p.next() else { break };
        match t {
            Token::CurlyBracketBlock => break,
            Token::Semicolon => return None,
            Token::ParenthesisBlock => {
                let inner = p
                    .parse_nested_block(|p| {
                        let mut v = Vec::new();
                        while let Ok(t) = p.next() {
                            let t = t.clone();
                            v.push(match t {
                                Token::ParenthesisBlock => MTok::Container,
                                t => mtok(&t),
                            });
                        }
                        Ok::<_, cssparser::ParseError<'_, ()>>(v)
                    })
                    .unwrap_or_default();
                out.push(MTok::Paren(inner));
            }
            t => {
                let t = t.clone();
                out.push(mtok(&t));
            }
        }
    }
    Some(out)
}

/// `CSSMedia3Parser.parse_media`.
fn parse_media(query: &str) -> Option<Vec<MediaQuery>> {
    let toks = tokens(query)?;
    if toks.is_empty() {
        return Some(vec![MediaQuery {
            media_type: "all".to_owned(),
            features: Vec::new(),
            negated: false,
        }]);
    }
    Some(
        toks.split(|t| *t == MTok::Comma)
            .map(|part| {
                parse_query(part).unwrap_or(MediaQuery {
                    media_type: "all".to_owned(),
                    features: Vec::new(),
                    negated: true,
                })
            })
            .collect(),
    )
}

/// One query of `parse_media`'s loop; `None` where it raises `MalformedExpression`.
fn parse_query(part: &[MTok]) -> Option<MediaQuery> {
    let mut negated = false;
    let mut media_type: Option<String> = None;
    let mut features = Vec::new();
    for (i, tok) in part.iter().enumerate() {
        if i == 0
            && let MTok::Ident(v) = tok
        {
            let lower = v.to_lowercase();
            if lower == "only" {
                continue;
            }
            if lower == "not" {
                negated = true;
                continue;
            }
        }
        if media_type.is_none() {
            if let MTok::Ident(v) = tok {
                media_type = Some(v.clone());
                continue;
            }
            media_type = Some("all".to_owned());
        }
        if let MTok::Ident(v) = tok
            && v.eq_ignore_ascii_case("and")
        {
            continue;
        }
        let MTok::Paren(content) = tok else {
            return None;
        };
        let (MTok::Ident(feature), rest) = content.split_first()? else {
            return None;
        };
        if !rest.is_empty() {
            if rest.len() < 2 || rest[0] != MTok::Colon {
                return None;
            }
            let expr = &rest[1..];
            let ratio = matches!(expr, [MTok::Integer, MTok::Delim('/'), MTok::Integer]);
            if expr.len() != 1 && !ratio {
                return None;
            }
        }
        features.push(feature.clone());
    }
    Some(MediaQuery {
        media_type: media_type.unwrap_or_else(|| "all".to_owned()),
        features,
        negated,
    })
}

/// Whether css_parser reads a media list, the text of an `@media` or `@import` rule. Where it
/// does not, it keeps the `@media` rule with no rules inside (and the `@import` media as
/// `all`): each comma-separated query must be `[only|not] type [and (feature)]…` or
/// `(feature) [and (feature)]…`, a feature an identifier with at most one value or a ratio.
/// css_parser keeps a few malformed queries cut short (`screen and color` as `screen and`);
/// the port drops them (DIVERGENCES.md, "Malformed media queries").
pub(crate) fn media_list_valid(query: &str) -> bool {
    let Some(toks) = tokens(query) else {
        return false;
    };
    if toks.is_empty() {
        return false;
    }
    toks.split(|t| *t == MTok::Comma).all(query_valid)
}

fn query_valid(part: &[MTok]) -> bool {
    let mut rest = part;
    let mut need_type = false;
    if let Some(MTok::Ident(v)) = rest.first()
        && (v.eq_ignore_ascii_case("only") || v.eq_ignore_ascii_case("not"))
    {
        rest = &rest[1..];
        need_type = true;
    }
    match rest.first() {
        Some(MTok::Ident(v)) if !v.eq_ignore_ascii_case("and") => rest = &rest[1..],
        Some(MTok::Paren(c)) if !need_type && feature_valid(c) => rest = &rest[1..],
        _ => return false,
    }
    while !rest.is_empty() {
        match rest {
            [MTok::Ident(a), MTok::Paren(c), ..]
                if a.eq_ignore_ascii_case("and") && feature_valid(c) =>
            {
                rest = &rest[2..];
            }
            _ => return false,
        }
    }
    true
}

fn feature_valid(content: &[MTok]) -> bool {
    match content {
        [MTok::Ident(_)] => true,
        [MTok::Ident(_), MTok::Colon, v] => !matches!(v, MTok::Paren(_) | MTok::Container),
        [
            MTok::Ident(_),
            MTok::Colon,
            MTok::Integer,
            MTok::Delim('/'),
            MTok::Integer,
        ] => true,
        _ => false,
    }
}
