//! The `Host` a request names, checked before anything routes on it (security review
//! 2026-09-28, finding 3).
//!
//! A DNS rebinding page reaches this service under the page's own name. The decision which
//! names pass is `rd_authn::host`; this is the plumbing that asks it for every request — the
//! API, the compatibility surfaces, MCP and the web interface's own files alike, because the
//! page an attacker loads under their name would otherwise be this service's interface.

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{ApiError, AppState, dto::SettingsResponse};

/// The proxy contract a settings document describes, the allowed host list included.
///
/// The one place the four fields become a [`rd_authn::ProxyConfig`], so the validation on save,
/// the live apply and the start cannot build it three different ways.
pub fn proxy_config(
    settings: &SettingsResponse,
) -> Result<rd_authn::ProxyConfig, rd_authn::ProxyConfigError> {
    rd_authn::ProxyConfig::parse(
        &settings.trusted_proxies,
        settings.external_url.as_deref(),
        settings.cookie_security,
    )?
    .with_allowed_hosts(&settings.allowed_hosts)
}

/// Applies the stored proxy contract before the listener opens.
///
/// Until now only a saved settings document set it, so after a restart the service knew no
/// external URL, no trusted proxy and no mount point until somebody saved again. With the host
/// check that gap would refuse the external URL's own name, so it is closed here.
pub async fn load(state: &AppState) {
    match crate::settings_store::stored_settings(&state.database).await {
        Ok(settings) => match proxy_config(&settings) {
            Ok(config) => *state.proxy.write().await = config,
            Err(error) => {
                tracing::warn!(%error, "the stored proxy configuration is not usable");
            }
        },
        Err(error) => {
            tracing::warn!(code = %error.code(), "the stored settings could not be read");
        }
    }
}

/// Refuses a request whose `Host` or URI authority names this service by a name it does not
/// answer to.
///
/// A request without either passes: every browser sends `Host`, so its absence means a client
/// that is not a browser, and a rebinding attack needs one. The refusal names the host, so
/// whoever typed it can add it to the list.
pub async fn require_known_host(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // Owned before anything is awaited: a `&Request` held across the lock below would make
    // this middleware's future non-`Send` (see `auth::RequestFacts`).
    let mut named = Vec::with_capacity(2);
    if let Some(authority) = request.uri().authority() {
        named.push(authority.host().to_owned());
    }
    if let Some(value) = request.headers().get(header::HOST) {
        // A value that is not text cannot be a name this service answers to.
        named.push(value.to_str().unwrap_or_default().to_owned());
    }
    let refused = {
        let proxy = state.proxy.read().await;
        named.into_iter().find(|host| !proxy.host_allowed(host))
    };
    let Some(host) = refused else {
        return next.run(request).await;
    };
    tracing::warn!(
        %host,
        "a request named a host this service does not answer to; add it to the allowed hosts \
         if it is yours"
    );
    ApiError::forbidden(
        "request.host_not_allowed",
        "This service does not answer to this host name; open it by its address or add the \
         name to the allowed hosts in the settings",
    )
    .with_param("host", host)
    .into_response()
}
