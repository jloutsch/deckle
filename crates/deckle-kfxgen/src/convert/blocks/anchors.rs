// SPDX-License-Identifier: GPL-3.0-only
//! The end of `extract_blocks_from_html` (at kfxgen `ad38ec8`): the ids left at the file's
//! end, the flat block of a file with no block element, and `_attach_anchor_keys`.
//!
//! Each anchor key is a copy of the file's href and an id, for every id of every block: the
//! copies are charged to the file's text budget, so a long href over many ids ends in
//! refusing the file, not in memory without bound.

use std::collections::HashMap;

use super::spacing::space_between;
use super::walk::Walked;
use super::{Block, Field, FileBlocks, dedupe, dict_set, strs};
use crate::convert::ItemError;
use crate::convert::href::key_doc;
use crate::convert::inline::Walk;
use crate::convert::inline_style::Raised;
use crate::generate::py;
use crate::pyvalue::PyValue;

/// From the walk's blocks to the file's: spacing, the trailing ids, anchor keys.
pub(super) fn finish(w: &mut Walk, walked: Walked) -> Result<FileBlocks, ItemError> {
    let Walked {
        blocks,
        pending_ids,
        rule_ids,
        listing_ids,
        nav_listing_at,
        body,
        mut margins,
        mut paddings,
        tables_seen,
        notes_seen,
    } = walked;
    // `measured=style_resolver is not None`: a file always has one here.
    let href = w.file().href.clone();
    let mut blocks = space_between(blocks, &mut margins, true, &mut paddings);
    let mut trailing_ids = Vec::new();
    let mut end_ids = Vec::new();
    if !blocks.is_empty() {
        // Not a rule's own, nor a discarded listing's or a wrapper's round it (#720, #773).
        trailing_ids = pending_ids
            .iter()
            .filter(|i| !rule_ids.contains(*i) && !listing_ids.contains(*i))
            .cloned()
            .collect();
        // A rule's own id ending the file is one too (#774).
        end_ids = pending_ids
            .iter()
            .filter(|i| !listing_ids.contains(*i))
            .cloned()
            .collect();
    }
    if !pending_ids.is_empty() && !blocks.is_empty() {
        let at = blocks
            .iter()
            .rposition(|b| b.kind() != Some("rule"))
            .unwrap_or(blocks.len() - 1);
        let last = &mut blocks[at];
        let mut ids = strings(last.get("anchor_ids"));
        ids.extend(pending_ids.iter().cloned());
        last.set("anchor_ids", strs(&dedupe(ids)));
        // A native table's chunks declare only its own, its rows' and its cells' keys, so the
        // ids go on its last row too (#219).
        if let Some(PyValue::Dict(tbl)) = last.get_mut("table")
            && let Some(PyValue::Dict(row)) = list_mut(tbl, "rows").and_then(|rows| rows.last_mut())
        {
            let mut ids = strings(dict_get(row, "anchor_ids"));
            ids.extend(pending_ids.iter().cloned());
            dict_set(row, "anchor_ids", strs(&dedupe(ids)));
        }
        if last.kind() == Some("box") {
            // Its last paragraph, not the box (#238).
            let Some(kid) = last.blocks_mut().and_then(|kids| kids.last_mut()) else {
                return Err(EMPTY_BOX.into());
            };
            let mut ids = strings(kid.get("anchor_ids"));
            ids.extend(pending_ids.iter().cloned());
            kid.set("anchor_ids", strs(&dedupe(ids)));
        }
    }
    let resolver = w.resolver();
    let charge = |bytes: usize| -> Result<(), ItemError> { Ok(resolver.charge(bytes)?) };
    if !blocks.is_empty() {
        attach_anchor_keys(&mut blocks, &href, &charge)?;
        return Ok(FileBlocks {
            blocks,
            nav_listing_at,
            tables_seen,
            notes_seen,
            trailing_ids,
            end_ids,
        });
    }
    // No block element: flat text, no spans (`body.xpath("string()")`).
    let dom = &w.file().dom;
    let text = dom.itertext(body);
    charge(text.len())?;
    let text = text
        .split('\n')
        .map(py::strip)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(Block {
            id: super::BlockId(0),
            fields: vec![
                ("text", Field::Py(PyValue::Str(text))),
                ("spans", Field::Py(PyValue::List(Vec::new()))),
                ("block_style", Field::Py(PyValue::None)),
                ("anchor_ids", Field::Py(strs(&dedupe(pending_ids)))),
            ],
        });
        attach_anchor_keys(&mut blocks, &href, &charge)?;
    }
    Ok(FileBlocks {
        blocks,
        nav_listing_at,
        tables_seen,
        notes_seen,
        trailing_ids,
        end_ids,
    })
}

/// `last_block["blocks"][-1]` of a box holding no block.
const EMPTY_BOX: Raised = Raised {
    exception: "IndexError",
    note: "extract_blocks_from_html: ids left at the file's end, its last block a box of no \
           blocks",
};

/// `d.get(key)` on a dict's pairs.
fn dict_get<'p>(pairs: &'p [(PyValue, PyValue)], key: &str) -> Option<&'p PyValue> {
    pairs
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

/// The list a dict holds under `key`, to change in place.
fn list_mut<'p>(pairs: &'p mut [(PyValue, PyValue)], key: &str) -> Option<&'p mut Vec<PyValue>> {
    pairs.iter_mut().find_map(|(k, v)| match v {
        PyValue::List(items) if k.as_str() == Some(key) => Some(items),
        _ => None,
    })
}

/// A list of strings' values.
fn strings(v: Option<&PyValue>) -> Vec<String> {
    v.and_then(PyValue::as_list)
        .unwrap_or(&[])
        .iter()
        .filter_map(|i| i.as_str().map(str::to_owned))
        .collect()
}

/// A part's `anchor_keys` and its re-keyed `anchor_offsets`.
type Keys = (Vec<PyValue>, Vec<(PyValue, PyValue)>);

/// `key_block`'s keys for one part: each id as `"<file>#<id>"`, and its offsets re-keyed the
/// same way (none without a file). Each key is copied twice, charged.
fn keys_of(
    ids: &[String],
    by_id: &HashMap<String, PyValue>,
    normalized: &str,
    charge: &dyn Fn(usize) -> Result<(), ItemError>,
) -> Result<Keys, ItemError> {
    if normalized.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut keys = Vec::with_capacity(ids.len());
    let mut offsets: Vec<(PyValue, PyValue)> = Vec::with_capacity(ids.len());
    for id in ids {
        // One copy in the keys, one in the offsets.
        charge(2 * (normalized.len() + 1 + id.len()))?;
        let key = format!("{normalized}#{id}");
        let at = by_id.get(id).cloned().unwrap_or(PyValue::Int(0));
        // A part's ids are deduplicated, so its keys are.
        offsets.push((PyValue::Str(key.clone()), at));
        keys.push(PyValue::Str(key));
    }
    Ok((keys, offsets))
}

/// `{id: offset}` of `anchor_offsets` (`block.get("anchor_offsets") or {}`).
fn offsets_by_id(v: Option<&PyValue>) -> HashMap<String, PyValue> {
    match v {
        Some(PyValue::Dict(pairs)) => pairs
            .iter()
            .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.clone())))
            .collect(),
        _ => HashMap::new(),
    }
}

/// One part of a table, a cell paragraph or a block in a box in a cell, keyed as `key_block`
/// keys a block: `part["anchor_keys"] = ...`, then `part["anchor_offsets"] = ...`.
fn key_part(
    part: &mut Vec<(PyValue, PyValue)>,
    normalized: &str,
    charge: &dyn Fn(usize) -> Result<(), ItemError>,
) -> Result<(), ItemError> {
    let ids = strings(dict_get(part, "anchor_ids"));
    let by_id = offsets_by_id(dict_get(part, "anchor_offsets"));
    let (keys, offsets) = keys_of(&ids, &by_id, normalized, charge)?;
    dict_set(part, "anchor_keys", PyValue::List(keys));
    dict_set(part, "anchor_offsets", PyValue::Dict(offsets));
    Ok(())
}

/// What is left to key inside a table: a table dict, or a block dict (in a box in a cell).
enum Part<'v> {
    Table(&'v mut PyValue),
    Block(&'v mut PyValue),
}

/// `key_block`'s table branch on `tbl`: the table, then every row, every cell, every cell
/// paragraph, then the blocks of each box among those paragraphs (each keyed as
/// `key_block` keys a block, its own box's blocks and table included). A stack, not
/// recursion.
fn key_table(
    tbl: &mut PyValue,
    normalized: &str,
    charge: &dyn Fn(usize) -> Result<(), ItemError>,
) -> Result<(), ItemError> {
    let mut stack = vec![Part::Table(tbl)];
    while let Some(part) = stack.pop() {
        match part {
            Part::Block(b) => {
                let PyValue::Dict(pairs) = b else {
                    continue;
                };
                key_part(pairs, normalized, charge)?;
                let boxed = dict_get(pairs, "type").and_then(PyValue::as_str) == Some("box");
                let tabled = dict_get(pairs, "table").is_some_and(PyValue::is_truthy);
                let mut next = Vec::new();
                for (k, v) in pairs.iter_mut() {
                    let key = k.as_str();
                    if boxed && key == Some("blocks") {
                        if let PyValue::List(kids) = v {
                            next.extend(kids.iter_mut().map(Part::Block));
                        }
                    } else if tabled && key == Some("table") {
                        next.push(Part::Table(v));
                    }
                }
                stack.extend(next.into_iter().rev());
            }
            Part::Table(t) => {
                let PyValue::Dict(t) = t else {
                    continue;
                };
                key_part(t, normalized, charge)?;
                for row in list_mut(t, "rows").into_iter().flatten() {
                    if let PyValue::Dict(row) = row {
                        key_part(row, normalized, charge)?;
                    }
                }
                for cell in cells_mut(t) {
                    key_part(cell, normalized, charge)?;
                }
                for cell in cells_mut(t) {
                    for p in list_mut(cell, "paragraphs").into_iter().flatten() {
                        if let PyValue::Dict(p) = p {
                            key_part(p, normalized, charge)?;
                        }
                    }
                }
                // A box in a cell (#645): its blocks are the targets.
                let mut next = Vec::new();
                for cell in cells_mut(t) {
                    for p in list_mut(cell, "paragraphs").into_iter().flatten() {
                        let PyValue::Dict(p) = p else {
                            continue;
                        };
                        if dict_get(p, "type").and_then(PyValue::as_str) != Some("box") {
                            continue;
                        }
                        for kid in list_mut(p, "blocks").into_iter().flatten() {
                            next.push(Part::Block(kid));
                        }
                    }
                }
                stack.extend(next.into_iter().rev());
            }
        }
    }
    Ok(())
}

/// Every cell dict of a table dict, row by row.
fn cells_mut(t: &mut [(PyValue, PyValue)]) -> impl Iterator<Item = &mut Vec<(PyValue, PyValue)>> {
    list_mut(t, "rows")
        .into_iter()
        .flatten()
        .filter_map(|row| match row {
            PyValue::Dict(row) => list_mut(row, "cells"),
            _ => None,
        })
        .flatten()
        .filter_map(|cell| match cell {
            PyValue::Dict(cell) => Some(cell),
            _ => None,
        })
}

/// `_attach_anchor_keys(blocks, base_href)`: each block's ids as `"<file>#<id>"`, its
/// offsets re-keyed the same way, and the file's own key first on its first block. A box's
/// blocks are keyed too, however deep (kfxgen calls `key_block` on each; here a stack), and a
/// table's own, rows', cells' and cell paragraphs' ids.
fn attach_anchor_keys(
    blocks: &mut [Block],
    base_href: &str,
    charge: &dyn Fn(usize) -> Result<(), ItemError>,
) -> Result<(), ItemError> {
    let normalized = key_doc(base_href, "");
    let mut stack: Vec<&mut Block> = blocks.iter_mut().rev().collect();
    while let Some(block) = stack.pop() {
        let ids = strings(block.get("anchor_ids"));
        let by_id = offsets_by_id(block.get("anchor_offsets"));
        let (keys, offsets) = keys_of(&ids, &by_id, &normalized, charge)?;
        block.set("anchor_keys", PyValue::List(keys));
        block.set("anchor_offsets", PyValue::Dict(offsets));
        if let Some(tbl) = block.get_mut("table")
            && tbl.is_truthy()
        {
            key_table(tbl, &normalized, charge)?;
        }
        if block.kind() == Some("box")
            && let Some(kids) = block.blocks_mut()
        {
            // Its blocks are the link targets; the box carries their ids for the chapter
            // split (#238, #310).
            stack.extend(kids.iter_mut().rev());
        }
    }
    // A link to the whole file lands on its first block (#62).
    if !normalized.is_empty()
        && let Some(first) = blocks.first_mut()
    {
        if let Some(PyValue::List(keys)) = first.get_mut("anchor_keys") {
            keys.insert(0, PyValue::Str(normalized.clone()));
        }
        if let Some(PyValue::Dict(offsets)) = first.get_mut("anchor_offsets") {
            dict_set(offsets, &normalized, PyValue::Int(0));
        }
        if let Some(PyValue::Dict(tbl)) = first.get_mut("table")
            && let Some(PyValue::List(keys)) = tbl
                .iter_mut()
                .find(|(k, _)| k.as_str() == Some("anchor_keys"))
                .map(|(_, v)| v)
        {
            keys.insert(0, PyValue::Str(normalized.clone()));
        }
    }
    Ok(())
}
