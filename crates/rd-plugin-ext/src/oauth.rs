//! OAuth provider plugins (RD-103-00, RD-106-01).
//!
//! The same bargain the authentication adapter strikes, extended by one thing: a token that
//! expires. The plugin runs the exchange, the core owns the flow -- which plugin runs for an
//! account, the waiting between polls, the vault the token disappears into, and above all the
//! address the person is sent to.
//!
//! Two ways in since RD-106-01, and one rule for both: what the manifest offers is what the
//! host calls. A plugin that serves only the redirect is never asked for a device code, so
//! nobody has to write an entrance whose whole purpose is to be refused -- and if the host is
//! asked for one anyway, the refusal is a typed error the caller can act on rather than a
//! sign-in that fails in front of somebody.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    OAuthFlowManifest, PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry,
    extension::{AuthorizationRequest, DeviceAuthorization, OAuthProvider, TokenOutcome},
};

use crate::{
    auth::declared,
    provider::{ProviderError, ProviderResult},
};

/// The installed OAuth providers, one per claimed provider slug.
pub struct OAuthProviders {
    plugins: Vec<Provider>,
    /// Provider slug the plugin claims to its index -- the key an account carries.
    by_slug: HashMap<String, usize>,
}

struct Provider {
    manifest: PluginManifest,
    plugin: OAuthProvider,
}

impl OAuthProviders {
    /// Loads every installed OAuth provider, skipping any that fails to build.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let loaded = registry.instantiate(&PluginType::OAuth, |package| {
            OAuthProvider::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| Provider {
                    manifest: package.manifest.clone(),
                    plugin,
                },
            )
        });
        let mut plugins = Vec::new();
        let mut by_slug = HashMap::new();
        for provider in loaded {
            let claims = provider
                .manifest
                .extension
                .as_ref()
                .map(|extension| extension.claims.clone())
                .unwrap_or_default();
            if claims.is_empty() {
                tracing::warn!(
                    plugin = %provider.manifest.name,
                    "oauth plugin claims no provider and cannot be used"
                );
                continue;
            }
            let index = plugins.len();
            plugins.push(provider);
            for slug in claims {
                by_slug.entry(slug.to_ascii_lowercase()).or_insert(index);
            }
        }
        Self { plugins, by_slug }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
            by_slug: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Whether an installed OAuth plugin claims this provider, whichever way in it serves.
    #[must_use]
    pub fn supports(&self, provider_slug: &str) -> bool {
        self.by_slug
            .contains_key(&provider_slug.to_ascii_lowercase())
    }

    /// The way in to offer this provider, or `None` when no plugin claims it.
    ///
    /// The manifest's first entry, because the order in it is the author's preference. A
    /// provider offering both is one plugin with both listed, which is the whole point of
    /// having put the device path in this interface rather than in a world of its own.
    #[must_use]
    pub fn preferred_flow(&self, provider_slug: &str) -> Option<OAuthFlowManifest> {
        self.provider(provider_slug)
            .ok()
            .and_then(|provider| provider.manifest.oauth_flows().first().copied())
    }

    /// Whether the plugin claiming this provider serves the given way in.
    #[must_use]
    pub fn serves(&self, provider_slug: &str, flow: OAuthFlowManifest) -> bool {
        self.provider(provider_slug)
            .is_ok_and(|provider| provider.manifest.serves_oauth_flow(flow))
    }

    /// The plugin id that would run a flow for this provider.
    #[must_use]
    pub fn plugin_id(&self, provider_slug: &str) -> Option<String> {
        self.provider(provider_slug)
            .ok()
            .map(|provider| provider.manifest.id.to_string())
    }

    /// Builds the address to send the person to.
    ///
    /// This is the one call with a gate on its way out. A flow's whole purpose is to put an
    /// address in front of somebody and ask them to sign in there, and for OAuth that address
    /// is where they type the password to their mail account. A plugin that could name any
    /// address would be a signed, installed phishing page, so the address has to be one the
    /// manifest they saw before installing already declared.
    pub async fn begin(
        &self,
        provider_slug: &str,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> ProviderResult<AuthorizationRequest> {
        let provider = self.served(provider_slug, OAuthFlowManifest::Redirect)?;
        let request = provider.plugin.begin(account_id, credential_ref).await?;
        gate(&provider.manifest, &request.authorization_url)?;
        Ok(request)
    }

    /// Asks the provider for a device code and reports what the person has to be shown.
    ///
    /// The same gate as `begin`, for the same reason: this address is put in front of
    /// somebody and they are asked to sign in there. That a device flow has no callback
    /// makes the address no less worth confining.
    pub async fn device_begin(
        &self,
        provider_slug: &str,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> ProviderResult<DeviceAuthorization> {
        let provider = self.served(provider_slug, OAuthFlowManifest::Device)?;
        let authorization = provider
            .plugin
            .device_begin(account_id, credential_ref)
            .await?;
        gate(&provider.manifest, &authorization.verification_url)?;
        Ok(authorization)
    }

    /// Asks whether the person has confirmed the device code yet.
    pub async fn device_poll(
        &self,
        provider_slug: &str,
        account_id: AccountId,
        flow_state: Option<&str>,
    ) -> ProviderResult<TokenOutcome> {
        let provider = self.served(provider_slug, OAuthFlowManifest::Device)?;
        Ok(provider.plugin.device_poll(account_id, flow_state).await?)
    }

    /// Exchanges the code a callback carried.
    pub async fn poll(
        &self,
        provider_slug: &str,
        account_id: AccountId,
        code: &str,
        flow_state: Option<&str>,
    ) -> ProviderResult<TokenOutcome> {
        let provider = self.served(provider_slug, OAuthFlowManifest::Redirect)?;
        Ok(provider.plugin.poll(account_id, code, flow_state).await?)
    }

    /// Mints a new access token from the stored refresh material.
    ///
    /// No address gate here: a renewal shows the person nothing, so there is nothing to
    /// mislead them with. What confines it is the manifest's domain list, which the sandbox
    /// applies to the request itself.
    pub async fn refresh(
        &self,
        provider_slug: &str,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> ProviderResult<TokenOutcome> {
        let provider = self.provider(provider_slug)?;
        Ok(provider.plugin.refresh(account_id, credential_ref).await?)
    }

    /// The plugin that claims this provider *and* serves this way in.
    ///
    /// Separate from [`Self::provider`] because the two refusals are different facts: nothing
    /// is installed, or something is installed and offers another entrance. The caller shows
    /// a different sentence for each, and the renewal sweep gives up on both.
    fn served(&self, provider_slug: &str, flow: OAuthFlowManifest) -> ProviderResult<&Provider> {
        let provider = self.provider(provider_slug)?;
        require_flow(&provider.manifest, provider_slug, flow)?;
        Ok(provider)
    }

    /// The plugin that claims this provider, or the reason there is none.
    ///
    /// Typed for the renewal sweep's sake: a provider nobody claims is terminal, while a
    /// renewal that failed is not, and the two used to be one string apart.
    fn provider(&self, provider_slug: &str) -> ProviderResult<&Provider> {
        self.by_slug
            .get(&provider_slug.to_ascii_lowercase())
            .and_then(|index| self.plugins.get(*index))
            .ok_or_else(|| ProviderError::no_plugin(provider_slug))
    }
}

/// Refuses an address the manifest the person saw before installing did not declare.
///
/// One function for both ways in, because both put an address in front of somebody: a plugin
/// that could name any address would be a signed, installed phishing page.
fn gate(manifest: &PluginManifest, url: &str) -> ProviderResult<()> {
    if declared(manifest, url) {
        return Ok(());
    }
    tracing::warn!(
        plugin = %manifest.name,
        "oauth plugin proposed a sign-in address outside its manifest"
    );
    // A refusal of this plugin's answer, not a missing plugin: one is installed and claiming
    // the provider, and it just proposed somewhere it may not send anybody.
    Err(ProviderError::Failed(anyhow::anyhow!(
        "the plugin proposed a sign-in address it did not declare"
    )))
}

/// Refuses a way in the manifest does not offer (RD-106-01).
///
/// A free function so the decision can be tested without a component, a host or an installed
/// package -- the same reason `renewal_action` was pulled out of the sweep.
fn require_flow(
    manifest: &PluginManifest,
    provider_slug: &str,
    flow: OAuthFlowManifest,
) -> ProviderResult<()> {
    if manifest.serves_oauth_flow(flow) {
        return Ok(());
    }
    tracing::warn!(
        plugin = %manifest.name,
        flow = flow.as_str(),
        "oauth plugin was asked for a way in its manifest does not offer"
    );
    Err(ProviderError::UnsupportedFlow {
        provider_slug: provider_slug.to_owned(),
        flow: flow.as_str(),
    })
}

#[cfg(test)]
mod tests {
    use super::{OAuthFlowManifest, PluginManifest, ProviderError, require_flow};

    /// An `oauth` manifest with whatever `oauth_flows` line is handed in.
    fn manifest(flows: &str) -> PluginManifest {
        let text = format!(
            r#"
manifest_version = 3
plugin_type = "oauth"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000000e2"
name = "Fixture OAuth"
version = "0.1.0"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
{flows}

[capabilities.net_http]
domains = ["oauth.example.invalid"]

[extension]
slug = "fixture_oauth"
claims = ["fixture"]

[metadata]
description = "A manifest, and nothing behind it."
author = "rDownloader project"
"#
        );
        toml::from_str(&text).expect("the fixture manifest parses")
    }

    /// A manifest written before the field existed means the redirect, and only that.
    ///
    /// The compatibility promise of RD-106-01 in one assertion: adding the device entrance
    /// must not turn an existing OAuth plugin into one the host thinks offers it.
    #[test]
    fn a_manifest_without_the_field_offers_the_redirect_and_nothing_else() {
        let silent = manifest("");
        assert_eq!(
            silent.oauth_flows(),
            [OAuthFlowManifest::Redirect].as_slice()
        );
        assert!(require_flow(&silent, "fixture", OAuthFlowManifest::Redirect).is_ok());
        let refused = require_flow(&silent, "fixture", OAuthFlowManifest::Device)
            .expect_err("a device sign-in must be refused");
        assert!(refused.is_terminal());
        // Not a missing plugin: one is installed and claims the provider. The two reach the
        // person as different sentences, so they must not collapse into one here.
        assert!(!refused.is_missing_plugin());
        assert!(matches!(
            refused,
            ProviderError::UnsupportedFlow { flow: "device", .. }
        ));
    }

    /// One plugin can hold both ways in, which is the fourth acceptance criterion of the job
    /// and the reason the device path went into this interface rather than a world of its own.
    #[test]
    fn a_provider_offering_both_ways_in_is_one_plugin() {
        let both = manifest(r#"oauth_flows = ["device", "redirect"]"#);
        assert!(require_flow(&both, "fixture", OAuthFlowManifest::Device).is_ok());
        assert!(require_flow(&both, "fixture", OAuthFlowManifest::Redirect).is_ok());
        // The order is the author's preference, and the first entry is what the host offers.
        assert_eq!(both.oauth_flows().first(), Some(&OAuthFlowManifest::Device));
    }

    /// A device-only plugin is never asked to build an authorization URL it has none of.
    #[test]
    fn a_device_only_plugin_is_not_asked_for_a_redirect() {
        let device = manifest(r#"oauth_flows = ["device"]"#);
        assert!(require_flow(&device, "fixture", OAuthFlowManifest::Device).is_ok());
        assert!(matches!(
            require_flow(&device, "fixture", OAuthFlowManifest::Redirect),
            Err(ProviderError::UnsupportedFlow {
                flow: "redirect",
                ..
            })
        ));
    }
}
