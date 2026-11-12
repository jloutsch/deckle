//! State files that are never silently replaced (state-file safety design).
//!
//! [`load`] tells a missing file from one that does not parse and from one that cannot be read,
//! and keeps a copy of a damaged file before anything can replace it. [`save`] writes through a
//! temp file, so a save never leaves a half-written file.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;

/// What [`load`] found at a path.
#[derive(Debug)]
pub enum Loaded<T> {
    Missing,
    Ok(T),
    /// Exists, was read, does not parse. `kept_at` is None when the copy could not be written.
    Damaged {
        kept_at: Option<PathBuf>,
        why: String,
    },
    /// Exists but cannot be read (permissions, I/O). Never treated as Missing.
    Unreadable(io::Error),
}

/// Reads and parses `path`. A file that exists but does not parse is copied to
/// `<name>.damaged-<UTC timestamp>` beside it and reported as `Damaged`; it is not removed.
pub fn load<T: DeserializeOwned>(path: &Path) -> Loaded<T> {
    load_with(path, |text| {
        serde_json::from_str(text).map_err(|e| e.to_string())
    })
}

/// The same for a file that is not JSON: `parse` decides whether the text is valid.
///
/// Text that is not UTF-8 is `Damaged`. The copy is not made again when a `.damaged-*` copy
/// with the same bytes is already beside the file; `kept_at` then names that copy.
pub fn load_with<T>(path: &Path, parse: impl Fn(&str) -> Result<T, String>) -> Loaded<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Unreadable(e),
    };
    let why = match std::str::from_utf8(&bytes) {
        Ok(text) => match parse(text) {
            Ok(value) => return Loaded::Ok(value),
            Err(why) => why,
        },
        Err(e) => format!("not UTF-8 text ({e})"),
    };
    Loaded::Damaged {
        kept_at: keep_copy(path, &bytes).ok(),
        why,
    }
}

/// Copies the damaged file at `path`, whose content is `bytes`, to
/// `<name>.damaged-<UTC timestamp>` beside it, unless a copy with the same bytes is already
/// there. Returns the copy's path.
fn keep_copy(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let dir = parent_of(path);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let mut prefix = name.to_os_string();
    prefix.push(".damaged-");

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let same_name = entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(prefix.as_encoded_bytes());
            let same_size = entry
                .metadata()
                .is_ok_and(|m| m.len() == bytes.len() as u64);
            if same_name && same_size && fs::read(entry.path()).is_ok_and(|b| b == bytes) {
                return Ok(entry.path());
            }
        }
    }

    let stamp = utc_stamp(SystemTime::now());
    let named = |suffix: &str| {
        let mut n = prefix.clone();
        n.push(&stamp);
        n.push(suffix);
        dir.join(n)
    };
    let mut copy = named("");
    let mut k = 2;
    while copy.symlink_metadata().is_ok() {
        copy = named(&format!("-{k}"));
        k += 1;
    }
    // `fs::copy` keeps the file's mode, so a secret file's copy stays private.
    if let Err(e) = fs::copy(path, &copy) {
        let _ = fs::remove_file(&copy);
        return Err(e);
    }
    Ok(copy)
}

/// The folder `path` is in; `.` for a bare file name.
fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Writes `bytes` to a temp file beside `path`, syncs it, and renames it into place.
///
/// A `secret` file is always left 0600, whatever mode the old file had. Any other file keeps
/// the old file's mode; a new one is 0600. Missing folders are made.
pub fn save(path: &Path, bytes: &[u8], secret: bool) -> io::Result<()> {
    save_with(path, bytes, secret, |from, to| fs::rename(from, to))
}

/// [`save`] with the rename passed in, so a test can make it fail.
fn save_with(
    path: &Path,
    bytes: &[u8],
    secret: bool,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let named = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    let dir = parent_of(path);
    fs::create_dir_all(dir).map_err(named)?;
    // Created 0600 on Unix.
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(named)?;
    tmp.write_all(bytes).map_err(named)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if secret {
            Some(0o600)
        } else {
            fs::metadata(path)
                .ok()
                .map(|m| m.permissions().mode() & 0o7777)
        };
        if let Some(mode) = mode {
            tmp.as_file()
                .set_permissions(fs::Permissions::from_mode(mode))
                .map_err(named)?;
        }
    }
    #[cfg(not(unix))]
    let _ = secret;
    tmp.as_file().sync_all().map_err(named)?;
    // From here the temp file is removed when `tmp` drops, unless the rename took it.
    let mut tmp = tmp.into_temp_path();
    rename(&tmp, path).map_err(named)?;
    tmp.disable_cleanup(true);
    // Make the rename itself durable. Not every system can sync a folder; the file is in place
    // either way.
    #[cfg(unix)]
    let _ = fs::File::open(dir).and_then(|d| d.sync_all());
    Ok(())
}

/// `20261112T140355Z` for a time.
fn utc_stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// Year, month and day of a day count since 1970-01-01, in the proleptic Gregorian calendar
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::time::Duration;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Settings {
        name: String,
        count: u32,
    }

    const VALID: &str = r#"{"name": "Deckle", "count": 3}"#;
    const TRUNCATED: &str = r#"{"name": "Deckle", "co"#;

    fn damaged_copies(dir: &Path, name: &str) -> Vec<PathBuf> {
        let prefix = format!("{name}.damaged-");
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
            .collect();
        found.sort();
        found
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_missing_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load::<Settings>(&dir.path().join("settings.json"));
        assert!(matches!(loaded, Loaded::Missing), "{loaded:?}");
        let loaded = load::<Settings>(&dir.path().join("no/such/settings.json"));
        assert!(matches!(loaded, Loaded::Missing), "{loaded:?}");
    }

    #[test]
    fn a_valid_file_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, VALID).unwrap();
        match load::<Settings>(&path) {
            Loaded::Ok(s) => assert_eq!(
                s,
                Settings {
                    name: "Deckle".into(),
                    count: 3
                }
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(names_in(dir.path()), ["settings.json"]);
    }

    #[test]
    fn a_truncated_file_is_damaged_and_a_copy_keeps_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, TRUNCATED).unwrap();
        let Loaded::Damaged { kept_at, why } = load::<Settings>(&path) else {
            panic!("not damaged")
        };
        let kept = kept_at.expect("a copy was kept");
        assert_eq!(kept.parent(), Some(dir.path()));
        let name = kept.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("settings.json.damaged-"), "{name}");
        let stamp = &name["settings.json.damaged-".len()..];
        assert_eq!(stamp.len(), "20261112T140355Z".len(), "{stamp}");
        assert!(
            stamp.ends_with('Z') && stamp.as_bytes()[8] == b'T',
            "{stamp}"
        );
        assert_eq!(fs::read(&kept).unwrap(), TRUNCATED.as_bytes());
        assert_eq!(
            fs::read(&path).unwrap(),
            TRUNCATED.as_bytes(),
            "not removed"
        );
        assert!(why.contains("EOF"), "{why}");
    }

    #[test]
    fn the_same_damage_is_copied_once_and_new_damage_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shelf.json");
        fs::write(&path, TRUNCATED).unwrap();
        let first = match load::<Settings>(&path) {
            Loaded::Damaged { kept_at, .. } => kept_at.unwrap(),
            other => panic!("{other:?}"),
        };
        let second = match load::<Settings>(&path) {
            Loaded::Damaged { kept_at, .. } => kept_at.unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(first, second);
        assert_eq!(
            damaged_copies(dir.path(), "shelf.json"),
            std::slice::from_ref(&first)
        );

        // Different damage, within the same second: a second copy, not an overwrite.
        fs::write(&path, "{ not json").unwrap();
        let third = match load::<Settings>(&path) {
            Loaded::Damaged { kept_at, .. } => kept_at.unwrap(),
            other => panic!("{other:?}"),
        };
        assert_ne!(third, first);
        assert_eq!(fs::read(&first).unwrap(), TRUNCATED.as_bytes());
        assert_eq!(fs::read(&third).unwrap(), b"{ not json");
        assert_eq!(damaged_copies(dir.path(), "shelf.json").len(), 2);
    }

    #[test]
    fn load_with_takes_any_parser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarklet.token");
        let token = |text: &str| {
            let t = text.trim();
            if t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
                Ok(t.to_string())
            } else {
                Err("not 32 hex characters".to_string())
            }
        };
        fs::write(&path, "0123456789abcdef0123456789abcdef\n").unwrap();
        assert!(
            matches!(load_with(&path, token), Loaded::Ok(t) if t == "0123456789abcdef0123456789abcdef")
        );
        fs::write(&path, "0123").unwrap();
        assert!(matches!(
            load_with(&path, token),
            Loaded::Damaged { kept_at: Some(_), why } if why == "not 32 hex characters"
        ));
    }

    #[test]
    fn text_that_is_not_utf8_is_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{\"name\": \"\xff\"}").unwrap();
        assert!(matches!(
            load::<Settings>(&path),
            Loaded::Damaged {
                kept_at: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_folder_where_the_file_should_be_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::create_dir(&path).unwrap();
        assert!(matches!(load::<Settings>(&path), Loaded::Unreadable(_)));
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_read_permission_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.json");
        fs::write(&path, VALID).unwrap();
        set_mode(&path, 0o000);
        if fs::read(&path).is_ok() {
            eprintln!("skipped: running as root, permissions do not stop a read");
            return;
        }
        let loaded = load::<Settings>(&path);
        assert!(
            matches!(&loaded, Loaded::Unreadable(e) if e.kind() == io::ErrorKind::PermissionDenied),
            "{loaded:?}"
        );
        assert!(damaged_copies(dir.path(), "daemon.json").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_that_cannot_be_written_is_reported_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("state");
        fs::create_dir(&sub).unwrap();
        let path = sub.join("settings.json");
        fs::write(&path, TRUNCATED).unwrap();
        set_mode(&sub, 0o555);
        let probe = sub.join("probe");
        if fs::write(&probe, b"").is_ok() {
            eprintln!("skipped: running as root, the folder stays writable");
            return;
        }
        let loaded = load::<Settings>(&path);
        set_mode(&sub, 0o755);
        assert!(
            matches!(&loaded, Loaded::Damaged { kept_at: None, .. }),
            "{loaded:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), TRUNCATED.as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn the_damaged_copy_keeps_the_files_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, TRUNCATED).unwrap();
        set_mode(&path, 0o600);
        let Loaded::Damaged {
            kept_at: Some(kept),
            ..
        } = load::<Settings>(&path)
        else {
            panic!("not damaged")
        };
        assert_eq!(mode_of(&kept), 0o600);
    }

    #[test]
    fn save_writes_the_bytes_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new/folder/morgue.json");
        save(&path, b"{}", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{}");
        save(&path, VALID.as_bytes(), false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), VALID.as_bytes());
        assert_eq!(names_in(path.parent().unwrap()), ["morgue.json"]);
    }

    #[test]
    fn a_failed_rename_leaves_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shelf.json");
        fs::write(&path, VALID).unwrap();
        let e = save_with(&path, b"new", false, |_, _| {
            Err(io::Error::other("rename failed"))
        })
        .unwrap_err();
        assert!(e.to_string().contains("shelf.json"), "{e}");
        assert!(e.to_string().contains("rename failed"), "{e}");
        assert_eq!(fs::read(&path).unwrap(), VALID.as_bytes());
        assert_eq!(
            names_in(dir.path()),
            ["shelf.json"],
            "the temp file is gone"
        );
    }

    #[test]
    fn the_temp_file_is_complete_and_beside_the_path_when_renamed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readers.json");
        save_with(&path, b"all of it", false, |from, to| {
            assert_eq!(from.parent(), to.parent());
            assert_eq!(fs::read(from).unwrap(), b"all of it");
            fs::rename(from, to)
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"all of it");
    }

    #[cfg(unix)]
    #[test]
    fn a_secret_file_ends_0600_even_when_it_was_0644() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, VALID).unwrap();
        set_mode(&path, 0o644);
        save(&path, b"{}", true).unwrap();
        assert_eq!(mode_of(&path), 0o600);

        let fresh = dir.path().join("bookmarklet.token");
        save(&fresh, b"0123", true).unwrap();
        assert_eq!(mode_of(&fresh), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_is_not_secret_keeps_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        for mode in [0o644, 0o640, 0o600] {
            let path = dir.path().join(format!("shelf-{mode:o}.json"));
            fs::write(&path, VALID).unwrap();
            set_mode(&path, mode);
            save(&path, b"{}", false).unwrap();
            assert_eq!(mode_of(&path), mode);
        }
        let fresh = dir.path().join("fresh.json");
        save(&fresh, b"{}", false).unwrap();
        assert_eq!(mode_of(&fresh), 0o600);
    }

    #[test]
    fn the_stamp_is_utc() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
        let at = |s| UNIX_EPOCH + Duration::from_secs(s);
        assert_eq!(utc_stamp(at(1_794_492_235)), "20261112T140355Z");
        assert_eq!(utc_stamp(at(951_868_799)), "20000229T235959Z");
    }
}
