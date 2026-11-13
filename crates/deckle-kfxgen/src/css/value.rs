// SPDX-License-Identifier: GPL-3.0-only
//! Declarations and their values in css_parser 1.1.1's form.
//!
//! css_parser is not part of Calibre's source; what is ported here was read from it inside
//! calibre 9.18 (`calibre-debug`, `parseStyle`, `Property.value`) and is listed in
//! `tests/it/css_sheet.rs`. A value is a list of items written with one space between them,
//! `, ` after a comma and nothing around a `/`; a declaration whose value css_parser cannot
//! read (a lone `#`, a block, an operator outside `calc()`, a custom property) is dropped.

use std::borrow::Cow;

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, Token,
};

use super::budget::{Budget, decl_units};
use super::{Decl, Decls};

/// How deep functions may nest in a value (`f(g(h()))`). css_parser has no limit of its own
/// short of Python's recursion limit; a deeper value is dropped (DIVERGENCES.md, "CSS nested
/// past 32 levels").
pub(crate) const MAX_NESTING: usize = 32;

/// Units css_parser drops from a zero (`0pt` is written `0`): CSS 2.1's lengths. `0rem`,
/// `0%` and `0deg` keep theirs.
const ZERO_LENGTH_UNITS: [&str; 8] = ["em", "ex", "px", "in", "cm", "mm", "pt", "pc"];

/// A value as css_parser writes it (`PropertyValue.cssText`): property names and units in
/// lower case, `.5em` as `0.5em`, a zero length as `0`, keywords and colours as written,
/// strings in double quotes. Returns an empty string when css_parser drops the declaration:
/// an empty value, or one it cannot read. A value it can read but no property accepts
/// (`foo`, `10 px`) is kept.
pub fn serialize_value(tokens: &str) -> String {
    write_value(tokens, None).unwrap_or_default()
}

/// [`serialize_value`], with each `url()` passed through `urls` on the way. `None` when
/// css_parser would drop it.
pub(crate) fn write_value(text: &str, urls: Option<&dyn Fn(&str) -> String>) -> Option<String> {
    let mut input = ParserInput::new(text);
    let mut p = Parser::new(&mut input);
    let mut out = String::new();
    let w = Writer { src: text, urls };
    w.items(&mut p, &mut out, Ctx::Top, 0).ok()?;
    Some(out)
}

/// Where an item list is: what separators and operators it allows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Top,
    /// Arguments of a function css_parser has no rule for: items and commas.
    Function,
    /// `calc()`: numbers and `var()` between `+ - * /`, written with spaces around them;
    /// `+` and `-` need white space on both sides.
    Calc,
    /// A `url(` holding more than one string (`url('a' 'b')`): items and commas, as
    /// [`Ctx::Function`] but without `/`.
    Url,
    /// `rgb() rgba() hsl() hsla()`: numbers and percentages.
    Color,
    /// `alpha(opacity=50)`, Internet Explorer's filter, the one place css_parser reads `=`.
    Alpha,
}

struct Writer<'a> {
    src: &'a str,
    urls: Option<&'a dyn Fn(&str) -> String>,
}

impl Writer<'_> {
    fn items<'i>(
        &self,
        p: &mut Parser<'i, '_>,
        out: &mut String,
        ctx: Ctx,
        depth: usize,
    ) -> Result<(), ()> {
        let mut first = true;
        let mut joiner: Option<&str> = None;
        // In `calc()`: whether the next item must be a number (not an operator).
        let mut want_operand = true;
        loop {
            let before_space = p.position();
            p.skip_whitespace();
            let space_before = p.position() != before_space;
            let start = p.state();
            let Ok(tok) = p.next_including_whitespace() else {
                break;
            };
            let tok = tok.clone();
            let raw = p.slice_from(start.position());
            if ctx == Ctx::Calc {
                let operator = matches!(tok, Token::Delim('+' | '-' | '*' | '/'));
                if operator == want_operand || matches!(tok, Token::Comma) {
                    return Err(());
                }
                if matches!(tok, Token::Delim('+' | '-')) {
                    let space_after = self
                        .src
                        .get(p.position().byte_index()..)
                        .is_some_and(|rest| rest.starts_with(is_css_space));
                    if !space_before || !space_after {
                        return Err(());
                    }
                }
                want_operand = operator;
            }
            let joins = match (&tok, ctx) {
                (Token::Comma, _) => Some(", "),
                (Token::Delim('/'), Ctx::Top | Ctx::Function) => Some("/"),
                (Token::Delim('='), Ctx::Alpha) => Some("="),
                _ => None,
            };
            if let Some(j) = joins {
                if first || joiner.is_some() {
                    return Err(());
                }
                joiner = Some(j);
                continue;
            }
            let item = self.item(p, &tok, raw, &start, ctx, depth)?;
            if !first {
                out.push_str(joiner.unwrap_or(" "));
            }
            out.push_str(&item);
            first = false;
            joiner = None;
        }
        if joiner.is_some() || (ctx == Ctx::Calc && want_operand) {
            return Err(());
        }
        Ok(())
    }

    fn item<'i>(
        &self,
        p: &mut Parser<'i, '_>,
        tok: &Token<'i>,
        raw: &str,
        start: &ParserState,
        ctx: Ctx,
        depth: usize,
    ) -> Result<String, ()> {
        if ctx == Ctx::Color && !matches!(tok, Token::Number { .. } | Token::Percentage { .. }) {
            return Err(());
        }
        let calc_item = match tok {
            Token::Number { .. } | Token::Dimension { .. } | Token::Percentage { .. } => true,
            Token::Delim('+' | '-' | '*' | '/') => true,
            Token::Function(name) => name.eq_ignore_ascii_case("var"),
            _ => false,
        };
        if ctx == Ctx::Calc && !calc_item {
            return Err(());
        }
        match tok {
            Token::Ident(name) => {
                if raw.starts_with("--") {
                    return Err(());
                }
                if name.eq_ignore_ascii_case("u")
                    && let Some(range) = self.unicode_range(p, start)
                {
                    return range;
                }
                Ok(decode_escapes(raw))
            }
            Token::Number { .. } | Token::Dimension { .. } | Token::Percentage { .. } => {
                format_number(&decode_escapes(raw)).ok_or(())
            }
            Token::QuotedString(_) => string_content(raw).map(|s| serialize_string(&s)).ok_or(()),
            Token::UnquotedUrl(_) => {
                let inner = raw
                    .find('(')
                    .zip(raw.rfind(')'))
                    .filter(|(a, b)| a < b)
                    .map(|(a, b)| raw[a + 1..b].trim_matches(is_css_space))
                    .ok_or(())?;
                Ok(self.url(&decode_escapes(inner)))
            }
            Token::Function(name) => {
                if depth + 1 > MAX_NESTING {
                    return Err(());
                }
                let lower = name.to_ascii_lowercase();
                if lower == "url" {
                    // `url(` then a quote: one string is an address; more items are a
                    // function css_parser writes as it is (`url("a" "b")`), its strings not
                    // addresses.
                    let content = p
                        .parse_nested_block(|p| {
                            let at_start = p.state();
                            p.skip_whitespace();
                            let s = p.state();
                            let one = matches!(p.next(), Ok(Token::QuotedString(_)));
                            let raw = p.slice_from(s.position());
                            p.skip_whitespace();
                            if one && p.is_exhausted() {
                                return string_content(raw)
                                    .map(|c| self.url(&c))
                                    .ok_or_else(|| p.new_custom_error::<(), ()>(()));
                            }
                            p.reset(&at_start);
                            let mut inner = String::new();
                            self.items(p, &mut inner, Ctx::Url, depth + 1)
                                .map_err(|()| p.new_custom_error::<(), ()>(()))?;
                            Ok(format!("url({inner})"))
                        })
                        .map_err(|_| ())?;
                    return Ok(content);
                }
                let (written, inner_ctx) = match lower.as_str() {
                    // css_parser's calc keeps the name as written (`CALC(`).
                    "calc" => (raw.trim_end_matches('(').to_owned(), Ctx::Calc),
                    "rgb" | "rgba" | "hsl" | "hsla" => (lower, Ctx::Color),
                    "alpha" => (lower, Ctx::Alpha),
                    _ => (lower, Ctx::Function),
                };
                let mut inner = String::new();
                p.parse_nested_block(|p| {
                    self.items(p, &mut inner, inner_ctx, depth + 1)
                        .map_err(|()| p.new_custom_error::<(), ()>(()))
                })
                .map_err(|_| ())?;
                if inner_ctx == Ctx::Color && inner.is_empty() {
                    return Err(());
                }
                Ok(format!("{written}({inner})"))
            }
            Token::Hash(_) | Token::IDHash(_) => {
                let digits = decode_escapes(&raw[1..]);
                let hex = digits.chars().all(|c| c.is_ascii_hexdigit());
                match digits.len() {
                    3 if hex => Ok(format!("#{digits}")),
                    6 if hex => {
                        let b = digits.as_bytes();
                        if b[0] == b[1] && b[2] == b[3] && b[4] == b[5] {
                            let short: String =
                                [b[0], b[2], b[4]].iter().map(|&c| c as char).collect();
                            Ok(format!("#{short}"))
                        } else {
                            Ok(format!("#{digits}"))
                        }
                    }
                    _ => Err(()),
                }
            }
            Token::Delim(c @ ('+' | '-' | '*' | '/')) if ctx == Ctx::Calc => Ok(c.to_string()),
            _ => Err(()),
        }
    }

    /// A `url()` item: quoted when the address holds a character css_parser quotes for.
    fn url(&self, address: &str) -> String {
        let address = match self.urls {
            Some(map) => map(address),
            None => address.to_owned(),
        };
        url_item(&address)
    }

    /// `U+0025-00FF`, which `cssparser` reads as an identifier and numbers: css_parser writes
    /// it in lower case. `None` when the text there is not a range.
    fn unicode_range<'i>(
        &self,
        p: &mut Parser<'i, '_>,
        start: &ParserState,
    ) -> Option<Result<String, ()>> {
        let from = start.position().byte_index();
        let rest = self.src.get(from..)?;
        let len = unicode_range_len(rest)?;
        let end = from + len;
        let after_ident = p.state();
        while p.position().byte_index() < end {
            if p.next_including_whitespace().is_err() {
                break;
            }
        }
        if p.position().byte_index() != end {
            p.reset(&after_ident);
            return None;
        }
        Some(Ok(rest[..len].to_lowercase()))
    }
}

/// The length of a unicode range at the start of `s`: `u+` then up to six hex digits or `?`,
/// optionally `-` and up to six hex digits.
fn unicode_range_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 3 || !b[0].eq_ignore_ascii_case(&b'u') || b[1] != b'+' {
        return None;
    }
    let mut i = 2;
    while i < b.len() && i < 8 && (b[i].is_ascii_hexdigit() || b[i] == b'?') {
        i += 1;
    }
    if i == 2 {
        return None;
    }
    if i < b.len() && b[i] == b'-' {
        let mut j = i + 1;
        while j < b.len() && j < i + 7 && b[j].is_ascii_hexdigit() {
            j += 1;
        }
        if j > i + 1 {
            i = j;
        }
    }
    Some(i)
}

fn is_css_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// A number as css_parser writes it (`DimensionValue`): the text split into sign, number and
/// unit (`([+-]?)(\d*\.\d+|\d+)(.*)`, so `1e3` is `1` with the unit `e3`); an integer written
/// as Python's `int`, a decimal that is whole as an integer, any other rounded to six places
/// with trailing zeros dropped (`1.0000001` is `1.0`); the unit in lower case; a zero without
/// its sign, and without its unit when that is a length. `None` for text css_parser does not
/// read as a number.
fn format_number(raw: &str) -> Option<String> {
    let (sign, rest) = match raw.as_bytes().first() {
        Some(b'+') => ("+", &raw[1..]),
        Some(b'-') => ("-", &raw[1..]),
        _ => ("", raw),
    };
    let b = rest.as_bytes();
    let int_len = b.iter().take_while(|c| c.is_ascii_digit()).count();
    let mut end = int_len;
    if b.get(end) == Some(&b'.') {
        let frac = b[end + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if frac > 0 {
            end += 1 + frac;
        }
    }
    if end == 0 {
        return None;
    }
    let (digits, unit) = rest.split_at(end);
    let unit = unit.to_lowercase();
    let (text, zero) = if digits.contains('.') {
        let v: f64 = digits.parse().ok()?;
        if !v.is_finite() {
            return None;
        }
        if v.fract() == 0.0 {
            (format!("{v:.0}"), v == 0.0)
        } else {
            let mut s = format!("{v:.6}");
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.push('0');
            }
            (s, false)
        }
    } else {
        let t = digits.trim_start_matches('0');
        if t.is_empty() {
            ("0".to_owned(), true)
        } else {
            (t.to_owned(), false)
        }
    };
    if zero {
        let unit = if ZERO_LENGTH_UNITS.contains(&unit.as_str()) {
            ""
        } else {
            unit.as_str()
        };
        return Some(format!("0{unit}"));
    }
    Some(format!("{sign}{text}{unit}"))
}

/// Text with css_parser's escapes resolved: `\` and one to six hex digits, and one white space
/// after them, become that character; any other escape is kept as written (`a\:b`). Python's
/// `re.sub` scans left to right, so in `\\31` the first backslash is kept and `\31` resolved.
/// A code point past U+10FFFF is kept as written; a surrogate, which Rust cannot hold, becomes
/// U+FFFD (Python keeps it).
pub(crate) fn decode_escapes(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if b[i] == b'\\' {
            let hex = b[i + 1..]
                .iter()
                .take(6)
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            if hex > 0 {
                let mut end = i + 1 + hex;
                let cp = u32::from_str_radix(&s[i + 1..end], 16).unwrap_or(u32::MAX);
                if s[end..].starts_with("\r\n") {
                    end += 2;
                } else if matches!(b.get(end), Some(b' ' | b'\t' | b'\r' | b'\n' | 0x0c)) {
                    end += 1;
                }
                if cp > 0x10_FFFF {
                    out.push_str(&s[i..end]);
                } else {
                    out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                }
                i = end;
                continue;
            }
        }
        let c = s[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// The text of a quoted string token as css_parser keeps it (`helper.stringvalue`): escapes
/// resolved as [`decode_escapes`] does, then a backslash before a line break dropped with it
/// (so `"\\a"`, a backslash and an escaped line feed, is empty), the quote it was written
/// with unescaped, other escapes as written. `None` for a string with no closing quote, which
/// css_parser drops.
pub(crate) fn string_content(raw: &str) -> Option<String> {
    let q = raw.chars().next().filter(|&c| c == '"' || c == '\'')?;
    let body = raw.get(1..)?;
    let inner = body.strip_suffix(q)?;
    let escapes = inner.bytes().rev().take_while(|&c| c == b'\\').count();
    if escapes % 2 == 1 {
        return None;
    }
    let decoded = decode_escapes(inner);
    let inner = drop_line_continuations(&decoded);
    Some(inner.replace(&format!("\\{q}"), &q.to_string()))
}

/// A string's text without its line continuations: each backslash before `\r\n`, `\n`, `\r`
/// or a form feed dropped with the break, in one pass from the left.
fn drop_line_continuations(s: &str) -> Cow<'_, str> {
    if !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut kept) = (0, 0);
    while i < b.len() {
        let len = match (b[i], b.get(i + 1), b.get(i + 2)) {
            (b'\\', Some(b'\r'), Some(b'\n')) => 3,
            (b'\\', Some(b'\n' | b'\r' | 0x0c), _) => 2,
            _ => {
                i += 1;
                continue;
            }
        };
        out.push_str(&s[kept..i]);
        i += len;
        kept = i;
    }
    out.push_str(&s[kept..]);
    Cow::Owned(out)
}

/// A string as css_parser writes it (`helper.string`): in double quotes, `"` escaped and a
/// line feed written `\a `.
pub(crate) fn serialize_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\n', "\\a ").replace('"', "\\\""))
}

/// A `url()` as css_parser writes it (`helper.uri`): in double quotes when the address holds
/// white space, a parenthesis, a quote, `;` or `,`.
fn url_item(address: &str) -> String {
    if address
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | ';' | ',' | '\'' | '"'))
    {
        format!("url(\"{}\")", address.replace('"', "\\\""))
    } else {
        format!("url({address})")
    }
}

/// Whether `href` starts with a URL scheme, as Python's `urlparse` reads one: a letter, then
/// letters, digits, `+`, `-` or `.`, then `:`.
fn has_scheme(href: &str) -> bool {
    let Some((scheme, _)) = href.split_once(':') else {
        return false;
    };
    scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Python's `posixpath.normpath`: `.` and empty segments dropped, `..` taken back where it
/// can be, `.` for nothing.
fn normpath(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}

/// The directory part of an href, `""` for none.
fn dirname(href: &str) -> &str {
    href.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `url` made book-absolute from the sheet at `sheet_href` (`base.py`
/// `Manifest.Item.abshref`): an address with a scheme is kept, a bare fragment names the
/// sheet, and a sheet at the root leaves the address as written.
pub(crate) fn abshref(sheet_href: &str, url: &str) -> String {
    if has_scheme(url) {
        return url.to_owned();
    }
    let (path, frag) = match url.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (url, None),
    };
    if path.is_empty() {
        return match frag {
            Some(f) if !f.is_empty() => format!("{sheet_href}#{f}"),
            _ => sheet_href.to_owned(),
        };
    }
    if !sheet_href.contains('/') {
        return url.to_owned();
    }
    if url.starts_with('/') {
        return normpath(url);
    }
    normpath(&format!("{}/{url}", dirname(sheet_href)))
}

/// `href`, written relative to the sheet at `base`, made relative to the sheet `base` is
/// relative to: how css_parser's `resolveImports` rewrites an imported sheet's addresses and
/// nested imports (`urljoin` with the import's address).
pub(crate) fn rel_join(base: &str, href: &str) -> String {
    if has_scheme(href) || href.starts_with('/') || href.starts_with('#') {
        return href.to_owned();
    }
    let dir = dirname(base);
    if dir.is_empty() {
        return href.to_owned();
    }
    normpath(&format!("{dir}/{href}"))
}

/// An `@import`'s address relative to the top sheet, written one way: `./p.css`, `a/../p.css`
/// and `p.css` are the same sheet (css_parser resolves each against the importing sheet's
/// URL). An address with a scheme is kept as written.
pub(crate) fn import_key(href: &str) -> String {
    if has_scheme(href) {
        href.to_owned()
    } else {
        normpath(href)
    }
}

/// A declaration list (a rule's block, a `style` attribute) read into css_parser's form, each
/// `url()` passed through `urls`. With a `budget`, each declaration is charged as it is read
/// and reading stops when the budget runs out (the caller then refuses the sheet).
pub(crate) fn declarations(
    input: &mut Parser<'_, '_>,
    budget: Option<&Budget>,
    urls: Option<&dyn Fn(&str) -> String>,
) -> Decls {
    let mut list = Vec::new();
    let mut collector = Collector {
        out: &mut list,
        budget,
        urls,
    };
    let mut items = RuleBodyParser::new(input, &mut collector);
    while items.next().is_some() {
        if budget.is_some_and(Budget::over) {
            // The sheet is refused: what was read is not needed.
            return Decls::new();
        }
    }
    dedupe(list)
}

/// The declarations of a `style` attribute as Calibre reads them (`stylizer.py`
/// `Style._apply_style_attr`): split on `;`, a part that starts with `mso-`, `panose-`,
/// `text-underline` or `tab-interval` dropped (`Style.MS_PAT`, which, unlike the stylesheet
/// preprocessor, is case-sensitive), the rest read as a declaration list.
pub fn parse_style_attr(text: &str) -> Decls {
    style_attr(text, None)
}

/// [`parse_style_attr`], each `url()` made book-absolute from the document at `href`, as
/// `_apply_style_attr` does with `replaceUrls(style, item.abshref)`.
pub(crate) fn style_attr_in(text: &str, href: &str) -> Decls {
    style_attr(text, Some(&|u: &str| abshref(href, u)))
}

fn style_attr(text: &str, urls: Option<&dyn Fn(&str) -> String>) -> Decls {
    const MS: [&str; 4] = ["mso-", "panose-", "text-underline", "tab-interval"];
    let kept: Vec<&str> = text
        .split(';')
        .map(str::trim)
        .filter(|y| !y.is_empty() && !MS.iter().any(|m| y.starts_with(m)))
        .collect();
    let joined = kept.join("; ");
    let mut input = ParserInput::new(&joined);
    let mut p = Parser::new(&mut input);
    declarations(&mut p, None, urls)
}

/// css_parser's view of a declaration list (`CSSStyleDeclaration.__iter__`): each name once,
/// in the order of its last declaration, with its last `!important` value, else its last
/// value. In place, names not copied: the positions sorted by name (stably) give each
/// name's declarations in order.
fn dedupe(mut list: Decls) -> Decls {
    if list.len() < 2 {
        return list;
    }
    let mut order: Vec<usize> = (0..list.len()).collect();
    order.sort_by(|&a, &b| list[a].0.cmp(&list[b].0));
    let mut keep = vec![false; list.len()];
    // (an earlier `!important` declaration, the last of its name) where the first wins.
    let mut moves = Vec::new();
    for group in order.chunk_by(|&a, &b| list[a].0 == list[b].0) {
        let last = group[group.len() - 1];
        if let Some(&from) = group.iter().rev().find(|&&i| list[i].1.important)
            && from != last
        {
            moves.push((from, last));
        }
        keep[last] = true;
    }
    for (from, last) in moves {
        let value = std::mem::take(&mut list[from].1.value);
        list[last].1 = Decl {
            value,
            important: true,
        };
    }
    let mut i = 0;
    list.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    list
}

/// Gathers the declarations of a declaration list, in order, repeats included.
struct Collector<'a> {
    out: &'a mut Decls,
    budget: Option<&'a Budget>,
    urls: Option<&'a dyn Fn(&str) -> String>,
}

impl<'i> DeclarationParser<'i> for Collector<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        _start: &ParserState,
    ) -> Result<(), ParseError<'i, ()>> {
        // css_parser lower-cases a property name and drops custom properties.
        let name = name.to_lowercase();
        if name.starts_with("--") {
            return Err(input.new_custom_error(()));
        }
        let start = input.position();
        let mut end = None;
        let mut important = false;
        loop {
            let before = input.position();
            let Ok(tok) = input.next_including_whitespace() else {
                break;
            };
            let opens_block = matches!(
                tok,
                Token::Function(_)
                    | Token::ParenthesisBlock
                    | Token::SquareBracketBlock
                    | Token::CurlyBracketBlock
            );
            if opens_block {
                // Read past the block now, so the next `before` is after it. `cssparser`
                // skips a block without recursion.
                let _ = input.parse_nested_block(|_| Ok::<(), ParseError<'i, ()>>(()));
                continue;
            }
            if matches!(tok, Token::Delim('!')) {
                // The value ends at the first `!`. It is important when `important` (any
                // case, after any spacing) is all that follows; anything else there
                // (`!foo`, `!important!important`, a lone `!`) leaves the value kept and not
                // important.
                end = Some(before);
                important = input
                    .try_parse(|i| {
                        i.skip_whitespace();
                        let word = i.expect_ident_cloned()?;
                        i.expect_exhausted()?;
                        Ok::<_, ParseError<'i, ()>>(word.eq_ignore_ascii_case("important"))
                    })
                    .unwrap_or(false);
                while input.next().is_ok() {}
                break;
            }
        }
        let end = end.unwrap_or_else(|| input.position());
        let raw = input.slice(start..end);
        if let Some(budget) = self.budget
            && !budget.charge(0, decl_units(&name, raw.len()))
        {
            return Err(input.new_custom_error(()));
        }
        let value = write_value(raw, self.urls).unwrap_or_default();
        if value.is_empty() {
            return Err(input.new_custom_error(()));
        }
        self.out.push((name, Decl { value, important }));
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for Collector<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for Collector<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, (), ()> for Collector<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}
