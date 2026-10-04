//! The service's half of signing in through an identity provider (RD-190-15, ADR 0021): what is
//! configured and which identity is bound to the administrator, the flows between their start and
//! their callback, and the three requests to the provider — discovery, key set, token endpoint.
//! The requests are `openidconnect`'s, sent through [`send`], the workspace's reqwest under this
//! module's limits; the rules the answers have to meet are `rd_authn::oidc`'s; the routes are in
//! `rd-api-access`.
//!
//! **Nothing the provider issues is kept or logged.** The authorization code is redeemed once, the
//! ID token is verified and dropped, no access or refresh token is asked for or stored (D5), and a
//! failure is logged by the step that failed, never with a body. The client secret lives in the
//! secret store, its reference in the configuration.
//!
//! Requests go out directly — never through a download proxy profile — with the system's roots
//! plus the installation's custom CA material, because a home-lab provider often sits behind a
//! private CA. No redirect is followed, every answer is read up to
//! [`oidc::RESPONSE_LIMIT_BYTES`], and every request ends after [`oidc::REQUEST_TIMEOUT`].

use std::{sync::Arc, time::Instant};

use openidconnect::{
    AuthType, AuthorizationCode, ClientId, ClientSecret, HttpRequest, HttpResponse, IssuerUrl,
    PkceCodeVerifier, RedirectUrl, RequestTokenError, TokenResponse,
    core::{CoreClient, CoreJsonWebKeySet},
};
use rd_authn::oidc::{
    self, Algorithm, DiscoveryError, Expectations, IdentityClaims, JsonWebKeySet, ProviderMetadata,
    TokenError,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{
    ApiError, AppState,
    input_checks::{BodyError, read_bounded_body},
};

/// The provider's configuration (`ProviderConfig`), JSON.
pub const CONFIG_SETTING: &str = "auth.oidc.config";
/// The identity bound to the administrator (`LinkedIdentity`), JSON.
pub const IDENTITY_SETTING: &str = "auth.oidc.identity";
/// The session the latest sign-in through the provider opened. Switching the password sign-in
/// off is only possible from that session: proof that the round trip works (D3).
pub const PROVEN_SESSION_SETTING: &str = "auth.oidc.proven_session";
/// `true` while the password form is switched off (D3). Read at every sign-in rather than held
/// in memory, so `rdownloader auth password-login on` reaches a stopped service as well.
pub const PASSWORD_LOGIN_OFF_SETTING: &str = "auth.password_login_disabled";

/// One provider, as configured in *Settings → Security*.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderConfig {
    pub issuer: String,
    pub client_id: String,
    /// What the sign-in button calls the provider.
    pub display_name: String,
    /// A claim that must also hold `group_value` (D2): it narrows, it never names.
    #[serde(default)]
    pub group_claim: Option<String>,
    #[serde(default)]
    pub group_value: Option<String>,
    /// Sign out at the provider too (D5), off unless chosen.
    #[serde(default)]
    pub provider_logout: bool,
    /// Where the client secret is in the secret store.
    pub secret_ref: String,
}

impl ProviderConfig {
    /// The group condition, when both halves are set.
    #[must_use]
    pub fn group(&self) -> Option<(&str, &str)> {
        self.group_claim.as_deref().zip(self.group_value.as_deref())
    }
}

/// The identity that is the administrator: `(issuer, client_id, sub)`, proven by a round trip
/// (D2). `sub` is unique per issuer only, and with pairwise identifiers per client, so all three
/// are the binding and a change of either of the first two ends it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LinkedIdentity {
    pub issuer: String,
    pub client_id: String,
    pub subject: String,
    /// What the provider called the person when the identity was linked. Shown, never compared.
    #[serde(default)]
    pub label: Option<String>,
    pub linked_at: chrono::DateTime<chrono::Utc>,
}

impl LinkedIdentity {
    /// Whether this binding belongs to `config`'s provider and client.
    #[must_use]
    pub fn belongs_to(&self, config: &ProviderConfig) -> bool {
        self.issuer == config.issuer && self.client_id == config.client_id
    }
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    state: &AppState,
    key: &str,
) -> Result<Option<T>, ApiError> {
    Ok(state
        .database
        .get_setting(key)
        .await?
        .filter(|value| !value.is_null())
        .and_then(|value| match serde_json::from_value(value) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                tracing::warn!(%error, setting = key, "a stored identity provider setting is not readable");
                None
            }
        }))
}

/// The configured provider, if there is one.
pub async fn config(state: &AppState) -> Result<Option<ProviderConfig>, ApiError> {
    read_json(state, CONFIG_SETTING).await
}

/// The bound identity, if there is one and it belongs to the configured provider.
pub async fn identity(
    state: &AppState,
    config: &ProviderConfig,
) -> Result<Option<LinkedIdentity>, ApiError> {
    Ok(read_json::<LinkedIdentity>(state, IDENTITY_SETTING)
        .await?
        .filter(|identity| identity.belongs_to(config)))
}

/// Writes or clears (`None`) one of the settings above.
pub async fn store<T: Serialize>(
    state: &AppState,
    key: &str,
    value: Option<&T>,
) -> Result<(), ApiError> {
    let value = match value {
        Some(value) => serde_json::to_value(value).map_err(|error| {
            tracing::error!(%error, setting = key, "could not encode an identity provider setting");
            ApiError::bad_request("auth.oidc_store_failed", "The setting could not be stored")
        })?,
        None => serde_json::Value::Null,
    };
    state.database.set_setting(key.to_owned(), value).await?;
    Ok(())
}

/// Whether the password form is switched off.
pub async fn password_login_off(state: &AppState) -> Result<bool, ApiError> {
    Ok(state
        .database
        .get_setting(PASSWORD_LOGIN_OFF_SETTING)
        .await?
        .and_then(|value| value.as_bool())
        .unwrap_or(false))
}

/// The session the latest provider sign-in opened.
pub async fn proven_session(state: &AppState) -> Result<Option<String>, ApiError> {
    Ok(state
        .database
        .get_setting(PROVEN_SESSION_SETTING)
        .await?
        .and_then(|value| value.as_str().map(str::to_owned)))
}

/// Where the provider sends the browser back: the external URL and the callback path, and
/// nothing taken from the request — no `Host`, no `X-Forwarded-*` (O-REDIR). `None` without an
/// external URL, which is why the provider cannot be switched on without one.
pub async fn redirect_uri(state: &AppState) -> Option<String> {
    let proxy = state.proxy.read().await;
    proxy
        .origin()
        .map(|origin| format!("{origin}{}{}", proxy.base_path(), oidc::CALLBACK_PATH))
}

/// Why a request to the provider did not give a usable answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderFailure {
    /// No answer: refused connection, TLS failure, timeout.
    Unreachable,
    /// An answer that is not a usable document: a status other than success, too large, not
    /// the JSON expected.
    Unreadable,
    /// The token endpoint refused the code.
    Refused,
    /// The discovery document breaks a rule.
    Discovery(DiscoveryError),
    /// The ID token breaks a rule.
    Token(TokenError),
}

impl ProviderFailure {
    /// The stable error code a refusal carries.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unreachable => "auth.oidc_provider_unreachable",
            Self::Unreadable => "auth.oidc_provider_unreadable",
            Self::Refused => "auth.oidc_token_refused",
            Self::Discovery(error) => error.code(),
            Self::Token(_) => "auth.oidc_token_invalid",
        }
    }

    /// The audit stage a refused callback is recorded under (ADR 0021, *Audit*).
    #[must_use]
    pub const fn stage(self) -> &'static str {
        match self {
            Self::Unreachable | Self::Unreadable | Self::Discovery(_) => "oidc_provider",
            Self::Refused | Self::Token(_) => "oidc_token",
        }
    }
}

struct Cached<T> {
    /// The issuer or key set address the value was fetched for.
    source: String,
    value: T,
    fetched: Instant,
}

/// The flows in flight and what was last fetched from the provider.
#[derive(Clone)]
pub struct OidcClient {
    /// Flows between start and callback, keyed by `state`. In memory like the passkey ceremonies,
    /// bounded per address and in total (O-FLOOD), each answerable once for ten minutes.
    pub flows: Arc<rd_authn::CeremonyStore<oidc::Flow>>,
    metadata: Arc<RwLock<Option<Cached<ProviderMetadata>>>>,
    keys: Arc<RwLock<Option<Cached<JsonWebKeySet>>>>,
    refetch: Arc<oidc::RefetchGate>,
}

impl Default for OidcClient {
    fn default() -> Self {
        Self {
            flows: Arc::new(rd_authn::CeremonyStore::with_ttl(oidc::FLOW_TTL)),
            metadata: Arc::default(),
            keys: Arc::default(),
            refetch: Arc::default(),
        }
    }
}

impl OidcClient {
    /// Fetches and checks the discovery document and its key set now, whatever is cached: what
    /// configuring a provider does, so a provider that cannot be used is refused before it is
    /// saved.
    ///
    /// # Errors
    ///
    /// When the provider cannot be reached or its document breaks a rule.
    pub async fn discover(
        &self,
        state: &AppState,
        issuer: &str,
    ) -> Result<(ProviderMetadata, Vec<Algorithm>), ProviderFailure> {
        let issuer_url = IssuerUrl::new(issuer.to_owned())
            .map_err(|_| ProviderFailure::Discovery(DiscoveryError::InsecureEndpoint))?;
        let client = http_client(state).await?;
        let http = |request: HttpRequest| send(client.clone(), request);
        // The library reads the document, requires its issuer to be the configured one byte for
        // byte, and fetches the key set it names.
        let metadata = ProviderMetadata::discover_async(issuer_url, &http)
            .await
            .map_err(discovery_failure)?;
        let algorithms =
            oidc::check_provider(&metadata, issuer).map_err(ProviderFailure::Discovery)?;
        let now = Instant::now();
        *self.keys.write().await = Some(Cached {
            source: metadata.jwks_uri().as_str().to_owned(),
            value: metadata.jwks().clone(),
            fetched: now,
        });
        *self.metadata.write().await = Some(Cached {
            source: issuer.to_owned(),
            value: metadata.clone(),
            fetched: now,
        });
        Ok((metadata, algorithms))
    }

    /// The discovery document, from the cache while it is younger than a day. A failed fetch
    /// keeps the last good copy of the same issuer, so a provider that is briefly away does not
    /// end a sign-in that its cached keys could still verify.
    ///
    /// # Errors
    ///
    /// When nothing usable is cached and the fetch fails.
    pub async fn metadata(
        &self,
        state: &AppState,
        issuer: &str,
    ) -> Result<(ProviderMetadata, Vec<Algorithm>), ProviderFailure> {
        let cached = self.cached_metadata(issuer, true).await;
        if let Some(cached) = cached {
            return Ok(cached);
        }
        match self.discover(state, issuer).await {
            Ok(fresh) => Ok(fresh),
            Err(failure) => self.cached_metadata(issuer, false).await.ok_or(failure),
        }
    }

    async fn cached_metadata(
        &self,
        issuer: &str,
        fresh_only: bool,
    ) -> Option<(ProviderMetadata, Vec<Algorithm>)> {
        let cached = self.metadata.read().await;
        let cached = cached.as_ref().filter(|cached| {
            cached.source == issuer
                && (!fresh_only || cached.fetched.elapsed() < oidc::METADATA_MAX_AGE)
        })?;
        let algorithms = oidc::check_provider(&cached.value, issuer).ok()?;
        Some((cached.value.clone(), algorithms))
    }

    /// Forgets what was fetched, when the provider changes or is removed.
    pub async fn forget(&self) {
        *self.metadata.write().await = None;
        *self.keys.write().await = None;
    }

    /// Verifies an ID token against the provider's keys. A token naming a key the cached set
    /// does not have fetches the set again — once, and no more often than the refetch gate
    /// allows (O-JWKS) — so a rotation at the provider needs no restart here.
    ///
    /// # Errors
    ///
    /// When the keys cannot be fetched or the token breaks a rule.
    pub async fn verify(
        &self,
        state: &AppState,
        metadata: &ProviderMetadata,
        token: &str,
        expect: &Expectations<'_>,
    ) -> Result<IdentityClaims, ProviderFailure> {
        let keys = self.keys(state, metadata, false).await?;
        match oidc::verify_id_token(token, &keys, expect) {
            Err(TokenError::UnknownKey) if self.refetch.allow(Instant::now()) => {
                let keys = self.keys(state, metadata, true).await?;
                oidc::verify_id_token(token, &keys, expect).map_err(ProviderFailure::Token)
            }
            verified => verified.map_err(ProviderFailure::Token),
        }
    }

    async fn keys(
        &self,
        state: &AppState,
        metadata: &ProviderMetadata,
        refresh: bool,
    ) -> Result<JsonWebKeySet, ProviderFailure> {
        let source = metadata.jwks_uri().as_str().to_owned();
        if !refresh {
            let cached = self.keys.read().await;
            if let Some(cached) = cached.as_ref().filter(|cached| {
                cached.source == source && cached.fetched.elapsed() < oidc::METADATA_MAX_AGE
            }) {
                return Ok(cached.value.clone());
            }
        }
        let client = http_client(state).await?;
        let http = |request: HttpRequest| send(client.clone(), request);
        let keys = CoreJsonWebKeySet::fetch_async(metadata.jwks_uri(), &http)
            .await
            .map_err(discovery_failure)?;
        *self.keys.write().await = Some(Cached {
            source,
            value: keys.clone(),
            fetched: Instant::now(),
        });
        Ok(keys)
    }

    /// Redeems an authorization code with the PKCE verifier and the client credentials — the
    /// library's code exchange — and returns the ID token as it arrived. Everything else in the
    /// answer — access token, refresh token — is dropped unread.
    ///
    /// # Errors
    ///
    /// When the provider cannot be reached, refuses the code, or answers without an ID token.
    #[allow(clippy::too_many_arguments)]
    pub async fn redeem(
        &self,
        state: &AppState,
        metadata: &ProviderMetadata,
        client_id: &str,
        client_secret: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, ProviderFailure> {
        let redirect =
            RedirectUrl::new(redirect_uri.to_owned()).map_err(|_| ProviderFailure::Unreadable)?;
        // RFC 6749 §2.3.1: Basic unless the provider takes only the form; the library encodes
        // both halves as the RFC asks.
        let auth_type = match oidc::client_authentication(metadata) {
            oidc::ClientAuthentication::Basic => AuthType::BasicAuth,
            oidc::ClientAuthentication::Post => AuthType::RequestBody,
        };
        let client = CoreClient::from_provider_metadata(
            metadata.clone(),
            ClientId::new(client_id.to_owned()),
            Some(ClientSecret::new(client_secret.to_owned())),
        )
        .set_redirect_uri(redirect)
        .set_auth_type(auth_type);
        let http_client = http_client(state).await?;
        let http = |request: HttpRequest| send(http_client.clone(), request);
        let answer = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|_| ProviderFailure::Discovery(DiscoveryError::InsecureEndpoint))?
            .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_owned()))
            .request_async(&http)
            .await
            .map_err(|error| match error {
                RequestTokenError::ServerResponse(_) => {
                    tracing::warn!("the identity provider refused the authorization code");
                    ProviderFailure::Refused
                }
                RequestTokenError::Request(SendError::Refused) => {
                    ProviderFailure::Discovery(DiscoveryError::InsecureEndpoint)
                }
                RequestTokenError::Request(_) => ProviderFailure::Unreachable,
                _ => ProviderFailure::Unreadable,
            })?;
        answer
            .id_token()
            .map(ToString::to_string)
            .ok_or(ProviderFailure::Unreadable)
    }
}

/// A discovery or key-set failure of the library, as the provider failure it is.
fn discovery_failure(error: openidconnect::DiscoveryError<SendError>) -> ProviderFailure {
    match error {
        openidconnect::DiscoveryError::Validation(_) => {
            ProviderFailure::Discovery(DiscoveryError::IssuerMismatch)
        }
        openidconnect::DiscoveryError::Request(SendError::Refused) => {
            ProviderFailure::Discovery(DiscoveryError::InsecureEndpoint)
        }
        openidconnect::DiscoveryError::Request(_) => ProviderFailure::Unreachable,
        _ => ProviderFailure::Unreadable,
    }
}

/// Why [`send`] gave no answer. Never carries a body or an address.
#[derive(Debug)]
pub enum SendError {
    /// The address is neither `https` nor on this machine.
    Refused,
    /// No answer: refused connection, TLS failure, timeout.
    Unreachable,
    /// An answer larger than [`oidc::RESPONSE_LIMIT_BYTES`].
    TooLarge,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Refused => "the address is neither https nor on this machine",
            Self::Unreachable => "the identity provider did not answer",
            Self::TooLarge => "the identity provider's answer is too large",
        })
    }
}

impl std::error::Error for SendError {}

/// A client for one exchange with the provider: no proxy, no redirect, a timeout, and the custom
/// CA material of the installation beside the system's roots.
async fn http_client(state: &AppState) -> Result<reqwest::Client, ProviderFailure> {
    let custom_ca_pem = state
        .scheduler
        .network_defaults()
        .read()
        .await
        .custom_ca_pem
        .clone();
    let mut builder = reqwest::Client::builder()
        .timeout(oidc::REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    for pem in &custom_ca_pem {
        match reqwest::Certificate::from_pem(pem) {
            Ok(certificate) => builder = builder.add_root_certificate(certificate),
            Err(error) => {
                tracing::warn!(%error, "ignoring an unparsable custom CA for the identity provider");
            }
        }
    }
    builder.build().map_err(|error| {
        tracing::warn!(%error, "could not build the identity provider client");
        ProviderFailure::Unreachable
    })
}

/// The library's HTTP requests, sent with the workspace's reqwest: only to an allowed endpoint,
/// and an answer read up to [`oidc::RESPONSE_LIMIT_BYTES`] — a larger one is refused, not cut.
pub async fn send(
    client: reqwest::Client,
    request: HttpRequest,
) -> Result<HttpResponse, SendError> {
    let (parts, body) = request.into_parts();
    let url = parts.uri.to_string();
    if !oidc::endpoint_allowed(&url) {
        return Err(SendError::Refused);
    }
    let response = client
        .request(parts.method, url)
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .map_err(|_| SendError::Unreachable)?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = read_bounded_body(response, oidc::RESPONSE_LIMIT_BYTES)
        .await
        .map_err(|error| match error {
            BodyError::TooLarge => SendError::TooLarge,
            BodyError::Interrupted(_) => SendError::Unreachable,
        })?;
    if !status.is_success() {
        tracing::warn!(%status, "the identity provider answered with a failure");
    }
    let mut answer = HttpResponse::new(body);
    *answer.status_mut() = status;
    *answer.headers_mut() = headers;
    Ok(answer)
}
