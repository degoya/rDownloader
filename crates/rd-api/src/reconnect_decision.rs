//! When a reconnect is worth attempting.
//!
//! Kept apart from the running of it so the rules can be read and tested on their own. Every
//! condition here exists to avoid the two ways this feature goes wrong: reconnecting when it
//! cannot help, and reconnecting so often that it becomes the problem.

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use rd_core::{DownloadFile, DownloadState, FailureKind};

/// What the decision is made from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReconnectInputs<'a> {
    pub enabled: bool,
    pub has_script: bool,
    /// Empty means any time.
    pub windows: &'a [rd_limits::QuietWindow],
    pub timezone: Tz,
    pub min_interval_minutes: u32,
    pub last_attempt: Option<DateTime<Utc>>,
    /// Whether running transfers may be paused to make room for the attempt.
    pub abort_active: bool,
    pub now: DateTime<Utc>,
}

/// Why a reconnect is not being attempted, or that it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReconnectVerdict {
    Go,
    Disabled,
    NoScript,
    OutsideWindow,
    TooSoon,
    NothingWaiting,
    TransfersRunning,
}

/// Decides whether to reconnect now.
pub(crate) fn evaluate(
    inputs: &ReconnectInputs<'_>,
    downloads: &[DownloadFile],
) -> ReconnectVerdict {
    if !inputs.enabled {
        return ReconnectVerdict::Disabled;
    }
    if !inputs.has_script {
        return ReconnectVerdict::NoScript;
    }
    if !inside_window(inputs) {
        return ReconnectVerdict::OutsideWindow;
    }
    if let Some(last) = inputs.last_attempt {
        let wait = Duration::minutes(i64::from(inputs.min_interval_minutes));
        if last + wait > inputs.now {
            return ReconnectVerdict::TooSoon;
        }
    }
    // The whole point is a different address. Nothing waiting on one means nothing to gain.
    if !downloads.iter().any(waiting_on_the_address) {
        return ReconnectVerdict::NothingWaiting;
    }
    if !inputs.abort_active && downloads.iter().any(|file| is_transferring(file.state)) {
        return ReconnectVerdict::TransfersRunning;
    }
    ReconnectVerdict::Go
}

/// A file held back by a limit that follows the address rather than the link.
///
/// Only account-less files: a premium download is not subject to the IP limit, so reconnecting
/// would interrupt it for nothing.
fn waiting_on_the_address(file: &DownloadFile) -> bool {
    file.account_id.is_none()
        && file.state == DownloadState::RetryWait
        && matches!(
            file.last_error.as_ref().map(|failure| &failure.category),
            Some(FailureKind::IpBlocked { .. })
        )
}

/// States that lose work if the connection drops under them.
const fn is_transferring(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Downloading | DownloadState::Resolving | DownloadState::Seeding
    )
}

fn inside_window(inputs: &ReconnectInputs<'_>) -> bool {
    if inputs.windows.is_empty() {
        return true;
    }
    let (weekday, minute) = rd_limits::local_position(inputs.timezone, inputs.now);
    inputs.windows.iter().any(|window| {
        rd_limits::covers_local(
            window.days,
            window.start_minute,
            window.end_minute,
            weekday,
            minute,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{ReconnectInputs, ReconnectVerdict, evaluate};
    use chrono::{Duration, Utc};
    use rd_core::{DownloadFile, DownloadState, Failure, FailureKind};

    fn inputs<'a>(windows: &'a [rd_limits::QuietWindow]) -> ReconnectInputs<'a> {
        ReconnectInputs {
            enabled: true,
            has_script: true,
            windows,
            timezone: chrono_tz::Tz::UTC,
            min_interval_minutes: 10,
            last_attempt: None,
            abort_active: false,
            now: Utc::now(),
        }
    }

    fn file(state: DownloadState, blocked: bool, with_account: bool) -> DownloadFile {
        let now = Utc::now();
        DownloadFile {
            recording: None,
            id: rd_core::DownloadId::new(),
            package_id: rd_core::PackageId::new(),
            source: "https://hoster.example/a".parse().expect("url"),
            file_name: "a.bin".to_owned(),
            state,
            total_bytes: None,
            committed_bytes: rd_core::ByteCount::default(),
            retry_count: 0,
            next_retry_at: None,
            expected_checksum: None,
            computed_checksum: None,
            last_error: blocked.then(|| {
                Failure::new(
                    FailureKind::IpBlocked {
                        retry_after_seconds: Some(900),
                    },
                    "one free download at a time",
                )
            }),
            account_id: with_account.then(rd_core::AccountId::new),
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

    fn blocked_file() -> DownloadFile {
        file(DownloadState::RetryWait, true, false)
    }

    #[test]
    fn a_blocked_free_download_is_reason_enough() {
        assert_eq!(
            evaluate(&inputs(&[]), &[blocked_file()]),
            ReconnectVerdict::Go
        );
    }

    #[test]
    fn nothing_happens_while_the_feature_is_off_or_unconfigured() {
        let mut off = inputs(&[]);
        off.enabled = false;
        assert_eq!(
            evaluate(&off, &[blocked_file()]),
            ReconnectVerdict::Disabled
        );

        let mut scriptless = inputs(&[]);
        scriptless.has_script = false;
        assert_eq!(
            evaluate(&scriptless, &[blocked_file()]),
            ReconnectVerdict::NoScript
        );
    }

    #[test]
    fn an_idle_queue_is_left_alone() {
        assert_eq!(
            evaluate(&inputs(&[]), &[]),
            ReconnectVerdict::NothingWaiting
        );
    }

    #[test]
    fn a_premium_download_waiting_is_not_an_address_problem() {
        // The limit belongs to anonymous access; an account is not subject to it, so
        // reconnecting would interrupt the transfer and change nothing.
        let with_account = file(DownloadState::RetryWait, true, true);

        assert_eq!(
            evaluate(&inputs(&[]), &[with_account]),
            ReconnectVerdict::NothingWaiting
        );
    }

    #[test]
    fn a_file_waiting_for_something_else_is_not_an_address_problem() {
        let other_reason = file(DownloadState::RetryWait, false, false);

        assert_eq!(
            evaluate(&inputs(&[]), &[other_reason]),
            ReconnectVerdict::NothingWaiting
        );
    }

    #[test]
    fn a_running_transfer_is_not_interrupted_unless_that_was_asked_for() {
        let downloads = [
            blocked_file(),
            file(DownloadState::Downloading, false, false),
        ];

        assert_eq!(
            evaluate(&inputs(&[]), &downloads),
            ReconnectVerdict::TransfersRunning
        );

        let mut allowed = inputs(&[]);
        allowed.abort_active = true;
        assert_eq!(evaluate(&allowed, &downloads), ReconnectVerdict::Go);
    }

    #[test]
    fn a_seeding_torrent_counts_as_a_running_transfer() {
        let downloads = [blocked_file(), file(DownloadState::Seeding, false, false)];

        assert_eq!(
            evaluate(&inputs(&[]), &downloads),
            ReconnectVerdict::TransfersRunning
        );
    }

    #[test]
    fn two_reconnects_keep_their_distance() {
        let mut recent = inputs(&[]);
        recent.last_attempt = Some(recent.now - Duration::minutes(3));
        assert_eq!(
            evaluate(&recent, &[blocked_file()]),
            ReconnectVerdict::TooSoon
        );

        let mut long_ago = inputs(&[]);
        long_ago.last_attempt = Some(long_ago.now - Duration::minutes(30));
        assert_eq!(evaluate(&long_ago, &[blocked_file()]), ReconnectVerdict::Go);
    }

    #[test]
    fn a_window_that_does_not_cover_now_holds_the_attempt_back() {
        // A window covering no weekday at all can never be inside.
        let never = [rd_limits::QuietWindow {
            days: rd_limits::DaySet(0),
            start_minute: 0,
            end_minute: 60,
        }];

        assert_eq!(
            evaluate(&inputs(&never), &[blocked_file()]),
            ReconnectVerdict::OutsideWindow
        );
    }
}
