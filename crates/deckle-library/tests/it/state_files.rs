// SPDX-License-Identifier: GPL-3.0-or-later
//! The library's state files are never saved over when they don't parse (state-file safety
//! design): each one cut short, then its save path tried; the file stays byte for byte and the
//! error names it.

use std::path::Path;

use deckle_library::shelves::Config as ShelvesConfig;
use deckle_library::writeback::{ReadLog, Writeback};
use deckle_library::{Library, LibraryConfig, upgrade::Upgrades};

const CUT: &[u8] = br#"{"root": "/made/up/Library", "calibre": "/made/up/Cal"#;

fn cut_at(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, CUT).unwrap();
}

fn kept(path: &Path, e: &str) {
    assert!(e.contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(path).unwrap(), CUT, "{}", path.display());
}

#[test]
fn library_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    cut_at(&path);
    let e = LibraryConfig::default().save(&path).unwrap_err();
    kept(&path, &e);
}

#[test]
fn upgrades_json() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("library.json");
    let path = Upgrades::path(&config);
    cut_at(&path);
    let e = Upgrades::default().save(&config).unwrap_err();
    kept(&path, &e);
}

#[test]
fn calibre_shelves_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calibre-shelves.json");
    cut_at(&path);
    let e = ShelvesConfig::default().save(&path).unwrap_err();
    kept(&path, &e.to_string());
}

#[test]
fn calibre_writeback_json_and_the_read_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calibre-writeback.json");
    cut_at(&path);
    let e = Writeback::default().save(&path).unwrap_err();
    kept(&path, &e.to_string());

    let log = dir.path().join("calibre-reading.json");
    cut_at(&log);
    let e = ReadLog::default().save(&log).unwrap_err();
    kept(&log, &e.to_string());
}

/// Losing the seen list would add every EPUB in the folder again.
#[test]
fn the_watch_folders_seen_list() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().join("Library"));
    let watched = dir.path().join("Downloads");
    std::fs::create_dir_all(&watched).unwrap();
    let seen = deckle_library::watch::seen_path(&dir.path().join("library.json"));
    cut_at(&seen);
    let e = deckle_library::watch::scan(&lib, &watched, &seen).unwrap_err();
    kept(&seen, &e);
}
