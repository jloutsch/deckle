# Device delivery that cannot lose the copy already there — Design

Date: 2026-11-12. Status: revised after validation (QA session, same day). Covers issues #74, #75, #33 and item 1 of #32.

## Why

Four places remove or overwrite something on a reader before the replacement is known to be
safe, or stop all delivery because of one bad item:

1. **Re-send over an existing file** (#74). `write_file` (`deckle-core/src/deliver.rs:370`) opens
   the final path with `File::create`, which empties the old copy, then writes. A failed write
   leaves a partial file under the book's name and no old copy. On MTP, `deliver_mtp` (line 412)
   deletes the existing object and then sends; `inventory::copy_in` does the same.
2. **Put back** (#33). `deckle-library/src/upgrade.rs:234-241` removes the upgraded copy from the
   Kindle, then copies the kept old one in. If the copy fails, the Kindle has neither.
3. **Outbox flush** (#75). `Queue::flush_with` (`deckle-core/src/queue.rs:490-515`) returns on the
   first entry that fails. Entries after it never go, on every later push, and the books delivered
   before it are in no report.
4. **MTP download** (#32 item 1). `deckle-transport/src/session.rs:85-96` returns a short payload
   as success. Callers treat it as the whole file. It belongs here because callers of the
   download are the same device code.

## Goals

- After any failed send, the reader holds either the complete old copy or the complete new one,
  never a partial file under the book's name and never neither.
- One bad outbox entry costs only that entry.
- What a command prints about the old copy is true.

## Not goals

- Transactions across several files (a book and its sidecars). Each file is made safe on its own;
  the order below keeps the book itself safe first.
- Any change to file names, folders or what gets pruned.

## Mass storage and folders: write beside, then rename

`reader::deliver` (`deckle-core/src/reader.rs:216-233`) already writes `.{name}.tmp` and renames.
That routine is moved to one shared function and `write_file(path, data)` calls it:

1. Write to `<dir>/.<name>.<pid>.part` (create, write, `sync_all`). The process id keeps two
   Deckle processes from sharing a temp file.
2. `rename` it onto `path`. On the same volume this replaces the old file in one step on macOS,
   Linux and Windows (Rust's `rename` replaces on all three).
3. On any error in 1 or 2, remove the `.part` file and return the error. The old file was never
   touched.

A leftover temp file from a pulled cable is removed at the start of the next delivery to that
folder: `.*.part`, and the older `.*.tmp` that `reader::deliver` writes today, older than one
hour (the age test keeps a second Deckle process's live write safe), in the Kindle's
`documents/` and in the Kobo and PocketBook `Deckle/` folder.

The Kindle ignores dot-files in `documents/`; this is my understanding from the existing `.sdr`
handling and is the one thing to confirm on a real Kindle before release.

Validation ran this on macOS against FAT32, exFAT and APFS images: rename over an existing file,
a case-only rename, rename over a read-only file and a 126-character temp name all work. Linux
vfat and Windows were not tested; if rename is refused there, both files are intact and step 3
cleans up.

`inventory::copy_in`'s mass-storage branch uses plain `fs::write` (`inventory.rs:811`, `:817`)
and moves to the same function; `care::restore_on` (`care.rs:399`) reaches it through `copy_in`.

Space: the new copy and the old one exist together for a moment, so a re-send needs free space
for one more copy. When the write fails with "no space", the error says so and the old copy is
intact, which is the point of the change.

## MTP: check space, send, then delete the old object

MTP has no rename-over and the Kindle rejects two objects with one name in a folder, so
"write beside" is done with a temporary name:

0. Look for leftover `<name>.*.part` objects. Each send uses its own temp name,
   `<name>.<n>.part`, so a leftover never blocks a new send. If `<name>` exists, leftovers are
   failed sends: delete them. If `<name>` is **missing**, a leftover may be the only copy on the
   device (step 6 below) or a truncated one; it cannot be told which, since no expected size is
   stored. It is left alone, under its `.part` name, until the new copy has been renamed into
   place, and deleted then. A truncated file is never given the book's real name.
1. Ask the device for free space (`GetStorageInfo`). The space needed is the new file plus its
   sidecars plus a 1 MB margin; with the rename route the old copy still exists during the send,
   so the test is `free >= new + sidecars + margin`, and with the fallback route (old copy deleted
   first) it is `free + old >= new + sidecars + margin`, so a re-send that fits today is not refused. If it does not
   fit, fail before touching anything: "not enough room on the Kindle for <name>". If
   `GetStorageInfo` itself fails, continue without the check, as `inventory.rs:424` does.
2. Send the new file as `<name>.<n>.part`.
3. Delete the existing object `<name>`, if any.
4. Rename `<name>.part` to `<name>` with MTP `SetObjectPropValue(ObjectFileName)`.
5. If step 2 fails: delete the `.part` object if it exists; the old copy is intact.
6. If step 4 fails after step 3 succeeded: the new copy is complete on the device under
   `<name>.part`. Report exactly that. It stays until the next delivery of that book succeeds.

Whether the Kindle's MTP stack accepts `SetObjectPropValue` for the file name is not known, and
it can only be learned on a real Kindle. `deckle-transport` has no rename today, and
`DeviceInfo::decode` drops the supported-operations list (`ptp.rs:214-219`); the implementer adds
both. The fake device (`fake_mtp.rs:279-281`) answers OK to unknown opcodes, so it must first be
changed to refuse them, to refuse duplicate names, and to inject a send failure; otherwise the
tests below pass falsely. **If rename is not supported**, the fallback is: step 1 (space check), then delete,
then send, as today, but with the space check and with the error wording corrected. That removes
the "disk full" loss, which is the common one, and leaves "cable pulled mid-send" as a known
limit written in `docs/devices.md`.

`inventory::copy_in` uses the same routine. Two kinds of file stay delete-then-send and are
named as a known limit: Word Wise system files (`deliver.rs:455-458`) and `.sdr` contents
(`inventory.rs:798-801`). Word Wise files can be built again. For `.sdr` contents put back by `copy_in`, the kept copy on
the computer is the safety net: a failed send leaves it there to retry.

## Put back: the book first, its reading data last

`sdr_name` is `with_extension("sdr")` (`inventory.rs:134`), so `X.azw3` and `X.kfx` share one
`X.sdr/` folder, and removing a book removes that folder (`inventory.rs:612-616`; MTP `:646-652`).
Copying the old `.sdr` in before removing the upgraded book would delete the restored reading
data. The order is therefore:

1. Copy the kept old **book file** in (safe, by the routine above).
2. Remove the upgraded copy when its name differs from the old one. This removes `X.sdr/`.
3. Copy the kept old `.sdr` in.

`copy_in` is split into a book-only part and an `.sdr`-only part for this.

When the names are equal, step 1 is the replacement and step 2 is skipped. A failure at step 2
other than "refused" returns an error and keeps the record (`remove_on` returning Refused is
already ignored, `upgrade.rs:237`). If step 1 fails,
nothing has changed on the Kindle and the upgrade record stays. If step 3 fails, the book is back
and the record stays, so the owner can retry; the message says the reading position was not
restored.

## Outbox flush: one entry's failure is a warning

The rule applies to **both** loops: `Queue::flush_with` (Kindle) and the loop in `run_reader`
(`sync.rs:300`, Kobo and PocketBook), whose `?` at `:334`, `:337` and `:356` stops the push today.
An error for one entry is recorded and the loop continues:

- `FlushReport` gains `failed: Vec<(String, String)>` (entry name, reason).
- The entry stays in the outbox, so the next push retries it. Each failure is printed on every
  push with the entry's name, so the owner can remove it.
- **Which errors stop the loop.** `DeliverError::Mtp(String)` (`deliver.rs:111`) mixes a lost
  session, a refused file and a full Kindle, so the variant alone cannot decide. The error keeps
  the transport's `MtpError` kind: a transport-level error (the session or cable is gone) and a
  missing volume root stop the loop; a response code from the device (refused, full) and an I/O
  error on the entry's own path skip only that entry.
- The function returns `Ok(report)` when the loop ran; callers print `failed` after `delivered`.
- Every step after the loop is a warning in the report, never an error that drops the delivered
  list: the prune in `run_with` (`sync.rs:258`), the prune in `run_reader` (`sync.rs:396-401`),
  and in `flush_with` the twin removal (`queue.rs:511`) and its own prune (`queue.rs:516-517`).
- `run_reader` labels a device write error with the queue entry's path (`sync.rs:327-330`,
  `:337`), so an unplugged Kobo would look like one failure per entry. Its stop test, checked after any entry fails, is whether the reader's marker is still there
  (`.kobo/version`, `kobo.rs:48`; PocketBook `system/explorer-3` or `system/config`,
  `reader.rs:172`), not whether the root folder exists: a fixed mount point (`DECKLE_MOUNTS`, a
  Linux fstab entry) leaves an empty folder after unplug, and `reader::deliver` would then
  create `Deckle/` on the computer's own disk and report a delivery. The Kindle's "missing
  volume" test has the same gap (`write_file` runs `create_dir_all`, `deliver.rs:376`) and
  checks `documents/` and `system/` together, as `detect_in` does. Both checks also run before
  the first write of a push.

**After the book is on the device, later errors are warnings.** In `deliver` a sidecar,
system-file or thumbnail error after the rename returns `Err` today (`deliver.rs:315-326`); the
pipeline then records no delivery (`pipeline.rs:882-891`) and reprocess prints "the old copy is
still there" (`deckle-cli/src/lib.rs:2492`) while the new copy is in place. The delivery result
gains a `warnings` list; once the book file is renamed into place the delivery is recorded, and
the message about the old copy is printed only when the book file itself was not replaced.

**Twins.** The twin rule (a PDF dropped because a `.kfx` of the same stem is queued) removes a
twin only after its partner was delivered in this flush. When the partner fails with a per-entry
error, the twin is delivered in its place after the loop, the failed partner is **removed from
the queue**, and the report says so; keeping both queued would deliver both forms on a later
push and prune an older edition one push early (`deliver.rs:506-511` counts both). After a
transport-error stop, nothing more is sent and both stay queued. The progress total
(`pending.len()`, `queue.rs:496`) counts such a twin. Narrowing it to dated paper names (#75 item 2) needs paper prefixes that `Finishing`
(`queue.rs:95-105`) does not carry; the implementer passes the prefixes in, as `run_reader` gets
its `&[Prune]`.

## MTP download: short is an error

The check goes in `read_container` (`session.rs:85-96`): fewer bytes than the data phase
announced is an error. An announced length of `0xFFFFFFFF` means unknown and is not checked.

## Tests

- `write_file` over an existing file with a writer that fails half-way (a file size limit, as the
  reviewer used): the old bytes are still there and no `.part` remains.
- The same through `deliver_named_on` on a folder target.
- MTP with the crate's fake device: space too small fails before any delete; a send that fails
  leaves the old object; rename unsupported takes the fallback.
- `put_back` with a copy-in that fails: both the upgraded copy and the record remain.
- `put_back` from `X.kfx` to `X.azw3`: the restored `X.sdr/` is on the device afterwards.
- `run_reader` with a bad entry between two good ones: two delivered, one in `failed`.
- A delivery whose thumbnail write fails: recorded as delivered, with a warning.
- A leftover `<name>.part` on the fake device beside `<name>`: the next send succeeds.
- A leftover `.part` with no `<name>`, then a send that fails at step 2: nothing is under
  `<name>` and the leftover is still there. Then a send that succeeds: the leftover is gone.
- A twin whose partner fails: the twin is delivered and the partner is no longer queued.
- A fixed mount point with the reader unplugged: nothing is written and the push stops.
- A Kobo unplugged mid-push: the loop stops with one error, not one per entry.
- `flush_with` with a good entry, an empty `.book` folder, another good entry and a dated paper:
  two delivered, one in `failed` and still queued, paper delivered.
- `flush_with` with a book and a PDF of the same stem: both delivered.
- `get_object` with a transport that announces 1,000 bytes and gives 100: error.

## What a user sees

- A failed re-send says the old copy is still on the reader, and it is.
- A push prints what went and what did not, and keeps going.
- CHANGELOG entries for each; `docs/devices.md` gets the MTP limit if the fallback is taken.

## As built: when a Daily run counts as failed

`digest run` fails while any connected reader missed the run's own edition, even when another
reader got it. The watcher's re-run is the only retry for a plugged-in Kindle, so "delivered if
any reader got it" would remove that retry. The JSON result carries `edition_sent` and
`edition_missed` per device, and the app shows the run as failed when the result is not `ok`,
naming the reader that missed it.

## Open questions

1. Dot-file `.part` names in `documents/`: acceptable, or use a `Deckle/` scratch folder on the
   device? A scratch folder is cleaner to look at but is a cross-folder rename, which is still
   one step on the same volume.
2. Is a 1 MB free-space margin on MTP enough? The Kindle writes its own index files after a send.

## Not verified

No real Kindle, Kobo or PocketBook was used: MTP rename support, what a Kindle keeps after an
interrupted send, and dot-file handling are unknown. Linux vfat, Linux mount behaviour and
Windows rename were not tested. Most line numbers are the validator's readings.
