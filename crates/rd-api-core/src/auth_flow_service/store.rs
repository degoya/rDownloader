//! Writing what a plugin reported about a flow.

use super::*;

impl AuthFlowService {
    /// Writes what a plugin reported.
    pub(super) async fn store(
        &self,
        account_id: AccountId,
        plugin_id: &str,
        progress: AuthProgress,
    ) -> anyhow::Result<AuthFlow> {
        let now = Utc::now();
        // The one place every address a sign-in shows passes through, whichever world and
        // whichever call produced it: one that is no safe link ends the flow instead.
        let progress = match progress {
            AuthProgress::UserAction {
                verification_url,
                user_code,
                expires_in_seconds,
                flow_state,
            } => match crate::auth_flow_guard::checked_sign_in_address(verification_url) {
                Ok(verification_url) => AuthProgress::UserAction {
                    verification_url,
                    user_code,
                    expires_in_seconds,
                    flow_state,
                },
                Err(error) => AuthProgress::Failed {
                    message: format!("{error:#}"),
                },
            },
            other => other,
        };
        let input = match progress {
            AuthProgress::Authorized => {
                // The renewal columns are carried over rather than cleared. By the time a
                // plugin reports success the host has already written them, from inside its
                // `store-oauth-token` call -- so blanking the row here would throw away the
                // expiry and the refresh reference microseconds after recording them, and the
                // token would never be renewed. A device flow has nothing there to carry.
                // A read that failed is an error, not "nothing to carry" (RA-DB-05): written as
                // `None`, the upsert would drop the references and the tokens behind them.
                let kept = self.flow(account_id).await?;
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
                expires_at: expires_in_seconds.map(|seconds| seconds_after(now, seconds)),
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
                let kept = self.flow(account_id).await?;
                rd_db::UpsertAuthFlow {
                    account_id,
                    plugin_id: plugin_id.to_owned(),
                    state: AuthFlowState::Polling,
                    verification_url: kept.as_ref().and_then(|flow| flow.verification_url.clone()),
                    user_code: kept.as_ref().and_then(|flow| flow.user_code.clone()),
                    expires_at: kept.as_ref().and_then(|flow| flow.expires_at),
                    next_poll_at: Some(seconds_after(
                        now,
                        retry_after_seconds.max(MIN_INTERVAL.unsigned_abs()),
                    )),
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
}
