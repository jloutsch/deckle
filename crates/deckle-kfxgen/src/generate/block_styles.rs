// SPDX-License-Identifier: GPL-3.0-or-later
//! A paragraph's own style, the `block_style` the converter reads from the book's CSS, and the
//! `$157` `_build_chapter_content` (`native_generator.py:5396`) makes of it: sizes against
//! the book's main text size (#241), line heights against its main line (#463), the book's
//! alignment, indent, margins, weight, direction and language (:7735), and the book's heading printing
//! as kfxgen's (#490, :7646).
use super::boxes::BoxStyle;
use super::chunks::{Chunk, InCell, Marker, Node};
use super::inline::run_font_size;
use super::styles::{Style157, StyleArgs, Styles};
use super::table_styles::{self, CellStyle};
use super::writing::{Bidi, bidi_diff};
use super::{GenerateError, Generator, model_err, py};
use crate::pyvalue::PyValue;

use GenerateError::Unsupported;

/// A KFX length as the converter writes it: (magnitude, unit symbol).
pub(crate) type Length = (String, String);

/// `block_style`: a dict of the keys below, all of them set (None when the CSS has none) on
/// a Calibre-path block.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct BlockStyle {
    pub align: Option<String>,
    pub indent: Option<Length>,
    pub margin_left: Option<Length>,
    pub margin_right: Option<Length>,
    pub family: Vec<String>,
    pub line_height: Option<f64>,
    pub bold: bool,
    pub italic: bool,
    /// In CSS px.
    pub font_size: Option<f64>,
    /// `bidi`: its direction and language (#246).
    pub bidi: Option<Bidi>,
}

pub(crate) fn number(v: &PyValue) -> Option<f64> {
    match v {
        PyValue::Int(i) => Some(*i as f64),
        PyValue::Float(f) => Some(*f),
        _ => None,
    }
}

/// A block's `block_style`; None for none or `{}`, which kfxgen reads alike (`or {}`).
pub(crate) fn read_block_style(
    v: Option<&PyValue>,
    at: &str,
) -> Result<Option<BlockStyle>, GenerateError> {
    let at = format!("{at}.block_style");
    let pairs = match v {
        None | Some(PyValue::None) => return Ok(None),
        Some(PyValue::Dict(pairs)) if pairs.is_empty() => return Ok(None),
        Some(PyValue::Dict(pairs)) => pairs,
        Some(_) => return Err(model_err(format!("{at}: a dict"))),
    };
    let mut bs = BlockStyle {
        align: None,
        indent: None,
        margin_left: None,
        margin_right: None,
        family: Vec::new(),
        line_height: None,
        bold: false,
        italic: false,
        font_size: None,
        bidi: None,
    };
    for (k, v) in pairs {
        let key = k
            .as_str()
            .ok_or_else(|| model_err(format!("{at}: string keys")))?;
        let err = |what: &str| model_err(format!("{at}.{key}: {what}"));
        if matches!(v, PyValue::None) {
            continue;
        }
        match key {
            // `_typography` (:7144) takes any of these that is not None.
            "small_caps" | "text_transform" | "letter_spacing" | "colour" | "text_orientation"
            | "text_emphasis" => {
                return Err(Unsupported(
                    "small caps, case, spacing and colour in paragraphs",
                ));
            }
            "bidi" => {
                bs.bidi = Bidi::read(Some(v), &format!("{at}.bidi"))?;
                continue;
            }
            // The others are read for their truth: a false one is none.
            _ if !v.is_truthy() => continue,
            _ => {}
        }
        match key {
            "align" => bs.align = Some(v.as_str().ok_or_else(|| err("a string"))?.to_owned()),
            "indent" | "margin_left" | "margin_right" => {
                let length =
                    read_length(v).ok_or_else(|| err("(a decimal of at most 38 digits, unit)"))?;
                match key {
                    "indent" => bs.indent = Some(length),
                    "margin_left" => bs.margin_left = Some(length),
                    _ => bs.margin_right = Some(length),
                }
            }
            "font_family" => {
                bs.family = v
                    .as_list()
                    .and_then(|l| l.iter().map(|f| f.as_str().map(str::to_owned)).collect())
                    .ok_or_else(|| err("a list of strings"))?;
            }
            "line_height" => bs.line_height = Some(number(v).ok_or_else(|| err("a number"))?),
            "font_size" => bs.font_size = Some(number(v).ok_or_else(|| err("a number"))?),
            "bold" => bs.bold = true,
            "italic" => bs.italic = true,
            "drop_cap" => return Err(Unsupported("drop caps")),
            _ => return Err(Unsupported("a block style")),
        }
    }
    Ok(Some(bs))
}

/// `(magnitude, unit)`, the magnitude a decimal kfxgen writes and deckle-kfx holds
/// (`py::checked_decimal`; `build_fragment_157` :3640).
fn read_length(v: &PyValue) -> Option<Length> {
    let [m, u] = v.as_list()? else {
        return None;
    };
    let m = m.as_str()?;
    py::checked_decimal(m)?;
    Some((m.to_owned(), u.as_str()?.to_owned()))
}

/// Characters by size or line height: a Python `Counter`, its counts summed in an i128 where
/// Python's ints have no bound.
type Counts = Vec<(f64, i128)>;

/// `_most` (:7163): the value most characters have, the smallest of a tie.
fn most(counts: &Counts) -> Option<f64> {
    let mut sorted = counts.clone();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut best: Option<(f64, i128)> = None;
    for (v, n) in sorted {
        if best.is_none_or(|(_, m)| n > m) {
            best = Some((v, n));
        }
    }
    best.map(|(v, _)| v)
}

fn add(counts: &mut Counts, value: f64, n: i128) {
    match counts.iter_mut().find(|(v, _)| *v == value) {
        Some((_, m)) => *m += n,
        None => counts.push((value, n)),
    }
}

/// `_chars_by_px` (:7147): a text entry's characters at their run's size, else at its CSS
/// size (#375 review). A count goes below 0 where sized runs overlap, as in Python.
fn chars_by_px(chunk: &Chunk) -> Counts {
    let mut counts = Counts::new();
    let Some(px) = chunk.style.as_ref().and_then(|b| b.font_size) else {
        return counts;
    };
    if px == 0.0 || chunk.marker.is_some() || chunk.rule.is_some() || chunk.picture.is_some() {
        return counts;
    }
    let mut in_runs = 0;
    for sp in &chunk.spans {
        if let Some(run_px) = run_font_size(&sp.flags).filter(|&p| p != 0.0) {
            add(&mut counts, run_px, sp.len.into());
            in_runs += i128::from(sp.len);
        }
    }
    add(&mut counts, px, py::len(&chunk.text) as i128 - in_runs);
    counts
}

/// The book's main text size and line height (:7171, :7183): what most of its text has.
pub(crate) struct Sizes {
    main_px: Option<f64>,
    main_line: Option<f64>,
}

impl Sizes {
    pub(crate) fn of(chunks: &[Chunk]) -> Self {
        let (mut px, mut lines) = (Vec::new(), Vec::new());
        for chunk in chunks {
            for (size, n) in chars_by_px(chunk) {
                add(&mut px, size, n);
            }
            let factor = chunk.style.as_ref().and_then(|bs| bs.line_height);
            if let Some(f) = factor.filter(|&f| f != 0.0)
                && chunk.marker.is_none()
                && chunk.rule.is_none()
                && chunk.picture.is_none()
            {
                add(&mut lines, f, py::len(&chunk.text) as i128);
            }
        }
        Sizes {
            main_px: most(&px),
            main_line: most(&lines),
        }
    }

    /// `main_px or 16.0` (:7666).
    pub(crate) fn main_px_or_16(&self) -> f64 {
        self.main_px.filter(|&p| p != 0.0).unwrap_or(16.0)
    }

    /// `_rem` (:7195): a CSS px size against the main text size.
    pub(crate) fn rem(&self, px: Option<f64>) -> f64 {
        match (px, self.main_px) {
            (Some(px), Some(main)) if px != 0.0 && main != 0.0 => py::round(px / main, 4),
            _ => 1.0,
        }
    }

    /// `_line_ratio` (:7188): Previewer writes none below 0.6.
    fn line_ratio(&self, bs: Option<&BlockStyle>) -> f64 {
        let factor = bs.and_then(|b| b.line_height).filter(|&f| f != 0.0);
        match (factor, self.main_line.filter(|&m| m != 0.0)) {
            (Some(f), Some(main)) => py::format_g((f / main).max(0.6), 6).parse().unwrap_or(1.0),
            _ => 1.0,
        }
    }

    /// `_block_font_size` (:7201): a page kfxgen sizes keeps its size; any other block takes
    /// the size of a span holding all its text, else its CSS size (#375 review).
    pub(crate) fn block_font_size(&self, chapter_fs: f64, chunk: &Chunk) -> f64 {
        if chapter_fs != 1.0 {
            return chapter_fs;
        }
        // `+counts`: the sizes some characters have.
        let sizes: Vec<f64> = chars_by_px(chunk)
            .into_iter()
            .filter(|&(_, n)| n > 0)
            .map(|(px, _)| px)
            .collect();
        match sizes[..] {
            [px] => self.rem(Some(px)),
            _ => self.rem(chunk.style.as_ref().and_then(|b| b.font_size)),
        }
    }
}

/// A text entry's `$157` (:7735): its block's style, or its cell's (#261).
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_entry_style(
    g: &mut Generator,
    styles: &mut Styles,
    sizes: &Sizes,
    chunk: &Chunk,
    chapter_fs: f64,
    open_sizes: &[Option<f64>],
    around: Option<&Bidi>,
) -> String {
    let bs = chunk.style.as_ref();
    let mut attrs = Style157::body(sizes.block_font_size(chapter_fs, chunk));
    // At the main size in a cell that isn't: 1, or it takes the cell's (#681).
    let cell_fs = open_sizes.iter().rev().flatten().find(|&&f| f != 0.0);
    if cell_fs.is_some_and(|&f| f != 1.0) && attrs.font_size == 1.0 {
        attrs.explicit_size = Some(true);
    }
    attrs.line_height = sizes.line_ratio(bs);
    if let Some(bs) = bs {
        attrs.align = bs.align.clone();
        attrs.text_indent = bs.indent.clone();
        attrs.margin_left = bs.margin_left.clone();
        attrs.margin_right = bs.margin_right.clone();
    }
    attrs.space_above = chunk.space_above;
    attrs.space_below = chunk.space_below.filter(|&b| b != 0.0);
    let (mut blk_bold, blk_italic) = bs.map_or((false, false), |b| (b.bold, b.italic));
    if matches!(chunk.in_cell, Some(InCell::Paragraph { header_bold: true })) {
        // A paragraph in a header cell is bold, as its own text is (#261).
        blk_bold = true;
    }
    attrs.font_family = g
        .fonts
        .family(bs.map_or(&[], |b| &b.family), blk_bold, blk_italic);
    attrs.bold = blk_bold;
    attrs.italic = blk_italic;
    let bidi = bidi_diff(bs.and_then(|b| b.bidi.as_ref()), around);
    match &chunk.in_cell {
        Some(InCell::Text(cell)) => {
            let cell = CellStyle::of(&g.fonts, cell, bs, attrs.font_size);
            styles.allocate_in(g, "_td", StyleArgs::Cell(cell), &bidi)
        }
        _ => styles.allocate_in(g, "", attrs, &bidi),
    }
}

/// kfxgen's heading's arguments (:6984).
pub(crate) fn heading_attrs(chapter_fs: f64, first_chapter: bool) -> Style157 {
    Style157 {
        font_size: py::round((chapter_fs * 1.2).max(1.0), 4),
        bold: true,
        margin_top: (!first_chapter).then_some(2.0),
        is_heading: true,
        ..Style157::default()
    }
}

/// kfxgen's heading's `$157` (:7646). Printing the book's heading, its weight, size,
/// alignment and side margins, as Kindle Previewer prints it, over kfxgen's spacing (#490);
/// the space below and above it in px (#653, #665). None: the chapter's heading style.
pub(crate) fn heading_style(
    g: &mut Generator,
    styles: &mut Styles,
    sizes: &Sizes,
    chunk: &Chunk,
    attrs: &Style157,
    around: Option<&Bidi>,
) -> Option<String> {
    let book = chunk.heading.book_style.as_ref();
    let own = match book {
        Some(b) => b.bidi.as_ref(),
        None => chunk.heading.page_bidi.as_ref(),
    };
    let bidi = bidi_diff(own, around);
    let mut attrs = attrs.clone();
    if let Some(book) = book {
        attrs.bold = book.bold;
        attrs.font_size = sizes.rem(book.font_size);
        attrs.align = book.align.clone();
        attrs.margin_left = book.margin_left.clone();
        attrs.margin_right = book.margin_right.clone();
        // None set: none written, not kfxgen's justified body default.
        attrs.no_align = book.align.is_none();
        attrs.italic = book.italic;
    }
    let size_px = attrs.font_size * sizes.main_px_or_16();
    if let Some(below) = chunk.heading.space_below_px.filter(|&b| b != 0.0) {
        attrs.space_below = Some(below / size_px);
    }
    if book.is_some() {
        if let Some(above) = chunk.heading.space_above_px {
            attrs.space_above = Some(above / size_px);
            attrs.margin_top = None;
        }
    } else if attrs.space_below.is_none() && bidi.is_empty() {
        return None;
    }
    Some(styles.allocate_in(g, "_h", attrs, &bidi))
}

/// What `_print_book_heading` (:1477) and the chapter's page put on kfxgen's heading.
#[derive(Clone, Default)]
pub(crate) struct HeadingStyle {
    /// The book's heading's style, when it prints as kfxgen's (#490); never `{}`.
    pub book_style: Option<BlockStyle>,
    pub space_above_px: Option<f64>,
    pub space_below_px: Option<f64>,
    /// The page's direction and language, from its first block that has them (#714).
    pub page_bidi: Option<Bidi>,
}

/// `_keep_space_above` and `_keep_space_below` (:1502, :1514): the book's heading's space,
/// in px.
pub(crate) fn keep_space(
    heading: &mut HeadingStyle,
    style: Option<&BlockStyle>,
    margin_above: Option<Option<f64>>,
    margin_below: f64,
) {
    let px = style
        .and_then(|b| b.font_size)
        .filter(|&p| p != 0.0)
        .unwrap_or(16.0);
    if margin_below != 0.0 {
        heading.space_below_px = Some(margin_below * px);
    }
    // 0 too: the book set none, and Previewer writes none (QA on #686).
    if let Some(above) = margin_above {
        heading.space_above_px = Some(above.unwrap_or(0.0) * px);
    }
}

/// The containers open around the entry being written (:7376): each one's size when it is a
/// cell (#681), and its direction and language (#246).
#[derive(Default)]
pub(crate) struct Containers {
    pub sizes: Vec<Option<f64>>,
    bidis: Vec<Option<Bidi>>,
}

impl Containers {
    /// The direction and language of the container around, None outside one.
    pub(crate) fn around(&self) -> Option<&Bidi> {
        self.bidis.last()?.as_ref()
    }

    /// A container's `open` or `close` (:7398): (the entry's style, its own `$157`, a row's,
    /// group's or box's).
    pub(crate) fn marker_styles(
        &mut self,
        g: &mut Generator,
        styles: &mut Styles,
        sizes: &Sizes,
        chunk: &Chunk,
        marker: &Marker,
        chapter_fs: f64,
    ) -> Result<(Option<String>, Option<String>), GenerateError> {
        let node = match marker {
            Marker::Open(node) => node,
            Marker::Close => {
                self.sizes.pop();
                self.bidis.pop();
                styles.fills.pop();
                return Ok((None, None));
            }
        };
        // A box's or cell's fill (#383, :7422).
        styles.fills.push(match node {
            Node::Box(b) => b.spec.background,
            Node::Cell(cell) => cell.background,
            _ => None,
        });
        let own = match node {
            Node::Table(t) => t.bidi.clone(),
            Node::Cell(_) => chunk.style.as_ref().and_then(|b| b.bidi.clone()),
            Node::Box(b) => b.spec.bidi.clone(),
            Node::Group(_, _, bidi) | Node::Row(_, bidi) => bidi.clone(),
            Node::Link(_) => None,
        };
        let around = self.around().cloned();
        let bidi = bidi_diff(own.as_ref(), around.as_ref());
        self.bidis.push(own.or(around));
        let cell_size =
            matches!(node, Node::Cell(_)).then(|| sizes.block_font_size(chapter_fs, chunk));
        self.sizes.push(cell_size);
        Ok(match node {
            // A linked picture's container centres it, as `$580` does a picture alone (:7505).
            Node::Link(_) => {
                let centred = Style157 {
                    align: Some("center".to_owned()),
                    ..Style157::body(chapter_fs)
                };
                (None, Some(styles.allocate(g, "", centred)))
            }
            Node::Box(b) => {
                let style = BoxStyle::of(b, sizes.rem(b.spec.font_px));
                style.check()?;
                let name = styles.allocate_in(g, "_box", StyleArgs::Box(style), &bidi);
                (None, Some(name))
            }
            _ => table_styles::marker_styles(
                g,
                styles,
                marker,
                chunk.style.as_ref(),
                cell_size.unwrap_or(chapter_fs),
                &bidi,
            ),
        })
    }
}
