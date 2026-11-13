// SPDX-License-Identifier: GPL-3.0-or-later
//! `readership.json` and `interests.json` are never saved over when they don't parse
//! (state-file safety design): the file stays byte for byte and the error names it.

use deckle_readership::{Learning, State};

const CUT: &[u8] = br#"{"dismissed": {"weather-quiet": "2026-10-"#;

#[test]
fn readership_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("readership.json");
    std::fs::write(&path, CUT).unwrap();
    let _ = State::load(&path);
    let e = State::default().save(&path).unwrap_err();
    assert!(e.contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
}

/// Damaged in a folder where no copy can be kept: the saves are refused and the files stay.
/// Skipped as root.
#[cfg(unix)]
#[test]
fn no_save_when_no_copy_could_be_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let (state, learning) = (ro.join("readership.json"), ro.join("interests.json"));
    std::fs::write(&state, CUT).unwrap();
    std::fs::write(&learning, CUT).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    assert!(State::load(&state).is_err());
    assert!(State::default().save(&state).is_err());
    assert!(Learning::load(&learning).is_err());
    assert!(Learning::default().save(&learning).is_err());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&state).unwrap(), CUT);
    assert_eq!(std::fs::read(&learning).unwrap(), CUT);
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 2);
}

/// It holds the subjects the owner dismissed and accepted.
#[test]
fn interests_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("interests.json");
    std::fs::write(&path, CUT).unwrap();
    let _ = Learning::load(&path);
    let e = Learning::default().save(&path).unwrap_err();
    assert!(e.contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
}
