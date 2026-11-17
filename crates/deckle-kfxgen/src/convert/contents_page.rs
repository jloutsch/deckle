// SPDX-License-Identifier: GPL-3.0-only
//! The last chapter steps of `extract_chapters_from_oeb` (`converter.py` at kfxgen `ad38ec8`):
//! whether a dropped contents listing needs a second pass (`_listing_without_contents`), and
//! the title-page step (`_replace_title_page`), which rebuilds the book's contents page
//! (`_nav_listing_contents_chapter`, the three splits, `_rebuild_contents_page`,
//! `_keep_illustrations`).
//!
//! Identity and value. kfxgen picks the contents page by identity (`is`, `id()`), here by its
//! place in the list, kept as chapters are inserted before it. It finds the place to insert a
//! split-off chapter with `chapters.index(ch)`, which compares by value and so could find an
//! earlier chapter equal to `ch`. For the chapters assembly makes it never does, and here
//! `ch`'s own place is used:
//!
//! - The contents page is the first chapter titled so (not a file's) or, with none, the first
//!   flagged chapter of a short text and at most one block of text. A chapter before it equal
//!   to it, once cut, has its title and its keys, so is titled so; or is flagged, with a text
//!   and blocks that are a part of the page's, so no longer and no more: it would have been
//!   picked first.
//! - This rests on a chapter's `text` being its blocks' texts joined, as assembly makes it
//!   (`_mk`; only its chapters carry `_nav_listing_at`, and a file's chapter is never the
//!   titled page). A chapter handed in with another `text` can break it: kfxgen then inserts
//!   before an earlier equal chapter, the port before the page.
//!
//! On the page itself, `b is first` compares blocks by identity: here by their place.
//!
//! kfxgen finds an entry by its words with a scan of every entry, for every block of the page,
//! and each heading's next link with a scan of every later line, then inserts each heading
//! line into the list; here the entries are found through a map, the next links in one pass
//! from the end, and the list is built once.
//!
//! What this step copies is bounded by the chapters it is given (each title and place once,
//! the page's own text and keys once), but for the lines a page's headings repeat, each a copy
//! of an entry's title and key: those are counted first and refused past `MAX_CONTENT_FILE_BYTES`
//! (DIVERGENCES, "Headings a contents page repeats").

use std::collections::{HashMap, HashSet};

use deckle_limits::MAX_CONTENT_FILE_BYTES;

use super::Error;
use super::assemble::{
    CONTENTS_SKIP_TITLES, CONTENTS_TITLES, LEADING_TITLE_FALLBACK, LEADING_TITLE_MAX_LEN,
    SMALL_TEXT_CHAPTERS, normalize_title,
};
use super::blocks::{Block, Field, dedupe, dict_set, s, strs};
use super::chapters::{Chapter, drop_keys, joined_text};
use super::inline_style::{LINK_FLAG, page_bidi};
use crate::generate::images::IMG_TOKEN;
use crate::generate::py;
use crate::pyvalue::{PyValue, truthy};

/// `SMALL_FONT_SIZE`.
const SMALL_FONT_SIZE: f64 = 0.75;

/// `CONTENTS_PAGE_TITLE`.
const CONTENTS_PAGE_TITLE: &str = "Contents";

/// `_NAV_REMNANT_MAX_LEN`.
const NAV_REMNANT_MAX_LEN: usize = LEADING_TITLE_MAX_LEN;

/// `_LISTING_HEADINGS`: `_CONTENTS_TITLES` and what else a listing's own heading says.
const LISTING_HEADINGS: [&str; 27] = [
    "contents",
    "table of contents",
    "navigation",
    "toc",
    "inhalt",
    "inhaltsverzeichnis",
    "table des matières",
    "sommaire",
    "índice",
    "indice",
    "contenido",
    "contenidos",
    "sumário",
    "inhoud",
    "innehåll",
    "indhold",
    "innhold",
    "sisällys",
    "spis treści",
    "obsah",
    "содержание",
    "оглавление",
    "目次",
    "目录",
    "目錄",
    "차례",
    "목차",
];

/// `_listing_without_contents`: the book dropped a contents listing (and its listings are not
/// `kept`) but will have no contents page, so kfxgen extracts it again keeping its listings.
pub fn listing_without_contents(chapters: &[Chapter], kept: bool) -> bool {
    if kept || !chapters.iter().any(|c| truthy(c.get("_had_nav_listing"))) {
        return false;
    }
    !has_titled_contents(chapters) && nav_listing_contents_chapter(chapters).is_none()
}

/// `_replace_title_page` with the book's `title` (kfxgen's `metadata["title"]`): a title page
/// keeps its text but not its label; the book's contents page is rebuilt from the chapters
/// (the text above its listing, the files after its own and the front matter below the
/// listing split off first, as chapters of their own); some pages are set small. Nothing for
/// a book with no title. `listings_kept` is the second pass: a page titled "Contents" is then
/// printed as the book has it.
pub fn replace_title_page(
    chapters: &mut Vec<Chapter>,
    title: &str,
    listings_kept: bool,
) -> Result<(), Error> {
    if title.is_empty() {
        return Ok(());
    }
    let mut nav = nav_listing_contents_chapter(chapters);
    // The rebuilt page's lines print in the direction of the page it replaces.
    let mut bidi = PyValue::None;
    if let Some(n) = nav {
        bidi = page_bidi(chapters[n].blocks(), Block::get)
            .cloned()
            .unwrap_or(PyValue::None);
        if let Some(lead) = split_text_above_listing(&mut chapters[n]) {
            // `chapters.index(nav_contents_ch)` finds the page itself (module documentation).
            chapters.insert(n, lead);
            nav = Some(n + 1);
        }
    }
    let book = nav.or_else(|| {
        chapters
            .iter()
            .position(|c| !truthy(c.get("_from_spine")) && is_contents(c))
    });
    if let (None, Some(b)) = (nav, book) {
        bidi = page_bidi(chapters[b].blocks(), Block::get)
            .cloned()
            .unwrap_or(PyValue::None);
    }
    if let Some(b) = book {
        if let Some(later) = split_files_after_contents(&mut chapters[b]) {
            chapters.insert(b + 1, later);
        }
        if let Some(after) = split_front_matter_below_listing(&mut chapters[b]) {
            chapters.insert(b + 1, after);
        }
    }
    let small = || PyValue::Float(SMALL_FONT_SIZE);
    for i in 0..chapters.len() {
        if Some(i) == nav {
            chapters[i].set("page_bidi", bidi.clone());
            rebuild_contents_page(chapters, i)?;
            let ch = &mut chapters[i];
            ch.remove("blocks");
            ch.set("font_size", small());
            ch.set("title", s(CONTENTS_PAGE_TITLE));
            if truthy(ch.get("_from_spine")) {
                ch.remove("_omit_title_heading");
            }
            continue;
        }
        if truthy(chapters[i].get("_from_spine")) {
            continue;
        }
        let t = normalize_title(title_of(&chapters[i]));
        // `TITLE_PAGE_TITLES | HALF_TITLE_TITLES`.
        if CONTENTS_SKIP_TITLES[..9].contains(&t.as_str()) {
            chapters[i].set("_omit_title_heading", PyValue::Bool(true));
        } else if Some(i) == book && !listings_kept {
            chapters[i].set("page_bidi", bidi.clone());
            rebuild_contents_page(chapters, i)?;
            chapters[i].remove("blocks");
            chapters[i].set("font_size", small());
        }
        if SMALL_TEXT_CHAPTERS.contains(&t.as_str()) {
            chapters[i].set("font_size", small());
        }
    }
    Ok(())
}

/// `ch["title"]`.
fn title_of(ch: &Chapter) -> &str {
    ch.get("title").and_then(PyValue::as_str).unwrap_or("")
}

/// `b.get("text") or ""`.
fn text_of(b: &Block) -> &str {
    b.get("text").and_then(PyValue::as_str).unwrap_or("")
}

/// Titled as the book's contents.
fn is_contents(ch: &Chapter) -> bool {
    CONTENTS_TITLES.contains(&normalize_title(title_of(ch)).as_str())
}

/// A chapter titled "Contents" (or "Table of Contents") that is not a file's.
fn has_titled_contents(chapters: &[Chapter]) -> bool {
    chapters
        .iter()
        .any(|c| is_contents(c) && !truthy(c.get("_from_spine")))
}

/// Text once the pictures are taken out: `bool(_IMG_TOKEN_RE.sub("", text).strip())`.
fn shows_text(text: &str) -> bool {
    !py::strip(&IMG_TOKEN.replace_all(text, "")).is_empty()
}

/// `_nav_listing_contents_chapter`: the place of the chapter whose dropped listing becomes the
/// contents page, the first flagged one whose text without pictures is title-sized and that
/// holds at most one block of text; none when a chapter is titled as the contents.
fn nav_listing_contents_chapter(chapters: &[Chapter]) -> Option<usize> {
    if has_titled_contents(chapters) {
        return None;
    }
    chapters.iter().position(|ch| {
        let text = ch.get("text").and_then(PyValue::as_str).unwrap_or("");
        truthy(ch.get("_had_nav_listing"))
            && py::len(py::strip(&IMG_TOKEN.replace_all(text, ""))) <= NAV_REMNANT_MAX_LEN
            && count_text_blocks(ch.blocks()) <= 1
    })
}

/// A block, or a paragraph or block held as a model value (in a table cell, or in a box
/// there).
#[derive(Clone, Copy)]
enum Part<'b> {
    Block(&'b Block),
    Py(&'b PyValue),
}

/// `_count_text_blocks`: the blocks with text, counting each paragraph inside a box and in a
/// native table's cells, however deep (a stack, not recursion).
fn count_text_blocks<'b>(blocks: impl IntoIterator<Item = &'b Block>) -> usize {
    let mut stack: Vec<Part> = blocks.into_iter().map(Part::Block).collect();
    let mut n = 0;
    while let Some(part) = stack.pop() {
        let get = |k: &str| match part {
            Part::Block(b) => b.get(k),
            Part::Py(v) => v.get(k),
        };
        match get("type").and_then(PyValue::as_str) {
            Some("box") => match part {
                Part::Block(b) => stack.extend(b.blocks().unwrap_or(&[]).iter().map(Part::Block)),
                Part::Py(v) => stack.extend(list(v.get("blocks")).iter().map(Part::Py)),
            },
            Some("table") => {
                let rows = get("table").and_then(|t| t.get("rows"));
                for row in list(rows) {
                    for cell in list(row.get("cells")) {
                        match cell.get("paragraphs") {
                            Some(p) if p.is_truthy() => {
                                stack.extend(list(Some(p)).iter().map(Part::Py))
                            }
                            _ => stack.push(Part::Py(cell)),
                        }
                    }
                }
            }
            _ => {
                n += usize::from(shows_text(
                    get("text").and_then(PyValue::as_str).unwrap_or(""),
                ))
            }
        }
    }
    n
}

/// A list's items; none for what is no list.
fn list(v: Option<&PyValue>) -> &[PyValue] {
    v.and_then(PyValue::as_list).unwrap_or(&[])
}

/// `ch["_nav_listing_at"]` as a place in its blocks (a slice's end, clamped as Python's are).
fn listing_at(ch: &Chapter) -> Option<usize> {
    let at = ch.get("_nav_listing_at")?.as_int()?;
    Some(usize::try_from(at).unwrap_or(0).min(ch.blocks().len()))
}

/// `_split_text_above_listing`: the blocks above the page's listing, as a chapter of their own
/// with the page's keys (and its contents entry), when the first of them with text is not a
/// listing's own heading. The page keeps the rest and loses its places after it.
fn split_text_above_listing(ch: &mut Chapter) -> Option<Chapter> {
    if !truthy(ch.get("_nav_listing_at")) {
        return None;
    }
    let at = listing_at(ch)?;
    let shown = ch.blocks()[..at]
        .iter()
        .find(|b| !py::strip(text_of(b)).is_empty())?;
    if LISTING_HEADINGS.contains(&normalize_title(text_of(shown)).as_str()) {
        return None;
    }
    let mut above = ch.take_blocks();
    let rest = above.split_off(at);
    let mut fields: Vec<(&'static str, Field)> = ch
        .fields
        .iter()
        .filter(|(k, _)| {
            !matches!(
                *k,
                "blocks" | "text" | "_had_nav_listing" | "_nav_listing_at"
            )
        })
        .filter_map(|(k, v)| match v {
            Field::Py(p) => Some((*k, Field::Py(p.clone()))),
            Field::Blocks(_) => None,
        })
        .collect();
    let text = joined_text(&above);
    fields.push(("blocks", Field::Blocks(above)));
    fields.push(("text", Field::Py(PyValue::Str(text))));
    ch.set("text", s(&joined_text(&rest)));
    ch.put_blocks(rest);
    ch.remove("sub_entries");
    Some(Chapter { fields })
}

/// A chapter kfxgen splits off the contents page to print after it: its own label, never
/// printed or listed, at the page's level.
fn split_off(page: &Chapter, kept: Vec<Block>) -> Chapter {
    let level = page.get("toc_level").cloned().unwrap_or(PyValue::Int(0));
    Chapter::new(
        LEADING_TITLE_FALLBACK.to_owned(),
        joined_text(&kept),
        vec![
            ("blocks", Field::Blocks(kept)),
            ("toc_level", Field::Py(level)),
            ("_omit_from_toc", Field::Py(PyValue::Bool(true))),
            ("_omit_title_heading", Field::Py(PyValue::Bool(true))),
        ],
    )
}

/// `_split_files_after_contents`: the page's blocks from the first of another file on, when
/// they hold text.
fn split_files_after_contents(ch: &mut Chapter) -> Option<Chapter> {
    let blocks = ch.blocks();
    let own = blocks
        .first()?
        .get("_spine_file")
        .filter(|v| **v != PyValue::None)?;
    let cut = blocks.iter().position(|b| {
        b.get("_spine_file")
            .is_some_and(|f| *f != PyValue::None && f != own)
    })?;
    if count_text_blocks(&blocks[cut..]) == 0 {
        return None;
    }
    let mut blocks = ch.take_blocks();
    let kept = blocks.split_off(cut);
    ch.set("text", s(&joined_text(&blocks)));
    ch.put_blocks(blocks);
    Some(split_off(ch, kept))
}

/// `_split_front_matter_below_listing`: the blocks below the page's listing the book marks as
/// front matter, when they hold text.
fn split_front_matter_below_listing(ch: &mut Chapter) -> Option<Chapter> {
    if ch
        .get("_nav_listing_at")
        .is_none_or(|v| *v == PyValue::None)
    {
        return None;
    }
    let at = listing_at(ch)?;
    let front = |b: &&Block| truthy(b.get("front_matter"));
    if count_text_blocks(ch.blocks()[at..].iter().filter(front)) == 0 {
        return None;
    }
    let mut left = ch.take_blocks();
    let below = left.split_off(at);
    let (kept, rest): (Vec<Block>, Vec<Block>) = below.into_iter().partition(|b| front(&b));
    left.extend(rest);
    ch.set("text", s(&joined_text(&left)));
    ch.put_blocks(left);
    Some(split_off(ch, kept))
}

/// One line of the rebuilt page before it is a dict: an entry's title, the chapter it opens,
/// the key of a place in that chapter (`target_key`, for a place after its start) and its
/// depth.
struct Line {
    text: String,
    chapter: usize,
    key: Option<PyValue>,
    level: i64,
}

/// A line of the page, in page order: a link (the entry it is, if any), a heading's line, or a
/// heading repeating entry `at`'s line with its own level and style (made once counted).
enum Event {
    Link(Option<usize>),
    Heading(Vec<(PyValue, PyValue)>),
    Repeat {
        at: usize,
        level: PyValue,
        style: PyValue,
    },
}

/// `_rebuild_contents_page(chapters[c], chapters)`: the page's text becomes a linked line for
/// each chapter and each place the contents name (but itself, title pages and kfxgen's own
/// labels), indented from the shallowest; links into its dropped blocks land on it; its other
/// headings print where they are; its pictures are kept (`_keep_illustrations`).
fn rebuild_contents_page(chapters: &mut [Chapter], c: usize) -> Result<(), Error> {
    let mut lines: Vec<Line> = Vec::new();
    for (i, ch) in chapters.iter().enumerate() {
        if i == c {
            continue;
        }
        let title = title_of(ch);
        let lower = normalize_title(title);
        let skip_own =
            CONTENTS_SKIP_TITLES.contains(&lower.as_str()) || py::strip(title).is_empty();
        if skip_own && !CONTENTS_TITLES.contains(&lower.as_str()) {
            continue;
        }
        if truthy(ch.get("_omit_from_toc")) {
            continue;
        }
        let level = |v: Option<&PyValue>| v.and_then(PyValue::as_int).unwrap_or(0);
        if !skip_own {
            lines.push(Line {
                text: title.to_owned(),
                chapter: i,
                key: None,
                level: level(ch.get("toc_level")),
            });
        }
        for sub in list(ch.get("sub_entries")) {
            let text = sub.get("title").and_then(PyValue::as_str).unwrap_or("");
            if py::strip(text).is_empty() {
                continue;
            }
            lines.push(Line {
                text: text.to_owned(),
                chapter: i,
                key: Some(sub.get("key").cloned().unwrap_or(PyValue::None)),
                level: level(sub.get("level")),
            });
        }
    }
    let top = lines.iter().map(|l| l.level).min().unwrap_or(0);

    // `_keep_illustrations`: the page's pictures, as tokens.
    let preserved: Vec<PyValue> = chapters[c]
        .blocks()
        .iter()
        .flat_map(|b| IMG_TOKEN.find_iter(text_of(b)).map(|m| s(m.as_str())))
        .collect();

    // Links into the page land on it: its listing's keys come off every block.
    let moved = match chapters[c].remove("_listing_keys") {
        Some(Field::Py(v)) => list(Some(&v))
            .iter()
            .filter_map(|k| k.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    if !moved.is_empty() {
        let keys: HashSet<String> = moved.iter().cloned().collect();
        for ch in chapters.iter_mut() {
            let mut blocks = ch.take_blocks();
            drop_keys(&mut blocks, &keys);
            ch.put_blocks(blocks);
        }
    }
    let page = &chapters[c];
    let mut keys: Vec<String> = page
        .blocks()
        .iter()
        .flat_map(|b| list(b.get("anchor_keys")))
        .filter_map(|k| k.as_str().map(str::to_owned))
        .collect();
    keys.extend(moved);
    let anchor_keys = strs(&dedupe(keys));
    let text = std::iter::once(CONTENTS_PAGE_TITLE)
        .chain(lines.iter().map(|l| l.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n\n");

    let blocks = page.blocks();
    let first = blocks
        .iter()
        .position(|b| !py::strip(text_of(b)).is_empty());
    let first_level = first
        .map(|f| &blocks[f])
        .and_then(|b| b.get("heading_level"))
        .filter(|v| v.is_truthy())
        .cloned();
    let listed: Vec<String> = lines.iter().map(|l| normalize_title(&l.text)).collect();
    let mut by_words: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, w) in listed.iter().enumerate() {
        by_words.entry(w.as_str()).or_default().push(i);
    }
    // `entry(words, after)`: the first entry of `words` past entry `after`, else the first.
    let entry = |words: &str, after: Option<usize>| -> Option<usize> {
        let hits = by_words.get(words)?;
        let past = hits.partition_point(|&i| after.is_some_and(|a| i <= a));
        Some(*hits.get(past).unwrap_or(&hits[0]))
    };
    let mut links: Vec<Vec<(PyValue, PyValue)>> = lines
        .iter()
        .map(|l| {
            let mut d = vec![
                (s("text"), s(&l.text)),
                (s("target_chapter_idx"), PyValue::Int(l.chapter as i64)),
            ];
            if let Some(key) = &l.key {
                d.push((s("target_key"), key.clone()));
            }
            d.push((s("level"), PyValue::Int(l.level - top)));
            d
        })
        .collect();

    // How many of the page's links each entry is.
    let mut links_to = vec![0usize; listed.len()];
    let mut last = None;
    for (n, b) in blocks.iter().enumerate() {
        if Some(n) == first || !is_link(b) {
            continue;
        }
        if let Some(at) = entry(&normalize_title(text_of(b)), last) {
            last = Some(at);
            links_to[at] += 1;
        }
    }

    let mut events: Vec<Event> = Vec::new();
    // The bytes of entry titles and keys the repeated lines copy, counted before any is made.
    let mut repeated = 0u64;
    let mut last = None;
    for (n, b) in blocks.iter().enumerate() {
        if Some(n) == first {
            continue;
        }
        let words = normalize_title(text_of(b));
        let link = is_link(b);
        let level = b.get("heading_level").filter(|v| v.is_truthy());
        let at = level.and_then(|_| entry(&words, last));
        let style = || b.get("block_style").cloned().unwrap_or(PyValue::None);
        let mut merged = false;
        if let (Some(at), Some(level)) = (at, level) {
            if links_to[at] <= usize::from(link) {
                // A heading the contents list that no other line links: it is that entry's
                // line, printed as the heading.
                dict_set(&mut links[at], "heading_level", level.clone());
                dict_set(&mut links[at], "block_style", style());
                merged = true;
            } else if link {
                // A heading that is a link the page repeats: printed where it is, the entry
                // keeping its own line. Merged lines are never repeated (`links_to` is at most
                // one for those, at least two here), so the line copied is the same now or
                // once all are counted.
                // The line's strings: its title and a place's key.
                let key = lines[at].key.as_ref().and_then(PyValue::as_str);
                let copied = lines[at].text.len() + key.map_or(0, str::len);
                repeated = repeated.saturating_add(copied as u64);
                events.push(Event::Repeat {
                    at,
                    level: level.clone(),
                    style: style(),
                });
                continue;
            }
        }
        if link {
            let at = entry(&words, last);
            if at.is_some() {
                last = at;
            }
            events.push(Event::Link(at));
        } else if let Some(level) = level.filter(|_| !words.is_empty() && !merged) {
            events.push(Event::Heading(vec![
                (s("text"), s(py::strip(text_of(b)))),
                (s("heading_level"), level.clone()),
                (s("block_style"), style()),
            ]));
        }
    }
    // A heading matches a title once the title's edge punctuation is stripped, so a short one
    // can repeat a long title, or a place's long key (DIVERGENCES, "Headings a contents page repeats").
    if repeated > MAX_CONTENT_FILE_BYTES {
        return Err(Error::TooLarge {
            what: format!(
                "the contents page repeats more than {MAX_CONTENT_FILE_BYTES} bytes of contents \
                 entry titles and keys"
            ),
        });
    }
    // Each heading's line goes before the entry of the next link with one (the end if none),
    // two at one place in page order.
    let mut before: Vec<Vec<Vec<(PyValue, PyValue)>>> = vec![Vec::new(); links.len() + 1];
    let mut next = None;
    let mut placed = Vec::new();
    for event in events.into_iter().rev() {
        match event {
            Event::Link(at) => next = at.or(next),
            heading => placed.push((next.unwrap_or(links.len()), heading)),
        }
    }
    for (place, heading) in placed.into_iter().rev() {
        let line = match heading {
            Event::Repeat { at, level, style } => {
                let mut line = links[at].clone();
                dict_set(&mut line, "heading_level", level);
                dict_set(&mut line, "block_style", style);
                line
            }
            Event::Heading(line) => line,
            Event::Link(_) => unreachable!("links are not placed"),
        };
        before[place].push(line);
    }
    let mut toc_links =
        Vec::with_capacity(links.len() + before.iter().map(Vec::len).sum::<usize>());
    let mut links = links.into_iter();
    for lines in before {
        toc_links.extend(lines.into_iter().map(PyValue::Dict));
        toc_links.extend(links.next().map(PyValue::Dict));
    }

    let page = &mut chapters[c];
    page.set("anchor_keys", anchor_keys);
    page.set("text", PyValue::Str(text));
    if let Some(level) = first_level {
        page.set("heading_level", level);
    }
    page.remove("blocks");
    page.set("toc_links", PyValue::List(toc_links));
    if !preserved.is_empty() {
        page.set("preserved_images", PyValue::List(preserved));
    }
    Ok(())
}

/// `is_link(b)`: a run of the block is a link.
fn is_link(b: &Block) -> bool {
    list(b.get("spans")).iter().any(|span| {
        let flags = match span.as_list() {
            Some([_, _, PyValue::Set(f) | PyValue::List(f) | PyValue::Tuple(f)]) => f.as_slice(),
            _ => &[],
        };
        flags
            .iter()
            .any(|f| matches!(f, PyValue::Tuple(t) if t.first().and_then(PyValue::as_str) == Some(LINK_FLAG)))
    })
}
