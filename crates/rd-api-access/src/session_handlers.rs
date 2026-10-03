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

/// What a sign-out answers.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct LogoutResponse {
    pub message: String,
    pub code: String,
    /// Kept from the answer this replaced (`MessageResponse`), so a client of 1.8 reads the same
    /// shape; a sign-out has no parameters, so it is always empty and never serialised.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: rd_api_core::error::MessageParams,
    /// Where to send the browser to sign out at the identity provider as well: present only when
    /// *sign out at the provider too* is on (D5, RD-190-15). The local session is already ended
    /// when this is handed out, so not following it leaves nothing signed in here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_logout_url: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "security",
    responses((status = 200, body = LogoutResponse))
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
    // Only for a caller that had a session: an anonymous request to this public route learns
    // nothing about the provider from it.
    let provider_logout_url = match session {
        Some(_) => crate::oidc_handlers::provider_logout_url(&state).await,
        None => None,
    };
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::Logout).by(&audit);
    // The route is public, so no middleware named the actor: the session that ended is it.
    if let Some(id) = session {
        event = event
            .actor(crate::audit::Actor::session(id.to_string()))
            .target("session", id);
    }
    if provider_logout_url.is_some() {
        event = event.detail("provider", true);
    }
    crate::audit::record(&state, event).await;
    let expired = crate::AuthService::expired_cookie(state.proxy.read().await.base_path());
    let mut response = Json(LogoutResponse {
        message: "Signed out".to_owned(),
        code: "auth.logged_out".to_owned(),
        params: rd_api_core::error::MessageParams::default(),
        provider_logout_url,
    })
    .into_response();
    response.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        // A base path that is no header value could not have carried the sign-in cookie
        // either; the root is then the only place a cookie can be.
        axum::http::HeaderValue::from_str(&expired).unwrap_or_else(|_| {
            axum::http::HeaderValue::from_static(
                "rd_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
            )
        }),
    );
    Ok(response)
}
