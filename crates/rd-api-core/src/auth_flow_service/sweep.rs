//! The sweep that keeps flows going: due sign-in polls and token renewals.

use super::*;

impl AuthFlowService {
    pub(super) async fn sweep_loop(self) {
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
                if let Err(error) = self
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
                continue;
            }
            let Some(account) = accounts
                .iter()
                .find(|account| account.id == flow.account_id)
            else {
                // The account was deleted mid-flow; the row goes with it.
                if let Err(error) = self.cancel(flow.account_id).await {
                    tracing::warn!(
                        error = %format!("{error:#}"),
                        "the sign-in of a deleted account could not be removed"
                    );
                }
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
    pub(super) async fn sweep_renewals(&self, now: DateTime<Utc>) -> anyhow::Result<()> {
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
                if let Err(error) = self.cancel(flow.account_id).await {
                    tracing::warn!(
                        error = %format!("{error:#}"),
                        "the token renewal of a deleted account could not be removed"
                    );
                }
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
                    // Nobody is in front of a renewal, and the account signs in no more until
                    // somebody does it again (RD-190-19).
                    let notice = crate::notify_notice::Notice::account_invalid(
                        account,
                        &message,
                        now.date_naive(),
                    );
                    if let Err(error) = self
                        .store(
                            flow.account_id,
                            &flow.plugin_id,
                            AuthProgress::Failed { message },
                        )
                        .await
                    {
                        tracing::warn!(
                            error = %format!("{error:#}"),
                            "a failed token renewal could not be recorded"
                        );
                    }
                    crate::notify_notice::announce(&self.inner.database, notice).await;
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
