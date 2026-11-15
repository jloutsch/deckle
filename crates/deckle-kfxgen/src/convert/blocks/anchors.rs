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
    } = walked;
    // `measured=style_resolver is not None`: a file always has one here.
    let href = w.file().href.clone();
    let mut blocks = space_between(&href, blocks, &mut margins, true, &mut paddings)?;
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
        // A table's last row takes them too: Task 16.
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
            tables_seen: Vec::new(),
            notes_seen: Vec::new(),
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
        tables_seen: Vec::new(),
        notes_seen: Vec::new(),
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

/// A list of strings' values.
fn strings(v: Option<&PyValue>) -> Vec<String> {
    v.and_then(PyValue::as_list)
        .unwrap_or(&[])
        .iter()
        .filter_map(|i| i.as_str().map(str::to_owned))
        .collect()
}

/// `_attach_anchor_keys(blocks, base_href)`: each block's ids as `"<file>#<id>"`, its
/// offsets re-keyed the same way, and the file's own key first on its first block. A box's
/// blocks are keyed too, however deep (kfxgen calls `key_block` on each; here a stack). A
/// table's parts are Task 16's.
fn attach_anchor_keys(
    blocks: &mut [Block],
    base_href: &str,
    charge: &dyn Fn(usize) -> Result<(), ItemError>,
) -> Result<(), ItemError> {
    let normalized = key_doc(base_href, "");
    let mut stack: Vec<&mut Block> = blocks.iter_mut().rev().collect();
    while let Some(block) = stack.pop() {
        let ids = strings(block.get("anchor_ids"));
        let by_id: HashMap<String, PyValue> = match block.get("anchor_offsets") {
            Some(PyValue::Dict(pairs)) => pairs
                .iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.clone())))
                .collect(),
            _ => HashMap::new(),
        };
        let (keys, offsets) = if normalized.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let mut keys = Vec::with_capacity(ids.len());
            let mut offsets: Vec<(PyValue, PyValue)> = Vec::with_capacity(ids.len());
            for id in &ids {
                // One copy in the keys, one in the offsets.
                charge(2 * (normalized.len() + 1 + id.len()))?;
                let key = format!("{normalized}#{id}");
                let at = by_id.get(id).cloned().unwrap_or(PyValue::Int(0));
                // A block's ids are deduplicated, so its keys are.
                offsets.push((PyValue::Str(key.clone()), at));
                keys.push(PyValue::Str(key));
            }
            (keys, offsets)
        };
        block.set("anchor_keys", PyValue::List(keys));
        block.set("anchor_offsets", PyValue::Dict(offsets));
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
    }
    Ok(())
}
