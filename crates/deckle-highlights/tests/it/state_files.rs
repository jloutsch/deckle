// SPDX-License-Identifier: GPL-3.0-or-later
//! `highlights.json` is never saved over when it doesn't parse (state-file safety design): it
//! holds the folder and which highlights each book's file already has.

use deckle_highlights::State;

const CUT: &[u8] = br#"{"folder": "/made/up/Highlights", "books": {"Walden": {"file": "Wal"#;

/// Damaged in a folder where no copy can be kept: the export and the save are refused and the
/// file stays. Skipped as root.
#[cfg(unix)]
#[test]
fn no_save_when_no_copy_could_be_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let path = ro.join("highlights.json");
    std::fs::write(&path, CUT).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    assert!(deckle_highlights::export_clippings(b"", &path, Some(&dir.path().join("H"))).is_err());
    assert!(State::default().save(&path).is_err());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 1);
}

#[test]
fn highlights_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("highlights.json");
    std::fs::write(&path, CUT).unwrap();
    let to = dir.path().join("Highlights");
    let e = deckle_highlights::export_clippings(b"", &path, Some(&to)).unwrap_err();
    assert!(e.contains(&path.display().to_string()), "{e}");
    let e = State::default().save(&path).unwrap_err();
    assert!(e.to_string().contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
}
