// SPDX-License-Identifier: GPL-3.0-only
//! A paragraph's initial (#240): kfxgen's `_initial`, `_with_initial`, `_first_letter_length`,
//! `_is_float_left` and `_leading_float` (`converter.py`, at kfxgen `ad38ec8`). The block walk
//! calls [`initial`] with a leaf paragraph's normalized text and spans.
//!
//! Lengths are in code points, as Python's. Hostile input: `_leading_float` is a loop with a
//! stack of its own over the start of the paragraph's subtree; `_with_initial` works on whole
//! spans, not on each of the initial's characters (kfxgen makes a set per character, so an
//! initial after a million punctuation marks costs a million sets); the text of a floated
//! element is charged to the file's budget.

use std::sync::LazyLock;

use regex::Regex;

use super::ItemError;
use super::inline::{emphasis_of, text_of, words};
use super::inline_style::{
    BLACK, FLAG_BOLD, FLAG_ITALIC, Flag, Flags, SIZE_FLAG, Span, TYPOGRAPHY_FLAGS, make_size_flag,
};
use super::resolver::{Resolved, Resolver};
use crate::dom::{Dom, NodeId};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// Characters of category P (punctuation) or Z (separators), from the start.
static PUNCTUATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{P}\p{Z}]*").unwrap());

/// `_first_letter_length(text)`: the punctuation before the first letter and the letter, in
/// code points; 0 when the text opens on no letter or digit.
fn first_letter_length(text: &str) -> usize {
    let lead = PUNCTUATION.find(text).map_or(0, |m| m.end());
    match text[lead..].chars().next() {
        // `unicodedata.category(c)[0] in "LN"`.
        Some(c) if py::is_alnum(c) => py::len(&text[..lead]) + 1,
        _ => 0,
    }
}

/// `dict.get(key)` of a `::first-letter` dict.
fn dict_get<'v>(dict: &'v PyValue, key: &str) -> Option<&'v PyValue> {
    let PyValue::Dict(pairs) = dict else {
        return None;
    };
    pairs
        .iter()
        .find(|(k, _)| matches!(k, PyValue::Str(s) if s == key))
        .map(|(_, v)| v)
}

/// `_is_float_left(css)` of a value `css.get("float")` gave.
fn is_float_left(float: Option<&PyValue>) -> bool {
    py::strip(&text_of(float)).to_lowercase() == "left"
}

/// `_with_initial(spans, n, add)`: `spans` with `add` on the first `n` characters, a size in
/// `add` replacing theirs. kfxgen writes each of the `n` characters' flags and merges equal
/// neighbours; the same is done here a span at a time. `spans` are
/// `normalize_runs_with_anchors`' spans, in order and apart, as kfxgen's always are.
fn with_initial(spans: Vec<Span>, n: usize, add: &Flags) -> Vec<Span> {
    let add_size = add.value(SIZE_FLAG).is_some();
    // The first `n` characters as pieces (start, end, flags), the ones no span covers empty.
    let mut head: Vec<(usize, usize, Flags)> = Vec::new();
    let mut rest = Vec::new();
    let mut at = 0;
    for s in spans {
        let end = s.start.saturating_add(s.len);
        let head_end = end.min(n);
        if s.start < head_end {
            if s.start > at {
                head.push((at, s.start, Flags::default()));
            }
            head.push((s.start, head_end, s.flags.clone()));
            at = head_end;
        }
        if end > n {
            let start = s.start.max(n);
            rest.push(Span {
                start,
                len: end - start,
                flags: s.flags,
            });
        }
    }
    if at < n {
        head.push((at, n, Flags::default()));
    }
    let mut out: Vec<Span> = Vec::with_capacity(head.len() + rest.len());
    for (start, end, mut flags) in head {
        if add_size {
            flags.retain(|f| !matches!(f, Flag::Pair(k, _) if k == SIZE_FLAG));
        }
        for f in add.iter() {
            flags.insert(f.clone());
        }
        match out.last_mut() {
            Some(last) if last.flags == flags => last.len += end - start,
            _ => out.push(Span {
                start,
                len: end - start,
                flags,
            }),
        }
    }
    let mut rest = rest.into_iter().peekable();
    if let (Some(last), Some(first)) = (out.last_mut(), rest.peek())
        && last.start + last.len == first.start
        && last.flags == first.flags
    {
        last.len += first.len;
        rest.next();
    }
    out.extend(rest);
    out
}

/// `_leading_float(node, style_resolver)`'s element: the element floated left that `node`'s
/// text opens on, with no text, picture or other element's text before it; None when there is
/// none. A loop: kfxgen recurses into each first child.
fn leading_float(
    dom: &Dom,
    resolver: &Resolver,
    node: NodeId,
) -> Result<Option<NodeId>, ItemError> {
    if !py::strip(dom.text(node)).is_empty() {
        return Ok(None);
    }
    // (element, its next child): kfxgen's call frames.
    let mut stack = vec![(node, 0usize)];
    while let Some(&mut (n, ref mut next)) = stack.last_mut() {
        let Some(&child) = dom.children(n).get(*next) else {
            // Nothing found inside `n`: its parent goes on after its tail.
            stack.pop();
            if !stack.is_empty() && !py::strip(dom.tail(n)).is_empty() {
                return Ok(None);
            }
            continue;
        };
        *next += 1;
        if !dom.is_comment(child) {
            if matches!(dom.tag(child), "img" | "svg") {
                return Ok(None);
            }
            let css = resolver.resolve(child)?;
            let float = match &css {
                Some(css) => css.get("float")?,
                None => None,
            };
            if is_float_left(float) {
                return Ok(Some(child));
            }
            if !py::strip(dom.text(child)).is_empty() {
                return Ok(None);
            }
            // Its tail is read when its search is over.
            stack.push((child, 0));
            continue;
        }
        if !py::strip(dom.tail(child)).is_empty() {
            return Ok(None);
        }
    }
    Ok(None)
}

/// `_initial(elem, css, text, spans, style_resolver)`: a paragraph's spans with its
/// `::first-letter` CSS on the letter, and how many characters (code points) a drop cap takes,
/// or None. A drop cap is a `::first-letter` floated left, or an element floated left that the
/// text opens on (`<span class="dropcap">O</span>nce`). `css` is the paragraph's resolved
/// style. kfxgen raises where `_css_emphasis` does on the `::first-letter` CSS.
pub fn initial(
    dom: &Dom,
    resolver: &Resolver,
    elem: NodeId,
    css: Option<&Resolved>,
    text: &str,
    spans: Vec<Span>,
) -> Result<(Vec<Span>, Option<usize>), ItemError> {
    let first = match css {
        Some(css) => css.get("first-letter")?,
        None => None,
    };
    if let Some(first) = first.filter(|f| f.is_truthy()) {
        let n = first_letter_length(text);
        if n == 0 {
            return Ok((spans, None));
        }
        // The dict's values were charged when it was read.
        let e = emphasis_of(|k| Ok(dict_get(first, k)))?;
        let mut add = Flags::default();
        if let Some(size) = e.size {
            add.insert(make_size_flag(size));
        }
        // Its colour, and the rest, as a run of its own (#239). Black, the reader's own, isn't.
        for (kind, value) in TYPOGRAPHY_FLAGS.iter().zip(e.typography) {
            if let Some(value) = value
                && value != PyValue::Int(BLACK)
            {
                add.insert(Flag::Pair((*kind).to_owned(), value));
            }
        }
        if e.bold {
            add.insert(Flag::Str(FLAG_BOLD.to_owned()));
        }
        if e.italic {
            add.insert(Flag::Str(FLAG_ITALIC.to_owned()));
        }
        let spans = if add.is_empty() {
            spans
        } else {
            with_initial(spans, n, &add)
        };
        let drop = is_float_left(dict_get(first, "float")).then_some(n);
        return Ok((spans, drop));
    }
    let Some(found) = leading_float(dom, resolver, elem)? else {
        return Ok((spans, None));
    };
    let shown = dom.itertext(found);
    resolver.charge(shown.len())?;
    let letters = words(&shown);
    if !letters.is_empty() && text.starts_with(&letters) {
        return Ok((spans, Some(py::len(&letters))));
    }
    Ok((spans, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_letters_count_their_punctuation() {
        assert_eq!(first_letter_length("Once"), 1);
        assert_eq!(first_letter_length("\u{201c}Once"), 2);
        assert_eq!(first_letter_length("\u{2014} \u{ab}\u{a0}a"), 5);
        assert_eq!(first_letter_length("1999"), 1);
        assert_eq!(first_letter_length(""), 0);
        assert_eq!(first_letter_length("\u{201c}"), 0);
        // A symbol (S) is neither punctuation nor a letter.
        assert_eq!(first_letter_length("$5"), 0);
        assert_eq!(first_letter_length("\u{1F600}a"), 0);
    }
}
