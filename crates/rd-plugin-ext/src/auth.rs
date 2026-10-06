//! Authentication provider plugins (RD-090-13).
//!
//! A plugin runs the flow; the core owns everything about it that matters. Which plugin runs
//! for an account is decided by the provider it claims, the waiting between polls is the
//! host's, the token goes into the vault through a host function the plugin cannot read back,
//! and the address it wants shown has to be one its own manifest declares.

use rd_core::AccountId;
use rd_plugin_host::{
    PluginManifest,
    extension::{AuthProgress, AuthProvider},
};

use crate::{ClaimedPlugins, plugin_set::Installed, provider::ProviderResult};

/// The installed authentication providers, one per claimed provider slug.
pub type AuthProviders = ClaimedPlugins<AuthProvider>;

type Provider = Installed<AuthProvider>;

impl AuthProviders {
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
            api_version = "0.10.0"
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
