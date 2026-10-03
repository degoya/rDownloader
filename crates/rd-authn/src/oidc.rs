//! Signing the administrator in through an OpenID Connect provider (RD-190-15, ADR 0021): the
//! decisions, testable without a server or a provider.
//!
//! rDownloader is a confidential relying party of one provider and uses the authorization code
//! flow with PKCE (S256). The flow lives in memory between its two requests — the start that
//! sends the browser to the provider and the callback the provider sends it back to — keyed by
//! its `state` and bound to the browser that started it by the `rd_oidc` cookie: a callback URL
//! that leaks (a proxy log, the history) or that a stranger's page sends somebody's browser to is
//! refused, because only the starting browser holds the binding, and the state is spent on first
//! use. The protocol itself — discovery, the key set, PKCE, the authorization request and the ID
//! token — is `openidconnect`'s; this module keeps what the library does not decide: which
//! provider is usable ([`check_provider`]), the flow and its browser binding, the return path and
//! the refetch gate. The ID token's verification is configured in [`crate::oidc_token`].
//!
//! What is *not* here: who the administrator is (`(issuer, client_id, sub)`, stored by the
//! service), and fetching anything — the HTTP half lives with the service state.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AdditionalProviderMetadata, AuthenticationFlow, ClientId, CsrfToken, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope,
    core::{
        CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod,
        CoreGrantType, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm,
        CoreJweKeyManagementAlgorithm, CoreResponseMode, CoreResponseType,
        CoreSubjectIdentifierType,
    },
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use crate::oidc_token::{
    Algorithm, CLOCK_LEEWAY_SECONDS, Expectations, IdentityClaims, JsonWebKeySet, TokenError,
    verify_id_token,
};

/// How long a started sign-in stays answerable. Twice a passkey's: somebody may first have to
/// type a password and a second factor at the provider.
pub const FLOW_TTL: Duration = Duration::from_secs(600);

/// How rarely a token naming an unknown key may make the service fetch the key set again. A
/// forged token costs the provider at most one request per interval, however many arrive.
pub const JWKS_REFETCH_INTERVAL: Duration = Duration::from_secs(300);

/// How long a discovery document and a key set are used before they are fetched again.
pub const METADATA_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The largest answer read from a provider; discovery, key set and token response are a few
/// kilobytes each.
pub const RESPONSE_LIMIT_BYTES: usize = 256 * 1024;

/// How long a request to the provider may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The cookie that binds a flow to the browser that started it.
pub const BINDING_COOKIE: &str = "rd_oidc";

/// Where the provider sends the browser back, below the mount point.
pub const CALLBACK_PATH: &str = "/api/v1/auth/oidc/callback";

/// The path the binding cookie is scoped to, below the mount point: it travels to the start, the
/// link and the callback and to nothing else.
pub const COOKIE_PATH: &str = "/api/v1/auth/oidc/";

/// Whether an address may be an issuer or an endpoint of one: `https`, or `http` to this
/// machine (a provider started beside the service for a test, as for a provider sign-in link).
/// Never with credentials or a fragment.
#[must_use]
pub fn endpoint_allowed(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => match url.host() {
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
            None => false,
        },
        _ => false,
    }
}

/// Whether `issuer` can be configured: an allowed endpoint without a query (OIDC Discovery §2).
#[must_use]
pub fn issuer_allowed(issuer: &str) -> bool {
    endpoint_allowed(issuer) && url::Url::parse(issuer).is_ok_and(|url| url.query().is_none())
}

/// The members of a discovery document `openidconnect` leaves to the application: PKCE's methods
/// (RFC 8414) and the end-session endpoint (RP-initiated logout).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExtraMetadata {
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
}

impl AdditionalProviderMetadata for ExtraMetadata {}

/// A provider's discovery document, read by `openidconnect` (which also requires its issuer to be
/// the configured one, byte for byte) and with the key set it fetched beside it.
pub type ProviderMetadata = openidconnect::ProviderMetadata<
    ExtraMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

/// Why a provider cannot be used. Refused when it is configured, not at the first sign-in.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DiscoveryError {
    #[error("the discovery document names another issuer than the one configured")]
    IssuerMismatch,
    #[error("an endpoint of the provider is neither https nor on this machine")]
    InsecureEndpoint,
    #[error("the provider does not offer PKCE with S256")]
    NoPkce,
    #[error("the provider signs ID tokens with no algorithm this service accepts")]
    NoAlgorithm,
}

impl DiscoveryError {
    /// The stable error code a refusal carries.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::IssuerMismatch => "auth.oidc_issuer_mismatch",
            Self::InsecureEndpoint => "auth.oidc_insecure_endpoint",
            Self::NoPkce => "auth.oidc_no_pkce",
            Self::NoAlgorithm => "auth.oidc_no_algorithm",
        }
    }
}

/// How the client authenticates at the token endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientAuthentication {
    /// `client_secret_basic`, the default of RFC 6749 and of OIDC.
    Basic,
    /// `client_secret_post`, for a provider that lists only that.
    Post,
}

/// Checks a discovery document against the configured issuer and returns the algorithms an ID
/// token may then use.
///
/// # Errors
///
/// The first rule the document breaks.
pub fn check_provider(
    metadata: &ProviderMetadata,
    issuer: &str,
) -> Result<Vec<Algorithm>, DiscoveryError> {
    // Byte for byte (OIDC Discovery §4.3). The library compared it on discovery already; a
    // document from the cache is held to it again.
    if metadata.issuer().as_str() != issuer {
        return Err(DiscoveryError::IssuerMismatch);
    }
    let endpoints = [
        Some(metadata.authorization_endpoint().as_str()),
        metadata.token_endpoint().map(|endpoint| endpoint.as_str()),
        Some(metadata.jwks_uri().as_str()),
        metadata
            .additional_metadata()
            .end_session_endpoint
            .as_deref(),
    ];
    if metadata.token_endpoint().is_none()
        || endpoints
            .into_iter()
            .flatten()
            .any(|endpoint| !endpoint_allowed(endpoint))
    {
        return Err(DiscoveryError::InsecureEndpoint);
    }
    if !metadata
        .additional_metadata()
        .code_challenge_methods_supported
        .iter()
        .any(|method| method == "S256")
    {
        return Err(DiscoveryError::NoPkce);
    }
    let algorithms = algorithms(metadata);
    if algorithms.is_empty() {
        return Err(DiscoveryError::NoAlgorithm);
    }
    Ok(algorithms)
}

/// [`Algorithm::ALLOWED`] intersected with the algorithms the provider says it signs with.
#[must_use]
pub fn algorithms(metadata: &ProviderMetadata) -> Vec<Algorithm> {
    Algorithm::ALLOWED
        .into_iter()
        .filter(|algorithm| {
            metadata
                .id_token_signing_alg_values_supported()
                .contains(&algorithm.core())
        })
        .collect()
}

/// How to present the client secret: Basic unless the provider lists only the form post.
#[must_use]
pub fn client_authentication(metadata: &ProviderMetadata) -> ClientAuthentication {
    let listed = |method: CoreClientAuthMethod| {
        metadata
            .token_endpoint_auth_methods_supported()
            .is_some_and(|methods| methods.contains(&method))
    };
    if !listed(CoreClientAuthMethod::ClientSecretBasic)
        && listed(CoreClientAuthMethod::ClientSecretPost)
    {
        ClientAuthentication::Post
    } else {
        ClientAuthentication::Basic
    }
}

/// The authorization request a flow sends the browser to, built by the library from the
/// provider's document: the code flow, `openid profile` (and the group claim's name as a scope
/// when one is configured), the client, the redirect URI, `state`, the nonce and the S256
/// challenge of the flow's verifier. `None` when the redirect URI is not a URL.
#[must_use]
pub fn authorization_url(
    metadata: &ProviderMetadata,
    client_id: &str,
    redirect_uri: &str,
    flow: &Flow,
    state: &str,
    group_claim: Option<&str>,
) -> Option<String> {
    let redirect = RedirectUrl::new(redirect_uri.to_owned()).ok()?;
    let client = CoreClient::from_provider_metadata(
        metadata.clone(),
        ClientId::new(client_id.to_owned()),
        None,
    )
    .set_redirect_uri(redirect);
    let state = state.to_owned();
    let nonce = flow.nonce.clone();
    let challenge =
        PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(flow.verifier.clone()));
    let mut request = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            move || CsrfToken::new(state),
            move || Nonce::new(nonce),
        )
        .add_scope(Scope::new("profile".to_owned()))
        .set_pkce_challenge(challenge);
    if let Some(claim) = group_claim {
        request = request.add_scope(Scope::new(claim.to_owned()));
    }
    let (url, _, _) = request.url();
    Some(url.into())
}

/// What a flow was started for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowPurpose {
    /// Signing in.
    SignIn,
    /// Binding the identity that comes back to the administrator (D2), started by this session
    /// after the password was typed again.
    Link { session: String },
}

/// One sign-in between its start and its callback. No `Debug`: the nonce and the verifier must not
/// reach a log by way of a `{:?}`.
#[derive(Clone)]
pub struct Flow {
    pub purpose: FlowPurpose,
    /// The provider it was started at. A callback after the configuration changed is refused.
    pub issuer: String,
    pub client_id: String,
    pub nonce: String,
    /// The PKCE verifier; the provider only ever sees its S256 challenge.
    pub verifier: String,
    /// SHA-256 of the `rd_oidc` cookie value, hex. The value itself is only in the browser.
    pub binding_digest: String,
    /// Where to send the browser afterwards, relative to the mount point ([`safe_return_path`]).
    pub return_to: String,
    /// Unix seconds.
    pub started_at: i64,
}

impl Flow {
    /// A new flow and the binding value to hand to the browser as the `rd_oidc` cookie.
    #[must_use]
    pub fn start(
        purpose: FlowPurpose,
        issuer: &str,
        client_id: &str,
        return_to: Option<&str>,
        now: i64,
    ) -> (Self, String) {
        let binding = random_value();
        let flow = Self {
            purpose,
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            nonce: Nonce::new_random().secret().clone(),
            verifier: PkceCodeChallenge::new_random_sha256().1.secret().clone(),
            binding_digest: digest_hex(&binding),
            return_to: safe_return_path(return_to),
            started_at: now,
        };
        (flow, binding)
    }

    /// Whether `cookie` is the binding this flow was started with, compared in constant time.
    #[must_use]
    pub fn bound_to(&self, cookie: Option<&str>) -> bool {
        cookie.is_some_and(|value| {
            aws_lc_rs::constant_time::verify_slices_are_equal(
                digest_hex(value).as_bytes(),
                self.binding_digest.as_bytes(),
            )
            .is_ok()
        })
    }
}

/// The S256 challenge of a PKCE verifier (RFC 7636 §4.2), as the library computes it.
#[must_use]
pub fn code_challenge(verifier: &str) -> String {
    PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(verifier.to_owned()))
        .as_str()
        .to_owned()
}

/// 32 random bytes, base64url: the browser binding.
#[must_use]
pub fn random_value() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn digest_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Where a finished sign-in may send the browser: a path inside this application, or `/`.
///
/// Stored with the flow on the server, never read back from the callback, and checked here so
/// that it can only ever be relative to the mount point — `//evil.example` (a scheme-relative
/// address), a backslash a browser reads as a slash, or a full URL fall back to `/`.
#[must_use]
pub fn safe_return_path(candidate: Option<&str>) -> String {
    candidate
        .filter(|path| {
            path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.contains("://")
                && path.len() <= 512
                && !path.chars().any(char::is_control)
        })
        .map_or_else(|| "/".to_owned(), str::to_owned)
}

/// The `Set-Cookie` value that binds a flow to this browser.
///
/// `SameSite=Lax`, unlike the session cookie: the callback is a top-level navigation from the
/// provider's site, which a `Strict` cookie does not accompany. Scoped to [`COOKIE_PATH`] and to
/// the flow's lifetime, so it reaches nothing else and outlives nothing.
#[must_use]
pub fn binding_cookie(value: &str, secure: bool, base_path: &str) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!(
        "{BINDING_COOKIE}={value}; HttpOnly; SameSite=Lax; Path={base_path}{COOKIE_PATH}{secure}; \
         Max-Age={}",
        FLOW_TTL.as_secs()
    )
}

/// The `Set-Cookie` value that clears the binding, at the path it was set at.
#[must_use]
pub fn expired_binding_cookie(base_path: &str) -> String {
    format!("{BINDING_COOKIE}=; HttpOnly; SameSite=Lax; Path={base_path}{COOKIE_PATH}; Max-Age=0")
}

/// The binding value in a `Cookie` header, if there is one.
#[must_use]
pub fn binding_from_cookies(header: &str) -> Option<&str> {
    header
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix("rd_oidc="))
        .filter(|value| !value.is_empty())
}

/// Lets a token that names an unknown key fetch the key set again at most once per
/// [`JWKS_REFETCH_INTERVAL`]. Rotation needs no restart, and a flood of forged tokens costs the
/// provider one request.
#[derive(Debug, Default)]
pub struct RefetchGate {
    last: Mutex<Option<Instant>>,
}

impl RefetchGate {
    /// Whether a refetch may happen at `now`; a `true` counts as the refetch.
    pub fn allow(&self, now: Instant) -> bool {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|at| now.saturating_duration_since(at) < JWKS_REFETCH_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}

#[cfg(test)]
#[path = "oidc_tests.rs"]
mod tests;
