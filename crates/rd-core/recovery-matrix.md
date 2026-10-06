# Recovery matrix

What rDownloader is proven to survive when it stops in the middle of something, and how each
claim is checked. Tracked as RD-140-04.

A download manager's whole promise is that an interruption costs time, not data. Ordinary
tests cannot check that promise: they run to completion, which is the one shape of run in
which nothing can be lost. The interesting instant — bytes on disk that the database has not
recorded yet — lasts microseconds and is not reachable by timing, so it is made addressable
instead. `rd_core::failpoint!` names such an instant; a test arms it, the code stops there,
and the test then asserts what the next start makes of the result.

## The four invariants

Every case asserts all four. They are not interchangeable, and each has its own failure mode:

1. **No confirmed byte is invented.** The recorded offset never exceeds what was actually
   fetched. Getting this wrong produces a file with a hole in it that passes every length
   check and fails only when someone opens it.
2. **No confirmed byte is overwritten with different content.** A resume rewrites the tail
   after the checkpoint and nothing before it.
3. **Resuming reaches the same bytes as an uninterrupted run.** Compared by SHA-256, so a
   corruption that happens to preserve the length still fails.
4. **Nothing is left behind.** No stray part, staging or lock file survives the interruption.

## The three axes

| Axis | What it proves | Where it runs | Cost |
| --- | --- | --- | --- |
| **A — deterministic crash points** | The state machine: every persistence step is safe to stop at | Locally and in CI, with the owning crates' `failpoints` features on | ~3 s |
| **B — real process kill** | That the operating system's write actually landed | CI only, `#[ignore]` by default | ~1 s per case, one spawned binary each |
| **C — migration forward** | An older installation upgrades without losing its queue | Every test run | <1 s |

Axis A only exists when it is asked for, and it is each *owning* crate's feature that asks. The
runs are `scripts/lib/crash-matrix.list`, the one list `scripts/check.sh` and CI's `crash-matrix`
job read (RD-191-09); by hand, each line `rd_crash_matrix_runs` prints is one
`cargo nextest run -j 2 <line>`:

```bash
bash -c 'source scripts/lib/crash-matrix.sh; rd_crash_matrix_runs'
```

`rd-core/failpoints` on its own is not enough, however plausible it looks. Every crash-test
file is gated on the feature of the crate that owns the point, and enabling a dependency's
feature does not enable its dependants'. A file compiled to nothing runs no cases and reports
success, which is the one failure mode this matrix cannot afford, so check the counts: without
those features `rd-http` runs 97 tests, `rd-scheduler` 111, `rd-usenet` 54, `rd-object-storage` 50
and `rd-backup` 52; with them 103, 117, 57, 51 and 54, measured per crate on 2026-09-29.
RD-170-07 added three `rd-backup` cases and the first `rd-plugin-host` one, all behind the
features, so those two counts change and are measured again with the next run; `rd-plugin-host`
has none recorded yet. RD-180-12 added one case each to `rd-extract`, `rd-api-core`, `rd-torrent`
and `rd-plugin-transfer` and two to `rd-api`'s admin suite; their counts are measured with the
next run as well. The `rd-plugin-transfer` case drives the reference backend component, so the
`no-components` profile (and with it CI's `-P ci`) counts it as skipped; it runs where the
components are built. RD-180-03 added two `rd-backup` cases (`tests/pre_update_crash.rs`, the
backup before an update); the count is measured with the next run. RD-180-02 added four
`rd-update` cases (`tests/install_crash.rs`, the portable self-update's switch); `rd-update` has
no count recorded yet. RD-190-04 added two `rd-db` cases in a binary of their own
(`tests/archive_password_crash.rs`, compiled to nothing without `rd-db/failpoints`); its count is
measured with the next run. RD-191-03 added the first `rd-transfer-file` case
(`transfer_file.before_progress_recorded`, the staging half FTP, SFTP and buckets share); the
crate has no count recorded yet. RD-191-12 added one `rd-scheduler` case
(`scheduler.before_auto_retry_requeued`), behind the feature; the count with it is measured with
the next run. RD-1100-04 added one `rd-db` case in a binary of its own
(`tests/download_history_crash.rs`, the history entry written with the package's outcome); its
count is measured with the next run.

the next run. RD-1100-05 added one `rd-usenet` case (`usenet.before_traffic_flushed`, in
`src/traffic_crash_tests.rs`) behind the feature and four ordinary ones without it; both counts
are measured with the next run.

Axis A returns an error at the crash point rather than killing the process. That drops the
whole worker, the open file handle included, which is the state a restart finds — everything
except the question of whether the kernel had flushed, which is exactly and only what Axis B
is for. Paying for a spawned process at every crash point to re-prove the state machine would
buy nothing; paying for it once per flush that matters is worth it.

**Axis B must not be run on a development machine.** It spawns real service processes, which is
why its cases are `#[ignore]` and run in CI only: `crates/rdownloader/tests/kill_restart.rs`, run by
`.github/workflows/recovery.yml` on Linux with
`cargo nextest run -p rdownloader --test kill_restart --run-ignored only`. `RD_AXIS_B_BINARY`
names another binary than the one Cargo built for the test.

The cases start `rdownloader serve` on a fresh data directory and fetch from an origin inside the
test whose gate holds every connection still once a given number of bytes has gone out, so the
`SIGKILL` lands at a known point rather than at a moment a timer picked:

- **Mid-download.** One connection, a 32 MiB payload, the gate at 20 MiB: two of the engine's
  8 MiB checkpoints are behind the transfer when it is killed. Read while nothing runs, the row
  claims no more than the origin sent and the part file holds exactly the source's bytes up to
  the checkpoint (invariants 1 and 2). The restart resumes — it fetches at least one recorded
  checkpoint's worth less than the whole file — and ends with the same SHA-256 and no part file
  (3 and 4).
- **Mid-post-processing.** A package whose user script waits is killed while the script runs.
  The restart does not leave the package `postprocessing`: the interrupted step runs again, the
  package completes, the payload is unchanged and nothing is left in staging.

## Registered crash points

This table is checked against `rd_crash_points::CRASH_POINTS` by a test, so it cannot
drift from the code. Adding a crash point without a row here fails the build, and so does a
row for a point that does not exist.

| Point | Owner | A restart must show |
| --- | --- | --- |
| `archive_password.after_secret_removed` | rd-db | a sweep stopped after it removed released archive passwords from the vault and before it recorded that keeps the record and never the entry; the next start removes the record, and no row points at an entry that is gone |
| `archive_password.before_reference_adopted` | rd-db | archive passwords written to the vault before any row points at them lose nothing: a stopped takeover keeps every plain value, the next start removes the entries the first attempt reserved and moves the values again, and the vault ends with exactly one entry per password; a stopped write keeps the row's previous password |
| `automation.before_outcome_recorded` | rd-api-core | a run whose action took effect before its outcome was recorded is queued again by the next start at that same action, never left running and never moved past an action nobody recorded; the action runs again and the run completes |
| `backup.after_database_snapshot` | rd-backup | a database copy staged for a run that stopped before its archive was sealed is removed by the next start with the rest of the staging; the run is recorded as interrupted and nothing reaches a destination |
| `backup.after_retention_removal` | rd-backup | an archive retention removed at its destination before the ledger forgot it is forgotten by the next pass, which finds it gone; the ledger never lists fewer archives than the destination holds, and an archive the plan keeps is never removed |
| `backup.before_archive_published` | rd-backup | an archive finished in staging but not yet at its destination never appears there under its final name; the next start records the run as interrupted and removes the staging |
| `backup.before_archive_recorded` | rd-backup | an archive that reached its destination before the ledger recorded it stays there whole and is never removed by retention, which removes only recorded archives; the next start records the run and that destination as interrupted |
| `history.before_entry_committed` | rd-db | a package outcome stopped after its history entry was written and before the transaction committed leaves neither behind: the package keeps its earlier state and the history has no entry for it; the outcome written again leaves exactly one entry, and every entry committed before survives the restart |
| `http.after_chunk_mac` | rd-http | a finished chunk MAC that was not recorded is recomputed from the start of its chunk, never assumed |
| `http.after_chunk_write` | rd-http | bytes written but not recorded are re-fetched, never counted as confirmed |
| `http.after_db_checkpoint` | rd-http | a recorded checkpoint is resumed from exactly, re-fetching nothing before it |
| `http.after_part_sync` | rd-http | a durable write without its commit falls back to the older checkpoint |
| `http.before_piece_check` | rd-http | a chunk confirmed but not checked against its piece hashes is checked before anything builds on it, and a piece that fails isolates the source named for it |
| `object_storage.after_part_upload` | rd-object-storage | a part the service confirmed but that was not recorded is uploaded again under the same number, never counted as confirmed; every part recorded before is not sent again |
| `plugin.before_install_recorded` | rd-api-admin | an automatic update whose version folder exists before its repository row was written stays installed whole and listed once, and the version pointers stay as they were: the next start runs what they chose before the update, the newest version when they chose none |
| `plugin.before_pointers_followed` | rd-api-admin | an automatic update recorded with its repository before the version pointers followed it stays installed whole and listed once, and the pointers stay as they were, never half moved: the next start runs what they chose before the update, the newest version when they chose none |
| `plugin.before_version_promoted` | rd-plugin-host | a package written under its staging name but not yet renamed into its version folder is never loaded or listed; the next start removes it, the installed version stays the one that runs, and the next update pass installs it again |
| `plugin_transfer.after_pin_saved` | rd-plugin-transfer | a plugin transfer stopped after its backend version was pinned and before its first byte runs again on the pinned version and finishes with the source's bytes; when that version is gone it begins anew on the newest backend, never refused for bytes it does not have |
| `plugin_transfer.before_checkpoint_saved` | rd-plugin-transfer | bytes a stopped plugin transfer wrote before its checkpoint was saved are continued by the next run from the part file, on the backend version pinned before its first byte and after the remote file was checked against what the first run saw; nothing past them is counted, and the finished file matches the source byte for byte |
| `postprocess.before_direct_unpack_adopted` | rd-extract | a set unpacked directly while its package downloaded, stopped before the pipeline moved it into the package, has put nothing at the destination; the next start removes its staging directory, unpacks the set the normal way and completes the package with the same files |
| `postprocess.after_sort_move` | rd-extract | a sort stopped after it placed a file and before it recorded the step is run again by the next start: the files still in the package are placed by the same templates, the ones already placed are neither moved again nor copied beside themselves, and the package leaves post-processing completed |
| `postprocess.before_scan_recorded` | rd-extract | a package whose malware scan ran before its verdict was recorded is scanned again by the next start and never released on a verdict nobody recorded; a finding fails it then, with the steps after the scan skipped and not run |
| `postprocess.before_unpack_recorded` | rd-extract | an archive unpacked before its step was recorded is unpacked again by the next start into the same place, replacing what the first run wrote; the package leaves post-processing completed, and no staging directory, not even one a killed extraction left, survives |
| `pre_update.before_archive_published` | rd-backup | an archive sealed and checked before an update but not yet moved into the pre-update folder never appears there; the next start removes the staging with the unencrypted copy it held, and the next preparation seals a whole one |
| `pre_update.before_copy_published` | rd-backup | a database copy written before an update but not yet checked never carries a copy's name, so no rollback can pick it; the live database is untouched and opens as it was, the next start removes the partial file, and the next preparation writes a whole, checked copy |
| `restore.after_live_set_aside` | rd-backup | a switch to a restored state stopped after a live item was set aside and before the restored one took its place is finished by the next start, which then opens the restored database; the previous installation stays in restore-previous until that start completes |
| `scheduler.after_package_row` | rd-scheduler | a package row written before any of its files is dropped by the next start, never left in the queue as an empty one |
| `scheduler.after_queue_pause_recorded` | rd-scheduler | a timed pause recorded before its files were paused holds the queue from the next start until its end, so none of its files starts early; once the end has passed, every file it paused is queued again and none stays paused for good |
| `scheduler.after_torrent_selection` | rd-scheduler | a torrent row whose reviewed file selection was written before it joined the queue stays paused with that selection after the next start, never queued and never started with the default selection; resuming it starts the reviewed one |
| `scheduler.before_auto_retry_requeued` | rd-scheduler | a failed download whose automatic retry came due before it was put back into the queue stays failed with its due time and its round uncounted; the pass after the next start puts it back exactly once and counts one round, with its attempts and limit waits starting from zero |
| `scheduler.before_mirror_promoted` | rd-scheduler | a mirror group whose active member has failed before its successor was promoted is given its next mirror by the start that follows, never left waiting for a link that is not coming |
| `scheduler.before_move_source_removed` | rd-scheduler | a move stopped between its verified copy and the removal of the original ends on the next pass with exactly one copy, at the new place, never a second one beside it |
| `scheduler.before_package_move` | rd-scheduler | a package whose row already points at the new folder still finds its data and finishes the move |
| `scheduler.before_promote` | rd-scheduler | a payload already in its final place is adopted by the next pass, never fetched a second time |
| `subscription.after_items_archived` | rd-api-core | release files a poll archived before handing them to the LinkGrabber stay pending in the archive after a restart: the next poll neither hands them over a second time nor loses them, and the review list still offers them |
| `torrent.after_relocation_commit` | rd-torrent | a torrent move stopped after its package was pointed at the new folder and before the originals were released is finished by the next start: every file is at the new place exactly once, the old folder is left empty, the journal is cleared and a seed seeds again from the new place |
| `torrent.before_relocation_commit` | rd-torrent | a torrent move stopped after its files were placed in the new folder and before its package was pointed there is taken back by the next start: every file is at the old place exactly once, nothing is left in the new folder, the journal is cleared and a seed seeds again from the old place |
| `torrent.before_seed_completed` | rd-torrent | a seed stopped after its seed time was closed and before its row completed is still seeding after the restart, is taken up again and completes when it is stopped; the seeded time is counted once |
| `transfer_file.before_progress_recorded` | rd-transfer-file | bytes an FTP, SFTP or bucket transfer synced to its part file before the row recorded them are continued by the next run from the part file's length, after the remote file was checked against what the first run saw; nothing is fetched twice, and the finished file matches the source byte for byte |
| `update.after_leftover_set_aside` | rd-update | a portable update stopped after a leftover of the update before (its .previous, staging or .failed folder) was moved into the trash and before the trash was swept has changed nothing live: the next start records it as failed with the old version in place and the database as it was, and the next update sweeps the trash and goes through; a leftover a running program still holds never fails an update |
| `update.after_new_placed` | rd-update | a portable update stopped after a new entry took its place, with other entries still the old version's, is taken back by the next start, whichever version that start runs: every entry is the old version's again, the new ones leave, and a newer program restarts as the old one; nothing below the data directory changes |
| `update.after_previous_set_aside` | rd-update | a portable update stopped after an old entry went into .previous and before its new one took its place is taken back by the next start: the entry comes back from .previous, nothing of the new version stays and the database is left as it was, since the new version never ran |
| `update.before_health_check` | rd-update | a portable update recorded as switched but never proven is proven by the first start of the new version that answers, and taken back with the database copy from before the update by the next start if that first one never answered; the program is never left as a mix of both versions |
| `usenet.after_article_write` | rd-usenet | an article on disk without its checkpoint is truncated and fetched again, never counted as confirmed |
| `usenet.before_checkpoint_batch` | rd-usenet | the articles of a checkpoint batch that did not commit are on disk but fetched again, never counted as confirmed; every batch committed before stays confirmed |
| `usenet.before_hopeless_abort` | rd-usenet | a set judged beyond repair whose rows were not yet failed is judged again after the next start from the segments every server refused, with the same counts, and the same rows fail; no row fails before that write and none is left waiting after it |
| `usenet.before_traffic_flushed` | rd-usenet | counts a flush had not yet written when the process stopped are lost, at most one flush interval of traffic, and nothing else: every flush committed before stays, and counting after the next start adds to it without counting anything twice |
| `vault.after_orphan_removed` | rd-db | a vault sweep stopped after it removed some of the entries no cell of the database names keeps every entry a row or a settings document names; the next start removes the remaining orphans and nothing else |

`archive_password.before_reference_adopted` and `archive_password.after_secret_removed` are the
archive passwords in the vault (RD-190-04, `rd-db/src/archive_password.rs`). A write records the
new references as reserved, writes the values into the vault under them, and only then points the
rows at them and empties the old plain column in one transaction; whatever a row lets go of, a
trigger records as released in the transaction that lets go, and the sweep removes the vault entry
before it removes the record. The first case stops a start's takeover after the vault holds the
values and before any row points at them, and asserts that the plain values are all still there,
that the next start removes the reserved entries and moves the values again, and that the vault
then holds one entry per password. The second stops the sweep after a deleted package's entry left
the vault and before the record did, and asserts that the next start removes the record. Both
cases (`crates/rd-db/tests/archive_password_crash.rs`) run with `rd-db/failpoints`.

`vault.after_orphan_removed` is the vault's sweep at start (DB-03, `rd-db/src/vault_sweep.rs`).
Every other owner of a vault entry writes the value before its row and removes it after the row
is gone, so a stop in between leaves an entry nothing names. The sweep lists the vault, reads every
reference any text cell of the database holds (columns and JSON documents alike) and removes the
rest, one entry at a time. The case stops it after the first removal and asserts that every named
entry is still there, that the next sweep removes the remaining orphans, and that a third finds
nothing. It runs with `rd-db/failpoints` in `crates/rd-db/tests/archive_password_crash.rs`, the
vault's crash binary.

`history.before_entry_committed` is the download history (RD-1100-04, `rd-db/src/history_store.rs`).
The entry is written in the transaction that gives the package its outcome — the writer's
`set_package_state` for `completed` and `failed`, and `record_failure` for the last file of a
package that ends without a single finished one — so the history can never describe an outcome
the queue does not know, nor miss one it does. The case stops after the entry is written and
before the commit, asserts that the package kept its earlier state and that the history is
empty after a restart, writes the outcome again and asserts exactly one entry, then restarts
once more and finds it still there. It runs with `rd-db/failpoints` in
`crates/rd-db/tests/download_history_crash.rs`.

`backup.before_archive_published` is the full backup's two-phase step (RD-160-01): the
encrypted archive is complete in the staging folder below the data directory, and only then is
it handed to its destination, which copies it under a temporary name, compares both sides by
SHA-256 and renames it into place. A stop in that window leaves a finished archive nobody
recorded and a history row that still says `running`. Its case asserts that the destination
holds nothing under the archive's name, that the recovery every start runs marks the row
`interrupted` with `backup.interrupted` and empties the staging, and that the next run succeeds
beside it. The case runs with `rd-backup/failpoints`.

`backup.after_database_snapshot` is the same run one step earlier (RD-170-07): the writer has
put a consistent copy of the database into the run's staging folder, and nothing is sealed
yet. That copy is plain — the only unencrypted state a backup ever writes — so its case asserts
that the start after the stop marks the run `interrupted`, that the sweep removes the copy with
the staging, and that no destination saw anything. The case runs with `rd-backup/failpoints`.

`backup.before_archive_recorded` and `backup.after_retention_removal` are the ledger's two
steps (RD-170-07, `rd_backup::ledger`). A delivery places the archive at its destination and
only then records it; a stop in between leaves an archive the ledger does not know. Its case
asserts that the archive is there whole (size and SHA-256 of the sealed file), that the next
start marks the run and that destination `interrupted`, and that the next run's retention,
which removes only recorded archives, leaves it where it is. Retention removes archives at the
destination first and forgets them in the ledger after; a stop in between leaves a ledger that
lists an archive the destination no longer holds, never the other way round. Its case asserts
that the next pass finds the missing archive gone (`NotFound` counts as removed), forgets it
with the rest, and keeps the newest. A stop inside the destination's own copy is the verified
copy's temporary name, which `rd_files::copy_verified` discards before its next copy under that
name and which no listing takes for an archive. Both cases run with `rd-backup/failpoints`.

`plugin.before_version_promoted` is an install's or update's switch (RD-170-07): the package is
downloaded, verified and written whole into a `.install-<id>` folder beside the version folders,
and a rename makes it a version. Before RD-170-07 a stop in between left a folder the loader
read as a second copy of the plugin that `remove_version` could not address. Its case asserts
that the folder is never loaded or listed, that the start (`PluginRepositoryService::load`)
removes it, that the version installed before stays the only one, and that the update then
installs. The download itself is kept under `downloads/` by digest, which every start clears.
The case runs with `rd-plugin-host/failpoints`.

`pre_update.before_copy_published` and `pre_update.before_archive_published` are the two
publishing steps of the backup the updater asks for before it switches versions (RD-180-03,
`rd_backup::pre_update`). The database copy is written under a `.partial` name, synced and
checked — `PRAGMA integrity_check`, the schema this build runs on with nothing pending, the core
tables readable — and only then renamed; the archive is sealed in `pre-update/staging`, opened
again under its key and read to its end, and only then moved into the folder. A stop before
either rename leaves nothing a rollback could mistake for a backup: the cases assert that no file
carries a final name, that the live database opens as it was, that the sweep every start runs
removes the leftovers, and that the next preparation writes a whole one. The cases run with
`rd-backup/failpoints`.

`update.after_previous_set_aside`, `update.after_new_placed` and `update.before_health_check`
are the portable self-update's switch (RD-180-02, `rd_update::install::portable`). The updater
unpacks the new archive beside the program, then per top-level entry moves the live one into
`.previous/` and the new one into its place, and records the switch before it starts the new
version and waits for its health route to name that version. A stop between two renames leaves
the program folder a mix of both versions, which no start may run: `recover_at_start` runs before
the database opens and takes the switch back from any point, and when the program that runs is the
newer one it starts the restored executable and ends. A switch recorded but never proven is proven
by the first start that answers (`confirm_started`); a second start without that proof takes it
back and puts the database copy from before the update in place, since the new version ran. The
four cases (`crates/rd-update/tests/install_crash.rs`) run with `rd-update/failpoints`. The
Windows installer has no points of its own: Windows Installer is transactional, and the start
reads from the version that runs which way its transaction went.

`update.after_leftover_set_aside` is the step before the switch (RD-180-02, live finding
2026-10-02, `rd_update::install::trash`): the leftovers of the update before — `.previous/`, the
staging, `.failed-<version>/` — are moved into `<install>/.trash/` and removed from there, because
Windows lets a running program be moved but not deleted and a capture agent may still run from
`.previous/`. A stop after a move and before the sweep leaves the program files untouched; the case
asserts that the next start records the update as failed with the old version and the live
database in place, and that the next update sweeps the trash and switches. Every start after an
ended update sweeps the trash again, so what a running program held goes once it has ended. The
case sits beside the four above and runs with `rd-update/failpoints` too.

`restore.after_live_set_aside` is the restore's cutover (RD-160-03). A restore never replaces
the running service's database: it stages the restored state in `restore-staged/` with a marker,
and the next start switches before it opens the database, renaming each live item (the
database, its journal files, the torrent folders) into `restore-previous/` and the staged one
into its place. A stop between the two renames of one item leaves no database where the start
looks for it. Its case asserts that the next start finishes the switch from exactly that state,
opens the restored database, and that the completed start removes the marker and the previous
state; a restored database that does not open is put back by the roll-back case beside it. The
case runs with `rd-backup/failpoints`.

`http.before_piece_check` belongs to the multi-source transfer (RD-150-03). A chunk fetched
from one mirror of a Metalink file is confirmed by its checkpoints like any other, and only
then read back and compared with the piece hashes the document stated. A stop between the two
leaves bytes the database calls confirmed that nothing has checked. The source a chunk is
fetched from is written down before its first byte arrives, so its case asserts that the next
run checks every complete, unchecked chunk first, moves a failed one back to the start of the
refused piece, isolates the mirror that sent it and reaches the right bytes from another one.

`object_storage.after_part_upload` is the upload side's two-phase step (RD-150-04): the
service confirms a part of a multipart upload, and only then is the part recorded with the
identifier the completion has to name it by. A stop in that window leaves a part in the bucket
that nothing here vouches for. Its case asserts that the next run uploads that part again under
the same number — which replaces it at the service — and none of the parts recorded before, and
that the completed object is the local file byte for byte. The case runs with
`rd-object-storage/failpoints`.

`scheduler.before_promote` is the other two-phase step in the scheduler: a finished `.part`
is renamed into the package folder and the row is only then marked complete. A stop in that
window leaves the payload where the user expects it while the queue still calls the transfer
unfinished, and `recover_interrupted` puts the row back into the queue — where, with the part
file gone, the ordinary resume has nothing to resume from. Its case therefore asserts the one
thing that matters here: the next pass recognises the file that is already there, finishes it,
and does not fetch a second full copy to file beside the first as `name (1).ext`.

`usenet.before_checkpoint_batch` is the batched form of `usenet.after_article_write`
(RD-130-22). The assembly no longer confirms each article in a transaction of its own: while
articles arrive faster than the writer confirms them, their checkpoints wait - at most
sixteen - and go into the database together, and whatever waits is confirmed before the
assembly waits for the network. A stop before that commit leaves several articles on disk
that nothing vouches for. Its case asserts that the resume fetches every one of them again
and none of the batch committed before it: the batch widens what a crash costs from one
article to sixteen, and changes nothing about what the resume trusts.

`usenet.before_hopeless_abort` sits between the verdict that a set is beyond repair
(RD-1100-02) and the one transaction that fails its waiting rows. The verdict itself persists
nothing: it is computed from what the database already records - the segments every server
refused, the rows' states and names. A stop at this point therefore leaves no row half-failed,
and the next judgement, after the restart, reads the same inputs. Its case asserts exactly
that: nothing failed before the write, then the same counts and the same failed rows from a
second judgement as an uninterrupted one would have given.

`usenet.before_traffic_flushed` is the traffic per Usenet server (RD-1100-05). The pool counts
every article body in memory, per server, and a flusher writes the counts every ten seconds in
one transaction, and once more after the scheduler stopped. A stop between counting and writing
loses what was counted since the last flush, by design: writing per article is what the batch
exists to avoid. Its case (`src/traffic_crash_tests.rs`) asserts the bound: the committed flush
stays whole, the lost counts are not invented after the restart, and counting afterwards adds to
what was written. The quota is counted in the same transaction, so a crash can delay its
notification by one interval and never fire it twice.

`scheduler.after_package_row` is the enqueue path's own two-phase step, and the only one of
the three where the order cannot be chosen: a download row needs a package to belong to, so
the package is always written first. A stop in that window leaves a package with nothing in
it, and nothing in the queue or the interface tells such a row apart from a package that
finished without downloading anything. Worse, it can never be removed afterwards — removing a
package is a side effect of removing its last file, and it has none. Its case therefore
asserts the folder-free form of invariant 4: `recover_interrupted`, the first thing a restart
runs, drops the empty row, and leaves a package that does have files exactly where it was.

`scheduler.after_torrent_selection` covers the order the LinkGrabber's torrents are queued in
(1.9.1, API-07): a torrent row is written paused, gets the file selection reviewed in the
LinkGrabber, and only then joins the queue. A stop between the last two leaves the row paused
with its selection — the order exists so that no instant has a queued row without one, because
a runner that starts on the default selection persists it over the reviewed one. Its case
asserts that the row is paused, holds the reviewed selection, and stays so after
`recover_interrupted`.

`scheduler.before_package_move` is the one point that is not about bytes inside a file. It
covers the two-phase protocol a package folder is moved or renamed with: the database is
written first and the disk follows, so the interesting instant is the one where the row
already names the new folder and nothing has moved yet. Its case therefore asserts the
folder-level form of invariants 3 and 4 — the data is all reachable under the new folder, and
the old one is gone — rather than the byte-level four, which have no meaning for a move.

`scheduler.before_move_source_removed` is the second half of the same move, one file at a
time (RD-150-02). Across devices a file is copied under a temporary name, both sides are
hashed, the verified copy is renamed into place, and only then is the original removed; the
point sits right before that removal, where both copies exist and are identical. Its case
asserts that the next pass recognises the identical target as this file, removes the original
and records the move, rather than filing the payload a second time as `name (1)`. A stop
earlier than that leaves the original and at most a temporary file, which the next copy
discards; the unit tests of `rd_files::place_verified` walk those states one by one, because a
test cannot make `rename` answer `EXDEV`.

`scheduler.before_mirror_promoted` is the mirror fallback's own two-phase step (RD-110-20).
When the link that held a mirror group's turn fails for a reason that lies at the hoster, the
queue records that failure and then promotes the next member of the group. The two writes go
through the serialized writer as separate commands and cannot share a transaction, so a stop
between them leaves a group in which nothing is running and every remaining mirror stands by
as `Skipped`. That state is invisible to the dispatcher, which only ever looks at rows that
are already queued, so the group would wait for a link that has already given up — for as
long as the install lives. Its case therefore asserts the queue-level form of invariant 4:
the start that follows finds the group with nobody holding it and gives the turn to the next
mirror, and it does so without the person touching anything.

`scheduler.before_auto_retry_requeued` is the automatic retry of failed downloads (RD-191-12).
A round is two writes, each a single statement: the due time on the failed row, then — once it
has passed — the row back to `queued` with its attempts and limit waits at zero and the round
counted. A stop between deciding that a row is due and the second write leaves the row exactly as
the first write left it. The case asserts the queue-level form of invariant 4: after the next
start the pass puts the download back into the queue once, counts one round and nothing else; the
round is never counted for a row that stayed failed, and never twice.

`scheduler.after_queue_pause_recorded` is the timed pause of the whole queue (RD-190-20). The
pause is recorded first — its end and the files it stops — and the files are paused one by one
after it, each a write of its own. A stop between the two leaves the record and files that are
still queued. The case asserts the queue-level form of invariant 4: the start that follows holds
the queue again before its first dispatch, so none of those files starts before the end, and a
pause whose end passed while the service was down queues its files again on the first tick
rather than leaving them paused for good. The order is the point: the other way round, a stop
after the files and before the record would leave them paused with no end anybody remembers.

`postprocess.before_unpack_recorded` is the pipeline's step between an archive being unpacked
and its step being recorded (RD-180-12, `rd_extract::unpack_job`). A stop there leaves the payload
in the package folder, a step still `Running` and a package still `Postprocessing`, which is the
state `ExtractionService::recover` looks for at every start. Its case asserts that the restart
unpacks the set again into the same place — replacing what the first run wrote, never beside it —
records one step for it and completes the package. A kill *inside* the extraction leaves more: the
staging directory the archive was being written into, with part of its output. Nothing removed
that before RD-180-12; now every extraction first removes the staging directories a killed one
left in its destination (only one job runs at a time, so any that is there is stale), and the case
plants one to prove it goes. The case runs with `rd-extract/failpoints`.

`postprocess.before_direct_unpack_adopted` is direct unpack (RD-1100-07, `rd_extract::direct_unpack`)
between a set unpacked while its package was still downloading and the pipeline moving that
output into the package. Everything the direct unpack writes goes into a staging directory of its
own in the package folder (`.rd-xd…`), so neither a stop there nor a kill while `unrar` is still
waiting for the next volume — nor a pause, which abandons the attempt — leaves half a file at the
destination. Which staging directory belongs to which set is only known in memory, so the start
that follows cannot trust one it finds: the pipeline removes every one it does not hold a result
for and unpacks the set the normal way. The case asserts exactly that: the restart leaves no
staging directory, records one completed unpack step for the set, writes the same bytes as an
uninterrupted run and completes the package. It also plants the staging directory a kill inside
the tool would leave, to prove it goes too. The case runs with `rd-extract/failpoints`.

`postprocess.before_scan_recorded` is the malware scan between `clamd` answering and the scan
step recording the verdict (RD-190-14, `rd_extract::malware_scan`). The scan itself writes nothing
but its step, so a stop there leaves the step `Running` and the package `Postprocessing`, and the
start that follows runs the pipeline again. The scan is never taken from an earlier pass — it runs
on every pass, having no side effects — so the case asserts that the restart asks `clamd` again,
that a finding then fails the package with the steps after the scan recorded as skipped, and that
the package was at no point `Completed`. The case runs with `rd-extract/failpoints`.

`postprocess.after_sort_move` is the sort between one file reaching its place and the next
(RD-1100-08, `rd_extract::sort_job`). The sort writes nothing but the files it moves and its step,
so a stop there leaves some files placed, the rest in the package, the step `Running` and the
package `Postprocessing`. A placed file is no longer in the package, so the next pass cannot move
it twice; what it has to get right is the rest. A video's companions move before the video, so the
video — by whose name the rest is recognised — is the last thing to leave. The case stops after the
first companion, asserts that the video stayed, and that the restart places the video and the
other companion beside the first with nothing under a second name, removes the emptied package
folder and completes the package. The case runs with `rd-extract/failpoints`.

`automation.before_outcome_recorded` is an automation run between an action taking effect and the
run recording it (RD-180-12, `rd_api_core::automation_service`). The run is claimed as `running`
before its action executes, and a start queues every `running` run again before it does anything
else. Its case asserts that the run is not due until that recovery, that the recovery puts it back
at the same action — which therefore runs a second time: an action is carried out at least once,
so a webhook's receiver or a script can see the same event twice after a stop — and that it then
completes, never skipping an action whose outcome nobody recorded. The case runs with `rd-api-core/failpoints`.

`subscription.after_items_archived` is a subscription poll between archiving its accepted items
and handing them to the LinkGrabber (RD-190-13, `rd_api_core::subscription_service`). An accepted
item is archived as pending and marked queued only once the LinkGrabber has it, so a stop there
leaves pending items and no LinkGrabber entry. Its case asserts, for a git-release subscription's
file, that the next poll finds the file archived and hands nothing over a second time, and that
the file is still pending, where the review list queues it by hand. An automatic queueing after
such a stop is deliberately not attempted: a pending item of an auto-queue subscription can also
be a backlog item kept for review, and the archive does not tell the two apart. The case runs
with `rd-api-core/failpoints`.

`torrent.before_seed_completed` is the end of a seed (RD-180-12, `rd_torrent::seeding`): the seed
time is folded into its total and the torrent left the session, but the queue row still says
`seeding`. Its case asserts that the row stays `seeding` rather than being lost or completed
without its stop, that the start's `recover` takes the torrent up again, that the seeded time is
the total the first run closed and not that twice, and that stopping it then completes the row.
Both sessions in the case are offline. The case runs with `rd-torrent/failpoints`.

`torrent.before_relocation_commit` and `torrent.after_relocation_commit` bracket the one step of a
torrent move (RD-1100-10, `rd_torrent::relocate`) that decides its outcome: pointing the package
at the new folder. The move writes a journal (`from`, `to`) into the row's torrent state before
the first file moves, places every file at the new place verified (`rd_files::place_verified`)
and releases the originals only after the package names the new folder. A start that finds the
journal reads the package: still the old folder means the move is taken back — files that were
renamed go back, copies and temporary copies are removed — and the new folder means it is
finished — originals still there are released. Either way the journal is cleared and a seed is
taken up again from where the package says. The two cases, one per point, assert that each file
exists exactly once and at the right place, that the other folder is gone, and that the restarted
seed is registered again; RD-1100-10 adds them behind the feature, so the `rd-torrent` count is
measured with the next run. Both sessions in the cases are offline.

`plugin_transfer.before_checkpoint_saved` is a plugin transfer that stopped with bytes on disk
before the runner saved the backend's checkpoint (RD-180-12, `rd_plugin_transfer::runner`). The
part file is the resume state; the pin saved before the first byte binds it to the backend version
that wrote it.
Its case asserts that the part file holds exactly the source's first bytes, that the next run
checks the remote file against what the first one saw and continues from the part file's length,
and that the finished file matches the source byte for byte. The case found a defect on its first
pass: the runner sized the part file to the whole payload before the transfer began, so a stopped
transfer continued from its end with nothing but zeros behind what had arrived — any stop, not only
this one. The part file is no longer preallocated. A stop here used to leave no pin, so a newer
version of the backend installed before the next run continued a file the older one began; the
runner now pins the version before the first byte (RD-1120-18), and the case also asserts that
the pin is there after the stop.

`plugin_transfer.after_pin_saved` is the moment between that pin and the first byte. A transfer
without a pin discards whatever its staging file holds — no build can be named for those bytes —
and records the pin before the backend writes; a pin it cannot record keeps it from starting.
Its cases stop there and assert that the pin names the version and holds no checkpoint, that the
next run on the same version finishes with the source's bytes, and that a run after an upgrade
that took the pinned version away begins anew on the newest backend instead of failing with
`plugin.pinned_version_missing`, which stays the answer whenever the pin holds a checkpoint or the
staging file holds bytes. Both cases drive the reference backend component and run with
`rd-plugin-transfer/failpoints` where the components are built.

`plugin.before_install_recorded` and `plugin.before_pointers_followed` are the two writes an
automatic update makes after its version folder exists (RD-180-12, `install_offer` in
`rd_api_admin::plugin_repository_handlers`): the repository row that says where the version came
from, then the version pointers that follow it. Their cases run the refresh with the point armed
on a plugin pointed at its installed version, and assert that both versions are installed whole
and listed once with no staging folder beside them, that the pointer still names the old version
with no rollback target recorded, and that the start after the stop runs the old version. The
missing repository row costs a third-party repository its reach over that version when it later
withdraws it; the pointers that did not move leave the update waiting in the plugin manager to be
activated by hand. The cases run in the admin suite with `rd-api/failpoints`.

## Migration baselines

`crates/rd-db/tests/database/migration_forward/` upgrades a database from each shipped release and
asserts that its queue survives, together with every row a later migration rewrites. A baseline
is the highest migration number that release carried. The count differs from it once the chain
has gaps (`0066`, `0068`, `0088` were never shipped):

| Release | Last migration | Migrations |
| --- | --- | --- |
| 0.6.0 | `0033` | 33 |
| 0.9.2 | `0047` | 47 |
| 1.0.0 | `0055` | 55 |
| 1.1.0 | `0087` | 85 |
| 1.2.0 | `0093` | 90 |
| 1.3.0 | `0096` | 93 |
| 1.4.0 | `0098` | 95 |
| 1.5.0 | `0108` | 105 |

Read off with `git ls-tree -r --name-only v<release> crates/rd-db/migrations`: the last file
and the line count. 1.5.0 is the head of the chain today, so it upgrades by nothing until the
next migration lands.

Every baseline gets the same fixture (`migration_forward/fixture.rs`), written as that release
would have written it, and every baseline must end in the same state: packages and downloads
in several states with their byte counts, a chunk checkpoint, one default storage root and one
default category, an account with its secret reference, finish times (`0048`), PAR2 recovery
flags (`0067`), extraction codes (`0069`), the LinkGrabber's shared order (`0074`), the merged
site-rule groups (`0095`) and the official plugin repository (`0097`).

The fixtures are built by applying the first *N* migrations, not by keeping a database file
from an old build. The migrations *are* the definition of what a release's schema was, so
this reconstructs it exactly, from the repository alone, and the result is reviewable in a
diff rather than a binary blob nobody can read.

## Downgrade

There is no downgrade path, and adding `.down.sql` files would not create one: a migration
that drops a column cannot be reversed, because the data is gone. The honest answer is the copy
from before the upgrade (RD-170-07): a start that finds migrations pending on an existing
database first writes it with `VACUUM INTO` to
`<data>/pre-migration/rdownloader-<from>-to-<to>-<timestamp>.sqlite3`, keeping the newest
three. sqlx applies every migration in a transaction of its own, so a failing one leaves nothing
of itself but the ones before it in the same start stay applied, and the previous build refuses
a file with migrations it does not know. A failure therefore puts the copy back in place of the
database and ends the start with `db.migration_failed`, naming the copy; the previous version
starts on the file as it was. `crates/rd-db/src/pre_migration_tests.rs` proves both halves with
a chain whose last migration fails after two real ones committed. The encrypted, verified
backup before an update, called by the updater, stays RD-180-03. The forward path is what
Axis C covers, in `crates/rd-db/tests/database/migration_forward/`.

## Not yet covered

Recorded here rather than left implicit, because a matrix that only lists what passes reads
as completeness it does not have:

- Usenet assembly has two crash points (RD-108-25, RD-130-22); the resume itself, which
  CRC-checks every checkpointed range against the disk rather than trusting the database, is
  covered by `assembly_resume_tests` and `resume_after_crash_tests` without a crash point.
- A plugin update stopped before its pointers followed is not moved on afterwards: the next
  refresh sees the version installed and offers nothing. That is on purpose — the rows cannot
  tell such a version from one somebody installed by hand beside a version they chose to keep —
  and it costs an activation by hand, never a half-switched plugin.
- Axis B has two cases, a download and a post-processing step; the other persistent states are
  covered by Axis A alone.
