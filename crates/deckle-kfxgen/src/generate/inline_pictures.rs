// SPDX-License-Identifier: GPL-3.0-or-later
//! A paragraph's entries (`_emit_paragraph`, `native_generator.py:5745` at kfxgen `4b81651`):
//! cut after its picture lines and at its empty lines (`_cut_after_picture_lines` :535), its
//! pictures written among its words as Kindle Previewer 4.0.3 writes them (`_inline_pictures`
//! :5563, #529, #565), or cut into text and pictures (`_emit_text_chunks` :5454), a linked
//! picture in a container of its link (#355). A picture among words in the storyline is
//! `inline_picture_entries`'.
use std::borrow::Cow;
use std::collections::BTreeMap;

use regex::Captures;

use super::chapters::Block;
use super::chunks::{ASCII_SPACE, ChapterNow, Chunk, LinkOpen, Marker, Node, space_to_write};
use super::images::{self, IMG_TOKEN, Images, Picture};
use super::inline::{Anchors, Flag, Span};
use super::inline_picture_entries::InlinePicture;
use super::py;

/// `INLINE_PICTURE` (:1920): a picture inside a paragraph's words, one character of its text.
const INLINE_PICTURE: char = '\u{fffc}';

/// A token's group `i`, when Python reads it as true: there and not empty.
fn group<'t>(m: &Captures<'t>, i: usize) -> Option<&'t str> {
    m.get(i).map(|g| g.as_str()).filter(|s| !s.is_empty())
}

fn has_words(text: &str) -> bool {
    !py::strip(&IMG_TOKEN.replace_all(text, "")).is_empty()
}

/// `text[a:b]`, in characters.
fn slice(text: &str, a: usize, b: usize) -> &str {
    py::take(py::skip(text, a), b.saturating_sub(a))
}

/// The runs of `spans` inside [a, b), counted from `a` (:600).
fn slice_spans(spans: &[Span], a: i64, b: i64) -> Vec<Span> {
    spans
        .iter()
        .filter_map(|s| {
            let (start, end) = (s.start.max(a), (s.start + s.len).min(b));
            (end > start).then(|| Span {
                start: start - a,
                len: end - start,
                flags: s.flags.clone(),
            })
        })
        .collect()
}

/// `_cut_after_picture_lines` (:535): the paragraph cut after each line holding only a
/// picture when more follows, and at a line with words after an empty one, as Previewer ends
/// an entry there (#565, #590, #593, #602). None when nothing cuts it, it has no words, or a
/// picture in it has a float side. A preformatted paragraph is Unsupported when read.
pub(crate) fn cut_after_picture_lines(block: &Block) -> Option<Vec<Block>> {
    let text = &block.text;
    if IMG_TOKEN
        .captures_iter(text)
        .any(|m| group(&m, 4).is_some())
        || !has_words(text)
    {
        return None;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let blank: Vec<bool> = lines.iter().map(|l| py::strip(l).is_empty()).collect();
    let last_words = blank.iter().rposition(|b| !b);
    // (end of a piece, start of the next, what the piece keeps after its words): a cut after
    // a picture line drops its line break; one before a line after an empty line drops
    // nothing.
    let mut cuts: Vec<(usize, usize, Option<String>)> = Vec::new();
    let (mut start, mut words_before) = (0, false);
    for (i, line) in lines.iter().enumerate() {
        let end = start + py::len(line);
        if !blank[i] && i > 0 && blank[i - 1] && words_before {
            // The piece ends at its last words, keeping one line break and the first blank
            // line's characters, its no-break spaces as plain ones (#626, QA on #628).
            let mut j = i - 1;
            while j > 0 && blank[j - 1] {
                j -= 1;
            }
            let words_end = lines[..j].iter().map(|x| py::len(x) + 1).sum::<usize>() - 1;
            let spacer = lines[j]
                .trim_start_matches(ASCII_SPACE)
                .replace('\u{a0}', " ");
            match cuts.last_mut() {
                // After a line of only a picture, cut there already (#590): the blank lines
                // go to the next piece.
                Some(last) if last.0 == words_end => *last = (words_end, start, None),
                _ => cuts.push((words_end, start, Some(format!("\n{spacer}")))),
            }
        }
        if IMG_TOKEN.is_match(line)
            && !has_words(line)
            && i < lines.len() - 1
            && last_words.is_some_and(|w| w > i)
        {
            cuts.push((end, end + 1, None));
        }
        words_before |= !blank[i];
        start = end + 1;
    }
    if cuts.is_empty() {
        return None;
    }
    let n = py::len(text);
    let ends = cuts
        .into_iter()
        .map(|(b, after, tail)| (b, Some(after), tail))
        .chain([(n, None, None)]);
    let mut pieces: Vec<Block> = Vec::new();
    let (mut a, mut lo, mut empty_line_cut) = (0, 0, false);
    for (b, after, tail) in ends {
        // A piece holds its end (#487), its cut (QA on #590), and the blank lines after it
        // when it keeps a tail (#626). One at the next piece's start (#592), or in the blank
        // lines after a picture line, is the next piece's.
        let hi = after.map(|after| (if tail.is_some() { after } else { b + 1 }) as i64);
        let tail_len = tail.as_deref().map_or(0, py::len) as i64;
        let (ai, bi) = (a as i64, b as i64);
        let offsets: BTreeMap<String, i64> = block
            .anchors
            .offsets
            .iter()
            .filter(|&(_, &o)| lo <= o && hi.is_none_or(|h| o < h))
            .map(|(k, &o)| {
                // In the blank lines after it: at the same place in its tail.
                let at = if o <= bi {
                    (o - ai).max(0)
                } else {
                    (bi - ai) + (o - bi - 1).min(tail_len - 1)
                };
                (k.clone(), at)
            })
            .collect();
        let keys = block
            .anchors
            .keys
            .iter()
            .filter(|k| offsets.contains_key(*k))
            .cloned()
            .collect();
        let mut piece = Block {
            text: slice(text, a, b).to_owned(),
            spans: slice_spans(&block.spans, ai, bi),
            anchors: Anchors { keys, offsets },
            ..block.clone()
        };
        if let Some(tail) = &tail {
            piece.trailing = tail.clone();
        }
        if a > 0 {
            // The paragraph goes on: no space above or first-line indent of its own. A
            // heading's parts after an empty line stay headings (#602).
            piece.space_above = 0.0;
            piece.margin_above = None;
            piece.tight = false;
            if !empty_line_cut {
                piece.heading_level = None;
            }
            if let Some(style) = &mut piece.style {
                style.indent = None;
            }
        }
        if hi.is_some() {
            piece.margin_below = 0.0; // the paragraph goes on (#553)
        }
        pieces.push(piece);
        empty_line_cut = tail.is_some();
        if let (Some(hi), Some(after)) = (hi, after) {
            (lo, a) = (hi, after);
        }
    }
    // A key with no offset, or one before the text, stays on the first piece.
    let placed: Vec<String> = pieces
        .iter()
        .flat_map(|p| p.anchors.keys.iter().cloned())
        .collect();
    let rest: Vec<String> = block
        .anchors
        .keys
        .iter()
        .filter(|k| !placed.contains(k))
        .cloned()
        .collect();
    pieces[0].anchors.keys.extend(rest);
    Some(pieces)
}

/// `_inline_pictures`' paragraph: its text, each picture one character, its runs and anchor
/// offsets moved with it, and its pictures.
struct Inline {
    text: String,
    spans: Vec<Span>,
    offsets: BTreeMap<String, i64>,
    pictures: Vec<InlinePicture>,
}

/// `_inline_pictures` (:5563) for a paragraph whose words hold pictures, or None: one of
/// pictures alone, or with a float outside a frame, is cut at them. A float among a frame's
/// words is written in them, the space after it dropped, and a linked one unfloated (#378).
/// Floats in a table cell or a list item are Unsupported when read.
fn inline_pictures(
    para: &str,
    spans: &[Span],
    offsets: &BTreeMap<String, i64>,
    floats_in_words: bool,
    images: &Images,
) -> Option<Inline> {
    let tokens: Vec<Captures> = IMG_TOKEN.captures_iter(para).collect();
    if tokens.is_empty()
        || tokens.iter().any(|m| group(m, 4).is_some()) && !floats_in_words
        || !has_words(para)
    {
        return None;
    }
    let mut text = String::new();
    let mut pictures = Vec::new();
    // (start, end, characters kept) of each token, in `para`'s characters.
    let mut cuts: Vec<(i64, i64, i64)> = Vec::new();
    let (mut last, mut last_chars) = (0, 0);
    for m in &tokens {
        let whole = m.get(0).unwrap();
        text.push_str(&para[last..whole.start()]);
        let start = last_chars + py::len(&para[last..whole.start()]) as i64;
        let side = group(m, 4);
        let floated = side.is_some() && images::picture_link(spans, start).is_none();
        let mut extra = group(m, 5).unwrap_or("").to_owned();
        if let Some(side) = side.filter(|_| floated) {
            extra = [extra.as_str(), &format!("fl={side}")]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(";");
        }
        let resource = images.resource(&m[1]);
        if let Some((resource, _)) = resource {
            pictures.push(InlinePicture {
                at: py::len(&text) as i64,
                resource: resource.clone(),
                alt: m[2].replace('\x02', " "),
                size: group(m, 3).unwrap_or("").to_owned(),
                extra,
                eid: 0,
                font_size: 1.0,
            });
            text.push(INLINE_PICTURE);
        }
        let mut end = whole.end();
        if floated {
            // Previewer drops the space after a floated picture.
            end += para[end..].len() - para[end..].trim_start_matches(' ').len();
        }
        let end_chars = start + py::len(&para[whole.start()..end]) as i64;
        cuts.push((start, end_chars, i64::from(resource.is_some())));
        (last, last_chars) = (end, end_chars);
    }
    text.push_str(&para[last..]);
    // Where `i` lands once each token is one character, or none for a picture the book
    // doesn't hold; inside a token, at its start.
    let moved = |i: i64| {
        let mut shift = 0;
        for &(start, end, kept) in &cuts {
            if i >= end {
                shift += end - start - kept;
            } else if i > start {
                return start - shift;
            } else {
                break;
            }
        }
        i - shift
    };
    let spans = spans
        .iter()
        .filter_map(|s| {
            let (a, b) = (moved(s.start), moved(s.start + s.len));
            (b > a).then(|| Span {
                start: a,
                len: b - a,
                flags: s.flags.clone(),
            })
        })
        .collect();
    let offsets = offsets
        .iter()
        .map(|(k, &v)| (k.clone(), moved(v)))
        .collect();
    Some(Inline {
        text,
        spans,
        offsets,
        pictures,
    })
}

/// One of `_emit_text_chunks`' (:5454) pieces: text, or a picture the book holds and where its
/// token starts, in characters.
enum Piece {
    Text(String),
    Picture(Picture, i64),
}

/// `_emit_text_chunks` (:5454): `para` cut at its pictures, each piece of words trimmed of
/// ASCII spaces, none that is only spaces, the last keeping `trailing` (#602).
fn emit_text_chunks(para: &str, trailing: &str, images: &Images) -> Vec<Piece> {
    let words = |seg: &str| {
        let seg = seg.trim_matches(ASCII_SPACE);
        (!py::strip(seg).is_empty()).then(|| seg.to_owned())
    };
    let mut out = Vec::new();
    let mut last = 0;
    for m in IMG_TOKEN.captures_iter(para) {
        let whole = m.get(0).unwrap();
        if let Some(seg) = words(&para[last..whole.start()]) {
            out.push(Piece::Text(seg));
        }
        if let Some(p) = images::picture_of(&m, images) {
            out.push(Piece::Picture(p, py::len(&para[..whole.start()]) as i64));
        }
        last = whole.end();
    }
    if let Some(seg) = words(&para[last..]) {
        out.push(Piece::Text(seg + trailing));
    }
    out
}

/// `_append_text_with_spans` (:5507): the runs and anchors of `chunk_text`, found at its first
/// place in `para` (kfxgen #21: a repeated piece takes the first's runs), else at 0. An anchor
/// one after its last character lands on it (#487).
fn append_text_with_spans(
    chunk_text: &str,
    para: &str,
    spans: &[Span],
    keys: &[String],
    offsets: &BTreeMap<String, i64>,
) -> (Vec<Span>, Anchors) {
    let base = para
        .find(chunk_text)
        .map_or(0, |b| py::len(&para[..b]) as i64);
    let len = py::len(chunk_text) as i64;
    let runs = slice_spans(spans, base, base + len);
    let offset = |k: &String| offsets.get(k).copied().unwrap_or(0);
    let mine: Vec<String> = keys
        .iter()
        .filter(|k| (base..=base + len).contains(&offset(k)))
        .cloned()
        .collect();
    let last = (len - 1).max(0);
    let offsets = mine
        .iter()
        .map(|k| (k.clone(), (offset(k) - base).min(last)))
        .collect();
    (
        runs,
        Anchors {
            keys: mine,
            offsets,
        },
    )
}

/// A text entry: not a container's marker, a rule or a picture.
fn is_text(c: &Chunk) -> bool {
    c.marker.is_none() && c.rule.is_none() && c.picture.is_none()
}

/// `_emit_paragraph`'s entries (:5773–5897): its words one text entry with its pictures among
/// them, else cut at its pictures into text and picture entries, a linked picture in its
/// link's container (#355). Ids at its start go on its first picture (#62), any left on its
/// first entry. Its level goes on its first text entry, its space above on its first entry
/// when that is text, its margin below on its last text entry. Returns its ids when it
/// emitted nothing.
pub(crate) fn emit_entries(
    block: &Block,
    first_in_chapter: bool,
    float_wrapped: bool,
    now: ChapterNow,
    images: &Images,
    chunks: &mut Vec<Chunk>,
) -> Vec<String> {
    let para = block.text.trim_matches(ASCII_SPACE);
    if py::strip(para).is_empty() {
        return Vec::new();
    }
    let first = chunks.len();
    let mut keys = block.anchors.keys.clone();
    let (para, spans, offsets, mut pictures) = match inline_pictures(
        para,
        &block.spans,
        &block.anchors.offsets,
        float_wrapped,
        images,
    ) {
        Some(i) => (Cow::Owned(i.text), i.spans, i.offsets, Some(i.pictures)),
        None => (
            Cow::Borrowed(para),
            block.spans.clone(),
            block.anchors.offsets.clone(),
            None,
        ),
    };
    // A chapter's first paragraph opens a page or sits under its heading (#230, :6496).
    let scene = if first_in_chapter {
        0.0
    } else {
        block.space_above
    };
    let space = if block.tight {
        Some(0.0)
    } else {
        match block.margin_above {
            // The book's margins, as Previewer writes them (#553).
            Some(margin) => margin,
            None => space_to_write(block, scene),
        }
    };
    for piece in emit_text_chunks(&para, &block.trailing, images) {
        match piece {
            Piece::Picture(mut picture, at) => {
                let (at_start, rest) = keys
                    .into_iter()
                    .partition(|k| offsets.get(k).is_none_or(|&o| o == 0));
                keys = rest;
                let link = images::picture_link(&spans, at).cloned();
                // A picture in a horizontal block of vertical text: only its size, as Kindle
                // Previewer 4.0.3 writes it (#247, 0bc705d :5870). In a vertical book it is
                // inline in a plain container, as is a linked one in any book; in an ltr-spine
                // book, a block picture aligned by `$580` as the block is.
                picture.plain = picture.float.is_none() && now.in_horizontal_block(chunks);
                // A picture walked as a block of its own beside the block's paragraphs has no
                // block_style: the text-align it inherits (S755).
                let align = block
                    .style
                    .as_ref()
                    .and_then(|s| s.align.clone())
                    .or_else(|| block.picture_align.clone());
                let boxed = picture.plain && (link.is_some() || now.vertical_book);
                if picture.plain
                    && !boxed
                    && matches!(align.as_deref(), Some("left" | "center" | "right"))
                {
                    picture.box_align = align.clone();
                }
                let contained = link.is_some() || boxed;
                if contained {
                    // A container whose link covers the picture (#355). The Voyage drops a
                    // floated picture's link, so it stays unfloated (#243).
                    let open = LinkOpen {
                        target: link.clone(),
                        plain: boxed,
                        align: if boxed { align } else { None },
                    };
                    chunks.push(Chunk {
                        spans: link
                            .iter()
                            .map(|target| Span {
                                start: 0,
                                len: 1,
                                flags: [Flag::Link(target.clone())].into(),
                            })
                            .collect(),
                        ..Chunk::marker(Marker::Open(Node::Link(open)))
                    });
                    picture.inline = true;
                    picture.float = None;
                }
                chunks.push(Chunk {
                    anchors: Anchors {
                        keys: at_start,
                        offsets: BTreeMap::new(),
                    },
                    picture: Some(picture),
                    ..Chunk::text(String::new())
                });
                if contained {
                    chunks.push(Chunk::marker(Marker::Close));
                }
            }
            Piece::Text(text) => {
                let (runs, anchors) = append_text_with_spans(&text, &para, &spans, &keys, &offsets);
                keys.retain(|k| !anchors.keys.contains(k));
                chunks.push(Chunk {
                    spans: runs,
                    anchors,
                    style: block.style.clone(),
                    pictures: pictures.take().unwrap_or_default(),
                    ..Chunk::text(text)
                });
            }
        }
    }
    let emitted = &mut chunks[first..];
    if emitted.is_empty() {
        return keys;
    }
    if !keys.is_empty() {
        emitted[0].anchors.attach(&keys);
    }
    if let Some(level) = block.heading_level
        && let Some(c) = emitted.iter_mut().find(|c| is_text(c))
    {
        c.heading_level = Some(level);
    }
    if is_text(&emitted[0]) {
        emitted[0].space_above = space;
    }
    if block.margin_below != 0.0
        && let Some(c) = emitted.iter_mut().rev().find(|c| is_text(c))
    {
        c.space_below = Some(block.margin_below);
    }
    Vec::new()
}
