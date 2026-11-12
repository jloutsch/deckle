// SPDX-License-Identifier: GPL-3.0-only
//! Frames: a box written as Kindle Previewer's plain container (`_is_frame`,
//! `native_generator.py:726`), a picture's size in a sized box (`_size_in_box` :224, #382), and
//! a paragraph opening with floated pictures, written in a frame of its own
//! (`_emit_floated_paragraph` :5642, #565). A float later among its words is written in them
//! (`inline_pictures`).
use std::collections::BTreeMap;

use regex::Captures;

use super::boxes::{BoxOpen, BoxSpec};
use super::chapters::Block;
use super::chunks::{
    ASCII_SPACE, ChapterNow, Chunk, Marker, Node, emit_float_wrapped, space_to_write,
};
use super::images::{IMG_TOKEN, Images, SIZE_HINT, picture_link};
use super::inline::{Anchors, Span};
use super::page_scale::column_px;
use super::styles::LINE_EM;
use super::{GenerateError, py};

use GenerateError::Unsupported;

/// Python's truth of a number from the model.
fn nonzero(n: Option<f64>) -> Option<f64> {
    n.filter(|&n| n != 0.0)
}

/// `_is_frame` (:726): a floated paragraph's frame, a horizontal block in vertical text with
/// no border or fill (#247), or a wrapper's width round a picture with no border, fill or
/// float (#382).
pub(crate) fn is_frame(b: &BoxSpec) -> bool {
    b.float_wrapper
        || b.horizontal && !(b.border.is_some() || b.background.is_some())
        || nonzero(b.width).is_some()
            && !(b.border.is_some()
                || b.background.is_some()
                || b.float.as_deref().is_some_and(|s| !s.is_empty()))
}

/// `_emit_box_chunks` (:5978): in a sized box, each picture's size is a share of the box, as
/// Previewer writes it (#382). Set on the chunk, not the token, whose length the runs count.
pub(crate) fn size_pictures_in_box(b: &BoxSpec, chunks: &mut [Chunk]) -> Result<(), GenerateError> {
    let Some(width) = nonzero(b.width) else {
        return Ok(());
    };
    for p in chunks.iter_mut().filter_map(|c| c.picture.as_mut()) {
        p.size = Some(size_in_box(p.size.as_deref(), b.width_px, width)?);
    }
    Ok(())
}

/// `_size_in_box` (:224): none fills the box, a % is already a share of it, and a width in a
/// length becomes one: of a box in a length, picture ÷ box; of a box in %, the picture's share
/// of the column ÷ the box's.
fn size_in_box(
    hint: Option<&str>,
    width_px: Option<f64>,
    width: f64,
) -> Result<String, GenerateError> {
    let Some(hint) = hint else {
        return Ok("w=100%".to_owned());
    };
    let Some(m) = SIZE_HINT.captures(hint) else {
        return Ok(hint.to_owned());
    };
    let unit = m.get(3).map_or("px", |u| u.as_str());
    if &m[1] != "w" || unit == "%" {
        return Ok(hint.to_owned());
    }
    if !m[2].is_ascii() {
        return Err(Unsupported("a picture size in other than ASCII digits"));
    }
    // `_CSS_PX_PER_UNIT` (:60).
    let per_unit = match unit {
        "em" => 16.0,
        "pt" => 4.0 / 3.0,
        _ => 1.0,
    };
    let px = m[2].parse::<f64>().unwrap() * per_unit;
    let pct = match nonzero(width_px) {
        Some(box_px) => px / box_px * 100.0,
        None => px / column_px() * 100.0 / width * 100.0,
    };
    Ok(format!("w={pct:.3}%"))
}

/// A token's float side, and no link covering it: the Voyage drops a floated picture's link
/// (#378), so a linked one stays in the words.
fn floats(text: &str, m: &Captures, spans: &[Span]) -> bool {
    let whole = m.get(0).unwrap();
    m.get(4).is_some_and(|f| !f.as_str().is_empty())
        && picture_link(spans, py::len(&text[..whole.start()]) as i64).is_none()
}

/// `_floats_among_words` (:315): a paragraph holding words and a floated picture. A table
/// cell's, a list item's and preformatted text never get here: with a picture they are
/// Unsupported when read.
pub(crate) fn floats_among_words(block: &Block) -> bool {
    let text = &block.text;
    IMG_TOKEN
        .captures_iter(text)
        .any(|m| floats(text, &m, &block.spans))
        && !py::strip(&IMG_TOKEN.replace_all(text, "")).is_empty()
}

/// `_emit_floated_paragraph` (:5642): a frame carrying the paragraph's space, holding the
/// pictures floated at its start as entries, then the rest as one entry. Returns the ids it
/// could not place.
pub(crate) fn emit_floated_paragraph(
    block: &Block,
    first_in_chapter: bool,
    now: ChapterNow,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    let text = &block.text;
    let mut lead = 0;
    for m in IMG_TOKEN.captures_iter(text) {
        let whole = m.get(0).unwrap();
        if !floats(text, &m, &block.spans)
            || !text[lead..whole.start()]
                .trim_matches(ASCII_SPACE)
                .is_empty()
        {
            break;
        }
        lead = whole.end();
    }
    let space = if block.tight {
        0.0
    } else {
        match block.margin_above {
            Some(margin) => margin.unwrap_or(0.0),
            None => {
                // A chapter's first paragraph has no scene break's space (:6496).
                let scene = if first_in_chapter {
                    0.0
                } else {
                    block.space_above
                };
                // Else kfxgen's own: a line above an unindented paragraph.
                let indented = block.style.as_ref().is_some_and(|b| b.indent.is_some());
                space_to_write(block, scene).unwrap_or(if indented { 0.0 } else { LINE_EM })
            }
        }
    };
    let frame = BoxSpec {
        float_wrapper: true,
        ..BoxSpec::default()
    };
    chunks.push(Chunk::marker(Marker::Open(Node::Box(Box::new(BoxOpen {
        spec: frame,
        space_above: space,
        space_below: block.margin_below,
    })))));
    // Its space is the frame's.
    let inner = Block {
        tight: true,
        margin_above: None,
        margin_below: 0.0,
        space_above: 0.0,
        ..block.clone()
    };
    let mut unplaced = Vec::new();
    let rest = if lead == 0 {
        inner
    } else {
        let lead = py::len(&text[..lead]);
        let mut head = slice_block(&inner, 0, lead);
        head.heading_level = None;
        unplaced.extend(emit_float_wrapped(&head, false, now, images, chunks)?);
        slice_block(&inner, lead, py::len(text))
    };
    unplaced.extend(emit_float_wrapped(&rest, false, now, images, chunks)?);
    // kfxgen drops a frame holding no entry (:5690); the words after the floats always make
    // one here.
    chunks.push(Chunk::marker(Marker::Close));
    Ok(unplaced)
}

/// `_slice_block` (:514): the characters [a, b) of a paragraph, its runs and ids moved with
/// them. An id at `b` belongs to the next part, and one with no offset to neither.
fn slice_block(block: &Block, a: usize, b: usize) -> Block {
    let last = b == py::len(&block.text);
    let (a, b) = (a as i64, b as i64);
    let offsets: BTreeMap<String, i64> = block
        .anchors
        .offsets
        .iter()
        .filter(|&(_, &o)| a <= o && o < b || last && o >= b)
        .map(|(k, &o)| (k.clone(), o - a))
        .collect();
    let spans = block
        .spans
        .iter()
        .filter_map(|s| {
            let (start, end) = (s.start.max(a), (s.start + s.len).min(b));
            (end > start).then(|| Span {
                start: start - a,
                len: end - start,
                flags: s.flags.clone(),
            })
        })
        .collect();
    let keys = block
        .anchors
        .keys
        .iter()
        .filter(|k| offsets.contains_key(*k))
        .cloned()
        .collect();
    Block {
        text: py::take(py::skip(&block.text, a as usize), (b - a) as usize).to_owned(),
        spans,
        anchors: Anchors { keys, offsets },
        ..block.clone()
    }
}
