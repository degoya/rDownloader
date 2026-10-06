//! Signing in through an identity provider (RD-190-15, ADR 0021): the start that sends the browser
//! to the provider, and the callback the provider sends it back to.
//!
//! Both are public, as the password sign-in is, and both are browser navigations rather than API
//! calls: they answer with redirects, and a refusal goes back to the page the flow started from
//! with a stable code in `oidc_error`, never with the provider's own text.
//!
//! The callback is where every rule of the ADR is applied, in this order: the flow is taken by its
//! `state` **before anything else** (spent whatever follows, O-STEAL), the `rd_oidc` cookie must be
//! the one the flow was started with (O-CSRF), the configuration must still be the one it was
//! started at, RFC 9207's `iss` must be the issuer when it is sent (O-ISS), the code is redeemed
//! with the PKCE verifier and the client secret, the ID token is verified
//! (`rd_authn::oidc_token`), and the identity must be the bound `(issuer, client_id, sub)` — a group
//! claim, when one is configured, only narrows (O-WHO). The tokens are dropped; what comes out is
//! an ordinary session, opened by `open_session` like a password sign-in.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use rd_api_core::oidc_client::{self, ProviderConfig, ProviderFailure};
use rd_authn::oidc::{self, Flow, FlowPurpose};
use secrecy::ExposeSecret;
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{ApiError, AppState};

mod callback;

use callback::*;

/// Where a link started from *Settings → Security* comes back to.
pub(crate) const SECURITY_PAGE: &str = "/settings/security";

/// Who a link flow names when it was started on this machine with the login switched off for
/// it: there is no session then.
pub(crate) const THIS_MACHINE: &str = "this_machine";

#[derive(Debug, Deserialize, IntoParams)]
pub struct OidcStartQuery {
    /// Where to land after signing in, inside this application. Anything else lands on `/`.
    #[serde(default)]
    pub return_to: Option<String>,
}

/// No `Debug`: the code is a credential until it is redeemed.
#[derive(Deserialize, IntoParams)]
pub struct OidcCallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// RFC 9207: the issuer of the answer, compared when the provider sends it.
    #[serde(default)]
    pub iss: Option<String>,
    /// The provider's refusal, if it refused. Only its kind is read, never its description.
    #[serde(default)]
    pub error: Option<String>,
}

/// Starts signing in through the provider: files a flow and sends the browser there.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/start",
    tag = "auth",
    params(OidcStartQuery),
    responses(
        (status = 302, description = "To the provider's authorization endpoint, with the `rd_oidc` cookie"),
        (status = 303, description = "Back to the sign-in screen with `oidc_error` set to a stable code"),
    )
)]
pub async fn oidc_start(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    Query(query): Query<OidcStartQuery>,
) -> Response {
    let base = state.proxy.read().await.base_path().to_owned();
    let return_to = oidc::safe_return_path(query.return_to.as_deref());
    match begin(&state, client.0, FlowPurpose::SignIn, &return_to).await {
        Ok(started) => {
            let mut response = redirect(StatusCode::FOUND, &started.authorization_url, &base);
            append_cookie(&mut response, &started.cookie);
            response
        }
        Err(refusal) => redirect(
            StatusCode::SEE_OTHER,
            &with_query(&format!("{base}{return_to}"), "oidc_error", refusal.code()),
            &base,
        ),
    }
}

/// A flow filed and the way to the provider.
pub(crate) struct Started {
    pub authorization_url: String,
    /// The `Set-Cookie` value binding the flow to this browser.
    pub cookie: String,
}

/// Files a flow for `purpose` and builds the authorization request.
///
/// Metered like the other sign-ins: the start is public and allocates server state for an
/// anonymous caller, so an address the limiter has locked out cannot keep starting flows.
pub(crate) async fn begin(
    state: &AppState,
    client: std::net::IpAddr,
    purpose: FlowPurpose,
    return_to: &str,
) -> Result<Started, ApiError> {
    let _attempt = state.auth.gate(client).await?;
    let Some(config) = oidc_client::config(state).await? else {
        return Err(not_configured());
    };
    if purpose == FlowPurpose::SignIn && oidc_client::identity(state, &config).await?.is_none() {
        return Err(ApiError::conflict(
            "auth.oidc_not_linked",
            "No identity at the provider is linked to the administrator yet",
        ));
    }
    let Some(redirect_uri) = oidc_client::redirect_uri(state).await else {
        return Err(requires_external_url());
    };
    let (metadata, _) = state
        .oidc
        .metadata(state, &config.issuer)
        .await
        .map_err(provider_error)?;

    let (flow, binding) = Flow::start(
        purpose,
        &config.issuer,
        &config.client_id,
        Some(return_to),
        chrono::Utc::now().timestamp(),
    );
    let request = flow.clone();
    let flow_state = state.oidc.flows.insert(client, flow);
    let Some(authorization_url) = oidc::authorization_url(
        &metadata,
        &config.client_id,
        &redirect_uri,
        &request,
        &flow_state,
        config.group().map(|(claim, _)| claim),
    ) else {
        state.oidc.flows.take(&flow_state);
        return Err(provider_error(ProviderFailure::Unreadable));
    };
    let proxy = state.proxy.read().await;
    Ok(Started {
        authorization_url,
        cookie: oidc::binding_cookie(&binding, proxy.cookie_is_secure(), proxy.base_path()),
    })
}

/// Where the provider sends the browser back. Finishes the sign-in or the link.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/callback",
    tag = "auth",
    params(OidcCallbackQuery),
    responses(
        (status = 303, description = "Signed in (with the session cookie) or linked, or back to where the flow started with `oidc_error` set to a stable code"),
    )
)]
pub async fn oidc_callback(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
    Query(query): Query<OidcCallbackQuery>,
) -> Response {
    // Spent before anything else is looked at, so a state answers once whatever follows.
    let flow = query
        .state
        .as_deref()
        .and_then(|value| state.oidc.flows.take(value));
    let (base, cleared) = {
        let proxy = state.proxy.read().await;
        (
            proxy.base_path().to_owned(),
            oidc::expired_binding_cookie(proxy.base_path()),
        )
    };
    let return_to = flow
        .as_ref()
        .map_or_else(|| "/".to_owned(), |flow| flow.return_to.clone());
    let landing = format!("{base}{return_to}");
    let mut response = match finish(&state, client.0, &headers, &query, flow).await {
        Ok(Finished::SignedIn { session_cookie }) => {
            let mut response = redirect(StatusCode::SEE_OTHER, &landing, &base);
            append_cookie(&mut response, &session_cookie);
            response
        }
        Ok(Finished::Linked) => redirect(
            StatusCode::SEE_OTHER,
            &with_query(&landing, "oidc", "linked"),
            &base,
        ),
        Err(refusal) => {
            let mut location = with_query(&landing, "oidc_error", refusal.code);
            if let Some(name) = refusal.name.as_deref() {
                location = with_query(&location, "oidc_name", name);
            }
            redirect(StatusCode::SEE_OTHER, &location, &base)
        }
    };
    append_cookie(&mut response, &cleared);
    response
}

/// The provider's display name when the sign-in screen may offer it: configured, an identity
/// bound, and an external URL to come back to.
pub(crate) async fn offered_provider(state: &AppState) -> Result<Option<String>, ApiError> {
    let Some(config) = oidc_client::config(state).await? else {
        return Ok(None);
    };
    if oidc_client::identity(state, &config).await?.is_none()
        || oidc_client::redirect_uri(state).await.is_none()
    {
        return Ok(None);
    }
    Ok(Some(config.display_name))
}

/// Where to send the browser to sign out at the provider too, when that is switched on (D5):
/// its `end_session_endpoint` with the client and the address to come back to. No
/// `id_token_hint` — it would mean keeping the ID token — so some providers ask first.
pub(crate) async fn provider_logout_url(state: &AppState) -> Option<String> {
    let config = oidc_client::config(state)
        .await
        .ok()
        .flatten()
        .filter(|config| config.provider_logout)?;
    let back = {
        let proxy = state.proxy.read().await;
        format!("{}{}/", proxy.origin()?, proxy.base_path())
    };
    let (metadata, _) = state.oidc.metadata(state, &config.issuer).await.ok()?;
    let mut url = url::Url::parse(
        metadata
            .additional_metadata()
            .end_session_endpoint
            .as_deref()?,
    )
    .ok()?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("post_logout_redirect_uri", &back);
    Some(url.into())
}

pub(crate) fn not_configured() -> ApiError {
    ApiError::conflict(
        "auth.oidc_not_configured",
        "No identity provider is configured",
    )
}

pub(crate) fn requires_external_url() -> ApiError {
    ApiError::conflict(
        "auth.oidc_requires_external_url",
        "Signing in through an identity provider needs the external URL to be set",
    )
}

/// A provider that could not be used, as an API refusal.
pub(crate) fn provider_error(failure: ProviderFailure) -> ApiError {
    let message = "The identity provider could not be used";
    match failure {
        ProviderFailure::Discovery(_) => ApiError::unprocessable(failure.code(), message),
        _ => ApiError::bad_gateway(failure.code(), message),
    }
}

/// A redirect to `location`. A location that is no header value — a return path with
/// characters a header cannot carry — lands on the application's root instead.
fn redirect(status: StatusCode, location: &str, base: &str) -> Response {
    let value = HeaderValue::from_str(location).unwrap_or_else(|_| {
        HeaderValue::from_str(&format!("{base}/")).unwrap_or(HeaderValue::from_static("/"))
    });
    let mut response = status.into_response();
    response.headers_mut().insert(header::LOCATION, value);
    response
}

fn append_cookie(response: &mut Response, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn with_query(location: &str, key: &str, value: &str) -> String {
    let separator = if location.contains('?') { '&' } else { '?' };
    let value: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
    format!("{location}{separator}{key}={value}")
}

#[cfg(test)]
mod tests {
    use super::with_query;

    #[test]
    fn a_code_is_added_to_the_return_path_whatever_query_it_has() {
        assert_eq!(
            with_query("/", "oidc_error", "auth.oidc_state_invalid"),
            "/?oidc_error=auth.oidc_state_invalid"
        );
        assert_eq!(
            with_query("/dl/downloads?view=1", "oidc_name", "Jane Doe"),
            "/dl/downloads?view=1&oidc_name=Jane+Doe"
        );
    }
}
