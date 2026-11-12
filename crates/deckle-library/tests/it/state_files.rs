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

/// `path` holds `CUT` in a folder where no copy can be made; `attempt` tries the save path.
/// Skipped as root, where the folder stays writable.
#[cfg(unix)]
fn with_no_copy(name: &str, attempt: impl FnOnce(&Path)) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let path = ro.join(name);
    std::fs::write(&path, CUT).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    attempt(&path);
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), CUT, "{name}");
    assert_eq!(
        std::fs::read_dir(&ro).unwrap().count(),
        1,
        "{name}: no copy, no temp"
    );
}

/// Damaged and no copy could be kept (`may_replace` false): every save path is refused.
#[cfg(unix)]
#[test]
fn no_save_when_no_copy_could_be_kept() {
    with_no_copy("library.json", |p| {
        assert!(LibraryConfig::load(p).is_err());
        assert!(LibraryConfig::default().save(p).is_err());
    });
    with_no_copy("upgrades.json", |p| {
        let config = p.with_file_name("library.json");
        assert!(Upgrades::load(&config).is_err());
        assert!(Upgrades::default().save(&config).is_err());
    });
    with_no_copy("calibre-shelves.json", |p| {
        assert!(ShelvesConfig::load(p).is_err());
        assert!(ShelvesConfig::default().save(p).is_err());
    });
    with_no_copy("calibre-writeback.json", |p| {
        assert!(Writeback::load(p).is_err());
        assert!(Writeback::default().save(p).is_err());
    });
    with_no_copy("reading-log.json", |p| {
        assert!(ReadLog::load(p).is_err());
        assert!(ReadLog::default().save(p).is_err());
    });
    with_no_copy("watched.json", |p| {
        let lib = Library::new(p.with_file_name("Library"));
        let watched = tempfile::tempdir().unwrap();
        assert!(deckle_library::watch::scan(&lib, watched.path(), p).is_err());
    });
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
