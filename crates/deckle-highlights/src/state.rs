// SPDX-License-Identifier: GPL-3.0-or-later
//! `highlights.json` (highlights export design §5): the folder, and per book the file Deckle
//! writes and the entries already in it.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use deckle_limits::state_file::{self, Loaded};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub folder: Option<PathBuf>,
    pub books: BTreeMap<String, BookState>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookState {
    /// The file's name inside the folder.
    pub file: String,
    /// Ids ([`crate::entry_id`]) of the entries written to it.
    pub written: BTreeSet<String>,
}

impl State {
    /// A missing file is an empty state. One that is there but does not parse or cannot be read
    /// is an error naming it (state-file safety design): it holds the folder and what each
    /// book's file already has, so nothing may be saved over it.
    pub fn load(path: &Path) -> Result<State, String> {
        // The same words as deckle_core::state_file, which this crate doesn't depend on.
        match state_file::load(path) {
            Loaded::Missing => Ok(State::default()),
            Loaded::Ok(s) => Ok(s),
            Loaded::Damaged {
                kept_at: Some(copy),
                why,
            } => Err(format!(
                "{} could not be read ({why}). Deckle has not changed it. A copy is at {}. Fix \
                 the file or remove it to start again.",
                path.display(),
                copy.display()
            )),
            Loaded::Damaged { kept_at: None, why } => Err(format!(
                "{} could not be read ({why}). Deckle has not changed it. Fix the file or remove \
                 it to start again.",
                path.display()
            )),
            Loaded::Unreadable(e) => Err(format!(
                "{} could not be read ({e}). Deckle has not changed it.",
                path.display()
            )),
        }
    }

    /// Writes through a synced temp file, refused while the file on disk can't be loaded.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        State::load(path).map_err(io::Error::other)?;
        state_file::save(path, &serde_json::to_vec_pretty(self)?, false)
    }
}
