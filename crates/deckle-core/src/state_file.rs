// SPDX-License-Identifier: GPL-3.0-or-later
//! State files that are never silently replaced (state-file safety design): the two classes of
//! state file on top of [`deckle_limits::state_file`], and the message the owner sees.
//!
//! **Owner data** ([`read`]): a file that does not parse or cannot be read is an error naming it,
//! so the feature that needs it stops and nothing is written over it.
//!
//! **Rebuildable** ([`rebuildable`]): such a file starts empty with one line on stderr; it may be
//! saved over only when a copy of it was kept.

use std::path::Path;

use serde::de::DeserializeOwned;

pub use deckle_limits::state_file::{Loaded, load, load_with, save};

/// Owner data: `None` for a missing file; an error that names the file for one that does not
/// parse or cannot be read.
pub fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    owner(path, load(path))
}

/// [`read`] for a file that may hold a secret (an API key, a password): the message names the
/// line, the column and the kind of error, never serde's text, which can quote a value.
pub fn read_secret<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    read_with(path, |text| {
        serde_json::from_str(text).map_err(|e| without_values(&e))
    })
}

/// A JSON error with nothing of the file's content in it.
fn without_values(e: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let what = match e.classify() {
        Category::Io => "could not be read",
        Category::Syntax => "not valid JSON",
        Category::Data => "a value of the wrong kind",
        Category::Eof => "the file ends too soon",
    };
    format!("line {}, column {}: {what}", e.line(), e.column())
}

/// [`read`] for a file that is not JSON.
pub fn read_with<T>(
    path: &Path,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, String> {
    owner(path, load_with(path, parse))
}

/// Saves owner data of type `T`: refused, with [`read`]'s message, while the file on disk does
/// not parse as `T` or cannot be read, so a value made without loading it can't replace it.
pub fn save_owner<T: DeserializeOwned>(
    path: &Path,
    bytes: &[u8],
    secret: bool,
) -> Result<(), String> {
    // A secret file's message must not quote its content.
    if secret {
        read_secret::<T>(path)?;
    } else {
        read::<T>(path)?;
    }
    save(path, bytes, secret).map_err(|e| e.to_string())
}

/// Saves a rebuildable file of type `T`: refused while the file on disk can't be read, or does
/// not parse and no copy of it could be kept ([`Loaded::may_replace`]).
pub fn save_rebuildable<T: DeserializeOwned>(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let on_disk = load::<T>(path);
    if !on_disk.may_replace() {
        return Err(format!(
            "{} could not be read and no copy of it was kept, so it was not replaced",
            path.display()
        ));
    }
    save(path, bytes, false).map_err(|e| e.to_string())
}

/// What [`read`] makes of a [`Loaded`].
pub fn owner<T>(path: &Path, loaded: Loaded<T>) -> Result<Option<T>, String> {
    match loaded {
        Loaded::Missing => Ok(None),
        Loaded::Ok(v) => Ok(Some(v)),
        Loaded::Damaged { kept_at, why } => Err(match kept_at {
            Some(copy) => format!(
                "{} could not be read ({why}). Deckle has not changed it. A copy is at {}. Fix \
                 the file or remove it to start again.",
                path.display(),
                copy.display()
            ),
            None => format!(
                "{} could not be read ({why}). Deckle has not changed it. Fix the file or remove \
                 it to start again.",
                path.display()
            ),
        }),
        Loaded::Unreadable(e) => Err(format!(
            "{} could not be read ({e}). Deckle has not changed it.",
            path.display()
        )),
    }
}

/// A rebuildable file as loaded: the value, empty when the file was missing or bad.
#[derive(Debug)]
pub struct Rebuilt<T> {
    pub value: T,
    /// False when the file is there but could not be read, or no copy of it could be kept:
    /// saving would then lose it.
    pub may_save: bool,
    /// The one line said about a bad file, also written to stderr.
    pub note: Option<String>,
}

/// Rebuildable: a file that does not parse starts empty, with one line on stderr and a copy of
/// it kept beside it.
pub fn rebuildable<T: DeserializeOwned + Default>(path: &Path) -> Rebuilt<T> {
    let loaded = load(path);
    let may_save = loaded.may_replace();
    let (value, note) = match loaded {
        Loaded::Missing => (T::default(), None),
        Loaded::Ok(v) => (v, None),
        Loaded::Damaged {
            kept_at: Some(copy),
            why,
        } => (
            T::default(),
            Some(format!(
                "{} could not be read ({why}); starting empty. A copy is at {}.",
                path.display(),
                copy.display()
            )),
        ),
        Loaded::Damaged { kept_at: None, why } => (
            T::default(),
            Some(format!(
                "{} could not be read ({why}) and no copy could be kept; going on without it and \
                 not saving over it.",
                path.display()
            )),
        ),
        Loaded::Unreadable(e) => (
            T::default(),
            Some(format!(
                "{} could not be read ({e}); going on without it and not saving over it.",
                path.display()
            )),
        ),
    };
    if let Some(note) = &note {
        eprintln!("{note}");
    }
    Rebuilt {
        value,
        may_save,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn owner_data_that_does_not_parse_is_an_error_naming_the_file_and_its_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shelf.json");
        assert_eq!(read::<BTreeMap<String, u32>>(&path), Ok(None));
        std::fs::write(&path, "{\"a\": 1").unwrap();
        let e = read::<BTreeMap<String, u32>>(&path).unwrap_err();
        assert!(e.contains(&path.display().to_string()), "{e}");
        assert!(e.contains("shelf.json.damaged-"), "{e}");
        assert!(e.contains("Deckle has not changed it"), "{e}");
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\": 1");
    }

    #[test]
    fn a_rebuildable_file_starts_empty_and_may_be_saved_once_copied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frost.json");
        std::fs::write(&path, "{\"a\": 1").unwrap();
        let r = rebuildable::<BTreeMap<String, u32>>(&path);
        assert!(r.value.is_empty());
        assert!(r.may_save);
        assert!(r.note.unwrap().contains("frost.json.damaged-"));

        let folder = dir.path().join("readers.json");
        std::fs::create_dir(&folder).unwrap();
        let r = rebuildable::<BTreeMap<String, u32>>(&folder);
        assert!(!r.may_save);
    }
}
