// SPDX-License-Identifier: GPL-3.0-or-later
//! Whether and how a chapter's title is already in its text: kfxgen's heading takes the title's
//! place, so the text and blocks are cut of it (`_normalize_title` :827 and its callers).
use super::chapters::Block;
use super::py;

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

/// `_label_blocks` (:1269): how many of the first blocks `title` spells out, or 0. With no
/// rules (`_drop_edge_rules`) and no equations (`label_texts`).
pub(crate) fn label_blocks(blocks: &[Block], title: &str) -> usize {
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
pub(crate) fn title_runs_in(blocks: &[Block], title: &str) -> bool {
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
pub(crate) fn consume_split_title(blocks: &[Block], title: &str) -> usize {
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
