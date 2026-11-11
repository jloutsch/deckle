// SPDX-License-Identifier: GPL-3.0-or-later
//! A chapter's entries: [`Chunk`], one per paragraph, picture or contents line, and
//! `_emit_paragraph`'s paragraph of text.

use super::chapters::Block;
use super::images::{self, Images, Picture};
use super::inline::{self, Anchors, Span};
use super::table_chunks::{CellInfo, TableOpen};
use super::tables::Border;
use super::{GenerateError, py};

use GenerateError::Unsupported;

/// `_ASCII_SPACE` (:1061): a paragraph's edges are trimmed of these only.
const ASCII_SPACE: [char; 5] = [' ', '\t', '\n', '\r', '\x0c'];

/// One entry of a storyline: a paragraph of text, a picture, or a table container's start
/// or end.
pub(crate) struct Chunk {
    pub text: String,
    pub heading_level: Option<i64>,
    pub spans: Vec<Span>,
    pub anchors: Anchors,
    pub picture: Option<Picture>,
    pub marker: Option<Marker>,
    /// A text entry in a table cell.
    pub in_cell: Option<InCell>,
}

/// A table container's (#219) `open`, which takes a position, or `close`, which takes none.
pub(crate) enum Marker {
    Open(Node),
    Close,
}

pub(crate) enum Node {
    Table(TableOpen),
    /// A row group, "head", "body" or "foot", and its border.
    Group(&'static str, Option<Border>),
    Row(Option<Border>),
    /// A cell holding several blocks, or none.
    Cell(CellInfo),
}

pub(crate) enum InCell {
    /// A cell's own text: the chunk's `cell` (:6201).
    Text(CellInfo),
    /// A block in a cell: `in_header_cell` (:6154), bold as a header cell's text is.
    Paragraph { header_bold: bool },
}

impl Chunk {
    pub(crate) fn text(text: String) -> Self {
        Chunk {
            text,
            heading_level: None,
            spans: Vec::new(),
            anchors: Anchors::default(),
            picture: None,
            marker: None,
            in_cell: None,
        }
    }

    pub(crate) fn marker(marker: Marker) -> Self {
        Chunk {
            marker: Some(marker),
            ..Chunk::text(String::new())
        }
    }

    /// A text or picture entry, not a container's marker: what a chapter's start and an id
    /// can land on.
    pub(crate) fn is_leaf(&self) -> bool {
        self.marker.is_none()
    }

    /// Bold on its own, as a header cell is (:7809).
    pub(crate) fn cell_bold(&self) -> bool {
        match &self.in_cell {
            Some(InCell::Text(c)) => c.header && !c.not_bold,
            Some(InCell::Paragraph { header_bold }) => *header_bold,
            None => false,
        }
    }
}

/// `_print_book_heading` (:1477): kfxgen's heading takes the place of the book's. With no
/// style, no sized runs and no margins, its runs and level carry over.
pub(crate) fn print_book_heading(heading: &mut Chunk, block: &Block, title: &str) {
    heading.spans = inline::title_spans(&block.text, &block.spans, py::len(title));
    if let Some(level) = block.heading_level {
        heading.heading_level = Some(level);
    }
}

/// `_emit_paragraph` (:5745) for a paragraph of text alone, one chunk, or of pictures alone,
/// a chunk per picture the book holds. Returns the ids it could not place because it emitted
/// nothing: only pictures the book doesn't hold.
pub(crate) fn emit_paragraph(
    block: &Block,
    first_in_chapter: bool,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    let para = block.text.trim_matches(ASCII_SPACE);
    if images::has_picture(para) {
        return images::emit_picture_paragraph(para, block, images, chunks);
    }
    // A chapter's first paragraph loses its space above (:6496).
    if block.space_above && !first_in_chapter {
        return Err(Unsupported("space above a paragraph"));
    }
    if py::strip(para).is_empty() {
        return Ok(Vec::new());
    }
    // Spans and offsets count from the block's text, not the trimmed paragraph (:5530).
    let (spans, anchors) = inline::place(&block.spans, &block.anchors, py::len(para) as i64);
    chunks.push(Chunk {
        heading_level: block.heading_level,
        spans,
        anchors,
        ..Chunk::text(para.to_owned())
    });
    Ok(Vec::new())
}
