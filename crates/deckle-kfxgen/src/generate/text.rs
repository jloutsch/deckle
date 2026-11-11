// SPDX-License-Identifier: GPL-3.0-or-later
//! A book's text: `_build_chapter_content` (`native_generator.py:5396`), the chapter loop that
//! makes chunks of the chapters' titles, paragraphs and contents lines, their positions, and
//! the `$145`, `$157` and `$259` fragments they fill. The pieces are `chapters`' (reading),
//! `title`, `chunks`, `styles` and `fragments`; runs, links and anchors are `inline`'s,
//! pictures `images`'.
use std::collections::{BTreeMap, BTreeSet};

use super::chapters::{Block, Chapter, cut_at_empty_line};
use super::chunks::{Chunk, emit_paragraph, print_book_heading};
use super::fragments::{
    ContentRef, build_fragment_145, build_fragment_259, pack_content_fragments, style_refs,
};
use super::images::{self, ImageStyles, Images, Picture};
use super::inline::{self, Anchors, ChunkLinks};
use super::styles::{Style157, Styles};
use super::title::{
    MAX_SPLIT_TITLE_BLOCKS, consume_split_title, label_blocks, opens_with_title, title_runs_in,
};
use super::{GenerateError, Generator, py};

use GenerateError::Unsupported;

/// `NativeKFXGenerator`'s position ids (:5259).
const CONTENT_POS_BASE: i64 = 1000;
const CONTENT_POS_STEP: i64 = 2;
const SECTION_POS_BASE: i64 = 10000;
const SECTION_POS_STEP: i64 = 2;
const HEADING_FONT_SIZE: f64 = 1.2;

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
        images::push_preserved(&chapter.preserved_images, images, &mut chunks)?;

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
