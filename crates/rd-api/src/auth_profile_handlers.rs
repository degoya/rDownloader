//! CRUD, approval and the live test action for reusable per-domain auth profiles.
//!
//! Credential values enter through write-only request fields, are validated, and are then
//! handed straight to the secret store. Nothing here ever returns a stored value or its
//! `vault://` reference.

use std::time::Duration;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_core::{
    AuthMethod, AuthOrigin, AuthProfile, AuthProfileId, AuthScope, MAX_AUTH_CERTIFICATE,
    MAX_AUTH_COOKIES, MAX_AUTH_SECRET, ScopeError,
};
use rd_db::StoreErrorKind;
use secrecy::ExposeSecret;

use crate::{
    ApiError, AppState,
    config_handlers::{cleanup_secrets, store_optional, validate_name, validate_secret_value},
    dto::{
        AuthProfileTestResponse, CaptureCookiesRequest, CreateAuthProfileRequest, MessageResponse,
        UpdateAuthProfileRequest,
    },
};

/// How long the test action waits for the scope to answer.
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

#[utoipa::path(get, path = "/api/v1/auth-profiles", tag = "configuration", responses((status = 200, body = [rd_core::AuthProfile])))]
pub async fn list_auth_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<AuthProfile>>, ApiError> {
    Ok(Json(state.database.list_auth_profiles().await?))
}

#[utoipa::path(post, path = "/api/v1/auth-profiles", tag = "configuration", request_body = CreateAuthProfileRequest, responses((status = 201, body = rd_core::AuthProfile), (status = 400)))]
pub async fn create_auth_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateAuthProfileRequest>,
) -> Result<(StatusCode, Json<AuthProfile>), ApiError> {
    validate_name(&request.name)?;
    let scope = parse_scope(&request.scope, request.include_subdomains)?;
    let username = normalized(request.username);
    let secret = normalized(request.secret);
    validate_method(request.method, username.as_deref(), secret.as_deref(), true)?;
    validate_secret_size(request.method, secret.as_deref())?;
    if let (AuthMethod::Cookies, Some(cookies)) = (request.method, secret.as_deref()) {
        validate_cookies(&scope, cookies)?;
    }
    let certificate = validate_certificate(normalized(request.certificate_pem))?;

    let secret_ref = store_optional(&state.secrets, secret).await?;
    let certificate_ref = store_optional(&state.secrets, certificate).await?;
    let result = state
        .database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: request.name.trim().to_owned(),
            scope,
            method: request.method,
            origin: AuthOrigin::Manual,
            enabled: request.enabled,
            expires_at: request.expires_at,
            username,
            secret_ref: secret_ref.clone(),
            certificate_ref: certificate_ref.clone(),
        })
        .await;
    match result {
        Ok(profile) => Ok((StatusCode::CREATED, Json(profile))),
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref, certificate_ref]).await;
            Err(map_store_error(error))
        }
    }
}

#[utoipa::path(put, path = "/api/v1/auth-profiles/{id}", tag = "configuration", params(("id" = rd_core::AuthProfileId, Path)), request_body = UpdateAuthProfileRequest, responses((status = 200, body = rd_core::AuthProfile), (status = 400), (status = 404)))]
pub async fn update_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<AuthProfileId>,
    Json(request): Json<UpdateAuthProfileRequest>,
) -> Result<Json<AuthProfile>, ApiError> {
    validate_name(&request.name)?;
    let scope = parse_scope(&request.scope, request.include_subdomains)?;
    let stored = state
        .database
        .auth_profile(id)
        .await?
        .ok_or_else(not_found)?;
    let username = normalized(request.username);
    let secret = normalized(request.secret);
    validate_secret_size(request.method, secret.as_deref())?;
    let certificate = validate_certificate(normalized(request.certificate_pem))?;

    // An omitted credential keeps the stored one; the method may only change when the
    // profile ends up with a credential that fits it.
    let keeps_secret = secret.is_none() && stored.method == request.method;
    validate_method(
        request.method,
        username.as_deref(),
        secret.as_deref(),
        !keeps_secret,
    )?;
    if request.method == AuthMethod::Cookies {
        match (secret.as_deref(), &stored.secret_ref) {
            (Some(cookies), _) => validate_cookies(&scope, cookies)?,
            // Kept cookies were checked against the stored scope; a new host or subdomain
            // setting has to hold for them as well.
            (None, Some(reference))
                if keeps_secret && changes_cookie_reach(&stored.scope, &scope) =>
            {
                let cookies = state.secrets.get(reference).await?;
                validate_cookies(&scope, cookies.expose_secret())?;
            }
            _ => {}
        }
    }
    let secret_ref = match secret {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if keeps_secret => stored.secret_ref.clone(),
        // Switching method invalidates the old credential rather than sending a password
        // as a bearer token.
        None => None,
    };
    let certificate_ref = match certificate {
        Some(value) => Some(state.secrets.put_string(value).await?),
        None if request.clear_certificate => None,
        None => stored.certificate_ref.clone(),
    };

    let result = state
        .database
        .update_auth_profile(
            id,
            rd_db::UpdateAuthProfile {
                name: request.name.trim().to_owned(),
                scope,
                method: request.method,
                enabled: request.enabled,
                expires_at: request.expires_at,
                username,
                secret_ref: secret_ref.clone(),
                certificate_ref: certificate_ref.clone(),
            },
        )
        .await;
    match result {
        Ok((profile, orphaned)) => {
            cleanup_secrets(&state.secrets, orphaned.into_iter().map(Some)).await;
            Ok(Json(profile))
        }
        Err(error) => {
            // Only the references minted in this request may be dropped; the stored ones
            // are still in use by the unchanged row.
            let minted = [
                secret_ref.filter(|value| Some(value) != stored.secret_ref.as_ref()),
                certificate_ref.filter(|value| Some(value) != stored.certificate_ref.as_ref()),
            ];
            cleanup_secrets(&state.secrets, minted).await;
            Err(map_store_error(error))
        }
    }
}

#[utoipa::path(post, path = "/api/v1/auth-profiles/{id}/enable", tag = "configuration", params(("id" = rd_core::AuthProfileId, Path)), responses((status = 200, body = rd_core::AuthProfile), (status = 404)))]
pub async fn enable_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<AuthProfileId>,
) -> Result<Json<AuthProfile>, ApiError> {
    set_enabled(&state, id, true).await
}

#[utoipa::path(post, path = "/api/v1/auth-profiles/{id}/disable", tag = "configuration", params(("id" = rd_core::AuthProfileId, Path)), responses((status = 200, body = rd_core::AuthProfile), (status = 404)))]
pub async fn disable_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<AuthProfileId>,
) -> Result<Json<AuthProfile>, ApiError> {
    set_enabled(&state, id, false).await
}

async fn set_enabled(
    state: &AppState,
    id: AuthProfileId,
    enabled: bool,
) -> Result<Json<AuthProfile>, ApiError> {
    state
        .database
        .set_auth_profile_enabled(id, enabled)
        .await
        .map(Json)
        .map_err(|_| not_found())
}

#[utoipa::path(delete, path = "/api/v1/auth-profiles/{id}", tag = "configuration", params(("id" = rd_core::AuthProfileId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn delete_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<AuthProfileId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let orphaned = state
        .database
        .delete_auth_profile(id)
        .await
        .map_err(|_| not_found())?;
    cleanup_secrets(&state.secrets, orphaned.into_iter().map(Some)).await;
    Ok(Json(MessageResponse::new(
        "authprofile.deleted",
        "Auth profile deleted",
    )))
}

#[utoipa::path(post, path = "/api/v1/auth-profiles/{id}/test", tag = "configuration", params(("id" = rd_core::AuthProfileId, Path)), responses((status = 200, body = AuthProfileTestResponse), (status = 404), (status = 502)))]
pub async fn test_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<AuthProfileId>,
) -> Result<Json<AuthProfileTestResponse>, ApiError> {
    let profile = state
        .database
        .auth_profile(id)
        .await?
        .ok_or_else(not_found)?;
    if profile.is_expired(chrono::Utc::now()) {
        return Err(ApiError::bad_request(
            "authprofile.expired",
            "Auth profile has expired",
        ));
    }
    let url = profile.scope.probe_url().ok_or_else(|| {
        ApiError::bad_request("authprofile.scope_invalid", "Auth profile scope is invalid")
    })?;
    // Bypass selection so the test exercises exactly this profile, even while it is
    // still disabled and awaiting approval.
    let network = state
        .scheduler
        .test_client(&url, profile)
        .await
        .map_err(|error| {
            ApiError::bad_gateway("authprofile.test_failed", "Auth profile test failed")
                .with_param("reason", error)
        })?;
    let mut request = network.client.get(url.clone());
    for (name, value) in &network.headers {
        request = request.header(name, value);
    }
    let response = tokio::time::timeout(TEST_TIMEOUT, request.send())
        .await
        .map_err(|_| {
            ApiError::bad_gateway("authprofile.test_failed", "Auth profile test timed out")
        })?
        .map_err(|error| {
            ApiError::bad_gateway("authprofile.test_failed", "Auth profile test failed")
                .with_param("reason", error)
        })?;
    let status = response.status();
    Ok(Json(AuthProfileTestResponse {
        reachable: true,
        authenticated: status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
        status: Some(status.as_u16()),
        url: url.to_string(),
    }))
}

#[utoipa::path(post, path = "/api/v1/capture/cookies", tag = "capture", request_body = CaptureCookiesRequest, responses((status = 201, body = rd_core::AuthProfile), (status = 400)))]
pub async fn capture_cookies(
    State(state): State<AppState>,
    Json(request): Json<CaptureCookiesRequest>,
) -> Result<(StatusCode, Json<AuthProfile>), ApiError> {
    let scope = parse_scope(&request.scope, request.include_subdomains)?;
    validate_secret_value(
        Some(&request.cookies),
        MAX_AUTH_COOKIES,
        "authprofile.credentials_invalid",
        "Cookies",
    )?;
    // Reject cookies that do not belong to the approved domain before storing anything.
    validate_cookies(&scope, &request.cookies)?;
    // A browser session ends when its cookies do, so inherit the earliest expiry.
    let expires_at = rd_http::earliest_expiry(&request.cookies);
    let name = normalized(request.name).unwrap_or_else(|| scope.host.clone());
    validate_name(&name)?;

    let secret_ref = Some(state.secrets.put_string(request.cookies).await?);
    let result = state
        .database
        .create_auth_profile(rd_db::NewAuthProfile {
            name,
            scope,
            method: AuthMethod::Cookies,
            // The store forces this disabled; a person approves it in the web UI.
            origin: AuthOrigin::BrowserCapture,
            enabled: false,
            expires_at,
            username: None,
            secret_ref: secret_ref.clone(),
            certificate_ref: None,
        })
        .await;
    match result {
        Ok(profile) => Ok((StatusCode::CREATED, Json(profile))),
        Err(error) => {
            cleanup_secrets(&state.secrets, [secret_ref]).await;
            Err(map_store_error(error))
        }
    }
}

fn parse_scope(input: &str, include_subdomains: bool) -> Result<AuthScope, ApiError> {
    AuthScope::parse(input, include_subdomains).map_err(|error| match error {
        ScopeError::PathInvalid => ApiError::bad_request(
            "authprofile.path_invalid",
            "Auth profile path prefix must start with a slash",
        ),
        ScopeError::UnsupportedScheme => ApiError::bad_request(
            "authprofile.insecure_scheme",
            "Auth profile scope must use http or https",
        ),
        ScopeError::Invalid => ApiError::bad_request(
            "authprofile.scope_invalid",
            "Auth profile scope is not a valid domain",
        ),
    })
}

/// Refuses a cookie set the download would refuse, before anything is stored (RD-120-54).
///
/// The scope is the one the download builds for the profile (`rd-scheduler`'s
/// `assemble_client`, `rd-media`'s cookie file), and the import is the jar's own: every
/// Netscape row must pass [`rd_http::CookieScope::admit`] (RD-120-49), so a parent-domain
/// row stays allowed while a foreign or public-suffix row refuses the whole set.
fn validate_cookies(scope: &AuthScope, cookies: &str) -> Result<(), ApiError> {
    let target = scope.probe_url().ok_or_else(|| {
        ApiError::bad_request("authprofile.scope_invalid", "Auth profile scope is invalid")
    })?;
    rd_http::CookieScope::new(&target, scope.include_subdomains)
        .and_then(|cookie_scope| rd_http::import_cookie_jar(cookies, &cookie_scope))
        .map(drop)
        .map_err(|error| match error.downcast_ref() {
            Some(rd_http::CookieDomainRefused::PublicSuffix) => ApiError::bad_request(
                "authprofile.cookie_public_suffix",
                "A cookie is set for a public suffix such as com or co.uk",
            ),
            Some(rd_http::CookieDomainRefused::OutsideScope) => ApiError::bad_request(
                "authprofile.cookie_outside_scope",
                format!("A cookie lies outside {}", scope.host),
            )
            .with_param("host", &scope.host),
            None => ApiError::bad_request(
                "authprofile.credentials_invalid",
                "Cookies could not be imported",
            )
            .with_param("reason", error),
        })
}

/// Whether a scope change can move a stored cookie out of reach: the path prefix plays no
/// part in which cookie domains are admitted.
fn changes_cookie_reach(stored: &AuthScope, next: &AuthScope) -> bool {
    stored.host != next.host || stored.include_subdomains != next.include_subdomains
}

/// Checks that the credential fields match the chosen method.
fn validate_method(
    method: AuthMethod,
    username: Option<&str>,
    secret: Option<&str>,
    secret_required: bool,
) -> Result<(), ApiError> {
    if secret_required && secret.is_none() {
        return Err(ApiError::bad_request(
            "authprofile.credentials_required",
            "Auth profile needs a credential",
        ));
    }
    let username_expected = method == AuthMethod::Basic;
    if username_expected != username.is_some() {
        return Err(ApiError::bad_request(
            "authprofile.method_mismatch",
            "Basic authentication needs a username, the other methods do not",
        ));
    }
    Ok(())
}

fn validate_secret_size(method: AuthMethod, secret: Option<&str>) -> Result<(), ApiError> {
    let maximum = if method == AuthMethod::Cookies {
        MAX_AUTH_COOKIES
    } else {
        MAX_AUTH_SECRET
    };
    validate_secret_value(
        secret,
        maximum,
        "authprofile.credentials_invalid",
        "Credential",
    )
}

/// Parses the bundle now so an unusable certificate is reported at save time rather than
/// at the first download. `Identity::from_pem` also rejects encrypted private keys, which
/// cannot be decrypted anywhere in this pipeline.
fn validate_certificate(pem: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(pem) = pem else { return Ok(None) };
    validate_secret_value(
        Some(&pem),
        MAX_AUTH_CERTIFICATE,
        "authprofile.certificate_invalid",
        "Client certificate",
    )?;
    if pem.contains("ENCRYPTED PRIVATE KEY") {
        return Err(ApiError::bad_request(
            "authprofile.key_encrypted",
            "Passphrase-protected private keys are not supported; supply a decrypted PEM",
        ));
    }
    reqwest::Identity::from_pem(pem.as_bytes()).map_err(|error| {
        ApiError::bad_request(
            "authprofile.certificate_invalid",
            "Client certificate could not be parsed",
        )
        .with_param("reason", error)
    })?;
    Ok(Some(pem))
}

fn normalized(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn not_found() -> ApiError {
    ApiError::not_found("authprofile.not_found", "Auth profile not found")
}

/// The unique scope index is the only constraint a caller can realistically trip.
fn map_store_error(error: anyhow::Error) -> ApiError {
    if rd_db::store_kind(&error) == Some(StoreErrorKind::Duplicate) {
        return ApiError::conflict(
            "authprofile.scope_conflict",
            "Another auth profile already covers this scope",
        );
    }
    ApiError::from(error)
}
