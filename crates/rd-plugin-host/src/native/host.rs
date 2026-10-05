use std::{sync::Arc, time::Duration};

/// Resolver plugins authenticate through provider accounts, which carry their own scoped
/// cookie jar and secrets. Domain auth profiles deliberately do not apply here; they are a
/// transfer-time credential, so a profile shows up on the download but not during resolve.
const NO_AUTH_PROFILE: rd_core::AuthProfileSelection = rd_core::AuthProfileSelection::None;

use rd_core::{Failure, FailureKind};
use rd_http::{ClientPool, SharedNetworkDefaults};
use rd_plugin_api::{CaptchaSolver, HostHttpResponse, ResolvedHeader};
use url::Url;

use super::expand::{http_failure, transient, validate_redirect};
// The impls in `resolver_host.rs` and `requests.rs` reach these as `super::name`, the path they
// had when they lived in this file.
use super::{account_credentials, account_username, permanent};

mod requests;
mod resolver_host;

/// Largest answer a request made outside a plugin invocation may bring; inside one, what the
/// manifest still allows decides (PLUG-06).
const DEFAULT_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// How long the server may take to send its headers once the request body is out.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one request may take as a whole, the answer's body included (RD-191-06, PLUG-03).
/// The client has only a connect timeout, so a server that sent its headers and then trickled
/// the body held a plugin's call open without end.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);
/// The slowest upload a request body is still given time for: 64 KiB/s, half a megabit.
///
/// Sending the body happens before the server can answer, so it used to count against
/// [`SEND_TIMEOUT`]: a WebDAV `PUT` over 15 s of upload failed whatever the line (RA-HOST-04).
const MIN_UPLOAD_BYTES_PER_SECOND: u64 = 64 * 1024;
/// Most time a request body is given, whatever its size: half an hour, which a body the
/// plugin's own memory can hold (64 MiB for the bundled storage plugin) never needs at
/// [`MIN_UPLOAD_BYTES_PER_SECOND`].
const MAX_UPLOAD_TIME: Duration = Duration::from_secs(30 * 60);
/// Longest token lifetime the host records from an `expires-in` (RA-HOST-02).
///
/// The value is the plugin's — or the provider's through it — and `now + u64::MAX` seconds is
/// a panic in `chrono`, not a date. A year is longer than any provider's access token lives,
/// and a renewal a year early costs one request.
const MAX_TOKEN_LIFETIME_SECONDS: u64 = 365 * 24 * 60 * 60;

tokio::task_local! {
    /// Response bytes the running invocation's manifest still allows (PLUG-06).
    static RESPONSE_ALLOWANCE: usize;
    /// Whether the running invocation may reach the person's own network (RA-HOST-01).
    static OWN_NETWORK: bool;
}

/// Runs one plugin request with the address rule of its invocation: the person's own network
/// only when `allowed` — the invocation's reach is an address they supplied themselves.
///
/// A task-local, like [`with_response_allowance`], so the request type the plugins share stays
/// unchanged.
pub(crate) async fn with_own_network<F: std::future::Future>(
    allowed: bool,
    request: F,
) -> F::Output {
    OWN_NETWORK.scope(allowed, request).await
}

/// Which addresses a plugin request to `url` may reach (RA-HOST-01, owner 2026-10-04):
/// [`crate::OwnEndpoints::policy`], with the person's own network where the invocation says
/// they entered the address. Before, a manifest that listed `localhost` reached the service's
/// own API from this machine, which the API trusts. A request made outside any invocation gets
/// the public rule.
fn address_policy(own: &crate::OwnEndpoints, url: &Url) -> rd_http::AddressPolicy {
    own.policy(OWN_NETWORK.try_with(|own| *own).unwrap_or(false), url)
}

/// Refuses a target before anything is sent, where the guarded resolver cannot judge it.
///
/// A name is resolved by the client's [`rd_http::GuardedResolver`], which hands the connector
/// only addresses the rule permits — resolved once, so the checked address is the dialled one
/// and a rebinding name gains nothing. Two cases never reach it: a literal address, and any
/// target behind a proxy, which resolves the name itself. Both are judged here; a name this
/// machine cannot resolve is left to the proxy, which may know it.
async fn check_reach(
    policy: &rd_http::AddressPolicy,
    url: &Url,
    proxied: bool,
    own: &crate::OwnEndpoints,
) -> Result<(), Failure> {
    if rd_http::literal_address(url).is_none() && !proxied {
        return Ok(());
    }
    match rd_http::check_target(policy, &rd_http::SystemLookup, url).await {
        Ok(_) | Err(rd_http::TargetRefusal::Unresolved(_)) => Ok(()),
        Err(rd_http::TargetRefusal::Refused(_)) => Err(own.refused(url)),
    }
}

/// A failed send, with a refusal of the address rule told apart from a network error.
fn send_failure(error: reqwest::Error, own: &crate::OwnEndpoints, url: &Url) -> Failure {
    if rd_http::is_refusal(&error) {
        return own.refused(url);
    }
    http_failure(error)
}

/// How much longer than the head timeout a request with a body of `body_bytes` is given:
/// the time the body takes at [`MIN_UPLOAD_BYTES_PER_SECOND`], at most [`MAX_UPLOAD_TIME`].
fn upload_allowance(body_bytes: usize) -> Duration {
    let bytes = u64::try_from(body_bytes).unwrap_or(u64::MAX);
    Duration::from_secs(bytes.div_ceil(MIN_UPLOAD_BYTES_PER_SECOND)).min(MAX_UPLOAD_TIME)
}

/// When a token that lives `seconds` from `now` expires, its lifetime held to
/// [`MAX_TOKEN_LIFETIME_SECONDS`] so the sum cannot overflow (RA-HOST-02).
fn token_expiry(now: chrono::DateTime<chrono::Utc>, seconds: u64) -> chrono::DateTime<chrono::Utc> {
    let seconds = i64::try_from(seconds.min(MAX_TOKEN_LIFETIME_SECONDS)).unwrap_or(i64::MAX);
    now.checked_add_signed(chrono::Duration::seconds(seconds))
        .unwrap_or(now)
}

/// Runs one plugin request with what is left of its invocation's response budget.
///
/// The budget lives in the store and is counted after the answer is back; the reading has to
/// know it before, or a fixed cap decides instead of the manifest — a hard 8 MiB that left
/// MEGA's 16 MiB listings failing whatever the manifest said (RD-191-06, PLUG-06). A task-local,
/// like `rd_http::with_redirect_gate`, so the request type the plugins share stays unchanged.
pub(crate) async fn with_response_allowance<F: std::future::Future>(
    bytes: u64,
    request: F,
) -> F::Output {
    RESPONSE_ALLOWANCE
        .scope(usize::try_from(bytes).unwrap_or(usize::MAX), request)
        .await
}

/// The most one answer may bring: the invocation's allowance, never past the manifest ceiling.
fn response_limit() -> usize {
    let ceiling = usize::try_from(crate::manifest::MAX_RESPONSE_BYTES).unwrap_or(usize::MAX);
    RESPONSE_ALLOWANCE
        .try_with(|left| *left)
        .map_or(DEFAULT_RESPONSE_BYTES, |left| left.min(ceiling))
}
/// Longest single countdown a resolver may ask for. Real free-download timers are under two
/// minutes; anything beyond this is a parse error or a limit that belongs in `ip-blocked`.
const MAX_SINGLE_WAIT: Duration = Duration::from_secs(10 * 60);

pub(super) struct NativeHost {
    pub(super) database: rd_db::Database,
    clients: ClientPool,
    pub(super) secrets: rd_secrets::SecretStore,
    network_defaults: SharedNetworkDefaults,
    captcha: Option<Arc<dyn CaptchaSolver>>,
    /// The service's own listeners, which no request reaches (RA-HOST-01).
    own: crate::OwnEndpoints,
}

impl NativeHost {
    pub(super) fn new(
        database: rd_db::Database,
        clients: ClientPool,
        secrets: rd_secrets::SecretStore,
        network_defaults: SharedNetworkDefaults,
        captcha: Option<Arc<dyn CaptchaSolver>>,
    ) -> Self {
        Self {
            database,
            clients,
            secrets,
            network_defaults,
            captcha,
            own: crate::OwnEndpoints::default(),
        }
    }

    /// The same host, refusing `own` as the service's own listeners.
    #[must_use]
    pub(super) fn with_own_endpoints(mut self, own: crate::OwnEndpoints) -> Self {
        self.own = own;
        self
    }
}

/// The two time limits of one exchange.
#[derive(Clone, Copy, Debug)]
struct Limits {
    /// Until the answer's headers are in: [`SEND_TIMEOUT`] plus the request body's time.
    head: Duration,
    /// For the whole exchange: [`EXCHANGE_TIMEOUT`] plus the request body's time.
    whole: Duration,
}

/// Sends one plugin request and reads its answer in full: at most `limit` bytes of it, the
/// headers within `limits.head` and all of it within `limits.whole`.
async fn exchange(
    builder: reqwest::RequestBuilder,
    source: &Url,
    carries_credential: bool,
    limit: usize,
    limits: Limits,
    own: &crate::OwnEndpoints,
) -> Result<HostHttpResponse, Failure> {
    let exchange = async {
        let mut response = tokio::time::timeout(limits.head, builder.send())
            .await
            .map_err(|_| transient("plugin.http_timeout", "Resolver HTTP request timed out"))?
            .map_err(|error| send_failure(error, own, source))?;
        let status = response.status().as_u16();
        let final_url = response.url().clone();
        validate_redirect(source, &final_url, carries_credential)?;
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value.to_str().ok().map(|value| ResolvedHeader {
                    name: name.as_str().to_owned(),
                    value: value.to_owned(),
                })
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(http_failure)? {
            if body.len().saturating_add(chunk.len()) > limit {
                return Err(Failure::coded(
                    FailureKind::Permanent,
                    "plugin.response_too_large",
                    "Resolver response exceeds the size limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HostHttpResponse {
            status,
            final_url,
            headers,
            body,
        })
    };
    tokio::time::timeout(limits.whole, exchange)
        .await
        .map_err(|_| transient("plugin.http_timeout", "Resolver HTTP request timed out"))?
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod host_tests;

#[cfg(test)]
#[path = "exchange_tests.rs"]
mod exchange_tests;

#[cfg(test)]
#[path = "host_basic_tests.rs"]
mod host_basic_tests;

#[cfg(test)]
#[path = "parts_tests.rs"]
mod parts_tests;

#[cfg(test)]
#[path = "address_rule_tests.rs"]
mod address_rule_tests;
