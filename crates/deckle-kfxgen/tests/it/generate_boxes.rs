// SPDX-License-Identifier: GPL-3.0-or-later
//! Block styles, boxes, rules, spaces, lists and pop-up notes against kfxgen's KFX (phase 1
//! plan, Task 9).
use crate::generate_skeleton::matches_kfxgen;

#[test]
fn boxes_matches_kfxgen() {
    matches_kfxgen("boxes");
}

#[test]
fn rules_spaces_matches_kfxgen() {
    matches_kfxgen("rules_spaces");
}

#[test]
fn spacers_matches_kfxgen() {
    matches_kfxgen("spacers");
}

#[test]
fn lists_matches_kfxgen() {
    matches_kfxgen("lists");
}

#[test]
fn notes_matches_kfxgen() {
    matches_kfxgen("notes");
}

/// Paragraph and heading styles the Calibre-made fixtures don't reach: alignment, italic,
/// line heights, indents, scene breaks, monospace, a language of its own, a box's language
/// and padding at another size.
#[test]
fn block_styles_matches_kfxgen() {
    matches_kfxgen("block_styles");
}

/// A chapter's title in and around boxes, boxes in boxes, a table in a box, a box's ids, a
/// title over rules, a rule first in its storyline, a note in a table cell.
#[test]
fn box_titles_matches_kfxgen() {
    matches_kfxgen("box_titles");
}

/// Two text sizes with as many characters each: the smaller is the main size.
#[test]
fn main_size_tie_matches_kfxgen() {
    matches_kfxgen("main_size_tie");
}
