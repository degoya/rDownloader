//! "Add again" from the download history (RD-1100-04): an entry's sources go back into the
//! LinkGrabber through the ordinary intake, so the blocklist, the routing rules, the online
//! check and the review apply exactly as they do to a pasted link.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use crate::{
    ApiError, AppState,
    collector_handlers::collector_intake_inner,
    dto::{CaptureLinkRequest, CollectorIntakeRequest, CollectorIntakeResponse},
};

/// Puts a history entry's sources back into the LinkGrabber as one package under its name.
///
/// The sources are the masked ones the history kept: a link whose token was masked arrives
/// without it and is checked like any other. An entry with no source of its own — an imported
/// NZB — is refused with `history.nothing_to_readd`.
#[utoipa::path(
    post,
    path = "/api/v1/history/{id}/readd",
    tag = "history",
    params(("id" = i64, Path, description = "The history entry")),
    responses(
        (status = 201, body = CollectorIntakeResponse),
        (status = 404, description = "history.not_found"),
        (status = 409, description = "history.nothing_to_readd"),
    )
)]
pub async fn readd_history_entry(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    let entry = state
        .database
        .get_history_entry(id)
        .await?
        .ok_or_else(|| ApiError::not_found("history.not_found", "No such history entry"))?;
    if entry.sources.is_empty() {
        return Err(ApiError::conflict(
            "history.nothing_to_readd",
            "This entry has no source address that could be added again",
        ));
    }
    let links = entry
        .sources
        .into_iter()
        .take(rd_core::MAX_CAPTURE_LINKS)
        .map(|url| CaptureLinkRequest {
            url,
            file_name: None,
            request: None,
        })
        .collect();
    let response = collector_intake_inner(
        &state,
        CollectorIntakeRequest {
            text: None,
            source: rd_core::IngressSource::Manual,
            source_label: Some("history".to_owned()),
            package_name: Some(entry.name),
            password: None,
            links,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(response)))
}
