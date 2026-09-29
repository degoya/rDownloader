//! The session inventory: what is signed in, and how to sign it out.
//!
//! A person who suspects something has their password needs two things, in this order: to see
//! what is currently signed in, and to end all of it but the browser they are sitting at. The
//! second is the reason the first exists, and it is why "sign out everywhere else" keeps the
//! caller's own session — an action that also signs you out leaves you unable to check whether
//! it worked.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};

use crate::{ApiError, AppState, dto::MessageResponse};

/// Marks the caller's own row so the interface can label it and refuse to end it by accident.
fn mark_current(sessions: &mut [rd_core::Session], current: Option<rd_core::SessionId>) {
    for session in sessions.iter_mut() {
        session.current = current.is_some_and(|id| id == session.id);
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/sessions",
    tag = "security",
    responses((status = 200, body = Vec<rd_core::Session>))
)]
pub async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<rd_core::Session>>, ApiError> {
    let current = state
        .auth
        .current_session(&state, &headers)
        .await
        .map(|session| session.id);
    let mut sessions = state
        .database
        .list_sessions(state.auth.session_limits())
        .await?;
    mark_current(&mut sessions, current);
    Ok(Json(sessions))
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions/{id}",
    tag = "security",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, description = "No live session with that id", body = crate::error::ErrorBody),
    )
)]
pub async fn revoke_session(
    State(state): State<AppState>,
    Path(id): Path<rd_core::SessionId>,
) -> Result<Json<MessageResponse>, ApiError> {
    // Ending your own session from this endpoint is allowed: it is a sign-out, and refusing it
    // would be a rule the caller has to discover rather than a protection.
    if !state.database.revoke_session(id).await? {
        return Err(ApiError::not_found(
            "session.not_found",
            "No live session with that id",
        ));
    }
    Ok(Json(MessageResponse::new(
        "session.revoked",
        "Session ended",
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions/revoke-others",
    tag = "security",
    responses((status = 200, body = MessageResponse))
)]
pub async fn revoke_other_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MessageResponse>, ApiError> {
    // Without a session of its own — a machine token calling this — every session ends, which
    // is the correct reading of "everywhere else" for a caller that has none.
    let keep = crate::auth::session_digest(&headers).unwrap_or_default();
    let ended = state.database.revoke_other_sessions(keep).await?;
    Ok(Json(
        MessageResponse::new("session.others_revoked", "Other sessions ended")
            .with_param("count", ended.to_string()),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "security",
    responses((status = 200, body = MessageResponse))
)]
pub async fn logout(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    headers: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;

    // Read before the session is closed; afterwards there is nothing left to name.
    let session = state
        .auth
        .current_session(&state, &headers)
        .await
        .map(|session| session.id);
    state.auth.logout(&state, &headers).await?;
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::Logout).by(&audit);
    if let Some(id) = session {
        event = event.target("session", id);
    }
    crate::audit::record(&state, event).await;
    let mut response = Json(MessageResponse::new("auth.logged_out", "Signed out")).into_response();
    response.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        axum::http::HeaderValue::from_static(crate::AuthService::EXPIRED_COOKIE),
    );
    Ok(response)
}
