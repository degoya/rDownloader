//! `torrents/filePrio` and `torrents/setShareLimits`: file priorities and share limits.

use super::*;

/// `POST /api/v2/torrents/filePrio`: include or exclude files of a queued torrent.
///
/// qBittorrent expresses "do not download" as priority `0` and everything else as a tier.
/// Only the inclusion half is honoured, because that is the decision our review model
/// records; the request goes through the same validation and engine call as the native
/// plan endpoint, so a selection that would leave nothing to download is refused here too.
pub(crate) async fn file_priority(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
    body: String,
) -> Response {
    let form = form(&body);
    let hash = query
        .hash
        .clone()
        .or_else(|| form.get("hash").cloned())
        .map(|value| map::normalize_hash(&value))
        .unwrap_or_default();
    let views = match views(&state).await {
        Ok(views) => views,
        Err(unavailable) => return *unavailable,
    };
    let Some(view) = views.iter().find(|view| view.hash == hash) else {
        return not_found();
    };
    let Some(indices) = form.get("id") else {
        return bad_request();
    };
    let indices: Vec<u32> = indices
        .split('|')
        .filter_map(|value| value.trim().parse().ok())
        .collect();
    let exclude = form
        .get("priority")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .is_some_and(|priority| priority == 0);
    let (included, excluded) = if exclude {
        (Vec::new(), indices)
    } else {
        (indices, Vec::new())
    };
    let request = crate::torrent_control::TorrentPlanRequest::selection(included, excluded);
    match crate::torrent_control::apply_download_plan(&state, view.download.id, request).await {
        Ok(_) => ok(),
        Err(_) => bad_request(),
    }
}

pub(super) fn bad_request() -> Response {
    (axum::http::StatusCode::BAD_REQUEST, "Priority is not valid").into_response()
}

/// `POST /api/v2/torrents/setShareLimits`: ratio and seeding-time targets.
///
/// Mapped onto the per-torrent seeding override rather than acknowledged and forgotten: a
/// client that sets a seed goal expects the torrent to actually stop. qBittorrent's
/// sentinels are kept — `-1` means "use the global setting", `-2` means "no limit" — and a
/// value outside our accepted range is refused rather than clamped, because silently
/// seeding to a different target than the one asked for is worse than a visible failure.
pub(crate) async fn set_share_limits(
    State(state): State<AppState>,
    Query(query): Query<TorrentQuery>,
    body: String,
) -> Response {
    let form = form(&body);
    let views = match views(&state).await {
        Ok(views) => views,
        Err(unavailable) => return *unavailable,
    };
    let hashes = requested(&query, &form, &views);
    let ratio = form
        .get("ratioLimit")
        .and_then(|value| value.trim().parse::<f64>().ok());
    let minutes = form
        .get("seedingTimeLimit")
        .and_then(|value| value.trim().parse::<i64>().ok());
    let request = crate::torrent_control::SeedingPolicyRequest {
        enabled: Some(true),
        // -1 asks for the global default, which is what "no override" already means here;
        // -2 is "no ratio limit", whose stored form is a ratio of zero, the value that
        // switches the ratio stop off. Anything else goes through as it is and is refused
        // below if it lies outside the accepted range.
        ratio: ratio.and_then(|value| {
            if value == -1.0 {
                None
            } else if value == -2.0 {
                Some(0.0)
            } else {
                Some(value)
            }
        }),
        time_minutes: minutes
            .filter(|value| *value > 0)
            .and_then(|value| u32::try_from(value).ok()),
        time_unlimited: (minutes == Some(-2)).then_some(true),
    };
    for view in views.iter().filter(|view| hashes.contains(&view.hash)) {
        let request = crate::torrent_control::SeedingPolicyRequest {
            enabled: request.enabled,
            ratio: request.ratio,
            time_minutes: request.time_minutes,
            time_unlimited: request.time_unlimited,
        };
        if crate::torrent_control::apply_download_seeding(&state, view.download.id, request)
            .await
            .is_err()
        {
            return bad_request();
        }
    }
    ok()
}
