//! REST halves of the browser-session handover (RD-120-45); the model is in
//! [`crate::browser_session`].
//!
//! The web half runs under the secrets scope, like typing cookies into the account does. The
//! capture half runs under the capture token the extension already holds, and it can do three
//! things only: list the requests a person opened, answer one with cookies of its scope, or
//! decline one. None of it is reachable over MCP (`mcp::coverage`): it carries a credential.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use url::Url;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    browser_session::{BrowserSessionResponse, CaptureBrowserSession, Handover, Refusal, Request},
    config_handlers::{cleanup_secrets, validate_secret_value},
    dto::MessageResponse,
    error_codes::account_not_found,
};

/// Largest cookie set accepted, the same bound a cookie session typed into an account has.
const MAX_COOKIE_BYTES: usize = 4 * 1024 * 1024;

/// The cookies the extension read for a waiting request, in Netscape format.
///
/// Deliberately without `Debug`: this carries a live session, and the one way it could reach
/// a log is a struct that knows how to print itself.
#[derive(Deserialize, ToSchema)]
pub struct DeliverBrowserSessionRequest {
    /// One Netscape row per cookie. Every row's domain must lie within the request's scope.
    #[schema(write_only)]
    pub cookies: String,
}

/// Asks the browser extension for this account's session at its provider.
#[utoipa::path(post, path = "/api/v1/accounts/{id}/browser-session", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 201, body = BrowserSessionResponse), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)))]
pub async fn begin_browser_session(
    State(state): State<AppState>,
    Path(id): Path<rd_core::AccountId>,
) -> Result<(StatusCode, Json<BrowserSessionResponse>), ApiError> {
    let account = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .find(|account| account.id == id)
        .ok_or_else(account_not_found)?;
    // The scope is the installed plugin's, and only an https one: the manifest check already
    // insists on it, and a cookie read for a plain-http origin is not one this path makes.
    let spec = rd_provider_registry::by_slug(&account.provider);
    let scope = rd_provider_registry::cookie_scope(&account.provider)
        .filter(|scope| scope.scheme() == "https" && scope.host_str().is_some())
        .ok_or_else(|| {
            ApiError::bad_request(
                "browser_session.no_cookie_scope",
                "This provider takes no session from a browser",
            )
        })?;
    let response = state.browser_sessions.begin(
        Request {
            account_id: account.id,
            account_label: account.label,
            provider_name: spec
                .map(|spec| spec.display_name)
                .unwrap_or_else(|| account.provider.clone()),
            provider: account.provider,
            scope,
        },
        chrono::Utc::now(),
    );
    Ok((StatusCode::CREATED, Json(response)))
}

/// Where this account's latest handover stands.
#[utoipa::path(get, path = "/api/v1/accounts/{id}/browser-session", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = BrowserSessionResponse), (status = 404, body = crate::error::ErrorBody)))]
pub async fn get_browser_session(
    State(state): State<AppState>,
    Path(id): Path<rd_core::AccountId>,
) -> Result<Json<BrowserSessionResponse>, ApiError> {
    state
        .browser_sessions
        .status(id, chrono::Utc::now())
        .map(Json)
        .ok_or_else(no_request)
}

/// Withdraws this account's handover request.
#[utoipa::path(delete, path = "/api/v1/accounts/{id}/browser-session", tag = "configuration", params(("id" = rd_core::AccountId, Path)), responses((status = 200, body = MessageResponse), (status = 404, body = crate::error::ErrorBody)))]
pub async fn cancel_browser_session(
    State(state): State<AppState>,
    Path(id): Path<rd_core::AccountId>,
) -> Result<Json<MessageResponse>, ApiError> {
    if !state.browser_sessions.cancel(id) {
        return Err(no_request());
    }
    Ok(Json(MessageResponse::new(
        "browser_session.cancelled",
        "The request for the browser session was withdrawn",
    )))
}

/// The handovers a person opened and the extension may answer.
#[utoipa::path(get, path = "/api/v1/capture/browser-sessions", tag = "capture", responses((status = 200, body = [CaptureBrowserSession]), (status = 401)))]
pub async fn list_capture_browser_sessions(
    State(state): State<AppState>,
) -> Json<Vec<CaptureBrowserSession>> {
    Json(state.browser_sessions.waiting(chrono::Utc::now()))
}

/// The extension's answer: the scope's cookies, stored on the account exactly as cookies
/// typed into the account form are — one vault entry, referenced by `accounts.cookie_ref`.
#[utoipa::path(post, path = "/api/v1/capture/browser-sessions/{id}", tag = "capture", params(("id" = Uuid, Path)), request_body = DeliverBrowserSessionRequest, responses((status = 200, body = MessageResponse), (status = 400, body = crate::error::ErrorBody), (status = 401), (status = 404, body = crate::error::ErrorBody)))]
pub async fn deliver_capture_browser_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<DeliverBrowserSessionRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    validate_secret_value(
        Some(&request.cookies),
        MAX_COOKIE_BYTES,
        "browser_session.cookies_invalid",
        "Cookies",
    )?;
    let handover = state
        .browser_sessions
        .claim(id, chrono::Utc::now())
        .map_err(refused)?;
    let stored = store(&state, &handover, request.cookies).await;
    state.browser_sessions.finish(id, stored.is_ok());
    let count = stored?;
    Ok(Json(
        MessageResponse::new(
            "browser_session.delivered",
            format!(
                "The browser session for {} was stored on the account",
                handover.host()
            ),
        )
        .with_param("host", handover.host())
        .with_param("count", count),
    ))
}

/// The person declined the handover in the extension.
#[utoipa::path(post, path = "/api/v1/capture/browser-sessions/{id}/decline", tag = "capture", params(("id" = Uuid, Path)), responses((status = 200, body = MessageResponse), (status = 401), (status = 404, body = crate::error::ErrorBody)))]
pub async fn decline_capture_browser_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .browser_sessions
        .decline(id, chrono::Utc::now())
        .map_err(refused)?;
    Ok(Json(MessageResponse::new(
        "browser_session.declined",
        "The browser session was not handed over",
    )))
}

/// Checks the cookies against the request's scope and stores them on its account.
///
/// Returns how many cookies were stored. The account row is written through the same
/// `update_account` the account form uses, with every other field as it stands.
async fn store(state: &AppState, handover: &Handover, cookies: String) -> Result<usize, ApiError> {
    let count = cookies_in_scope(&cookies, &handover.scope)?;
    let account = state
        .database
        .list_accounts()
        .await?
        .into_iter()
        .find(|account| account.id == handover.account_id)
        .ok_or_else(account_not_found)?;
    // The provider is what the scope was read from; an account moved to another provider
    // since the request was opened must not receive the first one's cookies.
    if account.provider != handover.provider {
        return Err(refused(Refusal::NotWaiting));
    }
    let (secret_ref, old_cookie_ref) = state
        .database
        .account_secret_refs(account.id)
        .await?
        .ok_or_else(account_not_found)?;
    let new_cookie_ref = state.secrets.put_string(cookies).await?;
    let result = state
        .database
        .update_account(
            account.id,
            rd_db::UpdateAccount {
                provider: account.provider,
                label: account.label,
                username: account.username,
                credential_mode: account.credential_mode,
                secret_ref,
                cookie_ref: Some(new_cookie_ref.clone()),
                proxy_profile_id: account.proxy_profile_id,
                enabled: account.enabled,
            },
        )
        .await;
    if let Err(error) = result {
        cleanup_secrets(&state.secrets, [Some(new_cookie_ref)]).await;
        return Err(error.into());
    }
    if old_cookie_ref.as_ref() != Some(&new_cookie_ref) {
        cleanup_secrets(&state.secrets, [old_cookie_ref]).await;
    }
    crate::hosters::forget(account.id);
    Ok(count)
}

/// Counts the Netscape rows of `content`, refusing the whole set if any row is not one or
/// lies outside `scope`.
///
/// The rule is the account's cookie jar's own, [`rd_http::CookieScope::admit`]: a row's
/// domain must be the scope's host or a domain above it that is not a public suffix. Asked
/// here as well, so a set the jar would refuse at the first download is refused while the
/// extension is still there to be told — and so nothing of another site is ever stored.
pub(crate) fn cookies_in_scope(content: &str, scope: &Url) -> Result<usize, ApiError> {
    let scope = rd_http::CookieScope::provider(scope).map_err(|_| refused(Refusal::NotWaiting))?;
    let host = scope.host();
    let mut rows = 0_usize;
    for line in content.lines().map(str::trim_end) {
        if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
            continue;
        }
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        let fields: Vec<&str> = line.split('\t').collect();
        let [domain, _, _, _, _, name, _] = fields.as_slice() else {
            return Err(ApiError::bad_request(
                "browser_session.cookies_invalid",
                "Every cookie must be one Netscape row",
            ));
        };
        if name.is_empty() {
            return Err(ApiError::bad_request(
                "browser_session.cookies_invalid",
                "Every cookie must be one Netscape row",
            ));
        }
        match scope.admit(domain) {
            Ok(()) => {}
            Err(rd_http::CookieDomainRefused::OutsideScope) => {
                return Err(ApiError::bad_request(
                    "browser_session.cookie_outside_scope",
                    format!("A cookie lies outside {host}"),
                )
                .with_param("host", host));
            }
            Err(rd_http::CookieDomainRefused::PublicSuffix) => {
                return Err(ApiError::bad_request(
                    "browser_session.cookie_public_suffix",
                    format!("A cookie for {host} is set for a public suffix"),
                )
                .with_param("host", host));
            }
        }
        rows += 1;
    }
    if rows == 0 {
        return Err(ApiError::bad_request(
            "browser_session.cookies_empty",
            "The browser sent no cookies",
        ));
    }
    // What the account's jar will do with them at the first request, done once now.
    rd_http::import_cookie_jar(content, &scope).map_err(|_| {
        ApiError::bad_request(
            "browser_session.cookies_invalid",
            "Every cookie must be one Netscape row",
        )
    })?;
    Ok(rows)
}

fn no_request() -> ApiError {
    ApiError::not_found(
        "browser_session.none",
        "No browser session was requested for this account",
    )
}

fn refused(refusal: Refusal) -> ApiError {
    match refusal {
        Refusal::NotWaiting => ApiError::not_found(
            "browser_session.not_waiting",
            "This request for a browser session is no longer waiting",
        ),
    }
}

#[cfg(test)]
#[path = "browser_session_handlers_tests.rs"]
mod tests;
