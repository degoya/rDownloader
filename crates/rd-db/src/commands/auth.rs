//! The commands of `writer/auth.rs`.

use super::Reply;

/// The commands `Writer::handle_auth` applies.
pub(crate) enum AuthCommand {
    /// Stores the probed format inventory of a media candidate (RD-080-01).
    /// Creates or advances an authentication flow (RD-090-13).
    UpsertAuthFlow {
        input: Box<crate::auth_flow_store::UpsertAuthFlow>,
        /// The flow, and the vault references the row let go of.
        reply: Reply<(rd_core::AuthFlow, Vec<String>)>,
    },
    /// Records the expiry and refresh reference a renewal produced (RD-103-00).
    SetAuthFlowRenewal {
        account_id: rd_core::AccountId,
        token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
        refresh_ref: Option<String>,
        /// `None` leaves whatever is stored alone; the token only moves when a caller says so.
        access_ref: Option<String>,
        /// The key reference a new token dropped.
        reply: Reply<Option<String>>,
    },
    /// Records a sign-in's session beside the account's own credential (RD-120-30).
    SetAuthFlowSession {
        account_id: rd_core::AccountId,
        access_ref: String,
        key_ref: Option<String>,
        reply: Reply<()>,
    },
    /// Records one named part a sign-in keeps beside its token (RD-150-09); answers the vault
    /// reference it replaced.
    SetAuthFlowPart {
        account_id: rd_core::AccountId,
        name: String,
        secret_ref: String,
        reply: Reply<Option<String>>,
    },
    /// Holds a renewal back after a provider asked for more time (RD-103-00).
    DeferAuthFlowRenewal {
        account_id: rd_core::AccountId,
        next_poll_at: chrono::DateTime<chrono::Utc>,
        reply: Reply<()>,
    },
    /// Removes an authentication flow; answers the vault references it and its parts held.
    DeleteAuthFlow {
        account_id: rd_core::AccountId,
        reply: Reply<Vec<String>>,
    },
    /// Hands back the flow waiting for an OAuth callback and forgets its state in the same write.
    TakeAuthFlowCallback {
        callback_state: String,
        reply: Reply<Option<rd_core::AuthFlow>>,
    },
    SetDownloadAuthProfile {
        id: rd_core::DownloadId,
        selection: rd_core::AuthProfileSelection,
        reply: Reply<()>,
    },
    CreateAuthProfile {
        input: crate::auth_profile_store::NewAuthProfile,
        reply: Reply<rd_core::AuthProfile>,
    },
    UpdateAuthProfile {
        id: rd_core::AuthProfileId,
        input: crate::auth_profile_store::UpdateAuthProfile,
        /// The profile plus the secret references it stopped using.
        reply: Reply<(rd_core::AuthProfile, Vec<String>)>,
    },
    SetAuthProfileEnabled {
        id: rd_core::AuthProfileId,
        enabled: bool,
        reply: Reply<rd_core::AuthProfile>,
    },
    DeleteAuthProfile {
        id: rd_core::AuthProfileId,
        /// Secret references orphaned by the deletion.
        reply: Reply<Vec<String>>,
    },
}
