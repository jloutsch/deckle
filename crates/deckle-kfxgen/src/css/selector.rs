// SPDX-License-Identifier: GPL-3.0-only
//! Selector lists as css_parser 1.1.1 reads and writes them (`Selector.selectorText`).
//!
//! css_parser is not in Calibre's source; this follows what it does inside calibre 9.18, listed
//! in `tests/it/css_sheet.rs`: combinators written ` > `, ` + `, ` ~ ` and one space;
//! pseudo-class and pseudo-element names in lower case; attribute selectors without spaces,
//! string values in double quotes; `:not()`, `:is()`, `:where()`, `:has()` holding selector
//! lists, other functions (`:matches()` and `:any()` among them) an expression of identifiers,
//! numbers, strings, `+` and `-`. A list css_parser cannot read in full is dropped whole: an
//! attribute flag (`[a="b" i]`), a number as an attribute value, `of` in `:nth-child()`, an undeclared
//! namespace prefix, anything after a pseudo-element.

use std::collections::BTreeSet;

use cssparser::{ParseError, Parser, Token};

use super::value::{MAX_NESTING, decode_escapes, serialize_string, string_content};

/// Functional pseudo-classes whose argument css_parser reads as a selector list.
const SELECTOR_FUNCTIONS: [&str; 4] = ["not", "is", "where", "has"];

/// The selectors of a rule's prelude, each as css_parser writes it; `Err` when css_parser
/// drops the rule. `namespaces` are the prefixes the sheet declared.
pub(crate) fn parse_list(
    p: &mut Parser<'_, '_>,
    namespaces: &BTreeSet<String>,
) -> Result<Vec<String>, ()> {
    list(p, namespaces, 0, false)
}

fn list(
    p: &mut Parser<'_, '_>,
    ns: &BTreeSet<String>,
    depth: usize,
    relative: bool,
) -> Result<Vec<String>, ()> {
    let mut out = Vec::new();
    loop {
        let (sel, more) = complex(p, ns, depth, relative)?;
        out.push(sel);
        if !more {
            return Ok(out);
        }
    }
}

/// One selector, up to a comma (`true`) or the end (`false`).
fn complex(
    p: &mut Parser<'_, '_>,
    ns: &BTreeSet<String>,
    depth: usize,
    relative: bool,
) -> Result<(String, bool), ()> {
    let mut out = String::new();
    let mut pending: Option<&str> = None;
    let mut in_compound = false;
    let mut after_pseudo_element = false;
    let mut more = false;
    loop {
        let start = p.position();
        let Ok(tok) = p.next_including_whitespace() else {
            break;
        };
        let tok = tok.clone();
        match tok {
            Token::WhiteSpace(_) => {
                if in_compound && pending.is_none() {
                    pending = Some(" ");
                }
            }
            Token::Comma => {
                more = true;
                break;
            }
            Token::Delim(c @ ('>' | '+' | '~')) => {
                let comb = match c {
                    '>' => " > ",
                    '+' => " + ",
                    _ => " ~ ",
                };
                if !in_compound && out.is_empty() {
                    // A relative selector, inside `:has()` and the like: `> em`.
                    if !relative {
                        return Err(());
                    }
                    out.push_str(comb.trim_start());
                    // Starts the first compound; no further combinator before it.
                    pending = Some("");
                    continue;
                }
                match pending {
                    None | Some(" ") => pending = Some(comb),
                    Some(_) => return Err(()),
                }
            }
            tok => {
                if let Some(c) = pending.take() {
                    out.push_str(c);
                    in_compound = false;
                    after_pseudo_element = false;
                }
                if after_pseudo_element {
                    return Err(());
                }
                let raw = p.slice_from(start).to_owned();
                simple(
                    p,
                    &tok,
                    &raw,
                    in_compound,
                    &mut after_pseudo_element,
                    &mut out,
                    ns,
                    depth,
                )?;
                in_compound = true;
            }
        }
    }
    // Empty, or ending on a combinator (`p >`).
    if !in_compound || matches!(pending, Some(c) if c != " ") {
        return Err(());
    }
    Ok((out, more))
}

/// One simple selector, starting at `tok`, written to `out`.
#[allow(clippy::too_many_arguments)]
fn simple(
    p: &mut Parser<'_, '_>,
    tok: &Token<'_>,
    raw: &str,
    in_compound: bool,
    after_pseudo_element: &mut bool,
    out: &mut String,
    ns: &BTreeSet<String>,
    depth: usize,
) -> Result<(), ()> {
    match tok {
        Token::Ident(_) | Token::Delim('*') | Token::Delim('|') => {
            // A type or universal selector, with its namespace: only first in a compound.
            if in_compound {
                return Err(());
            }
            if matches!(tok, Token::Delim('|')) {
                out.push('|');
                out.push_str(&type_name(p)?);
                return Ok(());
            }
            let first = if matches!(tok, Token::Delim('*')) {
                "*".to_owned()
            } else {
                decode_escapes(raw)
            };
            if p.try_parse(|p| match p.next_including_whitespace() {
                Ok(Token::Delim('|')) => Ok(()),
                _ => Err(()),
            })
            .is_ok()
            {
                if first != "*" && !ns.contains(&first) {
                    return Err(());
                }
                out.push_str(&first);
                out.push('|');
                out.push_str(&type_name(p)?);
            } else {
                out.push_str(&first);
            }
            Ok(())
        }
        Token::Delim('.') => {
            let start = p.position();
            match p.next_including_whitespace() {
                Ok(Token::Ident(_)) => {
                    out.push('.');
                    out.push_str(&decode_escapes(p.slice_from(start)));
                    Ok(())
                }
                _ => Err(()),
            }
        }
        Token::IDHash(_) | Token::Hash(_) => {
            out.push('#');
            out.push_str(&decode_escapes(&raw[1..]));
            Ok(())
        }
        Token::SquareBracketBlock => {
            let attr = p
                .parse_nested_block(|p| {
                    attribute(p, ns).map_err(|()| p.new_custom_error::<(), ()>(()))
                })
                .map_err(|_| ())?;
            out.push_str(&attr);
            Ok(())
        }
        Token::Colon => {
            let start = p.position();
            let tok = p.next_including_whitespace().map_err(|_| ())?.clone();
            let (element, start, tok) = if matches!(tok, Token::Colon) {
                let start = p.position();
                let tok = p.next_including_whitespace().map_err(|_| ())?.clone();
                (true, start, tok)
            } else {
                (false, start, tok)
            };
            out.push_str(if element { "::" } else { ":" });
            match tok {
                Token::Ident(_) => {
                    out.push_str(&decode_escapes(p.slice_from(start)).to_lowercase())
                }
                Token::Function(name) => {
                    if depth + 1 > MAX_NESTING {
                        return Err(());
                    }
                    let name = name.to_lowercase();
                    let selectors = !element && SELECTOR_FUNCTIONS.contains(&name.as_str());
                    let arg = p
                        .parse_nested_block(|p| {
                            if selectors {
                                list(p, ns, depth + 1, true).map(|l| l.join(", "))
                            } else {
                                expression(p)
                            }
                            .map_err(|()| p.new_custom_error::<(), ()>(()))
                        })
                        .map_err(|_: ParseError<'_, ()>| ())?;
                    out.push_str(&name);
                    out.push('(');
                    out.push_str(&arg);
                    out.push(')');
                }
                _ => return Err(()),
            }
            *after_pseudo_element = element;
            Ok(())
        }
        _ => Err(()),
    }
}

/// The element name after a namespace's `|`: an identifier or `*`.
fn type_name(p: &mut Parser<'_, '_>) -> Result<String, ()> {
    let start = p.position();
    match p.next_including_whitespace() {
        Ok(Token::Ident(_)) => Ok(decode_escapes(p.slice_from(start))),
        Ok(Token::Delim('*')) => Ok("*".to_owned()),
        _ => Err(()),
    }
}

/// The argument of a functional pseudo-class css_parser has no selector rule for
/// (`:nth-child( 2n + 1)`, `:lang("fr")`): identifiers, numbers, strings, `+` and `-`,
/// written as they came with white space before the first kept as one space and after the
/// last dropped.
fn expression(p: &mut Parser<'_, '_>) -> Result<String, ()> {
    let mut out = String::new();
    let mut space = false;
    let mut any = false;
    loop {
        let start = p.position();
        let Ok(tok) = p.next_including_whitespace() else {
            break;
        };
        let item = match tok {
            Token::WhiteSpace(_) => {
                space = true;
                continue;
            }
            Token::QuotedString(_) => {
                serialize_string(&string_content(p.slice_from(start)).ok_or(())?)
            }
            Token::Ident(_)
            | Token::Number { .. }
            | Token::Dimension { .. }
            | Token::Delim('+' | '-') => decode_escapes(p.slice_from(start)),
            _ => return Err(()),
        };
        if space {
            out.push(' ');
            space = false;
        }
        out.push_str(&item);
        any = true;
    }
    if !any {
        return Err(());
    }
    Ok(out)
}

/// The inside of `[ … ]`: `[ns|name]`, `[name op value]`, as css_parser writes it.
fn attribute(p: &mut Parser<'_, '_>, ns: &BTreeSet<String>) -> Result<String, ()> {
    let mut out = String::from("[");
    p.skip_whitespace();
    let start = p.position();
    let first = p.next_including_whitespace().map_err(|_| ())?.clone();
    match first {
        Token::Delim('|') => out.push_str(&attr_name(p)?),
        Token::Delim('*') => {
            match p.next_including_whitespace() {
                Ok(Token::Delim('|')) => {}
                _ => return Err(()),
            }
            out.push_str("*|");
            out.push_str(&attr_name(p)?);
        }
        Token::Ident(_) => {
            let name = decode_escapes(p.slice_from(start));
            let prefixed = p
                .try_parse(|p| match p.next_including_whitespace() {
                    Ok(Token::Delim('|')) => Ok(()),
                    _ => Err(()),
                })
                .is_ok();
            if prefixed {
                if !ns.contains(&name) {
                    return Err(());
                }
                out.push_str(&name);
                out.push('|');
                out.push_str(&attr_name(p)?);
            } else {
                out.push_str(&name);
            }
        }
        _ => return Err(()),
    }
    p.skip_whitespace();
    if p.is_exhausted() {
        out.push(']');
        return Ok(out);
    }
    let op = match p.next().map_err(|_| ())? {
        Token::Delim('=') => "=",
        Token::IncludeMatch => "~=",
        Token::DashMatch => "|=",
        Token::PrefixMatch => "^=",
        Token::SuffixMatch => "$=",
        Token::SubstringMatch => "*=",
        _ => return Err(()),
    };
    out.push_str(op);
    p.skip_whitespace();
    let start = p.position();
    let value = match p.next_including_whitespace().map_err(|_| ())? {
        Token::Ident(_) => decode_escapes(p.slice_from(start)),
        Token::QuotedString(_) => serialize_string(&string_content(p.slice_from(start)).ok_or(())?),
        _ => return Err(()),
    };
    out.push_str(&value);
    p.skip_whitespace();
    if !p.is_exhausted() {
        return Err(());
    }
    out.push(']');
    Ok(out)
}

fn attr_name(p: &mut Parser<'_, '_>) -> Result<String, ()> {
    let start = p.position();
    match p.next_including_whitespace() {
        Ok(Token::Ident(_)) => Ok(decode_escapes(p.slice_from(start))),
        _ => Err(()),
    }
}
