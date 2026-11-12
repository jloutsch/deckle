// SPDX-License-Identifier: GPL-3.0-or-later
//! deckle-digest's owner-data files are never saved over when they don't parse (state-file
//! safety design): the file stays byte for byte and the error names it.

use deckle_digest::puzzle::store::Store;

/// The puzzles printed and how each was solved.
#[test]
fn puzzles_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("puzzles.json");
    let cut = br#"{"days": {"2026-11-12": {"sudoku": "4"#;
    std::fs::write(&path, cut).unwrap();
    let _ = Store::load(&path);
    let e = Store::default().save(&path).unwrap_err();
    assert!(e.to_string().contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), cut);
}

/// A token that is not 32 hex characters is kept, not replaced by a new one that would break
/// every installed bookmarklet; the message says where it is and what deleting it does.
#[test]
fn bookmarklet_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bookmarklet.token");
    std::fs::write(&path, b"0123456789abcdef01234").unwrap();
    let e = deckle_digest::bookmarklet::token(dir.path()).unwrap_err();
    let e = e.to_string();
    assert!(e.contains(&path.display().to_string()), "{e}");
    assert!(e.contains("install"), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), b"0123456789abcdef01234");
}

/// Damaged in a folder where no copy can be kept: the save paths are refused and the files
/// stay. Skipped as root.
#[cfg(unix)]
#[test]
fn no_save_when_no_copy_could_be_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let (puzzles, token) = (ro.join("puzzles.json"), ro.join("bookmarklet.token"));
    std::fs::write(&puzzles, b"{\"days\": {").unwrap();
    std::fs::write(&token, b"0123").unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    assert!(Store::load(&puzzles).is_err());
    assert!(Store::default().save(&puzzles).is_err());
    assert!(deckle_digest::bookmarklet::token(&ro).is_err());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&puzzles).unwrap(), b"{\"days\": {");
    assert_eq!(std::fs::read(&token).unwrap(), b"0123");
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 2);
}

/// A missing token is made once, private to the owner, and read back the same.
#[test]
fn a_new_bookmarklet_token_is_made_once() {
    let dir = tempfile::tempdir().unwrap();
    let first = deckle_digest::bookmarklet::token(dir.path()).unwrap();
    assert_eq!(first.len(), 32);
    assert_eq!(
        deckle_digest::bookmarklet::token(dir.path()).unwrap(),
        first
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("bookmarklet.token"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

// ---- rebuildable files (class B): start empty, said, with a copy kept ----

use deckle_digest::daemon::DaemonState;
use deckle_digest::next::live::LiveCache;

fn copies(dir: &std::path::Path, name: &str) -> Vec<Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("{name}.damaged-"))
        })
        .map(|e| std::fs::read(e.path()).unwrap())
        .collect()
}

/// `next.json` (the forecast cache) and `daemon.json`: a damaged file starts empty, a copy
/// keeps its bytes, and the next save replaces it.
#[test]
fn rebuildable_files_start_empty_with_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let cut = b"{\"feeds\": {\"https://e";
    let live = dir.path().join("next.json");
    std::fs::write(&live, cut).unwrap();
    assert_eq!(LiveCache::load(&live), LiveCache::default());
    assert_eq!(copies(dir.path(), "next.json"), [cut.to_vec()]);
    LiveCache::default().save(&live).unwrap();
    assert_eq!(LiveCache::load(&live), LiveCache::default());

    let daemon = dir.path().join("daemon.json");
    std::fs::write(&daemon, cut).unwrap();
    assert_eq!(DaemonState::load(&daemon), DaemonState::default());
    assert_eq!(copies(dir.path(), "daemon.json"), [cut.to_vec()]);
    DaemonState::default().save(&daemon).unwrap();
}

/// Damaged where no copy can be kept, or unreadable (the root-owned daemon.json case): the
/// saves are refused and the files stay. Skipped as root.
#[cfg(unix)]
#[test]
fn a_rebuildable_file_with_no_copy_or_unreadable_is_never_saved_over() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let cut = b"{\"feeds\": {\"https://e";
    let (live, daemon) = (ro.join("next.json"), ro.join("daemon.json"));
    std::fs::write(&live, cut).unwrap();
    std::fs::write(&daemon, cut).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    assert_eq!(LiveCache::load(&live), LiveCache::default());
    assert!(LiveCache::default().save(&live).is_err());
    assert_eq!(DaemonState::load(&daemon), DaemonState::default());
    assert!(DaemonState::default().save(&daemon).is_err());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&live).unwrap(), cut);
    assert_eq!(std::fs::read(&daemon).unwrap(), cut);
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 2);

    let unreadable = dir.path().join("daemon-root.json");
    std::fs::write(&unreadable, b"{}").unwrap();
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(DaemonState::load(&unreadable), DaemonState::default());
    assert!(DaemonState::default().save(&unreadable).is_err());
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read(&unreadable).unwrap(), b"{}");
}
