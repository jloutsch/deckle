// SPDX-License-Identifier: GPL-3.0-or-later
//! `wireless.json` is never saved over when it doesn't parse (state-file safety design): a new
//! library id would make every reader see a different library.

use deckle_readerserver::wireless;

/// Damaged in a folder where no copy can be kept: the save paths are refused and the files
/// stay. Skipped as root.
#[cfg(unix)]
#[test]
fn no_save_when_no_copy_could_be_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let secrets = wireless::secrets_path(&ro.join("digest.toml"));
    let sync = ro.join("koreader-sync.json");
    let cut = br#"{"password": "made-up", "library_uuid": "0b6c"#;
    std::fs::write(&secrets, cut).unwrap();
    std::fs::write(&sync, cut).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    assert!(wireless::load_secrets(&secrets).is_err());
    assert!(wireless::save_secrets(&secrets, &wireless::Secrets::default()).is_err());
    assert!(deckle_readerserver::sync::load(&sync).is_err());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&secrets).unwrap(), cut);
    assert_eq!(std::fs::read(&sync).unwrap(), cut);
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 2);
}

#[test]
fn wireless_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = wireless::secrets_path(&dir.path().join("digest.toml"));
    let cut = br#"{"password": "made-up", "library_uuid": "0b6c"#;
    std::fs::write(&path, cut).unwrap();
    assert!(wireless::load_secrets(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), cut);
    let e = wireless::save_secrets(&path, &wireless::Secrets::default()).unwrap_err();
    assert!(e.to_string().contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), cut);
}

/// `readers.json` is rebuildable (each reader fills its entry again when it connects): a
/// damaged one starts empty with a copy kept, and the next log line replaces it.
#[test]
fn readers_json_starts_empty_with_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let store = wireless::Store::new(dir.path());
    let path = dir.path().join("readers.json");
    std::fs::write(&path, b"{\"readers\": {\"x").unwrap();
    assert!(store.readers().readers.is_empty());
    store.log("a line");
    assert_eq!(store.readers().log, ["a line"]);
    let copies: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("readers.json.damaged-")
        })
        .map(|e| std::fs::read(e.path()).unwrap())
        .collect();
    assert_eq!(copies, [b"{\"readers\": {\"x".to_vec()]);
}

/// Damaged where no copy can be kept: the log line is not written over it. Skipped as root.
#[cfg(unix)]
#[test]
fn readers_json_with_no_copy_is_never_saved_over() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let path = ro.join("readers.json");
    std::fs::write(&path, b"{\"readers\": {\"x").unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    wireless::Store::new(&ro).log("a line");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"readers\": {\"x");
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 1);
}
