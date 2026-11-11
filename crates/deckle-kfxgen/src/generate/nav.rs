// SPDX-License-Identifier: GPL-3.0-or-later
//! The navigation, `$389` (`build_fragment_389_toc`, `native_generator.py:3066`): the book's
//! headings, its contents and its pages. A cover and landmarks aren't ported yet.
use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::Value;

use super::text::ChapterData;
use super::{Generator, annotated, place, st, sym};

/// A contents entry: a chapter's title, where it starts and its depth.
pub(crate) struct NavEntry {
    pub title: String,
    pub position: i64,
    pub level: i64,
}

/// `_book_headings` (:1850): (position, level) of the book's `<h1>`–`<h6>` in reading order.
pub(crate) fn book_headings(data: &ChapterData) -> Vec<(i64, i64)> {
    data.chunks
        .iter()
        .zip(&data.chunk_positions)
        .filter_map(|(chunk, &position)| chunk.heading_level.map(|level| (position, level)))
        .collect()
}

/// `_HEADING_LEVEL_UNITS` (:1840).
fn heading_level_unit(level: i64) -> &'static str {
    ["$799", "$800", "$801", "$802", "$803", "$804"][(level - 1) as usize]
}

/// `_heading_levels` (:1862): a unit per level, in the order levels first appear.
fn heading_levels(headings: &[(i64, i64)]) -> Vec<Value> {
    let mut levels: Vec<i64> = Vec::new();
    for &(_, level) in headings {
        if !levels.contains(&level) {
            levels.push(level);
        }
    }
    let unit = || st([("$244", Value::string("heading-nav-unit"))]);
    levels
        .into_iter()
        .map(|level| {
            let positions: Vec<i64> = headings
                .iter()
                .filter(|&&(_, l)| l == level)
                .map(|&(p, _)| p)
                .collect();
            let children = positions
                .iter()
                .map(|&p| annotated("$393", st([("$241", unit()), ("$246", place(p, 0))])))
                .collect();
            annotated(
                "$393",
                st([
                    ("$238", sym(heading_level_unit(level))),
                    ("$241", unit()),
                    ("$246", place(positions[0], 0)),
                    ("$247", Value::List(children)),
                ]),
            )
        })
        .collect()
}

/// `_nest_nav_entries` (:2119): each entry's children are the entries after it at a deeper
/// level, up to the next at its own level or shallower. Its parent is the last entry before it
/// at a shallower level. Returns each entry's children and the top-level entries, by index.
fn nest_nav_entries(entries: &[NavEntry]) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut children = vec![Vec::new(); entries.len()];
    let mut roots = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        match (0..i).rev().find(|&j| entries[j].level < e.level) {
            Some(parent) => children[parent].push(i),
            None => roots.push(i),
        }
    }
    (children, roots)
}

fn nav_unit(entries: &[NavEntry], children: &[Vec<usize>], i: usize) -> Value {
    let entry = &entries[i];
    let Value::Struct(mut fields) = st([
        ("$241", st([("$244", Value::string(&entry.title))])),
        ("$246", place(entry.position, 0)),
    ]) else {
        unreachable!()
    };
    if !children[i].is_empty() {
        let kids = children[i]
            .iter()
            .map(|&k| nav_unit(entries, children, k))
            .collect();
        fields.insert("$247", Value::List(kids));
    }
    annotated("$393", Value::Struct(fields))
}

/// `build_fragment_389_toc` (:3066) with no cover and no landmarks. `pages` are (label,
/// position, offset).
pub(crate) fn build_fragment_389(
    g: &mut Generator,
    entries: &[NavEntry],
    headings: &[(i64, i64)],
    pages: &[(String, i64, i64)],
    page_list_name: &str,
) -> Fragment {
    let (heading_name, toc_name) = ("nH", "nT");
    if !headings.is_empty() {
        g.local(heading_name);
    }
    g.local(toc_name);

    let mut containers = Vec::new();
    if !headings.is_empty() {
        containers.push(annotated(
            "$391",
            st([
                ("$235", sym("$798")),
                ("$239", sym(heading_name)),
                ("$247", Value::List(heading_levels(headings))),
            ]),
        ));
    }
    let (children, roots) = nest_nav_entries(entries);
    let units = roots
        .iter()
        .map(|&i| nav_unit(entries, &children, i))
        .collect();
    containers.push(annotated(
        "$391",
        st([
            ("$235", sym("$212")),
            ("$239", sym(toc_name)),
            ("$247", Value::List(units)),
        ]),
    ));
    if !pages.is_empty() {
        g.local(page_list_name);
        let units = pages
            .iter()
            .map(|(label, pos, offset)| {
                annotated(
                    "$393",
                    st([
                        ("$241", st([("$244", Value::string(label))])),
                        ("$246", place(*pos, *offset)),
                    ]),
                )
            })
            .collect();
        containers.push(annotated(
            "$391",
            st([
                ("$235", sym("$237")),
                ("$239", sym(page_list_name)),
                ("$247", Value::List(units)),
            ]),
        ));
    }
    let top = st([("$178", sym("$351")), ("$392", Value::List(containers))]);
    Fragment::root("$389", Value::List(vec![top]))
}
