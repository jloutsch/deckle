// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's converter (`plugin/kfxgen/converter.py`), from the tree Calibre hands it to
//! `generate_full_book`'s arguments (phase 3).
//!
//! The submodules are private; what the tests in `tests/it/` and later `convert` need is
//! re-exported here, as `css` and `dom` do.

mod args;
mod bidi;
mod block_bidi;
mod block_style;
mod blocks;
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
pub use block_style::{StyleError, compute_block_style};
pub use blocks::{
    BeforeSpacing, Block, BlockId, ExtractOptions, Field, FileBlocks, blocks_before_spacing,
    extract_blocks,
};
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

use crate::{book, css};

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
