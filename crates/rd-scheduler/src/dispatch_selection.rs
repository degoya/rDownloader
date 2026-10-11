//! Which waiting file a dispatch pass offers a place to, and which keeps its own (RD-1250-07).
//!
//! The pass reads the startable rows in list order: package priority, then the package's
//! position, then the file's position in it. At equal priority the first file in that order
//! that may run starts, and no place is left empty while one may. A file whose attempt failed
//! and waits for its retry used to give its place up at once: the next pass, half a second
//! later, handed it to the next file in the list, and when the retry was due that file held the
//! place for a whole transfer. Every transient failure moved the place further down the list,
//! across packages, and left a begun file behind. A plain retry -- no wait anybody stated, the
//! backoff's first steps of one to eight seconds -- now keeps its place: no file after it in the
//! list takes the place it comes back to. Anything longer gives the place away: a hoster's limit,
//! a wait a hoster or a plugin stated, the backoff's later steps and the half hour from the sixth
//! failed attempt in a row on (`retry::LONG_RETRY_WAIT`). Once that wait is over the file comes
//! before the files after it again, because it is ahead of them in the list; nothing running
//! is stopped for it.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rd_core::{DownloadFile, DownloadKind, DownloadState};

use crate::SchedulerHandle;

/// How far a plain retry may still be away for its file to keep its place: the backoff's first
/// four steps (one, two, four, eight seconds, each spread by a tenth), not its sixteen seconds.
pub(super) const PLACE_KEPT_FOR: Duration = Duration::from_secs(10);

/// What a dispatch pass does with one startable row as of `now`.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Turn {
    /// Queued, or its retry is due: it starts unless something holds it back.
    Offered,
    /// A plain retry seconds away: it does not start yet, and the place it comes back to is
    /// not given to a file after it.
    KeepsPlace,
    /// It waits for longer, for a hoster's limit or for a wait somebody stated: the files after
    /// it may take its place.
    Waits,
}

pub(super) fn turn(file: &DownloadFile, now: DateTime<Utc>) -> Turn {
    if file.state == DownloadState::Queued {
        return Turn::Offered;
    }
    let Some(retry_at) = file.next_retry_at else {
        return Turn::Waits;
    };
    if retry_at <= now {
        return Turn::Offered;
    }
    let plain = file.last_error.as_ref().is_none_or(|failure| {
        !failure.category.is_limit() && failure.category.retry_after().is_none()
    });
    let close = (retry_at - now)
        .to_std()
        .is_ok_and(|left| left <= PLACE_KEPT_FOR);
    if close && plain {
        Turn::KeepsPlace
    } else {
        Turn::Waits
    }
}

impl SchedulerHandle {
    /// Whether a running file of `kind` takes one of the `max_active_files` places of its own:
    /// only such a file has a place to keep. Recordings run beside the limit, and the files of
    /// a pooled kind share one place.
    pub(super) fn takes_a_counted_place(&self, kind: DownloadKind) -> bool {
        match kind {
            DownloadKind::Http => true,
            kind => self.runners.get(kind).is_some_and(|runner| {
                runner.counts_against_global_limit() && !runner.shares_one_global_slot()
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};
    use rd_core::{DownloadFile, DownloadState, Failure, FailureKind};

    use super::{PLACE_KEPT_FOR, Turn, turn};

    /// A fixed clock: 2026-10-11 12:00:00 UTC.
    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_791_720_000, 0).expect("a valid instant")
    }

    fn waiting(retry_at: Option<DateTime<Utc>>, category: FailureKind) -> DownloadFile {
        DownloadFile {
            recording: None,
            id: rd_core::DownloadId::new(),
            package_id: rd_core::PackageId::new(),
            source: "https://files.example/episode.mkv".parse().expect("url"),
            file_name: "episode.mkv".to_owned(),
            state: DownloadState::RetryWait,
            total_bytes: None,
            committed_bytes: rd_core::ByteCount::default(),
            retry_count: 1,
            next_retry_at: retry_at,
            expected_checksum: None,
            computed_checksum: None,
            last_error: Some(Failure::new(category, "refused")),
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

    fn transient() -> FailureKind {
        FailureKind::Transient {
            retry_after_seconds: None,
        }
    }

    #[test]
    fn a_queued_file_and_a_due_retry_are_offered() {
        let mut queued = waiting(None, transient());
        queued.state = DownloadState::Queued;
        assert_eq!(turn(&queued, now()), Turn::Offered);
        assert_eq!(
            turn(&waiting(Some(now()), transient()), now()),
            Turn::Offered
        );
    }

    #[test]
    fn a_plain_retry_seconds_away_keeps_its_place() {
        let close = now() + Duration::seconds(4);
        assert_eq!(
            turn(&waiting(Some(close), transient()), now()),
            Turn::KeepsPlace
        );
        let edge = now() + Duration::from_std(PLACE_KEPT_FOR).expect("duration");
        assert_eq!(
            turn(&waiting(Some(edge), FailureKind::Offline), now()),
            Turn::KeepsPlace
        );
        // The backoff's fourth step (eight seconds) at the top of its spread keeps the place,
        // its fifth (sixteen) at the bottom of its spread does not.
        let spread = u64::from(rd_core::RETRY_JITTER_PERCENT);
        let kept = u64::try_from(PLACE_KEPT_FOR.as_millis()).expect("milliseconds");
        assert!(8_000 * (100 + spread) / 100 <= kept);
        assert!(16_000 * (100 - spread) / 100 > kept);
    }

    #[test]
    fn a_longer_wait_a_stated_wait_and_a_hoster_limit_leave_the_place() {
        let far =
            now() + Duration::from_std(PLACE_KEPT_FOR).expect("duration") + Duration::seconds(1);
        assert_eq!(turn(&waiting(Some(far), transient()), now()), Turn::Waits);
        let close = now() + Duration::seconds(5);
        for stated in [
            FailureKind::Transient {
                retry_after_seconds: Some(5),
            },
            FailureKind::RateLimited {
                retry_after_seconds: Some(5),
            },
            FailureKind::IpBlocked {
                retry_after_seconds: Some(5),
            },
            FailureKind::RateLimited {
                retry_after_seconds: None,
            },
        ] {
            assert_eq!(turn(&waiting(Some(close), stated), now()), Turn::Waits);
        }
        assert_eq!(turn(&waiting(None, transient()), now()), Turn::Waits);
    }
}
