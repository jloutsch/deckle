// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's converter (`plugin/kfxgen/converter.py`), from the tree Calibre hands it to
//! `generate_full_book`'s arguments (phase 3).
//!
//! The submodules are private; what the tests in `tests/it/` and later `convert` need is
//! re-exported here, as `css` and `dom` do.

mod args;
mod assemble;
mod bidi;
mod block_bidi;
mod block_style;
mod blocks;
mod chapters;
mod contents_page;
mod font_table;
mod href;
mod images;
mod initial;
mod inline;
// Tasks 11 to 17 call what the tests do not.
#[allow(dead_code)]
mod inline_style;
pub(crate) mod marks;
mod notes;
mod options;
mod package;
mod pictures;
mod quotes;
mod resolver;
mod toc;

/// The page width kfxgen's `convert` sets round the extraction, for the dump tests.
pub use crate::generate::page_scale::{HORIZONTAL_PT, PageWidth, VERTICAL_PT, page_width};
pub use args::book_args;
pub use assemble::assemble;
pub use block_style::{StyleError, compute_block_style};
pub use blocks::{
    BeforeSpacing, Block, BlockId, ExtractOptions, Field, FileBlocks, blocks_before_spacing,
    extract_blocks,
};
pub use chapters::{Chapter, SpineItem, spine_items, split_by_writing_mode};
pub use contents_page::{listing_without_contents, replace_title_page};
pub use font_table::{font_table, parse_style, parse_weight};
pub use href::{
    external_target, is_unsafe, key_doc, normalize, resolve_doc_path, resolve_link_target, unquote,
};
pub use images::{Images, images};
pub use initial::initial;
pub use inline::{Serials, Walk};
pub use inline_style::{
    Flag, Flags, Raised, Segment, Span, css_letter_spacing, css_link_colour, css_rgb,
    css_text_colour, normalize_runs_with_anchors, parse_css_length, parse_vertical_align,
};
pub use notes::{book_link_targets, footnote_keys, note_pair_ids, note_target_ids};
pub use options::ConvertOptions;
pub use package::{CalibreOpf, Nav, NavLink, Ncx};
pub(crate) use package::{item_path, kfxgen_paths};
pub use pictures::css_background_image;
pub use resolver::{Resolved, Resolver, TextBudget};
pub use toc::{TocEntry, respell_toc_hrefs, spine_in_order, toc_entries};

use std::path::Path;

use crate::load::Model;
use crate::pyvalue::PyValue;
use crate::{book, css};

/// What [`convert`] gives: the arguments kfxgen's `convert_oeb_to_kfx` hands
/// `generate_full_book`, and what kfxgen logs as a warning on the way.
#[derive(Debug)]
pub struct Converted {
    pub model: Model,
    pub warnings: Vec<String>,
}

/// kfxgen's `convert_oeb_to_kfx` up to the generator, on the EPUB at `path`, in its order:
/// the metadata, the cover and pictures, the chapters (a second pass printing the contents
/// listings when the first leaves the book no contents page), the split by writing mode, the
/// fonts, then the notes, landmarks and page list. `display_dims` is always empty, as kfxgen
/// passes it.
///
/// Errors: the book cannot be read ([`Error::Book`]), uses what the port refuses
/// ([`Error::Unsupported`]), passes one of its limits ([`Error::TooLarge`]), or shows nothing
/// ([`Error::NoContent`]).
pub fn convert(path: &Path, opts: &ConvertOptions) -> Result<Converted, Error> {
    let book = book::read_book(path).map_err(Error::Book)?;
    let opf = CalibreOpf::new(&book).map_err(Error::Book)?;
    let mut book_level = book_args(&book, &opf, opts);
    let pictures = images(&book, path, opts)?;
    let warnings = pictures.warnings;
    let title = match book_level.iter().find(|(k, _)| k == "title") {
        Some((_, PyValue::Str(t))) => t.clone(),
        _ => unreachable!("book_args gives a title"),
    };
    let chapters = chapters(&book, &opf, opts, pictures.cover_href.as_deref(), &title)?;
    let chapters = split_by_writing_mode(chapters);
    let font_table = font_table(&book, path, opts)?;
    let budgets: Vec<TextBudget> = book.files.iter().map(|_| TextBudget::new()).collect();
    let footnotes = footnote_keys(&book, &budgets)?;

    let bytes = |b: Vec<u8>| PyValue::Bytes(b.into());
    let mut arg = |key: &str| {
        let at = book_level.iter().position(|(k, _)| k == key);
        at.map(|i| book_level.swap_remove(i).1)
            .unwrap_or_else(|| unreachable!("book_args gives {key}"))
    };
    // `generate_full_book`'s parameters in its order, as the model holds them.
    let args = vec![
        ("title", arg("title")),
        ("author", arg("author")),
        (
            "chapters",
            PyValue::List(chapters.iter().map(Chapter::to_py).collect()),
        ),
        ("asin", PyValue::None),
        ("cover_image", pictures.cover.map_or(PyValue::None, bytes)),
        ("language", arg("language")),
        ("rtl", arg("rtl")),
        ("writing_mode", arg("writing_mode")),
        ("publisher", arg("publisher")),
        ("issue_date", arg("issue_date")),
        ("authors", arg("authors")),
        ("description", arg("description")),
        (
            "images",
            PyValue::Dict(
                (pictures.images.into_iter())
                    .map(|(href, b)| (PyValue::Str(href), bytes(b)))
                    .collect(),
            ),
        ),
        ("font_table", font_table),
        ("resolved_image_refs", arg("resolved_image_refs")),
        ("display_dims", PyValue::Dict(Vec::new())),
        (
            "footnote_keys",
            PyValue::Set(footnotes.into_iter().map(PyValue::Str).collect()),
        ),
        ("landmarks", arg("landmarks")),
        ("page_list", arg("page_list")),
        ("approximate_pages", arg("approximate_pages")),
    ];
    let args = args
        .into_iter()
        .map(|(k, v)| (PyValue::Str(k.to_owned()), v))
        .collect();
    Ok(Converted {
        model: Model {
            kfxgen_version: KFXGEN_VERSION.to_owned(),
            kfxgen_commit: KFXGEN_COMMIT.to_owned(),
            args: PyValue::Dict(args),
        },
        warnings,
    })
}

/// The kfxgen release and commit the port follows, as a model names them.
const KFXGEN_VERSION: &str = "5.24.0";
const KFXGEN_COMMIT: &str = "ad38ec8";

/// `extract_chapters_from_oeb` and its rerun (`_with_listings_kept`): the files' blocks
/// assembled into chapters, read again printing the contents listings when dropping them
/// leaves the book with no contents page, then the title page and the contents page made.
/// The serial numbers run on across both passes, as kfxgen's do.
fn chapters(
    book: &book::Book,
    opf: &CalibreOpf,
    opts: &ConvertOptions,
    cover_href: Option<&str>,
    title: &str,
) -> Result<Vec<Chapter>, Error> {
    let mut serials = Serials::default();
    let mut keep_nav_listings = false;
    loop {
        let (items, entries) = spine_items(book, opf, opts, &mut serials, keep_nav_listings)?;
        let mut chapters = assemble(items, &entries, cover_href);
        if listing_without_contents(&chapters, keep_nav_listings) {
            keep_nav_listings = true;
            continue;
        }
        replace_title_page(&mut chapters, title, keep_nav_listings)?;
        return Ok(chapters);
    }
}

/// Why a book cannot be converted.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The book could not be read as kfxgen receives it, or its styles passed a limit while
    /// the converter read them.
    #[error(transparent)]
    Book(book::Error),
    /// The book uses something the converter refuses (plan decision 2): `feature` names it,
    /// `file` the book file it is in.
    #[error("{file}: {feature} cannot be converted")]
    Unsupported { feature: &'static str, file: String },
    /// No spine file gives any content.
    #[error("the book shows nothing")]
    NoContent,
    /// A limit of the converter's own passed; `what` says which, and in which file.
    #[error("{what}")]
    TooLarge { what: String },
}

/// A style read past the file's text budget, or a style value with no default: as
/// `read_book` reports them.
impl From<css::Error> for Error {
    fn from(e: css::Error) -> Error {
        Error::Book(book::Error::from(e))
    }
}

/// What stops one spine file's extraction: something kfxgen raises on, which leaves that
/// file out of the book (kfxgen logs "Spine item N parse failed" and goes on; Task 21), or an
/// [`Error`], which stops the conversion.
#[derive(Debug)]
pub enum ItemError {
    Raised(Raised),
    Error(Error),
}

impl From<Raised> for ItemError {
    fn from(e: Raised) -> Self {
        ItemError::Raised(e)
    }
}

impl From<Error> for ItemError {
    fn from(e: Error) -> Self {
        ItemError::Error(e)
    }
}

impl From<css::Error> for ItemError {
    fn from(e: css::Error) -> Self {
        ItemError::Error(e.into())
    }
}

impl From<StyleError> for ItemError {
    fn from(e: StyleError) -> Self {
        match e {
            StyleError::Raised(r) => ItemError::Raised(r),
            StyleError::TooLarge(e) => e.into(),
        }
    }
}
