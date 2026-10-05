//! Database facade for accounts, their sign-in flows, proxy profiles and Usenet servers.

use anyhow::Result;

use crate::{
    Database, NetworkClientConfig, NewAccount, NewProxyProfile, NewUsenetServer, UpdateAccount,
    UpdateUsenetServer, UsenetConnectionConfig,
    commands::{AuthCommand, NetworkCommand},
    network_store, usenet_store, writer,
};

impl Database {
    /// Creates account metadata referring to separately stored secrets.
    pub async fn create_account(&self, input: NewAccount) -> Result<rd_core::Account> {
        writer::request(&self.writer, |reply| NetworkCommand::CreateAccount {
            input,
            reply,
        })
        .await
    }

    /// Updates public account metadata and its selected secret references.
    pub async fn update_account(
        &self,
        id: rd_core::AccountId,
        input: UpdateAccount,
    ) -> Result<rd_core::Account> {
        writer::request(&self.writer, |reply| NetworkCommand::UpdateAccount {
            id,
            input,
            reply,
        })
        .await
    }

    /// Deletes an unused provider account and returns its opaque secret references for cleanup.
    pub async fn delete_account(
        &self,
        id: rd_core::AccountId,
    ) -> Result<(Option<String>, Option<String>)> {
        let (references, sign_in) = writer::request(&self.writer, |reply| {
            NetworkCommand::DeleteAccount { id, reply }
        })
        .await?;
        // The sign-in's rows went with the account (cascade); its tokens go with them (DB-02).
        self.forget_secrets(sign_in).await;
        Ok(references)
    }

    /// Loads opaque account secret references for replacement without exposing values.
    pub async fn account_secret_refs(
        &self,
        id: rd_core::AccountId,
    ) -> Result<Option<(Option<String>, Option<String>)>> {
        network_store::account_secret_refs(&self.readers, id).await
    }

    /// Creates or advances the authentication flow of one account (RD-090-13).
    pub async fn upsert_auth_flow(
        &self,
        input: crate::auth_flow_store::UpsertAuthFlow,
    ) -> Result<rd_core::AuthFlow> {
        let (flow, released) = writer::request(&self.writer, |reply| AuthCommand::UpsertAuthFlow {
            input: Box::new(input),
            reply,
        })
        .await?;
        // A restarted sign-in writes no references; the previous one's tokens leave the vault
        // once nothing points at them (DB-02).
        self.forget_secrets(released).await;
        Ok(flow)
    }

    /// Records what a renewal produced: the expiry, the refresh reference (RD-103-00) and,
    /// for a provider that keeps its access token here rather than on the account, that
    /// reference too (RD-106-03).
    ///
    /// `access_ref` of `None` leaves the stored one alone. Every provider but the one shape
    /// passes `None` for ever, and clearing it on each renewal would take the token away from
    /// a resolver that is using it.
    pub async fn set_auth_flow_renewal(
        &self,
        account_id: rd_core::AccountId,
        token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
        refresh_ref: Option<String>,
        access_ref: Option<String>,
    ) -> Result<()> {
        let dropped_key = writer::request(&self.writer, |reply| AuthCommand::SetAuthFlowRenewal {
            account_id,
            token_expires_at,
            refresh_ref,
            access_ref,
            reply,
        })
        .await?;
        self.forget_secrets(dropped_key.into_iter().collect()).await;
        Ok(())
    }

    /// Records the session a sign-in stored beside the account's own credential: the token
    /// and, when the sign-in left one, its key material (RD-120-30).
    ///
    /// Both references in one statement, so a token is never paired with another session's
    /// key. Creates the row when the sign-in finished before the flow service wrote one.
    pub async fn set_auth_flow_session(
        &self,
        account_id: rd_core::AccountId,
        access_ref: String,
        key_ref: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| AuthCommand::SetAuthFlowSession {
            account_id,
            access_ref,
            key_ref,
            reply,
        })
        .await
    }

    /// Records one named part a sign-in keeps beside its token (RD-150-09).
    ///
    /// Answers the vault reference this replaced, which nothing references any more and the
    /// caller drops; `None` when the part is new.
    pub async fn set_auth_flow_part(
        &self,
        account_id: rd_core::AccountId,
        name: String,
        secret_ref: String,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| AuthCommand::SetAuthFlowPart {
            account_id,
            name,
            secret_ref,
            reply,
        })
        .await
    }

    /// The vault reference of one named part of an account's sign-in (RD-150-09).
    pub async fn auth_flow_part(
        &self,
        account_id: rd_core::AccountId,
        name: &str,
    ) -> Result<Option<String>> {
        crate::auth_flow_store::part(&self.readers, account_id, name).await
    }

    /// Removes the authentication flow of one account, and from the vault the tokens and
    /// parts it held (DB-02).
    pub async fn delete_auth_flow(&self, account_id: rd_core::AccountId) -> Result<()> {
        let released = writer::request(&self.writer, |reply| AuthCommand::DeleteAuthFlow {
            account_id,
            reply,
        })
        .await?;
        self.forget_secrets(released).await;
        Ok(())
    }

    /// The authentication flow of one account, if it has one.
    pub async fn auth_flow(
        &self,
        account_id: rd_core::AccountId,
    ) -> Result<Option<rd_core::AuthFlow>> {
        crate::auth_flow_store::get(&self.readers, account_id).await
    }

    /// The authentication flow an arriving OAuth callback belongs to (RD-103-00). The callback
    /// itself takes it (`take_auth_flow_callback`); this read is for the tests.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn auth_flow_by_callback_state(
        &self,
        callback_state: &str,
    ) -> Result<Option<rd_core::AuthFlow>> {
        crate::auth_flow_store::by_callback_state(&self.readers, callback_state).await
    }

    /// Takes the flow an OAuth callback belongs to, so the same state is never answered twice.
    pub async fn take_auth_flow_callback(
        &self,
        callback_state: String,
    ) -> Result<Option<rd_core::AuthFlow>> {
        writer::request(&self.writer, |reply| AuthCommand::TakeAuthFlowCallback {
            callback_state,
            reply,
        })
        .await
    }

    /// Every open authentication flow whose next poll is due.
    pub async fn due_auth_flows(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::AuthFlow>> {
        crate::auth_flow_store::due(&self.readers, now).await
    }

    /// Every authorised flow whose access token is due for renewal by `threshold` (RD-103-00).
    pub async fn due_refresh_auth_flows(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        threshold: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::AuthFlow>> {
        crate::auth_flow_store::due_refresh(&self.readers, now, threshold).await
    }

    /// Holds a renewal back after a provider asked for more time (RD-103-00).
    pub async fn defer_auth_flow_renewal(
        &self,
        account_id: rd_core::AccountId,
        next_poll_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| AuthCommand::DeferAuthFlowRenewal {
            account_id,
            next_poll_at,
            reply,
        })
        .await
    }

    /// Lists public account metadata without secret references or values.
    pub async fn list_accounts(&self) -> Result<Vec<rd_core::Account>> {
        network_store::list_accounts(&self.readers).await
    }

    /// One account's public metadata, or `None` when the id names none.
    ///
    /// For a caller that wants one account: [`Self::list_accounts`] reads every row, and 14
    /// call sites filtered it for a single id (audit 1.9.1, DB-04).
    pub async fn get_account(&self, id: rd_core::AccountId) -> Result<Option<rd_core::Account>> {
        network_store::get_account(&self.readers, id).await
    }

    /// Creates a proxy profile with an optional opaque credential reference.
    pub async fn create_proxy_profile(
        &self,
        input: NewProxyProfile,
    ) -> Result<rd_core::ProxyProfile> {
        writer::request(&self.writer, |reply| NetworkCommand::CreateProxyProfile {
            input,
            reply,
        })
        .await
    }

    /// Replaces the editable fields of one proxy profile.
    pub async fn update_proxy_profile(
        &self,
        id: rd_core::ProxyProfileId,
        input: NewProxyProfile,
    ) -> Result<rd_core::ProxyProfile> {
        writer::request(&self.writer, |reply| NetworkCommand::UpdateProxyProfile {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes a proxy profile, returning its credential reference for the vault sweep.
    ///
    /// Refused while an account, a Usenet server or an unfinished download still points at it.
    pub async fn delete_proxy_profile(
        &self,
        id: rd_core::ProxyProfileId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| NetworkCommand::DeleteProxyProfile {
            id,
            reply,
        })
        .await
    }

    /// Lists proxy profiles. Serialization omits their opaque secret reference.
    pub async fn list_proxy_profiles(&self) -> Result<Vec<rd_core::ProxyProfile>> {
        network_store::list_proxy_profiles(&self.readers).await
    }

    /// Loads one proxy profile, for callers that need nothing else from the network config.
    pub async fn proxy_profile(
        &self,
        id: rd_core::ProxyProfileId,
    ) -> Result<Option<rd_core::ProxyProfile>> {
        network_store::load_proxy(&self.readers, id).await
    }

    /// Resolves job, account, proxy and auth-profile metadata using the required precedence.
    pub async fn network_client_config(
        &self,
        account_id: Option<rd_core::AccountId>,
        job_proxy_id: Option<rd_core::ProxyProfileId>,
        global_proxy_id: Option<rd_core::ProxyProfileId>,
        auth_profile: rd_core::AuthProfileSelection,
        url: &url::Url,
    ) -> Result<NetworkClientConfig> {
        network_store::client_config(
            &self.readers,
            account_id,
            job_proxy_id,
            global_proxy_id,
            auth_profile,
            url,
        )
        .await
    }

    /// Persists one redaction-safe NNTP server configuration.
    pub async fn create_usenet_server(
        &self,
        input: NewUsenetServer,
    ) -> Result<rd_core::UsenetServer> {
        writer::request(&self.writer, |reply| NetworkCommand::CreateUsenetServer {
            input,
            reply,
        })
        .await
    }

    /// Updates one redaction-safe NNTP server configuration.
    pub async fn update_usenet_server(
        &self,
        id: rd_core::UsenetServerId,
        input: UpdateUsenetServer,
    ) -> Result<rd_core::UsenetServer> {
        writer::request(&self.writer, |reply| NetworkCommand::UpdateUsenetServer {
            id,
            input,
            reply,
        })
        .await
    }

    /// Deletes one NNTP endpoint and returns its opaque password reference for cleanup.
    pub async fn delete_usenet_server(
        &self,
        id: rd_core::UsenetServerId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| NetworkCommand::DeleteUsenetServer {
            id,
            reply,
        })
        .await
    }

    /// Lists NNTP endpoints in fallback priority order.
    pub async fn list_usenet_servers(&self) -> Result<Vec<rd_core::UsenetServer>> {
        usenet_store::list(&self.readers).await
    }

    /// Loads one enabled NNTP endpoint with its opaque password reference.
    pub async fn usenet_connection_config(
        &self,
        id: rd_core::UsenetServerId,
    ) -> Result<Option<UsenetConnectionConfig>> {
        usenet_store::connection_config(&self.readers, id).await
    }
}
