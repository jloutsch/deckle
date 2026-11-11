// SPDX-License-Identifier: GPL-3.0-or-later
//! A chapter of the model's `chapters` argument, read into what `_build_chapter_content` works
//! from. What kfxgen does with a chapter or block key not ported yet is
//! [`GenerateError::Unsupported`], so a book needing it fails rather than coming out different.

use super::images;
use super::inline::{Anchors, Span, TocLink, read_spans};
use super::tables::{self, Table};
use super::{GenerateError, model_err, py};
use crate::pyvalue::{PyValue, truthy};

use GenerateError::Unsupported;

/// Chapter keys whose value, when true, takes kfxgen down a path not ported yet.
const CHAPTER_UNPORTED: [(&str, &str); 5] = [
    ("sub_entries", "contents entries inside a chapter"),
    ("_is_cover", "a cover chapter in the arguments"),
    ("_omit_title_heading", "a chapter with no heading"),
    ("_omit_from_toc", "a chapter left out of the contents"),
    ("page_bidi", "text direction"),
];

/// Block keys whose value, when true, takes kfxgen down a path not ported yet.
const BLOCK_UNPORTED: [(&str, &str); 13] = [
    ("block_style", "block styles"),
    ("preformatted", "preformatted text"),
    ("in_list", "lists"),
    ("float_wrapped", "floated pictures"),
    ("in_cell_paragraph", "tables"),
    ("table", "tables"),
    ("box", "boxes"),
    ("tight", "tight spacing"),
    ("margin_below", "block margins"),
    ("trailing", "a paragraph cut at an empty line"),
    ("clear", "clearing floats"),
    ("label_texts", "equations"),
    ("writing_mode", "vertical text"),
];

pub(crate) struct Chapter {
    pub title: String,
    pub text: String,
    pub blocks: Option<Vec<Block>>,
    pub toc_level: i64,
    pub font_size: f64,
    /// Its own heading's level: a rebuilt contents page's (#580).
    pub heading_level: Option<i64>,
    /// The keys of blocks the converter replaced (#436).
    pub anchor_keys: Vec<String>,
    pub toc_links: Vec<TocLink>,
    /// Pictures of a contents page the converter replaced, as tokens (#117).
    pub preserved_images: Vec<String>,
    /// kfxgen's cover page (#32): no heading, not in the contents.
    pub is_cover: bool,
}

impl Chapter {
    /// The cover's page (:4897).
    pub(crate) fn cover() -> Self {
        Chapter {
            title: String::new(),
            text: images::cover_token(),
            blocks: None,
            toc_level: 0,
            font_size: 1.0,
            heading_level: None,
            anchor_keys: Vec::new(),
            toc_links: Vec::new(),
            preserved_images: Vec::new(),
            is_cover: true,
        }
    }
}

/// A paragraph, a block of type None or "text", or a table.
#[derive(Clone)]
pub(crate) struct Block {
    pub text: String,
    pub heading: bool,
    pub heading_level: Option<i64>,
    pub space_above: bool,
    pub spans: Vec<Span>,
    pub anchors: Anchors,
    pub table: Option<Box<Table>>,
}

/// The chapters of `generate_full_book`'s `chapters` argument.
pub(crate) fn read_chapters(chapters: &PyValue) -> Result<Vec<Chapter>, GenerateError> {
    let list = chapters
        .as_list()
        .ok_or_else(|| model_err("chapters: a list"))?;
    list.iter()
        .enumerate()
        .map(|(i, ch)| read_chapter(ch, &format!("chapters[{i}]")))
        .collect()
}

fn read_chapter(ch: &PyValue, at: &str) -> Result<Chapter, GenerateError> {
    for (key, feature) in CHAPTER_UNPORTED {
        if truthy(ch.get(key)) {
            return Err(Unsupported(feature));
        }
    }
    // `IonDecimal(str(font_size))`: an int other than 1 would print with no ".0".
    let font_size = match ch.get("font_size") {
        None | Some(PyValue::Int(1)) => 1.0,
        Some(PyValue::Float(f)) => *f,
        Some(_) => return Err(Unsupported("a chapter's own font size")),
    };
    let text_of = |key: &str| match ch.get(key) {
        Some(PyValue::Str(s)) => Ok(s.clone()),
        _ => Err(model_err(format!("{at}.{key}: a string"))),
    };
    let title = text_of("title")?;
    let text = text_of("text")?;
    let blocks = match ch.get("blocks") {
        None | Some(PyValue::None) => None,
        Some(v) => Some(
            v.as_list()
                .ok_or_else(|| model_err(format!("{at}.blocks: a list")))?
                .iter()
                .enumerate()
                .map(|(j, b)| read_block(b, &format!("{at}.blocks[{j}]")))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let toc_level = match ch.get("toc_level") {
        None => 0,
        Some(PyValue::Int(n)) => *n,
        Some(_) => return Err(model_err(format!("{at}.toc_level: an int"))),
    };
    // A level picks one of `_HEADING_LEVEL_UNITS`' six (:1840), as a block's does.
    let heading_level = match ch.get("heading_level") {
        Some(v) if v.is_truthy() => match v.as_int() {
            Some(n @ 1..=6) => Some(n),
            _ => return Err(model_err(format!("{at}.heading_level: 1 to 6"))),
        },
        _ => None,
    };
    let toc_links = match ch.get("toc_links") {
        Some(v) if v.is_truthy() => v
            .as_list()
            .ok_or_else(|| model_err(format!("{at}.toc_links: a list")))?
            .iter()
            .enumerate()
            .map(|(j, link)| TocLink::read(link, &format!("{at}.toc_links[{j}]")))
            .collect::<Result<_, _>>()?,
        _ => Vec::new(),
    };
    let preserved_images = match ch.get("preserved_images") {
        Some(v) if v.is_truthy() => v
            .as_list()
            .and_then(|l| l.iter().map(|t| t.as_str().map(str::to_owned)).collect())
            .ok_or_else(|| model_err(format!("{at}.preserved_images: a list of strings")))?,
        _ => Vec::new(),
    };
    Ok(Chapter {
        title,
        text,
        blocks,
        toc_level,
        font_size,
        heading_level,
        anchor_keys: Anchors::read(ch, at)?.keys,
        toc_links,
        preserved_images,
        is_cover: false,
    })
}

fn read_block(b: &PyValue, at: &str) -> Result<Block, GenerateError> {
    match b.get("type") {
        None | Some(PyValue::None) => {}
        Some(PyValue::Str(t)) if t == "text" => {}
        Some(PyValue::Str(t)) if t == "table" => return tables::read_table_block(b, at),
        Some(PyValue::Str(t)) if t == "rule" => return Err(Unsupported("rules")),
        Some(PyValue::Str(t)) if t == "box" => return Err(Unsupported("boxes")),
        Some(_) => return Err(Unsupported("a block type")),
    }
    for (key, feature) in BLOCK_UNPORTED {
        if truthy(b.get(key)) {
            return Err(Unsupported(feature));
        }
    }
    if b.get("margin_above").is_some() {
        return Err(Unsupported("block margins"));
    }
    let text = match b.get("text") {
        Some(PyValue::Str(s)) => s.clone(),
        _ => return Err(model_err(format!("{at}.text: a string"))),
    };
    // With pictures, `_cut_after_picture_lines` cuts only a paragraph that also has words,
    // which `emit_paragraph` reports.
    if !images::has_picture(&text) && cut_at_empty_line(&text) {
        return Err(Unsupported("a paragraph cut at an empty line"));
    }
    let heading_level = match b.get("heading_level") {
        Some(v) if v.is_truthy() => match v.as_int() {
            Some(n @ 1..=6) => Some(n),
            _ => return Err(model_err(format!("{at}.heading_level: 1 to 6"))),
        },
        _ => None,
    };
    Ok(Block {
        text,
        heading: truthy(b.get("heading")),
        heading_level,
        space_above: truthy(b.get("space_above")),
        spans: read_spans(b.get("spans"), at)?,
        anchors: Anchors::read(b, at)?,
        table: None,
    })
}

/// Whether `_cut_after_picture_lines` (:535) cuts a paragraph with no pictures: at a line with
/// words after an empty one, words before it.
pub(crate) fn cut_at_empty_line(text: &str) -> bool {
    let mut words_before = false;
    let mut previous_empty = false;
    for line in text.split('\n') {
        let empty = py::strip(line).is_empty();
        if !empty && previous_empty && words_before {
            return true;
        }
        words_before |= !empty;
        previous_empty = empty;
    }
    false
}
