//! Links in a package that point at the same file, once they are in the queue.
//!
//! A release is often posted to several hosters at once. Downloading every copy wastes the
//! bandwidth and the free-download slots of all of them to end up with the same bytes, so one
//! link of a group runs and the rest wait as [`DownloadState::Skipped`]. If the one that ran
//! gives up, one of the others takes over — which is the actual point: the mirrors are the
//! fallback, not a duplicate.
//!
//! **Where the group comes from (RD-110-20).** Not from here any more. Until this job the
//! queue recomputed a group of its own at enqueue (RD-094-05) from the declared file names
//! and sizes, while the LinkGrabber computed a second one (RD-110-18) from three sources —
//! what a site rule declared, name and size after the online check, name alone — kept it on
//! the candidate row and let a person change which member is selected. Two answers to the
//! same question that could disagree, and the weaker one won, because it ran last and
//! overwrote the choice. The LinkGrabber's group is now carried into the queue unchanged
//! (`rd_api::collector_enqueue`); what is left here is what the *queue* does with a group,
//! which the LinkGrabber has no business deciding: who runs, who waits, who takes over when
//! an attempt fails, and when the group is out of mirrors.

use rd_core::{DownloadFile, DownloadState, Failure, FailureKind};

/// Whether links of this transport are ever mirrors of each other.
///
/// Usenet and torrent files are members of one download each, not alternative routes to the
/// same bytes, and grouping them would hold back parts of the very thing being fetched. The
/// LinkGrabber groups on names and addresses and knows nothing of transports, so this is the
/// gate its answer passes through on the way into the queue.
#[must_use]
pub const fn groups_mirrors(kind: rd_core::DownloadKind) -> bool {
    matches!(
        kind,
        rd_core::DownloadKind::Http
            | rd_core::DownloadKind::Plugin
            | rd_core::DownloadKind::Ftp
            | rd_core::DownloadKind::Sftp
    )
}

/// Whether a failed attempt should hand the group's turn to another mirror.
///
/// Two questions, in this order.
///
/// **Is this the hoster's doing?** A cause that lies on this machine must never move the job
/// to the next mirror: the next one writes to the same full disk, into the same unwritable
/// directory, and a group of five mirrors burns five attempts to discover what the first one
/// already said. [`FailureKind`] answers this for every class but two — `Offline`,
/// `AuthRequired`, `AccountInvalid`, `RateLimited`, `IpBlocked`, `NeedsCaptcha`,
/// `CaptchaFailed` and `Unsupported` each name something the remote side did, and no local
/// condition produces them. `Permanent` and `Transient` carry both — a 404 and a failed
/// rename are both permanent, a 503 and a full disk are both transient — so for those two the
/// stable code decides, and a failure raised on this machine says so with one.
///
/// **Would waiting on this mirror help?** When the attempt is out of retries the link is done
/// for and the group's turn has to move. When a retry is still scheduled, it moves anyway for
/// every cause the same hoster will not resolve by being asked again in a minute: offline,
/// gone, refused, limited, out of captchas. Only a plain `Transient` and a rejected captcha
/// stay where they are, because those two genuinely mean "this mirror, in a moment".
#[must_use]
pub fn hands_over(failure: &Failure, retry_at: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    if is_local(failure) {
        return false;
    }
    if retry_at.is_none() {
        return true;
    }
    !matches!(
        failure.category,
        FailureKind::Transient { .. } | FailureKind::CaptchaFailed
    )
}

/// Whether the failure was raised by this machine rather than by the remote side.
#[must_use]
pub fn is_local(failure: &Failure) -> bool {
    matches!(
        failure.code.as_deref(),
        Some(rd_http::LOCAL_IO_CODE | LOCAL_PROMOTE_CODE)
    )
}

/// The file was fetched but could not be put in its place on this machine.
pub const LOCAL_PROMOTE_CODE: &str = "download.local_promote_failed";

/// Recorded on a mirror that is given up while another member takes over.
pub const HANDOVER_CODE: &str = "download.mirror_handover";

/// Recorded on the mirror the group started with once no member is left to try.
pub const EXHAUSTED_CODE: &str = "download.mirrors_exhausted";

/// The member of `group` that should run, out of those that could.
///
/// A mirror that has already been tried and failed comes last, which is what keeps a fallback
/// from becoming a loop: without it the dispatcher would compare a burnt link and a fresh one
/// on position alone and could hand the turn straight back to the one that just failed. The
/// evidence is [`DownloadFile::last_error`], a persisted column, so a restart does not forget
/// which mirrors have been through.
///
/// After that an account beats no account, because a premium link is not subject to the
/// free-download limits the others would queue behind. Everything else is decided by the
/// order the links were added, so the choice is predictable.
#[must_use]
pub fn best_candidate<'a>(candidates: &'a [&'a DownloadFile]) -> Option<&'a DownloadFile> {
    candidates
        .iter()
        .min_by_key(|file| {
            (
                file.last_error.is_some(),
                file.account_id.is_none(),
                file.position,
                file.created_at,
            )
        })
        .copied()
}

/// The other members of `file`'s group, if it belongs to one.
#[must_use]
pub fn siblings<'a>(file: &DownloadFile, downloads: &'a [DownloadFile]) -> Vec<&'a DownloadFile> {
    let Some(group) = file.mirror_group.as_deref() else {
        return Vec::new();
    };
    downloads
        .iter()
        .filter(|other| {
            other.id != file.id
                && other.package_id == file.package_id
                && other.mirror_group.as_deref() == Some(group)
        })
        .collect()
}

/// The member the group started with: the one whose failure is the group's verdict.
///
/// The first link of the group in the package's own order. The enqueue puts the member the
/// LinkGrabber selected in that slot, so this is the mirror the person chose — and when all
/// of them are through, its reason is the one that counts, not whichever mirror happened to
/// be tried last.
#[must_use]
pub fn leader<'a>(file: &'a DownloadFile, downloads: &'a [DownloadFile]) -> &'a DownloadFile {
    siblings(file, downloads)
        .into_iter()
        .chain(std::iter::once(file))
        .min_by_key(|member| (member.position, member.created_at))
        .unwrap_or(file)
}

/// Whether this member leaves its group nothing to wait for.
///
/// A group is stalled when no member is in any of these states: every one of them has either
/// failed, been cancelled or is standing by as [`DownloadState::Skipped`], which means the
/// turn is held by nobody. Completed counts as holding it, because a finished group wants no
/// second copy.
#[must_use]
pub const fn holds_the_group_open(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Queued
            | DownloadState::RetryWait
            | DownloadState::Paused
            | DownloadState::Blocked
            | DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Repairing
            | DownloadState::Extracting
            | DownloadState::Seeding
            | DownloadState::Completed
    )
}

/// Whether the file has already taken the group's turn and is under way or done with it.
///
/// Distinct from merely being in the queue: two links added together are both queued, and
/// neither has taken anything yet.
#[must_use]
pub const fn has_taken_the_turn(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Repairing
            | DownloadState::Extracting
            | DownloadState::Seeding
            | DownloadState::Completed
    )
}

/// Whether the file is in the running for the group's turn without having taken it.
#[must_use]
pub const fn is_contending(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Queued | DownloadState::RetryWait | DownloadState::Paused
    )
}

#[cfg(test)]
mod tests {
    use rd_core::{Failure, FailureKind};

    use super::{groups_mirrors, hands_over, is_local};

    fn coded(category: FailureKind, code: &str) -> Failure {
        Failure::coded(category, code, "test")
    }

    #[test]
    fn only_transports_with_alternative_routes_are_grouped() {
        assert!(groups_mirrors(rd_core::DownloadKind::Http));
        assert!(groups_mirrors(rd_core::DownloadKind::Plugin));
        // A Usenet or torrent file is one member of a single download, not another way to it.
        assert!(!groups_mirrors(rd_core::DownloadKind::Usenet));
        assert!(!groups_mirrors(rd_core::DownloadKind::Torrent));
    }

    #[test]
    fn a_mirror_that_went_offline_hands_over() {
        // Even with a retry still scheduled: another hoster serves now, this one does not.
        assert!(hands_over(
            &Failure::new(FailureKind::Offline, "no route"),
            Some(chrono::Utc::now()),
        ));
    }

    #[test]
    fn a_page_instead_of_the_file_hands_over() {
        assert!(hands_over(
            &coded(FailureKind::Permanent, "download.not_a_file"),
            None,
        ));
    }

    #[test]
    fn a_hoster_limit_hands_over() {
        assert!(hands_over(
            &Failure::new(
                FailureKind::IpBlocked {
                    retry_after_seconds: None,
                },
                "wait 15 minutes",
            ),
            Some(chrono::Utc::now()),
        ));
        assert!(hands_over(
            &Failure::new(FailureKind::AccountInvalid, "refused"),
            Some(chrono::Utc::now()),
        ));
    }

    #[test]
    fn a_full_disk_does_not_hand_over() {
        let full = coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            rd_http::LOCAL_IO_CODE,
        );
        assert!(is_local(&full));
        assert!(
            !hands_over(&full, None),
            "every mirror writes to the same disk, so switching proves nothing"
        );
    }

    #[test]
    fn a_file_that_could_not_be_put_in_place_does_not_hand_over() {
        assert!(!hands_over(
            &coded(FailureKind::Permanent, super::LOCAL_PROMOTE_CODE),
            None,
        ));
    }

    #[test]
    fn a_transient_server_error_waits_for_its_retry_before_handing_over() {
        let blip = Failure::new(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            "502 bad gateway",
        );
        assert!(
            !hands_over(&blip, Some(chrono::Utc::now())),
            "one hiccup must not burn a mirror while a retry is scheduled"
        );
        assert!(
            hands_over(&blip, None),
            "once the retries are used up the link is done for and the turn moves"
        );
    }
}
