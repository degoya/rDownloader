//! OAuth providers (RD-103-00, RD-106-01): two ways in, and the renewal that outlives both.
//!
//! The sibling of `auth`, and separate for the reason the WIT says: teaching the older world
//! about expiry would have meant an export every shipped auth plugin would have had to grow.
//!
//! Since RD-106-01 a device code is one of the two ways in here rather than a flow of the
//! older world. It changes nothing about the exchange and everything about what follows it:
//! `refresh` already lived in this interface, so a person who typed a code on their phone is
//! renewed by the same sweep as one who came back through a redirect, and is never asked a
//! second time.

use std::sync::Arc;

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, bindings::oauth};
use crate::{PluginManifest, runtime::PluginStoreState};

/// Where the person has to go, and what the host needs to believe what comes back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationRequest {
    pub authorization_url: String,
    /// Echoed back by the provider; a callback carrying anything else is not ours.
    pub state: String,
    pub expires_in_seconds: Option<u64>,
    /// The plugin's own bookkeeping for the exchange. Stored verbatim, never shown.
    pub flow_state: Option<String>,
}

/// What a device flow puts in front of the person, and what the poll needs afterwards.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceAuthorization {
    /// Where the person has to go. Held to the same rule as an authorization address.
    pub verification_url: String,
    /// The short code typed there, when the provider did not fold it into the address.
    pub user_code: Option<String>,
    pub expires_in_seconds: Option<u64>,
    /// The interval the provider asked for. A floor, never permission to ask faster.
    pub interval_seconds: Option<u64>,
    /// The plugin's own bookkeeping — the device code. Stored verbatim, never shown.
    pub flow_state: Option<String>,
}

/// What an exchange or a renewal reported. Mirrors the WIT variant without exposing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenOutcome {
    /// The host has stored what the exchange produced.
    Authorized,
    /// Not finished; ask again after this many seconds.
    Pending { retry_after_seconds: u64 },
    /// It failed, with the plugin's redaction-safe message and what kind of failure it was.
    ///
    /// The category is carried rather than dropped (RD-106-02): "the provider is offline" and
    /// "the provider refused this credential" arrive as the same variant, and the renewal
    /// sweep has to hold a token through the first and give it up on the second.
    Failed {
        category: rd_core::FailureKind,
        message: String,
    },
}

/// A compiled OAuth provider.
pub struct OAuthProvider {
    runtime: ExtensionRuntime,
    pre: oauth::OauthPluginPre<PluginStoreState>,
}

impl OAuthProvider {
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: oauth::OauthPluginPre::new(pre)?,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Builds the address to send the person to, with the challenge already in it.
    pub async fn begin(
        &self,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> Result<AuthorizationRequest> {
        let mut store = self.runtime.store(Some(account_id))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let request = instance
            .rdownloader_plugin_oauth()
            .call_begin(&mut store, &account_id.to_string(), credential_ref)
            .await??;
        Ok(AuthorizationRequest {
            authorization_url: request.authorization_url,
            state: request.state,
            expires_in_seconds: request.expires_in_seconds,
            flow_state: request.flow_state,
        })
    }

    /// Exchanges the code the callback carried, with the bookkeeping `begin` handed over.
    pub async fn poll(
        &self,
        account_id: AccountId,
        code: &str,
        flow_state: Option<&str>,
    ) -> Result<TokenOutcome> {
        let mut store = self.runtime.store(Some(account_id))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let outcome = instance
            .rdownloader_plugin_oauth()
            .call_poll(&mut store, &account_id.to_string(), code, flow_state)
            .await??;
        Ok(token_outcome(outcome))
    }

    /// Asks the provider for a device code and reports what the person has to be shown.
    pub async fn device_begin(
        &self,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> Result<DeviceAuthorization> {
        let mut store = self.runtime.store(Some(account_id))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let authorization = instance
            .rdownloader_plugin_oauth()
            .call_device_begin(&mut store, &account_id.to_string(), credential_ref)
            .await??;
        Ok(DeviceAuthorization {
            verification_url: authorization.verification_url,
            user_code: authorization.user_code,
            expires_in_seconds: authorization.expires_in_seconds,
            interval_seconds: authorization.interval_seconds,
            flow_state: authorization.flow_state,
        })
    }

    /// Asks whether the person has confirmed yet, with the device code handed back.
    pub async fn device_poll(
        &self,
        account_id: AccountId,
        flow_state: Option<&str>,
    ) -> Result<TokenOutcome> {
        let mut store = self.runtime.store(Some(account_id))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let outcome = instance
            .rdownloader_plugin_oauth()
            .call_device_poll(&mut store, &account_id.to_string(), flow_state)
            .await??;
        Ok(token_outcome(outcome))
    }

    /// Mints a new access token from the stored refresh material.
    ///
    /// The one call both ways in share; which entrance produced the stored material is not
    /// this call's business and never was.
    pub async fn refresh(
        &self,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> Result<TokenOutcome> {
        let mut store = self.runtime.store(Some(account_id))?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let outcome = instance
            .rdownloader_plugin_oauth()
            .call_refresh(&mut store, &account_id.to_string(), credential_ref)
            .await??;
        Ok(token_outcome(outcome))
    }
}

fn token_outcome(
    outcome: oauth::exports::rdownloader::plugin::oauth::TokenOutcome,
) -> TokenOutcome {
    use oauth::exports::rdownloader::plugin::oauth::TokenOutcome as Wit;
    match outcome {
        Wit::Authorized => TokenOutcome::Authorized,
        Wit::Pending(seconds) => TokenOutcome::Pending {
            retry_after_seconds: seconds,
        },
        Wit::Failed(failure) => {
            let failure = crate::component::from_wit_failure(failure);
            TokenOutcome::Failed {
                category: failure.category,
                message: failure.message,
            }
        }
    }
}
