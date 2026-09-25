//! A provider authentication flow in progress (RD-090-13).
//!
//! The flow lives in the database rather than in memory, which is what makes it survive a
//! restart: the service sweeps whatever is due, and the interface reads a state rather than
//! driving one. No token appears here — what a flow produces goes into the vault.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AccountId;

/// Where a flow has got to.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthFlowState {
    /// Waiting for the person to visit the address and confirm.
    WaitingForUser,
    /// The provider has been asked and has not answered yet.
    Polling,
    /// Finished; the credential is stored and the account can be used.
    Authorized,
    /// Gave up, or the provider refused. `message` says which.
    Failed,
    /// The person cancelled it.
    Cancelled,
}

impl AuthFlowState {
    /// Whether the sweep loop should still be asking about this flow.
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Self::WaitingForUser | Self::Polling)
    }
}

/// One flow, as the interface sees it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct AuthFlow {
    pub account_id: AccountId,
    /// Which plugin is running it.
    pub plugin_id: String,
    pub state: AuthFlowState,
    /// The address the person has to visit. Shown, never opened automatically.
    pub verification_url: Option<String>,
    /// The code to type there, for a flow that uses one.
    pub user_code: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    /// When the provider will next be asked.
    pub next_poll_at: Option<DateTime<Utc>>,
    /// Redaction-safe reason, for a flow that failed.
    pub message: Option<String>,
    pub started_at: DateTime<Utc>,
    /// When the access token stops working, for a flow that produced one with an expiry.
    ///
    /// Kept in the clear, unlike everything else a flow produces, because the sweep has to be
    /// able to ask whether this is due without decrypting a secret to find out. A time of day
    /// identifies nobody.
    pub token_expires_at: Option<DateTime<Utc>>,
    /// What the provider must echo back on the redirect for the callback to be believed.
    ///
    /// Never serialised: it is the one value that says "this callback is ours", and an API
    /// that handed it out would be publishing the answer to its own question.
    #[serde(skip)]
    #[schema(ignore)]
    pub callback_state: Option<String>,
    /// Vault reference to the material that mints the next access token.
    ///
    /// Never serialised, for the same reason `flow_state` is not: a reference is a handle the
    /// host resolves, and an API that returned it would be publishing the name of a secret for
    /// no reason. `None` means this flow has nothing to renew with — a device code that came
    /// back without refresh material, or a flow that never was an OAuth one.
    #[serde(skip)]
    #[schema(ignore)]
    pub refresh_ref: Option<String>,
    /// Vault reference to the access token, for a provider that keeps it here (RD-106-03).
    ///
    /// `None` for every provider whose access token *is* the account's credential, which is
    /// all of them but one shape: an OAuth provider where the person registers their own
    /// application already has something in `accounts.secret_ref` -- the client secret they
    /// typed -- and writing the token over it would destroy what the next renewal needs.
    ///
    /// Never serialised, for the same reason `refresh_ref` is not.
    #[serde(skip)]
    #[schema(ignore)]
    pub access_ref: Option<String>,
    /// Vault reference to the key material a sign-in left beside its session token
    /// (RD-120-30).
    ///
    /// The one credential column no marker, header or route ever reads: only a key derivation
    /// computes over it. That is also how the host knows where the value came from -- nothing
    /// a person types is written here, only what the host split off a sign-in's
    /// `store-token`. Kept and dropped together with `access_ref`, because the two are halves
    /// of one session.
    ///
    /// Never serialised, for the same reason `refresh_ref` is not.
    #[serde(skip)]
    #[schema(ignore)]
    pub key_ref: Option<String>,
    /// The plugin's own bookkeeping for the next poll.
    ///
    /// Never serialised: it is the plugin's business, of no use to a client, and an API that
    /// returned it would be publishing a short-lived handle for no reason.
    #[serde(skip)]
    #[schema(ignore)]
    pub flow_state: Option<String>,
}

impl AuthFlow {
    /// Whether the flow has outlived the window the provider gave it.
    #[must_use]
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expiry| now >= expiry)
    }

    /// Whether the access token should be renewed now, `lead` ahead of when it actually dies.
    ///
    /// Renewing early is the point: a token that expires between the check and the request it
    /// was fetched for is a failure the person sees, and the lead time is what buys the gap.
    /// A flow with nothing to renew with, or one that is not authorised, is never due.
    #[must_use]
    pub fn needs_refresh(&self, now: DateTime<Utc>, lead: Duration) -> bool {
        self.state == AuthFlowState::Authorized
            && self.refresh_ref.is_some()
            && self
                .token_expires_at
                .is_some_and(|expiry| now + lead >= expiry)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{AuthFlow, AuthFlowState};

    fn flow(expires_in: Option<Duration>) -> AuthFlow {
        let now = Utc::now();
        AuthFlow {
            account_id: crate::AccountId::new(),
            plugin_id: "demo".to_owned(),
            state: AuthFlowState::WaitingForUser,
            verification_url: Some("https://api.example.com/device".to_owned()),
            user_code: Some("ABCD-EFGH".to_owned()),
            expires_at: expires_in.map(|window| now + window),
            next_poll_at: Some(now),
            message: None,
            started_at: now,
            token_expires_at: None,
            refresh_ref: None,
            access_ref: None,
            key_ref: None,
            callback_state: None,
            flow_state: None,
        }
    }

    /// An authorised flow holding renewable material that dies in `expires_in`.
    fn renewable(expires_in: Duration) -> AuthFlow {
        let mut flow = flow(None);
        flow.state = AuthFlowState::Authorized;
        flow.refresh_ref = Some("vault://11111111-1111-1111-1111-111111111111".to_owned());
        flow.token_expires_at = Some(Utc::now() + expires_in);
        flow
    }

    #[test]
    fn only_an_unfinished_flow_is_still_asked_about() {
        assert!(AuthFlowState::WaitingForUser.is_open());
        assert!(AuthFlowState::Polling.is_open());
        for done in [
            AuthFlowState::Authorized,
            AuthFlowState::Failed,
            AuthFlowState::Cancelled,
        ] {
            assert!(!done.is_open(), "{done:?} is finished");
        }
    }

    #[test]
    fn a_flow_without_a_window_never_expires_on_its_own() {
        // A provider that names no expiry has not given us one to enforce. Inventing one
        // would end a flow the person is still in the middle of.
        assert!(!flow(None).is_expired(Utc::now() + Duration::days(7)));
    }

    #[test]
    fn renewal_starts_before_the_token_actually_dies() {
        let flow = renewable(Duration::minutes(10));
        assert!(!flow.needs_refresh(Utc::now(), Duration::minutes(1)));
        // Nine minutes on, one minute of lead reaches the expiry.
        assert!(flow.needs_refresh(Utc::now() + Duration::minutes(9), Duration::minutes(1)));
    }

    #[test]
    fn a_flow_with_nothing_to_renew_with_is_never_due() {
        // The expiry alone is not enough: without refresh material the only way back is the
        // person signing in again, and a sweep that claimed otherwise would spin forever.
        let mut flow = renewable(Duration::minutes(-5));
        flow.refresh_ref = None;
        assert!(!flow.needs_refresh(Utc::now(), Duration::minutes(1)));
    }

    #[test]
    fn only_an_authorised_flow_is_renewed() {
        // A sign-in still in progress has no token to replace yet.
        let mut flow = renewable(Duration::minutes(-5));
        flow.state = AuthFlowState::WaitingForUser;
        assert!(!flow.needs_refresh(Utc::now(), Duration::minutes(1)));
    }

    #[test]
    fn a_token_without_an_expiry_is_left_alone() {
        // A provider that names no expiry has not given us one to act on, exactly as with the
        // flow window above. Renewing on a guess would burn refresh material for nothing.
        let mut flow = renewable(Duration::minutes(10));
        flow.token_expires_at = None;
        assert!(!flow.needs_refresh(Utc::now() + Duration::days(7), Duration::minutes(1)));
    }

    #[test]
    fn a_window_that_has_passed_is_expired() {
        let flow = flow(Some(Duration::minutes(10)));
        assert!(!flow.is_expired(Utc::now()));
        assert!(flow.is_expired(Utc::now() + Duration::minutes(11)));
    }
}
