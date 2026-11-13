# Press queue repairs: bindings, identity, journal size — Design

Date: 2026-11-12. Status: revised after validation (QA session, same day). Covers issue #92
(four items) and the job-folder item of #87.

This changes behaviour inside the existing press protocol
(`crates/deckle-digest/src/press`). It adds no message type and changes no file format that a
reader plugin sees.

## Why

1. **A folder can never change paper.** `PressJournal::register_binding` (`queue.rs:303`) refuses
   a folder that is already bound to another paper or producer, and nothing ever removes a
   binding. After the documented disconnect and reconnect to a different paper, or a paper
   deleted and made again, the worker (`deckle-cli/src/press_worker.rs:510`) gets a protocol
   conflict, treats it as a policy error, and admits no paper at all.
2. **The identity file can be read half-written.** `setup::producer_identity` (`setup.rs:33`)
   creates `producer.json` empty and then writes it. A second process in that window, or any
   process after an interrupted first write, reads an empty file and fails as "unsupported" until
   the owner deletes the file. The app and the watcher both call it at start.
3. **The journal grows with refused requests.** Each refused or expired request id is stored for
   30 days; every commit rewrites and syncs the whole journal; `flush_statuses` re-checks every
   entry on each tick. 2,000 refused requests cost 27 ms per commit and 1.2 s per idle flush.
4. **A forgotten request comes back.** `forget_old` drops an entry 30 days after its last update,
   the request file is still in the folder, and the next scan journals it again as expired for
   another 30 days.
5. **Job folders are never removed** (#87). `cleanup_attempt` removes `jobs/<uuid>/attempt` only.

## Goals

- Reconnecting a folder to another paper works, without weakening the rule that request bytes
  cannot choose where output goes.
- Starting the app and the watcher together on a new machine works every time.
- The journal's size is bounded by recent work, not by how many requests were ever refused.

## Not goals

- Any change to the wire format, the manifest, or reader plugins.
- Sharing one folder between two papers at the same time. One folder, one paper, stays the rule.
- Deleting anything a reader wrote. The spec forbids it (`deckle-press.md:80`).
- #87's item about a secret in `job.json` (`config.rs:1620`). It stays open.

## 1. Replacing a binding

A binding is `(root, producer_id, paper_id, enabled, stopped)`. The conflict rule exists so that
a request already accepted for paper A can never be delivered as paper B.

A constraint shapes the design: `validate_data` (`queue.rs:75-133`) requires unique roots and a
binding for every entry and job, and `PressJournal::open` refuses a journal that fails it. So the
old paper's entries cannot stay in the journal once its binding is gone.

`register_binding` for a `root` that is bound to a different `(producer_id, paper_id)` replaces
the old binding, in one journal commit, when all of these hold; otherwise it returns the
conflict as today:

1. The old binding has no entry in a non-terminal state (pending, building, publishing).
2. The folder's owned manifest carries the journal's own `producer_id` and names the new
   `paper_id`, read through `safe_io` as publishing does.
3. The old binding's published outputs have been deleted, by the step below.

**Deleting the old paper's outputs.** The ordinary prune path cannot do this: it goes through
`check_binding(A)` and `setup::owned_manifest(A)` (`publish.rs:36-43`), which need the manifest to
name the old paper, while condition 2 needs it to name the new one; and `prune_results` keeps the
newest 7 outputs and everything under 48 hours (`retention.rs:236`). So replace has its own
delete mode, used nowhere else: when conditions 1 and 2 hold, it deletes **every** output the
journal records for the old binding, through the same `safe_io` deletion the prune path uses,
without the 7-output and 48-hour rules and without the `owned_manifest` test. The other checks
of `check_binding` stay: the root is a directory, `canonicalize(root) == root`, and
`.deckle-press` is a directory (`publish.rs:37-41`). It covers stages and unpublished intents
too (`retention.rs:247-283`). It deletes only files the
journal itself recorded as published by this producer into this root; nothing is taken from the
manifest or the folder listing. Each deletion commits on its own, as prune steps do, so a crash
part-way is resumed on the next tick. If a deletion fails (folder offline, read-only), the
conflict stands and the next tick retries. A deletion can also be refused for good: the bundle
check rejects any extra entry in a bundle (a `.DS_Store`, a sync-conflict copy) and a partly
deleted bundle fails its hash. So that replace cannot be blocked for ever without a word, the
folder's policy error names the path that was refused, and the owner can remove that bundle
folder by hand; Deckle does not delete files it cannot verify.

When all outputs are gone, the old binding's status files are removed, and then one commit
removes its entries, jobs and output records and adds the new binding. The status files go
first so that a crash leaves no orphan. After the commit the journal passes `validate_data`.

Two documents change with this: `docs/daily.md:1236` tells the owner to keep the old paper's
status files, and `deckle-press.md:52` keeps `result_removed` statuses for 30 days. Both get a
sentence saying that reconnecting a folder to another paper removes the old paper's outputs and
statuses at once.

The worker change: a conflict from `register_binding` for one folder is recorded as that
**folder's** policy error and no longer blocks the other papers. This needs both `policies()`
calls (`press_worker.rs:592`, `:723`) and `reconcile_with_watcher` (`:482`) changed, since each
blocks every paper on one folder's error today.

Nothing changes in `setup::disable`: it cannot reach the journal while the watcher holds
`supervisor.lock` (`journal.rs:42`), and `press_worker.rs:517-524` already disables bindings that
are no longer configured, which is what makes condition 1 reachable.

## 2. Writing the identity file

`producer_identity(path)`:

1. Read the file. If it parses and the id is not nil, return it.
2. If it does not exist: generate an id, write it to `producer.json.<random>.tmp` in the same
   folder, `sync_all`, `hard_link` the temp file to `producer.json`, remove the temp, and sync
   the folder. `hard_link` fails with "already exists" if another process won; then go to step 1
   and return the winner's id. (`safe_io.rs:276` already uses `hard_link` this way.)
3. If it exists but is empty or does not parse: fail closed, as today. Two processes repairing
   an empty file at once could each write a different id, and a new id orphans every folder bound
   to the old one. The error message changes to say what to do: "producer.json is empty or
   damaged; if this machine has no press folders yet, delete it and start Deckle again".

After this change no process can see an empty file that Deckle wrote, because the name appears
only when the content is complete. An empty file left by an older version still needs the owner
to delete it once.

## 3. Bounding the journal

Refusals stay journaled. A refused id has to stay terminal: the scan re-reads request files on
every pass (`scan.rs:85-161`), and without an entry a request refused as Busy would be queued
once the queue drains.

- **Refusals are forgotten sooner.** An entry refused without a job is forgotten at
  `max(first_seen + 72 h, expires_at + 48 h)` when its request's lifetime is exactly 24 hours
  (`protocol.rs:244`), and at `first_seen + 72 h` otherwise (a `ClockMismatch` refusal, whose
  `expires_at` cannot be trusted). Forgetting a refused entry leaves its status file in place.
- **A request whose status file exists is never journaled.** The status file is the record that
  the request was answered. This is checked in `reject_in` (`scan.rs:143-151`,
  `press_worker.rs:623-624`) before anything else, and it is what stops a forgotten entry from
  coming back, whatever the request's timestamps say: a request from a clock that was ahead
  cannot be accepted and built after its refusal is forgotten.
- **A request first seen long after it expired** (48 hours or more past `expires_at`, lifetime
  exactly 24 hours) gets one `expired` status file and no journal entry, so a reader whose clock
  is days slow still learns why nothing happens.
- Status files left this way are not swept. One exists per request file a reader wrote, and a
  sweep would need a listing of every reader's request folder, which the scan avoids on purpose
  (100 per tick, `scan.rs:85`). `deckle-press.md` is updated to say status files of refused
  requests stay until the folder is cleaned.
- **Status flush is incremental.** `flush_statuses` writes only entries whose status changed
  since the last successful flush. The dirty set is kept in `Engine`, not in the journal object,
  because a flush error drops and reopens the journal (`press_worker.rs:104-116`) and a set kept
  there would mean a full flush on every tick while a folder is offline. Every five minutes one
  full flush runs, so a status file removed by a sync tool or by hand is written again.
- **Cap as a backstop.** At 1,000 terminal entries for one binding, new request files for that
  binding are left unanswered. Entries are not forgotten early: forgetting an entry whose
  request file is still live would build it again. The cost is stated plainly: accepted entries
  live 30 days (`queue.rs:1049`), so a binding that reaches the cap through real builds (about
  100 accepted per build, `queue.rs:403-440`) answers nothing until entries age out. The worker
  logs when a binding reaches the cap and when it leaves it, not on every tick (a tick is one
  second, `press_worker.rs:85`), and the folder's policy error says "request history full", which reaches the app the way
  other policy errors do (the app cannot open the journal while the watcher holds
  `supervisor.lock`).
  `deckle-press.md:80`, which says `expires_at + 30 days`, is updated to the rules above.

## 4. Job folders

`cleanup_attempt` removes `jobs/<uuid>` as a whole once the attempt folder is gone and the job's
result has been recorded in the journal. At start, the worker sweeps `jobs/`: folder names are
random (`press_worker.rs:760`), so it reads each `job.json`'s `lease.job_id`, and removes a
folder older than a day whose job the journal does not hold or whose `job.json` cannot be read.

Worker tests that count `jobs/` entries (`tests/it/press_worker.rs:236-256`, `:311`, `:577`,
`:627`) change with it.

## Migration

Existing journals are read as they are. Refusal entries already stored age out under the new
48-hour rule on the first `forget_old` pass. No version bump and no new stored field.

## Tests

- Bind (root, A), stop configuring A, set the manifest to B, register (root, B): accepted; A's
  outputs and status files are gone; the journal reopens (`PressJournal::open` succeeds); a
  request for B is admitted.
- The same with a pending entry for A: conflict.
- The same with one of A's outputs undeletable: conflict, and accepted on a later tick once it
  can be deleted.
- Two papers, one with a conflicting folder: the other is still admitted.
- `producer_identity` from 16 threads on a new path, 1,000 rounds: every call returns `Ok` with
  the same id. A zero-length file and a file with unparseable content both fail with the new
  message.
- A request refused as Busy, then the queue drains, then a scan: it is not queued.
- A refused request, advance 73 hours, scan twice: the entry is gone and is not journaled again;
  the request file and its status file are still in the folder.
- A `ClockMismatch` request with `expires_at` in year 9999: forgotten 72 hours after first seen,
  and not journaled again on the next scan.
- A request from a clock 60 hours ahead, refused, then 72 hours pass: it is not accepted.
- A bundle with an extra `.DS_Store`: replace stays a conflict and the policy error names it.
- A request first seen 4 days after its `created_at`: one `expired` status file, no journal
  entry, and the file is not rewritten on later scans.
- A status file deleted by hand is back within five minutes.
- A binding whose old paper published 10 outputs in the last hour: replace deletes all 10.
- 2,000 refused requests over simulated days: the journal never holds more than three days'
  worth.
- A folder offline for 100 ticks: each tick flushes only dirty entries.
- A finished job leaves no folder under `jobs/`; a stale job folder is removed at start.

## Not verified

I re-read `queue.rs:75-133` and `:886-893`. The other line numbers in the revised parts are the
validator's readings. How reader plugins handle a missing status, or an `expired` status with no
journal entry behind it, was not checked; the plugins' status code was not read. The
replace-only delete mode and the one-time `expired` status are my design and were not tried.
