// SPDX-License-Identifier: GPL-3.0-only
//! Calibre's book view of an EPUB, as its conversion pipeline leaves it for kfxgen: the steps
//! of `conversion/plugins/epub_input.py` (`convert`, `postprocess_book`), `ebooks/oeb/reader.py`
//! (`OEBReader`) and the transforms `conversion/plumber.py` runs before the output plugin that
//! change the spine, manifest, guide, contents or metadata (`oeb/transforms/guide.py` `Clean`,
//! `metadata.py` `MergeMetadata`, `flatcss.py` `CSSFlattener`, `trimmanifest.py`
//! `ManifestTrimmer`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use deckle_kfx::epub_zip::{EpubReader, InspectError};
use deckle_limits::MAX_DEPTH;
use deckle_limits::roxmltree::Document;
use regex::Regex;

use super::package::{
    self, NCX_MIME, Opf, OpfItem, OpfItemref, OpfRef, abshref, dirname, is_linear, normpath,
    posix_join, read_text, splitext, unquote, urldefrag, urlnormalize, urlquote,
};
use super::toc::{self, NavPoint};
use super::{Error, GuideRef, Item, Metadata, Oeb, Package, SpineRef, TocNode};
use crate::dom::{self, Dom, NodeId, XHTML_NS};

const XHTML_MIME: &str = "application/xhtml+xml";
const CSS_MIME: &str = "text/css";
const BINARY_MIME: &str = "application/octet-stream";
/// `base.py` `OEB_DOCS`.
const OEB_DOCS: [&str; 4] = [
    XHTML_MIME,
    "text/html",
    "text/x-oeb1-document",
    "text/x-oeb-document",
];
/// `base.py` `OEB_STYLES`.
pub(crate) const OEB_STYLES: [&str; 4] =
    [CSS_MIME, "text/x-oeb1-css", "text/x-oeb-css", "xhtml/css"];
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
/// `base.py` `_link_attrs`: lxml's `html.defs.link_attrs` with `poster` and `altimg` (and
/// `xlink:href`, checked apart).
const LINK_ATTRS: [&str; 16] = [
    "action",
    "archive",
    "background",
    "cite",
    "classid",
    "codebase",
    "data",
    "href",
    "longdesc",
    "profile",
    "src",
    "usemap",
    "dynsrc",
    "lowsrc",
    "poster",
    "altimg",
];
/// `base.py` `Guide.Reference.TYPES`.
const GUIDE_TYPES: [&str; 17] = [
    "cover",
    "title-page",
    "toc",
    "index",
    "glossary",
    "acknowledgements",
    "bibliography",
    "colophon",
    "copyright-page",
    "dedication",
    "epigraph",
    "foreword",
    "loi",
    "lot",
    "notes",
    "preface",
    "text",
];
/// `transforms/guide.py` `KNOWN_GUIDE_TYPES`.
const KNOWN_GUIDE_TYPES: [&str; 19] = [
    "cover",
    "titlepage",
    "masthead",
    "toc",
    "title-page",
    "copyright-page",
    "text",
    "index",
    "glossary",
    "acknowledgements",
    "bibliography",
    "colophon",
    "dedication",
    "epigraph",
    "foreword",
    "loi",
    "lot",
    "notes",
    "preface",
];
/// `transforms/guide.py` `CUSTOM_GUIDE_TYPE_PREFIX`.
const CUSTOM_GUIDE_TYPE_PREFIX: &str = "other.calibre-epub-type-";

/// `base.py` `_css_url_re` and `_css_import_re`.
static CSS_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)url\s*\(['"]?(.*?)['"]?\)"#).unwrap());
static CSS_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@import "(.*?)""#).unwrap());
/// An `@import` in a stylesheet, quoted or as `url()`.
static SHEET_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)@import\s+(?:url\(\s*['"]?([^'")]*)['"]?\s*\)|['"]([^'"]*)['"])"#).unwrap()
});
static CSS_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)/\*.*?\*/").unwrap());

/// `_manifest_from_opf`: the media type Calibre's manifest gives an OPF item of `href` whose
/// `media-type` is `written`: guessed from the extension when empty or `text/xml`, lower-cased.
fn media_type(href: &str, written: &str) -> String {
    let mut mt = written.to_owned();
    if mt.is_empty() || mt == "text/xml" {
        mt = guess_type(href)
            .map(str::to_owned)
            .or(Some(mt).filter(|m| !m.is_empty()))
            .unwrap_or_else(|| BINARY_MIME.to_owned());
    }
    mt.to_lowercase()
}

/// Whether Calibre's manifest holds the OPF item `href` of `media-type` `written` as a
/// stylesheet (`OEB_STYLES`), so that a document's links and imports reach it.
pub(crate) fn is_stylesheet(href: &str, written: &str) -> bool {
    OEB_STYLES.contains(&media_type(href, written).as_str())
}

/// `calibre.guess_type` for the extensions a book holds (asked of Calibre 9.18).
fn guess_type(href: &str) -> Option<&'static str> {
    let ext = splitext(href)
        .1
        .trim_start_matches('.')
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" => "text/html",
        "xhtml" | "xht" => XHTML_MIME,
        "css" => CSS_MIME,
        "ncx" => NCX_MIME,
        "opf" => "application/oebps-package+xml",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "jxl" => "image/jxl",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "js" => "application/javascript",
        "xml" => "application/xml",
        "txt" => "text/plain",
        "smil" => "application/smil+xml",
        "mp3" | "m4a" => "audio/mpeg",
        "mp4" => "video/mp4",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "json" => "application/json",
        _ => return None,
    })
}

/// `base.py` `COLLAPSE_RE.sub(' ', s.strip())`.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.trim().chars() {
        if matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0b') {
            space = true;
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

fn is_doc(media_type: &str) -> bool {
    OEB_DOCS.contains(&media_type)
}

fn is_xml(media_type: &str) -> bool {
    media_type.ends_with("/xml") || media_type.ends_with("+xml")
}

// ---------------------------------------------------------------------------------------
// The archive
// ---------------------------------------------------------------------------------------

/// What the pipeline reads of a document: its links.
#[derive(Debug, Clone, Default)]
struct DocInfo {
    /// The root is `html` in a namespace (`reader.py` `_spine_from_opf`).
    html_root: bool,
    /// `h:body//h:a/@href` (`_spine_add_extra`).
    body_links: Vec<String>,
    /// `base.py` `iterlinks`.
    links: Vec<String>,
    /// The hrefs of its `<link rel="stylesheet">` elements.
    sheets: Vec<String>,
    /// The text of its `<style>` elements.
    styles: Vec<String>,
}

/// `n` is `name` in no namespace or XHTML's (`toc::is_html` for the element tree).
fn dom_is_html(dom: &Dom, n: NodeId, name: &str) -> bool {
    dom.tag(n) == name && matches!(dom.namespace(n), None | Some(XHTML_NS))
}

/// lxml's `string()` of `n`: its text and that of everything under it, in document order.
fn dom_string_value(dom: &Dom, n: NodeId) -> String {
    let mut out = String::new();
    // (element, entering): its text and children on entering, its tail on leaving.
    let mut stack = vec![(n, true)];
    while let Some((m, entering)) = stack.pop() {
        if entering {
            out.push_str(dom.text(m));
            if m != n {
                stack.push((m, false));
            }
            stack.extend(dom.children(m).iter().rev().map(|&c| (c, true)));
        } else {
            out.push_str(dom.tail(m));
        }
    }
    out
}

fn doc_info(dom: &Dom) -> DocInfo {
    let root = dom.root();
    let mut info = DocInfo {
        html_root: dom.tag(root) == "html" && dom.namespace(root).is_some_and(|n| !n.is_empty()),
        ..DocInfo::default()
    };
    // The `<a>`s under each of the root's `<body>` children, in document order.
    let mut stack: Vec<NodeId> = dom
        .children(root)
        .iter()
        .rev()
        .copied()
        .filter(|&c| dom_is_html(dom, c, "body"))
        .collect();
    while let Some(n) = stack.pop() {
        stack.extend(dom.children(n).iter().rev());
        if dom_is_html(dom, n, "a")
            && let Some(h) = dom.attr(n, "href")
        {
            info.body_links.push(h.to_owned());
        }
    }
    let mut push = |v: &str| info.links.push(v.to_owned());
    for el in dom.descendants() {
        let tag = dom.tag(el).to_lowercase();
        if tag == "object" {
            for a in ["codebase", "classid", "data"] {
                if let Some(v) = dom.attr(el, a) {
                    push(v);
                }
            }
            if let Some(v) = dom.attr(el, "archive") {
                v.split(' ').filter(|w| !w.is_empty()).for_each(&mut push);
            }
        } else {
            for (name, ns, value) in dom.attrs(el) {
                let local = name.rsplit_once(':').map_or(name, |(_, l)| l);
                let plain = ns.is_none() && LINK_ATTRS.contains(&name);
                if plain || (ns == Some(XLINK_NS) && local == "href") {
                    push(value);
                }
            }
        }
        if tag == "style" {
            let text = dom.text(el);
            for c in CSS_URL.captures_iter(text) {
                push(&c[1]);
            }
            for c in CSS_IMPORT.captures_iter(text) {
                push(&c[1]);
            }
        }
        if let Some(style) = dom.attr(el, "style") {
            for c in CSS_URL.captures_iter(style) {
                push(&c[1]);
            }
        }
    }
    for el in dom.descendants() {
        if dom_is_html(dom, el, "link")
            && dom.attr(el, "rel").unwrap_or("stylesheet").to_lowercase() == "stylesheet"
            && OEB_STYLES.contains(
                &dom.attr(el, "type")
                    .unwrap_or(CSS_MIME)
                    .to_lowercase()
                    .as_str(),
            )
            && let Some(h) = dom.attr(el, "href")
        {
            info.sheets.push(h.to_owned());
        }
        if dom_is_html(dom, el, "style") {
            info.styles.push(dom_string_value(dom, el));
        }
    }
    info
}

/// The `url()` values and `@import` targets of a stylesheet, its comments left out.
fn sheet_refs(text: &str) -> (Vec<String>, Vec<String>) {
    let text = CSS_COMMENT.replace_all(text, "");
    let imports: Vec<String> = SHEET_IMPORT
        .captures_iter(&text)
        .filter_map(|c| c.get(1).or(c.get(2)).map(|m| m.as_str().trim().to_owned()))
        .collect();
    let imported: BTreeSet<&str> = imports.iter().map(String::as_str).collect();
    let urls = CSS_URL
        .captures_iter(&text)
        .map(|c| c[1].trim().trim_matches(['"', '\'']).trim().to_owned())
        .filter(|u| !u.is_empty() && !imported.contains(u.as_str()))
        .collect();
    (imports, urls)
}

/// `f` of `text` parsed ([`package::parse_with`]), or None when `deckle_limits::parse_xml`
/// refuses it (too deep, an entity with markup) or it is not well-formed.
fn read_xml<R>(text: &str, f: impl FnOnce(&Document) -> R) -> Option<R> {
    package::parse_with(text, f).ok()
}

/// `reader.py` `_toc_from_html`: the links under the fragment of the guide's `toc` document
/// `dom` at `base`, one entry per target, their texts joined.
fn html_toc(dom: &Dom, base: &str, frag: &str, hrefs: &BTreeMap<String, usize>) -> Vec<TocNode> {
    let root = dom.root();
    let is_link = |a: NodeId| dom_is_html(dom, a, "a") && dom.attr(a, "href").is_some();
    let mut elem = root;
    if !frag.is_empty() {
        let by = |attr: &str| {
            dom.descendants()
                .skip(1)
                .find(|&e| dom.attr(e, attr) == Some(frag))
        };
        elem = by("id").or_else(|| by("name")).unwrap_or(root);
        // Up to the first element with a link inside: the ancestors of every link, each
        // marked once.
        let mut has_link: BTreeSet<NodeId> = BTreeSet::new();
        for a in dom.descendants().filter(|&a| is_link(a)) {
            let mut p = dom.parent(a);
            while let Some(q) = p {
                if !has_link.insert(q) {
                    break;
                }
                p = dom.parent(q);
            }
        }
        while elem != root && !has_link.contains(&elem) {
            elem = dom.parent(elem).unwrap_or(root);
        }
    }
    let mut order: Vec<String> = Vec::new();
    let mut titles: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The elements under `elem`, in document order.
    let mut stack: Vec<NodeId> = dom.children(elem).iter().rev().copied().collect();
    while let Some(a) = stack.pop() {
        stack.extend(dom.children(a).iter().rev());
        if !is_link(a) {
            continue;
        }
        let href = abshref(base, &urlnormalize(dom.attr(a, "href").unwrap_or("")));
        if !hrefs.contains_key(urldefrag(&href).0) {
            continue;
        }
        let title = collapse(&dom_string_value(dom, a));
        if !titles.contains_key(&href) {
            order.push(href.clone());
        }
        titles.entry(href).or_default().push(title);
    }
    order
        .into_iter()
        .map(|href| TocNode {
            title: titles[&href].join(" "),
            href,
            children: Vec::new(),
        })
        .collect()
}

/// The EPUB's files, read at most once each.
struct Book<'z> {
    zip: &'z mut EpubReader,
    names: BTreeSet<String>,
    docs: BTreeMap<String, Option<DocInfo>>,
}

impl Book<'_> {
    /// `DirContainer.exists`: the file an href names (hrefs are unquoted to file names).
    fn has(&self, href: &str) -> bool {
        self.names.contains(&unquote(href))
    }

    /// The text of the file `name`, or None if it is missing or not UTF-8.
    fn read(&mut self, name: &str) -> Result<Option<String>, Error> {
        if !self.names.contains(name) {
            return Ok(None);
        }
        read_text(self.zip, name)
    }

    /// The bytes of the file `name`, or None if it is missing.
    fn read_bytes(&mut self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        if !self.names.contains(name) {
            return Ok(None);
        }
        match self.zip.read_bytes(name) {
            Ok(b) => Ok(Some(b)),
            Err(InspectError::MissingFile(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The links of the document at `href`, read the first time it is asked for: as a
    /// document (`html`, `base.py` `_parse_xhtml`: `dom::parse`, which reads HTML that is not
    /// XML), or as XML (`_parse_xml`). None when it cannot be read: `dom::parse` refuses it
    /// (too deep or too large as HTML, entities past their limit), or XML that
    /// `deckle_limits::parse_xml` refuses or is not well-formed or UTF-8.
    fn doc(&mut self, href: &str, html: bool) -> Result<Option<DocInfo>, Error> {
        let name = unquote(href);
        if let Some(d) = self.docs.get(&name) {
            return Ok(d.clone());
        }
        let info = if html {
            self.read_bytes(&name)?
                .and_then(|b| dom::parse(&b).ok())
                .map(|d| doc_info(&d))
        } else {
            self.read(&name)?
                .and_then(|text| read_xml(&text, |doc| doc_info(&dom::from_xml(doc))))
        };
        self.docs.insert(name, info.clone());
        Ok(info)
    }
}

// ---------------------------------------------------------------------------------------
// epub_input.py `convert`: the OPF Calibre writes for its reader
// ---------------------------------------------------------------------------------------

enum TocAttr {
    None,
    Id(String),
    /// The NCX `convert_epub3_nav` made from the nav.
    Nav,
}

/// The nav's contents, as the NCX Calibre writes beside it.
struct NavToc {
    points: Vec<NavPoint>,
    /// The href of that NCX, which its hrefs are relative to.
    base: String,
    /// `opts.epub3_nav_href`.
    nav_href: String,
}

/// The OPF's guide as `epub_input.py` edits it: its references in order, each findable by its
/// type in lower case, the way `set_guide_type` compares them.
#[derive(Default)]
struct OpfGuide {
    refs: Vec<Option<OpfRef>>,
    /// Positions in `refs` of the live references, by lower-case type.
    by_type: BTreeMap<String, Vec<usize>>,
}

impl OpfGuide {
    fn new(refs: impl IntoIterator<Item = OpfRef>) -> OpfGuide {
        let mut g = OpfGuide::default();
        for r in refs {
            g.push(r);
        }
        g
    }

    fn push(&mut self, r: OpfRef) {
        let kind = r.kind.as_deref().unwrap_or("").to_lowercase();
        self.by_type.entry(kind).or_default().push(self.refs.len());
        self.refs.push(Some(r));
    }

    /// Removes every reference whose type in lower case is `kind`.
    fn remove_type(&mut self, kind: &str) {
        for i in self.by_type.remove(kind).unwrap_or_default() {
            self.refs[i] = None;
        }
    }

    /// The position of the first reference whose type in lower case is `kind`.
    fn first(&self, kind: &str) -> Option<usize> {
        self.by_type.get(kind).and_then(|v| v.first().copied())
    }

    fn get_mut(&mut self, i: usize) -> Option<&mut OpfRef> {
        self.refs.get_mut(i).and_then(Option::as_mut)
    }

    fn iter(&self) -> impl Iterator<Item = &OpfRef> {
        self.refs.iter().flatten()
    }
}

struct Input {
    manifest: Vec<OpfItem>,
    spine: Vec<OpfItemref>,
    guide: OpfGuide,
    toc: TocAttr,
}

fn media_ok(mt: Option<&str>) -> bool {
    mt.is_some_and(|m| !m.is_empty() && !m.contains("xml") && !m.contains("html"))
}

/// `polish/upgrade.py` `guide_type_for_epubtype`: the guide type for an EPUB 3 landmark.
fn guide_type_for_epubtype(t: &str) -> String {
    match t {
        "acknowledgments" => "acknowledgements",
        "bodymatter" => "text",
        "titlepage" => "title-page",
        "bibliography" | "colophon" | "copyright-page" | "cover" | "dedication" | "epigraph"
        | "foreword" | "glossary" | "index" | "loi" | "lot" | "preface" | "toc" => t,
        _ => return format!("{CUSTOM_GUIDE_TYPE_PREFIX}{t}"),
    }
    .to_owned()
}

impl Input {
    /// `convert`: the OPF's hrefs unquoted (`OPF.unquote_urls`) and, when the OPF is in a
    /// folder, made relative to the EPUB root (`normpath`).
    fn new(opf: &Opf, opf_dir: &str) -> Input {
        let fix = |h: &Option<String>| {
            h.as_ref().map(|h| {
                let h = unquote(h);
                if opf_dir.is_empty() {
                    h
                } else {
                    normpath(&format!("{opf_dir}{h}"))
                }
            })
        };
        let mut manifest = opf.manifest.clone();
        for i in &mut manifest {
            i.href = fix(&i.href);
        }
        let guide = opf.guide.iter().map(|g| OpfRef {
            href: fix(&g.href),
            ..g.clone()
        });
        Input {
            manifest,
            spine: opf.spine.clone(),
            guide: OpfGuide::new(guide),
            toc: match &opf.spine_toc {
                Some(id) => TocAttr::Id(id.clone()),
                None => TocAttr::None,
            },
        }
    }

    /// `opf2.py` `OPF.epub3_raster_cover`.
    fn epub3_raster_cover(&self) -> Option<String> {
        for item in &self.manifest {
            let props = item.properties.to_lowercase();
            if props.split_whitespace().any(|p| p == "cover-image")
                && media_ok(item.media_type.as_deref())
            {
                return item.href.clone();
            }
        }
        // "Open" Manga Format: the cover as the first spine item.
        let first = self.spine.first()?;
        let idref = first.idref.as_deref().unwrap_or("");
        let item = self
            .manifest
            .iter()
            .rfind(|i| i.id.as_deref().unwrap_or("") == idref)?;
        let mt = item.media_type.as_deref()?.to_lowercase();
        ["image/jpeg", "image/jpg", "image/png", "image/webp"]
            .contains(&mt.as_str())
            .then(|| item.href.clone())
            .flatten()
    }

    /// `opf2.py` `OPF.raster_cover`.
    fn raster_cover(&self, opf: &Opf) -> Option<String> {
        match &opf.cover_meta {
            Some(cid) => self
                .manifest
                .iter()
                .find(|i| i.id.as_deref() == Some(cid) && media_ok(i.media_type.as_deref()))
                .or_else(|| {
                    self.manifest.iter().find(|i| {
                        i.href.as_deref() == Some(cid) && media_ok(i.media_type.as_deref())
                    })
                })
                .and_then(|i| i.href.clone()),
            None if opf.version >= 3.0 => self.epub3_raster_cover(),
            None => None,
        }
    }

    /// The first guide reference of type cover: its position and its href without fragment.
    fn guide_cover(&self) -> Option<(usize, String)> {
        let i = self.guide.first("cover")?;
        let href = self.guide.refs[i]
            .as_ref()
            .and_then(|g| g.href.as_deref())
            .unwrap_or("");
        Some((i, urldefrag(href).0.to_owned()))
    }

    /// `epub_input.py` `set_guide_type`.
    fn set_guide_type(&mut self, kind: &str, href: Option<String>, title: &str) {
        self.guide.remove_type(kind);
        if let Some(href) = href {
            self.guide.push(OpfRef {
                kind: Some(kind.to_owned()),
                title: Some(title.to_owned()),
                href: Some(href),
            });
        }
    }

    /// `epub_input.py` `rationalize_cover2` (EPUB 2): an HTML cover first in the spine and
    /// named by the guide comes out of the spine and becomes the title page; the guide's cover
    /// becomes the raster cover. Returns the removed page's href.
    fn rationalize_cover2(&mut self, opf: &Opf) -> Option<String> {
        let cover = self.guide_cover().filter(|(_, h)| !h.is_empty());
        let Some((gi, guide_cover)) = cover else {
            if let Some(rc) = self.raster_cover(opf) {
                self.guide.push(OpfRef {
                    kind: Some("cover".to_owned()),
                    title: None,
                    href: Some(rc),
                });
            }
            return None;
        };
        let idref = self.spine.first()?.idref.clone().unwrap_or_default();
        if self.manifest.is_empty() {
            return None;
        }
        let first = self
            .manifest
            .iter()
            .find(|x| x.id.as_deref().unwrap_or("") == idref);
        if first.and_then(|x| x.href.as_deref()) != Some(guide_cover.as_str()) {
            return None;
        }
        if self.spine.len() == 1 {
            self.guide.remove_type("cover");
            return None;
        }
        self.spine.remove(0);
        let href = match self.raster_cover(opf) {
            Some(rc) => rc,
            None => {
                // Calibre renders the page to this file; the port cannot (DIVERGENCES.md,
                // "An HTML cover with no cover image"), so the reader finds no such file.
                self.manifest.push(OpfItem {
                    id: Some("calibre_raster_cover".to_owned()),
                    href: Some("calibre_raster_cover.jpg".to_owned()),
                    media_type: Some("image/jpeg".to_owned()),
                    properties: String::new(),
                });
                "calibre_raster_cover.jpg".to_owned()
            }
        };
        if let Some(g) = self.guide.get_mut(gi) {
            g.href = Some(href);
        }
        self.set_guide_type("titlepage", Some(guide_cover.clone()), "Cover page");
        Some(guide_cover)
    }

    /// `epub_input.py` `rationalize_cover3` (EPUB 3): the raster cover into the guide, and the
    /// title page (a `calibre:title-page` item, else a guide cover that is first in the spine)
    /// out of the spine. Returns the removed page's href.
    fn rationalize_cover3(&mut self, opf: &Opf) -> Option<String> {
        let mut guide_titlepage = None;
        if let Some((_, gc)) = self.guide_cover().filter(|(_, h)| !h.is_empty())
            && let Some(first) = self.spine.first()
        {
            let idref = first.idref.clone().unwrap_or_default();
            if self
                .manifest
                .iter()
                .any(|x| x.id.as_deref() == Some(&idref) && x.href.as_deref() == Some(&gc))
            {
                guide_titlepage = Some((gc, idref));
            }
        }
        if let Some(rc) = self.epub3_raster_cover().or_else(|| self.raster_cover(opf)) {
            self.set_guide_type("cover", Some(rc), "Cover image");
        }
        // `opf3.py` `items_with_property(root, 'calibre:title-page')`.
        let titlepage = self
            .manifest
            .iter()
            .find(|i| {
                i.properties
                    .to_lowercase()
                    .split_whitespace()
                    .any(|p| opf.expand_prefix(p) == "https://calibre-ebook.com:title-page")
                    && i.id.as_deref().is_some_and(|s| !s.is_empty())
                    && i.href.as_deref().is_some_and(|s| !s.is_empty())
            })
            .map(|i| {
                let href = i.href.as_deref().unwrap_or("");
                (
                    urldefrag(href).0.to_owned(),
                    i.id.clone().unwrap_or_default(),
                )
            })
            .or(guide_titlepage);
        let (href, id) = titlepage?;
        self.set_guide_type("titlepage", Some(href.clone()), "Cover page");
        if self.spine.len() > 1 {
            let pos = self
                .spine
                .iter()
                .position(|s| s.idref.as_deref() == Some(id.as_str()))?;
            self.spine.remove(pos);
            return Some(href);
        }
        None
    }

    /// `opf2.py` `OPF.epub3_nav`: the root-relative path of the EPUB 3 nav.
    fn epub3_nav(&self, opf: &Opf, names: &BTreeSet<String>) -> Option<String> {
        if opf.version < 3.0 {
            return None;
        }
        self.manifest
            .iter()
            .find(|i| {
                i.properties
                    .to_lowercase()
                    .split_whitespace()
                    .any(|p| p == "nav")
                    && i.media_type
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains("html")
                    && i.id.as_deref().is_some_and(|s| !s.is_empty())
                    && i.href.as_deref().is_some_and(|h| names.contains(h))
            })
            .and_then(|i| i.href.clone())
    }

    /// `epub_input.py` `convert_epub3_nav` and `landmarks_to_guide`, for the nav `text` at
    /// `nav_path`. The nav's landmarks go into the guide; its contents become the NCX the
    /// reader reads. A nav that is not well-formed XML is skipped until phase 2 Task 4 brings
    /// an HTML parser.
    fn convert_epub3_nav(
        &mut self,
        nav_path: &str,
        text: &str,
        book: &Book,
        removed_cover: Option<&str>,
    ) -> Option<NavToc> {
        let nav_dir = dirname(nav_path);
        let nav_name = &nav_path[nav_path.rfind('/').map_or(0, |i| i + 1)..];
        let base = if nav_dir.is_empty() { "." } else { nav_dir };
        let nav_href = urlquote(nav_name);
        // `landmarks_to_guide`: the accepted landmarks, (guide type, href, title).
        let read = |doc: &Document| {
            let mut seen: BTreeSet<String> = BTreeSet::new();
            let mut found: Vec<(String, String, String)> = Vec::new();
            for lm in toc::landmarks(doc) {
                let gtype = guide_type_for_epubtype(lm.epub_type);
                if gtype == "cover" || seen.contains(&gtype) {
                    continue;
                }
                let (path, frag) = urldefrag(lm.href);
                let path = normpath(&posix_join(
                    base,
                    if path.is_empty() { &nav_href } else { path },
                ));
                if Some(path.as_str()) == removed_cover || !book.has(&path) {
                    continue;
                }
                seen.insert(gtype.clone());
                let href = if frag.is_empty() {
                    path
                } else {
                    format!("{path}#{frag}")
                };
                found.push((gtype, href, lm.title()));
            }
            (found, toc::nav_points(doc, nav_name))
        };
        let (landmarks, points) = read_xml(text, read)?;
        let mut rank: BTreeMap<String, usize> = BTreeMap::new();
        for (gtype, href, title) in landmarks {
            rank.insert(gtype.clone(), rank.len());
            self.set_guide_type(&gtype, Some(href), &title);
        }
        // Landmarks first, in their order; the rest after, in theirs.
        if !rank.is_empty() {
            let mut refs: Vec<OpfRef> = std::mem::take(&mut self.guide.refs)
                .into_iter()
                .flatten()
                .collect();
            refs.sort_by_key(|g| {
                g.kind
                    .as_deref()
                    .and_then(|k| rank.get(k))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            self.guide = OpfGuide::new(refs);
        }

        let points = points?;
        self.toc = TocAttr::Nav;
        Some(NavToc {
            points,
            base: urlnormalize(&posix_join(nav_dir, "calibre_nav.ncx")),
            nav_href: urlnormalize(nav_path),
        })
    }

    /// The rest of `convert`: DTBook refused, page templates and mislabelled fonts out of the
    /// spine, and each spine item once.
    fn clean_spine(&mut self) -> Result<(), Error> {
        if self
            .manifest
            .iter()
            .any(|x| x.media_type.as_deref() == Some("application/x-dtbook+xml"))
        {
            return Err(Error::Package(
                "EPUB files with DTBook markup are not supported".to_owned(),
            ));
        }
        let mut not_for_spine = BTreeSet::new();
        for y in &mut self.manifest {
            let Some(id) = y.id.clone().filter(|i| !i.is_empty()) else {
                continue;
            };
            let mt = y.media_type.as_deref().unwrap_or("");
            if [
                "application/vnd.adobe-page-template+xml",
                "application/vnd.adobe.page-template+xml",
                "application/adobe-page-template+xml",
                "application/adobe.page-template+xml",
                "application/text",
            ]
            .contains(&mt)
            {
                not_for_spine.insert(id.clone());
            }
            let href = y.href.as_deref().unwrap_or("");
            let ext = href.rsplit('.').next().unwrap_or("").to_lowercase();
            if mt == "text/plain" && (ext == "otf" || ext == "ttf") {
                not_for_spine.insert(id);
                y.media_type = Some("application/font".to_owned());
            }
        }
        let mut seen = BTreeSet::new();
        self.spine.retain(|x| match x.idref.as_deref() {
            Some(r) if !r.is_empty() && !not_for_spine.contains(r) => seen.insert(r.to_owned()),
            _ => false,
        });
        if self.spine.is_empty() {
            return Err(Error::Package(
                "No valid entries in the spine of this EPUB".to_owned(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// reader.py `OEBReader` and the transforms after it
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MItem {
    id: String,
    href: String,
    media_type: String,
    properties: Vec<String>,
}

/// A guide reference (`Guide.Reference`), kept under its key: the type as written
/// (`Guide.refs`). The guide's order is not kept: `oeb.json` lists it sorted.
#[derive(Debug, Clone)]
struct GRef {
    kind: String,
    title: Option<String>,
    href: String,
}

/// Where the nav and the NCX are, found once by [`package_files`] for `read_package` and
/// `oeb_view`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageFiles {
    /// The nav's path in the archive (`opf2.py` `OPF.epub3_nav`).
    pub nav_path: Option<String>,
    /// The NCX's href in Calibre's manifest (`reader.py` `_find_ncx`).
    pub ncx_href: Option<String>,
}

/// The nav and the NCX of the OPF `opf` in a folder `opf_dir` of an archive of `names`: the
/// same rules `oeb_view` applies, on the manifest as Calibre's reader first makes it.
pub(crate) fn package_files(opf: &Opf, opf_dir: &str, names: &BTreeSet<String>) -> PackageFiles {
    let input = Input::new(opf, opf_dir);
    let mut st = State::default();
    st.manifest_from_opf(&input.manifest, names);
    PackageFiles {
        nav_path: input.epub3_nav(opf, names),
        ncx_href: st.ncx_item(&input.toc).map(|i| st.item(i).href.clone()),
    }
}

#[derive(Default)]
struct State {
    items: Vec<Option<MItem>>,
    ids: BTreeMap<String, usize>,
    hrefs: BTreeMap<String, usize>,
    /// The live items by href in lower case (`Manifest.generate`, `_guide_from_opf`).
    lower_hrefs: BTreeMap<String, BTreeSet<usize>>,
    /// For `Manifest.generate`: by base, the first number not known to be taken. Cleared when
    /// an item is removed.
    next_id: BTreeMap<String, usize>,
    next_href: BTreeMap<(String, String), usize>,
    spine: Vec<(usize, bool)>,
    in_spine: BTreeSet<usize>,
    ppd: Option<String>,
    guide: BTreeMap<String, GRef>,
    toc: Vec<TocNode>,
    auto: bool,
    meta: Metadata,
}

impl State {
    fn item(&self, i: usize) -> &MItem {
        self.items[i].as_ref().expect("a live manifest item")
    }

    fn live(&self) -> impl Iterator<Item = (usize, &MItem)> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| it.as_ref().map(|it| (i, it)))
    }

    /// `Manifest.add`.
    fn add(&mut self, id: String, href: &str, media_type: &str, properties: Vec<String>) {
        let href = urlnormalize(href);
        let i = self.items.len();
        self.ids.insert(id.clone(), i);
        self.hrefs.insert(href.clone(), i);
        self.lower_hrefs
            .entry(href.to_lowercase())
            .or_default()
            .insert(i);
        self.items.push(Some(MItem {
            id,
            href,
            media_type: media_type.to_owned(),
            properties,
        }));
    }

    /// `Manifest.remove`, which also takes the item out of the spine.
    fn remove(&mut self, i: usize) {
        let Some(it) = self.items[i].take() else {
            return;
        };
        self.ids.remove(&it.id);
        self.hrefs.remove(&it.href);
        let lower = it.href.to_lowercase();
        if let Some(set) = self.lower_hrefs.get_mut(&lower) {
            set.remove(&i);
            if set.is_empty() {
                self.lower_hrefs.remove(&lower);
            }
        }
        self.next_id.clear();
        self.next_href.clear();
        if self.in_spine.remove(&i) {
            self.spine.retain(|s| s.0 != i);
        }
    }

    fn push_spine(&mut self, i: usize, linear: bool) {
        if self.in_spine.insert(i) {
            self.spine.push((i, linear));
        }
    }

    /// `Manifest.generate(id)`: `base`, else the first of `base1`, `base2`… not taken.
    fn generate_id(&mut self, base: &str) -> String {
        if !self.ids.contains_key(base) {
            return base.to_owned();
        }
        let n = self.next_id.entry(base.to_owned()).or_insert(1);
        loop {
            let id = format!("{base}{n}");
            if !self.ids.contains_key(&id) {
                return id;
            }
            *n += 1;
        }
    }

    /// `Manifest.generate(href=…)`: `href`, else the first of `base1.ext`, `base2.ext`… whose
    /// lower case no href has.
    fn generate_href(&mut self, href: &str) -> String {
        let href = urlnormalize(href);
        if !self.lower_hrefs.contains_key(&href.to_lowercase()) {
            return href;
        }
        let (base, ext) = splitext(&href);
        let n = self
            .next_href
            .entry((base.to_owned(), ext.to_owned()))
            .or_insert(1);
        loop {
            let out = format!("{base}{n}{ext}");
            if !self.lower_hrefs.contains_key(&out.to_lowercase()) {
                return out;
            }
            *n += 1;
        }
    }

    /// `reader.py` `_metadata_from_opf`, then `MergeMetadata` with no user metadata (the
    /// cover is set later, by [`State::set_cover`]).
    fn metadata_from_opf(&mut self, opf: &Opf) {
        let c = &opf.calibre;
        // `mi.language` falls back to the interface language (DIVERGENCES.md, "A book with no
        // language").
        self.meta.language = if c.languages.is_empty() {
            vec!["en".to_owned()]
        } else {
            c.languages.clone()
        };
        self.meta.title = vec![
            c.title
                .clone()
                .filter(|t| t != "Unknown")
                .unwrap_or_else(|| "Unknown".to_owned()),
        ];
        self.meta.creator = if c.authors.is_empty() || c.authors == ["Unknown"] {
            vec!["Unknown".to_owned()]
        } else {
            c.authors.clone()
        };
        self.meta.description = c.comments.iter().cloned().collect();
        self.meta.publisher = c.publisher.iter().cloned().collect();
        self.meta.date = c.pubdate.iter().map(|d| d.isoformat()).collect();
    }

    /// `_manifest_from_opf`: each item whose file exists, once per href, ids made unique.
    fn manifest_from_opf(&mut self, manifest: &[OpfItem], names: &BTreeSet<String>) {
        for it in manifest {
            let Some(href) = it.href.clone() else {
                continue;
            };
            let mt = media_type(&href, it.media_type.as_deref().unwrap_or(""));
            if self.hrefs.contains_key(&href) || !names.contains(&unquote(&href)) {
                continue;
            }
            let props = it
                .properties
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            match it.id.as_deref().filter(|i| !i.is_empty()) {
                None => {
                    let id = self.generate_id("id");
                    self.add(id, &href, &mt, props);
                }
                Some(id) if self.ids.contains_key(id) => {
                    let (id, href) = (self.generate_id(id), self.generate_href(&href));
                    self.add(id, &href, &mt, props);
                }
                Some(id) => self.add(id.to_owned(), &href, &mt, props),
            }
        }
    }

    /// `_spine_from_opf` and `_spine_add_extra`.
    fn spine_from_opf(
        &mut self,
        input: &Input,
        opf: &Opf,
        book: &mut Book,
        removed_cover: Option<&str>,
    ) -> Result<(), Error> {
        for r in &input.spine {
            let Some(&i) = r.idref.as_ref().and_then(|id| self.ids.get(id)) else {
                continue;
            };
            let mt = self.item(i).media_type.to_lowercase();
            let linear = is_linear(r.linear.as_deref());
            if is_doc(&mt) {
                self.push_spine(i, linear);
            } else if is_xml(&mt) {
                let href = self.item(i).href.clone();
                if book.doc(&href, false)?.is_some_and(|d| d.html_root) {
                    if let Some(it) = self.items[i].as_mut() {
                        it.media_type = XHTML_MIME.to_owned();
                    }
                    self.push_spine(i, linear);
                }
            }
        }
        if self.spine.is_empty() {
            return Err(Error::Package("Spine is empty".to_owned()));
        }
        // `_spine_add_extra`: documents linked from the spine's documents, not linear. Calibre
        // collects them in a set; the port keeps the order it finds them in (DIVERGENCES.md,
        // "Linked files added to the spine").
        let mut extras: Vec<usize> = Vec::new();
        // The spine, the extras and the new ones.
        let mut known: BTreeSet<usize> = self.in_spine.clone();
        let mut unchecked: Vec<usize> = self.spine.iter().map(|s| s.0).collect();
        while !unchecked.is_empty() {
            let mut new: Vec<usize> = Vec::new();
            for i in unchecked {
                let item = self.item(i).clone();
                if !is_doc(&item.media_type) {
                    continue;
                }
                let Some(info) = book.doc(&item.href, true)? else {
                    continue;
                };
                for link in &info.body_links {
                    let path = urldefrag(link).0;
                    if path.is_empty() {
                        continue;
                    }
                    let href = abshref(&item.href, &urlnormalize(path));
                    let Some(&found) = self.hrefs.get(&href) else {
                        continue;
                    };
                    if is_doc(&self.item(found).media_type) && known.insert(found) {
                        new.push(found);
                    }
                }
            }
            extras.extend(&new);
            unchecked = new;
        }
        for i in extras {
            if Some(self.item(i).href.as_str()) != removed_cover {
                self.push_spine(i, false);
            }
        }
        for v in &opf.ppds {
            if v == "ltr" || v == "rtl" {
                self.ppd = Some(v.clone());
            }
        }
        Ok(())
    }

    /// `Guide.add`: `Guide.Reference` normalizes the type and the href.
    fn guide_add(&mut self, key: &str, title: Option<String>, href: &str) {
        let lower = key.to_lowercase();
        let kind = if GUIDE_TYPES.contains(&lower.as_str()) {
            lower
        } else if !key.starts_with("other.") {
            format!("other.{key}")
        } else {
            key.to_owned()
        };
        let r = GRef {
            kind,
            title,
            href: urlnormalize(href),
        };
        self.guide.insert(key.to_owned(), r);
    }

    /// `_guide_from_opf`: references to files in the manifest, the first of each type.
    fn guide_from_opf(&mut self, input: &Input) {
        for r in input.guide.iter() {
            let (Some(href), Some(kind)) = (
                r.href.as_deref().filter(|h| !h.is_empty()),
                r.kind.as_deref().filter(|k| !k.is_empty()),
            ) else {
                continue;
            };
            let path = urlnormalize(urldefrag(href).0);
            let mut href = href.to_owned();
            if !self.hrefs.contains_key(&path) {
                // The first href in the manifest's order that matches in lower case.
                match self
                    .lower_hrefs
                    .get(&path.to_lowercase())
                    .and_then(|s| s.first())
                {
                    Some(&i) => href = self.item(i).href.clone(),
                    None => continue,
                }
            }
            if !self.guide.contains_key(kind) {
                self.guide_add(kind, r.title.clone(), &href);
            }
        }
    }

    /// `_find_ncx`: the spine's `toc` item, else the first item of the NCX type.
    fn ncx_item(&self, toc: &TocAttr) -> Option<usize> {
        match toc {
            TocAttr::Nav => None,
            TocAttr::Id(id) => self.ids.get(id).copied(),
            TocAttr::None => self
                .live()
                .find(|(_, it)| it.media_type == NCX_MIME)
                .map(|(i, _)| i),
        }
    }

    /// `_find_ncx`: the NCX comes out of the manifest; its text is `pkg.ncx_text`. Should the
    /// item differ from the one `read_package` found (DIVERGENCES.md, "An NCX listed in the
    /// spine as a document"), no NCX is read.
    fn find_ncx<'p>(&mut self, toc: &TocAttr, pkg: &'p Package) -> Option<(&'p str, String)> {
        let i = self.ncx_item(toc)?;
        let href = self.item(i).href.clone();
        self.remove(i);
        (pkg.files.ncx_href.as_deref() == Some(href.as_str()))
            .then_some(pkg.ncx_text.as_deref())
            .flatten()
            .map(|text| (text, href))
    }

    /// `_toc_from_navpoint`: an entry with no label gives way to its children; one with no
    /// target, or a target outside the manifest, is dropped unless it has children.
    fn toc_from_points(&self, points: &[NavPoint], base: &str, depth: usize) -> Vec<TocNode> {
        let mut out = Vec::new();
        if depth >= MAX_DEPTH {
            return out;
        }
        for p in points {
            let title = collapse(&p.label);
            if title.is_empty() {
                out.extend(self.toc_from_points(&p.children, base, depth + 1));
                continue;
            }
            let src = p.src.as_deref().filter(|s| !s.is_empty());
            if src.is_none() && p.children.is_empty() {
                continue;
            }
            let href = src.map_or_else(String::new, |s| abshref(base, &urlnormalize(s)));
            let mut path = urldefrag(&href).0.to_owned();
            if !path.is_empty() && !self.hrefs.contains_key(&path) {
                path = urlnormalize(&path);
            }
            if !href.is_empty() && !self.hrefs.contains_key(&path) && p.children.is_empty() {
                continue;
            }
            out.push(TocNode {
                title,
                href,
                children: self.toc_from_points(&p.children, base, depth + 1),
            });
        }
        out
    }

    /// `_toc_from_opf`: the nav or the NCX, else the guide's HTML contents; else Calibre
    /// generates the contents (`auto_generated_toc`, left to phase 2 Task 8). The OPF's tour
    /// (OEB 1) is not read (DIVERGENCES.md, "Calibre steps the book view leaves out").
    fn toc_from_opf(
        &mut self,
        ncx: Option<(&str, String)>,
        nav: Option<&NavToc>,
        book: &mut Book,
    ) -> Result<(), Error> {
        self.auto = false;
        let points = match (nav, ncx) {
            (Some(nav), _) => Some((nav.points.clone(), nav.base.clone())),
            (None, Some((text, href))) => read_xml(text, toc::ncx_points).map(|p| (p, href)),
            (None, None) => None,
        };
        if let Some((points, base)) = points {
            self.toc = self.toc_from_points(&points, &base, 0);
            return Ok(());
        }
        if self.toc_from_html(book)? {
            return Ok(());
        }
        self.auto = true;
        Ok(())
    }

    /// `_toc_from_html`: the links of the guide's `toc` document, under its fragment.
    fn toc_from_html(&mut self, book: &mut Book) -> Result<bool, Error> {
        let Some(g) = self.guide.get("toc") else {
            return Ok(false);
        };
        let (path, frag) = urldefrag(&g.href);
        let (path, frag) = (path.to_owned(), frag.to_owned());
        let Some(&i) = self.hrefs.get(&path) else {
            return Ok(false);
        };
        let base = self.item(i).href.clone();
        // `item.data`: `parse_html` for a document (`base.py` `_parse_xhtml`).
        let Some(dom) = book
            .read_bytes(&unquote(&base))?
            .and_then(|b| dom::parse(&b).ok())
        else {
            return Ok(false);
        };
        self.toc = html_toc(&dom, &base, &frag, &self.hrefs);
        Ok(true)
    }

    /// `epub_input.py` `postprocess_book` and `plumber.py`: the first contents entry that
    /// points at the removed HTML cover goes, if it has no children (not for EPUB output).
    fn drop_cover_entry(&mut self, removed_cover: Option<&str>) {
        fn find(nodes: &[TocNode], rc: &str, path: &mut Vec<usize>) -> bool {
            for (i, n) in nodes.iter().enumerate() {
                path.push(i);
                if (!n.href.is_empty() && urldefrag(&n.href).0 == rc)
                    || (path.len() < MAX_DEPTH && find(&n.children, rc, path))
                {
                    return true;
                }
                path.pop();
            }
            false
        }
        let Some(rc) = removed_cover else { return };
        let mut path = Vec::new();
        if !find(&self.toc, rc, &mut path) {
            return;
        }
        let (last, parents) = path.split_last().expect("a found path");
        let mut nodes = &mut self.toc;
        for &p in parents {
            nodes = &mut nodes[p].children;
        }
        if nodes[*last].children.is_empty() {
            nodes.remove(*last);
        }
    }

    /// `transforms/guide.py` `Clean`: `start` becomes `text`, and types Calibre does not know
    /// go. Calibre also makes a Microsoft cover image the cover; the port does not
    /// (DIVERGENCES.md, "Calibre steps the book view leaves out").
    fn clean_guide(&mut self) {
        if !self.guide.contains_key("text")
            && let Some(start) = self.guide.get("start").cloned()
        {
            self.guide_add("text", start.title, &start.href);
            self.guide.remove("start");
        }
        self.guide.retain(|key, g| {
            let k = key.to_lowercase();
            KNOWN_GUIDE_TYPES.contains(&k.as_str())
                || k.starts_with(CUSTOM_GUIDE_TYPE_PREFIX)
                || g.title
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase() == "start")
        });
    }

    /// `MergeMetadata.set_cover` with no cover given: the guide's cover is the cover image.
    fn set_cover(&mut self) {
        let Some(href) = self.guide.get("cover").map(|g| g.href.clone()) else {
            self.meta.cover.clear();
            return;
        };
        let id = match self.hrefs.get(&href) {
            Some(&i) => self.item(i).id.clone(),
            None => {
                let id = self.generate_id("cover");
                self.add(id.clone(), &href, "image/jpeg", Vec::new());
                id
            }
        };
        self.meta.cover = vec![id];
    }

    /// The documents `CSSFlattener` flattens: the spine, the title page and the nav.
    fn flattened(&self, nav_href: Option<&str>) -> Vec<usize> {
        let mut items: Vec<usize> = self.spine.iter().map(|s| s.0).collect();
        let titlepage = self
            .guide
            .get("titlepage")
            .and_then(|g| self.hrefs.get(urldefrag(&g.href).0).copied());
        let nav = nav_href.and_then(|h| self.hrefs.get(h).copied());
        for i in [titlepage, nav].into_iter().flatten() {
            if !items.contains(&i) {
                items.push(i);
            }
        }
        items
    }

    /// The files the flattened stylesheets name: every `url()` of the stylesheets and
    /// `<style>` elements of the flattened documents, followed through `@import`. Calibre
    /// keeps only what a rule in use names (DIVERGENCES.md, "Which files stay in the
    /// manifest").
    fn flattened_css_urls(
        &self,
        book: &mut Book,
        docs: &[usize],
    ) -> Result<BTreeSet<String>, Error> {
        fn sheet(
            st: &State,
            book: &mut Book,
            href: &str,
            depth: usize,
            seen: &mut BTreeSet<String>,
            out: &mut BTreeSet<String>,
        ) -> Result<(), Error> {
            if depth >= MAX_DEPTH || !seen.insert(href.to_owned()) {
                return Ok(());
            }
            let Some(&i) = st.hrefs.get(href) else {
                return Ok(());
            };
            if !OEB_STYLES.contains(&st.item(i).media_type.as_str()) {
                return Ok(());
            }
            let Some(text) = book.read(&unquote(href))? else {
                return Ok(());
            };
            let (imports, urls) = sheet_refs(&text);
            out.extend(urls.iter().map(|u| abshref(href, &urlnormalize(u))));
            for imp in imports {
                sheet(
                    st,
                    book,
                    &abshref(href, &urlnormalize(&imp)),
                    depth + 1,
                    seen,
                    out,
                )?;
            }
            Ok(())
        }
        let mut out = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for &i in docs {
            let href = self.item(i).href.clone();
            let Some(info) = book.doc(&href, is_doc(&self.item(i).media_type))? else {
                continue;
            };
            for s in &info.sheets {
                sheet(
                    self,
                    book,
                    &abshref(&href, &urlnormalize(s)),
                    0,
                    &mut seen,
                    &mut out,
                )?;
            }
            for style in &info.styles {
                let (imports, urls) = sheet_refs(style);
                out.extend(urls.iter().map(|u| abshref(&href, &urlnormalize(u))));
                for imp in imports {
                    let target = abshref(&href, &urlnormalize(&imp));
                    sheet(self, book, &target, 0, &mut seen, &mut out)?;
                }
            }
        }
        Ok(out)
    }

    /// `CSSFlattener.replace_css` and `collect_global_css`: the book's stylesheets go, and
    /// Calibre's `stylesheet.css` and `page_styles.css` come in. Calibre writes one
    /// `page_styles*.css` per distinct set of `@page` and `@font-face` rules; the port writes
    /// one until the css module can tell them apart (DIVERGENCES.md, "One page_styles.css until
    /// the CSS is read"). Returns the two new items.
    fn replace_css(&mut self) -> [usize; 2] {
        let styles: Vec<usize> = self
            .live()
            .filter(|(_, it)| OEB_STYLES.contains(&it.media_type.as_str()))
            .map(|(i, _)| i)
            .collect();
        for i in styles {
            self.remove(i);
        }
        let mut made = [0; 2];
        for (n, (id, href)) in [("css", "stylesheet.css"), ("page_css", "page_styles.css")]
            .into_iter()
            .enumerate()
        {
            let (id, href) = (self.generate_id(id), self.generate_href(href));
            made[n] = self.items.len();
            self.add(id, &href, CSS_MIME, Vec::new());
        }
        made
    }

    /// `trimmanifest.py` `ManifestTrimmer`: what the metadata, guide and spine name, and what
    /// those link to, stays.
    fn trim(
        &mut self,
        book: &mut Book,
        sheets: [usize; 2],
        css_urls: &BTreeSet<String>,
    ) -> Result<(), Error> {
        let mut used: BTreeSet<usize> = BTreeSet::new();
        let m = &self.meta;
        for v in [
            &m.title,
            &m.creator,
            &m.description,
            &m.language,
            &m.publisher,
            &m.date,
            &m.cover,
        ]
        .into_iter()
        .flatten()
        {
            if let Some(&i) = self.hrefs.get(v).or_else(|| self.ids.get(v)) {
                used.insert(i);
            }
        }
        for g in self.guide.values() {
            if let Some(&i) = self.hrefs.get(urldefrag(&g.href).0) {
                used.insert(i);
            }
        }
        used.extend(self.spine.iter().map(|s| s.0));
        // Every flattened document links to Calibre's two stylesheets.
        used.extend(sheets);
        let mut unchecked = used.clone();
        while !unchecked.is_empty() {
            let mut new = BTreeSet::new();
            for i in unchecked {
                let item = self.item(i).clone();
                let links: Vec<String> = if is_doc(&item.media_type) || is_xml(&item.media_type) {
                    match book.doc(&item.href, is_doc(&item.media_type))? {
                        Some(info) => info
                            .links
                            .iter()
                            .map(|l| abshref(&item.href, &urlnormalize(l)))
                            .collect(),
                        None => Vec::new(),
                    }
                } else if sheets.contains(&i) {
                    css_urls.iter().cloned().collect()
                } else {
                    Vec::new()
                };
                for l in links {
                    if let Some(&f) = self.hrefs.get(&l)
                        && !used.contains(&f)
                    {
                        new.insert(f);
                    }
                }
            }
            used.extend(&new);
            unchecked = new;
        }
        let unused: Vec<usize> = self
            .live()
            .map(|(i, _)| i)
            .filter(|i| !used.contains(i))
            .collect();
        for i in unused {
            self.remove(i);
        }
        Ok(())
    }

    fn into_oeb(self) -> Oeb {
        let spine = self
            .spine
            .iter()
            .map(|&(i, linear)| SpineRef {
                href: self.item(i).href.clone(),
                linear,
            })
            .collect();
        let mut manifest: Vec<Item> = self
            .live()
            .map(|(_, it)| Item {
                id: it.id.clone(),
                href: it.href.clone(),
                media_type: it.media_type.clone(),
                properties: it.properties.clone(),
            })
            .collect();
        manifest.sort_by(|a, b| (&a.href, &a.id).cmp(&(&b.href, &b.id)));
        let mut guide: Vec<GuideRef> = self
            .guide
            .values()
            .map(|g| GuideRef {
                kind: g.kind.clone(),
                href: g.href.clone(),
            })
            .collect();
        guide.sort_by(|a, b| (&a.kind, &a.href).cmp(&(&b.kind, &b.href)));
        Oeb {
            spine,
            manifest,
            toc: self.toc,
            auto_generated_toc: self.auto,
            guide,
            metadata: self.meta,
            page_progression_direction: self.ppd,
        }
    }
}

/// Calibre 9.18's book view of the EPUB `pkg` read from `zip`, as kfxgen's output plugin
/// receives it.
pub fn oeb_view(pkg: &Package, zip: &mut EpubReader) -> Result<Oeb, Error> {
    let opf = &pkg.opf;
    let opf_dir = zip.opf_dir.clone();
    let names = zip.names().into_iter().collect();
    let mut book = Book {
        zip,
        names,
        docs: BTreeMap::new(),
    };

    // `epub_input.py` `convert`.
    let mut input = Input::new(opf, &opf_dir);
    let removed_cover = if opf.version >= 3.0 {
        input.rationalize_cover3(opf)
    } else {
        input.rationalize_cover2(opf)
    };
    let removed_cover = removed_cover.as_deref();
    let nav = match (&pkg.files.nav_path, &pkg.nav_text) {
        (Some(path), Some(text)) => input.convert_epub3_nav(path, text, &book, removed_cover),
        _ => None,
    };
    input.clean_spine()?;

    // `reader.py` `OEBReader._all_from_opf`.
    let mut st = State::default();
    st.metadata_from_opf(opf);
    st.manifest_from_opf(&input.manifest, &book.names);
    st.spine_from_opf(&input, opf, &mut book, removed_cover)?;
    st.guide_from_opf(&input);
    let ncx = st.find_ncx(&input.toc, pkg);
    st.toc_from_opf(ncx, nav.as_ref(), &mut book)?;

    // `postprocess_book`, then `plumber.py`'s transforms.
    st.drop_cover_entry(removed_cover);
    st.clean_guide();
    st.set_cover();
    let docs = st.flattened(nav.as_ref().map(|n| n.nav_href.as_str()));
    let css_urls = st.flattened_css_urls(&mut book, &docs)?;
    let sheets = st.replace_css();
    st.trim(&mut book, sheets, &css_urls)?;
    Ok(st.into_oeb())
}
