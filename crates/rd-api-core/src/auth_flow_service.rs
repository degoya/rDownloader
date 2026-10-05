//! Provider authentication flows: starting them, and keeping them going (RD-090-13).
//!
//! The loop here is what makes a flow survive a restart. The plugin answers "ask me again in
//! `n` seconds"; the waiting is the host's, recorded in the database, so nothing is lost when
//! the service stops in the middle of a sign-in and the interface only ever reads state.

use std::{
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use rd_core::{AccountId, AuthFlow, AuthFlowState, FailureKind};
use rd_plugin_ext::ProviderError;
use rd_plugin_host::extension::{AuthProgress, TokenOutcome};
use tokio_util::sync::CancellationToken;

mod oauth;
mod providers;
mod store;
mod sweep;

/// How often due flows are swept. A device flow's own interval is usually five seconds, so
/// checking a little more often than that costs nothing and keeps the answer prompt.
const SWEEP: Duration = Duration::from_secs(3);

/// Shortest wait a plugin can ask for. A provider that says "poll immediately" would otherwise
/// have this service in a loop against it, which is how an application gets rate-limited.
const MIN_INTERVAL: i64 = 2;

/// How far ahead of expiry a token is renewed. A token that dies between the check and the
/// request it was fetched for is a failure the person sees, and a minute of lead buys the gap.
const REFRESH_LEAD: i64 = 60;

/// How long a renewal waits after a provider refused to finish it at once. Deliberately far
/// longer than the sign-in interval: nobody is sitting in front of a renewal, so there is
/// nothing to make prompt, and asking again in three seconds only earns a rate limit.
const REFRESH_BACKOFF: i64 = 300;

/// How long a sign-in waits after a poll did not come back. Longer than the interval a
/// plugin asks for, because the answer to a provider that is not answering is not to ask it
/// faster, and short enough that a sign-in still finishes once the provider is back.
const SIGN_IN_BACKOFF: u64 = 30;

/// How long a sign-in whose provider named no window is retried before it is given up on.
///
/// A flow that carries an expiry is bounded by it, and most do. One that does not would
/// otherwise be polled for the rest of the process's life -- the same "repeat it forever"
/// this job exists to remove, one sweep over.
const SIGN_IN_GRACE: i64 = 900;

/// Recorded on a sign-in no installed plugin can run.
///
/// A stable code rather than a sentence: the interface translates it into the reader's
/// language, and unlike a provider's refusal there is no foreign answer here to quote --
/// the host decided this on its own, from what is installed.
const AUTH_PLUGIN_MISSING: &str = "account.auth_plugin_missing";

/// Recorded on a sign-in given up on because the provider kept not answering.
const AUTH_PROVIDER_UNREACHABLE: &str = "account.auth_provider_unreachable";

/// Recorded on a renewal no installed plugin can run.
const RENEWAL_PLUGIN_MISSING: &str = "account.renewal_plugin_missing";

/// Recorded on a flow whose plugin does not serve the way in it was asked for (RD-106-01).
///
/// As terminal as a missing plugin and told apart from it on purpose: a plugin *is* installed
/// and claims the provider, and what the person can do about it is different -- this one asks
/// for a plugin that offers the other entrance, not for any plugin at all.
const AUTH_FLOW_UNSUPPORTED: &str = "account.auth_flow_unsupported";

/// The longest first wait a provider's stated device interval can buy.
///
/// The interval is honoured rather than ignored -- a provider that asked to be left alone for
/// five seconds is -- but a provider that asked for an hour before the first check would make
/// a sign-in look broken, so the wait is bounded here.
const MAX_FIRST_DEVICE_INTERVAL: i64 = 60;

struct Inner {
    database: rd_db::Database,
    plugins: rd_plugin_host::PluginInstaller,
    plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    /// Behind a lock once loaded, so a first install can join the set the service runs with
    /// (RD-170-12); a reader takes the set as it is and never waits for more than that.
    providers: tokio::sync::OnceCell<RwLock<Arc<rd_plugin_ext::AuthProviders>>>,
    oauth: tokio::sync::OnceCell<RwLock<Arc<rd_plugin_ext::OAuthProviders>>>,
    shutdown: CancellationToken,
}

/// Cloneable handle of the authentication flow service.
#[derive(Clone)]
pub struct AuthFlowService {
    inner: Arc<Inner>,
}

impl AuthFlowService {
    #[must_use]
    pub fn start(
        database: rd_db::Database,
        plugins: rd_plugin_host::PluginInstaller,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    ) -> Self {
        let service = Self {
            inner: Arc::new(Inner {
                database,
                plugins,
                plugin_host,
                providers: tokio::sync::OnceCell::new(),
                oauth: tokio::sync::OnceCell::new(),
                shutdown: CancellationToken::new(),
            }),
        };
        tokio::spawn(service.clone().sweep_loop());
        service
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// A service with no sweep loop of its own, for tests that drive one sweep by hand.
    ///
    /// The plugin root it is given holds nothing, which is the point: every provider lookup
    /// then ends in `ProviderError::NoPlugin`, the case this service used to repeat forever.
    #[cfg(test)]
    fn detached(
        database: rd_db::Database,
        plugin_root: std::path::PathBuf,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                database,
                plugins: rd_plugin_host::PluginInstaller::new(
                    plugin_root,
                    rd_plugin_host::PluginVerifier::new(false),
                ),
                plugin_host,
                providers: tokio::sync::OnceCell::new(),
                oauth: tokio::sync::OnceCell::new(),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    /// Starts a flow for one account, replacing whatever it had.
    pub async fn begin(
        &self,
        account_id: AccountId,
        provider_slug: &str,
    ) -> anyhow::Result<AuthFlow> {
        let providers = self.providers().await;
        let plugin_id = providers
            .plugin_id(provider_slug)
            .ok_or_else(|| anyhow::anyhow!("no authentication plugin claims {provider_slug}"))?;
        let progress = providers.begin(provider_slug, account_id, None).await?;
        self.store(account_id, &plugin_id, progress).await
    }

    /// The flow of one account, if it has one.
    pub async fn flow(&self, account_id: AccountId) -> anyhow::Result<Option<AuthFlow>> {
        self.inner.database.auth_flow(account_id).await
    }

    /// Cancels a flow. The provider is not told: a device flow expires on its own, and there
    /// is no call in the contract that would say so anyway.
    pub async fn cancel(&self, account_id: AccountId) -> anyhow::Result<()> {
        self.inner.database.delete_auth_flow(account_id).await
    }
}

/// A token outcome as the flow store records it.
///
/// One place since RD-106-01, because three callers now produce one: the redirect callback,
/// the device poll, and the tests that stand in for either.
fn token_progress(outcome: TokenOutcome) -> AuthProgress {
    match outcome {
        TokenOutcome::Authorized => AuthProgress::Authorized,
        TokenOutcome::Pending {
            retry_after_seconds,
        } => AuthProgress::Pending {
            retry_after_seconds,
        },
        // The category is dropped here and only here: a sign-in the person is watching ends
        // either way, and what it shows them is the message. The renewal sweep, which does
        // act on the category, reads the outcome itself rather than this.
        TokenOutcome::Failed { message, .. } => AuthProgress::Failed { message },
    }
}

/// What the renewal sweep does with one row once the provider call came back.
#[derive(Clone, Debug, Eq, PartialEq)]
enum RenewalAction {
    /// Nothing to record: the host already wrote the new expiry and reference from inside its
    /// own `store-oauth-token` call, before the plugin ever returned.
    Settle,
    /// Ask again in this many seconds. The row keeps its token, which is the whole point.
    Defer(i64),
    /// Terminal, with the stable code or the plugin's message to record.
    Fail(String),
}

/// What a renewal's answer means for the row it was made for (RD-106-02).
///
/// Pulled out of the sweep because this decision, not the plumbing around it, is what went
/// wrong: a missing plugin arrived in the same arm as an unreachable provider, so a renewal
/// that could never succeed was retried every five minutes for the life of the process.
/// Separated here, and testable without a provider, a plugin or a database.
fn renewal_action(outcome: Result<TokenOutcome, ProviderError>) -> RenewalAction {
    match outcome {
        Ok(TokenOutcome::Authorized) => RenewalAction::Settle,
        Ok(TokenOutcome::Pending {
            retry_after_seconds,
        }) => RenewalAction::Defer(
            i64::try_from(rd_core::clamp_retry_after(retry_after_seconds))
                .unwrap_or(REFRESH_BACKOFF)
                .max(MIN_INTERVAL),
        ),
        Ok(TokenOutcome::Failed { category, message }) => match retry_after(&category) {
            // The plugin reached the provider and the provider was busy, rate-limiting or
            // away. That is about the moment, not about the credential, so the token stays.
            Some(seconds) => RenewalAction::Defer(seconds),
            // A refusal of the credential itself will not change by asking again sooner, so
            // the person is told rather than the request repeated.
            None => RenewalAction::Fail(message),
        },
        Err(ProviderError::NoPlugin { provider_slug }) => {
            // Waiting installs nothing. Said once, as a failure, instead of every five
            // minutes as a warning nobody reads.
            tracing::warn!(
                provider = %provider_slug,
                "no installed plugin can renew this provider's token"
            );
            RenewalAction::Fail(RENEWAL_PLUGIN_MISSING.to_owned())
        }
        Err(ProviderError::UnsupportedFlow {
            provider_slug,
            flow,
        }) => {
            // Not reachable through `refresh`, which both ways in share and neither gates.
            // Handled all the same, and terminally: it is a fact about the installed
            // manifest, so the next five minutes hold nothing new.
            tracing::warn!(
                provider = %provider_slug,
                flow,
                "the installed plugin does not serve this way in"
            );
            RenewalAction::Fail(AUTH_FLOW_UNSUPPORTED.to_owned())
        }
        Err(ProviderError::Failed(error)) => {
            // A call that did not complete says nothing about the credential -- an unreachable
            // provider is the ordinary case here -- so the flow keeps its token and is tried
            // again later rather than being failed outright.
            tracing::warn!(%error, "a token renewal did not complete");
            RenewalAction::Defer(REFRESH_BACKOFF)
        }
    }
}

/// `seconds` after `now`, held to [`rd_core::MAX_RETRY_AFTER_SECONDS`] (RA-HOST-02).
///
/// Every such number here is a plugin's — a sign-in window, an "ask again in" — and
/// `now + u64::MAX` seconds is a panic in `chrono`, not a date. A sign-in window or a poll
/// interval longer than a day is no provider's.
fn seconds_after(now: DateTime<Utc>, seconds: u64) -> DateTime<Utc> {
    let seconds = i64::try_from(rd_core::clamp_retry_after(seconds)).unwrap_or(MIN_INTERVAL);
    now + chrono::Duration::seconds(seconds)
}

/// How long a renewal waits after a plugin reported this kind of failure, or `None` when
/// waiting cannot help.
///
/// The category used to be dropped on the way out of the host, which left the sweep unable to
/// tell "the provider is offline" from "the provider refused this credential" -- and it failed
/// the flow for both.
fn retry_after(category: &FailureKind) -> Option<i64> {
    let seconds = match category {
        FailureKind::Transient {
            retry_after_seconds,
        }
        | FailureKind::RateLimited {
            retry_after_seconds,
        }
        | FailureKind::IpBlocked {
            retry_after_seconds,
        } => *retry_after_seconds,
        FailureKind::Offline => None,
        // The credential, the account or the plugin is the problem, and the next attempt is
        // the same attempt.
        FailureKind::Permanent
        | FailureKind::AuthRequired
        | FailureKind::AccountInvalid
        | FailureKind::NeedsCaptcha
        | FailureKind::Unsupported
        | FailureKind::CaptchaFailed => return None,
    };
    Some(
        seconds
            .and_then(|seconds| i64::try_from(rd_core::clamp_retry_after(seconds)).ok())
            .unwrap_or(REFRESH_BACKOFF)
            .max(MIN_INTERVAL),
    )
}

/// What a failed callback exchange records, in the words the sweep uses for the same failures.
fn callback_failure(error: &ProviderError) -> String {
    match error {
        ProviderError::NoPlugin { .. } => AUTH_PLUGIN_MISSING.to_owned(),
        ProviderError::UnsupportedFlow { .. } => AUTH_FLOW_UNSUPPORTED.to_owned(),
        ProviderError::Failed(_) => AUTH_PROVIDER_UNREACHABLE.to_owned(),
    }
}

/// What a polled sign-in reports, with a missing plugin told apart from a failed call.
///
/// The mirror image of the renewal bug (RD-106-02): here *every* error ended the flow, so a
/// provider that was briefly unreachable killed a sign-in that would have succeeded a minute
/// later. Only the missing plugin is terminal; a failed call is waited out, bounded by the
/// flow's own window or, when the provider named none, by `SIGN_IN_GRACE`.
fn sign_in_progress(
    result: Result<AuthProgress, ProviderError>,
    flow: &AuthFlow,
    now: DateTime<Utc>,
) -> AuthProgress {
    match result {
        Ok(progress) => progress,
        Err(ProviderError::NoPlugin { provider_slug }) => {
            tracing::warn!(
                provider = %provider_slug,
                "no installed plugin can sign in to this provider"
            );
            AuthProgress::Failed {
                message: AUTH_PLUGIN_MISSING.to_owned(),
            }
        }
        Err(ProviderError::UnsupportedFlow {
            provider_slug,
            flow,
        }) => {
            // Terminal like a missing plugin, and for the same kind of reason: the manifest
            // says which ways in it serves, and polling again reads the same manifest.
            tracing::warn!(
                provider = %provider_slug,
                flow,
                "the installed plugin does not serve the way in this sign-in used"
            );
            AuthProgress::Failed {
                message: AUTH_FLOW_UNSUPPORTED.to_owned(),
            }
        }
        Err(ProviderError::Failed(error)) => {
            tracing::warn!(%error, "a sign-in poll did not complete");
            if flow.expires_at.is_none()
                && now - flow.started_at >= chrono::Duration::seconds(SIGN_IN_GRACE)
            {
                return AuthProgress::Failed {
                    message: AUTH_PROVIDER_UNREACHABLE.to_owned(),
                };
            }
            // Everything the person is looking at survives this: `Pending` keeps the address,
            // the code and the window, and only moves the next poll out.
            AuthProgress::Pending {
                retry_after_seconds: SIGN_IN_BACKOFF,
            }
        }
    }
}

#[cfg(test)]
#[path = "auth_flow_service_tests.rs"]
mod tests;

/// Puts `client_id` where a plugin left the marker, or refuses.
///
/// Apart from the lookup so the decision can be driven without a database: a URL with no marker
/// is handed back untouched, a marker with no client registered is the one refusal, and the
/// value is form-encoded because it lands in a query string.
fn substitute_client_id(url: &str, client_id: Option<&str>) -> Result<String, ClientNotConfigured> {
    if !url.contains(rd_plugin_host::CLIENT_ID_MARKER) {
        return Ok(url.to_owned());
    }
    let client_id = client_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ClientNotConfigured)?;
    let encoded: String = url::form_urlencoded::byte_serialize(client_id.as_bytes()).collect();
    Ok(url.replace(rd_plugin_host::CLIENT_ID_MARKER, &encoded))
}

/// This installation has registered no OAuth client for a provider that needs one
/// (RD-106-04).
///
/// A type of its own rather than a message, so the handler can tell it apart from everything
/// else a sign-in can fail with. The difference is not cosmetic: every other failure here is
/// about the provider — it refused, it was unreachable, it answered something unreadable — and
/// is reported as a bad gateway with whatever the plugin said. This one is about *this*
/// installation's configuration, nothing is wrong at the provider, and the person is one form
/// field away from fixing it. Reported as a bad request under `oauth.client_not_configured`,
/// which the interface translates into the four languages with the steps in them.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ClientNotConfigured;

impl std::fmt::Display for ClientNotConfigured {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "this provider needs an OAuth client of its own; register one and enter its client \
             ID on the account",
        )
    }
}

impl std::error::Error for ClientNotConfigured {}
