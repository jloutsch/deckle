# State files that are never silently replaced — Design

Date: 2026-11-12. Status: revised after validation (QA session, same day). Covers issues #26,
#93 (the overwrite; not its "no lock between app and build" item), item 3 of #82, and the file
mode point of #28.

Not covered, and still open: #28 item 1 (keychain) and item 2 (paths from the webview); #82
item 2 (Word Wise `kll.rs:63`, `klld.rs:40` delete the database and build SQLite in place, which
needs build-to-temp-then-rename and is a separate small change); #93's missing lock.

## Why

About 28 state files are read with the same pattern:

```rust
std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
```

A file that is missing and a file that exists but does not parse both become the default value.
The next save then writes the default over the real file. One truncated write, one field a newer
version added in a way an older version cannot read, or one hand edit with a stray comma loses
the whole file: API keys and per-book presets (`settings.json`), every Kindle snapshot and the
shown-highlights history (`shelf.json`), the edition index (`morgue.json`), and so on.

Some of the same files are written with `std::fs::write`, which can itself leave a truncated
file if the process stops part-way.

Loaders with the pattern (found with grep; the implementer re-runs it):

- `deckle-app/src/commands.rs:788` `load_settings_from`
- `deckle-digest`: `shelf/store.rs:91`, `next/live.rs:43`, `puzzle/store.rs:77`
- `deckle-morgue/src/state.rs:38`
- `deckle-library`: `writeback.rs:38` and `:131`, `config.rs:61`, `upgrade.rs:141`,
  `shelves.rs:47`
- `deckle-core`: `devices.rs:127`, `koreader.rs:229`, `koreader_plugins.rs:287`
- `deckle-highlights/src/state.rs:29`
- `deckle-readership`: `state.rs:68`, `learn.rs:72`
- `deckle-readerserver`: `sync.rs:49`, `wireless/mod.rs:96`, `wireless/store.rs:238`
  (`readers.json`) and `:231` (shelf rules)
- `deckle-cli/src/daemon.rs:127` (the real `daemon.json` loader; `deckle-digest` `daemon.rs:35`
  is test-only)
- `deckle-library/src/watch.rs:45` (the watch folder's seen list)
- `deckle-core`: `shelves.rs:185`, `reader.rs:194`
- `deckle-digest/src/frost.rs:111`

The chain is split over several lines by rustfmt; validation found 28 such loads with a
three-line search. `reading.rs:233` is **not** one of them for this purpose: `reading.jsonl` is
one JSON value per line and is already read safely (`reading.rs:194-212`); it gets the synced
save only.

## Goals

- A state file that exists but does not parse is never replaced without a copy of it being kept,
  and owner data is never replaced at all. A file that cannot be read is never replaced.
- A save can never leave a half-written file.
- The owner is told which file is damaged and where the untouched copy is.

## Not goals

- Repairing damaged files, schema migration, or a backup history.
- Locking between processes. Two Deckle processes saving the same file is a separate problem
  (the reading queue already has a lock; see #41).
- Caches. A file that can be rebuilt from other data (`next/live.rs` forecast cache, `frost.rs`)
  may keep "unreadable means empty", decided per file below.

## One helper, two calls

A module `state_file` in the new `deckle-limits` crate from the size-limits design.
`deckle-highlights` depends on no Deckle crate today and has `tempfile` only as a
dev-dependency, so no existing crate is reachable by every caller:

```rust
pub enum Loaded<T> {
    Missing,
    Ok(T),
    /// Exists, was read, does not parse. `kept_at` is None when the copy could not be written.
    Damaged { kept_at: Option<PathBuf>, why: String },
    /// Exists but cannot be read (permissions, I/O). Never treated as Missing.
    Unreadable(io::Error),
}

/// Reads and parses `path`. A file that exists but does not parse is copied to
/// `<name>.damaged-<UTC timestamp>` beside it and reported as `Damaged`; it is not removed.
pub fn load<T: DeserializeOwned>(path: &Path) -> Loaded<T>;

/// The same for a file that is not JSON: `parse` decides whether the text is valid.
pub fn load_with<T>(path: &Path, parse: impl Fn(&str) -> Result<T, String>) -> Loaded<T>;

/// Writes `bytes` to a temp file beside `path`, syncs it, and renames it into place.
pub fn save(path: &Path, bytes: &[u8], secret: bool) -> io::Result<()>;
```

`save` is the existing public `deckle-library::write_atomic` (`lib.rs:179`, which already syncs)
moved to the shared crate. The private copies elsewhere (cookbook store, CLI cookbook delivery,
morgue folder, `koreader_backup.rs:530`) are not merged by this design; the two cookbook ones
lack the sync and get it.

`save` takes a `secret: bool`. A secret file (`settings.json`, `bookmarklets.html`,
`bookmarklet.token`, `reader-server.json`, wireless secrets) is **always** written 0600, whatever mode the old file
had, so existing 0644 files are corrected on the next save; this closes the mode point in #28.
Other files keep their existing mode, and a new non-secret file gets the process's default mode,
as `fs::write` gives today. A path that is a symlink is resolved and the target is replaced, so
the link survives. The `.damaged-*` copy holds the bytes that `load` read, is created with
`create_new` so two processes cannot overwrite each other's copy, and gets the source's mode.

**`Unreadable` never saves, in either class**, and the helper enforces it: `save` itself
refuses when the path exists but cannot be read, and `Loaded::may_replace()` is false for
`Unreadable` and for a `Damaged` whose copy failed, so a caller that forgets the rule cannot
bring the overwrite back. There is no copy of the file, so replacing it
would lose it (a root-owned `daemon.json` in a writable folder, the case noted at
`deckle-cli/src/daemon.rs:124`, would otherwise be replaced by the rename). The feature runs
without the file where it can and reports the system error. A `Damaged` whose copy failed is
treated the same way.

`bookmarklet.token` (`deckle-digest/src/bookmarklet.rs:71-82`) is written with plain `fs::write`
and silently regenerated when unreadable or invalid, which breaks every installed bookmarklet.
It is 32 hex characters, not JSON, so it uses `load_with` (damaged means "not 32 hex characters
after trimming", the test at `bookmarklet.rs:73-77`) and `save` as a secret, class A. A new
token is made only when the file is missing, with `create_new` and a re-read, so two processes
starting together end with one token. When it is damaged, the message names the file's path and
says that deleting it makes a new token and that installed bookmarklets must then be installed
again; it is shown on the bookmarklet page (`deckle-app/src/daily.rs:595`) and in the watcher's
log, where today the watcher only writes to stderr (`deckle-cli/src/lib.rs:3863-3866`).

## What each loader does with `Damaged`

Each state type is put in one of two classes.

**What "fails" means.** Only the feature that needs the file fails, not a whole multi-step
command. `reader_sync` (`deckle-cli/src/lib.rs:5140`) loads the puzzle store (`:5166`), runs
`shelves::on_sync` (`:5194`, which loads the library shelves config, `deckle-cli/src/shelves.rs:84`,
and returns nothing) and loads `shelf.json` (`:5197`) after books have been delivered; a damaged
file skips that step with its message printed and the command reports the delivery as done.
For an edition build, a damaged file drops the section that needs it (From Your Shelf for
`shelf.json`, the archive links for `morgue.json`, reading suggestions for the readership and
learning files) with a note in the build output, and the edition still prints. The implementer
maps each class A file to its section from the call sites; that mapping was not read for this
design.

**Class A, owner data: stop and say so.** The load returns an error, the feature that needs the
file fails with a message, and nothing is written:

> settings.json could not be read (line 14: expected `,`). Deckle has not changed it. A copy is
> at settings.json.damaged-20261112T140355Z. Fix the file or remove it to start again.

Class A: `settings.json`, `shelf.json`, `morgue.json`, library `config`, `upgrades`, `shelves`,
`writeback`, `ReadLog`, `devices`, highlights `State`, readership `State`, the readership
learning file (`learn.rs:55-60`: it holds the owner's dismissed and accepted subjects),
reader-server sync progress, wireless secrets, `readers.json`, `bookmarklet.token`, the watch
folder's seen list (losing it re-adds every EPUB), `koreader.rs:229`, the puzzle store.

The watch folder needs UI work: the frontend swallows every watch-scan error today
(`App.tsx:246-253`, an empty `catch`), so a damaged seen list would stop the watcher with no
sign. The scan's error is shown where the watch folder is configured.

For the desktop app, "the command fails" must not mean a blank window. `get_settings` returns the
defaults **for display** together with a `damaged` field; the Settings view shows the message in
a banner; every save of that file is refused while it is damaged. The same shape for the Daily
tab's files. With the watch-folder message above, this is the UI work in the design.

**Class B, rebuildable: start empty and log one line.** `next/live.rs` (forecast cache),
`frost.rs`, `daemon.json` (`decide` in `deckle-digest/src/daemon.rs:83-110` can repeat one
edition from an empty state and never skips one), KOReader plugin sources, wireless shelf rules
(`store.rs:231`), `reader.rs:194` (its caller only prints), and the list of shelves Deckle made
on a device (`deckle-core/src/shelves.rs:185`): with an empty list, `diff` (`shelves.rs:147-179`)
only creates shelves and adds books and never removes, and class A would block shelf sync until
the owner deleted a hidden file on the reader. It gets a per-device note in the output.
The damaged copy is still kept.

As built, `readers.json` is class B, not class A: it holds last-seen times, a log, the discovery flag and the calibre collections Deckle made, and losing the collections list is the same create-only trade as the device shelves list above. The reMarkable host-key pin is class A: a damaged pin refuses the connection.

The implementer confirms each file's class by reading what it holds; the lists above are from the
type names and the scan reports, not from reading every type.

## Order of work

1. The helper, with `settings.json` (#26) and the Settings banner.
2. `shelf.json` (#93) and `morgue.json` (#82 item 3).
3. The remaining class A files, one crate per pull request.
4. Class B files.

## Tests

For the helper: missing gives `Missing`; valid gives `Ok`; truncated gives `Damaged` and the
`.damaged-*` copy has the original bytes; `save` interrupted before the rename (simulated by
failing the rename) leaves the old file intact.

For each class A file: load a truncated file, attempt the type's save path, assert the file on
disk is byte-identical to the truncated one and the error names it.

One guard test fails on a new silent load outside an allowed list. A single-line pattern matches
nothing, because rustfmt splits the chain: the test reads each `.rs` file whole and uses a
multi-line regex for `fs::read` or `read_to_string`, then `.ok()`, then `from_slice` or
`from_str`, then `unwrap_or_default` within five lines. The test is first run against `main` and
must report the known loads, to prove the pattern matches (validation's regex found 25 of the
28). Two
shapes it cannot match are listed by hand in the test: the `match … unwrap_or_else` in
`deckle-cli/src/daemon.rs:127` and the `if let Ok` in `bookmarklet.rs:73`.

## What a user sees

A damaged file produces a message with the file's name, the parse error and the path of the
copy. Nothing is lost. CHANGELOG entry; one line in the README's troubleshooting section if it
has one, otherwise in `docs/how-it-works.md`.

## Open questions

1. Should the app offer "start again with defaults" as a button when a file is damaged? This
   design leaves it to the owner to move the file, so that no click can lose data.
2. Old `.damaged-*` copies are never removed by Deckle. Each is at most the size of the state
   file, and one is written per failed load of an unchanged damaged file only if none with the
   same content exists (compare by size and hash before copying).

## Not verified

The classes and line numbers added in the revision are the validator's readings; I did not
re-read those types.
