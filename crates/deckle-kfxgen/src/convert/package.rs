// SPDX-License-Identifier: GPL-3.0-only
//! What kfxgen reads in the OPF Calibre rewrote (`content.opf` at the root of the folder
//! Calibre unpacked the book into) and in the nav and NCX that OPF names (`converter.py`
//! `_opf_raw_dates`, `_spine_in_order`, `_book_files`, `_nav_links`, `_book_landmarks`,
//! `_book_page_list`, `_with_ncx_empty_labels`).
//!
//! kfxgen parses those files with lxml's XML parser, not in recover mode, and treats a file it
//! cannot parse as missing; so does the port, through `deckle_limits`, a file past its limits
//! included. XPath tests by local name (`local-name()='x'`) match an element of that name in
//! any namespace.

use std::collections::HashSet;

use deckle_limits::roxmltree::{Document, Node};

use super::href::unquote;
use crate::book::{Book, Error};
use crate::epub::package::{NCX_MIME, OpfItem, normpath, parse_options, parse_with, string_value};
use crate::epub::toc::NavPoint;

/// Calibre's rewritten OPF as kfxgen reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalibreOpf {
    /// `_opf_raw_dates`: the text of each `metadata/date`, as written.
    pub dates: Vec<String>,
    /// `_spine_in_order`'s `listed`: the file name (decoded, no folder, no fragment) of each
    /// spine entry's manifest item, "" for an entry naming none.
    pub listed: HashSet<String>,
    /// The guide's references as (type, title, href), "" for a missing attribute; hrefs from
    /// the root, decoded once by Calibre (`OPF.unquote_urls`).
    pub guide: Vec<(String, String, String)>,
    /// `_book_files`: the path of the first manifest item whose `properties` hold `nav`, from
    /// the root and decoded again; also the base its links are read from.
    pub nav_path: Option<String>,
    /// The links of that nav, when it could be read.
    pub nav: Option<Nav>,
    /// The first manifest item of the NCX type, when it could be read.
    pub ncx: Option<Ncx>,
}

/// A nav document's links that kfxgen reads (`_nav_links`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Nav {
    /// The links of `<nav epub:type="landmarks">`.
    pub landmarks: Vec<NavLink>,
    /// The links of `<nav epub:type="page-list">`.
    pub page_list: Vec<NavLink>,
}

/// An `<a>` of a nav.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavLink {
    /// Its `epub:type`.
    pub epub_type: Option<String>,
    /// `"".join(a.itertext())`, not stripped.
    pub text: String,
    /// Its `href`.
    pub href: Option<String>,
}

/// An NCX as kfxgen reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ncx {
    /// Its path from the root, the base its hrefs are read from.
    pub path: String,
    /// `pageList/pageTarget`: (the text under its `navLabel`s, not stripped; its first
    /// `content/@src`, or "").
    pub page_targets: Vec<(String, String)>,
    /// Each `navMap`'s nav points, depth first: (the text under `navLabel/text`; its first
    /// `content/@src`, or ""; its depth, 0 at the top).
    pub points: Vec<(String, String, usize)>,
}

impl CalibreOpf {
    /// What kfxgen reads of `book`'s rewritten OPF ([`crate::epub::Oeb`]'s), its nav and its
    /// NCX.
    ///
    /// [`Error::TooLarge`] when the text kept from the OPF, the nav or the NCX passes
    /// [`TEXT_BUDGET`].
    pub fn new(book: &Book) -> Result<CalibreOpf, Error> {
        let r = &book.oeb.rewritten;
        let mut ids = std::collections::HashMap::new();
        for i in &r.manifest {
            if let Some(id) = &i.id {
                ids.insert(id.as_str(), i.href.as_deref().unwrap_or(""));
            }
        }
        let (nav_path, ncx_path) = kfxgen_paths(&r.manifest);
        // The NCX `convert_epub3_nav` makes is the manifest's last item.
        let generated = r
            .nav_ncx
            .as_ref()
            .and_then(|_| r.manifest.last())
            .map(|i| item_path(i.href.as_deref().unwrap_or("")));
        let ncx = match ncx_path {
            Some(path) if Some(&path) == generated.as_ref() => {
                Some(generated_ncx(path, r.nav_ncx.as_deref().unwrap_or(&[])))
            }
            Some(path) => match r.ncx_text.as_deref() {
                Some(t) => ncx(&path, t)?,
                None => None,
            },
            None => None,
        };
        let nav = match (&nav_path, r.nav_text.as_deref()) {
            (Some(path), Some(t)) => nav_links(path, t)?,
            _ => None,
        };
        Ok(CalibreOpf {
            dates: opf_dates(&book.package.opf_path, &book.package.opf_text)?.unwrap_or_default(),
            listed: r
                .spine
                .iter()
                .map(|idref| file_name(ids.get(idref.as_str()).copied().unwrap_or("")))
                .collect(),
            guide: r
                .guide
                .iter()
                .map(|g| {
                    let a = |v: &Option<String>| v.clone().unwrap_or_default();
                    (a(&g.kind), a(&g.title), a(&g.href))
                })
                .collect(),
            nav,
            nav_path,
            ncx,
        })
    }
}

/// `_book_files`' two paths in the manifest of Calibre's OPF: the first item whose
/// `properties` hold `nav`, and the first whose `media-type` is the NCX's, each at
/// [`item_path`].
pub(crate) fn kfxgen_paths(manifest: &[OpfItem]) -> (Option<String>, Option<String>) {
    let path = |test: &dyn Fn(&OpfItem) -> bool| {
        // `item[test]/@href`: the first such item that has an href.
        manifest
            .iter()
            .filter(|i| test(i))
            .find_map(|i| i.href.as_deref())
            .map(item_path)
    };
    (
        path(&|i| has_nav_property(&i.properties)),
        path(&|i| i.media_type.as_deref() == Some(NCX_MIME)),
    )
}

/// The NCX `convert_epub3_nav` writes at `path`: its `navMap` from the nav's contents, no page
/// list.
fn generated_ncx(path: String, points: &[NavPoint]) -> Ncx {
    let mut out = Vec::new();
    // Depth first; the nav points are at most `MAX_DEPTH` deep.
    let mut stack: Vec<(&NavPoint, usize)> = points.iter().rev().map(|p| (p, 0)).collect();
    while let Some((p, depth)) = stack.pop() {
        out.push((p.label.clone(), p.src.clone().unwrap_or_default(), depth));
        stack.extend(p.children.iter().rev().map(|c| (c, depth + 1)));
    }
    Ncx {
        path,
        page_targets: Vec::new(),
        points: out,
    }
}

const OPS_NS: &str = "http://www.idpf.org/2007/ops";

fn is_named(n: &Node, local: &str) -> bool {
    n.is_element() && n.tag_name().name() == local
}

/// The bytes of text kept from one file: twice the largest file read
/// ([`deckle_limits::MAX_CONTENT_FILE_BYTES`]). Text kept once is at most the file's own size;
/// twice lets each kept piece sit inside one other kept piece (a link inside a link) in a file
/// at that size. Past it the text is copied once per level of nesting, which only a crafted
/// file does: up to `MAX_DEPTH` copies of the whole file.
pub const TEXT_BUDGET: u64 = 2 * deckle_limits::MAX_CONTENT_FILE_BYTES;

/// A node's place in document order.
fn index(n: Node) -> usize {
    n.id().get() as usize
}

/// The text of a document's subtrees, each charged to [`TEXT_BUDGET`] before it is copied.
struct Texts<'f> {
    file: &'f str,
    /// `before[i]`: the bytes of text in the nodes before node `i`, in document order.
    before: Vec<u64>,
    kept: u64,
}

impl<'f> Texts<'f> {
    fn new(doc: &Document, file: &'f str) -> Self {
        let mut before = vec![0u64];
        for n in doc.descendants() {
            // Node ids count in document order, the order `descendants` walks.
            debug_assert_eq!(index(n) + 1, before.len());
            let len = if n.is_text() {
                n.text().map_or(0, str::len)
            } else {
                0
            };
            before.push(before[before.len() - 1] + len as u64);
        }
        Texts {
            file,
            before,
            kept: 0,
        }
    }

    /// `string_value(n)`, charged.
    fn of(&mut self, n: Node) -> Result<String, Error> {
        let mut last = n;
        while let Some(c) = last.last_child() {
            last = c;
        }
        let len = self.before[index(last) + 1] - self.before[index(n)];
        self.kept = self.kept.saturating_add(len);
        if self.kept > TEXT_BUDGET {
            return Err(Error::TooLarge {
                what: format!(
                    "{}: the text read from its nested elements passes {} MiB",
                    self.file,
                    TEXT_BUDGET / (1024 * 1024)
                ),
            });
        }
        Ok(string_value(&n))
    }
}

/// `text` as lxml's `etree.parse` reads a file, or None when it would fail (or `deckle_limits`
/// refuses it).
fn parse<R>(text: &str, f: impl FnOnce(&Document) -> Result<R, Error>) -> Result<Option<R>, Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    deckle_limits::parse_xml_limiting_attributes(
        text,
        parse_options(),
        deckle_limits::MAX_TAG_ATTRIBUTES,
    )
    .ok()
    .map(|doc| f(&doc))
    .transpose()
}

/// `_opf_raw_dates` on the book's OPF `text`: the string value of each child named `date` of
/// an element named `metadata`, in document order. Calibre's OPF keeps the book's date
/// elements as written (a year alone stays `2020` in `package_more`); it is the tree Calibre
/// read, so the OPF is read as Calibre's reader reads it (`parse_with`, libxml2's recovery of
/// end tags). None when it cannot be parsed.
fn opf_dates(file: &str, text: &str) -> Result<Option<Vec<String>>, Error> {
    parse_with(text, |doc| {
        let mut texts = Texts::new(doc, file);
        doc.descendants()
            .filter(|n| is_named(n, "date") && n.parent().is_some_and(|p| is_named(&p, "metadata")))
            .map(|n| texts.of(n))
            .collect()
    })
    .ok()
    .transpose()
}

/// The path kfxgen reads a manifest href at: `normpath(join(opf_dir, unquote(href)))`, with
/// Calibre's OPF at the root.
pub(crate) fn item_path(href: &str) -> String {
    normpath(&unquote(href))
}

/// XPath 1.0 `normalize-space`: XML white space collapsed to single spaces, none at the ends.
fn normalize_space(s: &str) -> String {
    s.split([' ', '\t', '\r', '\n'])
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `properties` holds the token `nav` (`contains(concat(' ', normalize-space(@properties),
/// ' '), ' nav ')`).
pub(crate) fn has_nav_property(properties: &str) -> bool {
    format!(" {} ", normalize_space(properties)).contains(" nav ")
}

/// `posixpath.basename(unquote((href or "").split("#")[0]))`.
pub(crate) fn file_name(href: &str) -> String {
    let path = unquote(href.split('#').next().unwrap_or(""));
    path.rsplit('/').next().unwrap_or("").to_owned()
}

/// `_nav_links` for every kind kfxgen reads, from the nav at `file` with text `text`.
pub(crate) fn nav_links(file: &str, text: &str) -> Result<Option<Nav>, Error> {
    parse(text, |doc| {
        let mut texts = Texts::new(doc, file);
        Ok(Nav {
            landmarks: links_of(doc, "landmarks", &mut texts)?,
            page_list: links_of(doc, "page-list", &mut texts)?,
        })
    })
}

/// `//*[local-name()='nav'][contains(concat(' ', normalize-space(@*[local-name()='type']), ' '),
/// ' kind ')]//*[local-name()='a']`: each element named `a` under such a nav, once, in document
/// order. The nav's type is its first attribute named `type`, in any namespace or none.
fn links_of(doc: &Document, kind: &str, texts: &mut Texts) -> Result<Vec<NavLink>, Error> {
    let is_kind = |n: &Node| {
        is_named(n, "nav")
            && n.attributes()
                .find(|a| a.name() == "type")
                .is_some_and(|a| a.value().split([' ', '\t', '\r', '\n']).any(|t| t == kind))
    };
    // Nodes are numbered in document order, so a nav's descendants are the numbers after it
    // up to its last descendant's: one pass, each nav's last descendant found once.
    let mut inside_to = None;
    let mut out = Vec::new();
    for n in doc.descendants() {
        let id = n.id().get();
        if inside_to.is_some_and(|end| id <= end) {
            if is_named(&n, "a") {
                out.push(NavLink {
                    epub_type: n.attribute((OPS_NS, "type")).map(str::to_owned),
                    text: texts.of(n)?,
                    href: n.attribute("href").map(str::to_owned),
                });
            }
        } else if is_kind(&n) {
            let mut last = n;
            while let Some(c) = last.last_child() {
                last = c;
            }
            inside_to = Some(last.id().get());
        }
    }
    Ok(out)
}

/// The first `src` of `n`'s children named `content` (`(./content/@src or [""])[0]`), or "".
fn content_src(n: &Node) -> String {
    n.children()
        .filter(|c| is_named(c, "content"))
        .find_map(|c| c.attribute("src"))
        .unwrap_or("")
        .to_owned()
}

/// The NCX at `path` with text `text`, as kfxgen reads it.
pub(crate) fn ncx(path: &str, text: &str) -> Result<Option<Ncx>, Error> {
    parse(text, |doc| {
        let mut texts = Texts::new(doc, path);
        let page_targets = doc
            .descendants()
            .filter(|n| {
                is_named(n, "pageTarget") && n.parent().is_some_and(|p| is_named(&p, "pageList"))
            })
            .map(|t| {
                let label = t
                    .children()
                    .filter(|c| is_named(c, "navLabel"))
                    .map(|l| texts.of(l))
                    .collect::<Result<String, Error>>()?;
                Ok((label, content_src(&t)))
            })
            .collect::<Result<_, Error>>()?;
        let mut points = Vec::new();
        for map in doc.descendants().filter(|n| is_named(n, "navMap")) {
            // Depth first, as `walk` recurses; the stack holds (node, depth), next on top.
            let mut stack: Vec<(Node, usize)> = map
                .children()
                .filter(|c| is_named(c, "navPoint"))
                .map(|c| (c, 0))
                .collect();
            stack.reverse();
            while let Some((point, depth)) = stack.pop() {
                let label = point
                    .children()
                    .filter(|c| is_named(c, "navLabel"))
                    .flat_map(|l| l.children().filter(|t| is_named(t, "text")))
                    .map(|t| texts.of(t))
                    .collect::<Result<String, Error>>()?;
                points.push((label, content_src(&point), depth));
                let start = stack.len();
                stack.extend(
                    point
                        .children()
                        .filter(|c| is_named(c, "navPoint"))
                        .map(|c| (c, depth + 1)),
                );
                stack[start..].reverse();
            }
        }
        Ok(Ncx {
            path: path.to_owned(),
            page_targets,
            points,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The links lxml and `_nav_links` give for this nav (run with kfxgen `ad38ec8`'s venv):
    /// a nav's type is its first attribute named `type`, a nested nav adds no repeats, a
    /// comment's text is left out, and names match in any namespace.
    #[test]
    fn nav_links_are_kfxgens() {
        let nav = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body>
<nav type="toc" epub:type="landmarks"><a href="x1">X1</a></nav>
<nav epub:type="  landmarks
 other"><ol><li><a epub:type="bodymatter" href="c1.xhtml">St<b>ar</b>t<!-- c -->!</a>
<nav epub:type="landmarks"><a href="n">Nested</a></nav></li></ol></nav>
<nav epub:type="page-list"><a href="p#1"> 1 </a><a>no href</a></nav>
<foo:nav xmlns:foo="urn:x" foo:type="landmarks"><foo:a href="f">F</foo:a></foo:nav>
</body></html>"#;
        let link = |t: Option<&str>, text: &str, href: Option<&str>| NavLink {
            epub_type: t.map(str::to_owned),
            text: text.to_owned(),
            href: href.map(str::to_owned),
        };
        assert_eq!(
            nav_links("nav.xhtml", nav).unwrap(),
            Some(Nav {
                landmarks: vec![
                    link(Some("bodymatter"), "Start!", Some("c1.xhtml")),
                    link(None, "Nested", Some("n")),
                    link(None, "F", Some("f")),
                ],
                page_list: vec![link(None, " 1 ", Some("p#1")), link(None, "no href", None)],
            })
        );
        // A plain-text entity is expanded; lxml would give "1&e;2" (DIVERGENCES.md).
        let entity =
            r#"<!DOCTYPE x [<!ENTITY e "EE">]><x><nav type="page-list"><a>1&e;2</a></nav></x>"#;
        assert_eq!(
            nav_links("n", entity).unwrap().unwrap().page_list[0].text,
            "1EE2"
        );
        // Not well-formed: lxml's parser fails, so kfxgen has no nav.
        assert_eq!(nav_links("n", "<html><nav></html>").unwrap(), None);
    }

    #[test]
    fn text_kept_from_nested_links_is_bounded() {
        // A link inside a link keeps both texts, as lxml's `itertext` gives them.
        let nested = r#"<x><nav type="page-list"><a href="x">A<a href="y">B</a></a></nav></x>"#;
        let texts: Vec<String> = nav_links("n", nested)
            .unwrap()
            .unwrap()
            .page_list
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(texts, ["AB", "B"]);
        // 200 links one inside the next around 400 KB of text would keep 80 MB. A debug
        // build's parser needs more than a test thread's stack at that depth, as in the other
        // deep tests.
        let deep = format!(
            r#"<x><nav type="page-list">{}{}{}</nav></x>"#,
            "<a>".repeat(200),
            "t".repeat(400_000),
            "</a>".repeat(200)
        );
        let err = std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(move || nav_links("OEBPS/nav.xhtml", &deep).unwrap_err())
            .unwrap()
            .join()
            .unwrap();
        assert!(matches!(err, Error::TooLarge { .. }), "{err}");
        assert!(err.to_string().starts_with("OEBPS/nav.xhtml: "), "{err}");
    }

    #[test]
    fn the_paths_are_of_the_first_matching_item_with_an_href() {
        let item = |href: Option<&str>, mt: &str, props: &str| OpfItem {
            id: None,
            href: href.map(str::to_owned),
            media_type: Some(mt.to_owned()),
            properties: props.to_owned(),
        };
        let manifest = [
            item(None, "application/xhtml+xml", "nav"),
            item(
                Some("a%2520b.xhtml"),
                "application/xhtml+xml",
                " scripted\tnav ",
            ),
            item(None, NCX_MIME, ""),
            item(Some("x/../toc.ncx"), NCX_MIME, ""),
            item(Some("later.ncx"), NCX_MIME, ""),
        ];
        assert_eq!(
            kfxgen_paths(&manifest),
            (Some("a%20b.xhtml".to_owned()), Some("toc.ncx".to_owned()))
        );
    }

    #[test]
    fn the_ncx_calibre_makes_from_a_nav() {
        let p = |label: &str, src: Option<&str>, children| NavPoint {
            label: label.to_owned(),
            src: src.map(str::to_owned),
            children,
        };
        let points = [
            p(
                "A",
                Some("a.xhtml"),
                vec![p("", None, vec![p("C", Some("c.xhtml#x"), vec![])])],
            ),
            p("B", Some("b.xhtml"), vec![]),
        ];
        let ncx = generated_ncx("OEBPS/tmpXXXXXXXX.ncx".to_owned(), &points);
        assert_eq!(
            ncx.points,
            [
                ("A", "a.xhtml", 0),
                ("", "", 1),
                ("C", "c.xhtml#x", 2),
                ("B", "b.xhtml", 0)
            ]
            .map(|(l, s, d)| (l.to_owned(), s.to_owned(), d))
        );
        assert!(ncx.page_targets.is_empty());
    }
}
