//! Capability enforcement for the built-in resolvers.
//!
//! A component is confined twice over: the import allowlist decides which host interfaces
//! it may link at all, and the store carries the manifest's domain list into every call.
//! The built-in resolvers share one `NativeHost`, so before this they were confined only by
//! the provider registry — a union across *all* providers — and by nothing at all for
//! cookies and captchas. "Every grant is enforced at runtime" was therefore true of one
//! code path and not of the other, for the same hoster logic.
//!
//! [`GrantedHost`] closes that: it wraps the shared host in one plugin's own manifest, so a
//! built-in resolver reaches the same set of things its packaged twin does, and refuses with
//! the same failures.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    CaptchaAnswer, CaptchaChallenge, ClientIdentity, HostHttpRequest, HostHttpResponse,
    ResolverHost,
};
use url::Url;

use crate::{Capabilities, PluginManifest, captcha_target_refused, domain_allowed};

/// One plugin's view of the shared native host, narrowed to what its manifest declares.
pub(crate) struct GrantedHost {
    inner: Arc<dyn ResolverHost>,
    capabilities: Capabilities,
    /// Named in the refusals, so a log line says which plugin overstepped.
    plugin: String,
}

impl GrantedHost {
    /// Wraps `inner` in the grants of the plugin whose bundled manifest is `manifest_toml`.
    ///
    /// Returns the trait object rather than `Self`, because a caller has no reason to hold a
    /// narrowed host as anything but a host.
    ///
    /// # Panics
    ///
    /// The manifest is embedded at compile time and covered by the bundled manifest tests,
    /// so a parse failure is a build defect rather than a runtime condition.
    pub(crate) fn wrap(inner: Arc<dyn ResolverHost>, manifest_toml: &str) -> Arc<dyn ResolverHost> {
        let manifest: PluginManifest =
            toml::from_str(manifest_toml).expect("bundled plugin manifest is well-formed");
        Self::confine(inner, &manifest)
    }

    /// Narrows `inner` to an already parsed manifest.
    ///
    /// The extension types take this route: their manifest arrives with the installed
    /// package rather than compiled in, but what confines them has to be the same code, or
    /// "every grant is enforced at runtime" would again be true of one path and not another.
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
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.secret_target_not_allowed",
                format!("{} does not declare the secret {reference}", self.plugin),
            ));
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
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use rd_plugin_api::{HostHttpResponse, WidgetChallenge};

    use super::*;

    /// Records whether a call made it through the grant check to the real host.
    #[derive(Default)]
    struct Recording {
        reached: AtomicBool,
    }

    #[async_trait]
    impl ResolverHost for Recording {
        async fn http_request(
            &self,
            _client: &ClientIdentity,
            request: HostHttpRequest,
        ) -> Result<HostHttpResponse, Failure> {
            self.reached.store(true, Ordering::SeqCst);
            Ok(HostHttpResponse {
                status: 200,
                final_url: request.url,
                headers: Vec::new(),
                body: Vec::new(),
            })
        }

        async fn cookies_get(&self, _account_id: AccountId, _url: &Url) -> Vec<(String, String)> {
            self.reached.store(true, Ordering::SeqCst);
            vec![("session".to_owned(), "value".to_owned())]
        }

        async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
            self.reached.store(true, Ordering::SeqCst);
            true
        }

        async fn solve_captcha(
            &self,
            _client: &ClientIdentity,
            _challenge: CaptchaChallenge,
            _time_limit: Duration,
        ) -> Result<CaptchaAnswer, Failure> {
            self.reached.store(true, Ordering::SeqCst);
            Ok(CaptchaAnswer::Token("solved".to_owned()))
        }
    }

    fn identity() -> ClientIdentity {
        ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        }
    }

    fn challenge() -> CaptchaChallenge {
        CaptchaChallenge::RecaptchaV2(WidgetChallenge {
            site_key: "key".to_owned(),
            page_url: "https://ddownload.com/".to_owned(),
            invisible: false,
        })
    }

    fn request(url: &str) -> HostHttpRequest {
        HostHttpRequest {
            method: "GET".to_owned(),
            url: url.parse().expect("url"),
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
            granted_secret: None,
            authority: rd_plugin_api::RequestAuthority::Provider,
            write_methods: false,
        }
    }

    fn wrap(manifest: &str) -> (Arc<Recording>, Arc<dyn ResolverHost>) {
        let inner = Arc::new(Recording::default());
        let host = GrantedHost::wrap(inner.clone(), manifest);
        (inner, host)
    }

    /// Premiumize is an API-key multihoster: no captcha, no cookies. The component of the
    /// same plugin cannot even link those interfaces, and the built-in resolver must not be
    /// able to reach past them either.
    #[tokio::test]
    async fn a_plugin_without_the_captcha_grant_is_refused_on_the_native_path_too() {
        let (inner, host) = wrap(rd_plugin_premiumize::MANIFEST);
        let failure = host
            .solve_captcha(&identity(), challenge(), Duration::from_secs(60))
            .await
            .expect_err("premiumize declares no captcha capability");
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.capability_not_granted")
        );
        assert!(!inner.reached.load(Ordering::SeqCst));
    }

    /// A widget captcha ends with a person looking at that page in the desktop agent's
    /// WebView (RD-107-03). A plugin that may reach its own hoster must not be able to point
    /// that window at somebody else's site, so the page is held to the manifest's domains.
    #[tokio::test]
    async fn a_widget_captcha_page_outside_the_plugins_domains_is_refused() {
        let (inner, host) = wrap(rd_plugin_ddownload::MANIFEST);
        let elsewhere = CaptchaChallenge::Turnstile(WidgetChallenge {
            site_key: "0x4AAA".to_owned(),
            page_url: "https://www.premiumize.me/login".to_owned(),
            invisible: false,
        });

        let failure = host
            .solve_captcha(&identity(), elsewhere, Duration::from_secs(60))
            .await
            .expect_err("outside ddownload's declared domains");

        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.captcha_target_not_allowed")
        );
        assert!(
            !inner.reached.load(Ordering::SeqCst),
            "the broker must never see a challenge the manifest does not cover"
        );
    }

    /// The other direction: the hoster's own page still reaches the broker, and an image
    /// challenge — which names no page at all — is untouched by the check.
    #[tokio::test]
    async fn the_plugins_own_page_and_an_image_challenge_still_reach_the_broker() {
        let (inner, host) = wrap(rd_plugin_ddownload::MANIFEST);

        host.solve_captcha(&identity(), challenge(), Duration::from_secs(60))
            .await
            .expect("ddownload's own page passes");
        assert!(inner.reached.load(Ordering::SeqCst));

        let image = CaptchaChallenge::Image(rd_plugin_api::ImageChallenge {
            mime: "image/png".to_owned(),
            data: b"x".to_vec(),
            prompt: None,
        });
        host.solve_captcha(&identity(), image, Duration::from_secs(60))
            .await
            .expect("an image challenge names no page");
    }

    #[tokio::test]
    async fn a_plugin_without_the_cookies_grant_reads_no_cookies() {
        let (inner, host) = wrap(rd_plugin_premiumize::MANIFEST);
        let cookies = host
            .cookies_get(
                AccountId::new(),
                &"https://www.premiumize.me/".parse().expect("url"),
            )
            .await;
        assert!(cookies.is_empty());
        assert!(!inner.reached.load(Ordering::SeqCst));
    }

    /// The registry gate underneath allows every bundled plugin's domains at once, so
    /// without a per-plugin check ddownload could have reached premiumize's API.
    #[tokio::test]
    async fn a_request_outside_the_plugins_own_domains_is_refused() {
        let (inner, host) = wrap(rd_plugin_ddownload::MANIFEST);
        let failure = host
            .http_request(
                &identity(),
                request("https://www.premiumize.me/api/account/info"),
            )
            .await
            .expect_err("outside ddownload's declared domains");
        assert_eq!(
            failure.code.as_deref(),
            Some("plugin.http_target_not_allowed")
        );
        assert!(!inner.reached.load(Ordering::SeqCst));

        host.http_request(&identity(), request("https://ddownload.com/file"))
            .await
            .expect("its own domain still passes");
        assert!(inner.reached.load(Ordering::SeqCst));
    }

    /// The free flows reach hosts no link is ever posted for — an API endpoint, a CDN. Those
    /// live in `net_http` and nowhere else, so the per-plugin gate has to allow them.
    #[tokio::test]
    async fn the_free_flows_api_and_delivery_hosts_stay_requestable() {
        let (_, onefichier) = wrap(rd_plugin_onefichier::MANIFEST);
        onefichier
            .http_request(
                &identity(),
                request("https://api.1fichier.com/v1/download/get_token.cgi"),
            )
            .await
            .expect("1fichier's API host is granted");

        let (_, ddownload) = wrap(rd_plugin_ddownload::MANIFEST);
        ddownload
            .http_request(&identity(), request("https://s12.zeuscdn.org/file.bin"))
            .await
            .expect("ddownload's delivery wildcard is granted");
    }

    #[tokio::test]
    async fn only_the_granted_secret_reference_is_visible() {
        let (inner, host) = wrap(rd_plugin_ddownload::MANIFEST);
        assert!(
            !host
                .secret_available(AccountId::new(), "premiumize_api_key")
                .await
        );
        assert!(!inner.reached.load(Ordering::SeqCst));
        assert!(
            host.secret_available(AccountId::new(), "ddownload_api_key")
                .await
        );
        assert!(inner.reached.load(Ordering::SeqCst));
    }
}
