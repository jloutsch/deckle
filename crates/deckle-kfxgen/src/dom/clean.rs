// SPDX-License-Identifier: GPL-3.0-only
//! The tree as `ebooks/oeb/parse_utils.py` `parse_html` leaves it after its parse: names in
//! lower case for HTML 4, a root that is not `<html>` wrapped, the XHTML namespace forced,
//! several heads and bodies merged, a head with a title and Calibre's charset `<meta>`, a body,
//! Microsoft Office elements made spans, empty links and `<b>`, `<i>`, `<u>` removed, a `<br>`
//! with content made a `<div>`, and the head's whitespace reset.

use std::collections::BTreeMap;

use super::{Attr, Dom, NodeId, XHTML_NS};

/// `parse_html` from its parse on. `has_html4_doctype`: the DOCTYPE before `<html>` names
/// HTML 4.0.
pub(super) fn clean(mut dom: Dom, has_html4_doctype: bool) -> Dom {
    lower_case(&mut dom, has_html4_doctype);
    let root = dom.root;
    if dom.tag(root) != "html" {
        // `non_html_file_tags={'ncx'}` (`base.py` `_parse_xhtml`): read as plain XML.
        if dom.tag(root) == "ncx" {
            return dom;
        }
        wrap_root(&mut dom);
    }
    force_namespace(&mut dom);
    merge_heads_and_bodies(&mut dom);
    ensure_head(&mut dom);
    ensure_body(&mut dom);
    office_to_span(&mut dom);
    remove_empty(&mut dom);
    for n in xhtml(&dom, "br") {
        if !dom.children(n).is_empty() || !dom.text(n).is_empty() {
            dom.rename(n, "div");
        }
    }
    format_head(&mut dom);
    dom
}

/// The elements named `tag` in the XHTML namespace, in document order (`//h:tag`).
fn xhtml(dom: &Dom, tag: &str) -> Vec<NodeId> {
    dom.descendants()
        .filter(|&n| dom.is_xhtml(n, tag))
        .collect()
}

/// The root's children named `tag` in the XHTML namespace, when the root is XHTML `<html>`
/// (`/h:html/h:tag`).
fn top(dom: &Dom, tag: &str) -> Vec<NodeId> {
    if !dom.is_xhtml(dom.root, "html") {
        return Vec::new();
    }
    dom.children(dom.root)
        .iter()
        .copied()
        .filter(|&c| dom.is_xhtml(c, tag))
        .collect()
}

/// "Lower case all tag and attribute names" for an HTML 4 DOCTYPE, an un-namespaced `HTML`
/// root, or a root whose last child has a `LANG` or `DIR`. lxml's last child (`root[-1]`) can be
/// a comment, which has neither. The root's attributes stay.
fn lower_case(dom: &mut Dom, has_html4_doctype: bool) {
    let root = dom.root;
    let last_lang_dir = dom.children(root).last().is_some_and(|&c| {
        [dom.attr(c, "LANG"), dom.attr(c, "DIR")]
            .into_iter()
            .any(|v| v.is_some_and(|v| !v.is_empty()))
    });
    let upper_root = dom.namespace(root).is_none() && dom.tag(root) == "HTML";
    if !(has_html4_doctype || upper_root || last_lang_dir) {
        return;
    }
    let lower = dom.tag(root).to_lowercase();
    dom.rename(root, &lower);
    let all: Vec<NodeId> = dom.descendants().skip(1).collect();
    for n in all {
        let lower = dom.tag(n).to_lowercase();
        dom.rename(n, &lower);
        // Deleted and set again under the lower-case name, in order: a name seen twice keeps
        // its first place and its last value.
        let old = std::mem::take(&mut dom.el_mut(n).attrs);
        let mut attrs: Vec<Attr> = Vec::with_capacity(old.len());
        let mut at: BTreeMap<u32, usize> = BTreeMap::new();
        for mut a in old {
            let lower = dom.name(a.name).to_lowercase();
            a.name = dom.intern(&lower);
            match at.get(&a.name) {
                Some(&i) => attrs[i].value = a.value,
                None => {
                    at.insert(a.name, attrs.len());
                    attrs.push(a);
                }
            }
        }
        dom.el_mut(n).attrs = attrs;
    }
}

/// "does not appear to be (X)HTML": a new un-namespaced `<html>`, with a `<body>` unless the
/// old root has a `body` child, and every node of the old tree (`data.iter()`: comments too),
/// the root first, moved under it one after another in document order, each with its text and
/// tail and no children.
fn wrap_root(dom: &mut Dom) {
    let old = dom.root;
    let has_body = dom.children(old).iter().any(|&c| dom.tag(c) == "body");
    let order: Vec<NodeId> = dom.subtree(old).collect();
    let html = dom.alloc("html", None, Vec::new());
    dom.root = html;
    let parent = if has_body {
        html
    } else {
        let body = dom.alloc("body", None, Vec::new());
        dom.attach(html, body);
        body
    };
    for n in order {
        dom.take_children(n);
        dom.attach(parent, n);
    }
}

/// "Force into the XHTML namespace": an un-namespaced root and every element with no namespace
/// go into XHTML; a root in another namespace (OEB 1's, say) is replaced by an XHTML `<html>`
/// with its attributes and children, its namespace's elements renamed into XHTML.
fn force_namespace(dom: &mut Dom) {
    let root = dom.root;
    let xhtml = dom.intern(XHTML_NS);
    match dom.el(root).ns {
        super::NO_NS => {
            for n in dom.descendants().collect::<Vec<_>>() {
                if dom.el(n).ns == super::NO_NS {
                    dom.el_mut(n).ns = xhtml;
                }
            }
        }
        ns if ns != xhtml => {
            for n in dom.descendants().skip(1).collect::<Vec<_>>() {
                if dom.el(n).ns == ns {
                    dom.el_mut(n).ns = xhtml;
                }
            }
            let attrs = dom.el(root).attrs.clone();
            let html = dom.alloc("html", Some(XHTML_NS), attrs);
            for c in dom.take_children(root) {
                dom.attach(html, c);
            }
            dom.root = html;
        }
        _ => {}
    }
}

/// `merge_multiple_html_heads_and_bodies`: with more than one head or body anywhere, the root
/// keeps only a new head holding every head's children and a new body holding every body's.
fn merge_heads_and_bodies(dom: &mut Dom) {
    let (heads, bodies) = (xhtml(dom, "head"), xhtml(dom, "body"));
    if heads.len() < 2 && bodies.len() < 2 {
        return;
    }
    let root = dom.root;
    dom.take_children(root);
    let head = dom.alloc("head", Some(XHTML_NS), Vec::new());
    let body = dom.alloc("body", Some(XHTML_NS), Vec::new());
    for (into, from) in [(head, heads), (body, bodies)] {
        for h in from {
            for c in dom.take_children(h) {
                dom.attach(into, c);
            }
        }
    }
    dom.attach(root, head);
    dom.attach(root, body);
}

fn new_title(dom: &mut Dom, head: NodeId) {
    let title = dom.alloc("title", Some(XHTML_NS), Vec::new());
    dom.set_text(title, "Unknown");
    dom.attach(head, title);
}

/// "Ensure has a <head/>" with a title that is not blank, and Calibre's charset `<meta>` in
/// place of the file's.
fn ensure_head(dom: &mut Dom) {
    let root = dom.root;
    let head = match top(dom, "head").first() {
        Some(&h) => {
            if !top(dom, "head")
                .iter()
                .any(|&h| dom.children(h).iter().any(|&c| dom.is_xhtml(c, "title")))
            {
                new_title(dom, h);
            }
            h
        }
        None => {
            let h = dom.alloc("head", Some(XHTML_NS), Vec::new());
            dom.attach_first(root, h);
            new_title(dom, h);
            h
        }
    };
    let first_title = top(dom, "head")
        .iter()
        .flat_map(|&h| dom.children(h).to_vec())
        .find(|&c| dom.is_xhtml(c, "title"));
    if let Some(t) = first_title
        && dom.text(t).trim().is_empty()
    {
        dom.set_text(t, "Unknown");
    }
    // `META_XP`; lxml's `remove` takes the tail with the element.
    for h in top(dom, "head") {
        let metas: Vec<NodeId> = dom
            .children(h)
            .iter()
            .copied()
            .filter(|&c| {
                dom.is_xhtml(c, "meta") && dom.attr(c, "http-equiv") == Some("Content-Type")
            })
            .collect();
        for m in metas {
            dom.detach_with_tail(m);
        }
    }
    let attrs = [
        ("http-equiv", "Content-Type"),
        ("content", "text/html; charset=utf-8"),
    ]
    .into_iter()
    .map(|(name, value)| dom.new_attr(name, None, value.to_owned()))
    .collect();
    let meta = dom.alloc("meta", Some(XHTML_NS), attrs);
    dom.attach(head, meta);
}

/// "Ensure has a <body/>": the first body anywhere moved to the end of the root, or a new one.
fn ensure_body(dom: &mut Dom) {
    if !top(dom, "body").is_empty() {
        return;
    }
    let root = dom.root;
    let body = match xhtml(dom, "body").first() {
        Some(&b) => {
            dom.detach_with_tail(b);
            b
        }
        None => dom.alloc("body", Some(XHTML_NS), Vec::new()),
    };
    dom.attach(root, body);
}

/// "Remove microsoft office markup": an element whose namespaced name holds `microsoft-com`
/// becomes an XHTML `<span>`.
fn office_to_span(dom: &mut Dom) {
    let office: Vec<NodeId> = dom
        .descendants()
        .skip(1)
        .filter(|&n| {
            dom.tag(n).contains("microsoft-com")
                || dom
                    .namespace(n)
                    .is_some_and(|ns| ns.contains("microsoft-com"))
        })
        .collect();
    for n in office {
        dom.rename(n, "span");
        dom.set_namespace(n, Some(XHTML_NS));
    }
}

/// "Remove hyperlinks with no content … Also remove empty <b>, <u> and <i> tags": each
/// `a[@href]`, `i`, `b`, `u` with no `id` or `name`, no child and no text is removed, its tail
/// kept (`remove_elem`). Calibre checks and removes them one at a time in document order; only
/// the removal of an element's own children could change what is checked, and those come after
/// it, so they are all chosen first and removed in one pass ([`Dom::remove_many`]).
fn remove_empty(dom: &mut Dom) {
    let empty: Vec<NodeId> = dom
        .descendants()
        .filter(|&n| {
            ((dom.is_xhtml(n, "a") && dom.attr(n, "href").is_some())
                || ["i", "b", "u"].iter().any(|t| dom.is_xhtml(n, t)))
                && dom.attr(n, "id").is_none()
                && dom.attr(n, "name").is_none()
                && dom.children(n).is_empty()
                && dom.text(n).is_empty()
        })
        .collect();
    dom.remove_many(&empty);
}

/// "Remove any stray text in the <head> section and format it nicely".
fn format_head(dom: &mut Dom) {
    let root = dom.root;
    dom.set_text(root, "\n  ");
    let Some(&head) = xhtml(dom, "head").first() else {
        return;
    };
    dom.set_text(head, "\n    ");
    dom.set_tail(head, "\n  ");
    let kids = dom.children(head).to_vec();
    for &c in &kids {
        dom.set_tail(c, "\n    ");
    }
    if let Some(&last) = kids.last() {
        dom.set_tail(last, "\n  ");
    }
}
