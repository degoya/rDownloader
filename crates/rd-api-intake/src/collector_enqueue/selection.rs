//! What a link is queued with beyond its address: its media selection and its mirror group.

use super::*;

/// The selection a recorded live manifest is queued with (RD-080-06).
///
/// The stream runner reads `format` as a *streamlink quality name*, not as an extractor
/// expression — the same contract the channel monitor uses. A direct manifest has no
/// qualities to choose from before it is opened, so `best` stands in and streamlink picks.
/// Refuses a media link that carries no variant selection (RD-120-50).
///
/// Such a row can only fail: the runner's first step is `media.selection_missing`. Queued
/// anyway it looked like an ordinary download that broke later, while the reason — the page
/// offered nothing this installation can download, or the check never got that far — was
/// visible only here. Checked before anything is written, so a refused package leaves no
/// half-imported NZB behind it; the claim is released by the caller as for any refusal.
pub(super) fn ensure_media_selections(candidates: &[LinkCandidate]) -> Result<(), ApiError> {
    let unselected = candidates.iter().find(|candidate| {
        candidate.provider.as_deref() == Some(rd_core::MEDIA_PROVIDER)
            && candidate
                .media
                .as_ref()
                .and_then(rd_core::MediaInfo::selection)
                .is_none()
    });
    match unselected {
        Some(candidate) => Err(ApiError::unprocessable(
            crate::error_codes::MEDIA_SELECTION_MISSING,
            "This media link has no format selection that can be downloaded",
        )
        .with_param("candidate_id", candidate.id)
        .with_param("url", rd_core::redact_url(&candidate.url))),
        None => Ok(()),
    }
}

pub(super) fn record_selection(
    candidate: &rd_core::LinkCandidate,
) -> Option<rd_core::MediaSelection> {
    if candidate.provider.as_deref() != Some(rd_core::RECORD_PROVIDER) {
        return None;
    }
    Some(rd_core::MediaSelection {
        page_url: candidate.url.clone(),
        variant_id: "best".to_owned(),
        format: "best".to_owned(),
        kind: rd_core::MediaKind::Video,
        ext: "ts".to_owned(),
        title: candidate.file_name.clone().unwrap_or_default(),
        contract_version: rd_core::MEDIA_CONTRACT_VERSION,
        criteria: None,
        resolved: None,
    })
}

/// Carries the LinkGrabber's mirror groups into the queue (RD-110-20).
///
/// The queue used to work a group out for itself at this point (RD-094-05) from the declared
/// file names and sizes. That was a second answer to a question the LinkGrabber had already
/// answered better: its group (RD-110-18) knows what a site rule declared, was recomputed
/// once the online check brought the real names and sizes, and holds the member a person
/// picked. Two notions of the same thing can disagree, and the weaker one won here because it
/// ran last. So nothing is computed any more — the answer is carried.
///
/// Two things are still decided here, because they are the queue's and not the LinkGrabber's:
/// a transport that has no alternative routes is never grouped, and the selected member is
/// moved into the group's first slot so the queue can read the group's verdict off it when
/// every mirror has failed.
pub(super) fn group_mirrors(
    files: &mut [rd_scheduler::FileSpec],
    mirrors: &[Option<rd_core::CandidateMirror>],
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let mut groups: std::collections::BTreeMap<&str, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (index, entry) in mirrors.iter().enumerate() {
        let Some(entry) = entry else { continue };
        // A Usenet or torrent row is one member of a single download, not another way to it.
        if !rd_scheduler::mirrors::groups_mirrors(files[index].kind) {
            continue;
        }
        groups.entry(entry.group.as_str()).or_default().push(index);
    }
    for (key, members) in groups {
        // A link whose mirrors were all filtered out on the way here is a download again.
        if members.len() < 2 {
            continue;
        }
        let selected = members
            .iter()
            .copied()
            .find(|index| mirrors[*index].as_ref().is_some_and(|entry| entry.selected))
            .unwrap_or(members[0]);
        for index in members.iter().copied() {
            files[index].mirror_group = Some(key.to_owned());
            files[index].skipped = index != selected;
        }
        // The queue identifies the member a group started with by position, so the chosen one
        // has to hold the group's first slot. Swapping stays inside the group's own indices,
        // which leaves every other group's positions exactly where they were.
        files.swap(selected, members[0]);
    }
}
