//! Authentication providers (RD-090-13): a flow that ends in a stored token.

use std::sync::Arc;

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, bindings::auth};
use crate::{PluginManifest, runtime::PluginStoreState};

/// What an authentication flow reported. Mirrors the WIT variant without exposing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthProgress {
    /// The account is authorized; whatever the flow produced is already stored.
    Authorized,
    /// The user has to visit a URL and confirm.
    UserAction {
        verification_url: String,
        user_code: Option<String>,
        expires_in_seconds: Option<u64>,
        /// The plugin's own bookkeeping for the next poll. Stored verbatim, never shown.
        flow_state: Option<String>,
    },
    /// Still waiting; ask again after this many seconds.
    Pending { retry_after_seconds: u64 },
    /// The flow failed, with the plugin's redaction-safe message.
    Failed { message: String },
}

/// A compiled authentication provider.
pub struct AuthProvider {
    runtime: ExtensionRuntime,
    pre: auth::AuthPluginPre<PluginStoreState>,
}

impl AuthProvider {
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: auth::AuthPluginPre::new(pre)?,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Starts a flow. `credential_ref` is an opaque handle; the plugin never sees a secret.
    ///
    /// The store is bound to `account_id`, which is also the only account the plugin can
    /// store a token for: an invocation started for one account cannot write another's.
    pub async fn begin(
        &self,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> Result<AuthProgress> {
        let mut store = self.runtime.store(Some(account_id))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let state = instance
            .rdownloader_plugin_auth()
            .call_begin(&mut store, &account_id.to_string(), credential_ref)
            .await??;
        Ok(progress(state))
    }

    /// Continues a flow the host started earlier, with the bookkeeping it handed over.
    pub async fn poll(
        &self,
        account_id: AccountId,
        flow_state: Option<&str>,
    ) -> Result<AuthProgress> {
        let mut store = self.runtime.store(Some(account_id))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let state = instance
            .rdownloader_plugin_auth()
            .call_poll(&mut store, &account_id.to_string(), flow_state)
            .await??;
        Ok(progress(state))
    }
}

fn progress(state: auth::exports::rdownloader::plugin::auth::AuthState) -> AuthProgress {
    use auth::exports::rdownloader::plugin::auth::AuthState;
    match state {
        AuthState::Authorized => AuthProgress::Authorized,
        AuthState::UserAction(prompt) => AuthProgress::UserAction {
            verification_url: prompt.verification_url,
            user_code: prompt.user_code,
            expires_in_seconds: prompt.expires_in_seconds,
            flow_state: prompt.flow_state,
        },
        AuthState::Pending(seconds) => AuthProgress::Pending {
            retry_after_seconds: seconds,
        },
        AuthState::Failed(failure) => AuthProgress::Failed {
            message: failure.message,
        },
    }
}

/// Storing what a flow produced.
///
/// Three things confine this, and all three are needed. The interface is linked only into
/// the authentication world, so no other plugin type can name it. The account is compared
/// with the one this invocation was started for, so a plugin cannot write into a stranger's
/// account by naming it. And the host — not the guest — decides which vault reference the
/// account's provider owns, so there is no reference for a plugin to aim at in the first
/// place.
impl auth::rdownloader::plugin::credentials::Host for PluginStoreState {
    async fn store_token(
        &mut self,
        account_id: String,
        value: String,
    ) -> Result<(), crate::component::rdownloader::plugin::types::Failure> {
        let refused = |code: &str, message: &str| {
            crate::component::to_wit_failure(rd_core::Failure::coded(
                rd_core::FailureKind::Permanent,
                code,
                message.to_owned(),
            ))
        };
        let Ok(account_id) = account_id.parse::<AccountId>() else {
            return Err(refused("plugin.account_unavailable", "Unknown account"));
        };
        if self.identity().account_id != Some(account_id) {
            return Err(refused(
                "plugin.store_token_not_allowed",
                "A flow may only store a credential for its own account",
            ));
        }
        let Some(host) = self.host() else {
            return Err(refused(
                "plugin.store_token_unsupported",
                "Storing a credential is not supported by this host",
            ));
        };
        // Whatever a flow produced is a secret from here on: it must never appear in a log
        // line the plugin writes afterwards -- and a keyed session's token and key each on
        // their own, since a line quoting one half would pass a redaction of the whole
        // (RD-120-30).
        self.remember_redactions(crate::session::redactions(&value));
        host.store_token(account_id, &value)
            .await
            .map_err(crate::component::to_wit_failure)
    }

    async fn store_oauth_token(
        &mut self,
        account_id: String,
        access_token: String,
        refresh_token: Option<String>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), crate::component::rdownloader::plugin::types::Failure> {
        let refused = |code: &str, message: &str| {
            crate::component::to_wit_failure(rd_core::Failure::coded(
                rd_core::FailureKind::Permanent,
                code,
                message.to_owned(),
            ))
        };
        let Ok(account_id) = account_id.parse::<AccountId>() else {
            return Err(refused("plugin.account_unavailable", "Unknown account"));
        };
        if self.identity().account_id != Some(account_id) {
            return Err(refused(
                "plugin.store_token_not_allowed",
                "A flow may only store a credential for its own account",
            ));
        }
        let Some(host) = self.host() else {
            return Err(refused(
                "plugin.store_token_unsupported",
                "Storing a credential is not supported by this host",
            ));
        };
        // Both halves are secrets, and the refresh token is the more dangerous of the two: it
        // outlives the access token and mints replacements. Neither may surface in a log line
        // the plugin writes after this call.
        self.remember_redactions(
            std::iter::once(access_token.clone()).chain(refresh_token.clone()),
        );
        host.store_oauth_token(
            account_id,
            &access_token,
            refresh_token.as_deref(),
            expires_in_seconds,
        )
        .await
        .map_err(crate::component::to_wit_failure)
    }
}
