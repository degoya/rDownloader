//! Configuring the identity provider, binding the administrator's identity there, and the
//! password sign-in switch (RD-190-15, ADR 0021, decisions D2 and D3).
//!
//! **Each of these creates or removes a way in**, so each takes what enrolling a passkey takes: a
//! signed-in session and the password again (`crate::step_up`, O-CONF). A token holding
//! `api:admin` or `api:secrets` must not be able to make its own provider the administrator's,
//! bind its own account, or take the password form away.
//!
//! * The identity is bound by a round trip, never typed (D2): `POST /auth/oidc/link` starts the
//!   ordinary flow with the purpose *link*, and the callback stores the `(issuer, client_id, sub)`
//!   that came back. Changing the issuer or the client ID ends the binding.
//! * The password form goes off only from a session the provider opened — the latest one — with
//!   an identity bound (D3), and comes back on only from this machine:
//!   `rdownloader auth password-login on`, which calls `POST /auth/password-login/on` with the
//!   local control token or, with the service stopped, writes the setting itself. While it is off,
//!   nothing that would end the provider sign-in — removing the configuration, changing the
//!   provider, unlinking — is accepted either: that would leave passkeys and the command line as
//!   the only way in, and the command line is the way back the decision names.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use rd_api_core::oidc_client::{self, LinkedIdentity, ProviderConfig};
use rd_authn::oidc::{self, FlowPurpose};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState, audit::AuditContext, dto::MessageResponse};

/// The identity provider as *Settings → Security* shows it. Never the client secret.
#[derive(Debug, Serialize, ToSchema)]
pub struct OidcSettings {
    pub configured: bool,
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    pub display_name: Option<String>,
    /// Whether a client secret is stored. The secret itself is write-only.
    pub client_secret_set: bool,
    pub group_claim: Option<String>,
    pub group_value: Option<String>,
    pub provider_logout: bool,
    /// The address to register at the provider: the external URL and the callback path.
    /// `None` without an external URL, which the provider sign-in cannot work without.
    pub redirect_uri: Option<String>,
    pub identity: Option<OidcIdentity>,
    /// Whether the password form is offered on the sign-in screen.
    pub password_login: bool,
    /// Whether this session was opened by the latest sign-in through the provider, which is what
    /// switching the password sign-in off requires.
    pub provider_session: bool,
}

/// The identity bound to the administrator, as far as it is shown.
#[derive(Debug, Serialize, ToSchema)]
pub struct OidcIdentity {
    pub issuer: String,
    /// What the provider called the person when it was linked.
    pub label: Option<String>,
    pub linked_at: chrono::DateTime<chrono::Utc>,
}

/// Configures the provider, or changes it.
#[derive(Deserialize, ToSchema)]
pub struct OidcConfigRequest {
    /// The administrator password, again.
    #[schema(write_only)]
    pub password: String,
    pub issuer: String,
    pub client_id: String,
    /// Required the first time; left out, the stored secret is kept.
    #[serde(default)]
    #[schema(write_only)]
    pub client_secret: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub group_claim: Option<String>,
    #[serde(default)]
    pub group_value: Option<String>,
    #[serde(default)]
    pub provider_logout: bool,
}

/// The password, again, for a change that needs nothing else.
#[derive(Deserialize, ToSchema)]
pub struct OidcStepUpRequest {
    #[schema(write_only)]
    pub password: String,
}

/// Where to send the browser to link the identity.
#[derive(Serialize, ToSchema)]
pub struct OidcLinkStart {
    pub authorization_url: String,
}

/// The identity provider's configuration and the bound identity.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc",
    tag = "security",
    responses((status = 200, body = OidcSettings))
)]
pub async fn get_oidc_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<OidcSettings>, ApiError> {
    Ok(Json(view(&state, &headers).await?))
}

async fn view(state: &AppState, headers: &HeaderMap) -> Result<OidcSettings, ApiError> {
    let config = oidc_client::config(state).await?;
    let identity = match &config {
        Some(config) => oidc_client::identity(state, config).await?,
        None => None,
    };
    let current = state
        .auth
        .current_session(state, headers)
        .await
        .map(|session| session.id.to_string());
    let proven = oidc_client::proven_session(state).await?;
    Ok(OidcSettings {
        configured: config.is_some(),
        issuer: config.as_ref().map(|config| config.issuer.clone()),
        client_id: config.as_ref().map(|config| config.client_id.clone()),
        display_name: config.as_ref().map(|config| config.display_name.clone()),
        client_secret_set: config.is_some(),
        group_claim: config
            .as_ref()
            .and_then(|config| config.group_claim.clone()),
        group_value: config
            .as_ref()
            .and_then(|config| config.group_value.clone()),
        provider_logout: config.as_ref().is_some_and(|config| config.provider_logout),
        redirect_uri: oidc_client::redirect_uri(state).await,
        provider_session: identity.is_some() && current.is_some() && current == proven,
        identity: identity.map(|identity| OidcIdentity {
            issuer: identity.issuer,
            label: identity.label,
            linked_at: identity.linked_at,
        }),
        password_login: !oidc_client::password_login_off(state).await?,
    })
}

/// Configures the identity provider. Requires a signed-in session and the password.
///
/// The provider's discovery document is fetched and checked before anything is stored, so a
/// provider that could not be used — another issuer, no PKCE, no allowed algorithm — is refused
/// here and not at the first sign-in.
#[utoipa::path(
    put,
    path = "/api/v1/auth/oidc",
    tag = "security",
    request_body = OidcConfigRequest,
    responses(
        (status = 200, body = OidcSettings),
        (status = 400, description = "A field is not usable", body = crate::error::ErrorBody),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
        (status = 403, description = "Not a signed-in session", body = crate::error::ErrorBody),
        (status = 409, description = "No external URL, or the password sign-in is off", body = crate::error::ErrorBody),
        (status = 422, description = "The provider breaks a rule", body = crate::error::ErrorBody),
        (status = 502, description = "The provider could not be reached", body = crate::error::ErrorBody),
    )
)]
pub async fn put_oidc_settings(
    State(state): State<AppState>,
    audit: AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
    Json(request): Json<OidcConfigRequest>,
) -> Result<Json<OidcSettings>, ApiError> {
    crate::step_up::require_step_up(
        &state,
        &audit,
        this_machine,
        client.0,
        &request.password,
        rd_core::AuditAction::SettingsChanged,
    )
    .await?;
    let issuer = request.issuer.trim().to_owned();
    if !oidc::issuer_allowed(&issuer) {
        return Err(ApiError::bad_request(
            "auth.oidc_issuer_invalid",
            "The issuer has to be an https address, or http on this machine",
        ));
    }
    let client_id = request.client_id.trim().to_owned();
    if client_id.is_empty() || client_id.len() > 255 {
        return Err(ApiError::bad_request(
            "auth.oidc_client_id_invalid",
            "The client ID is required",
        ));
    }
    let display_name = request
        .display_name
        .trim()
        .chars()
        .take(60)
        .collect::<String>();
    if display_name.is_empty() {
        return Err(ApiError::bad_request(
            "auth.oidc_display_name_required",
            "The provider needs a name for the sign-in button",
        ));
    }
    let group_claim = cleaned(request.group_claim);
    let group_value = cleaned(request.group_value);
    if group_claim.is_some() != group_value.is_some()
        || group_claim.as_deref().is_some_and(|claim| {
            !claim
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-.:/".contains(c))
        })
    {
        return Err(ApiError::bad_request(
            "auth.oidc_group_invalid",
            "A group condition needs both a claim name and a value",
        ));
    }
    if oidc_client::redirect_uri(&state).await.is_none() {
        return Err(crate::oidc_handlers::requires_external_url());
    }

    let previous = oidc_client::config(&state).await?;
    let provider_changed = previous
        .as_ref()
        .is_none_or(|previous| previous.issuer != issuer || previous.client_id != client_id);
    let linked = match &previous {
        Some(previous) => oidc_client::identity(&state, previous).await?,
        None => None,
    };
    if provider_changed && linked.is_some() && oidc_client::password_login_off(&state).await? {
        return Err(crate::password_login_handlers::password_login_is_off());
    }
    let new_secret = cleaned(request.client_secret);
    // Another provider or client gets its own secret: the stored one was issued for the old.
    if new_secret.is_none() && provider_changed {
        return Err(ApiError::bad_request(
            "auth.oidc_secret_required",
            "The client secret is required",
        ));
    }

    // Another provider starts from an empty cache, so the discovery below is the one that
    // stays; forgetting after it would throw the fresh document and key set away again.
    if provider_changed {
        state.oidc.forget().await;
    }
    // Checked before anything is written: a provider that cannot be used is refused now.
    state
        .oidc
        .discover(&state, &issuer)
        .await
        .map_err(crate::oidc_handlers::provider_error)?;

    let secret_ref = match &new_secret {
        Some(secret) => state.secrets.put_string(secret.clone()).await?,
        None => previous
            .as_ref()
            .map(|previous| previous.secret_ref.clone())
            .unwrap_or_default(),
    };
    let config = ProviderConfig {
        issuer,
        client_id,
        display_name,
        group_claim,
        group_value,
        provider_logout: request.provider_logout,
        secret_ref,
    };
    oidc_client::store(&state, oidc_client::CONFIG_SETTING, Some(&config)).await?;
    if new_secret.is_some()
        && let Some(previous) = &previous
        && let Err(error) = state.secrets.remove(&previous.secret_ref).await
    {
        tracing::warn!(%error, "the replaced client secret could not be removed");
    }
    let fields = changed_fields(previous.as_ref(), &config, new_secret.is_some());
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(&audit)
            .client(client.0)
            .target("identity_provider", &config.issuer)
            .detail("fields", fields.join(" ")),
    )
    .await;
    if provider_changed {
        // The binding is the triple: another issuer or client cannot keep it.
        if let Some(linked) = linked {
            release_identity(&state, &audit, client.0, &linked).await?;
        }
    }
    Ok(Json(view(&state, &headers).await?))
}

/// Removes the identity provider, its secret and the bound identity.
#[utoipa::path(
    delete,
    path = "/api/v1/auth/oidc",
    tag = "security",
    request_body = OidcStepUpRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
        (status = 403, description = "Not a signed-in session", body = crate::error::ErrorBody),
        (status = 409, description = "The password sign-in is off", body = crate::error::ErrorBody),
    )
)]
pub async fn delete_oidc_settings(
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
        rd_core::AuditAction::SettingsChanged,
    )
    .await?;
    if oidc_client::password_login_off(&state).await? {
        return Err(crate::password_login_handlers::password_login_is_off());
    }
    let Some(config) = oidc_client::config(&state).await? else {
        return Err(crate::oidc_handlers::not_configured());
    };
    let linked = oidc_client::identity(&state, &config).await?;
    oidc_client::store::<ProviderConfig>(&state, oidc_client::CONFIG_SETTING, None).await?;
    if let Err(error) = state.secrets.remove(&config.secret_ref).await {
        tracing::warn!(%error, "the removed provider's client secret could not be removed");
    }
    state.oidc.forget().await;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(&audit)
            .client(client.0)
            .target("identity_provider", &config.issuer)
            .detail("fields", "removed"),
    )
    .await;
    if let Some(linked) = linked {
        release_identity(&state, &audit, client.0, &linked).await?;
    }
    Ok(Json(MessageResponse::new(
        "auth.oidc_removed",
        "The identity provider was removed",
    )))
}

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
        .unwrap_or_else(|| "this_machine".to_owned());
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
async fn release_identity(
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

fn cleaned(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// The names of the fields a change touched, for the audit record. Names only, never values.
fn changed_fields(
    previous: Option<&ProviderConfig>,
    next: &ProviderConfig,
    secret: bool,
) -> Vec<&'static str> {
    let Some(previous) = previous else {
        return vec!["configured"];
    };
    let mut fields = Vec::new();
    for (name, changed) in [
        ("issuer", previous.issuer != next.issuer),
        ("client_id", previous.client_id != next.client_id),
        ("client_secret", secret),
        ("display_name", previous.display_name != next.display_name),
        (
            "group",
            previous.group_claim != next.group_claim || previous.group_value != next.group_value,
        ),
        (
            "provider_logout",
            previous.provider_logout != next.provider_logout,
        ),
    ] {
        if changed {
            fields.push(name);
        }
    }
    fields
}
