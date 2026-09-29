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

Axis A only exists when it is asked for, and it is each *owning* crate's feature that asks:

```bash
cargo nextest run --features rd-http/failpoints,rd-scheduler/failpoints,rd-usenet/failpoints,rd-object-storage/failpoints,rd-backup/failpoints \
    -j 2 -p rd-core -p rd-http -p rd-scheduler -p rd-usenet -p rd-object-storage -p rd-backup
```

`rd-core/failpoints` on its own is not enough, however plausible it looks. Every crash-test
file is gated on the feature of the crate that owns the point, and enabling a dependency's
feature does not enable its dependants'. A file compiled to nothing runs no cases and reports
success, which is the one failure mode this matrix cannot afford, so check the counts: without
those features `rd-http` runs 97 tests, `rd-scheduler` 111, `rd-usenet` 54, `rd-object-storage` 50
and `rd-backup` 52; with them 103, 117, 57, 51 and 54, measured per crate on 2026-09-29.

Axis A returns an error at the crash point rather than killing the process. That drops the
whole worker, the open file handle included, which is the state a restart finds — everything
except the question of whether the kernel had flushed, which is exactly and only what Axis B
is for. Paying for a spawned process at every crash point to re-prove the state machine would
buy nothing; paying for it once per flush that matters is worth it.

**Axis B must not be run on a development machine.** It spawns real service processes, which is
why its cases are `#[ignore]` and run in CI only.

## Registered crash points

This table is checked against `rd_core::failpoint::CRASH_POINTS` by a test, so it cannot
drift from the code. Adding a crash point without a row here fails the build, and so does a
row for a point that does not exist.

| Point | Owner | A restart must show |
| --- | --- | --- |
| `backup.before_archive_published` | rd-backup | an archive finished in staging but not yet at its destination never appears there under its final name; the next start records the run as interrupted and removes the staging |
| `http.after_chunk_mac` | rd-http | a finished chunk MAC that was not recorded is recomputed from the start of its chunk, never assumed |
| `http.after_chunk_write` | rd-http | bytes written but not recorded are re-fetched, never counted as confirmed |
| `http.after_db_checkpoint` | rd-http | a recorded checkpoint is resumed from exactly, re-fetching nothing before it |
| `http.after_part_sync` | rd-http | a durable write without its commit falls back to the older checkpoint |
| `http.before_piece_check` | rd-http | a chunk confirmed but not checked against its piece hashes is checked before anything builds on it, and a piece that fails isolates the source named for it |

| `object_storage.after_part_upload` | rd-object-storage | a part the service confirmed but that was not recorded is uploaded again under the same number, never counted as confirmed; every part recorded before is not sent again |
| `restore.after_live_set_aside` | rd-backup | a switch to a restored state stopped after a live item was set aside and before the restored one took its place is finished by the next start, which then opens the restored database; the previous installation stays in restore-previous until that start completes |
| `scheduler.after_package_row` | rd-scheduler | a package row written before any of its files is dropped by the next start, never left in the queue as an empty one |
| `scheduler.before_mirror_promoted` | rd-scheduler | a mirror group whose active member has failed before its successor was promoted is given its next mirror by the start that follows, never left waiting for a link that is not coming |
| `scheduler.before_move_source_removed` | rd-scheduler | a move stopped between its verified copy and the removal of the original ends on the next pass with exactly one copy, at the new place, never a second one beside it |
| `scheduler.before_package_move` | rd-scheduler | a package whose row already points at the new folder still finds its data and finishes the move |
| `scheduler.before_promote` | rd-scheduler | a payload already in its final place is adopted by the next pass, never fetched a second time |
| `usenet.after_article_write` | rd-usenet | an article on disk without its checkpoint is truncated and fetched again, never counted as confirmed |
| `usenet.before_checkpoint_batch` | rd-usenet | the articles of a checkpoint batch that did not commit are on disk but fetched again, never counted as confirmed; every batch committed before stays confirmed |

`backup.before_archive_published` is the full backup's two-phase step (RD-160-01): the
encrypted archive is complete in the staging folder below the data directory, and only then is
it handed to its destination, which copies it under a temporary name, compares both sides by
SHA-256 and renames it into place. A stop in that window leaves a finished archive nobody
recorded and a history row that still says `running`. Its case asserts that the destination
holds nothing under the archive's name, that the recovery every start runs marks the row
`interrupted` with `backup.interrupted` and empties the staging, and that the next run succeeds
beside it. The case runs with `rd-backup/failpoints`.

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

`scheduler.after_package_row` is the enqueue path's own two-phase step, and the only one of
the three where the order cannot be chosen: a download row needs a package to belong to, so
the package is always written first. A stop in that window leaves a package with nothing in
it, and nothing in the queue or the interface tells such a row apart from a package that
finished without downloading anything. Worse, it can never be removed afterwards — removing a
package is a side effect of removing its last file, and it has none. Its case therefore
asserts the folder-free form of invariant 4: `recover_interrupted`, the first thing a restart
runs, drops the empty row, and leaves a package that does have files exactly where it was.

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

## Migration baselines

`crates/rd-db/tests/migration_forward/` upgrades a database from each shipped release and
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
that drops a column cannot be reversed, because the data is gone. The honest answer is
restore-from-backup, which is why a schema-changing update takes a verified snapshot first
(RD-130-03). The forward path is what Axis C covers, in
`crates/rd-db/tests/migration_forward/`.

## Not yet covered

Recorded here rather than left implicit, because a matrix that only lists what passes reads
as completeness it does not have:

- Post-processing, automation runs, torrent seeding and the plugin transfer runner have
  recovery paths in the code but no crash points registered yet. Usenet assembly has two
  (RD-108-25, RD-130-22); the resume itself, which CRC-checks every checkpointed range against the disk
  rather than trusting the database, is covered by `assembly_resume_tests` and
  `resume_after_crash_tests` without a crash point.
- Axis B has no cases yet.
