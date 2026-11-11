// SPDX-License-Identifier: GPL-3.0-or-later
//! The navigation, `$389` (`build_fragment_389_toc`, `native_generator.py:3066`): the book's
//! headings, its contents, its pages, its cover and its landmarks.
use std::collections::BTreeMap;

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::Value;

use super::chapters::Chapter;
use super::chunks::Chunk;
use super::inline::id_maps;
use super::pages::BookNav;
use super::text::ChapterData;
use super::{GenerateError, Generator, annotated, model_err, place, st, sym};
use crate::pyvalue::PyValue;

/// A contents entry: a chapter's title, where it starts and its depth, or a place inside it.
pub(crate) struct NavEntry {
    pub title: String,
    pub position: i64,
    pub offset: i64,
    pub level: i64,
}

/// A chapter's `sub_entries` (#363): a contents entry naming a place in the chapter, not a
/// chapter of its own.
pub(crate) struct SubEntry {
    pub title: String,
    pub key: Option<String>,
    pub level: i64,
}

/// A chapter's `sub_entries`: dicts of a `title`, a `key` (None for none) and a `level`
/// (0 when absent).
pub(crate) fn read_sub_entries(
    v: Option<&PyValue>,
    at: &str,
) -> Result<Vec<SubEntry>, GenerateError> {
    let Some(v) = v.filter(|v| v.is_truthy()) else {
        return Ok(Vec::new());
    };
    let list = v
        .as_list()
        .ok_or_else(|| model_err(format!("{at}: a list")))?;
    list.iter()
        .enumerate()
        .map(|(i, sub)| {
            let at = format!("{at}[{i}]");
            Ok(SubEntry {
                title: match sub.get("title") {
                    Some(PyValue::Str(t)) => t.clone(),
                    _ => return Err(model_err(format!("{at}.title: a string"))),
                },
                key: match sub.get("key") {
                    Some(PyValue::None) => None,
                    Some(PyValue::Str(k)) => Some(k.clone()),
                    _ => return Err(model_err(format!("{at}.key: a string or None"))),
                },
                level: match sub.get("level") {
                    None => 0,
                    Some(PyValue::Int(n)) => *n,
                    Some(_) => return Err(model_err(format!("{at}.level: an int"))),
                },
            })
        })
        .collect()
}

/// `key_positions` (:6741): the position and offset each contents entry's key lands on. A
/// leaf, as for a chapter's start; past a container, the leaf's start. A key nothing
/// declares is left out.
pub(crate) fn key_positions(
    chapters: &[Chapter],
    chunks: &[Chunk],
    chunk_positions: &[Option<i64>],
) -> Result<BTreeMap<String, (i64, i64)>, GenerateError> {
    let (key_to_chunk, _) = id_maps(chunks.iter().map(|c| &c.anchors));
    let mut out = BTreeMap::new();
    for sub in chapters.iter().flat_map(|ch| &ch.sub_entries) {
        let Some((key, &(idx, offset))) = sub
            .key
            .as_deref()
            .and_then(|k| Some((k, key_to_chunk.get(k)?)))
        else {
            continue;
        };
        let leaf = (idx..chunks.len())
            .find(|&i| chunks[i].is_leaf())
            .unwrap_or(idx);
        // A container's `close` takes no position: kfxgen would write None. No chunk the port
        // makes puts ids on one.
        let Some(position) = chunk_positions[leaf] else {
            return Err(GenerateError::Unsupported(
                "a contents entry on a container's end",
            ));
        };
        out.insert(
            key.to_owned(),
            (position, if leaf == idx { offset } else { 0 }),
        );
    }
    Ok(out)
}

/// `_book_headings` (:1850): (position, level) of the book's `<h1>`–`<h6>` in reading order.
pub(crate) fn book_headings(data: &ChapterData) -> Vec<(i64, i64)> {
    data.chunks
        .iter()
        .zip(&data.chunk_positions)
        .filter_map(|(chunk, &position)| Some((position?, chunk.heading_level?)))
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
fn nest_nav_entries(entries: &[NavEntry]) -> Result<(Vec<Vec<usize>>, Vec<usize>), GenerateError> {
    let mut children = vec![Vec::new(); entries.len()];
    let mut depths = vec![1; entries.len()];
    let mut roots = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        match (0..i).rev().find(|&j| entries[j].level < e.level) {
            Some(parent) => {
                children[parent].push(i);
                depths[i] = depths[parent] + 1;
                // Writing the entries recurses once a level: stop before an EPUB's
                // contents can overflow the stack (DIVERGENCES.md).
                if depths[i] > MAX_NAV_DEPTH {
                    return Err(GenerateError::Unsupported(
                        "contents nested more than 32 levels deep",
                    ));
                }
            }
            None => roots.push(i),
        }
    }
    Ok((children, roots))
}

/// The deepest contents the port writes.
const MAX_NAV_DEPTH: usize = 32;

fn nav_unit(entries: &[NavEntry], children: &[Vec<usize>], i: usize) -> Value {
    let entry = &entries[i];
    let Value::Struct(mut fields) = st([
        ("$241", st([("$244", Value::string(&entry.title))])),
        ("$246", place(entry.position, entry.offset)),
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

/// `build_fragment_389_toc` (:3066). `cover_position` is the cover page's section, marked
/// `$233` (#160).
pub(crate) fn build_fragment_389(
    g: &mut Generator,
    entries: &[NavEntry],
    cover_position: Option<i64>,
    headings: &[(i64, i64)],
    nav: &BookNav,
) -> Result<Fragment, GenerateError> {
    let (heading_name, toc_name, landmarks_name) = ("nH", "nT", "nL");
    if !headings.is_empty() {
        g.local(heading_name);
    }
    g.local(toc_name);
    if cover_position.is_some() || !nav.landmarks.is_empty() {
        g.local(landmarks_name);
    }

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
    let (children, roots) = nest_nav_entries(entries)?;
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
    if !nav.pages.is_empty() {
        g.local(nav.page_list_name);
        let units = nav
            .pages
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
                ("$239", sym(nav.page_list_name)),
                ("$247", Value::List(units)),
            ]),
        ));
    }
    // After it, the cover, then the book's contents and start landmarks (#579).
    let units: Vec<Value> = cover_position
        .map(|pos| ("$233", "cover-nav-unit", pos, 0))
        .into_iter()
        .chain(
            nav.landmarks
                .iter()
                .map(|m| (m.symbol, m.label.as_str(), m.position, m.offset)),
        )
        .map(|(symbol, label, pos, offset)| {
            annotated(
                "$393",
                st([
                    ("$238", sym(symbol)),
                    ("$241", st([("$244", Value::string(label))])),
                    ("$246", place(pos, offset)),
                ]),
            )
        })
        .collect();
    if !units.is_empty() {
        containers.push(annotated(
            "$391",
            st([
                ("$235", sym("$236")),
                ("$239", sym(landmarks_name)),
                ("$247", Value::List(units)),
            ]),
        ));
    }
    let top = st([("$178", sym("$351")), ("$392", Value::List(containers))]);
    Ok(Fragment::root("$389", Value::List(vec![top])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::chunks::Marker;

    /// Ids on a `close` with no leaf after it would land on no position: unsupported, not a
    /// panic.
    #[test]
    fn a_contents_entry_on_a_container_end_is_unsupported() {
        let mut chapter = Chapter::cover();
        chapter.sub_entries = vec![SubEntry {
            title: "End".to_owned(),
            key: Some("c.xhtml#end".to_owned()),
            level: 0,
        }];
        let mut close = Chunk::marker(Marker::Close);
        close.anchors.attach(&["c.xhtml#end".to_owned()]);
        let chunks = [Chunk::text("Words.".to_owned()), close];
        assert!(matches!(
            key_positions(&[chapter], &chunks, &[Some(1), None]),
            Err(GenerateError::Unsupported(
                "a contents entry on a container's end"
            ))
        ));
    }
}
