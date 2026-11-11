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
