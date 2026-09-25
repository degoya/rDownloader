//! Provider authentication flows: starting them, and keeping them going (RD-090-13).
//!
//! The loop here is what makes a flow survive a restart. The plugin answers "ask me again in
//! `n` seconds"; the waiting is the host's, recorded in the database, so nothing is lost when
//! the service stops in the middle of a sign-in and the interface only ever reads state.

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use rd_core::{AccountId, AuthFlow, AuthFlowState, FailureKind};
use rd_plugin_ext::ProviderError;
use rd_plugin_host::extension::{AuthProgress, TokenOutcome};
use tokio_util::sync::CancellationToken;

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
    providers: tokio::sync::OnceCell<Arc<rd_plugin_ext::AuthProviders>>,
    oauth: tokio::sync::OnceCell<Arc<rd_plugin_ext::OAuthProviders>>,
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

    /// The installed authentication providers, compiled on first use.
    pub async fn providers(&self) -> Arc<rd_plugin_ext::AuthProviders> {
        Arc::clone(
            self.inner
                .providers
                .get_or_init(|| async {
                    match rd_plugin_ext::AuthProviders::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(providers) => Arc::new(providers),
                        Err(error) => {
                            tracing::warn!(%error, "could not load authentication plugins");
                            Arc::new(rd_plugin_ext::AuthProviders::none())
                        }
                    }
                })
                .await,
        )
    }

    /// The installed OAuth providers, compiled on first use.
    pub async fn oauth_providers(&self) -> Arc<rd_plugin_ext::OAuthProviders> {
        Arc::clone(
            self.inner
                .oauth
                .get_or_init(|| async {
                    match rd_plugin_ext::OAuthProviders::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(providers) => Arc::new(providers),
                        Err(error) => {
                            tracing::warn!(%error, "could not load oauth plugins");
                            Arc::new(rd_plugin_ext::OAuthProviders::none())
                        }
                    }
                })
                .await,
        )
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

    /// Starts an OAuth sign-in for one account, replacing whatever it had.
    ///
    /// Unlike a device flow this is not polled afterwards. Nothing is asked of the provider
    /// until the person's browser comes back to the callback carrying a code, so the row is
    /// written with no next poll and the sweep leaves it alone.
    /// Puts this installation's own OAuth client into the address the person is sent to
    /// (RD-106-04).
    ///
    /// A plugin cannot do this itself, and that is deliberate rather than an oversight: a guest
    /// has no way to read a configured value — `secret-available` answers a bool and nothing
    /// else — and the one thing it may put a marker in that the host expands is an outbound
    /// request, which building an address is not. So the plugin writes `{{client_id}}` into the
    /// URL it returns and the substitution happens here, where the account already is.
    ///
    /// Only the client id, never a secret. This string is handed to a browser and stored in the
    /// flow row; a credential expanded into it would be a credential in somebody's history.
    async fn with_client_id(&self, account_id: AccountId, url: String) -> anyhow::Result<String> {
        if !url.contains(rd_plugin_host::CLIENT_ID_MARKER) {
            return Ok(url);
        }
        let client_id = self
            .inner
            .database
            .list_accounts()
            .await?
            .into_iter()
            .find(|account| account.id == account_id)
            .and_then(|account| account.username)
            .filter(|value| !value.trim().is_empty());
        // Said before the person is sent anywhere, because the alternative is a Google error
        // page about a client that does not exist and no hint of what to do about it.
        Ok(substitute_client_id(&url, client_id.as_deref())?)
    }

    pub async fn begin_oauth(
        &self,
        account_id: AccountId,
        provider_slug: &str,
    ) -> anyhow::Result<AuthFlow> {
        let providers = self.oauth_providers().await;
        let plugin_id = providers
            .plugin_id(provider_slug)
            .ok_or_else(|| anyhow::anyhow!("no oauth plugin claims {provider_slug}"))?;
        let request = providers.begin(provider_slug, account_id, None).await?;
        let authorization_url = self
            .with_client_id(account_id, request.authorization_url)
            .await?;
        let now = Utc::now();
        self.inner
            .database
            .upsert_auth_flow(rd_db::UpsertAuthFlow {
                account_id,
                plugin_id,
                state: AuthFlowState::WaitingForUser,
                verification_url: Some(authorization_url),
                user_code: None,
                expires_at: request
                    .expires_in_seconds
                    .and_then(|seconds| i64::try_from(seconds).ok())
                    .map(|seconds| now + chrono::Duration::seconds(seconds)),
                next_poll_at: None,
                message: None,
                token_expires_at: None,
                refresh_ref: None,
                access_ref: None,
                key_ref: None,
                callback_state: Some(request.state),
                flow_state: request.flow_state,
            })
            .await
    }

    /// Starts an OAuth device sign-in for one account, replacing whatever it had.
    ///
    /// The mirror of [`Self::begin_oauth`], and different in exactly one way that matters:
    /// this one *is* polled. There is no callback to wait for, so the row is written with a
    /// poll due at once (or after the interval the provider asked for) and the sweep carries
    /// it from there -- which is also what makes it survive a restart mid-sign-in.
    ///
    /// What it produces is an ordinary `AuthFlow` carrying `verification_url` and
    /// `user_code`, the same shape a device flow of the older `auth` world produces. The
    /// difference is invisible from the outside and decisive underneath: the token this ends
    /// in has an expiry and refresh material, so the renewal sweep keeps it alive and nobody
    /// is asked to type a code a second time.
    pub async fn begin_oauth_device(
        &self,
        account_id: AccountId,
        provider_slug: &str,
    ) -> anyhow::Result<AuthFlow> {
        let providers = self.oauth_providers().await;
        let plugin_id = providers
            .plugin_id(provider_slug)
            .ok_or_else(|| anyhow::anyhow!("no oauth plugin claims {provider_slug}"))?;
        let authorization = providers
            .device_begin(provider_slug, account_id, None)
            .await?;
        let verification_url = self
            .with_client_id(account_id, authorization.verification_url)
            .await?;
        let now = Utc::now();
        let first_wait = authorization
            .interval_seconds
            .and_then(|seconds| i64::try_from(seconds).ok())
            .unwrap_or(0)
            .clamp(0, MAX_FIRST_DEVICE_INTERVAL);
        self.inner
            .database
            .upsert_auth_flow(rd_db::UpsertAuthFlow {
                account_id,
                plugin_id,
                state: AuthFlowState::WaitingForUser,
                verification_url: Some(verification_url),
                user_code: authorization.user_code,
                expires_at: authorization
                    .expires_in_seconds
                    .and_then(|seconds| i64::try_from(seconds).ok())
                    .map(|seconds| now + chrono::Duration::seconds(seconds)),
                next_poll_at: Some(now + chrono::Duration::seconds(first_wait)),
                message: None,
                token_expires_at: None,
                refresh_ref: None,
                access_ref: None,
                key_ref: None,
                // No redirect, so nothing to echo back. It is also what keeps the two apart
                // in the sweep: `due_auth_flows` skips every row that carries one.
                callback_state: None,
                flow_state: authorization.flow_state,
            })
            .await
    }

    /// Finishes an OAuth sign-in from what the redirect carried.
    ///
    /// The lookup by state is the check. A callback quoting a value no flow claims matches
    /// nothing and is refused here, which is the whole reason the provider is made to echo it.
    pub async fn complete_oauth(
        &self,
        callback_state: &str,
        code: &str,
    ) -> anyhow::Result<AuthFlow> {
        let Some(flow) = self
            .inner
            .database
            .auth_flow_by_callback_state(callback_state)
            .await?
        else {
            anyhow::bail!("no sign-in is waiting for this callback");
        };
        let accounts = self.inner.database.list_accounts().await?;
        let Some(account) = accounts
            .into_iter()
            .find(|account| account.id == flow.account_id)
        else {
            anyhow::bail!("the account this sign-in belongs to is gone");
        };
        let providers = self.oauth_providers().await;
        let outcome = providers
            .poll(
                &account.provider,
                flow.account_id,
                code,
                flow.flow_state.as_deref(),
            )
            .await?;
        self.store(flow.account_id, &flow.plugin_id, token_progress(outcome))
            .await
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

    /// Writes what a plugin reported.
    async fn store(
        &self,
        account_id: AccountId,
        plugin_id: &str,
        progress: AuthProgress,
    ) -> anyhow::Result<AuthFlow> {
        let now = Utc::now();
        let input = match progress {
            AuthProgress::Authorized => {
                // The renewal columns are carried over rather than cleared. By the time a
                // plugin reports success the host has already written them, from inside its
                // `store-oauth-token` call -- so blanking the row here would throw away the
                // expiry and the refresh reference microseconds after recording them, and the
                // token would never be renewed. A device flow has nothing there to carry.
                let kept = self.flow(account_id).await.ok().flatten();
                rd_db::UpsertAuthFlow {
                    account_id,
                    plugin_id: plugin_id.to_owned(),
                    state: AuthFlowState::Authorized,
                    verification_url: None,
                    user_code: None,
                    expires_at: None,
                    next_poll_at: None,
                    message: None,
                    token_expires_at: kept.as_ref().and_then(|flow| flow.token_expires_at),
                    refresh_ref: kept.as_ref().and_then(|flow| flow.refresh_ref.clone()),
                    // Carried for the same reason the refresh reference is: an upsert that
                    // dropped it would take the access token away from a resolver mid-download
                    // (RD-106-03).
                    access_ref: kept.as_ref().and_then(|flow| flow.access_ref.clone()),
                    // The other half of the same session, carried for the same reason
                    // (RD-120-30): the sign-in stored it inside its own call, before this row.
                    key_ref: kept.and_then(|flow| flow.key_ref),
                    // Dropped once it has been answered: a state that survived its own
                    // callback would be a code somebody could present a second time.
                    callback_state: None,
                    flow_state: None,
                }
            }
            AuthProgress::UserAction {
                verification_url,
                user_code,
                expires_in_seconds,
                flow_state,
            } => rd_db::UpsertAuthFlow {
                account_id,
                plugin_id: plugin_id.to_owned(),
                state: AuthFlowState::WaitingForUser,
                verification_url: Some(verification_url),
                user_code,
                expires_at: expires_in_seconds
                    .and_then(|seconds| i64::try_from(seconds).ok())
                    .map(|seconds| now + chrono::Duration::seconds(seconds)),
                // Asked again on the next sweep: the person may confirm at once, and waiting
                // out a full interval before the first check makes a fast sign-in feel slow.
                next_poll_at: Some(now),
                message: None,
                token_expires_at: None,
                refresh_ref: None,
                access_ref: None,
                key_ref: None,
                callback_state: None,
                flow_state,
            },
            AuthProgress::Pending {
                retry_after_seconds,
            } => {
                // Everything the person is looking at — the address, the code, the window —
                // and the plugin's own bookkeeping survive a poll that says "not yet". A
                // plugin does not repeat them, and re-showing a fresh code every few seconds
                // would make a sign-in impossible to complete.
                let kept = self.flow(account_id).await.ok().flatten();
                rd_db::UpsertAuthFlow {
                    account_id,
                    plugin_id: plugin_id.to_owned(),
                    state: AuthFlowState::Polling,
                    verification_url: kept.as_ref().and_then(|flow| flow.verification_url.clone()),
                    user_code: kept.as_ref().and_then(|flow| flow.user_code.clone()),
                    expires_at: kept.as_ref().and_then(|flow| flow.expires_at),
                    next_poll_at: Some(
                        now + chrono::Duration::seconds(
                            i64::try_from(retry_after_seconds)
                                .unwrap_or(MIN_INTERVAL)
                                .max(MIN_INTERVAL),
                        ),
                    ),
                    message: None,
                    token_expires_at: kept.as_ref().and_then(|flow| flow.token_expires_at),
                    refresh_ref: kept.as_ref().and_then(|flow| flow.refresh_ref.clone()),
                    access_ref: kept.as_ref().and_then(|flow| flow.access_ref.clone()),
                    key_ref: kept.as_ref().and_then(|flow| flow.key_ref.clone()),
                    callback_state: kept.as_ref().and_then(|flow| flow.callback_state.clone()),
                    flow_state: kept.and_then(|flow| flow.flow_state),
                }
            }
            AuthProgress::Failed { message } => rd_db::UpsertAuthFlow {
                account_id,
                plugin_id: plugin_id.to_owned(),
                state: AuthFlowState::Failed,
                verification_url: None,
                user_code: None,
                expires_at: None,
                next_poll_at: None,
                message: Some(message.chars().take(500).collect()),
                token_expires_at: None,
                refresh_ref: None,
                access_ref: None,
                key_ref: None,
                callback_state: None,
                flow_state: None,
            },
        };
        self.inner.database.upsert_auth_flow(input).await
    }

    async fn sweep_loop(self) {
        let mut ticker = tokio::time::interval(SWEEP);
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }
            if let Err(error) = self.sweep().await {
                tracing::warn!(%error, "authentication flows could not be swept");
            }
        }
    }

    async fn sweep(&self) -> anyhow::Result<()> {
        let now = Utc::now();
        // Both halves run every tick, and the second one is reported separately: a provider
        // refusing to renew must not stop sign-ins from being advanced, or the other way
        // round.
        let sign_ins = self.sweep_sign_ins(now).await;
        let renewals = self.sweep_renewals(now).await;
        if let Err(error) = renewals {
            tracing::warn!(%error, "token renewals could not be swept");
        }
        sign_ins
    }

    async fn sweep_sign_ins(&self, now: DateTime<Utc>) -> anyhow::Result<()> {
        let due = self.inner.database.due_auth_flows(now).await?;
        if due.is_empty() {
            return Ok(());
        }
        let accounts = self.inner.database.list_accounts().await?;
        let providers = self.providers().await;
        // Loaded for the same sweep because a due row can belong to either world now
        // (RD-106-01). Which one it belongs to is not guessed: the row records the plugin
        // that started it, and only one plugin claims a provider in each world.
        let oauth = self.oauth_providers().await;
        for flow in due {
            if flow.is_expired(now) {
                // The provider's own window ran out. Saying so beats polling something that
                // will refuse for the rest of the day.
                let _ = self
                    .store(
                        flow.account_id,
                        &flow.plugin_id,
                        AuthProgress::Failed {
                            message: "the sign-in window expired".to_owned(),
                        },
                    )
                    .await;
                continue;
            }
            let Some(account) = accounts
                .iter()
                .find(|account| account.id == flow.account_id)
            else {
                // The account was deleted mid-flow; the row goes with it.
                let _ = self.cancel(flow.account_id).await;
                continue;
            };
            let result = if oauth.plugin_id(&account.provider).as_deref() == Some(&flow.plugin_id) {
                oauth
                    .device_poll(
                        &account.provider,
                        flow.account_id,
                        flow.flow_state.as_deref(),
                    )
                    .await
                    .map(token_progress)
            } else {
                providers
                    .poll(
                        &account.provider,
                        flow.account_id,
                        flow.flow_state.as_deref(),
                    )
                    .await
            };
            let progress = sign_in_progress(result, &flow, now);
            if let Err(error) = self.store(flow.account_id, &flow.plugin_id, progress).await {
                tracing::warn!(%error, "authentication flow could not be stored");
            }
        }
        Ok(())
    }

    /// Renews the tokens that are about to run out, without anybody being asked.
    async fn sweep_renewals(&self, now: DateTime<Utc>) -> anyhow::Result<()> {
        let threshold = now + chrono::Duration::seconds(REFRESH_LEAD);
        let due = self
            .inner
            .database
            .due_refresh_auth_flows(now, threshold)
            .await?;
        if due.is_empty() {
            return Ok(());
        }
        let accounts = self.inner.database.list_accounts().await?;
        let providers = self.oauth_providers().await;
        for flow in due {
            let Some(account) = accounts
                .iter()
                .find(|account| account.id == flow.account_id)
            else {
                // The account was deleted; the row goes with it.
                let _ = self.cancel(flow.account_id).await;
                continue;
            };
            let outcome = providers
                .refresh(
                    &account.provider,
                    flow.account_id,
                    flow.refresh_ref.as_deref(),
                )
                .await;
            match renewal_action(outcome) {
                RenewalAction::Settle => {}
                RenewalAction::Defer(seconds) => self.defer(flow.account_id, now, seconds).await,
                RenewalAction::Fail(message) => {
                    let _ = self
                        .store(
                            flow.account_id,
                            &flow.plugin_id,
                            AuthProgress::Failed { message },
                        )
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn defer(&self, account_id: AccountId, now: DateTime<Utc>, seconds: i64) {
        if let Err(error) = self
            .inner
            .database
            .defer_auth_flow_renewal(account_id, now + chrono::Duration::seconds(seconds))
            .await
        {
            tracing::warn!(%error, "a token renewal could not be held back");
        }
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
            i64::try_from(retry_after_seconds)
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
            .and_then(|seconds| i64::try_from(seconds).ok())
            .unwrap_or(REFRESH_BACKOFF)
            .max(MIN_INTERVAL),
    )
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
mod tests {

    /// This installation's own OAuth client goes into the address the person is sent to, and a
    /// missing one is a refusal rather than an address with an empty `client_id=` in it
    /// (RD-106-04).
    ///
    /// The refusal is the point. Sending somebody to Google with no client produces an error
    /// page about a client that does not exist, which names nothing anybody can act on; refusing
    /// here produces `oauth.client_not_configured`, whose translated text carries the steps.
    #[test]
    fn the_installations_own_client_goes_into_the_address_or_the_sign_in_refuses() {
        let url = "https://accounts.google.com/o/oauth2/v2/auth?client_id={{client_id}}&scope=x";
        assert_eq!(
            super::substitute_client_id(url, Some("42-abc.apps.googleusercontent.com"))
                .expect("substituted"),
            "https://accounts.google.com/o/oauth2/v2/auth\
             ?client_id=42-abc.apps.googleusercontent.com&scope=x"
        );
        // Whitespace somebody pasted along with it is not part of the client id.
        assert_eq!(
            super::substitute_client_id(url, Some("  42-abc  ")).expect("substituted"),
            "https://accounts.google.com/o/oauth2/v2/auth?client_id=42-abc&scope=x"
        );
        // A value that needs encoding gets it: this lands in a query string.
        assert_eq!(
            super::substitute_client_id("https://x/?c={{client_id}}", Some("a b&c"))
                .expect("substituted"),
            "https://x/?c=a+b%26c"
        );
        for missing in [None, Some(""), Some("   ")] {
            assert_eq!(
                super::substitute_client_id(url, missing),
                Err(super::ClientNotConfigured),
                "{missing:?}"
            );
        }
        // A plugin that names no client — one whose provider registers none — is untouched.
        let plain = "https://accounts.example.invalid/authorize?client_id=built-in";
        assert_eq!(
            super::substitute_client_id(plain, None).expect("untouched"),
            plain
        );
    }
    use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};

    use super::{
        AUTH_PLUGIN_MISSING, AUTH_PROVIDER_UNREACHABLE, AuthFlowService, AuthFlowState,
        AuthProgress, FailureKind, ProviderError, REFRESH_BACKOFF, RENEWAL_PLUGIN_MISSING,
        RenewalAction, SIGN_IN_BACKOFF, SIGN_IN_GRACE, TokenOutcome, renewal_action,
        sign_in_progress,
    };

    /// A host that can do nothing. No plugin is ever instantiated in these tests -- the point
    /// of them is the case where none is installed -- so nothing here is ever called.
    struct NoHost;

    #[async_trait::async_trait]
    impl ResolverHost for NoHost {
        async fn http_request(
            &self,
            _client: &ClientIdentity,
            _request: HostHttpRequest,
        ) -> Result<HostHttpResponse, rd_core::Failure> {
            Err(rd_core::Failure::new(
                FailureKind::Unsupported,
                "no host in this test".to_owned(),
            ))
        }

        async fn secret_available(
            &self,
            _account_id: rd_core::AccountId,
            _reference: &str,
        ) -> bool {
            false
        }
    }

    fn flow(started_at: chrono::DateTime<chrono::Utc>) -> rd_core::AuthFlow {
        rd_core::AuthFlow {
            account_id: rd_core::AccountId::new(),
            plugin_id: "019d0000-0000-7000-8000-0000000001ff".to_owned(),
            state: AuthFlowState::Polling,
            verification_url: Some("https://api.example.com/device".to_owned()),
            user_code: Some("ABCD-EFGH".to_owned()),
            expires_at: None,
            next_poll_at: Some(started_at),
            message: None,
            started_at,
            token_expires_at: None,
            refresh_ref: None,
            access_ref: None,
            key_ref: None,
            callback_state: None,
            flow_state: None,
        }
    }

    /// The renewal half of RD-106-02: the two errors are opposite outcomes, not one.
    #[test]
    fn a_renewal_without_a_plugin_fails_while_an_unreachable_provider_is_deferred() {
        assert_eq!(
            renewal_action(Err(ProviderError::NoPlugin {
                provider_slug: "example".to_owned(),
            })),
            RenewalAction::Fail(RENEWAL_PLUGIN_MISSING.to_owned()),
        );
        assert_eq!(
            renewal_action(Err(ProviderError::Failed(anyhow::anyhow!(
                "connection reset"
            )))),
            RenewalAction::Defer(REFRESH_BACKOFF),
        );
    }

    /// A provider's own answer keeps deciding, with the category it always sent and the host
    /// used to drop: busy is not the same as refused.
    #[test]
    fn a_provider_that_refused_ends_the_renewal_and_one_that_was_busy_does_not() {
        assert_eq!(
            renewal_action(Ok(TokenOutcome::Authorized)),
            RenewalAction::Settle
        );
        assert_eq!(
            renewal_action(Ok(TokenOutcome::Pending {
                retry_after_seconds: 90,
            })),
            RenewalAction::Defer(90),
        );
        assert_eq!(
            renewal_action(Ok(TokenOutcome::Failed {
                category: FailureKind::AccountInvalid,
                message: "invalid_grant".to_owned(),
            })),
            RenewalAction::Fail("invalid_grant".to_owned()),
        );
        assert_eq!(
            renewal_action(Ok(TokenOutcome::Failed {
                category: FailureKind::RateLimited {
                    retry_after_seconds: Some(120),
                },
                message: "slow down".to_owned(),
            })),
            RenewalAction::Defer(120),
        );
        assert_eq!(
            renewal_action(Ok(TokenOutcome::Failed {
                category: FailureKind::Offline,
                message: "no route to host".to_owned(),
            })),
            RenewalAction::Defer(REFRESH_BACKOFF),
        );
    }

    /// The sign-in half, which had the same mistake mirrored: everything ended the flow.
    #[test]
    fn a_sign_in_without_a_plugin_fails_while_an_unreachable_provider_is_polled_again() {
        let now = chrono::Utc::now();
        let fresh = flow(now);
        assert_eq!(
            sign_in_progress(
                Err(ProviderError::NoPlugin {
                    provider_slug: "example".to_owned(),
                }),
                &fresh,
                now,
            ),
            AuthProgress::Failed {
                message: AUTH_PLUGIN_MISSING.to_owned(),
            },
        );
        assert_eq!(
            sign_in_progress(
                Err(ProviderError::Failed(anyhow::anyhow!("connection reset"))),
                &fresh,
                now,
            ),
            AuthProgress::Pending {
                retry_after_seconds: SIGN_IN_BACKOFF,
            },
        );
    }

    /// Deferred, but not forever: a flow the provider gave no window for is given up on.
    #[test]
    fn a_sign_in_nobody_named_a_window_for_is_given_up_on_after_the_grace() {
        let now = chrono::Utc::now();
        let old = flow(now - chrono::Duration::seconds(SIGN_IN_GRACE + 1));
        assert_eq!(
            sign_in_progress(
                Err(ProviderError::Failed(anyhow::anyhow!("connection reset"))),
                &old,
                now,
            ),
            AuthProgress::Failed {
                message: AUTH_PROVIDER_UNREACHABLE.to_owned(),
            },
        );
        // One that carries its own expiry keeps being polled until that expiry runs out,
        // which the sweep checks before it ever asks.
        let mut bounded = old.clone();
        bounded.expires_at = Some(now + chrono::Duration::seconds(60));
        assert_eq!(
            sign_in_progress(
                Err(ProviderError::Failed(anyhow::anyhow!("connection reset"))),
                &bounded,
                now,
            ),
            AuthProgress::Pending {
                retry_after_seconds: SIGN_IN_BACKOFF,
            },
        );
    }

    /// The reported defect, end to end: a renewal row whose provider no installed plugin
    /// claims is attempted once, recorded as failed, and never queued again.
    #[tokio::test]
    async fn a_renewal_without_an_oauth_plugin_is_tried_once_and_then_fails() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(temporary.path().join("auth-flows.sqlite3"))
            .await
            .expect("database");
        let account = database
            .create_account(rd_db::NewAccount {
                provider: "example".to_owned(),
                label: "Example".to_owned(),
                username: None,
                credential_mode: None,
                secret_ref: None,
                cookie_ref: None,
                proxy_profile_id: None,
                enabled: true,
            })
            .await
            .expect("account");
        let now = chrono::Utc::now();
        database
            .upsert_auth_flow(rd_db::UpsertAuthFlow {
                account_id: account.id,
                plugin_id: "019d0000-0000-7000-8000-0000000001ff".to_owned(),
                state: AuthFlowState::Authorized,
                verification_url: None,
                user_code: None,
                expires_at: None,
                next_poll_at: None,
                message: None,
                token_expires_at: Some(now + chrono::Duration::seconds(10)),
                refresh_ref: Some("account/example/refresh".to_owned()),
                access_ref: None,
                key_ref: None,
                callback_state: None,
                flow_state: None,
            })
            .await
            .expect("flow");

        let service = AuthFlowService::detached(
            database.clone(),
            temporary.path().join("plugins"),
            std::sync::Arc::new(NoHost),
        );
        service.sweep_renewals(now).await.expect("sweep");

        let stored = database
            .auth_flow(account.id)
            .await
            .expect("read")
            .expect("flow");
        assert_eq!(stored.state, AuthFlowState::Failed);
        assert_eq!(stored.message.as_deref(), Some(RENEWAL_PLUGIN_MISSING));

        // And there is no second attempt: the row left the renewal queue instead of coming
        // back in five minutes for an attempt that could not have gone any differently.
        let later = now + chrono::Duration::seconds(REFRESH_BACKOFF * 2);
        assert!(
            database
                .due_refresh_auth_flows(later, later + chrono::Duration::seconds(60))
                .await
                .expect("due")
                .is_empty()
        );
    }
}

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
