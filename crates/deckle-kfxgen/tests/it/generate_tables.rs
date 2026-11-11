// SPDX-License-Identifier: GPL-3.0-or-later
//! Tables against kfxgen's KFX (phase 1 plan, Task 8).
use crate::generate_skeleton::matches_kfxgen;

#[test]
fn table_cells_matches_kfxgen() {
    matches_kfxgen("table_cells");
}

#[test]
fn table_layout_matches_kfxgen() {
    matches_kfxgen("table_layout");
}

#[test]
fn table_paragraph_cells_matches_kfxgen() {
    matches_kfxgen("table_paragraph_cells");
}

/// Borders, widths, padding, groups, header and empty cells, runs and ids no EPUB-made
/// fixture reaches.
#[test]
fn table_parts_matches_kfxgen() {
    matches_kfxgen("table_parts");
}

/// A chapter's title in or over a table.
#[test]
fn table_title_matches_kfxgen() {
    matches_kfxgen("table_title");
}
