//! OAuth sign-ins: the authorization address, the device flow and the callback exchange.

use super::*;

impl AuthFlowService {
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
        // The state is what the public callback checks, so it has to be a secret.
        crate::auth_flow_guard::checked_callback_state(&request.state)?;
        let authorization_url = crate::auth_flow_guard::checked_sign_in_address(
            self.with_client_id(account_id, request.authorization_url)
                .await?,
        )?;
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
                    .map(|seconds| seconds_after(now, seconds)),
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
        let verification_url = crate::auth_flow_guard::checked_sign_in_address(
            self.with_client_id(account_id, authorization.verification_url)
                .await?,
        )?;
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
                    .map(|seconds| seconds_after(now, seconds)),
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
    /// The lookup by state is the check, and since the callback is public it is the only one
    /// (security audit 2026-09-30, finding 5): a callback quoting a value no flow claims matches
    /// nothing. The state is taken rather than read, so it is answered once, and a flow no longer
    /// waiting for it — or waiting longer than `auth_flow_guard::CALLBACK_WINDOW_SECONDS` — is
    /// ended rather than completed.
    pub async fn complete_oauth(
        &self,
        callback_state: &str,
        code: &str,
    ) -> anyhow::Result<AuthFlow> {
        let Some(flow) = self
            .inner
            .database
            .take_auth_flow_callback(callback_state.to_owned())
            .await?
        else {
            anyhow::bail!("no sign-in is waiting for this callback");
        };
        if !crate::auth_flow_guard::callback_usable(&flow, Utc::now()) {
            if flow.state == AuthFlowState::WaitingForUser
                && let Err(error) = self
                    .store(
                        flow.account_id,
                        &flow.plugin_id,
                        AuthProgress::Failed {
                            message: "the sign-in window expired".to_owned(),
                        },
                    )
                    .await
            {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "an expired sign-in could not be marked as failed"
                );
            }
            anyhow::bail!("this sign-in is no longer waiting for a callback");
        }
        let accounts = self.inner.database.list_accounts().await?;
        let Some(account) = accounts
            .into_iter()
            .find(|account| account.id == flow.account_id)
        else {
            anyhow::bail!("the account this sign-in belongs to is gone");
        };
        let providers = self.oauth_providers().await;
        let outcome = match providers
            .poll(
                &account.provider,
                flow.account_id,
                code,
                flow.flow_state.as_deref(),
            )
            .await
        {
            Ok(outcome) => token_progress(outcome),
            // The state is spent, so no second callback can finish this flow; left waiting,
            // it would only look stuck until its window ran out.
            Err(error) => {
                let message = callback_failure(&error);
                if let Err(store_error) = self
                    .store(
                        flow.account_id,
                        &flow.plugin_id,
                        AuthProgress::Failed { message },
                    )
                    .await
                {
                    tracing::warn!(
                        error = %format!("{store_error:#}"),
                        "a failed sign-in callback could not be recorded"
                    );
                }
                return Err(error.into());
            }
        };
        self.store(flow.account_id, &flow.plugin_id, outcome).await
    }
}
