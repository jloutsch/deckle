// SPDX-License-Identifier: GPL-3.0-or-later
//! A chapter of the model's `chapters` argument, read into what `_build_chapter_content` works
//! from. What kfxgen does with a chapter or block key not ported yet is
//! [`GenerateError::Unsupported`], so a book needing it fails rather than coming out different.

use super::block_styles::{BlockStyle, number, read_block_style};
use super::boxes::{BoxBlock, RuleStyle, read_box, read_rule};
use super::images;
use super::inline::{Anchors, Span, TocLink, read_spans};
use super::tables::{self, Table};
use super::{GenerateError, model_err, py};
use crate::pyvalue::{PyValue, truthy};

use GenerateError::Unsupported;

/// Chapter keys whose value, when true, takes kfxgen down a path not ported yet.
const CHAPTER_UNPORTED: [(&str, &str); 3] = [
    ("sub_entries", "contents entries inside a chapter"),
    ("_is_cover", "a cover chapter in the arguments"),
    ("page_bidi", "text direction"),
];

/// Block keys whose value, when true, takes kfxgen down a path not ported yet.
const BLOCK_UNPORTED: [(&str, &str); 7] = [
    ("preformatted", "preformatted text"),
    ("float_wrapped", "floated pictures"),
    ("in_cell_paragraph", "tables"),
    ("table", "tables"),
    ("trailing", "a paragraph cut at an empty line"),
    ("clear", "clearing floats"),
    ("label_texts", "equations"),
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
    /// The caller's: no heading of kfxgen's (#33), and not in the contents.
    pub omit_title_heading: bool,
    pub omit_from_toc: bool,
    /// `_chapter_writing_mode` (:942): its first block's that carries one.
    pub writing_mode: Option<String>,
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
            omit_title_heading: false,
            omit_from_toc: true,
            writing_mode: None,
        }
    }
}

/// A paragraph, a block of type None or "text", a table, a rule or a box.
#[derive(Clone)]
pub(crate) struct Block {
    pub text: String,
    pub heading: bool,
    pub heading_level: Option<i64>,
    /// A scene break's space above, in ems (#230).
    pub space_above: f64,
    pub spans: Vec<Span>,
    pub anchors: Anchors,
    pub table: Option<Box<Table>>,
    pub rule: Option<RuleStyle>,
    pub boxed: Option<Box<BoxBlock>>,
    pub style: Option<BlockStyle>,
    /// The book's margins, in ems (#553): `margin_above` None when absent, `Some(None)` for
    /// a None kfxgen reads as it is (`"margin_above" in block`).
    pub margin_above: Option<Option<f64>>,
    pub margin_below: f64,
    /// A list item's line, set with no space above (#701).
    pub tight: bool,
}

impl Block {
    /// `{"text": text, "spans": []}`.
    pub(crate) fn plain(text: String) -> Self {
        Block {
            text,
            heading: false,
            heading_level: None,
            space_above: 0.0,
            spans: Vec::new(),
            anchors: Anchors::default(),
            table: None,
            rule: None,
            boxed: None,
            style: None,
            margin_above: None,
            margin_below: 0.0,
            tight: false,
        }
    }

    /// `_is_text_block`'s and `_label_blocks`' block of type None or "text".
    pub(crate) fn is_paragraph(&self) -> bool {
        self.table.is_none() && self.rule.is_none() && self.boxed.is_none()
    }
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
    let writing_mode = match ch.get("blocks").and_then(PyValue::as_list) {
        Some(blocks) => blocks
            .iter()
            .map(|b| b.get("writing_mode"))
            .find(|m| truthy(*m))
            .flatten()
            .map(|m| {
                m.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| model_err(format!("{at}.blocks: a writing_mode string")))
            })
            .transpose()?,
        None => None,
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
        omit_title_heading: truthy(ch.get("_omit_title_heading")),
        omit_from_toc: truthy(ch.get("_omit_from_toc")),
        writing_mode,
    })
}

/// A number or None, as `block.get(key) or 0.0` reads it.
fn ems(b: &PyValue, key: &str, at: &str) -> Result<Option<f64>, GenerateError> {
    match b.get(key) {
        None | Some(PyValue::None) => Ok(None),
        Some(v) => number(v)
            .map(Some)
            .ok_or_else(|| model_err(format!("{at}.{key}: a number"))),
    }
}

pub(crate) fn read_block(b: &PyValue, at: &str) -> Result<Block, GenerateError> {
    let kind = match b.get("type") {
        None | Some(PyValue::None) => None,
        Some(PyValue::Str(t)) if t == "text" => None,
        Some(PyValue::Str(t)) if t == "table" => return tables::read_table_block(b, at),
        Some(PyValue::Str(t)) if t == "rule" || t == "box" => Some(t.as_str()),
        Some(_) => return Err(Unsupported("a block type")),
    };
    for (key, feature) in BLOCK_UNPORTED {
        if truthy(b.get(key)) {
            return Err(Unsupported(feature));
        }
    }
    let margins = |mut block: Block| -> Result<Block, GenerateError> {
        block.margin_above = match b.get("margin_above") {
            None => None,
            Some(_) => Some(ems(b, "margin_above", at)?),
        };
        block.margin_below = ems(b, "margin_below", at)?.unwrap_or(0.0);
        block.anchors = Anchors::read(b, at)?;
        Ok(block)
    };
    match kind {
        Some("rule") => {
            return margins(Block {
                rule: Some(read_rule(b, at)?),
                ..Block::plain(String::new())
            });
        }
        Some(_) => {
            let boxed = read_box(b, at)?;
            let text = match b.get("text") {
                Some(PyValue::Str(s)) => s.clone(),
                _ => return Err(model_err(format!("{at}.text: a string"))),
            };
            return margins(Block {
                boxed: Some(Box::new(boxed)),
                ..Block::plain(text)
            });
        }
        None => {}
    }
    if truthy(b.get("box")) {
        return Err(Unsupported("boxes"));
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
    // A list item's picture isn't floated (#701), not ported.
    if truthy(b.get("in_list")) && images::has_picture(&text) {
        return Err(Unsupported("a picture in a list item"));
    }
    margins(Block {
        heading: truthy(b.get("heading")),
        heading_level,
        space_above: ems(b, "space_above", at)?.unwrap_or(0.0),
        spans: read_spans(b.get("spans"), at)?,
        style: read_block_style(b.get("block_style"), at)?,
        tight: truthy(b.get("tight")),
        ..Block::plain(text)
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
