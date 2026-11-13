// SPDX-License-Identifier: GPL-3.0-only
//! The contents files: the EPUB 3 nav as Calibre turns it into an NCX
//! (`conversion/plugins/epub_input.py` `convert_epub3_nav`, `landmarks_to_guide`) and the NCX's
//! nav points (`ebooks/oeb/reader.py` `_toc_from_navpoint`).

use deckle_limits::MAX_DEPTH;
use deckle_limits::roxmltree::{Document, Node};

use super::package::string_value;

const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";
const EPUB_NS: &str = "http://www.idpf.org/2007/ops";
const NCX_NS: &str = "http://www.daisy.org/z3986/2005/ncx/";

/// An NCX nav point: its label's text, its `content/@src`, its children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NavPoint {
    pub label: String,
    pub src: Option<String>,
    pub children: Vec<NavPoint>,
}

/// An HTML element `name`. Calibre parses HTML into the XHTML namespace, so an element with no
/// namespace counts as XHTML too.
pub(crate) fn is_html(n: &Node, name: &str) -> bool {
    n.is_element()
        && n.tag_name().name() == name
        && matches!(n.tag_name().namespace(), None | Some(XHTML_NS))
}

fn epub_type<'a>(n: &Node<'a, '_>) -> Option<&'a str> {
    n.attribute((EPUB_NS, "type"))
}

fn first_child<'a, 'i>(n: &Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is_html(c, name))
}

/// `convert_epub3_nav`: the nav points of the first `<nav epub:type="toc">` that has an `<ol>`
/// child, or None when there is none (Calibre then keeps the book's NCX). `nav_name` is the nav
/// file's name, which a fragment-only href is given.
pub(crate) fn nav_points(doc: &Document, nav_name: &str) -> Option<Vec<NavPoint>> {
    // `add_from_li`.
    fn from_li(li: &Node, nav_name: &str) -> NavPoint {
        let mut np = NavPoint::default();
        if let Some(x) = li
            .children()
            .find(|c| is_html(c, "a") || is_html(c, "span"))
        {
            let text = string_value(&x).trim().to_owned();
            np.label = if text.is_empty() {
                let titles: Vec<&str> = x
                    .descendants()
                    .filter(|d| d.is_element())
                    .filter_map(|d| d.attribute("title"))
                    .collect();
                titles.join(" ").trim().to_owned()
            } else {
                text
            };
            np.src = x.attribute("href").filter(|h| !h.is_empty()).map(|h| {
                if h.starts_with('#') {
                    format!("{nav_name}{h}")
                } else {
                    h.to_owned()
                }
            });
        }
        np
    }
    // `process_nav_node`.
    fn walk(ol: &Node, nav_name: &str, depth: usize) -> Vec<NavPoint> {
        if depth >= MAX_DEPTH {
            return Vec::new();
        }
        ol.children()
            .filter(|c| is_html(c, "li"))
            .map(|li| {
                let mut np = from_li(&li, nav_name);
                if let Some(ol) = first_child(&li, "ol") {
                    np.children = walk(&ol, nav_name, depth + 1);
                }
                np
            })
            .collect()
    }
    doc.descendants()
        .filter(|n| is_html(n, "nav") && epub_type(n) == Some("toc"))
        .find_map(|nav| first_child(&nav, "ol"))
        .map(|ol| walk(&ol, nav_name, 0))
}

/// A landmark of the nav: its first `epub:type`, href and text.
pub(crate) struct Landmark {
    pub epub_type: String,
    pub href: String,
    pub title: String,
}

/// `landmarks_to_guide`: the links of every `<nav epub:type="landmarks">` that have an href
/// and an `epub:type`, in document order.
pub(crate) fn landmarks(doc: &Document) -> Vec<Landmark> {
    doc.descendants()
        .filter(|n| is_html(n, "nav") && epub_type(n) == Some("landmarks"))
        .flat_map(|nav| nav.descendants().filter(|a| is_html(a, "a")))
        .filter_map(|a| {
            let href = a.attribute("href").filter(|h| !h.is_empty())?;
            let first = epub_type(&a)?.split_whitespace().next()?;
            Some(Landmark {
                epub_type: first.to_owned(),
                href: href.to_owned(),
                title: string_value(&a).trim().to_owned(),
            })
        })
        .collect()
}

/// The nav points of every `navMap` of an NCX (`_toc_from_ncx`).
pub(crate) fn ncx_points(doc: &Document) -> Vec<NavPoint> {
    fn is_ncx(n: &Node, name: &str) -> bool {
        n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(NCX_NS)
    }
    fn walk(parent: &Node, depth: usize) -> Vec<NavPoint> {
        if depth >= MAX_DEPTH {
            return Vec::new();
        }
        parent
            .children()
            .filter(|c| is_ncx(c, "navPoint"))
            .map(|np| NavPoint {
                // `ncx:navLabel/ncx:text/text()`, joined.
                label: np
                    .children()
                    .filter(|c| is_ncx(c, "navLabel"))
                    .flat_map(|l| l.children().filter(|t| is_ncx(t, "text")))
                    .flat_map(|t| t.children().filter(|x| x.is_text()))
                    .filter_map(|x| x.text())
                    .collect(),
                // `ncx:content/@src`, the first.
                src: np
                    .children()
                    .filter(|c| is_ncx(c, "content"))
                    .find_map(|c| c.attribute("src"))
                    .map(str::to_owned),
                children: walk(&np, depth + 1),
            })
            .collect()
    }
    let root = doc.root_element();
    root.children()
        .filter(|c| is_ncx(c, "navMap"))
        .flat_map(|m| walk(&m, 0))
        .collect()
}
