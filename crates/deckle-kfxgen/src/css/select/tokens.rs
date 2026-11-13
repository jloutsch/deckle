// SPDX-License-Identifier: GPL-3.0-only
//! The tokens of a selector: `css_selectors/parser.py`'s `tokenize` and `TokenStream`.

use super::Fail;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Tok {
    /// White space.
    S,
    Ident(String),
    Hash(String),
    Str(String),
    Num(String),
    Delim(char),
    Eof,
}

static EOF: Tok = Tok::Eof;

pub(super) fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c')
}

/// The end of the escape at `i`: a backslash and 1 to 6 hex digits with one white space after
/// them, or a backslash and a character that is neither a newline nor a hex digit
/// (`TokenMacros.escape`, matched case-insensitively).
fn escape_end(c: &[char], i: usize) -> Option<usize> {
    if c.get(i) != Some(&'\\') {
        return None;
    }
    let next = *c.get(i + 1)?;
    if next.is_ascii_hexdigit() {
        let mut j = i + 1;
        while j < c.len() && j < i + 7 && c[j].is_ascii_hexdigit() {
            j += 1;
        }
        if c.get(j) == Some(&'\r') && c.get(j + 1) == Some(&'\n') {
            j += 2;
        } else if c.get(j).is_some_and(|&w| is_ws(w)) {
            j += 1;
        }
        return Some(j);
    }
    (!matches!(next, '\n' | '\r' | '\x0c')).then_some(i + 2)
}

/// `nmstart`: a letter, `_`, an escape or a character past ASCII.
fn nmstart_end(c: &[char], i: usize) -> Option<usize> {
    match *c.get(i)? {
        ch if ch.is_ascii_alphabetic() || ch == '_' || !ch.is_ascii() => Some(i + 1),
        '\\' => escape_end(c, i),
        _ => None,
    }
}

/// `nmchar`: `nmstart`, a digit or `-`.
fn nmchar_end(c: &[char], i: usize) -> Option<usize> {
    match *c.get(i)? {
        ch if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || !ch.is_ascii() => Some(i + 1),
        '\\' => escape_end(c, i),
        _ => None,
    }
}

fn ident_end(c: &[char], i: usize) -> Option<usize> {
    let start = if c.get(i) == Some(&'-') { i + 1 } else { i };
    let mut j = nmstart_end(c, start)?;
    while let Some(k) = nmchar_end(c, j) {
        j = k;
    }
    Some(j)
}

fn number_end(c: &[char], i: usize) -> Option<usize> {
    let digit = |j: usize| c.get(j).is_some_and(char::is_ascii_digit);
    let mut j = i;
    if matches!(c.get(j), Some('+' | '-')) {
        j += 1;
    }
    let mut d = j;
    while digit(d) {
        d += 1;
    }
    if c.get(d) == Some(&'.') && digit(d + 1) {
        let mut e = d + 1;
        while digit(e) {
            e += 1;
        }
        return Some(e);
    }
    (d > j).then_some(d)
}

/// An identifier's or a hash's value: unicode escapes, then simple ones.
fn unescape(c: &[char]) -> String {
    if c.contains(&'\\') {
        unescape_simple(&unescape_unicode(c))
    } else {
        c.iter().collect()
    }
}

/// `_sub_unicode_escape(_replace_unicode, …)`: `\` and 1 to 6 hex digits (and one white space)
/// to the character; past U+10FFFF, U+FFFD.
fn unescape_unicode(c: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && c.get(i + 1).is_some_and(char::is_ascii_hexdigit) {
            let mut j = i + 1;
            let mut cp: u32 = 0;
            while j < c.len() && j < i + 7 && c[j].is_ascii_hexdigit() {
                cp = cp * 16 + c[j].to_digit(16).unwrap_or(0);
                j += 1;
            }
            // A surrogate, which Python keeps alone in a string, has no Rust character.
            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
            if c.get(j) == Some(&'\r') && c.get(j + 1) == Some(&'\n') {
                j += 2;
            } else if c.get(j).is_some_and(|&w| is_ws(w)) {
                j += 1;
            }
            i = j;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `_sub_simple_escape(_replace_simple, …)`: `\` and any character but a newline to that
/// character. It runs after [`unescape_unicode`], so an escaped backslash escapes again.
fn unescape_simple(c: &[char]) -> String {
    let mut out = String::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && c.get(i + 1).is_some_and(|&n| n != '\n') {
            out.push(c[i + 1]);
            i += 2;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `_sub_newline_escape('', …)`: drops `\` and a newline.
fn drop_newline_escapes(c: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' {
            match c.get(i + 1) {
                Some('\n' | '\x0c') => {
                    i += 2;
                    continue;
                }
                Some('\r') => {
                    i += if c.get(i + 2) == Some(&'\n') { 3 } else { 2 };
                    continue;
                }
                _ => {}
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

pub(super) fn tokenize(s: &str) -> Result<Vec<Tok>, Fail> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if is_ws(c[i]) {
            while i < c.len() && is_ws(c[i]) {
                i += 1;
            }
            out.push(Tok::S);
            continue;
        }
        if let Some(end) = ident_end(&c, i) {
            out.push(Tok::Ident(unescape(&c[i..end])));
            i = end;
            continue;
        }
        if c[i] == '#'
            && let Some(mut end) = nmchar_end(&c, i + 1)
        {
            while let Some(k) = nmchar_end(&c, end) {
                end = k;
            }
            out.push(Tok::Hash(unescape(&c[i + 1..end])));
            i = end;
            continue;
        }
        if c[i] == '"' || c[i] == '\'' {
            let quote = c[i];
            let mut j = i + 1;
            loop {
                match c.get(j) {
                    None => break,
                    Some(&ch) if ch == quote || matches!(ch, '\n' | '\r' | '\x0c') => break,
                    Some('\\') => match c.get(j + 1) {
                        Some('\n' | '\x0c') => j += 2,
                        Some('\r') => j += if c.get(j + 2) == Some(&'\n') { 3 } else { 2 },
                        _ => match escape_end(&c, j) {
                            Some(e) => j = e,
                            None => break,
                        },
                    },
                    Some(_) => j += 1,
                }
            }
            // "Unclosed string" or "Invalid string".
            if c.get(j) != Some(&quote) {
                return Err(Fail::Invalid);
            }
            let raw = drop_newline_escapes(&c[i + 1..j]);
            out.push(Tok::Str(unescape_simple(&unescape_unicode(&raw))));
            i = j + 1;
            continue;
        }
        if let Some(end) = number_end(&c, i) {
            out.push(Tok::Num(c[i..end].iter().collect()));
            i = end;
            continue;
        }
        if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let mut j = i + 2;
            while j < c.len() && !(c[j] == '*' && c.get(j + 1) == Some(&'/')) {
                j += 1;
            }
            i = (j + 2).min(c.len());
            continue;
        }
        out.push(Tok::Delim(c[i]));
        i += 1;
    }
    Ok(out)
}

/// `TokenStream`: the tokens and how many were taken. A function's tokens are read through a
/// stream over their slice of the selector's, never copied.
pub(super) struct Stream<'a> {
    pub(super) toks: &'a [Tok],
    pub(super) at: usize,
}

impl<'a> Stream<'a> {
    pub(super) fn new(toks: &'a [Tok]) -> Stream<'a> {
        Stream { toks, at: 0 }
    }

    pub(super) fn peek(&self) -> &'a Tok {
        self.toks.get(self.at).unwrap_or(&EOF)
    }

    pub(super) fn next(&mut self) -> &'a Tok {
        let t = self.peek();
        self.at += 1;
        t
    }

    pub(super) fn is(&self, c: char) -> bool {
        *self.peek() == Tok::Delim(c)
    }

    pub(super) fn skip_ws(&mut self) {
        if *self.peek() == Tok::S {
            self.at += 1;
        }
    }

    pub(super) fn next_ident(&mut self) -> Result<String, Fail> {
        match self.next() {
            Tok::Ident(v) => Ok(v.clone()),
            _ => Err(Fail::Invalid),
        }
    }

    /// An identifier, or None for `*`.
    pub(super) fn next_ident_or_star(&mut self) -> Result<Option<String>, Fail> {
        match self.next() {
            Tok::Ident(v) => Ok(Some(v.clone())),
            Tok::Delim('*') => Ok(None),
            _ => Err(Fail::Invalid),
        }
    }
}
