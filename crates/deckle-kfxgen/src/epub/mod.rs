// SPDX-License-Identifier: GPL-3.0-only
//! The EPUB as Calibre 9.18 hands it to kfxgen's output plugin (phase 2 plan, Task 3).
//!
//! [`read_package`] reads the package files as written: the OPF, the EPUB 3 nav and the NCX.
//! [`oeb_view`] turns that into Calibre's book view after its conversion pipeline has run, as
//! `tools/ref/kfxgen_hook/book_dump.py` records it in `fixtures/kfxgen/cascade/<book>/oeb.json`:
//! hrefs from the EPUB root, the HTML cover page out of the spine, linked files added to it,
//! the contents from the nav or the NCX, the book's stylesheets replaced by Calibre's two, and
//! files nothing uses trimmed from the manifest.
//!
//! The rules are ported from Calibre (GPL v3, `NOTICE`); each names its source file and
//! function. Where the port differs on purpose, `DIVERGENCES.md` says so.

mod oeb;
pub(crate) mod package;
pub(crate) mod toc;

pub use oeb::oeb_view;
pub(crate) use oeb::{OEB_STYLES, data_is_bytes, is_stylesheet};
pub use package::read_package;

use deckle_kfx::epub_zip::InspectError;
use deckle_limits::XmlError;

/// The package files as written in the EPUB.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// The OPF's path in the archive, from `META-INF/container.xml`.
    pub opf_path: String,
    /// The OPF's text, byte for byte.
    pub opf_text: String,
    /// The EPUB 3 navigation document (`opf2.py` `OPF.epub3_nav`: an EPUB 3 manifest item
    /// with the `nav` property).
    pub nav_text: Option<String>,
    /// The NCX as Calibre's reader finds it in its manifest (`reader.py` `_find_ncx`): the
    /// spine's `toc` item, or else the first item of the NCX type.
    pub ncx_text: Option<String>,
    /// The manifest in OPF order, hrefs as written (relative to the OPF).
    pub manifest: Vec<Item>,
    /// The spine's itemrefs that name a manifest item, hrefs as written.
    pub spine: Vec<SpineRef>,
    /// The `<guide>` references as written.
    pub guide: Vec<GuideRef>,
    /// The metadata as written: each field's elements' text, in OPF order.
    pub metadata: Metadata,
    /// The spine's `page-progression-direction`, as written.
    pub page_progression_direction: Option<String>,
    /// The OPF as parsed, for [`oeb_view`].
    pub(crate) opf: package::Opf,
    /// Where the nav and the NCX are.
    pub(crate) files: oeb::PackageFiles,
}

/// A manifest item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub href: String,
    pub media_type: String,
    /// The OPF `properties` attribute, split on whitespace; empty for an item Calibre made.
    pub properties: Vec<String>,
}

/// A spine entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpineRef {
    pub href: String,
    pub linear: bool,
}

/// A guide reference. `kind` is its `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideRef {
    pub kind: String,
    pub href: String,
}

/// A contents entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocNode {
    pub title: String,
    pub href: String,
    pub children: Vec<TocNode>,
}

/// The metadata fields `oeb.json` records, each a list of strings as Calibre's
/// `OEBBook.metadata` holds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub title: Vec<String>,
    pub creator: Vec<String>,
    pub description: Vec<String>,
    pub language: Vec<String>,
    pub publisher: Vec<String>,
    pub date: Vec<String>,
    /// The manifest id of the cover image.
    pub cover: Vec<String>,
}

/// Calibre's book view, as kfxgen's `convert_oeb_to_kfx` receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Oeb {
    pub spine: Vec<SpineRef>,
    /// Sorted by href, then id, as `oeb.json` lists it.
    pub manifest: Vec<Item>,
    /// Empty when `auto_generated_toc` is set: Calibre then builds the contents from the
    /// documents' headings (`DetectStructure`), which this module does not read.
    pub toc: Vec<TocNode>,
    pub auto_generated_toc: bool,
    /// Sorted by type, then href, as `oeb.json` lists it.
    pub guide: Vec<GuideRef>,
    pub metadata: Metadata,
    pub page_progression_direction: Option<String>,
    /// The OPF Calibre writes for the book, which kfxgen reads.
    pub(crate) rewritten: RewrittenOpf,
    /// The spine's files Calibre leaves out of its spine as no document (an SVG page, a
    /// picture), in spine order; `oeb.json` does not record them.
    pub spine_left_out: Vec<String>,
}

/// The OPF `epub_input.py` `convert` writes at the root of the folder Calibre unpacks the book
/// into (`content.opf`), and the two files beside it kfxgen reads (`converter.py`
/// `_book_files`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RewrittenOpf {
    /// The manifest, hrefs from the root and decoded once (`OPF.unquote_urls`), with the NCX
    /// `convert_epub3_nav` makes from the nav last.
    pub manifest: Vec<package::OpfItem>,
    /// The spine's idrefs, the cover page and repeats taken out.
    pub spine: Vec<String>,
    pub guide: Vec<package::OpfRef>,
    /// The nav points of the NCX `convert_epub3_nav` makes, when it makes one.
    pub nav_ncx: Option<Vec<toc::NavPoint>>,
    /// The text of the file at kfxgen's nav path, when it is in the book and UTF-8.
    pub nav_text: Option<String>,
    /// The text of the file at kfxgen's NCX path, likewise (none for the NCX Calibre makes).
    pub ncx_text: Option<String>,
}

/// Why the package could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Zip(#[from] InspectError),
    /// The OPF is not XML Deckle will read: refused for its depth or entities, or not
    /// well-formed. Other files that are not are read as having no links.
    #[error("{file}: {source}")]
    Xml { file: String, source: XmlError },
    /// The package is one Calibre refuses: an OPF in a foreign namespace, DTBook content, or
    /// no usable spine entry.
    #[error("{0}")]
    Package(String),
    /// A file past a limit on what is read of a book, named in the text, or the book's files
    /// together past theirs.
    #[error("{0}")]
    TooLarge(String),
}

/// [`Error::TooLarge`] when `file`, `len` bytes, is past
/// [`deckle_limits::MAX_CONTENT_FILE_BYTES`]: checked before a document or XML file is parsed.
pub(crate) fn check_size(file: &str, len: usize) -> Result<(), Error> {
    use deckle_limits::MAX_CONTENT_FILE_BYTES;
    if u64::try_from(len).map_or(true, |len| len > MAX_CONTENT_FILE_BYTES) {
        return Err(Error::TooLarge(format!(
            "{file}: this file is larger than {} MiB",
            MAX_CONTENT_FILE_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}
