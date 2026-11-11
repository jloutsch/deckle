// SPDX-License-Identifier: GPL-3.0-or-later
//! Pop-up notes (#445): a noteref link to a footnote, which Kindle Previewer marks, its link
//! `$616: $617` and the note's entry `$615: $281` (`native_generator.py:6786`, :4403, :4474).
use std::collections::BTreeSet;

use super::inline::{Flag, Span};
use super::{GenerateError, model_err};
use crate::pyvalue::PyValue;

/// `generate_full_book`'s `footnote_keys` (:4739): the anchor keys that name a footnote.
pub(crate) fn read_footnote_keys(v: Option<&PyValue>) -> Result<BTreeSet<String>, GenerateError> {
    match v {
        None | Some(PyValue::None) => Ok(BTreeSet::new()),
        Some(PyValue::Set(keys) | PyValue::List(keys) | PyValue::Tuple(keys)) => keys
            .iter()
            .map(|k| k.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or_else(|| model_err("args.footnote_keys: strings")),
        Some(_) => Err(model_err("args.footnote_keys: a set of strings")),
    }
}

/// The targets of noteref links that are footnotes (:6797).
pub(crate) fn popup_targets<'a>(
    spans: impl IntoIterator<Item = &'a Span>,
    footnote_keys: &BTreeSet<String>,
) -> BTreeSet<String> {
    spans
        .into_iter()
        .filter(|sp| sp.flags.contains(&Flag::NoteRef))
        .flat_map(|sp| &sp.flags)
        .filter_map(|f| match f {
            Flag::Link(t) if footnote_keys.contains(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// The `$266` anchors of pop-up notes and the positions of the notes they land on. Marked
/// per anchor, so a plain link to the same note is marked too (:6795).
#[derive(Default)]
pub(crate) struct Popups {
    pub targets: BTreeSet<String>,
    pub anchors: BTreeSet<String>,
    pub positions: BTreeSet<i64>,
}

impl Popups {
    /// A body anchor for `target` landing at `position`: marked when it is a pop-up note's,
    /// unless the note is in a table cell, where Previewer marks none (#446 review).
    pub(crate) fn landed(&mut self, target: &str, anchor: &str, position: i64, in_cell: bool) {
        if self.targets.contains(target) && !in_cell {
            self.anchors.insert(anchor.to_owned());
            self.positions.insert(position);
        }
    }
}
