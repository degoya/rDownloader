//! Sign-in, first-run setup and the pairing of capture agents.
//!
//! The three sign-in operations carry `tag = "handlers"` explicitly: it is the tag utoipa derived
//! from the module they lived in until RD-160-06, and the API contract does not move with a file.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{
    ApiError, AppState,
    dto::{
        AuthStatus, CapturePairRequest, CapturePairResponse, LoginRequest, MessageResponse,
        SetupRequest,
    },
};

#[utoipa::path(get, path = "/api/v1/auth/status", tag = "handlers", responses((status = 200, body = AuthStatus)))]
pub async fn auth_status(
    State(state): State<AppState>,
    crate::client::ThisMachine(from_this_machine): crate::client::ThisMachine,
    headers: HeaderMap,
) -> Result<Json<AuthStatus>, ApiError> {
    // A caller elsewhere is told the truth about the login it has to pass: the switch only
    // lets this machine in.
    if state.auth.disabled_for(from_this_machine) {
        return Ok(Json(AuthStatus {
            setup_required: false,
            authenticated: true,
            login_disabled: true,
            passkeys_available: false,
            oidc_available: false,
            oidc_display_name: None,
            password_login: true,
        }));
    }
    let configured = state.auth.is_configured(&state).await?;
    let authenticated = configured && state.auth.authenticated(&state, &headers).await;
    // The same reasoning as for the passkey below: the button has to be known to be drawn, and
    // the start route would tell anybody who asked that a provider is configured.
    let provider = crate::oidc_handlers::offered_provider(&state).await?;
    Ok(Json(AuthStatus {
        setup_required: !configured,
        authenticated,
        login_disabled: false,
        // Told to an anonymous caller deliberately: the sign-in screen cannot offer a passkey
        // button it does not know to draw, and the alternative — always offering it and
        // failing — teaches people to ignore the button. What it reveals is that this
        // installation has a passkey, which the challenge endpoint would reveal anyway.
        passkeys_available: crate::passkey_handlers::any_passkey_enrolled(&state).await?,
        oidc_available: provider.is_some(),
        oidc_display_name: provider,
        password_login: !rd_api_core::oidc_client::password_login_off(&state).await?,
    }))
}

#[utoipa::path(post, path = "/api/v1/auth/setup", tag = "handlers", request_body = SetupRequest, responses((status = 200, body = MessageResponse)))]
pub async fn setup(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    Json(request): Json<SetupRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    state.auth.setup(&state, &request.password).await?;
    // Who set the first password, and from where: until this moment the installation belonged
    // to whoever reached it first, so the address is the one fact worth having afterwards.
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SetupCompleted)
            .actor(crate::audit::Actor::anonymous())
            .client(client.0),
    )
    .await;
    Ok(Json(MessageResponse::new(
        "auth.setup_done",
        "Setup completed",
    )))
}

/// One refused sign-in, recorded.
///
/// `reason` names the *stage* that refused, never the value that was wrong: "password",
/// "password+totp", "locked_out". An audit log of failed sign-ins that quoted the attempts
/// would be a dictionary of near-miss passwords, which is the one thing it must not become.
async fn note_failed_login_audit(state: &AppState, client: std::net::IpAddr, reason: &str) {
    crate::audit::record(
        state,
        crate::audit::AuditEvent::failure(rd_core::AuditAction::LoginFailed)
            .actor(crate::audit::Actor::anonymous())
            .client(client)
            .detail("stage", reason),
    )
    .await;
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "handlers",
    request_body = LoginRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 429, description = "Too many failed attempts from this address", body = crate::error::ErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    // Checked before the password, and reported as a refusal rather than a wrong password:
    // telling a locked-out caller "invalid credentials" would leave them guessing at why a
    // password they know is right keeps failing.
    // Held until the verdict is recorded: until then the attempt counts as a failure.
    let _attempt = match state.auth.gate(client.0).await {
        Ok(attempt) => attempt,
        Err(refusal) => {
            note_failed_login_audit(&state, client.0, "locked_out").await;
            return Err(refusal);
        }
    };
    // Switched off after a proven sign-in through the identity provider (D3, RD-190-15). Refused
    // before the password is looked at, so the refusal says nothing about it; the password stays
    // the step-up credential everywhere else. Only `rdownloader auth password-login on`, on this
    // machine, turns the form back on.
    if rd_api_core::oidc_client::password_login_off(&state).await? {
        note_failed_login_audit(&state, client.0, "password_login_off").await;
        return Err(ApiError::forbidden(
            "auth.password_login_off",
            "Signing in with the password is switched off; sign in through the identity provider",
        ));
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .and_then(rd_core::truncate_user_agent);

    // The password is checked first and its result held, so a wrong password and a wrong code
    // take the same path and the same time. Refusing early on the password would make the two
    // distinguishable, and an attacker who can tell which half failed has halved the problem.
    let password_ok = state
        .auth
        .password_matches(&state, &request.password)
        .await?;
    // Only the authenticator app gates this path. A passkey is an alternative way in, not an
    // extra step in front of the password — it already carries its own user verification — and
    // counting one here would demand a code from somebody who has no authenticator app at all.
    let second_factor = state.database.list_mfa_credentials().await?;
    let mfa_required = second_factor.iter().any(|credential| {
        credential.kind == rd_core::MfaKind::Totp && credential.confirmed_at.is_some()
    });
    if mfa_required {
        match request
            .code
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            None if password_ok => {
                // Only after the password was right: otherwise this reply would tell an
                // unauthenticated caller that the account exists and uses a second factor.
                return Err(ApiError::unauthorized(
                    "auth.mfa_required",
                    "A code from your authenticator app is required",
                ));
            }
            None => {
                state.auth.note_failed_login(client.0).await;
                note_failed_login_audit(&state, client.0, "password").await;
                return Err(crate::error_codes::invalid_credentials());
            }
            Some(code) => {
                // Two steps on purpose. Working out *what* the code is happens whichever half
                // is wrong — it is the secret-store read, the HMAC over the accepted window and
                // the recovery-digest comparison — so a wrong password does not return
                // measurably sooner than a wrong code and the refusal below is the same either
                // way. Only *spending* what was found waits for the password: consuming first
                // let anyone holding a recovery-code printout and no password burn a code
                // permanently on a login that was refused anyway.
                let matched = crate::mfa_handlers::second_factor_match(&state, code).await?;
                let code_ok = password_ok
                    && crate::mfa_handlers::consume_second_factor(&state, matched).await?;
                if !code_ok {
                    state.auth.note_failed_login(client.0).await;
                    // Deliberately the same reason either way. The refusal does not tell the
                    // caller which half failed, and neither does the record: an audit log a
                    // caller cannot read is still one an attacker who gains it could.
                    note_failed_login_audit(&state, client.0, "password+totp").await;
                    return Err(crate::error_codes::invalid_credentials());
                }
            }
        }
    } else if !password_ok {
        state.auth.note_failed_login(client.0).await;
        note_failed_login_audit(&state, client.0, "password").await;
        return Err(crate::error_codes::invalid_credentials());
    }

    let opened = state
        .auth
        .open_session(&state, client.0, user_agent)
        .await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::LoginSucceeded)
            .actor(crate::audit::Actor::session(opened.id.to_string()))
            .client(client.0)
            .target("session", opened.id)
            .detail(
                "method",
                if mfa_required {
                    "password+totp"
                } else {
                    "password"
                },
            ),
    )
    .await;
    let mut response = Json(MessageResponse::new("auth.logged_in", "Logged in")).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&{
            let proxy = state.proxy.read().await;
            crate::AuthService::cookie(
                &opened.token,
                proxy.cookie_is_secure(),
                proxy.base_path(),
                opened.max_age_seconds,
            )
        })
        .map_err(|_| {
            ApiError::bad_request("auth.session_create_failed", "Session could not be created")
        })?,
    );
    Ok(response)
}

#[utoipa::path(post, path = "/api/v1/capture/pair", tag = "capture", request_body = CapturePairRequest, responses((status = 201, body = CapturePairResponse)))]
pub async fn pair_capture(
    State(state): State<AppState>,
    Json(request): Json<CapturePairRequest>,
) -> Result<(StatusCode, Json<CapturePairResponse>), ApiError> {
    let mut scopes = vec![rd_core::CAPTURE_SCOPE.to_owned()];
    if request.queue_control {
        scopes.push(rd_core::CAPTURE_QUEUE_SCOPE.to_owned());
    }
    let response =
        crate::api_tokens::pair_with_scopes(&state, &request.label, scopes, "capture.label_length")
            .await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(get, path = "/api/v1/capture/agents", tag = "capture", responses((status = 200, body = [rd_core::CaptureToken])))]
pub async fn list_capture_agents(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::CaptureToken>>, ApiError> {
    Ok(Json(
        state
            .database
            .list_capture_tokens(&[rd_core::CAPTURE_SCOPE])
            .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/capture/agents/{id}", tag = "capture", params(("id" = rd_core::CaptureTokenId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn revoke_capture_agent(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CaptureTokenId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .revoke_capture_token(id)
        .await
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "capture.not_found",
                "Capture token not found",
            )
        })?;
    Ok(Json(MessageResponse::new(
        "capture.revoked",
        "Capture token revoked",
    )))
}
