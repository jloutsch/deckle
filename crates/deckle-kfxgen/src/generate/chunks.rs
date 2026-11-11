// SPDX-License-Identifier: GPL-3.0-or-later
//! A chapter's entries: [`Chunk`], one per paragraph, picture or contents line, and
//! `_emit_paragraph`'s dispatch.

use super::block_styles::{BlockStyle, HeadingStyle, keep_space};
use super::boxes::{BoxOpen, RuleStyle};
use super::chapters::Block;
use super::images::{Images, Picture};
use super::inline::{self, Anchors, Span};
use super::inline_picture_entries::InlinePicture;
use super::inline_pictures;
use super::styles::LINE_EM;
use super::table_chunks::{CellInfo, TableOpen};
use super::tables::Border;
use super::writing::{Bidi, Tcy};
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
    /// The pictures among a text entry's words, each one U+FFFC of its text (#529).
    pub pictures: Vec<InlinePicture>,
    /// Its 縦中横, each one placeholder of its text (#247).
    pub tcys: Vec<Tcy>,
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
    /// A row group, "head", "body" or "foot", its border and its direction (#605).
    Group(&'static str, Option<Border>, Option<Bidi>),
    Row(Option<Border>, Option<Bidi>),
    /// A cell holding several blocks, or none.
    Cell(CellInfo),
    /// A box (#238), or a frame (#382, #565).
    Box(Box<BoxOpen>),
    /// A picture's container: a linked one's (#355), or a plain one in a horizontal block of
    /// vertical text (#247).
    Link(LinkOpen),
}

impl Node {
    /// `_opens_horizontal` (0bc705d :974): a horizontal-tb box or table, as in vertical text
    /// (P247h, #247).
    pub(crate) fn opens_horizontal(&self) -> bool {
        match self {
            Node::Box(b) => b.spec.horizontal,
            Node::Table(t) => t.horizontal,
            _ => false,
        }
    }
}

/// A picture's container (0bc705d :5900): its link's target, if any; whether it is a plain
/// one in a horizontal block of vertical text, and its block's alignment there (#247).
pub(crate) struct LinkOpen {
    pub target: Option<String>,
    pub plain: bool,
    pub align: Option<String>,
}

/// The chapter being written (0bc705d :5489): where its chunks start, whether it is
/// vertical-rl text, and whether the book is.
#[derive(Clone, Copy)]
pub(crate) struct ChapterNow {
    pub start: usize,
    pub vertical: bool,
    pub vertical_book: bool,
}

impl ChapterNow {
    /// `_in_horizontal_block` (0bc705d :5491): whether a chunk appended now is inside a
    /// horizontal block of a vertical-rl chapter.
    pub(crate) fn in_horizontal_block(&self, chunks: &[Chunk]) -> bool {
        if !self.vertical {
            return false;
        }
        let mut open = Vec::new();
        for c in &chunks[self.start..] {
            match &c.marker {
                Some(Marker::Open(node)) => open.push(node.opens_horizontal()),
                Some(Marker::Close) => {
                    open.pop();
                }
                None => {}
            }
        }
        open.contains(&true)
    }
}

/// An entry among a paragraph's words.
pub(crate) enum AmongWords<'a> {
    Picture(&'a InlinePicture),
    Tcy(&'a Tcy),
}

impl AmongWords<'_> {
    pub(crate) fn at(&self) -> i64 {
        match self {
            AmongWords::Picture(p) => p.at,
            AmongWords::Tcy(t) => t.at,
        }
    }

    pub(crate) fn eid(&self) -> i64 {
        match self {
            AmongWords::Picture(p) => p.eid,
            AmongWords::Tcy(t) => t.eid,
        }
    }

    /// `_item_widths` (:1592): a picture takes a position, 縦中横 one per character.
    pub(crate) fn width(&self) -> i64 {
        match self {
            AmongWords::Picture(_) => 1,
            AmongWords::Tcy(t) => py::len(&t.text) as i64,
        }
    }
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
            pictures: Vec::new(),
            tcys: Vec::new(),
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

    /// The pictures and 縦中横 among its words, in their order (:7879).
    pub(crate) fn among_words(&self) -> Vec<AmongWords<'_>> {
        let mut items: Vec<AmongWords> = self.pictures.iter().map(AmongWords::Picture).collect();
        items.extend(self.tcys.iter().map(AmongWords::Tcy));
        // Stable, as Python's sort: a picture before a 縦中横 at the same place.
        items.sort_by_key(AmongWords::at);
        items
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
/// its style (#490), its size when its words are in runs of one (QA on #720), the space above
/// and below it (#665) and its level (#461).
pub(crate) fn print_book_heading(heading: &mut Chunk, block: &Block, title: &str) {
    heading.spans = inline::title_spans(&block.text, &block.spans, py::len(title));
    heading.heading.book_style = block.style.clone();
    // Words all in runs of one size print at that size, as Kindle Previewer 4.0.3 writes a
    // 0.75em heading holding a 2em span: 1.5em (QA on #720). A heading of no style then has
    // one of only that size.
    let mut sizes = vec![None; py::len(title)];
    for sp in &heading.spans {
        let px = inline::run_font_size(&sp.flags);
        sizes[sp.start as usize..(sp.start + sp.len) as usize].fill(px);
    }
    let mut printed: Vec<Option<f64>> = Vec::new();
    for (px, c) in sizes.into_iter().zip(title.chars()) {
        if !py::is_space(c) && !printed.contains(&px) {
            printed.push(px);
        }
    }
    if let [Some(px)] = printed[..] {
        let style = heading.heading.book_style.get_or_insert_default();
        style.font_size = Some(px);
    }
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

/// `_emit_paragraph` (:5745): a paragraph cut after its picture lines and at an empty line,
/// each part on its own; one opening with floated pictures in a frame of its own (#565);
/// else its entries. Returns the ids it could not place because it emitted nothing: only
/// pictures the book doesn't hold.
pub(crate) fn emit_paragraph(
    block: &Block,
    first_in_chapter: bool,
    now: ChapterNow,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    emit(block, first_in_chapter, false, now, images, chunks)
}

/// `_emit_paragraph` of a block kfxgen marks `float_wrapped`: a frame's, whose floats are
/// written among its words.
pub(crate) fn emit_float_wrapped(
    block: &Block,
    first_in_chapter: bool,
    now: ChapterNow,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    emit(block, first_in_chapter, true, now, images, chunks)
}

fn emit(
    block: &Block,
    first_in_chapter: bool,
    float_wrapped: bool,
    now: ChapterNow,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    if let Some(pieces) = inline_pictures::cut_after_picture_lines(block) {
        let mut unplaced = Vec::new();
        for piece in &pieces {
            unplaced.extend(emit(
                piece,
                first_in_chapter,
                float_wrapped,
                now,
                images,
                chunks,
            )?);
        }
        return Ok(unplaced);
    }
    if !float_wrapped && frames::floats_among_words(block) {
        return frames::emit_floated_paragraph(block, first_in_chapter, now, images, chunks);
    }
    Ok(inline_pictures::emit_entries(
        block,
        first_in_chapter,
        float_wrapped,
        now,
        images,
        chunks,
    ))
}
