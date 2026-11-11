// SPDX-License-Identifier: GPL-3.0-or-later
//! A book's text: `_build_chapter_content` (`native_generator.py:5396`), the chapter loop that
//! makes chunks of the chapters' titles, paragraphs and contents lines, their positions, and
//! the `$145`, `$157` and `$259` fragments they fill. The pieces are `chapters`' (reading),
//! `title`, `chunks`, `styles`, `block_styles` and `fragments`; runs, links and anchors are
//! `inline`'s, pictures `images`', boxes and rules `boxes`', pop-up notes `notes`'.
use std::collections::{BTreeMap, BTreeSet};

use deckle_kfx::fragment::Fragment;

use super::block_styles::{
    BlockStyle, Containers, HeadingStyle, Sizes, heading_attrs, heading_style, text_entry_style,
};
use super::boxes::{self, drop_edge_rules, emit_box_chunks, rule_chunk, unboxed};
use super::chapters::{Block, Chapter};
use super::chunks::{Chunk, Marker, Node, emit_paragraph, print_book_heading};
use super::fragments::{
    ContentRef, build_fragment_145, build_fragment_259, pack_content_fragments, style_refs,
};
use super::images::{self, ImageStyles, Images, Picture};
use super::inline::{self, ChunkLinks, RunBase};
use super::inline_picture_entries::{self, InlineStyles};
use super::notes::{self, Popups};
use super::styles::{Style157, StyleArgs, Styles};
use super::table_chunks;
use super::title::{
    eat_split_title, heading_near_start, label_blocks, opens_with_title, title_in_table,
    title_runs_in,
};
use super::writing;
use super::{GenerateError, Generator, py};

use GenerateError::Unsupported;

/// `NativeKFXGenerator`'s position ids (:5259).
const CONTENT_POS_BASE: i64 = 1000;
const CONTENT_POS_STEP: i64 = 2;
const SECTION_POS_BASE: i64 = 10000;
const SECTION_POS_STEP: i64 = 2;

/// What `_build_chapter_content` returns.
pub(crate) struct ChapterData {
    pub chunks: Vec<Chunk>,
    /// None for a container's `close`, which takes no position (#219).
    pub chunk_positions: Vec<Option<i64>>,
    pub chapter_chunk_ranges: Vec<(usize, usize)>,
    pub section_positions: Vec<i64>,
    pub chapter_start_positions: Vec<i64>,
    pub story_names: Vec<String>,
    pub storyline_names: Vec<String>,
    pub content_names: Vec<String>,
    pub extra_style_names: Vec<String>,
    /// The `$266` anchors, for the entity index.
    pub anchor_names: Vec<String>,
    /// The chapters' ruby fragments (#236).
    pub ruby_names: Vec<String>,
}

/// `_build_chapter_content` (:5396).
pub(crate) fn build_chapter_content(
    g: &mut Generator,
    chapters: &[Chapter],
    images: &Images,
    footnote_keys: &BTreeSet<String>,
    vertical_book: bool,
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
        let all: Vec<&Block> = blocks.iter().collect();
        let toc = !chapter.toc_links.is_empty();
        // A heading whose label runs its readings in (#236), else `_book_heading_title`
        // (:1304) decides.
        let mut title = chapter.title.clone();
        let mut title_spans = None;
        let matched = label_blocks(&all, &title);
        let first_block = drop_edge_rules(blocks).first();
        let book_block = if matched == 1 {
            first_block
        } else if matched == 0 {
            // Not a heading holding a picture, which kfxgen's heading can't print.
            first_block.filter(|b| b.heading && !images::has_picture(&b.text))
        } else {
            None
        };
        if let Some((base, spans)) = writing::ruby_heading_title(first_block, &title) {
            title = base;
            title_spans = Some(spans);
        } else if let Some(b) = book_block {
            title = py::strip(&b.text).to_owned();
            title_spans = Some(inline::title_spans(&b.text, &b.spans, py::len(&title)));
        }

        // A body of pictures alone after the title prints no heading of kfxgen's on top of
        // them, nor one under pictures above the book's own heading (#33, #346, :6258).
        let stripped = py::lstrip(&chapter.text);
        let body_after_title = if opens_with_title(stripped, &title) {
            py::lstrip(py::skip(stripped, py::len(&title)))
        } else {
            stripped
        };
        let image_only = !toc && images::only_pictures(body_after_title);
        // Rules are passed over and boxes opened; a table says no (:1148).
        let title_after_pictures = !toc
            && images::title_after_pictures(
                unboxed(blocks)
                    .into_iter()
                    .filter(|b| b.rule.is_none())
                    .map_while(|b| b.table.is_none().then_some(b.text.as_str())),
                &title,
            );
        let runs_in = !toc && title_runs_in(blocks, &title);
        let in_table = !toc && title_in_table(blocks, &title);
        let heading_near_start =
            !toc && title_spans.is_none() && heading_near_start(blocks, &title);
        let label_over_blocks = !toc && label_blocks(&unboxed(blocks), &title) > 1;
        let omit_heading = chapter.is_cover
            || py::strip(&title).is_empty()
            || chapter.omit_title_heading
            || image_only
            || title_after_pictures
            || runs_in
            || in_table
            || heading_near_start
            || label_over_blocks;
        let mut heading_chunk = None;
        // The page's direction and language, for kfxgen's lines with no block of the book's
        // to take them from: its heading, a rebuilt contents page's lines (#714, :6307).
        let page_bidi = chapter.page_bidi.clone().filter(|b| b.set).or_else(|| {
            writing::page_bidi(blocks.iter().flat_map(|b| {
                let boxed = b.boxed.as_ref().and_then(|x| x.spec.bidi.as_ref());
                let table = b.table.as_ref().and_then(|t| t.bidi.as_ref());
                [b.style.as_ref().and_then(|s| s.bidi.as_ref()), boxed, table]
            }))
        });
        if !omit_heading {
            heading_chunk = Some(chunks.len());
            chunks.push(Chunk {
                heading_level: chapter.heading_level,
                spans: title_spans.unwrap_or_default(),
                heading: HeadingStyle {
                    page_bidi: page_bidi.clone(),
                    ..HeadingStyle::default()
                },
                ..Chunk::text(title.clone())
            });
        }

        if toc {
            for link in &chapter.toc_links {
                toc_link_chunks.insert(chunks.len(), (link.target, link.level));
                chunks.push(Chunk {
                    style: page_bidi.clone().map(|bidi| BlockStyle {
                        bidi: Some(bidi),
                        ..BlockStyle::default()
                    }),
                    ..Chunk::text(link.text.clone())
                });
            }
        } else {
            let cut_title = !chapter.omit_title_heading
                && !image_only
                && !runs_in
                && !in_table
                && !heading_near_start
                && !label_over_blocks;
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
                        let mut iter_blocks = drop_edge_rules(blocks).to_vec();
                        // An opening table printing the title is kept whole; one that
                        // doesn't has none to cut (:6382).
                        if !iter_blocks.is_empty() && cut_title && iter_blocks[0].table.is_none() {
                            let first_stripped = py::lstrip(&iter_blocks[0].text).to_owned();
                            if iter_blocks[0].boxed.is_some() {
                                // A box is written from its blocks: a title split over its
                                // lines is eaten from them (#344).
                                if !boxes::cut_title_in_box(
                                    &mut iter_blocks,
                                    &title,
                                    &mut chunks,
                                    &mut heading_chunk,
                                    &mut carried,
                                ) && let Some((rest, keys)) =
                                    eat_split_title(&iter_blocks, &title)
                                {
                                    iter_blocks = rest;
                                    carried.extend(keys);
                                }
                            } else if opens_with_title(&first_stripped, &title) {
                                let remainder = py::lstrip(py::skip(&first_stripped, n)).to_owned();
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
                                // Split over blocks or a table's rows (#64, #219). The book's
                                // heading prints: its style (#490).
                                let first_style = iter_blocks[0].style.clone();
                                if let Some((rest, keys)) = eat_split_title(&iter_blocks, &title) {
                                    if let Some(h) = heading_chunk {
                                        chunks[h].heading.book_style = first_style;
                                    }
                                    iter_blocks = rest;
                                    carried.extend(keys);
                                }
                            }
                        }
                        iter_blocks
                    }
                    None => text
                        .split("\n\n")
                        .map(|p| Block::plain(p.to_owned()))
                        .collect(),
                };
                // Ids of a paragraph that emitted nothing, only pictures the book doesn't
                // hold, wait for the next block's first chunk (#291), else go on the
                // chapter's last, else its start (:6483).
                let mut pending: Vec<String> = Vec::new();
                for (b_idx, block) in para_iter.iter().enumerate() {
                    let first = chunks.len();
                    let unplaced = if let Some(t) = &block.table {
                        table_chunks::emit_table_chunks(block, t, &mut chunks);
                        Vec::new()
                    } else if let Some(rule) = &block.rule {
                        chunks.push(rule_chunk(rule));
                        block.anchors.keys.clone()
                    } else if let Some(bx) = &block.boxed {
                        emit_box_chunks(block, bx, images, &mut chunks)?
                    } else {
                        emit_paragraph(block, b_idx == 0, images, &mut chunks)?
                    };
                    // `_emitted_content` (:5940): a table of rows with no cells, a box of
                    // paragraphs that emitted nothing, is only its markers.
                    if !pending.is_empty() && chunks[first..].iter().any(Chunk::is_content) {
                        let at = table_chunks::link_target(&chunks, first);
                        chunks[at].anchors.attach(&pending);
                        pending.clear();
                    }
                    pending.extend(unplaced);
                }
                if !pending.is_empty() {
                    match (start_idx..chunks.len())
                        .rev()
                        .find(|&i| chunks[i].is_leaf())
                    {
                        Some(last) => chunks[last].anchors.attach(&pending),
                        None => carried.extend(pending),
                    }
                }
            } else if !blocks.is_empty() {
                // Only its title: the blocks' ids still name this chapter (#338), and the
                // book's heading prints as kfxgen's, the rules under it with it (:6522, #717).
                for b in blocks {
                    carried.extend(boxes::block_keys(b));
                }
                if let (Some(h), Some(first)) = (heading_chunk, first_block)
                    && py::strip(&first.text) == title
                {
                    // A box holding the heading and only rules is drawn round it (#646).
                    if first.boxed.as_deref().is_some_and(|bx| {
                        bx.blocks[0].boxed.is_none()
                            && bx.blocks[1..].iter().all(|b| b.rule.is_some())
                    }) {
                        return Err(Unsupported("a box round a chapter's title alone"));
                    }
                    heading_offsets = inline::heading_offsets(&first.text, &first.anchors, &title);
                    print_book_heading(&mut chunks[h], first, &title);
                    let at = blocks
                        .iter()
                        .position(|b| std::ptr::eq(b, first))
                        .expect("first is one of the blocks");
                    for b in &blocks[at + 1..] {
                        if let Some(rule) = &b.rule {
                            chunks.push(rule_chunk(rule));
                        }
                    }
                }
            }
        }
        // Pictures of a contents page the converter replaced, after it (#117).
        images::push_preserved(&chapter.preserved_images, images, &mut chunks)?;

        // Every chapter has an entry to land on (:6597): a no-break space. Containers and
        // rules don't count.
        if !chunks[start_idx..].iter().any(Chunk::is_leaf) {
            chunks.push(Chunk::text("\u{a0}".to_owned()));
        }
        if !carried.is_empty() {
            // A table's first row, a box's first entry, not the container (#219, #238); the
            // offsets of the block kfxgen's heading prints, else the chapter's start (:6622).
            let at = table_chunks::link_target(&chunks, start_idx);
            let offsets = if Some(at) == heading_chunk {
                heading_offsets
            } else {
                BTreeMap::new()
            };
            chunks[at].anchors.attach_at(&carried, &offsets);
        }
        if let Some(h) = heading_chunk {
            heading_chunks.insert(h);
        }
        chapter_chunk_ranges.push((start_idx, chunks.len()));
    }

    // Each chapter: an outer position, then one per chunk but a `close` (:6638).
    let mut chunk_positions = Vec::with_capacity(chunks.len());
    let mut content_pos_id = CONTENT_POS_BASE;
    for &(start, end) in &chapter_chunk_ranges {
        content_pos_id += CONTENT_POS_STEP;
        for chunk in &chunks[start..end] {
            if matches!(chunk.marker, Some(Marker::Close)) {
                chunk_positions.push(None);
                continue;
            }
            chunk_positions.push(Some(content_pos_id));
            content_pos_id += CONTENT_POS_STEP;
        }
    }
    let ruby_eids = writing::ruby_eids(
        chunks.iter().map(|c| &c.spans[..]),
        &mut content_pos_id,
        CONTENT_POS_STEP,
    );
    writing::number_tcy(&mut chunks, &mut content_pos_id, CONTENT_POS_STEP);
    let sizes = Sizes::of(&chunks);
    for (ch_idx, &(start, end)) in chapter_chunk_ranges.iter().enumerate() {
        let fs = chapters[ch_idx].font_size;
        inline_picture_entries::number(
            &mut chunks[start..end],
            |c| sizes.block_font_size(fs, c),
            &mut content_pos_id,
            CONTENT_POS_STEP,
        );
    }
    // `_section_base` (:5267): above the content's positions.
    let content_max = content_pos_id - CONTENT_POS_STEP;
    let section_base = SECTION_POS_BASE.max(content_max + SECTION_POS_STEP);
    let section_positions = (0..chapters.len() as i64)
        .map(|i| section_base + i * SECTION_POS_STEP)
        .collect();
    // `_first_leaf_position` (:6713): a contents entry lands on a leaf, never a container.
    let chapter_start_positions: Vec<i64> = chapter_chunk_ranges
        .iter()
        .map(|&(start, end)| {
            (start..end)
                .find(|&i| chunks[i].is_leaf())
                .and_then(|i| chunk_positions[i])
                .expect("every chapter has an entry to land on")
        })
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
            // Only a `close` has none, and it holds no ids or runs.
            position: position.unwrap_or_default(),
            in_cell: c.in_cell.is_some(),
        })
        .collect();
    let mut popups = Popups {
        targets: notes::popup_targets(chunks.iter().flat_map(|c| &c.spans), footnote_keys),
        ..Popups::default()
    };
    let body_anchor_map = inline::build_body_anchors(g, &links, &mut anchor_names, &mut popups);

    // $145: each chapter's text in fragments under the cap (:6863); a chapter of pictures
    // alone, a cover's, has none (#102).
    let mut content_names = Vec::new();
    let mut content_refs_per_chapter = Vec::new();
    let mut next_content_index = 1;
    for &(start, end) in &chapter_chunk_ranges {
        let text_chunks: Vec<usize> = (start..end)
            // A paragraph holding pictures holds its words in its entry (#567).
            .filter(|&i| {
                chunks[i].is_leaf() && chunks[i].picture.is_none() && chunks[i].pictures.is_empty()
            })
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
            let name = styles.allocate(g, "_h", heading_attrs(fs, i == 0));
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
    let mut inline_styles = InlineStyles::new(images);

    // $259: a storyline per chapter (:7359).
    let mut storyline_names = Vec::new();
    // Each chapter's readings, (id, reading): one ruby fragment per storyline (:6694).
    let mut ruby_readings: Vec<(usize, Vec<(i64, String)>)> = Vec::new();
    for (ch_idx, &(start, end)) in chapter_chunk_ranges.iter().enumerate() {
        let sl_name = format!("l{ch_idx}");
        let mut readings = Vec::new();
        // A vertical-rl chapter's styles have their margins moved (#247, :7383).
        styles.vertical = match chapters[ch_idx].writing_mode.as_deref() {
            Some(mode) => mode == "vertical-rl",
            None => vertical_book,
        };
        let fs = chapters[ch_idx].font_size;
        let mut entry_styles = Vec::new();
        // A container's own `$157`: a row's or group's border (:7556), a box's (:7520).
        let mut entry_parts = Vec::new();
        let mut entry_runs = Vec::new();
        let mut open = Containers::default();
        for chunk_idx in start..end {
            let chunk = &chunks[chunk_idx];
            let around = open.around().cloned();
            let around = around.as_ref();
            if let Some(marker) = &chunk.marker {
                let (style, part) =
                    open.marker_styles(g, &mut styles, &sizes, chunk, marker, fs)?;
                // A `close`, a box, or a row or group drawing no border, takes the
                // chapter's, unused.
                entry_styles.push(style.unwrap_or_else(|| story_names[ch_idx].clone()));
                entry_parts.push(part);
                // A linked picture's container: its link's anchor (:7495).
                let link = match marker {
                    Marker::Open(Node::Link(target)) => body_anchor_map
                        .get(target)
                        .map(|a| (a.clone(), String::new(), 1)),
                    _ => None,
                };
                entry_runs.push((link, Vec::new()));
                continue;
            }
            entry_parts.push(None);
            if let Some(rule) = &chunk.rule {
                entry_styles.push(styles.allocate(g, "_r", StyleArgs::Rule(rule.clone())));
                entry_runs.push((None, Vec::new()));
                continue;
            }
            if let Some(p) = &chunk.picture {
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
                (Some(name), _) if heading => {
                    let attrs = heading_attrs(fs, ch_idx == 0);
                    heading_style(g, &mut styles, &sizes, chunk, &attrs, around)
                        .unwrap_or_else(|| name.clone())
                }
                (_, Some(anchor)) => {
                    // A contents line: the page's style, its link underlined (:7707).
                    let (_, level) = toc_link_chunks[&chunk_idx];
                    link = Some((
                        anchor.clone(),
                        toc_link_style_names[ch_idx].clone().unwrap_or_default(),
                        py::len(&chunk.text) as i64,
                    ));
                    // The page's direction and language (#714).
                    let bidi = writing::bidi_diff(
                        chunk.style.as_ref().and_then(|b| b.bidi.as_ref()),
                        around,
                    );
                    if level != 0 || !bidi.is_empty() {
                        // Indented by depth, 6.25% a level (#238, #313).
                        let indent = Style157 {
                            margin_left: (level != 0)
                                .then(|| (py::format_g(6.25 * level as f64, 6), "$314".into())),
                            ..Style157::body(fs)
                        };
                        styles.allocate_in(g, "", indent, &bidi)
                    } else {
                        story_names[ch_idx].clone()
                    }
                }
                _ => text_entry_style(g, &mut styles, &sizes, chunk, fs, &open.sizes, around),
            };
            entry_styles.push(style);
            // kfxgen's chapter heading is bold, and so are the runs in it (#366), unless it
            // prints the book's heading in that heading's weight (#490); a header cell's are
            // (#219).
            let bs = chunk.style.as_ref();
            let book_bold = chunk.heading.book_style.as_ref().is_none_or(|b| b.bold);
            // The book's heading printing as kfxgen's: its words in sized spans print at
            // their size against its own (#721). kfxgen's own heading keeps its size (#241).
            let heading_book = chunk.heading.book_style.as_ref().filter(|_| heading);
            let base = RunBase {
                bold: bs.is_some_and(|b| b.bold) || chunk.cell_bold() || (heading && book_bold),
                italic: bs.is_some_and(|b| b.italic),
                family: bs.map_or(&[], |b| &b.family),
                in_cell: chunk.in_cell.is_some(),
                blk_fs: match heading_book {
                    Some(book) => sizes.rem(book.font_size),
                    None => sizes.block_font_size(fs, chunk),
                },
                sized: !heading || heading_book.is_some(),
            };
            let mut runs = inline::entry_runs(
                g,
                &mut styles,
                &mut extra_style_names,
                &sizes,
                &chunk.spans,
                &base,
                &body_anchor_map,
                &popups,
            );
            writing::place_rubies(
                &chunk.spans,
                &mut runs,
                &ruby_eids,
                chunk_idx,
                ch_idx,
                &mut readings,
            );
            entry_runs.push((link, runs));
        }
        let fragment = build_fragment_259(
            g,
            &entry_styles,
            &entry_parts,
            &content_refs_per_chapter[ch_idx],
            &sl_name,
            &chunk_positions[start..end],
            &entry_runs,
            &chunks[start..end],
            &popups,
            &mut inline_styles,
        )?;
        g.push(fragment);
        storyline_names.push(sl_name);
        if !readings.is_empty() {
            ruby_readings.push((ch_idx, readings));
        }
    }

    // Every allocated style is registered, and one no storyline uses is dropped (:8088).
    let mut registered: BTreeSet<String> = story_names.iter().cloned().collect();
    registered.extend(extra_style_names.iter().cloned());
    for (.., name) in &styles.cache {
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
    // Ruby, built after the pruning, which looks only in the storylines (:8126).
    if !ruby_readings.is_empty() {
        let f = writing::build_ruby_style_157(g);
        g.push(f);
        extra_style_names.push(writing::RUBY_STYLE_NAME.to_owned());
    }
    let mut ruby_names = Vec::new();
    for (ch_idx, readings) in &ruby_readings {
        let name = format!("ruby{ch_idx}");
        let f = writing::build_fragment_756(g, &name, readings);
        g.push(f);
        ruby_names.push(name);
    }
    // Pictures among words: their styles after the pruning (:8133).
    for (name, value) in inline_styles.into_styles() {
        g.push(Fragment::new("$157", Some(&name), value));
        extra_style_names.push(name);
    }
    writing::build_tcy_fragments(g, &mut extra_style_names, &mut content_names);

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
        ruby_names,
    })
}
