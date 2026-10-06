//! The registry of crash points (RD-140-04): every name `rd_core::failpoint!` stops at, the crate
//! that owns the state it interrupts, and what a restart after it has to prove.
//!
//! A crate of its own, used only by tests (RD-1120-12). The `failpoint!` macro needs nothing but
//! a name, so the table has no business in the production `rd-core`: there, adding a point
//! rebuilt every one of its 46 dependants. Here it rebuilds this crate and the test targets
//! that check their points are armed by a case.

/// One registered crash point: where it stops, and what a restart then has to prove.
///
/// The registry exists so the recovery matrix is reviewable. A failpoint buried in a runner is
/// invisible; a table of them, checked against `crates/rd-core/recovery-matrix.md` by a test, is
/// a list somebody can read and notice a gap in. Registering a point is deliberately separate from
/// *using* it, so a point that is added and never covered by a case shows up as a hole rather
/// than as nothing at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CrashPoint {
    /// `<owner>.<before|after>_<what>`, the name passed to `rd_core::failpoint!`.
    pub name: &'static str,
    /// The crate that owns the state this point interrupts.
    pub owner: &'static str,
    /// What a restart after this point must demonstrate.
    pub invariant: &'static str,
}

/// Every crash point this workspace registers.
///
/// Kept sorted by name so a diff shows an addition rather than a reshuffle.
pub const CRASH_POINTS: &[CrashPoint] = &[
    CrashPoint {
        name: "archive_password.after_secret_removed",
        owner: "rd-db",
        invariant: "a sweep stopped after it removed released archive passwords from the vault and before it recorded that keeps the record and never the entry; the next start removes the record, and no row points at an entry that is gone",
    },
    CrashPoint {
        name: "archive_password.before_reference_adopted",
        owner: "rd-db",
        invariant: "archive passwords written to the vault before any row points at them lose nothing: a stopped takeover keeps every plain value, the next start removes the entries the first attempt reserved and moves the values again, and the vault ends with exactly one entry per password; a stopped write keeps the row's previous password",
    },
    CrashPoint {
        name: "automation.before_outcome_recorded",
        owner: "rd-api-core",
        invariant: "a run whose action took effect before its outcome was recorded is queued again by the next start at that same action, never left running and never moved past an action nobody recorded; the action runs again and the run completes",
    },
    CrashPoint {
        name: "backup.after_database_snapshot",
        owner: "rd-backup",
        invariant: "a database copy staged for a run that stopped before its archive was sealed is removed by the next start with the rest of the staging; the run is recorded as interrupted and nothing reaches a destination",
    },
    CrashPoint {
        name: "backup.after_retention_removal",
        owner: "rd-backup",
        invariant: "an archive retention removed at its destination before the ledger forgot it is forgotten by the next pass, which finds it gone; the ledger never lists fewer archives than the destination holds, and an archive the plan keeps is never removed",
    },
    CrashPoint {
        name: "backup.before_archive_published",
        owner: "rd-backup",
        invariant: "an archive finished in staging but not yet at its destination never appears there under its final name; the next start records the run as interrupted and removes the staging",
    },
    CrashPoint {
        name: "backup.before_archive_recorded",
        owner: "rd-backup",
        invariant: "an archive that reached its destination before the ledger recorded it stays there whole and is never removed by retention, which removes only recorded archives; the next start records the run and that destination as interrupted",
    },
    CrashPoint {
        name: "history.before_entry_committed",
        owner: "rd-db",
        invariant: "a package outcome stopped after its history entry was written and before the transaction committed leaves neither behind: the package keeps its earlier state and the history has no entry for it; the outcome written again leaves exactly one entry, and every entry committed before survives the restart",
    },
    CrashPoint {
        name: "http.after_chunk_mac",
        owner: "rd-http",
        invariant: "a finished chunk MAC that was not recorded is recomputed from the start of its chunk, never assumed",
    },
    CrashPoint {
        name: "http.after_chunk_write",
        owner: "rd-http",
        invariant: "bytes written but not recorded are re-fetched, never counted as confirmed",
    },
    CrashPoint {
        name: "http.after_db_checkpoint",
        owner: "rd-http",
        invariant: "a recorded checkpoint is resumed from exactly, re-fetching nothing before it",
    },
    CrashPoint {
        name: "http.after_part_sync",
        owner: "rd-http",
        invariant: "a durable write without its commit falls back to the older checkpoint",
    },
    CrashPoint {
        name: "http.before_piece_check",
        owner: "rd-http",
        invariant: "a chunk confirmed but not checked against its piece hashes is checked before anything builds on it, and a piece that fails isolates the source named for it",
    },
    CrashPoint {
        name: "object_storage.after_part_upload",
        owner: "rd-object-storage",
        invariant: "a part the service confirmed but that was not recorded is uploaded again under the same number, never counted as confirmed; every part recorded before is not sent again",
    },
    CrashPoint {
        name: "plugin.before_install_recorded",
        owner: "rd-api-admin",
        invariant: "an automatic update whose version folder exists before its repository row was written stays installed whole and listed once, and the version pointers stay as they were: the next start runs what they chose before the update, the newest version when they chose none",
    },
    CrashPoint {
        name: "plugin.before_pointers_followed",
        owner: "rd-api-admin",
        invariant: "an automatic update recorded with its repository before the version pointers followed it stays installed whole and listed once, and the pointers stay as they were, never half moved: the next start runs what they chose before the update, the newest version when they chose none",
    },
    CrashPoint {
        name: "plugin.before_version_promoted",
        owner: "rd-plugin-host",
        invariant: "a package written under its staging name but not yet renamed into its version folder is never loaded or listed; the next start removes it, the installed version stays the one that runs, and the next update pass installs it again",
    },
    CrashPoint {
        name: "plugin_transfer.after_pin_saved",
        owner: "rd-plugin-transfer",
        invariant: "a plugin transfer stopped after its backend version was pinned and before its first byte runs again on the pinned version and finishes with the source's bytes; when that version is gone it begins anew on the newest backend, never refused for bytes it does not have",
    },
    CrashPoint {
        name: "plugin_transfer.before_checkpoint_saved",
        owner: "rd-plugin-transfer",
        invariant: "bytes a stopped plugin transfer wrote before its checkpoint was saved are continued by the next run from the part file, on the backend version pinned before its first byte and after the remote file was checked against what the first run saw; nothing past them is counted, and the finished file matches the source byte for byte",
    },
    CrashPoint {
        name: "postprocess.after_sort_move",
        owner: "rd-extract",
        invariant: "a sort stopped after it placed a file and before it recorded the step is run again by the next start: the files still in the package are placed by the same templates, the ones already placed are neither moved again nor copied beside themselves, and the package leaves post-processing completed",
    },
    CrashPoint {
        name: "postprocess.before_direct_unpack_adopted",
        owner: "rd-extract",
        invariant: "a set unpacked directly while its package downloaded, stopped before the pipeline moved it into the package, has put nothing at the destination; the next start removes its staging directory, unpacks the set the normal way and completes the package with the same files",
    },
    CrashPoint {
        name: "postprocess.before_scan_recorded",
        owner: "rd-extract",
        invariant: "a package whose malware scan ran before its verdict was recorded is scanned again by the next start and never released on a verdict nobody recorded; a finding fails it then, with the steps after the scan skipped and not run",
    },
    CrashPoint {
        name: "postprocess.before_unpack_recorded",
        owner: "rd-extract",
        invariant: "an archive unpacked before its step was recorded is unpacked again by the next start into the same place, replacing what the first run wrote; the package leaves post-processing completed, and no staging directory, not even one a killed extraction left, survives",
    },
    CrashPoint {
        name: "pre_update.before_archive_published",
        owner: "rd-backup",
        invariant: "an archive sealed and checked before an update but not yet moved into the pre-update folder never appears there; the next start removes the staging with the unencrypted copy it held, and the next preparation seals a whole one",
    },
    CrashPoint {
        name: "pre_update.before_copy_published",
        owner: "rd-backup",
        invariant: "a database copy written before an update but not yet checked never carries a copy's name, so no rollback can pick it; the live database is untouched and opens as it was, the next start removes the partial file, and the next preparation writes a whole, checked copy",
    },
    CrashPoint {
        name: "restore.after_live_set_aside",
        owner: "rd-backup",
        invariant: "a switch to a restored state stopped after a live item was set aside and before the restored one took its place is finished by the next start, which then opens the restored database; the previous installation stays in restore-previous until that start completes",
    },
    CrashPoint {
        name: "scheduler.after_package_row",
        owner: "rd-scheduler",
        invariant: "a package row written before any of its files is dropped by the next start, never left in the queue as an empty one",
    },
    CrashPoint {
        name: "scheduler.after_queue_pause_recorded",
        owner: "rd-scheduler",
        invariant: "a timed pause recorded before its files were paused holds the queue from the next start until its end, so none of its files starts early; once the end has passed, every file it paused is queued again and none stays paused for good",
    },
    CrashPoint {
        name: "scheduler.after_torrent_selection",
        owner: "rd-scheduler",
        invariant: "a torrent row whose reviewed file selection was written before it joined the queue stays paused with that selection after the next start, never queued and never started with the default selection; resuming it starts the reviewed one",
    },
    CrashPoint {
        name: "scheduler.before_auto_retry_requeued",
        owner: "rd-scheduler",
        invariant: "a failed download whose automatic retry came due before it was put back into the queue stays failed with its due time and its round uncounted; the pass after the next start puts it back exactly once and counts one round, with its attempts and limit waits starting from zero",
    },
    CrashPoint {
        name: "scheduler.before_mirror_promoted",
        owner: "rd-scheduler",
        invariant: "a mirror group whose active member has failed before its successor was promoted is given its next mirror by the start that follows, never left waiting for a link that is not coming",
    },
    CrashPoint {
        name: "scheduler.before_move_source_removed",
        owner: "rd-scheduler",
        invariant: "a move stopped between its verified copy and the removal of the original ends on the next pass with exactly one copy, at the new place, never a second one beside it",
    },
    CrashPoint {
        name: "scheduler.before_package_move",
        owner: "rd-scheduler",
        invariant: "a package whose row already points at the new folder still finds its data and finishes the move",
    },
    CrashPoint {
        name: "scheduler.before_promote",
        owner: "rd-scheduler",
        invariant: "a payload already in its final place is adopted by the next pass, never fetched a second time",
    },
    CrashPoint {
        name: "subscription.after_items_archived",
        owner: "rd-api-core",
        invariant: "release files a poll archived before handing them to the LinkGrabber stay pending in the archive after a restart: the next poll neither hands them over a second time nor loses them, and the review list still offers them",
    },
    CrashPoint {
        name: "torrent.after_relocation_commit",
        owner: "rd-torrent",
        invariant: "a torrent move stopped after its package was pointed at the new folder and before the originals were released is finished by the next start: every file is at the new place exactly once, the old folder is left empty, the journal is cleared and a seed seeds again from the new place",
    },
    CrashPoint {
        name: "torrent.before_relocation_commit",
        owner: "rd-torrent",
        invariant: "a torrent move stopped after its files were placed in the new folder and before its package was pointed there is taken back by the next start: every file is at the old place exactly once, nothing is left in the new folder, the journal is cleared and a seed seeds again from the old place",
    },
    CrashPoint {
        name: "torrent.before_seed_completed",
        owner: "rd-torrent",
        invariant: "a seed stopped after its seed time was closed and before its row completed is still seeding after the restart, is taken up again and completes when it is stopped; the seeded time is counted once",
    },
    CrashPoint {
        name: "transfer_file.before_progress_recorded",
        owner: "rd-transfer-file",
        invariant: "bytes an FTP, SFTP or bucket transfer synced to its part file before the row recorded them are continued by the next run from the part file's length, after the remote file was checked against what the first run saw; nothing is fetched twice, and the finished file matches the source byte for byte",
    },
    CrashPoint {
        name: "update.after_leftover_set_aside",
        owner: "rd-update",
        invariant: "a portable update stopped after a leftover of the update before (its .previous, staging or .failed folder) was moved into the trash and before the trash was swept has changed nothing live: the next start records it as failed with the old version in place and the database as it was, and the next update sweeps the trash and goes through; a leftover a running program still holds never fails an update",
    },
    CrashPoint {
        name: "update.after_new_placed",
        owner: "rd-update",
        invariant: "a portable update stopped after a new entry took its place, with other entries still the old version's, is taken back by the next start, whichever version that start runs: every entry is the old version's again, the new ones leave, and a newer program restarts as the old one; nothing below the data directory changes",
    },
    CrashPoint {
        name: "update.after_previous_set_aside",
        owner: "rd-update",
        invariant: "a portable update stopped after an old entry went into .previous and before its new one took its place is taken back by the next start: the entry comes back from .previous, nothing of the new version stays and the database is left as it was, since the new version never ran",
    },
    CrashPoint {
        name: "update.before_health_check",
        owner: "rd-update",
        invariant: "a portable update recorded as switched but never proven is proven by the first start of the new version that answers, and taken back with the database copy from before the update by the next start if that first one never answered; the program is never left as a mix of both versions",
    },
    CrashPoint {
        name: "usenet.after_article_write",
        owner: "rd-usenet",
        invariant: "an article on disk without its checkpoint is truncated and fetched again, never counted as confirmed",
    },
    CrashPoint {
        name: "usenet.before_checkpoint_batch",
        owner: "rd-usenet",
        invariant: "the articles of a checkpoint batch that did not commit are on disk but fetched again, never counted as confirmed; every batch committed before stays confirmed",
    },
    CrashPoint {
        name: "usenet.before_hopeless_abort",
        owner: "rd-usenet",
        invariant: "a set judged beyond repair whose rows were not yet failed is judged again after the next start from the segments every server refused, with the same counts, and the same rows fail; no row fails before that write and none is left waiting after it",
    },
    CrashPoint {
        name: "usenet.before_traffic_flushed",
        owner: "rd-usenet",
        invariant: "counts a flush had not yet written when the process stopped are lost, at most one flush interval of traffic, and nothing else: every flush committed before stays, and counting after the next start adds to it without counting anything twice",
    },
    CrashPoint {
        name: "vault.after_orphan_removed",
        owner: "rd-db",
        invariant: "a vault sweep stopped after it removed some of the entries no cell of the database names keeps every entry a row or a settings document names; the next start removes the remaining orphans and nothing else",
    },
];

/// Looks a registered crash point up by name.
#[must_use]
pub fn crash_point(name: &str) -> Option<&'static CrashPoint> {
    CRASH_POINTS.iter().find(|point| point.name == name)
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    /// A duplicate name would silently make one entry's invariant unreachable.
    #[test]
    fn every_crash_point_is_registered_once() {
        let mut names: Vec<&str> = CRASH_POINTS.iter().map(|point| point.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "a crash point name is registered twice");
    }

    /// The naming scheme is what makes the table readable; enforce it rather than hope.
    #[test]
    fn names_follow_the_owner_dot_moment_scheme() {
        for point in CRASH_POINTS {
            let (owner, moment) = point
                .name
                .split_once('.')
                .unwrap_or_else(|| panic!("{} has no owner prefix", point.name));
            assert!(!owner.is_empty(), "{}", point.name);
            assert!(
                moment.starts_with("before_") || moment.starts_with("after_"),
                "{} does not say whether it stops before or after the step",
                point.name
            );
            assert!(!point.invariant.is_empty(), "{}", point.name);
        }
    }

    /// The table is kept sorted so a diff shows what was added.
    #[test]
    fn the_registry_is_sorted_by_name() {
        let names: Vec<&str> = CRASH_POINTS.iter().map(|point| point.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn a_registered_point_can_be_looked_up() {
        assert!(crash_point("http.after_chunk_write").is_some());
        assert!(crash_point("nothing.at_all").is_none());
    }
}
