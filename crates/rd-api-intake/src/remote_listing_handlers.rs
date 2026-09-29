//! Reviewing the directory listing of an ftp/sftp/webdav link before it is queued.
//!
//! Mirrors the torrent plan endpoints: the listing is stored on the candidate, the full
//! tree is fetched per candidate rather than inlined into every list response, and the
//! selection is stored as exclusions.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::{CandidateId, ResolvedRemoteListing};

use crate::{ApiError, AppState, dto::RemoteListingPlanRequest};

/// Directory listing of one link candidate, with the current selection applied.
#[utoipa::path(
    get,
    path = "/api/v1/collector/candidates/{id}/listing",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    responses(
        (status = 200, body = rd_core::ResolvedRemoteListing),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn get_candidate_listing(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<ResolvedRemoteListing>, ApiError> {
    let stored = state
        .database
        .candidate_listing(id)
        .await?
        .ok_or_else(listing_missing)?;
    if stored.is_future_contract() {
        return Err(ApiError::conflict(
            "remote.listing_future_contract",
            "This listing was written by a newer version of rDownloader",
        ));
    }
    Ok(Json(stored.resolve()))
}

/// Replaces the file selection of one link candidate.
#[utoipa::path(
    put,
    path = "/api/v1/collector/candidates/{id}/listing/plan",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    request_body = RemoteListingPlanRequest,
    responses(
        (status = 200, body = rd_core::ResolvedRemoteListing),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn put_candidate_listing_plan(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<RemoteListingPlanRequest>,
) -> Result<Json<ResolvedRemoteListing>, ApiError> {
    let stored = state
        .database
        .candidate_listing(id)
        .await?
        .ok_or_else(listing_missing)?;
    let excluded = normalize_exclusions(request.excluded, &stored.listing)?;
    let resolved = state
        .database
        .set_candidate_listing_plan(id, rd_core::RemoteListingPlan { excluded })
        .await
        .map_err(|_| listing_missing())?;
    Ok(Json(resolved))
}

/// Cleans an incoming selection and refuses paths the listing does not contain.
///
/// Accepting arbitrary strings would let a client store exclusions that match nothing (or,
/// after a re-listing, match something unintended), so the plan is kept anchored to what
/// was actually reviewed.
fn normalize_exclusions(
    excluded: Vec<String>,
    listing: &rd_core::RemoteListing,
) -> Result<Vec<String>, ApiError> {
    if excluded.len() > rd_core::MAX_REMOTE_ENTRIES {
        return Err(ApiError::bad_request(
            "remote.selection_too_large",
            "The selection names more entries than the listing holds",
        ));
    }
    let mut cleaned: Vec<String> = Vec::with_capacity(excluded.len());
    for path in excluded {
        let path = path.trim().trim_end_matches('/').to_owned();
        if path.is_empty() {
            continue;
        }
        if !listing.entries.iter().any(|entry| entry.path == path) {
            return Err(ApiError::bad_request(
                "remote.selection_unknown_path",
                "The selection names a file the listing does not contain",
            ));
        }
        if !cleaned.contains(&path) {
            cleaned.push(path);
        }
    }
    Ok(cleaned)
}

fn listing_missing() -> ApiError {
    ApiError::not_found(
        "remote.listing_missing",
        "This link has no directory listing to review",
    )
}

#[cfg(test)]
mod tests {
    use rd_core::{RemoteEntry, RemoteListing};

    use super::normalize_exclusions;

    fn listing() -> RemoteListing {
        RemoteListing {
            root: "/pub".to_owned(),
            single_file: false,
            entries: ["extras", "extras/notes.txt", "movie.mkv"]
                .into_iter()
                .map(|path| RemoteEntry {
                    path: path.to_owned(),
                    is_dir: !path.contains('.'),
                    size: None,
                    modified: None,
                    etag: None,
                })
                .collect(),
            truncated: None,
            supports_resume: true,
        }
    }

    #[test]
    fn a_selection_is_trimmed_and_deduplicated() {
        let cleaned = normalize_exclusions(
            vec![
                " movie.mkv ".to_owned(),
                "extras/".to_owned(),
                "movie.mkv".to_owned(),
                String::new(),
            ],
            &listing(),
        )
        .expect("cleaned");
        assert_eq!(cleaned, ["movie.mkv", "extras"]);
    }

    #[test]
    fn a_path_outside_the_listing_is_refused() {
        // The bug this guards: a stored exclusion that matches nothing today could match
        // something unintended after the directory is listed again.
        for hostile in ["../escape", "/etc/passwd", "unknown.bin"] {
            assert!(
                normalize_exclusions(vec![hostile.to_owned()], &listing()).is_err(),
                "{hostile} should have been refused"
            );
        }
    }

    #[test]
    fn an_empty_selection_includes_everything() {
        assert!(
            normalize_exclusions(Vec::new(), &listing())
                .expect("cleaned")
                .is_empty()
        );
    }
}
