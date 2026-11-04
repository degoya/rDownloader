//! Capability enforcement on the application's own host, per plugin.
//!
//! A component is confined twice over: the import allowlist decides which host interfaces
//! it may link at all, and the store carries the manifest's domain list into every call.
//! The extension plugin types are handed the shared `NativeHost` itself, and without a
//! narrowing of their own they would be confined only by the provider registry — a union
//! across *all* providers — and by nothing at all for cookies and captchas.
//!
//! [`GrantedHost`] closes that: it wraps the shared host in one plugin's own manifest, so an
//! extension reaches the same set of things a resolver component of the same manifest does,
//! and refuses with the same failures. Until RD-150-18 it confined the built-in resolvers
//! too; nothing is compiled in any more.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CaptchaAnswer, CaptchaChallenge, ClientIdentity, HostHttpRequest, HostHttpResponse,
    ResolverHost,
};
use url::Url;

use crate::{Capabilities, PluginManifest, captcha_target_refused, domain_allowed};

/// The prefix of a reference the vault minted itself, as opposed to a manifest slot name.
const HOST_MINTED_PREFIX: &str = "vault://";

/// One plugin's view of the shared native host, narrowed to what its manifest declares.
pub(crate) struct GrantedHost {
    inner: Arc<dyn ResolverHost>,
    capabilities: Capabilities,
    /// Named in the refusals, so a log line says which plugin overstepped.
    plugin: String,
}

impl GrantedHost {
    /// Narrows `inner` to what `manifest` declares.
    ///
    /// The extension types take this route: their manifest arrives with the installed
    /// package, and what confines them has to be the same code for every plugin, or "every
    /// grant is enforced at runtime" would be true of one path and not another.
    pub(crate) fn confine(
        inner: Arc<dyn ResolverHost>,
        manifest: &PluginManifest,
    ) -> Arc<dyn ResolverHost> {
        Arc::new(Self {
            inner,
            capabilities: manifest.capabilities.clone(),
            plugin: manifest.name.clone(),
        })
    }

    /// The refusal for a plugin that may compute over a credential it could not send to
    /// everywhere it can reach.
    fn reach_too_wide(&self, reference: &str) -> Failure {
        Failure::coded(
            FailureKind::Permanent,
            "plugin.key_derivation_reach_too_wide",
            format!(
                "{} reaches beyond the domains the secret {reference} may be sent to",
                self.plugin
            ),
        )
    }

    /// Whether every vault reference the request names is one this manifest declares.
    ///
    /// The same grant `secret_available` and `derive_from_secret` check, applied to the
    /// `{{secret:…}}` and `{{basic:…}}` markers of an outgoing request. A host-minted
    /// reference (`vault://…`) cannot be written into a manifest at all; it is what an OAuth
    /// plugin is handed back as its renewal material, and the host underneath accepts it only
    /// where it is exactly the stored renewal reference of the invocation's own account.
    fn check_named_secrets(&self, request: &HostHttpRequest) -> Result<(), Failure> {
        for reference in super::references::secret_references(request)? {
            if reference.starts_with(HOST_MINTED_PREFIX) {
                continue;
            }
            if !self
                .capabilities
                .secrets
                .iter()
                .any(|granted| granted == reference)
            {
                return Err(self.secret_not_declared(reference));
            }
        }
        Ok(())
    }

    fn secret_not_declared(&self, reference: &str) -> Failure {
        Failure::coded(
            FailureKind::Permanent,
            "plugin.secret_target_not_allowed",
            format!("{} does not declare the secret {reference}", self.plugin),
        )
    }

    fn refuse(&self, capability: &str) -> Failure {
        Failure::coded(
            FailureKind::Unsupported,
            "plugin.capability_not_granted",
            format!(
                "{} does not declare the {capability} capability",
                self.plugin
            ),
        )
    }
}

#[async_trait]
impl ResolverHost for GrantedHost {
    async fn http_request(
        &self,
        client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        if self.capabilities.net_http.is_none() {
            return Err(self.refuse("net_http"));
        }
        // The registry gate underneath allows the union of every provider's domains, which
        // is wider than this plugin's own list. Checking here is what makes the manifest the
        // authority — the same check `ComponentResolver` applies to its store's domains.
        if !domain_allowed(&request.url, self.capabilities.domains()) {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.http_target_not_allowed",
                "Resolver request target is outside the plugin's declared domains",
            ));
        }
        self.check_named_secrets(&request)?;
        self.inner.http_request(client, request).await
    }

    async fn cookies_get(&self, account_id: AccountId, url: &Url) -> Vec<(String, String)> {
        if !self.capabilities.cookies {
            return Vec::new();
        }
        self.inner.cookies_get(account_id, url).await
    }

    async fn secret_available(&self, account_id: AccountId, reference: &str) -> bool {
        if !self
            .capabilities
            .secrets
            .iter()
            .any(|granted| granted == reference)
        {
            return false;
        }
        self.inner.secret_available(account_id, reference).await
    }

    /// Two refusals the shared host cannot make, because both are about this manifest
    /// (RD-120-20).
    ///
    /// The first is the grant itself. The second is the reach, and it is the one that keeps
    /// the primitive from widening anything: a credential may be *sent* only to the domains
    /// its slot names, and derived material is not gated that way -- once it is bytes in the
    /// guest's memory, the only thing deciding where it can go is this plugin's own
    /// `net_http` list. So a plugin may compute over a credential exactly when everything it
    /// can reach is somewhere that credential could itself have been sent. A plugin that
    /// declares a wider reach than the slot does is refused rather than narrowed, because
    /// narrowing would silently disable the request the author wrote.
    async fn derive_from_secret(
        &self,
        client: &ClientIdentity,
        reference: &str,
        steps: &[rd_plugin_api::DerivationStep],
    ) -> Result<Vec<u8>, Failure> {
        if !self.capabilities.key_derivation {
            return Err(self.refuse("key_derivation"));
        }
        if !self
            .capabilities
            .secrets
            .iter()
            .any(|granted| granted == reference)
        {
            return Err(self.secret_not_declared(reference));
        }
        // Compared as patterns (RD-120-30). A wildcard reach used to be judged by its bare
        // suffix, which answered for one host the plugin could reach and none of the others;
        // now it passes only where the slot names that wildcard, or a wider one, too.
        for domain in self.capabilities.domains() {
            if !rd_provider_registry::secret_reach_allowed(reference, domain) {
                return Err(self.reach_too_wide(reference));
            }
        }
        self.inner
            .derive_from_secret(client, reference, steps)
            .await
    }

    async fn store_token(&self, account_id: AccountId, value: &str) -> Result<(), Failure> {
        // Not gated on a capability: writing a token is what an authentication plugin is,
        // and the interface is linked only into that world. What confines it is underneath —
        // the host writes only into the reference the account's own provider owns.
        self.inner.store_token(account_id, value).await
    }

    /// The OAuth sibling of `store_token`, and delegated for the same reason (RD-105-01).
    ///
    /// Without this it fell through to the trait's default, which refuses — so every OAuth
    /// plugin reached the end of a successful exchange and was told the host could not keep
    /// what it had just obtained. The default is there for hosts that genuinely have no
    /// vault; a `GrantedHost` wraps one that does, and hiding it was an oversight rather than
    /// a policy.
    async fn store_oauth_token(
        &self,
        account_id: AccountId,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in_seconds: Option<u64>,
    ) -> Result<(), Failure> {
        self.inner
            .store_oauth_token(account_id, access_token, refresh_token, expires_in_seconds)
            .await
    }

    /// Delegated like its two siblings (RD-150-09). What confines it is underneath: the host
    /// writes only into a part slot the account's own provider declares.
    async fn store_flow_secret(
        &self,
        account_id: AccountId,
        name: &str,
        value: &str,
    ) -> Result<(), Failure> {
        self.inner.store_flow_secret(account_id, name, value).await
    }

    async fn wait(&self, client: &ClientIdentity, seconds: u32) -> Result<(), Failure> {
        self.inner.wait(client, seconds).await
    }

    async fn captcha_allowance(&self) -> Duration {
        self.inner.captcha_allowance().await
    }

    // The clock is no capability, so nothing to check; but it has to be forwarded, or the
    // trait's default reads the system clock past a host that set its own (RD-120-67).
    fn now_unix_seconds(&self) -> u64 {
        self.inner.now_unix_seconds()
    }

    async fn solve_captcha(
        &self,
        client: &ClientIdentity,
        challenge: CaptchaChallenge,
        time_limit: Duration,
    ) -> Result<CaptchaAnswer, Failure> {
        if !self.capabilities.captcha {
            return Err(self.refuse("captcha"));
        }
        // A widget challenge ends with a person looking at that page in their browser, so it
        // is an outbound target and is held to the manifest boundary the `http_request` arm
        // above applies (RD-107-03).
        if let Some(page) = challenge.page_url() {
            let allowed = Url::parse(page)
                .is_ok_and(|page| domain_allowed(&page, self.capabilities.domains()));
            if !allowed {
                return Err(captcha_target_refused());
            }
        }
        self.inner
            .solve_captcha(client, challenge, time_limit)
            .await
    }
}

#[cfg(test)]
#[path = "granted_tests.rs"]
mod tests;
