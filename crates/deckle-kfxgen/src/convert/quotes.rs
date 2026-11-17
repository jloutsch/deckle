// SPDX-License-Identifier: GPL-3.0-only
//! The marks a `<q>` is printed with: kfxgen's `_q_marks`, `_document_q_marks`, `_q_content`,
//! `_q_pair`, `_css_strings` and `_Q_MARKS` (`converter.py`, at kfxgen `ad38ec8`).
//!
//! The quote depth runs on across the document in reading order, so the marks of every `<q>`
//! are worked out together, once per file ([`document_q_marks`]); the walk looks each up.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::inline_style::Raised;
use crate::dom::{Dom, NodeId};
use crate::generate::py;

/// `_Q_MARKS`: (open, close, nested open, nested close) by a `<q>`'s lang as a whole tag,
/// measured from Kindle Previewer 4.0.3; sorted by lang, for a binary search.
const Q_MARKS: [(&str, &str); 59] = [
    ("af", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("ar", "\u{201d}\u{201c}\u{2019}\u{2018}"),
    ("az-cyrl", "\u{ab}\u{bb}\u{2039}\u{203a}"),
    ("bg", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("bn", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("br", "\u{ab}\u{bb}\u{2039}\u{203a}"),
    ("bs-cyrl", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("ca", "\u{201c}\u{201d}\u{ab}\u{bb}"),
    ("cs", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("da", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("de", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("de-ch", "\u{ab}\u{bb}\u{2039}\u{203a}"),
    ("el", "\u{ab}\u{bb}\u{201c}\u{201d}"),
    ("en", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("en-gb", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("es", "\u{201c}\u{201d}\u{ab}\u{bb}"),
    ("et", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("eu", "\u{201c}\u{201d}\u{ab}\u{bb}"),
    ("fa", "\u{ab}\u{bb}\u{2039}\u{203a}"),
    ("fi", "\u{201d}\u{201d}\u{2019}\u{2019}"),
    ("fr", "\u{ab}\u{bb}\u{ab}\u{bb}"),
    ("fr-ca", "\u{ab}\u{bb}\u{2039}\u{203a}"),
    ("fr-ch", "\u{ab}\u{bb}\u{2039}\u{203a}"),
    ("hi", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("hr", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("hu", "\u{201e}\u{201d}\u{bb}\u{ab}"),
    ("id", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("it", "\u{ab}\u{bb}\u{201c}\u{201d}"),
    ("ja", "\u{300c}\u{300d}\u{300e}\u{300f}"),
    ("ko", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("lt", "\u{201e}\u{201c}\u{201e}\u{201c}"),
    ("lv", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("mk", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("ms", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("nb", "\u{ab}\u{bb}\u{2018}\u{2019}"),
    ("nl", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("nn", "\u{ab}\u{bb}\u{2018}\u{2019}"),
    ("pl", "\u{201e}\u{201d}\u{ab}\u{bb}"),
    ("pt", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("pt-pt", "\u{ab}\u{bb}\u{201c}\u{201d}"),
    ("ro", "\u{201e}\u{201d}\u{ab}\u{bb}"),
    ("ru", "\u{ab}\u{bb}\u{201e}\u{201c}"),
    ("sk", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("sl", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("sq", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("sr", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("sr-latn", "\u{201e}\u{201c}\u{201a}\u{2018}"),
    ("sv", "\u{201d}\u{201d}\u{2019}\u{2019}"),
    ("sw", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("ta", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("te", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("th", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("tr", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("uk", "\u{ab}\u{bb}\u{201e}\u{201c}"),
    ("ur", "\u{201d}\u{201c}\u{2019}\u{2018}"),
    ("vi", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("zh", "\u{201c}\u{201d}\u{2018}\u{2019}"),
    ("zh-hant", "\u{300c}\u{300d}\u{300e}\u{300f}"),
    ("zu", "\u{201c}\u{201d}\u{2018}\u{2019}"),
];
/// `_Q_PLAIN`: what Previewer writes for a lang not in the table, or none.
const Q_PLAIN: &str = "\"\"''";

/// `_CSS_STRING_RE`.
static CSS_STRING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'"#).unwrap());
/// `_CONTENT_TOKEN_RE`.
static CONTENT_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|([a-zA-Z-]+)\(([^)]*)\)|([a-zA-Z-]+)"#)
        .unwrap()
});

const CHR_RANGE: Raised = Raised {
    exception: "ValueError",
    note: "_css_strings: chr() of a CSS escape past U+10FFFF",
};
const EMPTY_NAME: Raised = Raised {
    exception: "ValueError",
    note: "_q_content: lxml's get() of an attr() name that is empty",
};
const BAD_NAME: Raised = Raised {
    exception: "ValueError",
    note: "_q_content: lxml's get() of an attr() name of '{' with no '}'",
};

/// Each `<q>` of a document and its (open, close) marks.
pub(crate) type QMarks = HashMap<NodeId, (String, String)>;

/// A `<q>`'s `q-css` from the resolver: its `quotes`, `::before` and `::after` content, each
/// `None` where kfxgen's dict holds None.
#[derive(Default)]
pub(crate) struct QCss {
    pub quotes: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The most characters a lang in [`Q_MARKS`] has.
const LONGEST_Q_LANG: usize = 7;

/// The four marks of a lang, as `_Q_MARKS.get(lang.lower(), _Q_PLAIN)`. Lowering never makes
/// a string fewer characters (each character lowers to one or more), so a lang longer than
/// the table's longest is not in it, and is not lowered: it can be a megabyte long.
fn marks_of(lang: &str) -> [char; 4] {
    let marks = if lang.chars().nth(LONGEST_Q_LANG).is_some() {
        Q_PLAIN
    } else {
        let lang = lang.to_lowercase();
        match Q_MARKS.binary_search_by(|(k, _)| (*k).cmp(lang.as_str())) {
            Ok(i) => Q_MARKS[i].1,
            Err(_) => Q_PLAIN,
        }
    };
    let mut c = marks.chars();
    std::array::from_fn(|_| c.next().unwrap_or('"'))
}

/// `_css_strings(value)`: the quoted strings in a CSS value, escapes read.
fn css_strings(value: &str) -> Result<Vec<String>, Raised> {
    CSS_STRING
        .captures_iter(value)
        .map(|c| {
            // `a or b`: the double-quoted string, else the single-quoted one.
            let a = c.get(1).map_or("", |m| m.as_str());
            let b = c.get(2).map_or("", |m| m.as_str());
            unescape(if a.is_empty() { b } else { a })
        })
        .collect()
}

/// `_css_strings`' `unescape`: first `re.sub(r"\\([0-9a-fA-F]{1,6})\s?", chr(int(hex, 16)))`,
/// then `re.sub(r"\\(.)", r"\1")` over the result, so a backslash the first gave escapes the
/// character after it, as in kfxgen. `chr` past U+10FFFF raises; a surrogate, which Python's
/// `chr` gives and a Rust string cannot hold, is U+FFFD (DIVERGENCES.md).
fn unescape(text: &str) -> Result<String, Raised> {
    let mut first = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('\\') {
        first.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let hex = after
            .char_indices()
            .take_while(|&(k, c)| k < 6 && c.is_ascii_hexdigit())
            .count();
        if hex == 0 {
            first.push('\\');
            rest = after;
            continue;
        }
        let value = u32::from_str_radix(&after[..hex], 16).unwrap_or(u32::MAX);
        if value > 0x10_FFFF {
            return Err(CHR_RANGE);
        }
        first.push(char::from_u32(value).unwrap_or('\u{fffd}'));
        rest = &after[hex..];
        // `\s?`: one white space character after the digits is part of the escape.
        if let Some(c) = rest.chars().next().filter(|&c| py::is_space(c)) {
            rest = &rest[c.len_utf8()..];
        }
    }
    first.push_str(rest);
    // `\\(.)`: `.` is any character but "\n".
    let mut out = String::with_capacity(first.len());
    let mut chars = first.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some(&n)) if n != '\n' => {
                out.push(n);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// `_q_pair(q, css, depth)`: the (open, close) pair a quote keyword prints at `depth`.
fn q_pair(lang: Option<&str>, css: &QCss, depth: u64) -> Result<(String, String), Raised> {
    let marks = marks_of(lang.unwrap_or(""));
    let pair = if depth > 0 {
        (marks[2].to_string(), marks[3].to_string())
    } else {
        (marks[0].to_string(), marks[1].to_string())
    };
    let raw = css.quotes.as_deref().unwrap_or("");
    if py::strip(raw).to_lowercase() == "none" {
        return Ok((String::new(), String::new()));
    }
    let strings = css_strings(raw)?;
    let pairs = strings.len() / 2;
    if pairs > 0 {
        let k = 2 * usize::try_from(depth).unwrap_or(usize::MAX).min(pairs - 1);
        return Ok((strings[k].clone(), strings[k + 1].clone()));
    }
    Ok(pair)
}

/// lxml's `q.get(name)` for an `attr()` name: an attribute in no namespace, or `{uri}local`;
/// lxml refuses an empty name and a `{` with no `}`.
fn get_attr<'d>(dom: &'d Dom, q: NodeId, name: &str) -> Result<Option<&'d str>, Raised> {
    if let Some(clark) = name.strip_prefix('{') {
        let Some((uri, local)) = clark.split_once('}') else {
            return Err(BAD_NAME);
        };
        if local.is_empty() {
            return Err(EMPTY_NAME);
        }
        if uri.is_empty() {
            return Ok(plain_attr(dom, q, local));
        }
        return Ok(dom.attr_ns(q, uri, local));
    }
    if name.is_empty() {
        return Err(EMPTY_NAME);
    }
    Ok(plain_attr(dom, q, name))
}

/// lxml's `elem.get(name)` for a name with no `{uri}`: the attribute of that name in no
/// namespace.
pub(crate) fn plain_attr<'d>(dom: &'d Dom, n: NodeId, name: &str) -> Option<&'d str> {
    dom.attrs(n)
        .find(|&(k, ns, _)| ns.is_none() && k == name)
        .map(|(_, _, v)| v)
}

/// `_q_content(q, css, side, depth)`: what a `<q>`'s `::before` (`before` true) or `::after`
/// prints, and the quote depth after it.
fn q_content(
    dom: &Dom,
    q: NodeId,
    lang: Option<&str>,
    css: &QCss,
    before: bool,
    mut depth: u64,
) -> Result<(String, u64), Raised> {
    let raw = if before { &css.before } else { &css.after };
    let mut value = py::strip(raw.as_deref().unwrap_or(""));
    if value.is_empty() || value == "normal" {
        value = if before { "open-quote" } else { "close-quote" };
    }
    if value == "none" {
        return Ok((String::new(), depth));
    }
    let tokens: Vec<_> = CONTENT_TOKEN.captures_iter(value).collect();
    let alone = tokens.len() == 1;
    let mut out = String::new();
    for t in &tokens {
        let group = |i: usize| t.get(i).map_or("", |m| m.as_str());
        let (dq, sq, func, arg, word) = (group(1), group(2), group(3), group(4), group(5));
        if !func.is_empty() {
            if func.to_lowercase() == "attr" {
                out.push_str(get_attr(dom, q, py::strip(arg))?.unwrap_or(""));
            }
        } else if !word.is_empty() {
            let word = word.to_lowercase();
            if word == "open-quote" || word == "no-open-quote" {
                if word == "open-quote" && alone && before {
                    out.push_str(&q_pair(lang, css, depth)?.0);
                }
                depth = depth.saturating_add(1);
            } else if (word == "close-quote" || word == "no-close-quote") && depth > 0 {
                depth -= 1;
                if word == "close-quote" && alone && !before {
                    out.push_str(&q_pair(lang, css, depth)?.1);
                }
            }
        } else {
            // A string, its escapes read: `_css_strings` of it quoted again, which finds it
            // whole.
            out.push_str(&unescape(if sq.is_empty() { dq } else { sq })?);
        }
    }
    Ok((out, depth))
}

/// `_document_q_marks(root, css_of)`: (open, close) for every `<q>` of the document, the quote
/// depth running on across it in reading order. `css_of(q)` is the `<q>`'s `q-css` and its
/// nearest `lang` (`_lang_of`), or `Ok(None)` where the resolver gives none. A loop, not
/// recursion.
pub(crate) fn document_q_marks<'l, E: From<Raised>>(
    dom: &Dom,
    mut css_of: impl FnMut(NodeId) -> Result<(Option<QCss>, Option<&'l str>), E>,
) -> Result<QMarks, E> {
    let mut marks = QMarks::new();
    let mut depth = 0;
    // (node, entering): `iterwalk`'s start and end events.
    let mut stack = vec![(dom.root(), true)];
    while let Some((n, entering)) = stack.pop() {
        if entering {
            stack.push((n, false));
            stack.extend(dom.children(n).iter().rev().map(|&c| (c, true)));
        }
        if dom.is_comment(n) || dom.tag(n) != "q" {
            continue;
        }
        let (css, lang) = css_of(n)?;
        let css = css.unwrap_or_default();
        if entering {
            let (text, d) = q_content(dom, n, lang, &css, true, depth)?;
            depth = d;
            marks.insert(n, (text, String::new()));
        } else {
            let (text, d) = q_content(dom, n, lang, &css, false, depth)?;
            depth = d;
            if let Some(m) = marks.get_mut(&n) {
                m.1 = text;
            }
        }
    }
    Ok(marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_for_its_search() {
        assert!(Q_MARKS.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(
            marks_of("ZU"),
            ['\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}']
        );
        assert_eq!(marks_of("en-US"), ['"', '"', '\'', '\'']);
        assert_eq!(
            Q_MARKS.iter().map(|(k, _)| k.chars().count()).max(),
            Some(LONGEST_Q_LANG)
        );
        // A lang as long as the longest is still looked up; one longer is plain.
        assert_eq!(
            marks_of("ZH-HANT"),
            ['\u{300c}', '\u{300d}', '\u{300e}', '\u{300f}']
        );
        assert_eq!(marks_of("zh-hantx"), marks_of("en-US"));
    }

    /// Values from the pinned `_css_strings`, run under Python 3.14.
    #[test]
    fn css_strings_read_escapes_as_kfxgen() {
        let s = |v: &str| css_strings(v).unwrap();
        assert_eq!(s(r#""\201C" '\2018 x'"#), ["\u{201c}", "\u{2018}x"]);
        // `\d` is a hex escape: carriage return.
        assert_eq!(s(r#""a\"b" 'c\\d'"#), ["a\"b", "c\r"]);
        // The first pass gives a backslash, the second reads it as an escape.
        assert_eq!(s(r#""\5C x\5Cy""#), ["xy"]);
        assert_eq!(s(r#""\\41""#), ["A"]);
        assert_eq!(s(r#""\10FFFF\1c x""#), ["\u{10ffff}\u{1c}x"]);
        assert_eq!(s("\"a\\\nb\""), Vec::<String>::new());
        assert_eq!(s("none"), Vec::<String>::new());
        assert_eq!(css_strings(r#""\110000""#), Err(CHR_RANGE));
        // Python gives a lone surrogate (DIVERGENCES.md).
        assert_eq!(s(r#""\D800""#), ["\u{fffd}"]);
    }
}
