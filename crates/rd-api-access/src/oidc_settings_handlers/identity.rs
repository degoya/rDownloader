//! Binding the administrator's identity at the provider, and releasing it.

use super::*;

/// Starts linking an identity at the provider to the administrator (D2). Requires a signed-in
/// session and the password; the browser is then sent to the provider and comes back to
/// *Settings → Security*.
#[utoipa::path(
    post,
    path = "/api/v1/auth/oidc/link",
    tag = "security",
    request_body = OidcStepUpRequest,
    responses(
        (status = 200, body = OidcLinkStart),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
        (status = 403, description = "Not a signed-in session", body = crate::error::ErrorBody),
        (status = 409, description = "No provider configured, or no external URL", body = crate::error::ErrorBody),
    )
)]
pub async fn link_oidc_identity(
    State(state): State<AppState>,
    audit: AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
    client: crate::client::ClientAddress,
    Json(request): Json<OidcStepUpRequest>,
) -> Result<Response, ApiError> {
    crate::step_up::require_step_up(
        &state,
        &audit,
        this_machine,
        client.0,
        &request.password,
        rd_core::AuditAction::IdentityLinked,
    )
    .await?;
    // Who asked, for the record the callback writes. With the login switched off for this
    // machine there is no session; the flow is then this machine's.
    let session = audit
        .actor
        .id
        .clone()
        .unwrap_or_else(|| crate::oidc_handlers::THIS_MACHINE.to_owned());
    let started = crate::oidc_handlers::begin(
        &state,
        client.0,
        FlowPurpose::Link { session },
        crate::oidc_handlers::SECURITY_PAGE,
    )
    .await?;
    let mut response = Json(OidcLinkStart {
        authorization_url: started.authorization_url,
    })
    .into_response();
    let cookie = HeaderValue::from_str(&started.cookie).map_err(|_| {
        ApiError::bad_request("auth.oidc_start_failed", "The sign-in could not be started")
    })?;
    response.headers_mut().append(header::SET_COOKIE, cookie);
    Ok(response)
}

/// Releases the bound identity. Requires a signed-in session and the password.
#[utoipa::path(
    delete,
    path = "/api/v1/auth/oidc/identity",
    tag = "security",
    request_body = OidcStepUpRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
        (status = 403, description = "Not a signed-in session", body = crate::error::ErrorBody),
        (status = 404, description = "No identity is linked", body = crate::error::ErrorBody),
        (status = 409, description = "The password sign-in is off", body = crate::error::ErrorBody),
    )
)]
pub async fn unlink_oidc_identity(
    State(state): State<AppState>,
    audit: AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
    client: crate::client::ClientAddress,
    Json(request): Json<OidcStepUpRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::step_up::require_step_up(
        &state,
        &audit,
        this_machine,
        client.0,
        &request.password,
        rd_core::AuditAction::IdentityUnlinked,
    )
    .await?;
    if oidc_client::password_login_off(&state).await? {
        return Err(crate::password_login_handlers::password_login_is_off());
    }
    let linked = match oidc_client::config(&state).await? {
        Some(config) => oidc_client::identity(&state, &config).await?,
        None => None,
    };
    let Some(linked) = linked else {
        return Err(ApiError::not_found(
            "auth.oidc_not_linked",
            "No identity at the provider is linked to the administrator",
        ));
    };
    release_identity(&state, &audit, client.0, &linked).await?;
    Ok(Json(MessageResponse::new(
        "auth.oidc_unlinked",
        "The identity was unlinked",
    )))
}

/// Ends the binding, and with it the proof a provider session gave.
pub(super) async fn release_identity(
    state: &AppState,
    audit: &AuditContext,
    client: std::net::IpAddr,
    linked: &LinkedIdentity,
) -> Result<(), ApiError> {
    oidc_client::store::<LinkedIdentity>(state, oidc_client::IDENTITY_SETTING, None).await?;
    oidc_client::store::<String>(state, oidc_client::PROVEN_SESSION_SETTING, None).await?;
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::IdentityUnlinked)
            .by(audit)
            .client(client)
            .target("identity_provider", &linked.issuer),
    )
    .await;
    Ok(())
}
