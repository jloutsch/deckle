// SPDX-License-Identifier: GPL-3.0-or-later
//! A book's text: `_build_chapter_content` (`native_generator.py:5396`) for chapters of
//! paragraphs and rebuilt contents pages, with the `$145` content, `$157` paragraph styles and
//! `$259` storylines it makes, and the `$260` sections that hold them. Runs, links and anchors
//! are `inline`'s, pictures `images`'.
use std::collections::{BTreeMap, BTreeSet};

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::{Decimal, Value};

use super::images::{self, ImageStyles, Images, Picture};
use super::inline::{self, Anchors, ChunkLinks, EntryLink, Run, Span, TocLink, read_spans};
use super::{GenerateError, Generator, measure, model_err, py, st, sym};
use crate::pyvalue::{PyValue, truthy};

use GenerateError::Unsupported;

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
    text: String,
    pub blocks: Option<Vec<Block>>,
    pub toc_level: i64,
    font_size: f64,
    /// Its own heading's level: a rebuilt contents page's (#580).
    heading_level: Option<i64>,
    /// The keys of blocks the converter replaced (#436).
    anchor_keys: Vec<String>,
    pub toc_links: Vec<TocLink>,
    /// Pictures of a contents page the converter replaced, as tokens (#117).
    preserved_images: Vec<String>,
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

/// A paragraph: a block of type None or "text".
#[derive(Clone)]
pub(crate) struct Block {
    pub text: String,
    heading: bool,
    heading_level: Option<i64>,
    space_above: bool,
    spans: Vec<Span>,
    pub anchors: Anchors,
}

/// One entry of a storyline: a paragraph of text, or a picture.
pub(crate) struct Chunk {
    pub text: String,
    pub heading_level: Option<i64>,
    spans: Vec<Span>,
    anchors: Anchors,
    pub picture: Option<Picture>,
}

impl Chunk {
    fn text(text: String) -> Self {
        Chunk {
            text,
            heading_level: None,
            spans: Vec::new(),
            anchors: Anchors::default(),
            picture: None,
        }
    }
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
    /// The `$266` anchors, for the entity index.
    pub anchor_names: Vec<String>,
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

/// `_print_book_heading` (:1477): kfxgen's heading takes the place of the book's. With no
/// style, no sized runs and no margins, its runs and level carry over.
fn print_book_heading(heading: &mut Chunk, block: &Block, title: &str) {
    heading.spans = inline::title_spans(&block.text, &block.spans, py::len(title));
    if let Some(level) = block.heading_level {
        heading.heading_level = Some(level);
    }
}

/// `_emit_paragraph` (:5745) for a paragraph of text alone, one chunk, or of pictures alone,
/// a chunk per picture the book holds. Returns the ids it could not place because it emitted
/// nothing: only pictures the book doesn't hold.
fn emit_paragraph(
    block: &Block,
    first_in_chapter: bool,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Result<Vec<String>, GenerateError> {
    let para = block.text.trim_matches(ASCII_SPACE);
    if images::has_picture(para) {
        // Pictures among words are cut at picture lines and written in the words (#529,
        // #565), not ported.
        if !images::only_pictures(para) {
            return Err(Unsupported("pictures among words"));
        }
        let pictures = images::pictures(para, &block.spans, images)?;
        if pictures.is_empty() {
            return Ok(block.anchors.keys.clone());
        }
        // Ids at its start go on its first picture (#62), then the others, all at 0 (:5847);
        // a picture has no text for a heading level or space above to go on.
        let (at_start, rest): (Vec<String>, Vec<String>) = block
            .anchors
            .keys
            .iter()
            .cloned()
            .partition(|k| block.anchors.offsets.get(k).is_none_or(|&o| o == 0));
        let mut anchors = Anchors {
            keys: at_start,
            offsets: BTreeMap::new(),
        };
        if !rest.is_empty() {
            anchors.attach(&rest);
        }
        for picture in pictures {
            chunks.push(Chunk {
                anchors: std::mem::take(&mut anchors),
                picture: Some(picture),
                ..Chunk::text(String::new())
            });
        }
        return Ok(Vec::new());
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
        text: para.to_owned(),
        heading_level: block.heading_level,
        spans,
        anchors,
        picture: None,
    });
    Ok(Vec::new())
}

/// `build_fragment_157`'s arguments that the ported paths set, defaulting as it does. Two
/// styles are one when their arguments are, as `_allocate_style` keys them (:6950).
#[derive(Clone, PartialEq)]
pub(crate) struct Style157 {
    pub font_size: f64,
    pub bold: bool,
    pub italic: bool,
    pub upright: bool,
    pub underline: bool,
    pub strike: bool,
    pub margin_top: Option<f64>,
    /// A contents line's indent in %, as `f"{6.25 * level:g}"` writes it.
    pub margin_left: Option<String>,
    pub baseline_style: Option<&'static str>,
    pub is_heading: bool,
}

impl Default for Style157 {
    fn default() -> Self {
        Style157 {
            font_size: 1.0,
            bold: false,
            italic: false,
            upright: false,
            underline: false,
            strike: false,
            margin_top: None,
            margin_left: None,
            baseline_style: None,
            is_heading: false,
        }
    }
}

impl Style157 {
    fn body(font_size: f64) -> Self {
        Style157 {
            font_size,
            ..Style157::default()
        }
    }
}

/// `_allocate_style` (:6941): one `$157` per kind and arguments, named `s<n><kind>`.
#[derive(Default)]
pub(crate) struct Styles {
    cache: Vec<(&'static str, Style157, String)>,
    counts: Vec<(&'static str, usize)>,
}

impl Styles {
    pub(crate) fn allocate(
        &mut self,
        g: &mut Generator,
        kind: &'static str,
        style: Style157,
    ) -> String {
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
    let margin_left = style.margin_left.as_deref().unwrap_or("0.5");
    let Value::Struct(mut v) = st([
        ("$48", measure(py::decimal(margin_left), "$314")),
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
    if style.underline {
        v.insert("$23", sym("$328"));
    }
    if style.strike {
        v.insert("$27", sym("$328"));
    }
    if style.italic {
        v.insert("$12", sym("$382"));
    } else if style.upright {
        v.insert("$12", sym("$350"));
    }
    if let Some(top) = style.margin_top {
        v.insert("$46", measure(py::decimal_of_float(top), "$310"));
    }
    if let Some(baseline) = style.baseline_style {
        v.insert("$44", sym(baseline));
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

/// `build_fragment_259` (:4199): one entry per chunk, text or picture, the first marked
/// `$790`, a text entry with its link and runs.
fn build_fragment_259(
    g: &mut Generator,
    styles: &[String],
    refs: &[Option<ContentRef>],
    name: &str,
    positions: &[i64],
    runs: &[(Option<EntryLink>, Vec<Run>)],
    chunks: &[Chunk],
) -> Fragment {
    g.local(name);
    for (content, _) in refs.iter().flatten() {
        g.local(content);
    }
    let entries = styles
        .iter()
        .zip(refs)
        .zip(positions)
        .zip(runs)
        .zip(chunks)
        .enumerate()
        .map(|(i, ((((style, r), &position), (link, runs)), chunk))| {
            let mut entry = match &chunk.picture {
                Some(p) => images::picture_entry(g, p, position, style),
                None => {
                    let Value::Struct(mut entry) = st([
                        ("$155", Value::Int(position)),
                        ("$157", sym(style)),
                        ("$159", sym("$269")),
                    ]) else {
                        unreachable!()
                    };
                    if let Some((content, index)) = r {
                        entry.insert(
                            "$145",
                            st([("name", sym(content)), ("$403", Value::Int(*index))]),
                        );
                    }
                    entry
                }
            };
            if i == 0 {
                entry.insert("$790", Value::Int(1));
            }
            if chunk.picture.is_none() {
                inline::write_runs(g, &mut entry, link.as_ref(), runs);
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
    images: &Images,
) -> Result<ChapterData, GenerateError> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut chapter_chunk_ranges = Vec::new();
    let mut heading_chunks = BTreeSet::new();
    // chunk index -> (target chapter, depth in the contents) (:5451).
    let mut toc_link_chunks: BTreeMap<usize, (i64, i64)> = BTreeMap::new();

    for chapter in chapters {
        let start_idx = chunks.len();
        // Ids of blocks this chapter drops, put on its first chunk below (#62, #436).
        let mut carried = chapter.anchor_keys.clone();
        // Offsets in kfxgen's heading for ids of the block it prints (#491).
        let mut heading_offsets = BTreeMap::new();
        let blocks = chapter.blocks.as_deref().unwrap_or_default();
        let toc = !chapter.toc_links.is_empty();
        // `_ruby_heading_title` finds no ruby, so `_book_heading_title` (:1304) decides.
        let mut title = chapter.title.clone();
        let mut title_spans = None;
        let matched = label_blocks(blocks, &title);
        let book_block = if matched == 1 {
            blocks.first()
        } else if matched == 0 {
            // Not a heading holding a picture, which kfxgen's heading can't print.
            blocks
                .first()
                .filter(|b| b.heading && !images::has_picture(&b.text))
        } else {
            None
        };
        if let Some(b) = book_block {
            title = py::strip(&b.text).to_owned();
            title_spans = Some(inline::title_spans(&b.text, &b.spans, py::len(&title)));
        }

        // A body of pictures alone after the title prints no heading of kfxgen's on top of
        // them, nor one under pictures above the book's own heading (#33, #346, :6258).
        // With no rules and no tables, the title isn't in a table.
        let stripped = py::lstrip(&chapter.text);
        let body_after_title = if opens_with_title(stripped, &title) {
            py::lstrip(py::skip(stripped, py::len(&title)))
        } else {
            stripped
        };
        let image_only = !toc && images::only_pictures(body_after_title);
        let title_after_pictures =
            !toc && images::title_after_pictures(blocks.iter().map(|b| b.text.as_str()), &title);
        let runs_in = !toc && title_runs_in(blocks, &title);
        let heading_near_start = !toc
            && title_spans.is_none()
            && blocks
                .iter()
                .take(MAX_SPLIT_TITLE_BLOCKS)
                .any(|b| b.heading);
        let label_over_blocks = !toc && label_blocks(blocks, &title) > 1;
        let omit_heading = chapter.is_cover
            || py::strip(&title).is_empty()
            || image_only
            || title_after_pictures
            || runs_in
            || heading_near_start
            || label_over_blocks;
        let mut heading_chunk = None;
        if !omit_heading {
            heading_chunks.insert(chunks.len());
            heading_chunk = Some(chunks.len());
            chunks.push(Chunk {
                heading_level: chapter.heading_level,
                spans: title_spans.unwrap_or_default(),
                ..Chunk::text(title.clone())
            });
        }

        if toc {
            for link in &chapter.toc_links {
                toc_link_chunks.insert(chunks.len(), (link.target, link.level));
                chunks.push(Chunk::text(link.text.clone()));
            }
        } else {
            let cut_title = !image_only && !runs_in && !heading_near_start && !label_over_blocks;
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
                                let remainder = py::lstrip(py::skip(first_stripped, n)).to_owned();
                                if !remainder.is_empty() {
                                    // `_trim_text` (:909): runs and offsets move with the cut.
                                    let rest = py::len(&remainder) as i64;
                                    let first = &mut iter_blocks[0];
                                    let cut = py::len(&first.text) as i64 - rest;
                                    first.text = remainder;
                                    inline::trim(&mut first.spans, &mut first.anchors, cut, rest);
                                } else {
                                    let first = iter_blocks.remove(0);
                                    carried.extend(first.anchors.keys.iter().cloned());
                                    if let Some(h) = heading_chunk {
                                        heading_offsets = inline::heading_offsets(
                                            &first.text,
                                            &first.anchors,
                                            &title,
                                        );
                                        print_book_heading(&mut chunks[h], &first, &title);
                                    }
                                }
                            } else {
                                // `_eat_split_title` (:1524) for paragraphs. The heading takes
                                // the first block's style, which is none.
                                let eaten = consume_split_title(&iter_blocks, &title);
                                for b in iter_blocks.drain(..eaten) {
                                    carried.extend(b.anchors.keys);
                                }
                            }
                        }
                        iter_blocks
                    }
                    None => text
                        .split("\n\n")
                        .map(|p| {
                            if !images::has_picture(p) && cut_at_empty_line(p) {
                                return Err(Unsupported("a paragraph cut at an empty line"));
                            }
                            Ok(Block {
                                text: p.to_owned(),
                                heading: false,
                                heading_level: None,
                                space_above: false,
                                spans: Vec::new(),
                                anchors: Anchors::default(),
                            })
                        })
                        .collect::<Result<_, _>>()?,
                };
                // Ids of a paragraph that emitted nothing, only pictures the book doesn't
                // hold, wait for the next block's first chunk (#291), else go on the
                // chapter's last, else its start (:6483).
                let mut pending: Vec<String> = Vec::new();
                for (b_idx, block) in para_iter.iter().enumerate() {
                    let first = chunks.len();
                    let unplaced = emit_paragraph(block, b_idx == 0, images, &mut chunks)?;
                    if !pending.is_empty() && chunks.len() > first {
                        chunks[first].anchors.attach(&pending);
                        pending.clear();
                    }
                    pending.extend(unplaced);
                }
                if !pending.is_empty() {
                    if chunks.len() > start_idx {
                        chunks.last_mut().unwrap().anchors.attach(&pending);
                    } else {
                        carried.extend(pending);
                    }
                }
            } else if let Some(first) = blocks.first() {
                // Only its title: the blocks' ids still name this chapter (#338), and the
                // book's heading prints as kfxgen's (:6522).
                for b in blocks {
                    carried.extend(b.anchors.keys.iter().cloned());
                }
                if let Some(h) = heading_chunk
                    && py::strip(&first.text) == title
                {
                    heading_offsets = inline::heading_offsets(&first.text, &first.anchors, &title);
                    print_book_heading(&mut chunks[h], first, &title);
                }
            }
        }
        // Pictures of a contents page the converter replaced, after it (#117).
        for token in &chapter.preserved_images {
            if !py::strip(&images::IMG_TOKEN.replace_all(token, "")).is_empty() {
                return Err(Unsupported("words kept from a replaced contents page"));
            }
            for picture in images::pictures(token, &[], images)? {
                chunks.push(Chunk {
                    picture: Some(picture),
                    ..Chunk::text(String::new())
                });
            }
        }

        // Every chapter has an entry to land on (:6597): a no-break space.
        if chunks.len() == start_idx {
            chunks.push(Chunk::text("\u{a0}".to_owned()));
        }
        if !carried.is_empty() {
            // Offsets of the block kfxgen's heading prints, else the chapter's start (:6622).
            let offsets = match heading_chunk {
                Some(_) => heading_offsets,
                None => BTreeMap::new(),
            };
            chunks[start_idx].anchors.attach_at(&carried, &offsets);
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
    let chapter_start_positions: Vec<i64> = chapter_chunk_ranges
        .iter()
        .map(|&(start, _)| chunk_positions[start])
        .collect();

    // $266: a contents line's target (:6763), then each link target the book declares.
    let mut anchor_names = Vec::new();
    let mut chunk_anchor_map = BTreeMap::new();
    for (&idx, &(target, _)) in &toc_link_chunks {
        let name =
            inline::build_toc_anchor(g, &mut anchor_names, &chapter_start_positions, target)?;
        chunk_anchor_map.insert(idx, name);
    }
    let links: Vec<ChunkLinks> = chunks
        .iter()
        .zip(&chunk_positions)
        .map(|(c, &position)| ChunkLinks {
            spans: &c.spans,
            anchors: &c.anchors,
            position,
        })
        .collect();
    let body_anchor_map = inline::build_body_anchors(g, &links, &mut anchor_names);

    // $145: each chapter's text in fragments under the cap (:6863); a chapter of pictures
    // alone, a cover's, has none (#102).
    let mut content_names = Vec::new();
    let mut content_refs_per_chapter = Vec::new();
    let mut next_content_index = 1;
    for &(start, end) in &chapter_chunk_ranges {
        let text_chunks: Vec<usize> = (start..end)
            .filter(|&i| chunks[i].picture.is_none())
            .collect();
        let mut refs_by_chunk: Vec<Option<ContentRef>> = vec![None; end - start];
        if !text_chunks.is_empty() {
            let strings: Vec<&str> = text_chunks
                .iter()
                .map(|&i| chunks[i].text.as_str())
                .collect();
            let (groups, refs) = pack_content_fragments(&strings, next_content_index);
            next_content_index += groups.len();
            for (name, strings) in groups {
                let fragment = build_fragment_145(g, strings, &name);
                g.push(fragment);
                content_names.push(name);
            }
            for (&i, r) in text_chunks.iter().zip(refs) {
                refs_by_chunk[i - start] = Some(r);
            }
        }
        content_refs_per_chapter.push(refs_by_chunk);
    }

    // $157: a body style per chapter and a heading style per chapter with a heading (:6973).
    let mut styles = Styles::default();
    let mut story_names = Vec::new();
    let mut extra_style_names: Vec<String> = Vec::new();
    let mut heading_style_names = Vec::new();
    let mut toc_link_style_names = Vec::new();
    for (i, &(start, end)) in chapter_chunk_ranges.iter().enumerate() {
        let fs = chapters[i].font_size;
        story_names.push(styles.allocate(g, "", Style157::body(fs)));
        let mut heading_name = None;
        if heading_chunks.range(start..end).next().is_some() {
            let heading = Style157 {
                font_size: py::round((fs * HEADING_FONT_SIZE).max(1.0), 4),
                bold: true,
                margin_top: (i > 0).then_some(2.0),
                is_heading: true,
                ..Style157::default()
            };
            let name = styles.allocate(g, "_h", heading);
            if !extra_style_names.contains(&name) {
                extra_style_names.push(name.clone());
            }
            heading_name = Some(name);
        }
        heading_style_names.push(heading_name);
        // A contents page's links are underlined (:7002).
        let mut link_name = None;
        if !chapters[i].toc_links.is_empty() {
            let link = Style157 {
                underline: true,
                ..Style157::body(fs)
            };
            let name = styles.allocate(g, "_link", link);
            if !extra_style_names.contains(&name) {
                extra_style_names.push(name.clone());
            }
            link_name = Some(name);
        }
        toc_link_style_names.push(link_name);
    }
    let pictures: Vec<&Picture> = chunks.iter().filter_map(|c| c.picture.as_ref()).collect();
    let image_styles = ImageStyles::allocate(g, &pictures, images, &mut extra_style_names)?;

    // $259: a storyline per chapter (:7359).
    let mut storyline_names = Vec::new();
    for (ch_idx, &(start, end)) in chapter_chunk_ranges.iter().enumerate() {
        let sl_name = format!("l{ch_idx}");
        let fs = chapters[ch_idx].font_size;
        let mut entry_styles = Vec::new();
        let mut entry_runs = Vec::new();
        for chunk_idx in start..end {
            if let Some(p) = &chunks[chunk_idx].picture {
                let style = image_styles.style_for(p)?;
                entry_styles.push(style.unwrap_or_else(|| story_names[ch_idx].clone()));
                entry_runs.push((None, Vec::new()));
                continue;
            }
            let heading = heading_chunks.contains(&chunk_idx);
            let mut link = None;
            let style = match (
                &heading_style_names[ch_idx],
                chunk_anchor_map.get(&chunk_idx),
            ) {
                (Some(name), _) if heading => name.clone(),
                (_, Some(anchor)) => {
                    // A contents line: the page's style, its link underlined (:7707).
                    let (_, level) = toc_link_chunks[&chunk_idx];
                    link = Some((
                        anchor.clone(),
                        toc_link_style_names[ch_idx].clone().unwrap_or_default(),
                        py::len(&chunks[chunk_idx].text) as i64,
                    ));
                    if level != 0 {
                        // Indented by depth, 6.25% a level (#238, #313).
                        let indent = Style157 {
                            margin_left: Some(format!("{}", 6.25 * level as f64)),
                            ..Style157::body(fs)
                        };
                        styles.allocate(g, "", indent)
                    } else {
                        story_names[ch_idx].clone()
                    }
                }
                // `_block_font_size` with no CSS size: the chapter's.
                _ => styles.allocate(g, "", Style157::body(fs)),
            };
            entry_styles.push(style);
            // kfxgen's chapter heading is bold, and so are the runs in it (#366).
            let runs = inline::entry_runs(
                g,
                &mut styles,
                &mut extra_style_names,
                &chunks[chunk_idx].spans,
                heading,
                &body_anchor_map,
            );
            entry_runs.push((link, runs));
        }
        let fragment = build_fragment_259(
            g,
            &entry_styles,
            &content_refs_per_chapter[ch_idx],
            &sl_name,
            &chunk_positions[start..end],
            &entry_runs,
            &chunks[start..end],
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
        anchor_names,
    })
}
