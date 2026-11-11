// SPDX-License-Identifier: GPL-3.0-or-later
//! A book's text: `_build_chapter_content` (`native_generator.py:5396`) for chapters of plain
//! paragraphs, with the `$145` content, `$157` paragraph styles and `$259` storylines it makes,
//! and the `$260` sections that hold them.
use std::collections::BTreeSet;
use std::sync::LazyLock;

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::{Decimal, Value};
use regex::Regex;

use super::{GenerateError, Generator, measure, model_err, py, st, sym};
use crate::pyvalue::{PyValue, truthy};

use GenerateError::Unsupported;

/// `_img_tokens.IMG_TOKEN_RE`: a picture in the text.
static IMG_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\x00IMG\x01([^\x01]*)\x01([^\x00\x01]*)(?:\x01([^\x00\x01]*))?(?:\x01([^\x00\x01]*))?(?:\x01([^\x00]*))?\x00",
    )
    .unwrap()
});

/// `_ASCII_SPACE` (:1061): a paragraph's edges are trimmed of these only.
const ASCII_SPACE: [char; 5] = [' ', '\t', '\n', '\r', '\x0c'];
/// `_MAX_SPLIT_TITLE_BLOCKS` (:824).
const MAX_SPLIT_TITLE_BLOCKS: usize = 3;
/// `MAX_CONTENT_FRAGMENT_SIZE` (:1585).
const MAX_CONTENT_FRAGMENT_SIZE: usize = 8192;
/// `NativeKFXGenerator`'s position ids (:5259).
const CONTENT_POS_BASE: i64 = 1000;
const CONTENT_POS_STEP: i64 = 2;
const SECTION_POS_BASE: i64 = 10000;
const SECTION_POS_STEP: i64 = 2;
const HEADING_FONT_SIZE: f64 = 1.2;

/// Chapter keys whose value, when true, takes kfxgen down a path not ported yet.
const CHAPTER_UNPORTED: [(&str, &str); 8] = [
    ("toc_links", "a rebuilt contents page"),
    ("sub_entries", "contents entries inside a chapter"),
    ("_is_cover", "the cover"),
    ("_omit_title_heading", "a chapter with no heading"),
    ("_omit_from_toc", "a chapter left out of the contents"),
    ("heading_level", "a rebuilt contents page"),
    ("page_bidi", "text direction"),
    ("preserved_images", "images"),
];

/// Block keys whose value, when true, takes kfxgen down a path not ported yet.
const BLOCK_UNPORTED: [(&str, &str); 14] = [
    ("spans", "inline spans"),
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
    text: String,
    blocks: Option<Vec<Block>>,
    pub toc_level: i64,
}

/// A paragraph: a block of type None or "text".
#[derive(Clone)]
struct Block {
    text: String,
    heading: bool,
    heading_level: Option<i64>,
    space_above: bool,
}

/// One entry of a storyline: here always a paragraph of text.
pub(crate) struct Chunk {
    pub text: String,
    pub heading_level: Option<i64>,
}

/// What `_build_chapter_content` returns.
pub(crate) struct ChapterData {
    pub chunks: Vec<Chunk>,
    pub chunk_positions: Vec<i64>,
    pub chapter_chunk_ranges: Vec<(usize, usize)>,
    pub section_positions: Vec<i64>,
    pub chapter_start_positions: Vec<i64>,
    pub story_names: Vec<String>,
    pub storyline_names: Vec<String>,
    pub content_names: Vec<String>,
    pub extra_style_names: Vec<String>,
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
    match ch.get("font_size") {
        None | Some(PyValue::Int(1)) => {}
        Some(PyValue::Float(f)) if *f == 1.0 => {}
        Some(_) => return Err(Unsupported("a chapter's own font size")),
    }
    let text_of = |key: &str| match ch.get(key) {
        Some(PyValue::Str(s)) => Ok(s.clone()),
        _ => Err(model_err(format!("{at}.{key}: a string"))),
    };
    let title = text_of("title")?;
    let text = text_of("text")?;
    if IMG_TOKEN.is_match(&text) {
        return Err(Unsupported("images"));
    }
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
    Ok(Chapter {
        title,
        text,
        blocks,
        toc_level,
    })
}

fn read_block(b: &PyValue, at: &str) -> Result<Block, GenerateError> {
    match b.get("type") {
        None | Some(PyValue::None) => {}
        Some(PyValue::Str(t)) if t == "text" => {}
        Some(PyValue::Str(t)) if t == "table" => return Err(Unsupported("tables")),
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
    if IMG_TOKEN.is_match(&text) {
        return Err(Unsupported("images"));
    }
    if cut_at_empty_line(&text) {
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
    })
}

/// Whether `_cut_after_picture_lines` (:535) cuts a paragraph with no pictures: at a line with
/// words after an empty one, words before it.
fn cut_at_empty_line(text: &str) -> bool {
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

/// `_normalize_title` (:827).
fn normalize_title(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|&c| py::is_alnum(c))
        .collect()
}

/// `_opens_with_title` (:1356).
fn opens_with_title(text: &str, title: &str) -> bool {
    let n = py::len(title);
    if py::take(text, n).to_lowercase() != title.to_lowercase() {
        return false;
    }
    let rest = py::skip(text, n);
    let (Some(last), Some(next)) = (title.chars().last(), rest.chars().next()) else {
        return true;
    };
    if py::is_east_asian_wide(last) || py::is_east_asian_wide(next) {
        return true;
    }
    if py::is_alpha(next) && py::is_alnum(last) {
        return false;
    }
    if py::is_digit(next) && py::is_digit(last) {
        return false;
    }
    let after = rest.chars().nth(1);
    !(py::is_alpha(last) && matches!(next, '\'' | '\u{2019}') && after.is_some_and(py::is_alpha))
}

/// `_label_blocks` (:1269): how many of the first blocks `title` spells out, or 0. With no
/// rules (`_drop_edge_rules`) and no equations (`label_texts`).
fn label_blocks(blocks: &[Block], title: &str) -> usize {
    let want = py::without_spaces(title);
    let target = normalize_title(title);
    let mut joined = String::new();
    for (i, block) in blocks.iter().take(MAX_SPLIT_TITLE_BLOCKS).enumerate() {
        joined.push_str(&block.text);
        if (!target.is_empty() && normalize_title(&joined) == target)
            || py::without_spaces(&joined) == want
        {
            return i + 1;
        }
    }
    0
}

/// `_title_runs_in` (:1169).
fn title_runs_in(blocks: &[Block], title: &str) -> bool {
    let Some(first) = blocks.first() else {
        return false;
    };
    if first.heading {
        return false;
    }
    let text = py::lstrip(&first.text);
    opens_with_title(text, title) && py::skip(text, py::len(title)).chars().any(py::is_alnum)
}

/// `_consume_split_title` (:837).
fn consume_split_title(blocks: &[Block], title: &str) -> usize {
    let target = normalize_title(title);
    if target.is_empty() {
        return 0;
    }
    let mut acc = String::new();
    for (i, block) in blocks.iter().take(MAX_SPLIT_TITLE_BLOCKS).enumerate() {
        acc.push_str(&normalize_title(&block.text));
        if acc == target {
            return i + 1;
        }
        if !target.starts_with(&acc) {
            return 0;
        }
    }
    0
}

/// `_print_book_heading` (:1477): kfxgen's heading takes the place of the book's. With no runs,
/// no style and no margins, only the level carries over.
fn print_book_heading(heading: &mut Chunk, block: &Block) {
    if let Some(level) = block.heading_level {
        heading.heading_level = Some(level);
    }
}

/// `_emit_paragraph` (:5745) for a paragraph of text alone: one chunk.
fn emit_paragraph(
    block: &Block,
    first_in_chapter: bool,
    chunks: &mut Vec<Chunk>,
) -> Result<(), GenerateError> {
    // A chapter's first paragraph loses its space above (:6496).
    if block.space_above && !first_in_chapter {
        return Err(Unsupported("space above a paragraph"));
    }
    let para = block.text.trim_matches(ASCII_SPACE);
    if py::strip(para).is_empty() {
        return Ok(());
    }
    chunks.push(Chunk {
        text: para.to_owned(),
        heading_level: block.heading_level,
    });
    Ok(())
}

/// `build_fragment_157`'s arguments that the paragraph path sets. Two styles are one when
/// their arguments are, as `_allocate_style` keys them (:6950).
#[derive(Clone, PartialEq)]
struct Style157 {
    font_size: f64,
    bold: bool,
    margin_top: Option<f64>,
    is_heading: bool,
}

impl Style157 {
    fn body(font_size: f64) -> Self {
        Style157 {
            font_size,
            bold: false,
            margin_top: None,
            is_heading: false,
        }
    }
}

/// `_allocate_style` (:6941): one `$157` per kind and arguments, named `s<n><kind>`.
#[derive(Default)]
struct Styles {
    cache: Vec<(&'static str, Style157, String)>,
    counts: Vec<(&'static str, usize)>,
}

impl Styles {
    fn allocate(&mut self, g: &mut Generator, kind: &'static str, style: Style157) -> String {
        if let Some((_, _, name)) = self
            .cache
            .iter()
            .find(|(k, s, _)| *k == kind && *s == style)
        {
            return name.clone();
        }
        let idx = match self.counts.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => {
                *n += 1;
                *n - 1
            }
            None => {
                self.counts.push((kind, 1));
                0
            }
        };
        let name = format!("s{idx}{kind}");
        let fragment = build_fragment_157(g, &name, &style);
        g.push(fragment);
        self.cache.push((kind, style, name.clone()));
        name
    }
}

/// `build_fragment_157` (:3568), for the arguments [`Style157`] holds.
fn build_fragment_157(g: &mut Generator, name: &str, style: &Style157) -> Fragment {
    g.local(name);
    let Value::Struct(mut v) = st([
        ("$48", measure(py::decimal("0.5"), "$314")),
        ("$34", sym("$321")),
        ("$36", measure(py::decimal("0"), "$314")),
        // IonDecimal(str(line_height)), line_height 1.0
        ("$42", measure(py::decimal("1.0"), "$310")),
        ("$173", sym(name)),
        ("$13", sym(if style.bold { "$361" } else { "$350" })),
    ]) else {
        unreachable!()
    };
    if !style.is_heading {
        // f"{1 / line_height:.6g}", line_height 1.0
        v.insert(
            "$47",
            measure(
                Decimal {
                    coefficient: 1,
                    exponent: 0,
                },
                "$310",
            ),
        );
    }
    if style.font_size != 1.0 {
        v.insert(
            "$16",
            measure(py::decimal_of_float(style.font_size), "$505"),
        );
    }
    if let Some(top) = style.margin_top {
        v.insert("$46", measure(py::decimal_of_float(top), "$310"));
    }
    Fragment::new("$157", Some(name), Value::Struct(v))
}

/// `build_fragment_145` (:3545).
fn build_fragment_145(g: &mut Generator, strings: Vec<String>, name: &str) -> Fragment {
    g.local(name);
    let value = st([
        ("name", sym(name)),
        (
            "$146",
            Value::List(strings.into_iter().map(Value::String).collect()),
        ),
    ]);
    Fragment::new("$145", Some(name), value)
}

/// A `$145` fragment's name and strings.
type ContentGroup = (String, Vec<String>);
/// Where a string is: its `$145` fragment and its index there.
type ContentRef = (String, i64);

/// `pack_content_fragments` (:1675): a chapter's strings in `$145` fragments under kfxlib's
/// cap, which charges a fragment the UTF-8 bytes of all its strings but the last.
fn pack_content_fragments(
    strings: &[&str],
    mut index: usize,
) -> (Vec<ContentGroup>, Vec<ContentRef>) {
    let mut name = format!("content_{index}");
    let (mut groups, mut refs) = (Vec::new(), Vec::new());
    let mut current: Vec<String> = Vec::new();
    let mut committed = 0;
    for s in strings {
        if let Some(last) = current.last() {
            let projected = committed + last.len();
            if projected >= MAX_CONTENT_FRAGMENT_SIZE {
                groups.push((name, std::mem::take(&mut current)));
                index += 1;
                name = format!("content_{index}");
                committed = 0;
            } else {
                committed = projected;
            }
        }
        refs.push((name.clone(), current.len() as i64));
        current.push((*s).to_owned());
    }
    groups.push((name, current));
    (groups, refs)
}

/// `build_fragment_259` (:4199) for entries of text: one per chunk, the first marked `$790`.
fn build_fragment_259(
    g: &mut Generator,
    styles: &[String],
    refs: &[ContentRef],
    name: &str,
    positions: &[i64],
) -> Fragment {
    g.local(name);
    for (content, _) in refs {
        g.local(content);
    }
    let entries = styles
        .iter()
        .zip(refs)
        .zip(positions)
        .enumerate()
        .map(|(i, ((style, (content, index)), &position))| {
            let Value::Struct(mut entry) = st([
                ("$155", Value::Int(position)),
                ("$157", sym(style)),
                ("$159", sym("$269")),
                (
                    "$145",
                    st([("name", sym(content)), ("$403", Value::Int(*index))]),
                ),
            ]) else {
                unreachable!()
            };
            if i == 0 {
                entry.insert("$790", Value::Int(1));
            }
            Value::Struct(entry)
        })
        .collect();
    let value = st([("$176", sym(name)), ("$146", Value::List(entries))]);
    Fragment::new("$259", Some(name), value)
}

/// `build_fragment_260` (:4500).
pub(crate) fn build_fragment_260(
    g: &mut Generator,
    storyline: &str,
    section: &str,
    position: i64,
) -> Fragment {
    g.local(section);
    g.local(storyline);
    let entry = st([
        ("$155", Value::Int(position)),
        ("$176", sym(storyline)),
        ("$159", sym("$269")),
    ]);
    let value = st([("$174", sym(section)), ("$141", Value::List(vec![entry]))]);
    Fragment::new("$260", Some(section), value)
}

/// The `$157` names `value`'s `$259` entries apply, as `_refs` (:8102) collects them.
fn style_refs(value: &Value, out: &mut BTreeSet<String>) {
    for entry in value.as_list().into_iter().flatten() {
        let Some(e) = entry.as_struct() else {
            continue;
        };
        if let Some(name) = e.get("$157").and_then(Value::as_sym) {
            out.insert(name.0.clone());
        }
        for span in e.get("$142").and_then(Value::as_list).into_iter().flatten() {
            if let Some(name) = span
                .as_struct()
                .and_then(|s| s.get("$157"))
                .and_then(Value::as_sym)
            {
                out.insert(name.0.clone());
            }
        }
        if let Some(kids) = e.get("$146") {
            style_refs(kids, out);
        }
    }
}

/// `_build_chapter_content` (:5396).
pub(crate) fn build_chapter_content(
    g: &mut Generator,
    chapters: &[Chapter],
) -> Result<ChapterData, GenerateError> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut chapter_chunk_ranges = Vec::new();
    let mut heading_chunks = BTreeSet::new();

    // Anchor keys aren't tracked: only links, landmarks, a page list and contents entries
    // inside a chapter read them, and none is ported yet.
    for chapter in chapters {
        let start_idx = chunks.len();
        let blocks = chapter.blocks.as_deref().unwrap_or_default();
        // `_ruby_heading_title` finds no ruby, so `_book_heading_title` (:1304) decides:
        // `book_heading` is its `title_spans is not None`.
        let mut title = chapter.title.clone();
        let mut book_heading = false;
        let matched = label_blocks(blocks, &title);
        if matched == 1 {
            title = py::strip(&blocks[0].text).to_owned();
            book_heading = true;
        } else if matched == 0
            && let Some(first) = blocks.first().filter(|b| b.heading)
        {
            title = py::strip(&first.text).to_owned();
            book_heading = true;
        }

        // With no pictures, no rules and no tables, the chapter isn't image-only, nor its
        // title after pictures or in a table (:6258–6279).
        let runs_in = title_runs_in(blocks, &title);
        let heading_near_start = !book_heading
            && blocks
                .iter()
                .take(MAX_SPLIT_TITLE_BLOCKS)
                .any(|b| b.heading);
        let label_over_blocks = label_blocks(blocks, &title) > 1;
        let omit_heading =
            py::strip(&title).is_empty() || runs_in || heading_near_start || label_over_blocks;
        let mut heading_chunk = None;
        if !omit_heading {
            heading_chunks.insert(chunks.len());
            heading_chunk = Some(chunks.len());
            chunks.push(Chunk {
                text: title.clone(),
                heading_level: None,
            });
        }

        let cut_title = !runs_in && !heading_near_start && !label_over_blocks;
        let n = py::len(&title);
        let stripped = py::lstrip(&chapter.text);
        let text = if cut_title && opens_with_title(stripped, &title) {
            py::lstrip(py::skip(stripped, n))
        } else {
            &chapter.text
        };
        if !text.is_empty() {
            let para_iter: Vec<Block> = match &chapter.blocks {
                Some(blocks) => {
                    let mut iter_blocks = blocks.clone();
                    if !iter_blocks.is_empty() && cut_title {
                        let first_stripped = py::lstrip(&iter_blocks[0].text);
                        if opens_with_title(first_stripped, &title) {
                            let remainder = py::lstrip(py::skip(first_stripped, n));
                            if !remainder.is_empty() {
                                // `_trim_text` (:909): no spans or offsets to move.
                                iter_blocks[0].text = remainder.to_owned();
                            } else {
                                let first = iter_blocks.remove(0);
                                if let Some(h) = heading_chunk {
                                    print_book_heading(&mut chunks[h], &first);
                                }
                            }
                        } else {
                            // `_eat_split_title` (:1524) for paragraphs. The heading takes the
                            // first block's style, which is none.
                            let eaten = consume_split_title(&iter_blocks, &title);
                            iter_blocks.drain(..eaten);
                        }
                    }
                    iter_blocks
                }
                None => text
                    .split("\n\n")
                    .map(|p| {
                        if cut_at_empty_line(p) {
                            return Err(Unsupported("a paragraph cut at an empty line"));
                        }
                        Ok(Block {
                            text: p.to_owned(),
                            heading: false,
                            heading_level: None,
                            space_above: false,
                        })
                    })
                    .collect::<Result<_, _>>()?,
            };
            for (b_idx, block) in para_iter.iter().enumerate() {
                emit_paragraph(block, b_idx == 0, &mut chunks)?;
            }
        } else if let (Some(h), Some(first)) = (heading_chunk, blocks.first()) {
            // Only its title: the book's heading prints as kfxgen's (:6522).
            if py::strip(&first.text) == title {
                print_book_heading(&mut chunks[h], first);
            }
        }

        // Every chapter has an entry to land on (:6597).
        if chunks.len() == start_idx {
            chunks.push(Chunk {
                text: " ".to_owned(),
                heading_level: None,
            });
        }
        chapter_chunk_ranges.push((start_idx, chunks.len()));
    }

    // Each chapter: an outer position, then one per chunk (:6638).
    let mut chunk_positions = Vec::with_capacity(chunks.len());
    let mut content_pos_id = CONTENT_POS_BASE;
    for &(start, end) in &chapter_chunk_ranges {
        content_pos_id += CONTENT_POS_STEP;
        for _ in start..end {
            chunk_positions.push(content_pos_id);
            content_pos_id += CONTENT_POS_STEP;
        }
    }
    // `_section_base` (:5267): above the content's positions.
    let content_max = content_pos_id - CONTENT_POS_STEP;
    let section_base = SECTION_POS_BASE.max(content_max + SECTION_POS_STEP);
    let section_positions = (0..chapters.len() as i64)
        .map(|i| section_base + i * SECTION_POS_STEP)
        .collect();
    let chapter_start_positions = chapter_chunk_ranges
        .iter()
        .map(|&(start, _)| chunk_positions[start])
        .collect();

    // $145: each chapter's text in fragments under the cap (:6863).
    let mut content_names = Vec::new();
    let mut content_refs_per_chapter = Vec::new();
    let mut next_content_index = 1;
    for &(start, end) in &chapter_chunk_ranges {
        let strings: Vec<&str> = chunks[start..end].iter().map(|c| c.text.as_str()).collect();
        let (groups, refs) = pack_content_fragments(&strings, next_content_index);
        next_content_index += groups.len();
        for (name, strings) in groups {
            let fragment = build_fragment_145(g, strings, &name);
            g.push(fragment);
            content_names.push(name);
        }
        content_refs_per_chapter.push(refs);
    }

    // $157: a body style per chapter and a heading style per chapter with a heading (:6973).
    let mut styles = Styles::default();
    let mut story_names = Vec::new();
    let mut extra_style_names: Vec<String> = Vec::new();
    let mut heading_style_names = Vec::new();
    for (i, &(start, end)) in chapter_chunk_ranges.iter().enumerate() {
        let fs = 1.0;
        story_names.push(styles.allocate(g, "", Style157::body(fs)));
        let mut heading_name = None;
        if heading_chunks.range(start..end).next().is_some() {
            let heading = Style157 {
                font_size: py::round((fs * HEADING_FONT_SIZE).max(1.0), 4),
                bold: true,
                margin_top: (i > 0).then_some(2.0),
                is_heading: true,
            };
            let name = styles.allocate(g, "_h", heading);
            if !extra_style_names.contains(&name) {
                extra_style_names.push(name.clone());
            }
            heading_name = Some(name);
        }
        heading_style_names.push(heading_name);
    }

    // $259: a storyline per chapter (:7359).
    let mut storyline_names = Vec::new();
    for (ch_idx, &(start, end)) in chapter_chunk_ranges.iter().enumerate() {
        let sl_name = format!("l{ch_idx}");
        let mut entry_styles = Vec::new();
        for chunk_idx in start..end {
            let style = match &heading_style_names[ch_idx] {
                Some(name) if heading_chunks.contains(&chunk_idx) => name.clone(),
                // `_block_font_size` with no CSS size: 1.
                _ => styles.allocate(g, "", Style157::body(1.0)),
            };
            entry_styles.push(style);
        }
        let fragment = build_fragment_259(
            g,
            &entry_styles,
            &content_refs_per_chapter[ch_idx],
            &sl_name,
            &chunk_positions[start..end],
        );
        g.push(fragment);
        storyline_names.push(sl_name);
    }

    // Every allocated style is registered, and one no storyline uses is dropped (:8088).
    let mut registered: BTreeSet<String> = story_names.iter().cloned().collect();
    registered.extend(extra_style_names.iter().cloned());
    for (_, _, name) in &styles.cache {
        if registered.insert(name.clone()) {
            extra_style_names.push(name.clone());
        }
    }
    let mut referenced = BTreeSet::new();
    for f in g.fragments.iter().filter(|f| f.ftype.as_str() == "$259") {
        if let Some(entries) = f.value.as_struct().and_then(|s| s.get("$146")) {
            style_refs(entries, &mut referenced);
        }
    }
    let unused: BTreeSet<&String> = registered.difference(&referenced).collect();
    if !unused.is_empty() {
        g.fragments.retain(|f| {
            !(f.ftype.as_str() == "$157" && f.fid.as_ref().is_some_and(|n| unused.contains(&n.0)))
        });
        story_names.retain(|n| !unused.contains(n));
        extra_style_names.retain(|n| !unused.contains(n));
    }

    Ok(ChapterData {
        chunks,
        chunk_positions,
        chapter_chunk_ranges,
        section_positions,
        chapter_start_positions,
        story_names,
        storyline_names,
        content_names,
        extra_style_names,
    })
}
