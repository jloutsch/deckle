// SPDX-License-Identifier: GPL-3.0-only
//! The OPF as written, read the way Calibre 9.18 reads it: its manifest, spine and guide
//! (`ebooks/oeb/reader.py`), its metadata (`ebooks/metadata/opf2.py` for an EPUB 2 package,
//! `ebooks/metadata/opf3.py` for EPUB 3), dates (`utils/date.py`), languages
//! (`utils/localization.py`) and hrefs (`ebooks/oeb/base.py`).

use std::collections::BTreeSet;

use deckle_kfx::epub_zip::{EpubReader, join, percent_decode};
use deckle_limits::XmlError;
use deckle_limits::roxmltree::{Document, Node, ParsingOptions};

use super::{Error, GuideRef, Item, Metadata, Package, SpineRef};

pub(crate) const OPF2_NS: &str = "http://www.idpf.org/2007/opf";
/// `reader.py` `_read_opf` maps every `…/oeb-package/1.0` spelling to OPF 1.
const OPF1_NS_PREFIX: &str = "http://openebook.org/namespaces/oeb-package/1.0";
/// `base.py` `DC_NSES`: DC 1.1, 1.0 and 0.9, all read as DC 1.1 (`_clean_opf`).
const DC_NSES: [&str; 3] = [
    "http://purl.org/dc/elements/1.1/",
    "http://purl.org/dc/elements/1.0/",
    "http://purl.org/metadata/dublin_core",
];
pub(crate) const NCX_MIME: &str = "application/x-dtbncx+xml";
/// `opf3.py` `reserved_prefixes['marc']`.
const MARC_NS: &str = "http://id.loc.gov/vocabulary/";

/// Options for every parse: a DOCTYPE is allowed (XHTML files carry one);
/// `deckle_limits::parse_xml` refuses entities that hold markup.
pub(crate) fn parse_options<'a>() -> ParsingOptions<'a> {
    ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    }
}

/// `text` parsed through `deckle_limits::parse_xml` and handed to `f`. Calibre parses XML with
/// libxml2 in recover mode (`utils/xml_parse.py` `safe_xml_fromstring`); where `text` is not
/// well-formed only for its end tags, it is read as libxml2 recovers it
/// ([`recover_end_tags`]). Otherwise the parser's error.
pub(crate) fn parse_with<R>(text: &str, f: impl FnOnce(&Document) -> R) -> Result<R, XmlError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match deckle_limits::parse_xml(text, parse_options()) {
        Ok(doc) => Ok(f(&doc)),
        Err(XmlError::Parse(e)) => {
            let Some(fixed) = recover_end_tags(text) else {
                return Err(XmlError::Parse(e));
            };
            match deckle_limits::parse_xml(&fixed, parse_options()) {
                Ok(doc) => Ok(f(&doc)),
                Err(refused @ (XmlError::TooDeep | XmlError::EntityMarkup)) => Err(refused),
                Err(_) => Err(XmlError::Parse(e)),
            }
        }
        Err(e) => Err(e),
    }
}

/// End of a markup declaration (`<!DOCTYPE …>`), past its internal subset and quotes.
fn declaration_end(s: &str) -> Option<usize> {
    let (mut depth, mut quote) = (0usize, None);
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[') => depth += 1,
            (None, ']') => depth = depth.saturating_sub(1),
            (None, '>') if depth == 0 => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// End of a start tag, past quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// `text` with its end tags as libxml2's recover mode reads them (`xmlParseEndTag2`): an end
/// tag closes the element that is open, whatever it names; once the root element is closed
/// the rest is dropped (`XML_ERR_DOCUMENT_END` stops the parse); elements still open at the
/// end are closed. None when a comment, CDATA section, processing instruction, declaration or
/// tag does not end.
pub(crate) fn recover_end_tags(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len() + 64);
    let mut open: Vec<&str> = Vec::new();
    let (mut i, mut copied) = (0, 0);
    while let Some(off) = text[i..].find('<') {
        let s = i + off;
        let rest = &text[s..];
        let end = if rest.starts_with("<!--") {
            s + rest.find("-->")? + 3
        } else if rest.starts_with("<![CDATA[") {
            s + rest.find("]]>")? + 3
        } else if rest.starts_with("<?") {
            s + rest.find("?>")? + 2
        } else if rest.starts_with("<!") {
            s + declaration_end(rest)?
        } else if rest.starts_with("</") {
            let end = s + rest.find('>')? + 1;
            out.push_str(&text[copied..s]);
            copied = end;
            match open.pop() {
                Some(name) => {
                    out.push_str("</");
                    out.push_str(name);
                    out.push('>');
                }
                None => return Some(out),
            }
            if open.is_empty() {
                return Some(out);
            }
            end
        } else {
            let end = s + tag_end(rest)?;
            let name = text[s + 1..end]
                .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .next()
                .unwrap_or("");
            if !text[..end].ends_with("/>") {
                open.push(name);
            } else if open.is_empty() {
                out.push_str(&text[copied..end]);
                return Some(out);
            }
            end
        };
        i = end;
    }
    out.push_str(&text[copied..]);
    for name in open.iter().rev() {
        out.push_str("</");
        out.push_str(name);
        out.push('>');
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------
// The OPF model
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct OpfItem {
    pub id: Option<String>,
    pub href: Option<String>,
    /// `media-type`, else the misspelt `mediatype` (`reader.py` `_manifest_from_opf`).
    pub media_type: Option<String>,
    pub properties: String,
}

#[derive(Debug, Clone)]
pub(crate) struct OpfItemref {
    pub idref: Option<String>,
    pub linear: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct OpfRef {
    pub kind: Option<String>,
    pub title: Option<String>,
    pub href: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct OpfSite {
    pub title: Option<String>,
    pub href: Option<String>,
}

/// The metadata Calibre's OPF readers make (`Metadata` in `ebooks/metadata/book/base.py`), for
/// the fields `oeb.json` records.
#[derive(Debug, Clone, Default)]
pub(crate) struct CalibreMeta {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub comments: Option<String>,
    pub publisher: Option<String>,
    pub languages: Vec<String>,
    pub pubdate: Option<Date>,
}

/// One parsed OPF.
#[derive(Debug, Clone)]
pub(crate) struct Opf {
    /// `OPF.package_version`: the `version` attribute as a number, 0 if it isn't one.
    pub version: f64,
    pub manifest: Vec<OpfItem>,
    pub spine: Vec<OpfItemref>,
    pub spine_toc: Option<String>,
    /// The spine's `page-progression-direction` attributes, as written.
    pub ppds: Vec<String>,
    pub guide: Vec<OpfRef>,
    /// The first `<tour>`'s sites (OEB 1).
    pub tour: Option<Vec<OpfSite>>,
    /// `content` of the first metadata `meta` whose `name` holds "cover" (`OPF.raster_cover`).
    pub cover_meta: Option<String>,
    /// `opf3.py` `read_prefixes`: the reserved prefixes, then the package's `prefix`.
    pub prefixes: Vec<(String, String)>,
    pub raw: Metadata,
    pub calibre: CalibreMeta,
}

fn opf_ns(ns: Option<&str>) -> bool {
    match ns {
        None | Some("") => true,
        Some(ns) => ns == OPF2_NS || ns.starts_with(OPF1_NS_PREFIX),
    }
}

/// An OPF element `name` (`_clean_opf` puts the empty and OPF 1 namespaces into OPF 2).
fn is_opf(n: &Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && opf_ns(n.tag_name().namespace())
}

/// A Dublin Core element `name`, any of `DC_NSES`, its name in any case (`_clean_opf`).
fn is_dc(n: &Node, name: &str) -> bool {
    n.is_element()
        && n.tag_name()
            .namespace()
            .is_some_and(|ns| DC_NSES.contains(&ns))
        && n.tag_name().name().eq_ignore_ascii_case(name)
}

fn attr(n: &Node, name: &str) -> Option<String> {
    n.attribute(name).map(str::to_owned)
}

/// XPath `string()`: all the text inside `n`.
pub(crate) fn string_value(n: &Node) -> String {
    n.descendants()
        .filter(|d| d.is_text())
        .filter_map(|d| d.text())
        .collect()
}

/// lxml's `.text`: the text before the first child.
fn direct_text(n: &Node) -> Option<String> {
    n.first_child()
        .filter(|c| c.is_text())
        .and_then(|c| c.text())
        .map(str::to_owned)
}

/// `opf3.py` `reserved_prefixes`.
const RESERVED_PREFIXES: [(&str, &str); 8] = [
    ("dcterms", "http://purl.org/dc/terms/"),
    ("epubsc", "http://idpf.org/epub/vocab/sc/#"),
    ("marc", MARC_NS),
    ("media", "http://www.idpf.org/epub/vocab/overlays/#"),
    (
        "onix",
        "http://www.editeur.org/ONIX/book/codelists/current.html#",
    ),
    ("rendition", "http://www.idpf.org/vocab/rendition/#"),
    ("schema", "http://schema.org/"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
];

/// `opf3.py` `read_prefixes`: `parse_prefixes` reads `(\S+): \s*(\S+)`.
fn read_prefixes(root: &Node) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = RESERVED_PREFIXES
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    if let Some(p) = root.attribute("prefix") {
        let words: Vec<&str> = p.split_whitespace().collect();
        let mut i = 0;
        while i + 1 < words.len() {
            if let Some(name) = words[i].strip_suffix(':').filter(|n| !n.is_empty()) {
                out.retain(|(k, _)| k != name);
                out.push((name.to_owned(), words[i + 1].to_owned()));
                i += 2;
            } else {
                i += 1;
            }
        }
    }
    out
}

impl Opf {
    /// The URL a prefix stands for.
    fn prefix(&self, p: &str) -> Option<&str> {
        self.prefixes
            .iter()
            .find(|(k, _)| k == p)
            .map(|(_, v)| v.as_str())
    }

    /// `opf3.py` `expand_prefix` of one property with the package's prefixes.
    pub(crate) fn expand_prefix(&self, raw: &str) -> String {
        match raw.rsplit_once(':') {
            Some((p, r)) if !p.is_empty() && !r.is_empty() => {
                format!("{}:{r}", self.prefix(p).unwrap_or(p))
            }
            _ => raw.to_owned(),
        }
    }

    /// Parses the OPF `text` at `file` (`reader.py` `_read_opf`).
    pub(crate) fn parse(text: &str, file: &str) -> Result<Opf, Error> {
        parse_with(text, Opf::from_doc).map_err(|source| Error::Xml {
            file: file.to_owned(),
            source,
        })?
    }

    fn from_doc(doc: &Document) -> Result<Opf, Error> {
        let root = doc.root_element();
        let ns = root.tag_name().namespace();
        if !opf_ns(ns) {
            return Err(Error::Package(format!(
                "Invalid namespace {:?} for OPF document",
                ns.unwrap_or("")
            )));
        }
        let version = root
            .attribute("version")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.0);
        let children = |name: &'static str| {
            root.children()
                .filter(move |c| is_opf(c, name))
                .flat_map(|c| c.children())
        };
        let manifest = children("manifest")
            .filter(|c| is_opf(c, "item"))
            .map(|c| OpfItem {
                id: attr(&c, "id"),
                href: attr(&c, "href"),
                media_type: attr(&c, "media-type").or_else(|| attr(&c, "mediatype")),
                properties: attr(&c, "properties").unwrap_or_default(),
            })
            .collect();
        let spine = children("spine")
            .filter(|c| is_opf(c, "itemref"))
            .map(|c| OpfItemref {
                idref: attr(&c, "idref"),
                linear: attr(&c, "linear"),
            })
            .collect();
        let spines: Vec<Node> = root.children().filter(|c| is_opf(c, "spine")).collect();
        let spine_toc = spines.iter().find_map(|s| attr(s, "toc"));
        let ppds = spines
            .iter()
            .filter_map(|s| attr(s, "page-progression-direction"))
            .collect();
        let guide = children("guide")
            .filter(|c| is_opf(c, "reference"))
            .map(|c| OpfRef {
                kind: attr(&c, "type"),
                title: attr(&c, "title"),
                href: attr(&c, "href"),
            })
            .collect();
        let tour = children("tours").find(|c| is_opf(c, "tour")).map(|t| {
            t.children()
                .filter(|c| is_opf(c, "site"))
                .map(|c| OpfSite {
                    title: attr(&c, "title"),
                    href: attr(&c, "href"),
                })
                .collect()
        });
        // `_clean_opf`: everything under the package's metadata elements, flattened.
        let meta: Vec<Node> = root
            .children()
            .filter(|c| is_opf(c, "metadata"))
            .flat_map(|m| m.descendants().skip(1))
            .filter(|n| n.is_element())
            .collect();
        let cover_meta = meta
            .iter()
            .find(|n| {
                n.tag_name().name().to_lowercase().contains("meta")
                    && n.attribute("name")
                        .is_some_and(|v| v.to_lowercase().contains("cover"))
                    && n.attribute("content").is_some()
            })
            .and_then(|n| attr(n, "content"));
        let raw = raw_metadata(&meta);
        let prefixes = read_prefixes(&root);
        let calibre = if version >= 3.0 {
            opf3_metadata(&prefixes, &meta)
        } else {
            opf2_metadata(&meta)
        };
        Ok(Opf {
            version,
            manifest,
            spine,
            spine_toc,
            ppds,
            guide,
            tour,
            cover_meta,
            prefixes,
            raw,
            calibre,
        })
    }
}

/// The metadata as written: each Dublin Core field's text, trimmed, and the `cover` meta.
fn raw_metadata(meta: &[Node]) -> Metadata {
    let field = |name: &str| -> Vec<String> {
        meta.iter()
            .filter(|n| is_dc(n, name))
            .map(|n| string_value(n).trim().to_owned())
            .collect()
    };
    Metadata {
        title: field("title"),
        creator: field("creator"),
        description: field("description"),
        language: field("language"),
        publisher: field("publisher"),
        date: field("date"),
        cover: meta
            .iter()
            .filter(|n| is_opf(n, "meta") && n.attribute("name") == Some("cover"))
            .filter_map(|n| attr(n, "content"))
            .collect(),
    }
}

/// `opf3.py` `normalize_whitespace`.
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `opf3.py` `read_metadata`, for the fields `oeb.json` records.
fn opf3_metadata(prefixes: &[(String, String)], meta: &[Node]) -> CalibreMeta {
    let prefix = |p: &str| -> Option<String> {
        prefixes
            .iter()
            .find(|(k, _)| k == p)
            .map(|(_, v)| v.clone())
    };
    // `read_refines`.
    let refines: Vec<&Node> = meta
        .iter()
        .filter(|n| is_opf(n, "meta"))
        .filter(|n| n.attribute("refines").is_some_and(|r| r.starts_with('#')))
        .collect();
    // `properties_for_id_with_scheme`: (property, scheme namespace, scheme, value).
    let props = |id: Option<&str>| -> Vec<(String, Option<String>, Option<String>, String)> {
        let Some(id) = id else { return Vec::new() };
        refines
            .iter()
            .filter(|m| m.attribute("refines").and_then(|r| r.strip_prefix('#')) == Some(id))
            .filter_map(|m| {
                let key = m.attribute("property").filter(|k| !k.is_empty())?;
                let val = direct_text(m).unwrap_or_default().trim().to_owned();
                if val.is_empty() {
                    return None;
                }
                let mut scheme = m
                    .attribute("scheme")
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                let mut scheme_ns = None;
                if let Some(s) = scheme.clone()
                    && let Some((p, r)) = s.split_once(':')
                    && !p.is_empty()
                    && !r.is_empty()
                    && let Some(ns) = prefix(p)
                {
                    scheme_ns = Some(ns);
                    scheme = Some(r.to_owned());
                }
                Some((key.to_owned(), scheme_ns, scheme, val))
            })
            .collect()
    };
    let prop = |id: Option<&str>, key: &str| -> Option<String> {
        props(id)
            .into_iter()
            .rev()
            .find(|p| p.0 == key)
            .map(|p| p.3)
    };
    let dc = |name: &str| -> Vec<&Node> { meta.iter().filter(|n| is_dc(n, name)).collect() };

    // `find_main_title`, `find_subtitle`, `read_title`.
    let titles: Vec<&Node> = dc("title")
        .into_iter()
        .filter(|t| direct_text(t).is_some_and(|s| !s.trim().is_empty()))
        .collect();
    let main = titles
        .iter()
        .find(|t| prop(t.attribute("id"), "title-type").as_deref() == Some("main"))
        .or(titles.first());
    let title = main.map(|m| {
        let mut ans = direct_text(m).unwrap_or_default().trim().to_owned();
        let sub = titles.iter().find(|t| {
            let q = prop(t.attribute("id"), "title-type").unwrap_or_default();
            q.contains("subtitle") || q.contains("sub-title")
        });
        if let Some(st) = sub
            && st.id() != m.id()
        {
            ans.push_str(": ");
            ans.push_str(direct_text(st).unwrap_or_default().trim());
        }
        normalize_whitespace(&ans)
    });

    // `read_authors`: (name, sort, display-seq).
    type Author = (String, Option<String>, i64);
    let mut roled: Vec<Author> = Vec::new();
    let mut unroled: Vec<Author> = Vec::new();
    let mut editors: Vec<Author> = Vec::new();
    for item in dc("creator") {
        let val = direct_text(item).unwrap_or_default().trim().to_owned();
        if val.is_empty() {
            continue;
        }
        let ps = props(item.attribute("id"));
        let author = || -> Author {
            let file_as = ps
                .iter()
                .find(|p| p.0 == "file-as")
                .map(|p| p.3.clone())
                .or_else(|| item.attribute((OPF2_NS, "file-as")).map(str::to_owned))
                .filter(|s| !s.is_empty());
            let seq = ps
                .iter()
                .find(|p| p.0 == "display-seq")
                .and_then(|p| p.3.trim().parse().ok())
                .unwrap_or(0);
            (
                normalize_whitespace(&val),
                file_as.map(|s| normalize_whitespace(&s)),
                seq,
            )
        };
        let roles: Vec<_> = ps.iter().filter(|p| p.0 == "role").collect();
        // `is_relators_role`.
        let is_role = |q: &str| {
            roles.iter().any(|(_, ns, scheme, role)| {
                role.to_lowercase() == q
                    && (ns.is_none()
                        || (ns.as_deref() == Some(MARC_NS)
                            && scheme.as_deref() == Some("relators")))
            })
        };
        if !roles.is_empty() {
            if is_role("aut") {
                roled.push(author());
            }
            if is_role("edt") {
                let a = author();
                match editors.iter_mut().find(|e| e.0 == a.0) {
                    Some(e) => *e = a,
                    None => editors.push(a),
                }
            }
        } else if let Some(r) = item.attribute((OPF2_NS, "role")) {
            if r.to_lowercase() == "aut" {
                roled.push(author());
            }
        } else {
            unroled.push(author());
        }
    }
    let mut authors = if !roled.is_empty() {
        roled
    } else if !unroled.is_empty() {
        unroled
    } else {
        editors
    };
    let mut seen = Vec::new();
    authors.retain(|a| {
        let new = !seen.contains(a);
        if new {
            seen.push(a.clone());
        }
        new
    });
    authors.sort_by_key(|a| a.2);

    // `read_languages`.
    let mut languages: Vec<String> = Vec::new();
    for l in dc("language") {
        if let Some(v) = canonicalize_lang(direct_text(l).unwrap_or_default().trim())
            && !languages.contains(&v)
            && v != "und"
        {
            languages.push(v);
        }
    }
    // `read_pubdate`, then `read_metadata` drops an undefined date.
    let pubdate = dc("date")
        .into_iter()
        .filter_map(|d| {
            let val = direct_text(d).unwrap_or_default().trim().to_owned();
            (!val.is_empty()).then(|| parse_date(&val, true)).flatten()
        })
        .next()
        .filter(|d| !d.is_undefined());
    // `read_comments`.
    let mut comments = String::new();
    for d in dc("description") {
        if let Some(t) = direct_text(d).filter(|t| !t.is_empty()) {
            comments.push('\n');
            comments.push_str(t.trim());
        }
    }
    let comments = Some(comments.trim().to_owned()).filter(|c| !c.is_empty());
    // `read_publisher`.
    let publisher = dc("publisher")
        .into_iter()
        .find_map(|p| direct_text(p).filter(|t| !t.is_empty()))
        .map(|t| normalize_whitespace(&t))
        .filter(|p| !p.is_empty());
    CalibreMeta {
        title,
        authors: authors.into_iter().map(|a| a.0).collect(),
        comments,
        publisher,
        languages,
        pubdate,
    }
}

/// `opf2.py` `OPF.get_text`: a `meta`'s `content`, else the element's text.
fn opf2_text(n: &Node) -> String {
    if n.tag_name().name().to_lowercase().ends_with("meta")
        && let Some(c) = n.attribute("content")
    {
        return c.to_owned();
    }
    string_value(n)
}

/// `opf2.py`: `MetadataField(name)`, the last element named `…name` or a `calibre:name` meta,
/// its text stripped.
fn opf2_field(meta: &[Node], name: &str) -> Option<String> {
    meta.iter()
        .rfind(|n| {
            let tag = n.tag_name().name().to_lowercase();
            tag.ends_with(name)
                || (tag.ends_with("meta")
                    && n.attribute("name")
                        .is_some_and(|v| v.to_lowercase() == format!("calibre:{name}")))
        })
        .map(|n| opf2_text(n).trim().to_owned())
}

/// `ebooks/metadata/__init__.py` `string_to_authors`, with the default `authors_split_regex`.
fn string_to_authors(raw: &str) -> Vec<String> {
    static SPLIT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i),?\s+(and|with)\s+").unwrap());
    let raw = raw.replace("&&", "\u{ffff}");
    let raw = SPLIT.replace_all(&raw, "&");
    raw.split('&')
        .map(|a| a.trim().replace('\u{ffff}', "&"))
        .filter(|a| !a.is_empty())
        .collect()
}

/// `MetaInformation(opf)` for an EPUB 2 package (`opf2.py` `OPF`, `book/base.py`
/// `smart_update`), for the fields `oeb.json` records.
fn opf2_metadata(meta: &[Node]) -> CalibreMeta {
    let named = |part: &str| -> Vec<&Node> {
        meta.iter()
            .filter(|n| n.tag_name().name().to_lowercase().contains(part))
            .collect()
    };
    // `OPF.title`.
    let title = named("title").into_iter().find_map(|n| {
        let t = opf2_text(n);
        let t = t.trim();
        (!t.is_empty()).then(|| normalize_whitespace(t))
    });
    // `OPF.authors`: creators with role aut or none, else editors.
    let role_is = |n: &Node, r: Option<&str>| {
        let (a, b) = (n.attribute("role"), n.attribute((OPF2_NS, "role")));
        match r {
            Some(r) => a == Some(r) || b == Some(r),
            None => a.is_none() && b.is_none(),
        }
    };
    let creators = named("creator");
    let mut authors: Vec<String> = creators
        .iter()
        .filter(|n| role_is(n, Some("aut")) || role_is(n, None))
        .flat_map(|n| string_to_authors(&opf2_text(n)))
        .collect();
    if authors.is_empty() {
        authors = creators
            .iter()
            .filter(|n| role_is(n, Some("edt")))
            .flat_map(|n| string_to_authors(&opf2_text(n)))
            .collect();
    }
    // `OPF.languages`.
    let languages = meta
        .iter()
        .filter(|n| n.tag_name().name() == "language")
        .filter_map(|n| {
            let t = opf2_text(n);
            canonicalize_lang(t.trim())
        })
        .filter(|l| l != "und")
        .collect();
    // `OPF.pubdate`: the earliest date that parses; an empty one is the undefined date.
    let pubdate = named("date")
        .into_iter()
        .filter_map(|n| parse_date(string_value(n).trim(), false))
        .min()
        .filter(|d| !d.is_undefined());
    CalibreMeta {
        title,
        authors,
        comments: opf2_field(meta, "description").filter(|c| !c.is_empty()),
        publisher: opf2_field(meta, "publisher").filter(|p| !p.is_empty()),
        languages,
        pubdate,
    }
}

// ---------------------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------------------

/// A moment in UTC, ordered by time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Date {
    days: i64,
    secs: u32,
    micros: u32,
}

/// Days from 1970-01-01 to `y-m-d` in the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Date {
    /// `utils/date.py` `is_date_undefined`: before 101-01-02 (`UNDEFINED_DATE` is 101-01-01).
    fn is_undefined(&self) -> bool {
        let (y, m, d) = civil_from_days(self.days);
        y < 101 || (y == 101 && m == 1 && d == 1)
    }

    /// Python's `datetime.isoformat()` of the date in UTC, as `utils/date.py` `isoformat`
    /// writes it.
    pub(crate) fn isoformat(&self) -> String {
        let (y, m, d) = civil_from_days(self.days);
        let (h, mi, s) = (self.secs / 3600, self.secs / 60 % 60, self.secs % 60);
        let frac = if self.micros == 0 {
            String::new()
        } else {
            format!(".{:06}", self.micros)
        };
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}{frac}+00:00")
    }
}

/// Reads up to `max` ASCII digits from the front of `s`, at least `min`.
fn digits(s: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    (n >= min).then(|| s[..n].parse().ok().map(|v| (v, &s[n..])))?
}

/// A `dc:date` as Calibre reads it, in UTC (the dumps ran with `TZ=UTC`, so a time with no
/// zone is UTC either way). `utils/date.py` `parse_date` fills a missing day with the 15th and
/// a missing month with the month of the run; the port fills January (DIVERGENCES.md, "The
/// month of a year-only date"). With `only_date_fix` (`opf3.py` `parse_date`), a date with no
/// time is moved off the first and last day of its month (`fix_only_date`). Only ISO 8601
/// forms are read: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, then `[T ]HH:MM[:SS[.f]]` and `Z` or
/// `±HH[:MM]` (DIVERGENCES.md, "Dates in other forms"). An empty string is Calibre's undefined
/// date.
pub(crate) fn parse_date(raw: &str, only_date_fix: bool) -> Option<Date> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(Date {
            days: days_from_civil(101, 1, 1),
            secs: 0,
            micros: 0,
        });
    }
    let (y, rest) = digits(raw, 4, 4)?;
    let (mut m, mut d, mut rest) = (1, 15, rest);
    if let Some(r) = rest.strip_prefix('-') {
        let (mm, r) = digits(r, 1, 2)?;
        m = mm;
        rest = r;
        if let Some(r) = rest.strip_prefix('-') {
            let (dd, r) = digits(r, 1, 2)?;
            d = dd;
            rest = r;
        }
    }
    let y = i64::from(y);
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    let (mut secs, mut micros, mut offset) = (0i64, 0u32, 0i64);
    if let Some(r) = rest.strip_prefix(['T', ' ']) {
        let (h, r) = digits(r, 1, 2)?;
        let (mi, r) = digits(r.strip_prefix(':')?, 2, 2)?;
        let (mut s, mut r) = (0, r);
        if let Some(r2) = r.strip_prefix(':') {
            let (ss, r2) = digits(r2, 2, 2)?;
            s = ss;
            r = r2;
            if let Some(r3) = r.strip_prefix(['.', ',']) {
                let n = r3.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 {
                    return None;
                }
                let frac = format!("{:0<6}", &r3[..n.min(6)]);
                micros = frac.parse().ok()?;
                r = &r3[n..];
            }
        }
        if h > 23 || mi > 59 || s > 59 {
            return None;
        }
        secs = i64::from(h * 3600 + mi * 60 + s);
        rest = r;
        if let Some(r) = rest.strip_prefix(['Z', 'z']) {
            rest = r;
        } else if let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') {
            let (oh, r) = digits(&rest[1..], 2, 2)?;
            let r = r.strip_prefix(':').unwrap_or(r);
            let (om, r) = match digits(r, 2, 2) {
                Some(x) => x,
                None => (0, r),
            };
            let o = i64::from(oh * 3600 + om * 60);
            offset = if sign == '-' { -o } else { o };
            rest = r;
        }
    }
    if !rest.is_empty() {
        return None;
    }
    let mut days = days_from_civil(y, m, d);
    if only_date_fix && !raw.contains(' ') && !raw.contains('T') && secs == 0 && micros == 0 {
        // `utils/date.py` `fix_only_date`: `n.month > val.month` is false for 31 December.
        if d == days_in_month(y, m) && m < 12 {
            days -= 1;
        }
        if civil_from_days(days).2 == 1 {
            days += 1;
        }
    }
    let total = days * 86_400 + secs - offset;
    Some(Date {
        days: total.div_euclid(86_400),
        secs: total.rem_euclid(86_400) as u32,
        micros,
    })
}

// ---------------------------------------------------------------------------------------
// Languages
// ---------------------------------------------------------------------------------------

/// ISO 639-1 to ISO 639-3, Calibre 9.18's `iso639['2to3']` (from
/// `resources/localization/iso639.calibre_msgpack`, built from the iso-codes package): all
/// 184 two-letter codes. Sorted for binary search.
#[rustfmt::skip]
const ISO639_2TO3: [(&str, &str); 184] = [
    ("aa", "aar"), ("ab", "abk"), ("ae", "ave"), ("af", "afr"), ("ak", "aka"), ("am", "amh"),
    ("an", "arg"), ("ar", "ara"), ("as", "asm"), ("av", "ava"), ("ay", "aym"), ("az", "aze"),
    ("ba", "bak"), ("be", "bel"), ("bg", "bul"), ("bi", "bis"), ("bm", "bam"), ("bn", "ben"),
    ("bo", "bod"), ("br", "bre"), ("bs", "bos"), ("ca", "cat"), ("ce", "che"), ("ch", "cha"),
    ("co", "cos"), ("cr", "cre"), ("cs", "ces"), ("cu", "chu"), ("cv", "chv"), ("cy", "cym"),
    ("da", "dan"), ("de", "deu"), ("dv", "div"), ("dz", "dzo"), ("ee", "ewe"), ("el", "ell"),
    ("en", "eng"), ("eo", "epo"), ("es", "spa"), ("et", "est"), ("eu", "eus"), ("fa", "fas"),
    ("ff", "ful"), ("fi", "fin"), ("fj", "fij"), ("fo", "fao"), ("fr", "fra"), ("fy", "fry"),
    ("ga", "gle"), ("gd", "gla"), ("gl", "glg"), ("gn", "grn"), ("gu", "guj"), ("gv", "glv"),
    ("ha", "hau"), ("he", "heb"), ("hi", "hin"), ("ho", "hmo"), ("hr", "hrv"), ("ht", "hat"),
    ("hu", "hun"), ("hy", "hye"), ("hz", "her"), ("ia", "ina"), ("id", "ind"), ("ie", "ile"),
    ("ig", "ibo"), ("ii", "iii"), ("ik", "ipk"), ("io", "ido"), ("is", "isl"), ("it", "ita"),
    ("iu", "iku"), ("ja", "jpn"), ("jv", "jav"), ("ka", "kat"), ("kg", "kon"), ("ki", "kik"),
    ("kj", "kua"), ("kk", "kaz"), ("kl", "kal"), ("km", "khm"), ("kn", "kan"), ("ko", "kor"),
    ("kr", "kau"), ("ks", "kas"), ("ku", "kur"), ("kv", "kom"), ("kw", "cor"), ("ky", "kir"),
    ("la", "lat"), ("lb", "ltz"), ("lg", "lug"), ("li", "lim"), ("ln", "lin"), ("lo", "lao"),
    ("lt", "lit"), ("lu", "lub"), ("lv", "lav"), ("mg", "mlg"), ("mh", "mah"), ("mi", "mri"),
    ("mk", "mkd"), ("ml", "mal"), ("mn", "mon"), ("mr", "mar"), ("ms", "msa"), ("mt", "mlt"),
    ("my", "mya"), ("na", "nau"), ("nb", "nob"), ("nd", "nde"), ("ne", "nep"), ("ng", "ndo"),
    ("nl", "nld"), ("nn", "nno"), ("no", "nor"), ("nr", "nbl"), ("nv", "nav"), ("ny", "nya"),
    ("oc", "oci"), ("oj", "oji"), ("om", "orm"), ("or", "ori"), ("os", "oss"), ("pa", "pan"),
    ("pi", "pli"), ("pl", "pol"), ("ps", "pus"), ("pt", "por"), ("qu", "que"), ("rm", "roh"),
    ("rn", "run"), ("ro", "ron"), ("ru", "rus"), ("rw", "kin"), ("sa", "san"), ("sc", "srd"),
    ("sd", "snd"), ("se", "sme"), ("sg", "sag"), ("sh", "hbs"), ("si", "sin"), ("sk", "slk"),
    ("sl", "slv"), ("sm", "smo"), ("sn", "sna"), ("so", "som"), ("sq", "sqi"), ("sr", "srp"),
    ("ss", "ssw"), ("st", "sot"), ("su", "sun"), ("sv", "swe"), ("sw", "swa"), ("ta", "tam"),
    ("te", "tel"), ("tg", "tgk"), ("th", "tha"), ("ti", "tir"), ("tk", "tuk"), ("tl", "tgl"),
    ("tn", "tsn"), ("to", "ton"), ("tr", "tur"), ("ts", "tso"), ("tt", "tat"), ("tw", "twi"),
    ("ty", "tah"), ("ug", "uig"), ("uk", "ukr"), ("ur", "urd"), ("uz", "uzb"), ("ve", "ven"),
    ("vi", "vie"), ("vo", "vol"), ("wa", "wln"), ("wo", "wol"), ("xh", "xho"), ("yi", "yid"),
    ("yo", "yor"), ("za", "zha"), ("zh", "zho"), ("zu", "zul"),
];

/// `utils/localization.py` `canonicalize_lang`: the ISO 639-3 code of a language tag. A
/// two-letter code goes through [`ISO639_2TO3`]; a three-letter code is kept. Calibre keeps a
/// three-letter code only when ISO 639-3 has it, and also reads English language names
/// (`english`); the port does neither (DIVERGENCES.md, "Language codes").
pub(crate) fn canonicalize_lang(raw: &str) -> Option<String> {
    let raw = raw.trim().to_lowercase().replace('_', "-");
    let raw = raw.split('-').next().unwrap_or("");
    let raw = raw.split('@').next().unwrap_or("").trim();
    match raw.len() {
        2 => ISO639_2TO3
            .binary_search_by_key(&raw, |(two, _)| two)
            .ok()
            .map(|i| ISO639_2TO3[i].1.to_owned()),
        3 if raw.bytes().all(|b| b.is_ascii_lowercase()) => Some(raw.to_owned()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Hrefs (`ebooks/oeb/base.py`, Python's `posixpath` and `urllib.parse`)
// ---------------------------------------------------------------------------------------

/// `urllib.parse.unquote`.
pub(crate) fn unquote(s: &str) -> String {
    percent_decode(s)
}

/// `base.py` `urlquote`: ASCII outside `A-Za-z0-9_.-/~` as `%xx`, other characters kept.
pub(crate) fn urlquote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() && !(c.is_ascii_alphanumeric() || "_.-/~".contains(c)) {
            out.push_str(&format!("%{:02x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// The scheme of `href` as `urllib.parse.urlsplit` finds it, lower-cased.
pub(crate) fn scheme(href: &str) -> Option<String> {
    let i = href.find(':')?;
    let s = &href[..i];
    let first = s.chars().next()?;
    (first.is_ascii_alphabetic()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
    .then(|| s.to_ascii_lowercase())
}

/// `urllib.parse.urldefrag` for a relative href: the parts before and after the first `#`.
pub(crate) fn urldefrag(href: &str) -> (&str, &str) {
    href.split_once('#').unwrap_or((href, ""))
}

/// `base.py` `urlnormalize`: every part unquoted, then quoted again with [`urlquote`]. An href
/// with a scheme other than `file` is returned as it is; it never names a file in the book.
pub(crate) fn urlnormalize(href: &str) -> String {
    if scheme(href).is_some_and(|s| s != "file") {
        return href.to_owned();
    }
    let (path, frag) = urldefrag(href);
    let fix = |p: &str| urlquote(&unquote(&p.replace('\\', "/")));
    let mut out = fix(path);
    let frag = fix(frag);
    if !frag.is_empty() {
        out.push('#');
        out.push_str(&frag);
    }
    out
}

/// `posixpath.normpath`.
pub(crate) fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let slashes = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else if path.starts_with('/') {
        1
    } else {
        0
    };
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (slashes == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let out = format!("{}{}", "/".repeat(slashes), comps.join("/"));
    if out.is_empty() { ".".to_owned() } else { out }
}

/// `posixpath.dirname`.
pub(crate) fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => {
            let head = &path[..=i];
            let trimmed = head.trim_end_matches('/');
            if trimmed.is_empty() { head } else { trimmed }
        }
        None => "",
    }
}

/// `posixpath.join` of two parts.
pub(crate) fn posix_join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        b.to_owned()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `posixpath.splitext`.
pub(crate) fn splitext(path: &str) -> (&str, &str) {
    let base_start = path.rfind('/').map_or(0, |i| i + 1);
    let base = &path[base_start..];
    let lead = base.len() - base.trim_start_matches('.').len();
    match base[lead..].rfind('.') {
        Some(i) => path.split_at(base_start + lead + i),
        None => (path, ""),
    }
}

/// `base.py` `Manifest.Item.abshref`: `href` (already normalized) made book-absolute from the
/// item at `base`.
pub(crate) fn abshref(base: &str, href: &str) -> String {
    if scheme(href).is_some_and(|s| s != "file") {
        return href.to_owned();
    }
    let (path, frag) = urldefrag(href);
    if path.is_empty() {
        return if frag.is_empty() {
            base.to_owned()
        } else {
            format!("{base}#{frag}")
        };
    }
    if !base.contains('/') {
        return href.to_owned();
    }
    normpath(&posix_join(dirname(base), href))
}

// ---------------------------------------------------------------------------------------
// read_package
// ---------------------------------------------------------------------------------------

/// `epub_input.py` and `base.py` `Spine._linear`: `no` and `false` in any case are not linear.
pub(crate) fn is_linear(linear: Option<&str>) -> bool {
    !linear.is_some_and(|l| {
        let l = l.to_lowercase();
        l == "no" || l == "false" || l.is_empty()
    })
}

/// Reads the package files of `zip`: the OPF, the EPUB 3 nav and the NCX.
pub fn read_package(zip: &mut EpubReader) -> Result<Package, Error> {
    let opf_path = zip.opf_path.clone();
    let opf_text = zip.read_string(&opf_path)?;
    let opf = Opf::parse(&opf_text, &opf_path)?;
    let names: BTreeSet<String> = zip.names().into_iter().collect();
    let opf_dir = zip.opf_dir.clone();
    let path_of = |href: &str| join(&opf_dir, &unquote(href));

    let manifest: Vec<Item> = opf
        .manifest
        .iter()
        .filter_map(|i| {
            Some(Item {
                id: i.id.clone().unwrap_or_default(),
                href: i.href.clone()?,
                media_type: i.media_type.clone().unwrap_or_default(),
                properties: i.properties.split_whitespace().map(str::to_owned).collect(),
            })
        })
        .collect();
    let by_id = |id: &str| manifest.iter().find(|i| i.id == id);
    let spine = opf
        .spine
        .iter()
        .filter_map(|r| {
            let item = by_id(r.idref.as_deref()?)?;
            Some(SpineRef {
                href: item.href.clone(),
                linear: is_linear(r.linear.as_deref()),
            })
        })
        .collect();
    let guide = opf
        .guide
        .iter()
        .filter_map(|g| {
            Some(GuideRef {
                kind: g.kind.clone()?,
                href: g.href.clone()?,
            })
        })
        .collect();
    // `opf2.py` `OPF.epub3_nav`.
    let nav = (opf.version >= 3.0)
        .then(|| {
            manifest.iter().find(|i| {
                i.properties.iter().any(|p| p.to_lowercase() == "nav")
                    && i.media_type.to_lowercase().contains("html")
                    && !i.id.is_empty()
                    && names.contains(&path_of(&i.href))
            })
        })
        .flatten()
        .map(|i| path_of(&i.href));
    // `reader.py` `_find_ncx`: the spine's `toc` item, else the first item of the NCX type.
    let ncx = match &opf.spine_toc {
        Some(id) => by_id(id),
        None => manifest
            .iter()
            .find(|i| i.media_type.to_lowercase() == NCX_MIME),
    }
    .map(|i| path_of(&i.href))
    .filter(|p| names.contains(p));
    let nav_text = nav.map(|p| zip.read_string(&p)).transpose()?;
    let ncx_text = ncx.map(|p| zip.read_string(&p)).transpose()?;
    Ok(Package {
        opf_path,
        opf_text,
        nav_text,
        ncx_text,
        manifest,
        spine,
        guide,
        metadata: opf.raw,
        page_progression_direction: opf.ppds.first().cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso(raw: &str, fix: bool) -> Option<String> {
        parse_date(raw, fix).map(|d| d.isoformat())
    }

    #[test]
    fn dates_fill_and_fix_as_calibre_does() {
        assert_eq!(
            iso("2021-03-04", true).unwrap(),
            "2021-03-04T00:00:00+00:00"
        );
        assert_eq!(iso("2026-03", true).unwrap(), "2026-03-15T00:00:00+00:00");
        assert_eq!(iso("2019", true).unwrap(), "2019-01-15T00:00:00+00:00");
        // `fix_only_date`: off the first and last day, but not 31 December; EPUB 3 only.
        assert_eq!(
            iso("2021-03-01", true).unwrap(),
            "2021-03-02T00:00:00+00:00"
        );
        assert_eq!(
            iso("2021-03-01", false).unwrap(),
            "2021-03-01T00:00:00+00:00"
        );
        assert_eq!(
            iso("2021-02-28", true).unwrap(),
            "2021-02-27T00:00:00+00:00"
        );
        assert_eq!(
            iso("2021-12-31", true).unwrap(),
            "2021-12-31T00:00:00+00:00"
        );
        assert_eq!(
            iso("2021-03-01T00:00:00Z", true).unwrap(),
            "2021-03-01T00:00:00+00:00"
        );
        assert_eq!(
            iso("2021-03-01T01:30:00.5+02:00", true).unwrap(),
            "2021-02-28T23:30:00.500000+00:00"
        );
        assert_eq!(iso("March 2021", true), None);
        assert_eq!(iso("2021-02-30", true), None);
    }

    #[test]
    fn end_tags_recover_as_libxml2_does() {
        let r = |s: &str| recover_end_tags(s).unwrap();
        // A stray end tag closes the open element; the root's end then closes the rest.
        assert_eq!(r("<a><b><c/></b></b></a></z>"), "<a><b><c/></b></a>");
        assert_eq!(r("<a><b>x</c></a>"), "<a><b>x</b></a>");
        assert_eq!(
            r("<?xml version=\"1.0\"?><a><b>t"),
            "<?xml version=\"1.0\"?><a><b>t</b></a>"
        );
        assert_eq!(
            r("<!DOCTYPE a [<!ENTITY e \"x>\">]><a k='</v>'><!-- </a> --></a>tail"),
            "<!DOCTYPE a [<!ENTITY e \"x>\">]><a k='</v>'><!-- </a> --></a>"
        );
        assert_eq!(recover_end_tags("<a><!-- open"), None);
        // The OPF and NCX readers fall back to it only where the parser failed.
        let ncx = "<n><m><p/></p></m></n>";
        assert_eq!(
            parse_with(ncx, |d| d.root_element().children().count()),
            Ok(1)
        );
        assert!(matches!(
            parse_with("<a>&nbsp;</a>", |_| ()),
            Err(XmlError::Parse(_))
        ));
    }

    #[test]
    fn languages_as_calibre_writes_them() {
        assert_eq!(canonicalize_lang("en-US").as_deref(), Some("eng"));
        assert_eq!(canonicalize_lang("ja").as_deref(), Some("jpn"));
        assert_eq!(canonicalize_lang("he_IL").as_deref(), Some("heb"));
        assert_eq!(canonicalize_lang("fra").as_deref(), Some("fra"));
        assert_eq!(canonicalize_lang("xx"), None);
        assert_eq!(canonicalize_lang(""), None);
        assert!(ISO639_2TO3.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn hrefs_as_calibre_normalizes_them() {
        assert_eq!(
            urlnormalize("a b/ü%20c.xhtml#x y"),
            "a%20b/ü%20c.xhtml#x%20y"
        );
        assert_eq!(urlnormalize("a?b"), "a%3fb");
        assert_eq!(urlnormalize("../x.html"), "../x.html");
        assert_eq!(normpath("OEBPS/../x/./y.xhtml"), "x/y.xhtml");
        assert_eq!(normpath("../a"), "../a");
        assert_eq!(
            abshref("OEBPS/toc.ncx", "c1.xhtml#sec"),
            "OEBPS/c1.xhtml#sec"
        );
        assert_eq!(abshref("toc.ncx", "c1.xhtml"), "c1.xhtml");
        assert_eq!(abshref("OEBPS/a.xhtml", "#n"), "OEBPS/a.xhtml#n");
        assert_eq!(
            splitext("dir.x/stylesheet.css"),
            ("dir.x/stylesheet", ".css")
        );
        assert_eq!(splitext(".hidden"), (".hidden", ""));
        assert_eq!(
            string_to_authors("A. Writer and B. Writer & C && D"),
            ["A. Writer", "B. Writer", "C & D"]
        );
    }
}
