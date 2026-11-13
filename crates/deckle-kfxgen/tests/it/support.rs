// SPDX-License-Identifier: GPL-3.0-only
//! Helpers the CSS tests share: the cascade fixture books and each spine file's stylesheets in
//! Calibre's order.

use std::collections::BTreeMap;
use std::path::PathBuf;

use deckle_kfx::epub_zip::{EpubReader, join, percent_decode};
use deckle_kfx::fixtures;
use deckle_kfxgen::css::{Decls, Origin, Sheet, media_ok, parse_sheet};
use deckle_kfxgen::dom::{Dom, NodeId};
use deckle_kfxgen::epub::read_package;

/// Every book under `fixtures/kfxgen/cascade/`.
/// How long a cost test may take. Debug builds run several times slower and share the machine
/// with other test threads; 30 s still fails a quadratic loop (the ones found took 19 to 106 s).
pub const COST_LIMIT: std::time::Duration =
    std::time::Duration::from_secs(if cfg!(debug_assertions) { 30 } else { 5 });

pub const BOOKS: [&str; 13] = [
    "css_quirks",
    "directions",
    "fake_margins",
    "fonts",
    "frames",
    "lists",
    "malformed",
    "navigation",
    "no_toc",
    "notes",
    "package_quirks",
    "rtl",
    "vertical",
];

/// A cascade fixture book: its EPUB and its dump.
pub struct Book {
    pub zip: EpubReader,
    /// The manifest's stylesheets, as paths from the EPUB root.
    pub css_items: Vec<String>,
    /// Spine href → `NNN`, the dump's name for the file (`files.json`).
    pub files: BTreeMap<String, String>,
    /// `fixtures/kfxgen/cascade/<book>`.
    pub dir: PathBuf,
}

impl Book {
    pub fn open(name: &str) -> Book {
        let dir = fixtures::kfxgen(&format!("cascade/{name}"));
        let files =
            serde_json::from_slice(&std::fs::read(dir.join("files.json")).unwrap()).unwrap();
        let mut zip = EpubReader::open(&fixtures::kfxgen(&format!("sources/{name}.epub"))).unwrap();
        let pkg = read_package(&mut zip).unwrap();
        let opf_dir = dir_of(&pkg.opf_path).to_owned();
        let css_items = pkg
            .manifest
            .iter()
            .filter(|i| i.media_type == "text/css")
            .map(|i| join(&opf_dir, &percent_decode(&i.href)))
            .collect();
        Book {
            zip,
            css_items,
            files,
            dir,
        }
    }

    /// A spine file's bytes.
    pub fn read(&mut self, href: &str) -> Vec<u8> {
        self.zip
            .read_bytes(href)
            .or_else(|_| self.zip.read_bytes(&percent_decode(href)))
            .unwrap()
    }

    /// The stylesheets of the spine file `href`, whose tree is `dom` ([`file_sheets`]).
    pub fn sheets(&mut self, href: &str, dom: &Dom) -> Vec<Sheet> {
        file_sheets(&mut self.zip, &self.css_items, href, dom)
    }
}

/// The element's local name in lower case.
pub fn local(dom: &Dom, n: NodeId) -> String {
    let tag = dom.tag(n);
    tag.rsplit(':').next().unwrap_or(tag).to_ascii_lowercase()
}

/// The directory part of an href from the EPUB root.
pub fn dir_of(href: &str) -> &str {
    href.rsplit_once('/').map_or("", |(d, _)| d)
}

/// A spine file's stylesheets in Calibre's order (`stylizer.py` `Stylizer.__init__`): each
/// `<style>` and `<link rel=stylesheet>` in document order, with their imports. A linked
/// sheet's `@page` rules are dropped (`base.py` `_parse_css`); its URLs and those in a
/// `<style>` are made book-absolute (`flatcss.py` `CSSFlattener.__call__`, `replaceUrls`).
pub fn file_sheets(
    zip: &mut EpubReader,
    css_items: &[String],
    href: &str,
    dom: &Dom,
) -> Vec<Sheet> {
    let mut read_css = |path: &str| -> Option<String> {
        if !css_items.iter().any(|c| c == path) {
            return None;
        }
        zip.read_string(path).ok()
    };
    let mut sheets = Vec::new();
    // Calibre's rule index runs on across the file's sheets.
    let mut next = 0;
    for n in dom.descendants() {
        match local(dom, n).as_str() {
            "style" => {
                if dom.attr(n, "type").is_some_and(|t| t != "text/css")
                    || !media_ok(dom.attr(n, "media").unwrap_or(""))
                {
                    continue;
                }
                let mut text = dom.text(n).to_owned();
                for &c in dom.children(n) {
                    for t in [dom.text(c), dom.tail(c)] {
                        if !t.is_empty() {
                            text.push_str("\n\n");
                            text.push_str(t);
                        }
                    }
                }
                if text.is_empty() {
                    continue;
                }
                let base = dir_of(href);
                let mut import = |h: &str| read_css(&join(base, h));
                let sheet = parse_sheet(&text, Origin::Book, href, next, &mut import).unwrap();
                next = sheet.next_index;
                sheets.push(sheet);
            }
            "link" => {
                let Some(link) = dom.attr(n, "href").filter(|h| !h.is_empty()) else {
                    continue;
                };
                if !dom
                    .attr(n, "rel")
                    .unwrap_or("stylesheet")
                    .eq_ignore_ascii_case("stylesheet")
                    || !dom
                        .attr(n, "type")
                        .unwrap_or("text/css")
                        .eq_ignore_ascii_case("text/css")
                    || !media_ok(dom.attr(n, "media").unwrap_or(""))
                {
                    continue;
                }
                let path = join(dir_of(href), &percent_decode(link));
                let Some(text) = read_css(&path) else {
                    continue;
                };
                let base = dir_of(&path).to_owned();
                let mut import = |h: &str| read_css(&join(&base, h));
                let mut sheet = parse_sheet(&text, Origin::Book, &path, next, &mut import).unwrap();
                sheet.page.clear();
                next = sheet.next_index;
                sheets.push(sheet);
            }
            _ => {}
        }
    }
    sheets
}

/// The `@font-face` rules of `sheets`, in order.
pub fn font_faces(sheets: &[Sheet]) -> Vec<Decls> {
    sheets.iter().flat_map(|s| s.font_face.clone()).collect()
}
