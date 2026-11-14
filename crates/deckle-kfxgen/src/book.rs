// SPDX-License-Identifier: GPL-3.0-only
//! An EPUB as Calibre 9.18 hands it to kfxgen, from the file alone (phase 2 plan, Task 11).
//!
//! [`read_book`] runs phase 2's steps in Calibre's order: the package and Calibre's view of it
//! ([`crate::epub`]); each spine file parsed and its chapters marked
//! ([`crate::dom::mark_chapters`]), the contents made from them when the book has none
//! ([`crate::dom::generated_toc`], `structure.py` `DetectStructure`); then, file by file in
//! spine order, `<body>`'s style set ([`css::prepare_body`]), the first cascade over the file's
//! stylesheets ([`css::cascade`]) and the flattener ([`css::flatten`], one [`ClassNamer`] for
//! the book); and last `RemoveFakeMargins` over the whole book ([`css::remove_fake_margins`]).
//! What it gives is what kfxgen's `convert_oeb_to_kfx` receives, checked against dumps taken
//! inside a kfxgen conversion (`tests/it/book.rs`, `tests/it/corpus_cascade.rs`).
//!
//! Hostile input: the archive is read through [`EpubReader`], whose entries are capped one by
//! one and together when it is opened; each file's tree, stylesheets and cascades through the
//! limits of [`crate::dom`] and [`crate::css`], and the styles of the whole book by
//! [`deckle_limits::MAX_BOOK_CASCADE_DECLS`] in the [`ClassNamer`].

use std::cell::RefCell;
use std::path::Path;

use deckle_kfx::epub_zip::{EpubReader, InspectError, join, percent_decode};
use deckle_limits::XmlError;

use crate::css::{
    self, ClassNamer, DEFAULT_PROFILE, Decls, Flat, Origin, Profile, Rule, Sheet, Styles, cascade,
    flatten, media_ok, parse_sheet, prepare_body, remove_fake_margins,
};
use crate::dom::{self, Dom, generated_toc, mark_chapters};
use crate::epub::{self, Oeb, Package, oeb_view, read_package};

/// A book as kfxgen receives it from Calibre.
#[derive(Debug, Clone)]
pub struct Book {
    /// The package files as written.
    pub package: Package,
    /// Calibre's book view. When the book has no contents file, `toc` holds the contents
    /// Calibre makes from its chapters and links.
    pub oeb: Oeb,
    /// The spine files in spine order.
    pub files: Vec<File>,
    /// The `@font-face` rules of each spine file, in spine order, as kfxgen's `Stylizer`
    /// reports them (`font_table.py` `build_font_table` collects them so; it drops the
    /// repeats).
    pub font_face: Vec<Decls>,
    /// The units of [`deckle_limits::MAX_CASCADE_DECLS`] the book's files used together,
    /// counted against [`deckle_limits::MAX_BOOK_CASCADE_DECLS`] ([`ClassNamer::units`]).
    pub style_units: usize,
}

/// A spine file after Calibre's flattener.
#[derive(Debug, Clone)]
pub struct File {
    /// Its href from the book's root, as Calibre's manifest writes it.
    pub href: String,
    /// Its tree as kfxgen's `Stylizer` reads it: chapter marks and contents ids in, styles
    /// moved into classes, `<style>` and stylesheet links gone.
    pub dom: Dom,
    /// The styles kfxgen's `Stylizer` gives its elements.
    pub flat: Flat,
    /// The rules of the first cascade, `html.css`'s first, as Calibre applied them: the only
    /// place the book's own CSS is still seen (`rules/NNN.json` in the dumps).
    pub first_pass_rules: Vec<Rule>,
}

impl File {
    /// The style values of the file's elements, as Calibre's `Style` gives them.
    pub fn styles<'a>(&'a self, profile: &'a Profile) -> Styles<'a> {
        Styles::new(&self.dom, &self.flat, profile)
    }
}

/// Why a book could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(std::io::Error),
    /// Not an EPUB Deckle can open, or a file it names is not in it.
    #[error(transparent)]
    Zip(InspectError),
    /// The OPF is not XML Deckle will read: refused for its depth or entities, or not
    /// well-formed.
    #[error("{file}: {source}")]
    Xml { file: String, source: XmlError },
    /// The OPF is not UTF-8.
    #[error("{file} is not UTF-8")]
    Encoding { file: String },
    /// The package is one Calibre refuses: an OPF in a foreign namespace, DTBook content, or
    /// no usable spine entry.
    #[error("{0}")]
    Package(String),
    /// A limit passed: the archive's size, a file's tree, its stylesheets or its styles, or
    /// the styles of the whole book. `what` says which, and in which file.
    #[error("{what}")]
    TooLarge { what: String },
    #[error(
        "EPUB is DRM-protected (META-INF/encryption.xml); only DRM-free books can be converted"
    )]
    Drm,
    /// A style value Calibre has no default for was asked of an element that has none
    /// ([`Styles::inherited`], [`Styles::computed`]).
    #[error("CSS property {prop} has no value and no default")]
    NoDefault { prop: String },
}

impl From<InspectError> for Error {
    fn from(e: InspectError) -> Error {
        match e {
            InspectError::Drm => Error::Drm,
            InspectError::Io(e) => Error::Io(e),
            InspectError::TooLarge(_) | InspectError::EntryTooLarge(_) => Error::TooLarge {
                what: e.to_string(),
            },
            e => Error::Zip(e),
        }
    }
}

impl From<epub::Error> for Error {
    fn from(e: epub::Error) -> Error {
        match e {
            epub::Error::Zip(e) => e.into(),
            epub::Error::Xml { file, source } => Error::Xml { file, source },
            epub::Error::Package(why) => Error::Package(why),
        }
    }
}

/// A style error with no file named: [`css::Error::NoDefault`] is [`Error::NoDefault`], a
/// limit [`Error::TooLarge`].
impl From<css::Error> for Error {
    fn from(e: css::Error) -> Error {
        match e {
            css::Error::NoDefault { prop } => Error::NoDefault { prop },
            e => Error::TooLarge {
                what: e.to_string(),
            },
        }
    }
}

/// A limit a file's styles passed, the file named.
fn too_large_in(href: &str, e: impl std::fmt::Display) -> Error {
    Error::TooLarge {
        what: format!("{href}: {e}"),
    }
}

/// The book at `path` as kfxgen receives it from Calibre 9.18.
pub fn read_book(path: &Path) -> Result<Book, Error> {
    let mut zip = EpubReader::open(path)?;
    let package = read_package(&mut zip).map_err(|e| match e {
        // `read_package` reads the OPF as UTF-8 text; the nav and NCX it reads as missing.
        epub::Error::Zip(InspectError::Io(io)) if io.kind() == std::io::ErrorKind::InvalidData => {
            Error::Encoding {
                file: zip.opf_path.clone(),
            }
        }
        e => e.into(),
    })?;
    let mut oeb = oeb_view(&package, &mut zip)?;
    let css_items = stylesheets(&package);

    // `parse_html`, then `DetectStructure`: marks in every file, then the contents.
    let mut doms = Vec::with_capacity(oeb.spine.len());
    for s in &oeb.spine {
        let bytes = zip.read_bytes(&percent_decode(&s.href))?;
        let mut dom = dom::parse(&bytes).map_err(|e| too_large_in(&s.href, e))?;
        drop(bytes);
        mark_chapters(&mut dom);
        doms.push(dom);
    }
    if oeb.auto_generated_toc {
        let mut files: Vec<(&str, bool, &mut Dom)> = oeb
            .spine
            .iter()
            .zip(doms.iter_mut())
            .map(|(s, d)| (s.href.as_str(), s.linear, d))
            .collect();
        oeb.toc = generated_toc(&mut files);
    }

    // `CSSFlattener`: each file in spine order, one namer for the book.
    let mut namer = ClassNamer::default();
    let mut files = Vec::with_capacity(doms.len());
    let mut font_face = Vec::new();
    for (s, mut dom) in oeb.spine.iter().zip(doms) {
        let href = &s.href;
        prepare_body(&mut dom);
        let sheets = file_sheets(&mut zip, &css_items, href, &dom)?;
        let mut first =
            cascade(&mut dom, href, sheets, &DEFAULT_PROFILE).map_err(|e| too_large_in(href, e))?;
        // `flatten` takes the first cascade whole and keeps neither.
        let first_pass_rules = std::mem::take(&mut first.rules);
        font_face.append(&mut first.font_face);
        let flat = flatten(&mut dom, href, first, &mut namer, &DEFAULT_PROFILE).map_err(|e| {
            match e {
                // Past the book's limit: no one file is to blame.
                css::Error::BookCascade(_) => Error::from(e),
                e => too_large_in(href, e),
            }
        })?;
        files.push(File {
            href: href.clone(),
            dom,
            flat,
            first_pass_rules,
        });
    }
    let mut docs: Vec<(&Dom, &mut Flat)> =
        files.iter_mut().map(|f| (&f.dom, &mut f.flat)).collect();
    remove_fake_margins(&mut namer, &mut docs);

    Ok(Book {
        package,
        oeb,
        files,
        font_face,
        style_units: namer.units(),
    })
}

/// The bytes of the file `href` (from the book's root, as [`Oeb`]'s manifest writes it) of the
/// book at `path`, read through the archive's limits.
pub fn manifest_bytes(path: &Path, href: &str) -> Result<Vec<u8>, Error> {
    let mut zip = EpubReader::open(path)?;
    Ok(zip.read_bytes(&percent_decode(href))?)
}

/// The book's stylesheets as paths from the EPUB root, the ones a file's links and imports
/// may name: the manifest items Calibre's manifest holds as CSS, their media type lower-cased
/// or guessed from the extension when empty (`reader.py` `_manifest_from_opf`, `base.py`
/// `OEB_STYLES`).
pub fn stylesheets(pkg: &Package) -> Vec<String> {
    let opf_dir = pkg.opf_path.rsplit_once('/').map_or("", |(d, _)| d);
    pkg.manifest
        .iter()
        .filter(|i| epub::is_stylesheet(&i.href, &i.media_type))
        .map(|i| join(opf_dir, &percent_decode(&i.href)))
        .collect()
}

/// The directory part of an href from the EPUB root.
fn dir_of(href: &str) -> &str {
    href.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The local name of element `n`, in lower case.
fn local(dom: &Dom, n: dom::NodeId) -> String {
    let tag = dom.tag(n);
    tag.rsplit(':').next().unwrap_or(tag).to_ascii_lowercase()
}

/// A spine file's stylesheets in Calibre's order (`stylizer.py` `Stylizer.__init__`): each
/// `<style>` and `<link rel=stylesheet>` in document order, with their imports. A linked
/// sheet's `@page` rules are dropped (`base.py` `_parse_css`); its URLs and those in a
/// `<style>` are made book-absolute (`flatcss.py` `CSSFlattener.__call__`, `replaceUrls`).
/// `css_items` are the book's stylesheets ([`stylesheets`]); a link or import to anything
/// else is skipped. A stylesheet is decoded as Calibre decodes it (`OEBBook.decode`).
pub fn file_sheets(
    zip: &mut EpubReader,
    css_items: &[String],
    href: &str,
    dom: &Dom,
) -> Result<Vec<Sheet>, Error> {
    // The first read error, kept while the import callback can only answer "none".
    let failed: RefCell<Option<InspectError>> = RefCell::new(None);
    let mut read_css = |path: &str| -> Option<String> {
        if failed.borrow().is_some() || !css_items.iter().any(|c| c == path) {
            return None;
        }
        match zip.read_bytes(path) {
            Ok(bytes) => Some(dom::decode(&bytes)),
            Err(InspectError::MissingFile(_)) => None,
            Err(e) => {
                *failed.borrow_mut() = Some(e);
                None
            }
        }
    };
    let mut sheets = Vec::new();
    // Calibre's rule index runs on across the file's sheets.
    let mut next = 0;
    for n in dom.descendants() {
        let sheet = match local(dom, n).as_str() {
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
                parse_sheet(&text, Origin::Book, href, next, &mut import)
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
                parse_sheet(&text, Origin::Book, &path, next, &mut import).map(|mut sheet| {
                    sheet.page.clear();
                    sheet
                })
            }
            _ => continue,
        };
        if let Some(e) = failed.take() {
            return Err(e.into());
        }
        let sheet = sheet.map_err(|e| too_large_in(href, e))?;
        next = sheet.next_index;
        sheets.push(sheet);
    }
    Ok(sheets)
}
