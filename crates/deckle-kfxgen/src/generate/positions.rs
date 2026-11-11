// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the text's positions fall: `_build_position_data` (`native_generator.py:3290`) and the
//! `$264`, `$265` and `$550` fragments built from it.
use std::collections::{BTreeMap, BTreeSet};

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::Value;

use super::chunks::{Marker, Node};
use super::inline_picture_entries;
use super::text::ChapterData;
use super::{Generator, place, py, st, sym};

/// `POSITIONS_PER_LOCATION` (:1591), `POSITIONS_PER_PAGE` and `MAX_PAGE_BREAK_ADJUST` (:1638).
const POSITIONS_PER_LOCATION: i64 = 110;
const POSITIONS_PER_PAGE: i64 = 1850;
const MAX_PAGE_BREAK_ADJUST: i64 = 50;

pub(crate) struct PositionData {
    /// `$265`'s (character offset, position id, offset into the entry), in text order, then
    /// the end.
    pub entries_265: Vec<(i64, i64, i64)>,
    /// `$264`: each section's position ids.
    pub section_positions_264: Vec<(String, Vec<i64>)>,
    /// `$550`'s (position id, offset).
    pub locations: Vec<(i64, i64)>,
    /// Approximate pages' (position id, offset).
    pub pages: Vec<(i64, i64)>,
    /// kfxlib's position count of the largest section.
    pub max_section_pids: i64,
}

/// `_build_position_data` (:3290) for entries of text, pictures, rules and containers: a
/// picture, a rule or a container's `open` takes one position, a `close` none (:3372), and a
/// link's container none, its picture starting at the same place (#355). A picture among words
/// takes a position of its own (#529).
pub(crate) fn build_position_data(section_names: &[String], data: &ChapterData) -> PositionData {
    // (offset, id, offset into the entry, kept beside another at its offset).
    let mut entries = Vec::new();
    let mut char_offset = 0;
    // Each section's (position id, length, offset into the entry), its own element first.
    let mut sections: Vec<Vec<(i64, i64, i64)>> = Vec::new();
    let mut page_texts = BTreeMap::new();
    let mut link_positions = BTreeSet::new();
    for (ch_idx, &(start, end)) in data.chapter_chunk_ranges.iter().enumerate() {
        let section = data.section_positions[ch_idx];
        entries.push((char_offset, section, 0, false));
        char_offset += 1;
        let mut located = vec![(section, 1, 0)];
        for idx in start..end {
            let chunk = &data.chunks[idx];
            let Some(pid) = data.chunk_positions[idx] else {
                continue;
            };
            let length = if matches!(chunk.marker, Some(Marker::Open(Node::Link(_)))) {
                link_positions.insert(pid);
                0
            } else if chunk.picture.is_some() || chunk.marker.is_some() || chunk.rule.is_some() {
                1
            } else {
                page_texts.insert(pid, chunk.text.as_str());
                py::len(&chunk.text) as i64
            };
            entries.push((char_offset, pid, 0, false));
            if !chunk.pictures.is_empty() {
                inline_picture_entries::place(chunk, pid, char_offset, &mut entries, &mut located);
            } else {
                located.push((pid, length, 0));
            }
            char_offset += length;
        }
        sections.push(located);
    }
    entries.sort_by_key(|e| e.0);
    // One entry per offset, but a link's container and its picture, and a picture among
    // words and the words around it (:3427).
    let mut seen = BTreeSet::new();
    let mut deduped: Vec<(i64, i64, i64)> = Vec::new();
    for (offset, pid, at, kept) in entries {
        let shares_link = deduped
            .last()
            .is_some_and(|d| d.0 == offset && link_positions.contains(&d.1));
        if seen.insert(offset) || shares_link || kept {
            deduped.push((offset, pid, at));
        }
    }
    deduped.push((char_offset, 0, 0));

    let section_positions_264 = section_names
        .iter()
        .zip(&data.chapter_chunk_ranges)
        .enumerate()
        .map(|(ch_idx, (name, &(start, end)))| {
            let mut pids = vec![data.section_positions[ch_idx]];
            for (chunk, pid) in data.chunks[start..end]
                .iter()
                .zip(&data.chunk_positions[start..end])
            {
                if let Some(pid) = pid {
                    pids.push(*pid);
                    pids.extend(chunk.pictures.iter().map(|p| p.eid));
                }
            }
            (name.clone(), pids)
        })
        .collect();

    PositionData {
        entries_265: deduped,
        section_positions_264,
        locations: place_locations(&sections),
        pages: place_pages(&sections, &page_texts),
        max_section_pids: sections
            .iter()
            .map(|s| s.iter().map(|&(_, n, _)| n.max(0)).sum())
            .max()
            .unwrap_or(0),
    }
}

/// `place_locations` (:1609): kfxlib's `generate_approximate_locations`.
fn place_locations(sections: &[Vec<(i64, i64, i64)>]) -> Vec<(i64, i64)> {
    let mut locations = Vec::new();
    for entries in sections {
        let (mut pid, mut next_location) = (0, 0);
        for &(eid, length, start) in entries {
            if length <= 0 {
                // A link's container shares its picture's position (#355): a location falling
                // there is the container's, as in kfxlib.
                if next_location == pid {
                    locations.push((eid, 0));
                    next_location += POSITIONS_PER_LOCATION;
                }
                continue;
            }
            while next_location < pid + length {
                locations.push((eid, start + next_location - pid));
                next_location += POSITIONS_PER_LOCATION;
            }
            pid += length;
        }
    }
    locations
}

/// `place_pages` (:1642): kfxlib's `determine_approximate_pages`, a page moved back to the
/// start of its word.
fn place_pages(sections: &[Vec<(i64, i64, i64)>], texts: &BTreeMap<i64, &str>) -> Vec<(i64, i64)> {
    let mut pages = Vec::new();
    for entries in sections {
        let mut pid = 0;
        let mut next_page = None;
        // Not the section's own element.
        for &(eid, length, start) in entries.iter().skip(1) {
            let next_page = next_page.get_or_insert(pid);
            let text: Vec<char> = texts
                .get(&eid)
                .copied()
                .unwrap_or_default()
                .chars()
                .skip(start as usize)
                .take(length.max(0) as usize)
                .collect();
            let mut lowest = 0;
            loop {
                let mut offset = (*next_page - pid).max(0);
                if offset >= length {
                    break;
                }
                if !text.is_empty() && !py::is_space(text[offset as usize]) {
                    let first = offset;
                    while offset > 0 && !py::is_space(text[offset as usize - 1]) {
                        if offset <= lowest {
                            offset = first;
                            break;
                        }
                        offset -= 1;
                    }
                }
                pages.push((eid, start + offset));
                *next_page += POSITIONS_PER_PAGE;
                lowest = offset + (POSITIONS_PER_PAGE - MAX_PAGE_BREAK_ADJUST).max(1);
            }
            pid += length.max(0);
        }
    }
    pages
}

/// `build_fragment_264` (:3239).
pub(crate) fn build_fragment_264(g: &mut Generator, sections: &[(String, Vec<i64>)]) -> Fragment {
    let entries = sections
        .iter()
        .map(|(name, pids)| {
            g.local(name);
            st([
                (
                    "$181",
                    Value::List(pids.iter().map(|&p| Value::Int(p)).collect()),
                ),
                ("$174", sym(name)),
            ])
        })
        .collect();
    Fragment::root("$264", Value::List(entries))
}

/// `build_fragment_265` (:3261).
pub(crate) fn build_fragment_265(entries: &[(i64, i64, i64)]) -> Fragment {
    let entries = entries
        .iter()
        .map(|&(offset, pid, at)| {
            let Value::Struct(mut entry) =
                st([("$184", Value::Int(offset)), ("$185", Value::Int(pid))])
            else {
                unreachable!()
            };
            if at != 0 {
                entry.insert("$143", Value::Int(at));
            }
            Value::Struct(entry)
        })
        .collect();
    Fragment::root("$265", Value::List(entries))
}

/// `$550` (:5119): the locations.
pub(crate) fn build_fragment_550(locations: &[(i64, i64)]) -> Fragment {
    let places = locations.iter().map(|&(p, off)| place(p, off)).collect();
    Fragment::root(
        "$550",
        Value::List(vec![st([("$182", Value::List(places))])]),
    )
}
