//! Package by package (RD-1410-01): with `sequential_packages` on, a file that takes a counted
//! place starts only while no package ahead of its own still has a waiting file.
//!
//! Off, the pass offers a free place to the first file in list order that may run, in whatever
//! package it is (RD-1250-07): a file at the head that waits for a hoster's limit, a stated wait
//! or a busy host leaves its place to a later package, which then holds it for a whole transfer.
//! On, a waiting file -- `Queued` or `RetryWait`, however long the wait -- holds every package
//! after its own; the files of its own package still start. A file that started holds nothing,
//! so with more than one place the next package starts once every file of the earlier one has
//! begun, in the same pass. What holds nothing either: a file that is not waiting (running,
//! paused, failed, blocked, skipped, finished), a file of a kind that runs beside the limit or
//! shares one place, a file of a switched-off kind, a file the dispatcher may not touch now, and
//! every file of a package that is not due or that its download window or the schedule's pause
//! holds -- otherwise the whole list would stand until its moment.

use std::collections::HashSet;

use rd_core::{DownloadFile, DownloadId, DownloadState, PackageId};

use crate::SchedulerHandle;

/// One pass's view of the package order: fed every row in list order through
/// [`PackageOrder::look_at`], asked [`PackageOrder::admits`] for the rows that may start, told
/// [`PackageOrder::started`] for those that did.
#[derive(Debug)]
pub(super) struct PackageOrder {
    /// The waiting files that hold the packages after theirs until they start.
    holders: HashSet<DownloadId>,
    /// The holder the pass looked at last; whether it started is known once the pass moves on.
    last: Option<(DownloadId, PackageId)>,
    /// The first package in list order with a holder that did not start in this pass.
    waiting: Option<PackageId>,
}

impl PackageOrder {
    /// The order over `files`, the pass's rows in list order; `holds_nothing` names the waiting
    /// files that do not hold the packages after theirs.
    pub(super) fn new<'a>(
        files: impl IntoIterator<Item = &'a DownloadFile>,
        holds_nothing: impl Fn(&DownloadFile) -> bool,
    ) -> Self {
        let holders = files
            .into_iter()
            .filter(|file| {
                matches!(file.state, DownloadState::Queued | DownloadState::RetryWait)
                    && !holds_nothing(file)
            })
            .map(|file| file.id)
            .collect();
        Self {
            holders,
            last: None,
            waiting: None,
        }
    }

    /// The pass reached `file`: every row, in list order, before anything skips it.
    pub(super) fn look_at(&mut self, file: &DownloadFile) {
        // The holder looked at before did not start, or `started` would have cleared it.
        if let Some((_, package)) = self.last.take() {
            self.waiting.get_or_insert(package);
        }
        if self.holders.contains(&file.id) {
            self.last = Some((file.id, file.package_id));
        }
    }

    /// Whether `file`, the row looked at last, may take a counted place: no earlier package has
    /// a file still waiting.
    pub(super) fn admits(&self, file: &DownloadFile) -> bool {
        self.waiting
            .is_none_or(|package| package == file.package_id)
    }

    /// `id` took its place in this pass; it holds nothing any more.
    pub(super) fn started(&mut self, id: DownloadId) {
        if self.last.is_some_and(|(last, _)| last == id) {
            self.last = None;
        }
    }
}

impl SchedulerHandle {
    /// The package order of this pass over `files`, or `None` while `sequential_packages` is off;
    /// `held` names the files whose package is not due or held by its window or the schedule.
    pub(super) async fn package_order(
        &self,
        files: &[DownloadFile],
        held: impl Fn(&DownloadFile) -> bool,
    ) -> Option<PackageOrder> {
        if !self
            .sequential_packages
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        let disabled = self.disabled_kinds.lock().await.clone();
        let active = self.active.lock().await;
        Some(PackageOrder::new(files, |file| {
            held(file)
                || !self.takes_a_counted_place(file.kind)
                || disabled.contains(&file.kind)
                || active.untouchable(&file.id)
        }))
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use rd_core::{DownloadFile, DownloadState, Failure, FailureKind, PackageId};

    use super::PackageOrder;

    /// A fixed clock: 2026-11-02 12:00:00 UTC.
    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_793_620_800, 0).expect("a valid instant")
    }

    fn file(package: PackageId, state: DownloadState) -> DownloadFile {
        DownloadFile {
            recording: None,
            id: rd_core::DownloadId::new(),
            package_id: package,
            source: "https://files.example/episode.mkv".parse().expect("url"),
            file_name: "episode.mkv".to_owned(),
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
            position: 1,
            kind: rd_core::DownloadKind::Http,
            nzb_file_id: None,
            recovery: false,
            media: None,
            enrichment: Vec::new(),
            created_at: now(),
            updated_at: now(),
        }
    }

    /// A file whose hoster's limit holds it for an hour.
    fn limited(package: PackageId) -> DownloadFile {
        let mut file = file(package, DownloadState::RetryWait);
        file.next_retry_at = Some(now() + chrono::Duration::hours(1));
        file.last_error = Some(Failure::new(
            FailureKind::RateLimited {
                retry_after_seconds: Some(3600),
            },
            "limit",
        ));
        file
    }

    /// Runs a pass over `files` in list order and answers which rows were admitted; the rows
    /// named in `starting` start when admitted.
    fn pass(
        files: &[DownloadFile],
        holds_nothing: impl Fn(&DownloadFile) -> bool,
        starting: &[rd_core::DownloadId],
    ) -> Vec<bool> {
        let mut order = PackageOrder::new(files, holds_nothing);
        files
            .iter()
            .map(|file| {
                order.look_at(file);
                let admitted = order.admits(file);
                if admitted && starting.contains(&file.id) {
                    order.started(file.id);
                }
                admitted
            })
            .collect()
    }

    #[test]
    fn a_file_waiting_an_hour_for_its_limit_holds_the_packages_after_its_own() {
        let (first, second) = (PackageId::new(), PackageId::new());
        let files = [
            limited(first),
            file(first, DownloadState::Queued),
            file(second, DownloadState::Queued),
        ];
        // The waiting file itself and the rest of its package are admitted; the next is not.
        assert_eq!(pass(&files, |_| false, &[files[1].id]), [true, true, false]);
    }

    #[test]
    fn a_file_that_started_holds_nothing_and_the_next_package_starts_in_the_same_pass() {
        let (first, second) = (PackageId::new(), PackageId::new());
        let files = [
            file(first, DownloadState::Queued),
            file(second, DownloadState::Queued),
        ];
        assert_eq!(pass(&files, |_| false, &[files[0].id]), [true, true]);
        // Not started (no place left, a busy host): the next package waits.
        assert_eq!(pass(&files, |_| false, &[]), [true, false]);
    }

    #[test]
    fn files_that_do_not_wait_hold_nothing() {
        let (first, second) = (PackageId::new(), PackageId::new());
        let mut files: Vec<DownloadFile> = [
            DownloadState::Downloading,
            DownloadState::Paused,
            DownloadState::Failed,
            DownloadState::Blocked,
            DownloadState::Skipped,
            DownloadState::Cancelled,
            DownloadState::Completed,
        ]
        .into_iter()
        .map(|state| file(first, state))
        .collect();
        files.push(file(second, DownloadState::Queued));
        assert!(
            pass(&files, |_| false, &[])
                .iter()
                .all(|admitted| *admitted)
        );
    }

    #[test]
    fn a_package_not_due_or_held_by_its_window_holds_nothing() {
        let (held, next) = (PackageId::new(), PackageId::new());
        let files = [limited(held), file(next, DownloadState::Queued)];
        assert_eq!(
            pass(&files, |file| file.package_id == held, &[]),
            [true, true]
        );
    }

    #[test]
    fn the_list_order_decides_which_package_goes_first() {
        // The rows come in list order -- priority first, then position -- so the package of
        // higher priority is the first one here, and the earlier package of lower priority waits.
        let (urgent, earlier) = (PackageId::new(), PackageId::new());
        let files = [
            file(urgent, DownloadState::Queued),
            file(earlier, DownloadState::Queued),
        ];
        assert_eq!(pass(&files, |_| false, &[]), [true, false]);
    }

    #[test]
    fn an_order_over_no_waiting_file_admits_everything() {
        let none: [DownloadFile; 0] = [];
        let order = PackageOrder::new(&none, |_| false);
        assert!(order.admits(&file(PackageId::new(), DownloadState::Queued)));
    }
}
