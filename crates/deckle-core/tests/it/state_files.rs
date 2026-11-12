// SPDX-License-Identifier: GPL-3.0-or-later
//! deckle-core's owner-data files are never saved over when they don't parse (state-file
//! safety design): the file stays byte for byte and the error names it.

use deckle_core::devices::DeviceRegistry;
use deckle_core::koreader::documents;

const CUT: &[u8] = br#"{"profiles": [{"key": "G000 0000 0000 0000", "cde_override": "eb"#;

/// The owner's per-Kindle choices (EBOK or PDOC, a verified ASIN).
#[test]
fn devices_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("devices.json");
    std::fs::write(&path, CUT).unwrap();
    let _ = DeviceRegistry::load(&path);
    let e = DeviceRegistry::default().save(&path).unwrap_err();
    assert!(e.contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
}

/// Damaged in a folder where no copy can be kept (`may_replace` false): the save paths are
/// refused and the file stays. Skipped as root.
#[cfg(unix)]
#[test]
fn no_save_when_no_copy_could_be_kept() {
    use std::os::unix::fs::PermissionsExt;
    for name in ["devices.json", "koreader-documents.json"] {
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
        if name == "devices.json" {
            assert!(DeviceRegistry::load(&path).is_err());
            assert!(DeviceRegistry::default().save(&path).is_err());
        } else {
            assert!(documents::record(&path, "a.epub", b"EPUB").is_err());
        }
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), CUT, "{name}");
        assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 1, "{name}");
    }
}

/// The books handed to a KOReader, by the names its sync gives them.
#[test]
fn koreader_documents_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("koreader-documents.json");
    std::fs::write(&path, CUT).unwrap();
    let e = documents::record(&path, "deckle-daily-2026-11-12.kepub.epub", b"EPUB").unwrap_err();
    assert!(e.to_string().contains(&path.display().to_string()), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
}

// ---- rebuildable files (class B): start empty, said, with a copy kept ----

use deckle_core::shelves::{Diff, Shelves, ours, record_ours};
use std::collections::BTreeSet;

/// The shelves Deckle made on a reader: a damaged list starts empty (diff then never removes),
/// its copy keeps the bytes, and the next record replaces it.
#[test]
fn deckle_shelves_json_starts_empty_with_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deckle-shelves.json");
    std::fs::write(&path, b"[\"Sci-Fi\", \"Fan").unwrap();
    assert!(ours(&path).is_empty());
    let copy = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("deckle-shelves.json.damaged-")
        })
        .expect("a copy");
    assert_eq!(std::fs::read(copy.path()).unwrap(), b"[\"Sci-Fi\", \"Fan");
    let wanted: Shelves = [("New".to_owned(), BTreeSet::new())].into();
    let d = Diff {
        create: vec!["New".into()],
        ..Default::default()
    };
    record_ours(&path, &BTreeSet::new(), &d, &wanted).unwrap();
    assert_eq!(ours(&path).into_iter().collect::<Vec<_>>(), ["New"]);
}

/// Damaged where no copy can be kept (`may_replace` false), or unreadable: never saved over.
#[cfg(unix)]
#[test]
fn a_rebuildable_file_with_no_copy_or_unreadable_is_never_saved_over() {
    use std::os::unix::fs::PermissionsExt;
    let wanted: Shelves = [("New".to_owned(), BTreeSet::new())].into();
    let d = Diff {
        create: vec!["New".into()],
        ..Default::default()
    };

    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let path = ro.join("deckle-shelves.json");
    std::fs::write(&path, CUT).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"").is_ok() {
        eprintln!("skipped: running as root");
        return;
    }
    assert!(ours(&path).is_empty());
    assert!(record_ours(&path, &BTreeSet::new(), &d, &wanted).is_err());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), CUT);
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 1);

    // Unreadable (no read permission), in a writable folder: still not replaced.
    let path = dir.path().join("unreadable.json");
    std::fs::write(&path, b"[\"Mine\"]").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(ours(&path).is_empty());
    assert!(record_ours(&path, &BTreeSet::new(), &d, &wanted).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"[\"Mine\"]");
}
