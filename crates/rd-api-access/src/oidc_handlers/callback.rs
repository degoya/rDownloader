//! What a callback ends in: the code redeemed and the token verified, then a session, a linked
//! identity or a refusal.

use super::*;

pub(super) enum Finished {
    SignedIn { session_cookie: String },
    Linked,
}

/// A refused callback: the stable code, and the name the provider reported for a person signed
/// in there with the wrong account, so they can tell which account it was.
pub(super) struct Refusal {
    pub(super) code: &'static str,
    pub(super) name: Option<String>,
}

pub(super) async fn finish(
    state: &AppState,
    client: std::net::IpAddr,
    headers: &HeaderMap,
    query: &OidcCallbackQuery,
    flow: Option<Flow>,
) -> Result<Finished, Refusal> {
    let action = match flow.as_ref().map(|flow| &flow.purpose) {
        Some(FlowPurpose::Link { .. }) => rd_core::AuditAction::IdentityLinked,
        _ => rd_core::AuditAction::LoginFailed,
    };
    let deny = |stage: &'static str, code: &'static str| refuse(state, client, action, stage, code);

    if let rd_authn::Decision::Locked { .. } = state.auth.throttle_check(client).await {
        return Err(deny("locked_out", "auth.too_many_attempts").await);
    }
    // Cancelled at the provider: a decision, not an attack. Nothing recorded, nothing counted.
    if query.error.as_deref() == Some("access_denied") {
        return Err(Refusal {
            code: "auth.oidc_cancelled",
            name: None,
        });
    }
    if query.error.is_some() {
        return Err(deny("oidc_provider", "auth.oidc_provider_error").await);
    }
    let Some(flow) = flow else {
        return Err(deny("oidc_state", "auth.oidc_state_invalid").await);
    };
    // Every `Cookie` header: HTTP/2 lets a browser send its cookies as several.
    let binding = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(oidc::binding_from_cookies);
    if !flow.bound_to(binding) {
        return Err(deny("oidc_state", "auth.oidc_state_invalid").await);
    }
    let config = match oidc_client::config(state).await {
        Ok(Some(config)) if config.issuer == flow.issuer && config.client_id == flow.client_id => {
            config
        }
        _ => return Err(deny("oidc_state", "auth.oidc_state_invalid").await),
    };
    if query
        .iss
        .as_deref()
        .is_some_and(|issuer| issuer != config.issuer)
    {
        return Err(deny("oidc_token", "auth.oidc_issuer_mismatch").await);
    }
    let Some(code) = query.code.as_deref().filter(|code| !code.is_empty()) else {
        return Err(deny("oidc_provider", "auth.oidc_provider_error").await);
    };
    let Some(redirect_uri) = oidc_client::redirect_uri(state).await else {
        return Err(deny("oidc_state", "auth.oidc_requires_external_url").await);
    };

    let identity = match redeem_and_verify(state, &config, &flow, code, &redirect_uri).await {
        Ok(identity) => identity,
        Err(failure) => {
            let reason = match failure {
                ProviderFailure::Token(error) => error.reason(),
                _ => "",
            };
            tracing::warn!(
                stage = failure.stage(),
                code = failure.code(),
                reason,
                "a sign-in through the identity provider was refused"
            );
            return Err(deny(failure.stage(), failure.code()).await);
        }
    };
    let label = identity.label();
    if let Some((claim, value)) = config.group()
        && !identity.has_group(claim, value)
    {
        return Err(not_the_administrator(deny("oidc_account", "").await, label));
    }

    match flow.purpose {
        FlowPurpose::Link { session } => {
            let linked = oidc_client::LinkedIdentity {
                issuer: config.issuer.clone(),
                client_id: config.client_id.clone(),
                subject: identity.subject,
                label,
                linked_at: chrono::Utc::now(),
            };
            let stored = oidc_client::store(state, oidc_client::IDENTITY_SETTING, Some(&linked))
                .await
                .and(
                    // A new binding needs its own proof before the password can go (D3).
                    oidc_client::store::<String>(state, oidc_client::PROVEN_SESSION_SETTING, None)
                        .await,
                );
            if stored.is_err() {
                return Err(Refusal {
                    code: "auth.oidc_store_failed",
                    name: None,
                });
            }
            crate::audit::record(
                state,
                crate::audit::AuditEvent::success(rd_core::AuditAction::IdentityLinked)
                    .actor(crate::audit::Actor::session(session))
                    .client(client)
                    .target("identity_provider", &config.issuer)
                    .named(config.display_name.clone()),
            )
            .await;
            Ok(Finished::Linked)
        }
        FlowPurpose::SignIn => {
            let bound = oidc_client::identity(state, &config).await.ok().flatten();
            if bound.is_none_or(|bound| bound.subject != identity.subject) {
                return Err(not_the_administrator(deny("oidc_account", "").await, label));
            }
            open_session(state, client, headers).await
        }
    }
}

/// Any other identity than the bound one (O-WHO): refused, with the name the provider reported,
/// so a person signed in there with the wrong account can tell which one it was.
pub(super) fn not_the_administrator(refusal: Refusal, label: Option<String>) -> Refusal {
    Refusal {
        code: "auth.oidc_not_administrator",
        name: label.or(refusal.name),
    }
}

pub(super) async fn redeem_and_verify(
    state: &AppState,
    config: &ProviderConfig,
    flow: &Flow,
    code: &str,
    redirect_uri: &str,
) -> Result<rd_authn::oidc::IdentityClaims, ProviderFailure> {
    let (metadata, algorithms) = state.oidc.metadata(state, &config.issuer).await?;
    let secret = state
        .secrets
        .get(&config.secret_ref)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "the identity provider's client secret is not readable");
            ProviderFailure::Unreadable
        })?;
    let token = state
        .oidc
        .redeem(
            state,
            &metadata,
            &config.client_id,
            secret.expose_secret(),
            code,
            &flow.verifier,
            redirect_uri,
        )
        .await?;
    let expect = oidc::Expectations {
        issuer: &config.issuer,
        client_id: &config.client_id,
        nonce: &flow.nonce,
        flow_started_at: flow.started_at,
        now: chrono::Utc::now().timestamp(),
        algorithms: &algorithms,
    };
    state.oidc.verify(state, &metadata, &token, &expect).await
}

pub(super) async fn open_session(
    state: &AppState,
    client: std::net::IpAddr,
    headers: &HeaderMap,
) -> Result<Finished, Refusal> {
    let failed = || Refusal {
        code: "auth.session_create_failed",
        name: None,
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .and_then(rd_core::truncate_user_agent);
    let opened = state
        .auth
        .open_session(state, client, user_agent)
        .await
        .map_err(|_| failed())?;
    // The session that may switch the password sign-in off (D3): the latest one the provider
    // opened. Best effort — failing a sign-in over it would cost more than it protects.
    if let Err(error) = oidc_client::store(
        state,
        oidc_client::PROVEN_SESSION_SETTING,
        Some(&opened.id.to_string()),
    )
    .await
    {
        tracing::warn!(error = %error.message(), "could not record the provider's sign-in");
    }
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::LoginSucceeded)
            .actor(crate::audit::Actor::session(opened.id.to_string()))
            .client(client)
            .target("session", opened.id)
            .detail("method", "oidc"),
    )
    .await;
    let proxy = state.proxy.read().await;
    Ok(Finished::SignedIn {
        session_cookie: crate::AuthService::cookie(
            &opened.token,
            proxy.cookie_is_secure(),
            proxy.base_path(),
            opened.max_age_seconds,
        ),
    })
}

/// One refused callback: counted by the sign-in limiter and recorded by the stage that refused
/// it — never with a code, a token, a nonce, a state or the subject of the refused identity.
pub(super) async fn refuse(
    state: &AppState,
    client: std::net::IpAddr,
    action: rd_core::AuditAction,
    stage: &'static str,
    code: &'static str,
) -> Refusal {
    state.auth.note_failed_login(client).await;
    crate::audit::record(
        state,
        crate::audit::AuditEvent::failure(action)
            .actor(crate::audit::Actor::anonymous())
            .client(client)
            .detail("stage", stage),
    )
    .await;
    Refusal { code, name: None }
}
