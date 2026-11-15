// SPDX-License-Identifier: GPL-3.0-only
//! A spine file's blocks: kfxgen's `extract_blocks_from_html` (`converter.py`, at kfxgen
//! `ad38ec8`), the tree walk that decides what is a block, writes each run of inline content
//! as a paragraph, and records anchors, page breaks and front matter.
//!
//! Blocks are ordered maps ([`Block`], plan decision 3): the model compares Python's key order.
//! Each has a [`BlockId`] that stays with it when it moves (into a box, out of a cell), so the
//! values kfxgen keeps beside its blocks by identity (`_ById`: margins, paddings) are kept
//! beside them here, in maps keyed by [`BlockId`].
//!
//! Ported so far: the walk, its leaf and container branches, inline runs, hidden and
//! non-rendered elements, `<epub:switch>`, the contents-listing discard, `<img>` blocks,
//! direction and language, page breaks, `clear`, front matter, trailing ids and anchor keys
//! (Task 11); margins, padding, carried side margins, spacers and the space between blocks
//! (Task 12, `spacing.rs`).
//! Each branch a later task ports stops the file with [`Error::Unsupported`] naming that task
//! (`walk.rs`), so a book that needs one cannot be converted by accident.

mod anchors;
mod spacing;
mod walk;

use std::collections::{HashMap, HashSet};

use super::inline::Walk;
use super::{Error, ItemError};
use crate::pyvalue::PyValue;

/// A block's identity, Python's `id(block)`: unique within one file's extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(usize);

/// A block's value under one key: a model value, or the blocks a box holds (which keep their
/// identity).
#[derive(Debug)]
pub enum Field {
    Py(PyValue),
    Blocks(Vec<Block>),
}

/// One of kfxgen's block dicts: its keys in insertion order, and its identity.
#[derive(Debug)]
pub struct Block {
    id: BlockId,
    fields: Vec<(&'static str, Field)>,
}

impl Block {
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// `block.get(key)` for a model value; None when absent or when it holds blocks.
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.fields.iter().find_map(|(k, v)| match v {
            Field::Py(p) if *k == key => Some(p),
            _ => None,
        })
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut PyValue> {
        self.fields.iter_mut().find_map(|(k, v)| match v {
            Field::Py(p) if *k == key => Some(p),
            _ => None,
        })
    }

    pub fn contains(&self, key: &str) -> bool {
        self.fields.iter().any(|(k, _)| *k == key)
    }

    /// `block[key] = value`: in place when the key is there, else at the end.
    fn set(&mut self, key: &'static str, value: PyValue) {
        match self.fields.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = Field::Py(value),
            None => self.fields.push((key, Field::Py(value))),
        }
    }

    /// `block.get("type")` as a string, None for a paragraph.
    fn kind(&self) -> Option<&str> {
        self.get("type").and_then(PyValue::as_str)
    }

    /// The dict as the model holds it, the blocks a box holds included.
    pub fn to_py(&self) -> PyValue {
        PyValue::Dict(
            self.fields
                .iter()
                .map(|(k, v)| {
                    let v = match v {
                        Field::Py(p) => p.clone(),
                        Field::Blocks(b) => PyValue::List(b.iter().map(Block::to_py).collect()),
                    };
                    (PyValue::Str((*k).to_owned()), v)
                })
                .collect(),
        )
    }
}

/// What a spine file's extraction is asked to do: `extract_blocks_from_html`'s options.
pub struct ExtractOptions<'o> {
    /// Write an eligible `<table>` as one table block, not a paragraph per row.
    pub native_tables: bool,
    /// The fragment ids this file's contents entries name.
    pub toc_targets: &'o HashSet<String>,
    /// The book's link targets, `"file#id"`.
    pub link_targets: &'o HashSet<String>,
    /// Print contents listings as the book has them (the second pass).
    pub keep_nav_listings: bool,
}

/// A spine file's blocks and what the extraction noted about them (kfxgen's out-lists).
#[derive(Debug)]
pub struct FileBlocks {
    pub blocks: Vec<Block>,
    /// For each contents listing left out: the index of the block that now follows it, and
    /// the ids the listing held (a frozenset: sorted, each once).
    pub nav_listing_at: Vec<(usize, Vec<String>)>,
    /// The first block of each table written as rows of text.
    pub tables_seen: Vec<BlockId>,
    /// The first block of each notes table written as paragraphs.
    pub notes_seen: Vec<BlockId>,
    /// The ids at the file's very end with no block after them.
    pub trailing_ids: Vec<String>,
    /// The ids at the file's very end, a closing rule's own included, for chapter assembly.
    pub end_ids: Vec<String>,
}

/// `extract_blocks_from_html(root, style_resolver, base_href=file.href, …)` for the file `w`
/// walks, with every out-list given.
///
/// It changes nothing in the book: the walk's own per-element answers live in `w`, so a
/// second pass over the file with another `w` is a fresh run.
///
/// Errors: [`ItemError::Raised`] where kfxgen raises (it leaves the spine item out with a
/// warning); [`ItemError::Error`] stops the conversion: a branch not ported yet or refused
/// ([`Error::Unsupported`]), a tree nested past `MAX_DEPTH` or a file past its text budget
/// ([`Error::TooLarge`], [`Error::Book`]).
pub fn extract_blocks(w: &mut Walk, opts: &ExtractOptions) -> Result<FileBlocks, ItemError> {
    let walked = walk::walk(w, opts)?;
    anchors::finish(w, walked)
}

/// A file's blocks as kfxgen hands them to its `_space_between`, with what the walk noted
/// beside them by identity (kfxgen's `_ById` tables).
#[derive(Debug)]
pub struct BeforeSpacing {
    pub blocks: Vec<Block>,
    /// `margins`: a block's margin-top and margin-bottom, in the 16px root's ems.
    pub margins: HashMap<BlockId, [f64; 2]>,
    /// `paddings`: a block's padding-top and padding-bottom, and the margins inside them
    /// (top, bottom), in the 16px root's ems.
    pub paddings: HashMap<BlockId, [f64; 4]>,
}

/// The blocks as kfxgen hands them to the file's `_space_between`, before spacing, trailing
/// ids and anchor keys: the dump's D2a, for tests of the walk alone.
pub fn blocks_before_spacing(
    w: &mut Walk,
    opts: &ExtractOptions,
) -> Result<BeforeSpacing, ItemError> {
    let walked = walk::walk(w, opts)?;
    Ok(BeforeSpacing {
        blocks: walked.blocks,
        margins: walked.margins,
        paddings: walked.paddings,
    })
}

/// A file's [`Error::Unsupported`].
fn unsupported(w: &Walk, feature: &'static str) -> ItemError {
    ItemError::Error(Error::Unsupported {
        feature,
        file: w.file().href.clone(),
    })
}

/// A string as the model holds it.
pub(in crate::convert) fn s(v: &str) -> PyValue {
    PyValue::Str(v.to_owned())
}

/// A list of strings as the model holds it.
pub(in crate::convert) fn strs(items: &[String]) -> PyValue {
    PyValue::List(items.iter().map(|i| s(i)).collect())
}

/// `_dedupe_keep_order(items)`.
pub(in crate::convert) fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect()
}

/// `d[key] = value` on a dict's pairs, string keys: in place when the key is there, else at
/// the end.
pub(in crate::convert) fn dict_set(pairs: &mut Vec<(PyValue, PyValue)>, key: &str, value: PyValue) {
    match pairs.iter_mut().find(|(k, _)| k.as_str() == Some(key)) {
        Some((_, v)) => *v = value,
        None => pairs.push((s(key), value)),
    }
}
