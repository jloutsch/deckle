// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's href helpers (`converter.py` `_is_unsafe_href`, `_normalize_href`,
//! `_resolve_doc_path`, `_key_doc`, `_resolve_link_target`, `_external_link_target`), and
//! Python's `urllib.parse.unquote`, which they all go through.
//!
//! Hrefs come from a stranger's book. None of these panics, each is linear in its input
//! (`is_unsafe` decodes at most 16 times), and [`resolve_doc_path`] never gives a path that
//! starts with `/` or `..`. The keys they give are names to match, never paths that are opened.
//! kfxgen logs each rejected href to its security log; the port has no such log.

use crate::epub::package::{dirname, normpath, posix_join};
use crate::generate::py;

/// `inline_style.EXTERNAL_LINK_PREFIX`: a link that leaves the book.
const EXTERNAL_LINK_PREFIX: &str = "url:";

/// Python's `urllib.parse.unquote(s)`: each run of ASCII characters has its `%xx` escapes
/// (two hex digits, either case) decoded to bytes, which are read as UTF-8 with each invalid
/// sequence one U+FFFD (`errors="replace"`); other characters stay as they are.
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let ascii = rest.find(|c: char| !c.is_ascii()).unwrap_or(rest.len());
        let (run, tail) = rest.split_at(ascii);
        if run.contains('%') {
            out.push_str(&String::from_utf8_lossy(&unquote_to_bytes(run.as_bytes())));
        } else {
            out.push_str(run);
        }
        let other = tail.find(|c: char| c.is_ascii()).unwrap_or(tail.len());
        out.push_str(&tail[..other]);
        rest = &tail[other..];
    }
    out
}

/// `urllib.parse._unquote_impl` on ASCII bytes.
fn unquote_to_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            )
        {
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// `_is_unsafe_href`: whether `href`, as written or decoded until it stops changing (at most
/// 16 times), starts with `/` or `\`, with a drive letter (`C:`), holds `://` or starts with
/// `javascript:`, `data:` or `file:` in any case, or has a `..` part between `/` or `\`.
pub fn is_unsafe(href: &str) -> bool {
    if href.is_empty() {
        return false;
    }
    let original_len = py::len(href).max(1);
    let mut decoded = href.to_owned();
    for _ in 0..16 {
        let next = unquote(&decoded);
        if next == decoded {
            break;
        }
        // Decoding never makes a string longer; kfxgen guards against it anyway.
        if py::len(&next) > original_len.saturating_mul(4) {
            return true;
        }
        decoded = next;
    }
    [href, decoded.as_str()].into_iter().any(|candidate| {
        let mut chars = candidate.chars();
        if candidate.starts_with(['/', '\\'])
            || matches!((chars.next(), chars.next()), (Some(a), Some(':')) if a.is_ascii_alphabetic())
        {
            return true;
        }
        // Python's `lower()`; only ASCII letters and U+212A lower to ASCII, and neither changes
        // whether these prefixes match.
        let lowered = candidate.to_lowercase();
        if lowered.contains("://")
            || ["javascript:", "data:", "file:"]
                .iter()
                .any(|p| lowered.starts_with(p))
        {
            return true;
        }
        candidate.split(['/', '\\']).any(|part| part == "..")
    })
}

/// `_normalize_href`: the file name of `href` without its fragment, or "" when `href` or that
/// name is unsafe.
pub fn normalize(href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    if href.is_empty() || is_unsafe(href) {
        return String::new();
    }
    let name = href.rsplit(['/', '\\']).next().unwrap_or("");
    if is_unsafe(name) {
        return String::new();
    }
    name.to_owned()
}

/// `_resolve_doc_path`: `href` from the document `base_href` as a path from the book's root,
/// `posixpath`'s way, backslashes read as slashes. "" when `href` is unsafe (a leading `..` is
/// let through to be resolved) or the result leaves the book (`..`, `/`) or is the root (`.`).
/// An empty `href` is the document itself.
pub fn resolve_doc_path(base_href: &str, href: &str) -> String {
    if !href.is_empty() && is_unsafe(href) && !href.starts_with("..") {
        return String::new();
    }
    let base = base_href.replace('\\', "/");
    let joined = if href.is_empty() {
        base
    } else {
        posix_join(dirname(&base), &href.replace('\\', "/"))
    };
    let resolved = normpath(&joined);
    if resolved.starts_with("..") || resolved.starts_with('/') || resolved == "." {
        return String::new();
    }
    resolved
}

/// `_key_doc`: [`resolve_doc_path`], decoded once: the file part of an anchor or link key.
pub fn key_doc(base_href: &str, href: &str) -> String {
    unquote(&resolve_doc_path(base_href, href))
}

/// `_href_fragment`: what follows the first `#`, or "".
fn fragment(href: &str) -> &str {
    href.split_once('#').map_or("", |(_, f)| f)
}

/// `_URL_SCHEME_RE`: `^[a-zA-Z][a-zA-Z0-9+.\-]*:`.
fn has_scheme(href: &str) -> bool {
    let mut b = href.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphabetic())
        && b.find(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))) == Some(b':')
}

/// `_resolve_link_target`: an in-book `<a href>` from the document `base_href` as a
/// `"<file>#<fragment>"` key (the fragment decoded, and left off when empty), or None for an
/// empty href, one with a URL scheme, or one [`key_doc`] refuses. A bare `#frag` names
/// `base_href` itself, and nothing when `base_href` is empty.
pub fn resolve_link_target(href: &str, base_href: &str) -> Option<String> {
    let href = py::strip(href);
    if href.is_empty() || has_scheme(href) {
        return None;
    }
    let frag = unquote(fragment(href));
    let file_part = href.split('#').next().unwrap_or("");
    let target = if !file_part.is_empty() {
        key_doc(base_href, file_part)
    } else if !base_href.is_empty() {
        key_doc(base_href, "")
    } else {
        String::new()
    };
    if target.is_empty() {
        return None;
    }
    Some(if frag.is_empty() {
        target
    } else {
        format!("{target}#{frag}")
    })
}

/// Whether `s` starts with `word` (lower-case ASCII) as Python's `re.I` matches it: each
/// letter in either case, and also `İ` and `ı` for `i` and `ſ` for `s`.
fn starts_with_re_i(s: &str, word: &str) -> bool {
    let mut chars = s.chars();
    word.chars().all(|p| {
        chars.next().is_some_and(|c| {
            c.to_ascii_lowercase() == p
                || matches!((p, c), ('i', '\u{130}' | '\u{131}') | ('s', '\u{17F}'))
        })
    })
}

/// `_external_link_target`: an `http:`, `https:` or `mailto:` href (`re.I`), stripped, as a
/// link target out of the book; None for any other.
pub fn external_target(href: &str) -> Option<String> {
    let href = py::strip(href);
    ["http:", "https:", "mailto:"]
        .iter()
        .any(|p| starts_with_re_i(href, p))
        .then(|| format!("{EXTERNAL_LINK_PREFIX}{href}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_is_pythons() {
        assert_eq!(unquote("a%20b"), "a b");
        assert_eq!(unquote("%7e%7E"), "~~");
        // Not two hex digits: kept, `+` among them (`int(..., 16)` would take "+1").
        assert_eq!(unquote("%+1%g0%4"), "%+1%g0%4");
        // Invalid UTF-8 is one U+FFFD per bad sequence, Python's "replace".
        assert_eq!(unquote("%ff"), "\u{FFFD}");
        assert_eq!(unquote("%e2%82"), "\u{FFFD}");
        assert_eq!(unquote("%f0%80"), "\u{FFFD}\u{FFFD}");
        assert_eq!(unquote("%C3%A9"), "é");
        // A character outside ASCII ends a run: the bytes on either side are not joined.
        assert_eq!(unquote("%C3é%A9"), "\u{FFFD}é\u{FFFD}");
        assert_eq!(unquote("%00"), "\0");
    }

    #[test]
    fn link_schemes_fold_as_python_re_i_does() {
        assert_eq!(external_target(" HTTPS://x "), Some("url:HTTPS://x".into()));
        assert_eq!(external_target("httpſ://x"), Some("url:httpſ://x".into()));
        assert_eq!(external_target("maİlto:a@b"), Some("url:maİlto:a@b".into()));
        assert_eq!(external_target("tel:1"), None);
        assert_eq!(external_target("javascript:x"), None);
        assert_eq!(external_target("http"), None);
    }

    #[test]
    fn link_targets() {
        let t = |h, b| resolve_link_target(h, b);
        assert_eq!(
            t("#fn%3A1", "text/c1.xhtml"),
            Some("text/c1.xhtml#fn:1".into())
        );
        assert_eq!(
            t("notes.xhtml#n1", "text/c1.xhtml"),
            Some("text/notes.xhtml#n1".into())
        );
        assert_eq!(
            t("../b/x%20y.xhtml", "a/c.xhtml"),
            Some("b/x y.xhtml".into())
        );
        assert_eq!(t("#x", ""), None);
        assert_eq!(t("mailto:a", "c.xhtml"), None);
        assert_eq!(t("  ", "c.xhtml"), None);
        assert_eq!(t("c.xhtml#", "c.xhtml"), Some("c.xhtml".into()));
    }
}
