//! Authentication provider plugins (RD-090-13).
//!
//! A plugin runs the flow; the core owns everything about it that matters. Which plugin runs
//! for an account is decided by the provider it claims, the waiting between polls is the
//! host's, the token goes into the vault through a host function the plugin cannot read back,
//! and the address it wants shown has to be one its own manifest declares.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry,
    extension::{AuthProgress, AuthProvider},
};

use crate::provider::{ProviderError, ProviderResult};

/// The installed authentication providers, one per claimed provider slug.
pub struct AuthProviders {
    plugins: Vec<Provider>,
    /// Provider slug the plugin claims — the same key resolver dispatch uses — to its index.
    /// A slug rather than a plugin id, because that is what an account carries.
    by_slug: HashMap<String, usize>,
}

struct Provider {
    manifest: PluginManifest,
    plugin: AuthProvider,
}

impl AuthProviders {
    /// Loads every installed authentication provider, skipping any that fails to build.
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
        let loaded = registry.instantiate(&PluginType::Auth, |package| {
            AuthProvider::new(package.manifest.clone(), &package.component, host.clone()).map(
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
                // A provider slug is what decides which plugin signs an account in. One that
                // claims nothing names no account it could run for, so it is left out rather
                // than offered for everything.
                tracing::warn!(
                    plugin = %provider.manifest.name,
                    "authentication plugin claims no provider and cannot be used"
                );
                continue;
            }
            let index = plugins.len();
            plugins.push(provider);
            for slug in claims {
                // The newest version of each plugin comes first, and the first claim of a
                // slug wins: two plugins claiming one provider is a conflict, not a chain.
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

    /// Whether a provider can be signed in to by a plugin rather than by typing a key.
    #[must_use]
    pub fn supports(&self, provider_slug: &str) -> bool {
        self.by_slug
            .contains_key(&provider_slug.to_ascii_lowercase())
    }

    /// The plugin id that would run a flow for this provider.
    #[must_use]
    pub fn plugin_id(&self, provider_slug: &str) -> Option<String> {
        self.provider(provider_slug)
            .ok()
            .map(|provider| provider.manifest.id.to_string())
    }

    /// Starts a flow for one account.
    pub async fn begin(
        &self,
        provider_slug: &str,
        account_id: AccountId,
        credential_ref: Option<&str>,
    ) -> ProviderResult<AuthProgress> {
        let provider = self.provider(provider_slug)?;
        let progress = provider.plugin.begin(account_id, credential_ref).await?;
        Ok(Self::checked(provider, progress))
    }

    /// Continues a flow the host started earlier.
    pub async fn poll(
        &self,
        provider_slug: &str,
        account_id: AccountId,
        flow_state: Option<&str>,
    ) -> ProviderResult<AuthProgress> {
        let provider = self.provider(provider_slug)?;
        let progress = provider.plugin.poll(account_id, flow_state).await?;
        Ok(Self::checked(provider, progress))
    }

    /// The plugin that claims this provider, or the reason there is none.
    ///
    /// The error is typed rather than prose because the sweep that calls this has to tell a
    /// provider nobody claims -- which waiting never fixes -- from a call that failed.
    fn provider(&self, provider_slug: &str) -> ProviderResult<&Provider> {
        self.by_slug
            .get(&provider_slug.to_ascii_lowercase())
            .and_then(|index| self.plugins.get(*index))
            .ok_or_else(|| ProviderError::no_plugin(provider_slug))
    }

    /// Refuses a verification address the plugin's own manifest does not cover.
    ///
    /// This is the phishing gate. A flow's whole purpose is to put an address in front of
    /// somebody and ask them to sign in there; a plugin that could name any address would be
    /// a signed, installed phishing page. The manifest already lists where it may talk, and
    /// that list is what the person saw before installing it.
    fn checked(provider: &Provider, progress: AuthProgress) -> AuthProgress {
        let AuthProgress::UserAction {
            verification_url,
            user_code,
            expires_in_seconds,
            flow_state,
        } = progress
        else {
            return progress;
        };
        if !declared(&provider.manifest, &verification_url) {
            tracing::warn!(
                plugin = %provider.manifest.name,
                "authentication plugin proposed a verification address outside its manifest"
            );
            return AuthProgress::Failed {
                message: "the plugin proposed a sign-in address it did not declare".to_owned(),
            };
        }
        AuthProgress::UserAction {
            verification_url,
            user_code,
            expires_in_seconds,
            flow_state,
        }
    }
}

/// Whether an address is https and on a host the manifest's `net_http` list covers.
pub(crate) fn declared(manifest: &PluginManifest, address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|url| {
        url.scheme() == "https" && rd_plugin_host::domain_allowed(&url, manifest.domains())
    })
}

#[cfg(test)]
mod tests {
    use rd_plugin_host::PluginManifest;

    use super::declared;

    fn manifest() -> PluginManifest {
        toml::from_str(
            r#"
            manifest_version = 3
            plugin_type = "auth"
            api_version = "0.9.0"
            id = "019d0000-0000-7000-8000-0000000001ff"
            name = "Demo"
            version = "0.1.0"
            key_id = "demo-v1"
            public_key = "AAAA"
            [capabilities.net_http]
            domains = ["api.example.com"]
            [extension]
            slug = "demo"
            claims = ["example"]
            [metadata]
            description = "d"
            author = "a"
            license = "MIT"
            min_app_version = "0.9.0"
            "#,
        )
        .expect("manifest")
    }

    #[test]
    fn a_sign_in_address_must_be_one_the_manifest_declared() {
        let manifest = manifest();
        assert!(declared(&manifest, "https://api.example.com/device"));
        // The whole risk of this plugin type in one line: an installed, signed plugin that
        // could name any address would be a phishing page with a signature on it.
        assert!(!declared(&manifest, "https://evil.example/device"));
        assert!(!declared(&manifest, "https://api.example.com.evil/device"));
        // Plain http would send whatever the person types there in the clear.
        assert!(!declared(&manifest, "http://api.example.com/device"));
        assert!(!declared(&manifest, "not a url"));
    }
}
