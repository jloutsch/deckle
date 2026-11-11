// SPDX-License-Identifier: GPL-3.0-or-later
//! A chapter's entries: [`Chunk`], one per paragraph, picture or contents line, and
//! `_emit_paragraph`'s paragraph of text.

use super::block_styles::{BlockStyle, HeadingStyle, keep_space};
use super::boxes::{BoxOpen, RuleStyle};
use super::chapters::Block;
use super::images::{self, Images, Picture};
use super::inline::{self, Anchors, Span};
use super::styles::LINE_EM;
use super::table_chunks::{CellInfo, TableOpen};
use super::tables::Border;
use super::{GenerateError, frames, py};

/// `_ASCII_SPACE` (:1061): a paragraph's edges are trimmed of these only.
pub(crate) const ASCII_SPACE: [char; 5] = [' ', '\t', '\n', '\r', '\x0c'];

/// One entry of a storyline: a paragraph of text, a picture, a rule, or a table's or box's
/// container's start or end.
pub(crate) struct Chunk {
    pub text: String,
    pub heading_level: Option<i64>,
    pub spans: Vec<Span>,
    pub anchors: Anchors,
    pub picture: Option<Picture>,
    pub rule: Option<RuleStyle>,
    pub marker: Option<Marker>,
    /// A text entry in a table cell.
    pub in_cell: Option<InCell>,
    /// Its block's `block_style`; a cell's for a cell's text or container.
    pub style: Option<BlockStyle>,
    /// In ems: the book's margins (#553), a scene break's space (#230).
    pub space_above: Option<f64>,
    pub space_below: Option<f64>,
    /// kfxgen's heading: what prints in it of the book's own (#490).
    pub heading: HeadingStyle,
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
    /// A box (#238), or a frame (#382, #565).
    Box(Box<BoxOpen>),
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
            rule: None,
            marker: None,
            in_cell: None,
            style: None,
            space_above: None,
            space_below: None,
            heading: HeadingStyle::default(),
        }
    }

    pub(crate) fn marker(marker: Marker) -> Self {
        Chunk {
            marker: Some(marker),
            ..Chunk::text(String::new())
        }
    }

    /// A text or picture entry: what a chapter's start lands on (:6597, :6713).
    pub(crate) fn is_leaf(&self) -> bool {
        self.marker.is_none() && self.rule.is_none()
    }

    /// Not a container's marker: what `_emitted_content` (:5940) counts, ids can go on.
    pub(crate) fn is_content(&self) -> bool {
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

/// `_print_book_heading` (:1477): kfxgen's heading takes the place of the book's: its runs,
/// its style (#490), the space above and below it (#665) and its level (#461). With no
/// sized runs.
pub(crate) fn print_book_heading(heading: &mut Chunk, block: &Block, title: &str) {
    heading.spans = inline::title_spans(&block.text, &block.spans, py::len(title));
    heading.heading.book_style = block.style.clone();
    keep_space(
        &mut heading.heading,
        block.style.as_ref(),
        block.margin_above,
        block.margin_below,
    );
    if let Some(level) = block.heading_level {
        heading.heading_level = Some(level);
    }
}

/// `_space_to_write` (:1080): a scene break's space above, or None to keep kfxgen's own when
/// that is as much: a line above an unindented paragraph, none above an indented one.
pub(crate) fn space_to_write(block: &Block, space_above: f64) -> Option<f64> {
    if space_above == 0.0 {
        return None;
    }
    let indented = block.style.as_ref().is_some_and(|b| b.indent.is_some());
    let own = if indented { 0.0 } else { LINE_EM };
    (space_above > own + 0.01).then_some(space_above)
}

/// `_emit_paragraph` (:5745) for a paragraph of text alone, one chunk, of pictures alone, a
/// chunk per picture the book holds, or opening with floated pictures, a frame round them and
/// its words (#565). Returns the ids it could not place because it emitted nothing: only
/// pictures the book doesn't hold.
pub(crate) fn emit_paragraph(
    block: &Block,
    first_in_chapter: bool,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    if frames::floats_among_words(block) {
        return frames::emit_floated_paragraph(block, first_in_chapter, images, chunks);
    }
    emit_float_wrapped(block, first_in_chapter, images, chunks)
}

/// `_emit_paragraph` of a block kfxgen marks `float_wrapped`, or one with no float among words:
/// no frame of its own.
pub(crate) fn emit_float_wrapped(
    block: &Block,
    first_in_chapter: bool,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    let para = block.text.trim_matches(ASCII_SPACE);
    if images::has_picture(para) {
        // A picture has no text for a level, a style or space to go on (:5862).
        return images::emit_picture_paragraph(para, block, images, chunks);
    }
    if py::strip(para).is_empty() {
        return Ok(Vec::new());
    }
    // A chapter's first paragraph opens a page or sits under its heading (#230, :6496).
    let scene = if first_in_chapter {
        0.0
    } else {
        block.space_above
    };
    let space_above = if block.tight {
        Some(0.0)
    } else {
        match block.margin_above {
            // The book's margins, as Previewer writes them (#553).
            Some(margin) => margin,
            None => space_to_write(block, scene),
        }
    };
    // Spans and offsets count from the block's text, not the trimmed paragraph (:5530).
    let (spans, anchors) = inline::place(&block.spans, &block.anchors, py::len(para) as i64);
    chunks.push(Chunk {
        heading_level: block.heading_level,
        spans,
        anchors,
        style: block.style.clone(),
        space_above,
        space_below: (block.margin_below != 0.0).then_some(block.margin_below),
        ..Chunk::text(para.to_owned())
    });
    Ok(Vec::new())
}
