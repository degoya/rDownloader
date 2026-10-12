//! LinkGrabber intake, packages, ordering and enqueueing.

use axum::{Json, extract::State, http::StatusCode};

use crate::{
    ApiError, AppState,
    dto::{CollectorIntakeRequest, CollectorIntakeResponse, MessageResponse},
};

mod candidates;
mod crawl;
mod intake;
pub(crate) mod links;
mod mirrors;
mod packages;

pub use candidates::*;
pub(crate) use intake::collector_intake_crawled;
pub use intake::collector_intake_inner;
pub use mirrors::*;
pub use packages::*;

#[utoipa::path(post, path = "/api/v1/collector/batches", tag = "collector", request_body = CollectorIntakeRequest, responses((status = 201, body = CollectorIntakeResponse)))]
pub async fn collector_intake(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(mut request): Json<CollectorIntakeRequest>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    request.source = crate::collector_source_sets::of_caller(&audit, request.source);
    let response = collector_intake_inner(&state, request).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(post, path = "/api/v1/capture/batches", tag = "capture", request_body = CollectorIntakeRequest, responses((status = 201, body = CollectorIntakeResponse), (status = 400, description = "Invalid links or request metadata"), (status = 401, description = "Capture token missing or revoked")))]
pub async fn capture_intake(
    state: State<AppState>,
    audit: crate::audit::AuditContext,
    Json(mut request): Json<CollectorIntakeRequest>,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    request.source = crate::collector_source_sets::captured(request.source);
    crate::capture_notice::intake(state, audit, request).await
}

fn message(code: &str, text: &str) -> Json<MessageResponse> {
    Json(MessageResponse::new(code, text))
}
