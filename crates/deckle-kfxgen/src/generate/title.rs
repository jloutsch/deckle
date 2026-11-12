// SPDX-License-Identifier: GPL-3.0-or-later
//! Whether and how a chapter's title is already in its text: kfxgen's heading takes the title's
//! place, so the text and blocks are cut of it (`_normalize_title` :827 and its callers).
use super::boxes::{Line, box_title_lines, drop_box_lines, drop_edge_rules, unboxed};
use super::chapters::Block;
use super::{py, tables};

/// `_MAX_SPLIT_TITLE_BLOCKS` (:824).
pub(crate) const MAX_SPLIT_TITLE_BLOCKS: usize = 3;

/// `_normalize_title` (:827).
fn normalize_title(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|&c| py::is_alnum(c))
        .collect()
}

/// `_opens_with_title` (:1356).
pub(crate) fn opens_with_title(text: &str, title: &str) -> bool {
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

/// `_label_blocks` (:1269): how many of the first blocks `title` spells out, or 0; a table,
/// a rule or a box among them says 0. With no equations (`label_texts`).
pub(crate) fn label_blocks(blocks: &[&Block], title: &str) -> usize {
    let want = py::without_spaces(title);
    let target = normalize_title(title);
    let mut joined = String::new();
    let blocks = drop_edge_rules(blocks);
    for (i, block) in blocks.iter().take(MAX_SPLIT_TITLE_BLOCKS).enumerate() {
        if !block.is_paragraph() {
            return 0;
        }
        joined.push_str(&block.text);
        if (!target.is_empty() && normalize_title(&joined) == target)
            || py::without_spaces(&joined) == want
        {
            return i + 1;
        }
    }
    0
}

/// The first of `blocks` after any rule at their start, through the boxes it opens.
fn first_unboxed(blocks: &[Block]) -> Option<&Block> {
    let mut first = drop_edge_rules(blocks).first();
    while let Some(bx) = first.and_then(|b| b.boxed.as_deref()) {
        first = bx.blocks.first();
    }
    first
}

/// `_title_runs_in` (:1169): a box's first paragraph counts too (#654).
pub(crate) fn title_runs_in(blocks: &[Block], title: &str) -> bool {
    let Some(first) = first_unboxed(blocks) else {
        return false;
    };
    if first.heading || !first.is_paragraph() {
        return false;
    }
    let text = py::lstrip(&first.text);
    opens_with_title(text, title) && py::skip(text, py::len(title)).chars().any(py::is_alnum)
}

/// `_consume_split_title` (:837).
fn consume_split_title(texts: &[&str], title: &str) -> usize {
    let target = normalize_title(title);
    if target.is_empty() {
        return 0;
    }
    let mut acc = String::new();
    for (i, text) in texts.iter().take(MAX_SPLIT_TITLE_BLOCKS).enumerate() {
        acc.push_str(&normalize_title(text));
        if acc == target {
            return i + 1;
        }
        if !target.starts_with(&acc) {
            return 0;
        }
    }
    0
}

/// The blocks left after a title cut, and the ids of those it dropped.
pub(crate) type Eaten = (Vec<Block>, Vec<String>);

/// `_eat_split_title` (:1524): the blocks less the leading ones that make up `title`, and
/// their ids; None when they don't. A table counts one block per row with text, and one
/// partly eaten keeps its other rows (#64, #219); a box one per line it opens with, through
/// a box it opens with, never into a table (#238, #310).
pub(crate) fn eat_split_title(blocks: &[Block], title: &str) -> Option<Eaten> {
    // (block, the line in it: a row of a table, a box's line, the text)
    let mut entries: Vec<(usize, Line, String)> = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        if entries.len() >= MAX_SPLIT_TITLE_BLOCKS {
            break;
        }
        if let Some(bx) = &b.boxed {
            let (lines, whole) = box_title_lines(bx, &[]);
            entries.extend(
                lines
                    .into_iter()
                    .map(|(path, row, text)| (i, Line::Boxed(path, row), text)),
            );
            if !whole {
                break;
            }
            continue;
        }
        match &b.table {
            Some(t) => entries.extend(
                tables::source_order(&t.rows)
                    .into_iter()
                    .map(|j| (i, Line::Row(j), tables::row_text(&t.rows[j])))
                    .filter(|e| !e.2.is_empty()),
            ),
            None => entries.push((i, Line::Whole, b.text.clone())),
        }
    }
    let texts: Vec<&str> = entries.iter().map(|e| e.2.as_str()).collect();
    let eaten = consume_split_title(&texts, title);
    if eaten == 0 {
        return None;
    }
    let (last, line, _) = &entries[eaten - 1];
    let last = *last;
    let mut carried: Vec<String> = blocks[..last]
        .iter()
        .flat_map(|b| b.anchors.keys.iter().cloned())
        .collect();
    let rest = |kept: Option<Block>| {
        let mut rest: Vec<Block> = kept.into_iter().collect();
        rest.extend_from_slice(&blocks[last + 1..]);
        rest
    };
    match line {
        Line::Boxed(path, row) => {
            let (kept, more) = drop_box_lines(&blocks[last], path, *row);
            carried.extend(more);
            return Some((rest(kept), carried));
        }
        Line::Row(row) => {
            let t = blocks[last].table.as_deref()?;
            let order = tables::source_order(&t.rows);
            let upto = order.iter().position(|j| j == row).unwrap_or(0);
            if let Some(kept) = tables::drop_rows(&blocks[last], &order[..=upto]) {
                return Some((rest(Some(kept)), carried));
            }
        }
        Line::Whole => {}
    }
    carried.extend(blocks[last].anchors.keys.iter().cloned());
    Some((rest(None), carried))
}

/// `_title_in_table` (:1187): the chapter opens with a table, or a box opening with one,
/// whose first rows print its title, in one row or spelled over rows. The table prints as
/// written, and kfxgen prints no heading (#470).
pub(crate) fn title_in_table(blocks: &[Block], title: &str) -> bool {
    match first_unboxed(blocks).and_then(|b| b.table.as_deref()) {
        Some(t) => {
            tables::table_opens_with_title(t, title)
                || eat_split_title(drop_edge_rules(blocks), title).is_some()
        }
        None => false,
    }
}

/// `_heading_near_start` (:1204): the book prints its own heading among the first blocks,
/// a box's or a table cell's included (#492, #654, #660). A heading opening the first box
/// counts only when it is not `title` (#646, #749).
pub(crate) fn heading_near_start(blocks: &[Block], title: &str) -> bool {
    let blocks = drop_edge_rules(blocks);
    blocks
        .iter()
        .take(MAX_SPLIT_TITLE_BLOCKS)
        .enumerate()
        .any(|(i, b)| {
            if b.heading {
                return true;
            }
            if let Some(bx) = &b.boxed {
                let kids = unboxed(&bx.blocks);
                if i == 0
                    && kids.first().is_some_and(|k| k.heading)
                    && label_blocks(&kids[..1], title) == 0
                {
                    return true;
                }
                let skip = if i == 0 { 1 } else { 0 };
                if kids.iter().skip(skip).any(|k| k.heading) {
                    return true;
                }
            }
            b.table.as_deref().is_some_and(|t| {
                t.rows
                    .iter()
                    .flat_map(|r| &r.cells)
                    .any(|c| c.heading || c.paragraphs.iter().any(|p| p.heading))
            })
        })
}
