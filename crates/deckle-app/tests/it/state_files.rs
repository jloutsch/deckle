// SPDX-License-Identifier: GPL-3.0-or-later
//! QA's setup for "no copy could be kept" in a folder that is still writable: every
//! `<name>.damaged-<stamp>[-k]` name is already taken for the next few seconds, so loading a
//! damaged file gives `Damaged { kept_at: None }`. Every save of it is then refused, while a
//! plain save of another file in the same folder works.

use std::collections::BTreeSet;
use std::path::Path;

use deckle_app::commands::{AppState, Settings, get_settings, set_settings};
use deckle_core::shelves::{Diff, Shelves, ours, record_ours};
use deckle_core::state_file::{self, Loaded};
use deckle_digest::chrono::{Duration, Utc};

const CUT: &[u8] = b"{\"cut\": [\"sho";

/// Takes every copy name of `name` for this second and the next few, with empty files (so none
/// matches the damaged bytes), then writes the damaged file.
#[cfg(unix)]
fn every_copy_name_taken(dir: &Path, name: &str, now: deckle_digest::chrono::DateTime<Utc>) {
    for s in 0..4 {
        let stamp = (now + Duration::seconds(s)).format("%Y%m%dT%H%M%SZ");
        for k in 1..=1000 {
            let suffix = if k == 1 {
                String::new()
            } else {
                format!("-{k}")
            };
            std::fs::write(dir.join(format!("{name}.damaged-{stamp}{suffix}")), b"").unwrap();
        }
    }
    std::fs::write(dir.join(name), CUT).unwrap();
}

// Unix only: the names are made fast enough there for the window below.
#[cfg(unix)]
#[test]
fn no_copy_kept_in_a_writable_folder_refuses_every_save() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let start = Utc::now();
    for name in [
        "frost.json",
        "deckle-shelves.json",
        "readers.json",
        "settings.json",
    ] {
        every_copy_name_taken(d, name, start);
    }
    // The names cover four seconds from `start`; the checks below take well under one. A file
    // system too slow for that can't run this test.
    if Utc::now() - start > Duration::seconds(2) {
        eprintln!("skipped: making the names took more than two seconds");
        return;
    }

    // The setup gives what it says.
    let loaded = state_file::load::<serde_json::Value>(&d.join("frost.json"));
    assert!(
        matches!(loaded, Loaded::Damaged { kept_at: None, .. }),
        "{loaded:?}"
    );

    // Class B: a cache, the shelves Deckle made, the wireless readers.
    assert!(
        state_file::save_rebuildable::<serde_json::Value>(&d.join("frost.json"), b"{}").is_err()
    );
    let wanted: Shelves = [("New".to_owned(), BTreeSet::new())].into();
    let diff = Diff {
        create: vec!["New".into()],
        ..Default::default()
    };
    let shelves = d.join("deckle-shelves.json");
    assert!(ours(&shelves).is_empty());
    assert!(record_ours(&shelves, &BTreeSet::new(), &diff, &wanted).is_err());
    let store = deckle_readerserver::wireless::Store::new(d);
    assert!(store.readers().readers.is_empty());
    store.log("a line");

    // Class A: the settings.
    let state = AppState {
        settings_file: d.join("settings.json"),
        devices_file: d.join("devices.json"),
        preload: Default::default(),
        preload_target: Default::default(),
        start_tab: "books",
    };
    assert!(get_settings(&state).damaged.is_some());
    assert!(set_settings(&state, Settings::default()).is_err());

    for name in [
        "frost.json",
        "deckle-shelves.json",
        "readers.json",
        "settings.json",
    ] {
        assert_eq!(std::fs::read(d.join(name)).unwrap(), CUT, "{name}");
    }
    // The folder itself takes a save.
    state_file::save(&d.join("other.json"), b"{}", false).unwrap();
    assert_eq!(std::fs::read(d.join("other.json")).unwrap(), b"{}");
}
