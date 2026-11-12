// SPDX-License-Identifier: GPL-3.0-or-later
//! `morgue.json` is never saved over when it doesn't parse (#82 item 3).

use deckle_morgue::state::MorgueState;

const CUT: &[u8] = br#"{"imported": true, "resent": {"2026-09-30": {"queued": "2026-"#;

#[test]
fn a_damaged_morgue_file_is_an_error_and_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("morgue.json");
    std::fs::write(&path, CUT).unwrap();
    let e = MorgueState::load(&path).unwrap_err();
    assert!(e.contains("morgue.json.damaged-"), "{e}");
    assert!(MorgueState::default().save(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
}

/// Damaged in a folder where no copy can be kept: the save is refused. Skipped as root.
#[cfg(unix)]
#[test]
fn no_save_when_no_copy_could_be_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let path = ro.join("morgue.json");
    std::fs::write(&path, CUT).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    assert!(MorgueState::load(&path).is_err());
    assert!(MorgueState::default().save(&path).is_err());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 1);
}
