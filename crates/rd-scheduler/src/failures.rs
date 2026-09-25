//! Classification and persistence of download failures, including the guard that reports a
//! served web page instead of writing it to disk as the file.

use anyhow::Result;
use rd_core::{DownloadFile, Failure, FailureKind};
use rd_http::HttpDownloadError;

use crate::{SchedulerHandle, retry};

/// Maximum body bytes read from a non-file response to quote the hoster's own wording.
const NOT_A_FILE_PEEK_BYTES: usize = 64 * 1024;
/// Upper bound of the quoted excerpt kept in the failure message.
const NOT_A_FILE_MESSAGE_BYTES: usize = 300;

pub(crate) async fn record_error(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    failure: Failure,
) -> Result<()> {
    let retry_at = retry::retry_at(&failure, file.retry_count, scheduler.max_retries());
    // An IP limit is a property of the hoster, not of this one link: hold the whole hoster
    // back so its other free links do not burn a wait and a captcha to be refused too.
    if matches!(failure.category, FailureKind::IpBlocked { .. })
        && let Some(until) = retry_at
    {
        scheduler.block_host(&file.source, until);
    }
    if file.mirror_group.is_some() && crate::mirrors::hands_over(&failure, retry_at) {
        return hand_over(scheduler, file, failure, retry_at).await;
    }
    scheduler
        .database
        .record_failure(file.id, failure, retry_at)
        .await?;
    Ok(())
}

/// Moves the group's turn to the next mirror, or ends the group when there is none.
///
/// A link that does hand over is finished with: it is recorded without a retry, so it cannot
/// come back and take the turn a second time while another member has it. That, and the fact
/// that a promotion always consumes a member sitting in `Skipped`, is what keeps a fallback
/// from becoming a loop — every mirror of a group is tried at most once per attempt, and the
/// group runs out rather than going round. The member that finds nobody waiting is the
/// exception and keeps its retries, because it is no longer a group of anything.
async fn hand_over(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    failure: Failure,
    retry_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<()> {
    let downloads = scheduler.database.list_downloads().await?;
    let waiting: Vec<&DownloadFile> = crate::mirrors::siblings(file, &downloads)
        .into_iter()
        .filter(|sibling| sibling.state == rd_core::DownloadState::Skipped)
        .collect();
    let Some(next) = crate::mirrors::best_candidate(&waiting) else {
        // No mirror is waiting, so there is nothing to hand the turn to — and the last member
        // of a group is an ordinary download again. It keeps the retry it was owed: a link
        // with attempts left is not an exhausted group, and cutting them short here would
        // make a group of mirrors *less* persistent than a single link.
        if retry_at.is_some() {
            scheduler
                .database
                .record_failure(file.id, failure, retry_at)
                .await?;
            return Ok(());
        }
        // Out of members and out of attempts. The verdict of a group is the reason its
        // *chosen* member gave, not whichever one happened to be tried last: the first was
        // the one the person picked, and "the fourth hoster is offline too" says nothing
        // about the download they asked for. The row that just failed carries it, because
        // that is the one the queue is about to move to `Failed`.
        let leader = crate::mirrors::leader(file, &downloads);
        let members = crate::mirrors::siblings(file, &downloads).len() + 1;
        let verdict = exhausted(leader.last_error.as_ref().unwrap_or(&failure), members);
        scheduler
            .database
            .record_failure(file.id, verdict, None)
            .await?;
        return Ok(());
    };
    // Bytes from two hosters are not the same file even when both are the right one: a
    // different build, a different encode, a padded copy. The staging file of the mirror
    // being given up goes, so nothing the next one writes can be laid on top of it. The new
    // mirror stages under its own id and starts from zero in any case; this is about the
    // abandoned data, not about the new transfer.
    discard_partial_data(scheduler, file).await;
    let took_over = next
        .source
        .host_str()
        .unwrap_or("another mirror")
        .to_owned();
    scheduler
        .database
        .record_failure(file.id, handover(&failure, &took_over), None)
        .await?;
    // The member that held the turn has given it up and nothing holds it yet. The two writes
    // cannot share a transaction — they go through the serialized writer as separate commands
    // — so this is the window, and `recover_stalled_mirror_groups` is what closes it on the
    // next start. Without that a crash here leaves a group of live mirrors waiting forever
    // for a link that has already failed.
    rd_core::failpoint!("scheduler.before_mirror_promoted", || anyhow::anyhow!(
        "crash point: scheduler.before_mirror_promoted"
    ));
    scheduler
        .database
        .transition_download(next.id, rd_core::DownloadState::Queued)
        .await?;
    tracing::info!(
        failed = %file.file_name,
        took_over = %next.source,
        "a mirror took over after the active link gave up"
    );
    Ok(())
}

/// Restarts every mirror group that is waiting for a link which is never coming.
///
/// Run once at start-up, and it is the other half of `scheduler.before_mirror_promoted`: the
/// handover records the failure of the member that gave up and promotes the next one as two
/// separate writes, and a process that stops between them leaves a group in which nothing
/// holds the turn and every remaining mirror sits in [`DownloadState::Skipped`]. The
/// dispatcher cannot find it — it iterates over queued rows and a skipped one is not queued —
/// so it would wait for as long as the install lives.
///
/// Deliberately not restricted to that window. Any way a group ends up with members standing
/// by and nobody running is the same defect with the same fix, and a start-up sweep costs one
/// query.
pub(crate) async fn recover_stalled_mirror_groups(scheduler: &SchedulerHandle) -> Result<()> {
    let downloads = scheduler.database.list_downloads().await?;
    let mut groups: std::collections::BTreeMap<(rd_core::PackageId, &str), Vec<&DownloadFile>> =
        std::collections::BTreeMap::new();
    for file in &downloads {
        if let Some(group) = file.mirror_group.as_deref() {
            groups
                .entry((file.package_id, group))
                .or_default()
                .push(file);
        }
    }
    for ((package_id, group), members) in groups {
        if members
            .iter()
            .any(|member| crate::mirrors::holds_the_group_open(member.state))
        {
            continue;
        }
        let waiting: Vec<&DownloadFile> = members
            .iter()
            .copied()
            .filter(|member| member.state == rd_core::DownloadState::Skipped)
            .collect();
        let Some(next) = crate::mirrors::best_candidate(&waiting) else {
            continue;
        };
        scheduler
            .database
            .transition_download(next.id, rd_core::DownloadState::Queued)
            .await?;
        tracing::info!(
            %package_id,
            group,
            took_over = %next.source,
            "a mirror group was left with nobody running and was restarted"
        );
    }
    Ok(())
}

/// Promotes one waiting mirror after the member holding the turn stepped aside.
///
/// The cancel and remove paths, which end a link without recording a failure: there is
/// nothing to classify and nothing to hand over, only a turn that has to move on. Only one
/// mirror is promoted — the group still downloads a single copy, it has just changed which
/// link that is.
pub(crate) async fn wake_mirror(scheduler: &SchedulerHandle, file: &DownloadFile) -> Result<()> {
    if file.mirror_group.is_none() {
        return Ok(());
    }
    let downloads = scheduler.database.list_downloads().await?;
    let waiting: Vec<&DownloadFile> = crate::mirrors::siblings(file, &downloads)
        .into_iter()
        .filter(|sibling| sibling.state == rd_core::DownloadState::Skipped)
        .collect();
    let Some(next) = crate::mirrors::best_candidate(&waiting) else {
        return Ok(());
    };
    scheduler
        .database
        .transition_download(next.id, rd_core::DownloadState::Queued)
        .await?;
    tracing::info!(
        stepped_aside = %file.file_name,
        took_over = %next.source,
        "a mirror took over after the active link stepped aside"
    );
    Ok(())
}

/// Removes the staging file of a mirror that is being given up.
///
/// Logged rather than propagated: the handover has to happen even when the staging directory
/// is already gone, and a leftover `.part` is worth a warning, not a stuck group.
async fn discard_partial_data(scheduler: &SchedulerHandle, file: &DownloadFile) {
    let destination = match scheduler.database.list_packages().await {
        Ok(packages) => packages
            .into_iter()
            .find(|package| package.id == file.package_id)
            .map(|package| package.destination),
        Err(error) => {
            tracing::warn!(download_id = %file.id, %error, "the given-up mirror kept its staging file");
            return;
        }
    };
    let Some(destination) = destination.filter(|path| !path.is_empty()) else {
        return;
    };
    if let Err(error) = crate::control::remove_part_file(&destination, file.id).await {
        tracing::warn!(download_id = %file.id, %error, "the given-up mirror kept its staging file");
    }
}

/// The failure a mirror carries once it has been given up and another one has taken over.
///
/// The original reason is kept verbatim in `reason`, and its own stable code in
/// `reason_code`, so nothing a translator or a later reader needs is lost — the wrapping only
/// adds who took over.
fn handover(original: &Failure, took_over: &str) -> Failure {
    let failure = Failure::coded(
        original.category.clone(),
        crate::mirrors::HANDOVER_CODE,
        format!(
            "this mirror was given up and {took_over} took over: {}",
            original.message
        ),
    )
    .with_param("mirror", took_over)
    .with_param("reason", original.message.clone());
    match original.code.clone() {
        Some(code) => failure.with_param("reason_code", code),
        None => failure,
    }
}

/// The verdict of a group that has run out of mirrors.
fn exhausted(leader: &Failure, members: usize) -> Failure {
    // The leader's row is itself a handover once it has been given up, so its own reason is
    // read back out of the parameters rather than out of the wrapper's message.
    let reason = leader
        .params
        .get("reason")
        .cloned()
        .unwrap_or_else(|| leader.message.clone());
    let code = leader
        .params
        .get("reason_code")
        .cloned()
        .or_else(|| leader.code.clone());
    let failure = Failure::coded(
        leader.category.clone(),
        crate::mirrors::EXHAUSTED_CODE,
        format!("all {members} mirrors of this file failed; the chosen one reported: {reason}"),
    )
    .with_param("mirrors", members.to_string())
    .with_param("reason", reason);
    match code {
        Some(code) => failure.with_param("reason_code", code),
        None => failure,
    }
}

pub(crate) async fn record_http_error(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    error: HttpDownloadError,
) -> Result<()> {
    record_http_error_with_replay(scheduler, file, error, false).await
}

pub(crate) async fn record_http_error_with_replay(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    error: HttpDownloadError,
    is_post_replay: bool,
) -> Result<()> {
    let failure = from_http_error_with_replay(error, is_post_replay);
    // A captured browser link has no account, which used to exclude it from the one
    // reactive refresh after a 401/403 -- exactly the case this feature exists for.
    let has_template = scheduler
        .database
        .request_template(file.id)
        .await
        .ok()
        .flatten()
        .is_some();
    let needs_resolver_refresh = matches!(
        failure.category,
        FailureKind::AuthRequired | FailureKind::AccountInvalid
    ) && (file.account_id.is_some() || has_template);
    if needs_resolver_refresh && scheduler.database.claim_resolver_refresh(file.id).await? {
        scheduler.clients.clear().await;
        scheduler
            .database
            .record_failure(file.id, failure, Some(chrono::Utc::now()))
            .await?;
        return Ok(());
    }
    record_error(scheduler, file, failure).await
}

/// Turns "the server served a web page" into an actionable failure, quoting the page when
/// it carries the hoster's explanation (a wait notice, a limit, an expired link).
pub(crate) async fn not_a_file(
    client: &reqwest::Client,
    source: &url::Url,
    headers: &[(String, String)],
    probe_result: &rd_http::ProbeResult,
) -> Failure {
    let content_type = probe_result
        .content_type
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    // A hoster's "link expired" page routinely echoes the signed URL back at us, so the
    // excerpt is redacted before it becomes part of a persisted, user-visible message.
    let excerpt = rd_http::peek_body_text(client, source.clone(), headers, NOT_A_FILE_PEEK_BYTES)
        .await
        .map(|text| {
            truncate_on_char_boundary(rd_core::redact_text(&text), NOT_A_FILE_MESSAGE_BYTES)
        });
    let message = match &excerpt {
        Some(text) => format!("The server returned {content_type} instead of a file: {text}"),
        None => format!("The server returned {content_type} instead of a file"),
    };
    let failure = Failure::coded(FailureKind::Permanent, "download.not_a_file", message)
        .with_param("content_type", content_type);
    match excerpt {
        Some(text) => failure.with_param("excerpt", text),
        None => failure,
    }
}

/// The hoster announced one size and the server offered another: the transfer would otherwise
/// have run to completion over the wrong bytes and checksummed them (RD-109-36).
pub(crate) fn size_mismatch(announced: u64, offered: u64) -> Failure {
    Failure::coded(
        FailureKind::Permanent,
        "download.size_mismatch",
        format!("The server offered {offered} bytes for a file announced as {announced} bytes"),
    )
    .with_param("announced_bytes", announced.to_string())
    .with_param("offered_bytes", offered.to_string())
}

fn truncate_on_char_boundary(mut text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let cut = (0..=limit)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or_default();
    text.truncate(cut);
    text
}

fn transient(error: anyhow::Error) -> Failure {
    Failure::new(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        error.to_string(),
    )
}

/// Classifies a transfer error, distinguishing a POST that cannot be resumed.
///
/// RFC 9110 defines no range semantics for POST, and most endpoints that answer one with a
/// file ignore `Range` outright. Re-posting from zero would risk triggering the server-side
/// side effect a second time (a second preparation job, a decremented quota, a second
/// charge), so the download is blocked with a legible reason and the partial file is kept.
/// A person can then discard it deliberately with the queue row's reset action.
fn from_http_error_with_replay(error: HttpDownloadError, is_post_replay: bool) -> Failure {
    match error {
        HttpDownloadError::RangeIgnored if is_post_replay => Failure::coded(
            FailureKind::AuthRequired,
            "download.post_resume_unsupported",
            "This download was started with a POST and the server does not support resuming it",
        ),
        // Retryable, and coded so it reads as a sentence in the user's language. A hoster
        // that answers a ranged request with something else is usually saying it is busy:
        // pressing start again worked, so waiting and trying again works too. The planner
        // drops to a single connection for that host in the meantime.
        HttpDownloadError::RangeIgnored => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            "download.range_ignored",
            "The server did not answer the requested part of the file",
        ),
        HttpDownloadError::RemoteChanged => Failure::coded(
            FailureKind::Permanent,
            "download.remote_changed",
            "The file on the server changed while it was being downloaded",
        ),
        HttpDownloadError::Failure(failure) => failure,
        // Coded, and that is the whole point: the class alone cannot tell a full disk from a
        // flaky server, and without the code a mirror group would give up on the hoster and
        // try the next one — which writes to the same disk (RD-110-20).
        HttpDownloadError::Local(error) => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            rd_http::LOCAL_IO_CODE,
            error.to_string(),
        ),
        HttpDownloadError::Internal(error) => transient(error),
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_on_char_boundary;

    #[test]
    fn an_excerpt_is_cut_without_splitting_a_character() {
        let text = "quota café exceeded";
        // Byte 10 lands inside the two-byte "é", so the cut steps back to byte 9.
        assert_eq!(truncate_on_char_boundary(text.to_owned(), 10), "quota caf");
        // Byte 11 is the boundary right after "é", so it is kept.
        assert_eq!(truncate_on_char_boundary(text.to_owned(), 11), "quota café");
        // A short excerpt is returned unchanged.
        assert_eq!(truncate_on_char_boundary("short".to_owned(), 300), "short");
    }
}
