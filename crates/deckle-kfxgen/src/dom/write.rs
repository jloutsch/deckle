// SPDX-License-Identifier: GPL-3.0-only
//! A [`Dom`] written as XHTML in the shape of lxml's `etree.tostring(root, encoding="unicode")`,
//! which wrote the `cascade/<book>/dom/NNN.xhtml` dumps: no XML declaration, an empty element
//! as `<tag/>`, a namespace declared where an element's differs from its parent's.

use std::collections::BTreeMap;

use super::{Dom, NodeId, XML_NS};

/// `dom` as XHTML text.
pub fn to_xhtml(dom: &Dom) -> String {
    // The prefixes of namespaced attributes, declared on the root with the first URI each is
    // seen with; one seen with another URI is declared where it is used.
    let mut prefixes: BTreeMap<&str, &str> = BTreeMap::new();
    for n in dom.descendants() {
        for (name, ns, _) in dom.attrs(n) {
            if let (Some((p, _)), Some(ns)) = (name.split_once(':'), ns)
                && ns != XML_NS
            {
                prefixes.entry(p).or_insert(ns);
            }
        }
    }

    let mut out = String::new();
    let root = dom.root();
    // (element, entering)
    let mut stack: Vec<(NodeId, bool)> = vec![(root, true)];
    while let Some((n, entering)) = stack.pop() {
        // A comment is not written, its tail is.
        if dom.is_comment(n) {
            escape(&mut out, dom.tail(n), false);
            continue;
        }
        if !entering {
            out.push_str("</");
            out.push_str(dom.tag(n));
            out.push('>');
            escape(&mut out, dom.tail(n), false);
            continue;
        }
        out.push('<');
        out.push_str(dom.tag(n));
        let ns = dom.namespace(n);
        let parent_ns = dom.parent(n).and_then(|p| dom.namespace(p));
        if n == root || ns != parent_ns {
            match ns {
                Some(ns) => attr(&mut out, "xmlns", ns),
                None if n != root => attr(&mut out, "xmlns", ""),
                None => {}
            }
        }
        if n == root {
            for (p, ns) in &prefixes {
                attr(&mut out, &format!("xmlns:{p}"), ns);
            }
        }
        for (name, ns, value) in dom.attrs(n) {
            if n != root
                && let (Some((p, _)), Some(ns)) = (name.split_once(':'), ns)
                && ns != XML_NS
                && prefixes.get(p) != Some(&ns)
            {
                attr(&mut out, &format!("xmlns:{p}"), ns);
            }
            attr(&mut out, name, value);
        }
        if dom.is_empty(n) {
            out.push_str("/>");
            if n != root {
                escape(&mut out, dom.tail(n), false);
            }
            continue;
        }
        out.push('>');
        escape(&mut out, dom.text(n), false);
        stack.push((n, false));
        stack.extend(dom.children(n).iter().rev().map(|&c| (c, true)));
    }
    out
}

fn attr(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    escape(out, value, true);
    out.push('"');
}

/// lxml's escaping: `&`, `<`, `>` always; in an attribute also `"` and the line breaks and tab
/// that attribute normalization would turn into spaces.
fn escape(out: &mut String, s: &str, in_attr: bool) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if in_attr => out.push_str("&quot;"),
            '\n' if in_attr => out.push_str("&#10;"),
            '\r' if in_attr => out.push_str("&#13;"),
            '\t' if in_attr => out.push_str("&#9;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}
