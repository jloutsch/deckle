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
