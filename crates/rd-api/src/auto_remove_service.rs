//! Removes finished packages once they have been finished long enough (RD-094-01).
//!
//! The interface has always had a "clear completed" button, but it only ever helped somebody
//! who was looking at the queue. A machine that downloads unattended accumulates finished
//! packages until the list is useless, so this does the same thing on a delay.
//!
//! Deliberately the same removal path the button uses, rather than a direct delete: part files
//! and empty package directories are cleaned up there, and an automatic removal must not leave
//! anything a manual one would not.

use std::{collections::HashSet, time::Duration};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rd_core::{DownloadFile, DownloadPackage, DownloadState, PackageId, PackageState};

use crate::{AppState, dto::SettingsResponse};

/// Checked every minute: the delay is measured in hours, so nothing is gained by looking more
/// often, and a pass that finds the feature switched off costs one settings read.
const TICK: Duration = Duration::from_secs(60);

/// Starts the removal loop. Ends with the application state, like the auto-queue watcher.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            if let Err(error) = run_once(&state).await {
                tracing::warn!(%error, "finished packages could not be removed");
            }
        }
    });
}

/// One pass over the queue.
async fn run_once(state: &AppState) -> anyhow::Result<()> {
    // Re-read every pass, like the other supervisors, so a change applies without a restart.
    let settings = match crate::handlers::stored_settings(&state.database).await {
        Ok(settings) => settings,
        Err(error) => anyhow::bail!("{}", error.message()),
    };
    if !settings.auto_remove_finished {
        return Ok(());
    }
    let packages = state.database.list_packages().await?;
    let downloads = state.database.list_downloads().await?;
    let pending = state.extraction.pending().await;
    let due = due_packages(&packages, &downloads, &pending, &settings, Utc::now());
    if due.is_empty() {
        return Ok(());
    }
    let count = due.len();
    // Never forced: `due_packages` only ever hands over settled packages, and a race
    // that started one again must lose against the person who started it.
    match crate::package_handlers::remove_packages(state, due, false).await {
        Ok(removed) => tracing::info!(removed, of = count, "finished packages removed"),
        Err(error) => tracing::warn!(
            error = %error.message(),
            "finished packages could not be removed"
        ),
    }
    Ok(())
}

/// How far along a package's members are, as far as removing the package is concerned.
///
/// The one predicate behind both ways a package leaves the queue: the timed removal below and
/// the manual "clear the list" in `package_handlers::clear_targets`. Both exist to avoid
/// throwing away something somebody is still waiting for, so both have to answer the question
/// the same way; a second copy of these rules is how the two drift apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageProgress {
    /// A member is resolving, downloading, verifying, repairing or extracting right now.
    Running,
    /// A member has not run yet: queued, waiting for a retry, or paused by hand.
    Waiting,
    /// A torrent member is still seeding. Its payload is in use even though it finished.
    Seeding,
    /// Every member settled, and every one of them succeeded.
    ///
    /// A skipped mirror counts as success: the file it stood in for came down another link.
    Finished,
    /// Every member settled, but at least one failed, was blocked or was cancelled.
    Unfinished,
}

impl PackageProgress {
    /// How strongly a single member's state speaks for the whole package. The highest one
    /// among the members wins: one running file outranks any number of finished ones.
    const fn rank(self) -> u8 {
        match self {
            Self::Finished => 0,
            Self::Unfinished => 1,
            Self::Waiting => 2,
            Self::Seeding => 3,
            Self::Running => 4,
        }
    }

    /// The stable error code that names why a package may not be cleared, if any.
    pub(crate) const fn blocking_code(self) -> Option<&'static str> {
        match self {
            Self::Running | Self::Waiting => Some("package.members_active"),
            Self::Seeding => Some("package.members_seeding"),
            Self::Finished | Self::Unfinished => None,
        }
    }
}

/// The verdict for one package, read from the states of its members.
///
/// A package with no members at all reports `Finished`: there is nothing left to wait for, and
/// the row is exactly the leftover both removal paths are meant to clean up.
pub(crate) fn package_progress(
    downloads: &[DownloadFile],
    package_id: PackageId,
) -> PackageProgress {
    downloads
        .iter()
        .filter(|file| file.package_id == package_id)
        .map(|file| match file.state {
            DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Repairing
            | DownloadState::Extracting => PackageProgress::Running,
            DownloadState::Seeding => PackageProgress::Seeding,
            DownloadState::Queued | DownloadState::RetryWait | DownloadState::Paused => {
                PackageProgress::Waiting
            }
            DownloadState::Failed | DownloadState::Blocked | DownloadState::Cancelled => {
                PackageProgress::Unfinished
            }
            DownloadState::Completed | DownloadState::Skipped => PackageProgress::Finished,
        })
        // The loudest member decides, so the answer does not depend on the row order.
        .max_by_key(|progress| progress.rank())
        .unwrap_or(PackageProgress::Finished)
}

/// The packages that are due for removal.
///
/// Kept free of I/O so the rules are testable on their own: they are the whole feature, and
/// every one of them exists to avoid removing something somebody still wants.
fn due_packages(
    packages: &[DownloadPackage],
    downloads: &[DownloadFile],
    pending_extraction: &HashSet<PackageId>,
    settings: &SettingsResponse,
    now: DateTime<Utc>,
) -> Vec<PackageId> {
    let delay = ChronoDuration::hours(i64::from(settings.auto_remove_delay_hours));
    packages
        .iter()
        .filter(|package| package.state == PackageState::Completed)
        .filter(|package| {
            package
                .completed_at
                .is_some_and(|finished| finished + delay <= now)
        })
        // Post-processing still has the package in hand; its files are being written to.
        .filter(|package| !pending_extraction.contains(&package.id))
        .filter(|package| match package_progress(downloads, package.id) {
            // A package counts as finished only when every file did.
            PackageProgress::Finished => true,
            // Still working, still waiting, or still seeding: none of those is ours to remove.
            PackageProgress::Running | PackageProgress::Waiting | PackageProgress::Seeding => false,
            // Settled, but something did not succeed — a loose end worth looking at, unless
            // the operator said otherwise.
            PackageProgress::Unfinished => !settings.auto_remove_keep_failed,
        })
        .map(|package| package.id)
        .collect()
}

/// Shared with `package_clear`, which tests the same predicate from the manual side and would
/// otherwise need a second copy of these thirty fields.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn settings(delay_hours: u32, keep_failed: bool) -> SettingsResponse {
        SettingsResponse {
            auto_remove_finished: true,
            auto_remove_delay_hours: delay_hours,
            auto_remove_keep_failed: keep_failed,
            ..SettingsResponse::default()
        }
    }

    pub(crate) fn package(
        state: PackageState,
        completed_at: Option<DateTime<Utc>>,
    ) -> DownloadPackage {
        DownloadPackage {
            id: PackageId::new(),
            name: "Release".to_owned(),
            state,
            created_at: Utc::now(),
            destination: "/tmp".to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            position: 0,
            has_password: false,
            password: None,
            kind: rd_core::DownloadKind::Http,
            nzb_import_id: None,
            completed_at,
            postprocess_level: None,
            script: None,
            postprocess: rd_core::PostprocessStatus::default(),
            extraction_result: None,
            enrichment: Vec::new(),
        }
    }

    pub(crate) fn file(package_id: PackageId, state: DownloadState) -> DownloadFile {
        let now = Utc::now();
        DownloadFile {
            recording: None,
            id: rd_core::DownloadId::new(),
            package_id,
            source: "https://example.test/a.bin".parse().expect("url"),
            file_name: "a.bin".to_owned(),
            state,
            total_bytes: None,
            committed_bytes: rd_core::ByteCount::default(),
            retry_count: 0,
            next_retry_at: None,
            expected_checksum: None,
            computed_checksum: None,
            last_error: None,
            account_id: None,
            proxy_profile_id: None,
            remote_credential_id: None,
            mirror_group: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            position: 0,
            kind: rd_core::DownloadKind::Http,
            nzb_file_id: None,
            recovery: false,
            media: None,
            enrichment: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn a_package_waits_out_the_delay() {
        let now = Utc::now();
        let fresh = package(
            PackageState::Completed,
            Some(now - ChronoDuration::hours(1)),
        );
        let stale = package(
            PackageState::Completed,
            Some(now - ChronoDuration::hours(25)),
        );
        let packages = vec![fresh, stale.clone()];

        let due = due_packages(&packages, &[], &HashSet::new(), &settings(24, true), now);

        assert_eq!(due, vec![stale.id], "only the one past the delay");
    }

    #[test]
    fn an_unfinished_package_is_never_due() {
        let now = Utc::now();
        let packages = vec![package(
            PackageState::Downloading,
            Some(now - ChronoDuration::hours(48)),
        )];

        let due = due_packages(&packages, &[], &HashSet::new(), &settings(1, true), now);

        assert!(due.is_empty(), "a package that is not finished stays");
    }

    #[test]
    fn a_package_without_a_finish_time_is_left_alone() {
        let now = Utc::now();
        let packages = vec![package(PackageState::Completed, None)];

        let due = due_packages(&packages, &[], &HashSet::new(), &settings(1, true), now);

        assert!(due.is_empty(), "nothing to measure the delay against");
    }

    #[test]
    fn a_failed_file_keeps_its_package_unless_that_is_switched_off() {
        let now = Utc::now();
        let package = package(
            PackageState::Completed,
            Some(now - ChronoDuration::hours(4)),
        );
        let downloads = vec![
            file(package.id, DownloadState::Completed),
            file(package.id, DownloadState::Failed),
        ];
        let packages = vec![package.clone()];

        let kept = due_packages(
            &packages,
            &downloads,
            &HashSet::new(),
            &settings(1, true),
            now,
        );
        assert!(kept.is_empty(), "the failed file is a loose end");

        let removed = due_packages(
            &packages,
            &downloads,
            &HashSet::new(),
            &settings(1, false),
            now,
        );
        assert_eq!(
            removed,
            vec![package.id],
            "unless the setting says otherwise"
        );
    }

    #[test]
    fn a_seeding_torrent_is_never_removed() {
        let now = Utc::now();
        let package = package(
            PackageState::Completed,
            Some(now - ChronoDuration::hours(4)),
        );
        let downloads = vec![file(package.id, DownloadState::Seeding)];
        let packages = vec![package];

        // Even with the loose-end rule switched off: seeding is work in progress, not a remnant.
        let due = due_packages(
            &packages,
            &downloads,
            &HashSet::new(),
            &settings(1, false),
            now,
        );

        assert!(due.is_empty());
    }

    #[test]
    fn a_package_still_being_unpacked_waits() {
        let now = Utc::now();
        let package = package(
            PackageState::Completed,
            Some(now - ChronoDuration::hours(4)),
        );
        let pending = HashSet::from([package.id]);
        let packages = vec![package];

        let due = due_packages(&packages, &[], &pending, &settings(1, true), now);

        assert!(due.is_empty());
    }

    #[test]
    fn one_running_member_outranks_any_number_of_finished_ones() {
        let id = PackageId::new();
        let downloads = vec![
            file(id, DownloadState::Completed),
            file(id, DownloadState::Downloading),
            file(id, DownloadState::Completed),
        ];

        assert_eq!(package_progress(&downloads, id), PackageProgress::Running);
        assert!(package_progress(&downloads, id).blocking_code().is_some());
        assert_eq!(
            package_progress(&downloads, id).blocking_code(),
            Some("package.members_active")
        );
    }

    #[test]
    fn a_waiting_member_blocks_just_as_a_running_one_does() {
        let id = PackageId::new();
        for state in [
            DownloadState::Queued,
            DownloadState::RetryWait,
            DownloadState::Paused,
        ] {
            let downloads = vec![file(id, DownloadState::Completed), file(id, state)];

            assert_eq!(
                package_progress(&downloads, id),
                PackageProgress::Waiting,
                "{state:?} is work that has not happened yet"
            );
            assert_eq!(
                package_progress(&downloads, id).blocking_code(),
                Some("package.members_active")
            );
        }
    }

    #[test]
    fn a_skipped_mirror_still_counts_as_finished() {
        let id = PackageId::new();
        let downloads = vec![
            file(id, DownloadState::Completed),
            file(id, DownloadState::Skipped),
        ];

        assert_eq!(package_progress(&downloads, id), PackageProgress::Finished);
        assert_eq!(package_progress(&downloads, id).blocking_code(), None);
    }

    #[test]
    fn a_failed_member_settles_the_package_without_finishing_it() {
        let id = PackageId::new();
        let downloads = vec![
            file(id, DownloadState::Completed),
            file(id, DownloadState::Failed),
        ];

        assert_eq!(
            package_progress(&downloads, id),
            PackageProgress::Unfinished
        );
        assert!(package_progress(&downloads, id).blocking_code().is_none());
    }

    #[test]
    fn seeding_blocks_with_a_code_of_its_own() {
        let id = PackageId::new();
        let downloads = vec![file(id, DownloadState::Seeding)];

        assert_eq!(package_progress(&downloads, id), PackageProgress::Seeding);
        assert_eq!(
            package_progress(&downloads, id).blocking_code(),
            Some("package.members_seeding")
        );
    }

    #[test]
    fn the_members_of_other_packages_do_not_count() {
        let id = PackageId::new();
        let downloads = vec![
            file(id, DownloadState::Completed),
            file(PackageId::new(), DownloadState::Downloading),
        ];

        assert_eq!(package_progress(&downloads, id), PackageProgress::Finished);
    }
}
