// SPDX-License-Identifier: GPL-3.0-or-later
//! `morgue.json`: what the Morgue learns at each Kindle connection and what has been sent again
//! (design §5, plan rulings). It sits beside `digest.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// An edition sent to the Kindle again. Times are RFC 3339 UTC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resent {
    pub queued: String,
    /// When a Kindle took it; `None` while it waits in the outbox.
    pub delivered: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MorgueState {
    /// The cached editions were imported once.
    pub imported: bool,
    pub resent: BTreeMap<NaiveDate, Resent>,
    /// Passages highlighted per edition, from the last `My Clippings.txt` read.
    pub highlights: BTreeMap<NaiveDate, usize>,
    /// Whether each edition (by stem) was on the Kindle at the last connection.
    pub on_kindle: BTreeMap<String, bool>,
}

/// `morgue.json` beside `digest.toml`.
pub fn state_path() -> PathBuf {
    deckle_digest::config::default_path().with_file_name("morgue.json")
}

impl MorgueState {
    /// The state at `path`; a missing file is an empty state. A file that does not parse or
    /// cannot be read is an error naming it (#82): it holds the editions sent again and the
    /// import flag, so nothing may be saved over it.
    pub fn load(path: &Path) -> Result<MorgueState, String> {
        Ok(deckle_core::state_file::read(path)?.unwrap_or_default())
    }

    /// Writes through a synced temp file and a rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        deckle_core::state_file::save_owner::<MorgueState>(path, &json, false)
            .map_err(std::io::Error::other)
    }
}
