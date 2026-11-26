//! `GET <collection>/{id}` of the credentials (RD-1480-10, API-6): one row as its list shows it.
//!
//! Every handler keeps one row of its list, so the two cannot drift apart in shape or in what
//! they reveal (`rd_api_core::single_read`). The route costs what the list costs.

use axum::extract::State;
use rd_api_core::extract::{Json, Path};
use rd_api_core::single_read::one_of;

use crate::{ApiError, AppState, error_codes::parse_id};

/// One API token: its name, scopes and limits, never its secret.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/api-tokens/{id}", tag = "api-tokens", params(("id" = rd_core::CaptureTokenId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::CaptureToken), (status = 404)))]
pub async fn get_api_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::CaptureToken>, ApiError> {
    let id = parse_id::<rd_core::CaptureTokenId>(&id)?;
    let Json(rows) = crate::api_tokens::list_api_tokens(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("api.token_not_found", "API token not found"),
    )
}

/// One stored browser profile, without its cookies and passwords.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/auth-profiles/{id}", tag = "configuration", params(("id" = rd_core::AuthProfileId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::AuthProfile), (status = 404)))]
pub async fn get_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::AuthProfile>, ApiError> {
    let id = parse_id::<rd_core::AuthProfileId>(&id)?;
    let Json(rows) = crate::auth_profile_handlers::list_auth_profiles(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("authprofile.not_found", "Auth profile not found"),
    )
}
