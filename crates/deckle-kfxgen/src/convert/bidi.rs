// SPDX-License-Identifier: GPL-3.0-only
//! An inline element's own direction: kfxgen's `_first_strong` and `_inline_bidi`
//! (`converter.py`, at kfxgen `ad38ec8`).
//!
//! Hostile input: `_first_strong` recurses into the children; here it is a loop. Its reads do
//! not overlap: it passes over every element with a `dir` of its own and every `<bdi>`, the only
//! elements the walk asks it of, so each text is read at most once per walk of a block.

use crate::dom::{Dom, NodeId};
use crate::generate::py::{self, Strong, bidi_strong};

/// The namespace of the `xml:` prefix.
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// `_UNICODE_BIDI`: CSS unicode-bidi as Kindle Previewer 4.0.3 writes it.
fn unicode_bidi(value: &str) -> Option<&'static str> {
    Some(match value {
        "embed" => "embed",
        "isolate" => "isolate",
        "bidi-override" => "bidi-override",
        "isolate-override" => "isolate-override",
        "plaintext" => "plaintext",
        "normal" => "normal",
        _ => return None,
    })
}

/// `_first_strong(elem)`: "ltr" or "rtl", from the first letter of `n`'s text that has a
/// direction, passing over a `<bdi>`, `<script>`, `<style>` or element with its own `dir`
/// (their tails count); "ltr" when no letter has one.
pub(crate) fn first_strong(dom: &Dom, n: NodeId) -> &'static str {
    // (node, entering): an element's text and children on entering, its tail on leaving.
    let mut stack = vec![(n, true)];
    while let Some((m, entering)) = stack.pop() {
        let text = if entering {
            stack.push((m, false));
            for &child in dom.children(m).iter().rev() {
                if read_inside(dom, child) {
                    stack.push((child, true));
                } else {
                    // Its tail only.
                    stack.push((child, false));
                }
            }
            dom.text(m)
        } else if m == n {
            // `n`'s own tail is not its text.
            continue;
        } else {
            dom.tail(m)
        };
        let found = text.chars().find_map(|c| match bidi_strong(c)? {
            Strong::L => Some("ltr"),
            Strong::R | Strong::Al => Some("rtl"),
        });
        if let Some(d) = found {
            return d;
        }
    }
    "ltr"
}

/// Whether `_first_strong`'s `texts` reads inside `child`: an element other than a `<bdi>`,
/// `<script>` or `<style>`, with no `dir` of its own (`child.get("dir")` falsy).
fn read_inside(dom: &Dom, child: NodeId) -> bool {
    !dom.is_comment(child)
        && !matches!(dom.tag(child), "bdi" | "script" | "style")
        && dom.attr(child, "dir").is_none_or(str::is_empty)
}

/// `_inline_bidi(elem, css)`'s dict: an inline element's own direction, unicode-bidi and
/// language, each only where the element sets it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct InlineBidi {
    pub direction: Option<&'static str>,
    pub unicode_bidi: Option<&'static str>,
    pub lang: Option<String>,
}

impl InlineBidi {
    /// The (key, value) pairs `make_bidi_flag` takes.
    pub(crate) fn keys(&self) -> Vec<(&str, &str)> {
        let mut keys = Vec::new();
        if let Some(d) = self.direction {
            keys.push(("direction", d));
        }
        if let Some(b) = self.unicode_bidi {
            keys.push(("unicode_bidi", b));
        }
        if let Some(l) = &self.lang {
            keys.push(("lang", l.as_str()));
        }
        keys
    }
}

/// `_inline_bidi(elem, css)`, given `str(css.get("own-direction") or "").strip().lower()` and
/// the same of `own-unicode-bidi` (each "" without a resolved style).
pub(crate) fn inline_bidi(
    dom: &Dom,
    n: NodeId,
    own_direction: &str,
    own_unicode_bidi: &str,
) -> InlineBidi {
    let local = dom.tag(n);
    let attr = py::strip(dom.attr(n, "dir").unwrap_or("")).to_lowercase();
    let mut out = InlineBidi::default();
    if matches!(own_direction, "rtl" | "ltr") {
        out.direction = Some(if own_direction == "rtl" { "rtl" } else { "ltr" });
    } else if matches!(attr.as_str(), "rtl" | "ltr") {
        out.direction = Some(if attr == "rtl" { "rtl" } else { "ltr" });
    } else if attr == "auto" || (local == "bdi" && attr.is_empty()) {
        out.direction = Some(first_strong(dom, n));
    }
    let mut bidi = unicode_bidi(own_unicode_bidi);
    if bidi.is_none() && local == "bdo" {
        bidi = Some("isolate-override");
    } else if bidi.is_none() && (matches!(attr.as_str(), "rtl" | "ltr" | "auto") || local == "bdi")
    {
        bidi = Some("isolate");
    }
    out.unicode_bidi = bidi;
    // `elem.get(_XML_LANG) or elem.get("lang")`: an empty xml:lang reads the lang.
    let lang = dom
        .attr_ns(n, XML_NS, "lang")
        .filter(|l| !l.is_empty())
        .or_else(|| dom.attr(n, "lang"));
    if let Some(lang) = lang.map(py::strip).filter(|l| !l.is_empty()) {
        out.lang = Some(lang.to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(body: &str) -> Dom {
        crate::dom::parse(
            format!("<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>{body}</body></html>")
                .as_bytes(),
        )
        .unwrap()
    }

    fn nth(dom: &Dom, tag: &str, i: usize) -> NodeId {
        dom.descendants()
            .filter(|&n| dom.tag(n) == tag)
            .nth(i)
            .unwrap()
    }

    /// Values from the pinned `_first_strong`, run under Python 3.14 with lxml.
    #[test]
    fn first_strong_reads_as_kfxgen() {
        let d = dom(
            "<p>12 <bdi>abc</bdi><span dir=\"rtl\">abc</span><!--x-->,<b>!<i>\u{5d0}</i></b> \
             a</p><p>123</p><p><span>1<span>2</span></span><bdo>\u{627}</bdo></p>",
        );
        assert_eq!(first_strong(&d, nth(&d, "p", 0)), "rtl");
        assert_eq!(first_strong(&d, nth(&d, "b", 0)), "rtl");
        assert_eq!(first_strong(&d, nth(&d, "p", 1)), "ltr");
        assert_eq!(first_strong(&d, nth(&d, "p", 2)), "rtl");
        assert_eq!(first_strong(&d, nth(&d, "span", 1)), "ltr");
    }
}
