//! The ports the poller drives: feed fetching, the site rules in force, the vault, and the
//! script sandbox.
//!
//! Each is this crate's implementation of a trait `rd-subscription` declares, and each is
//! the one place a subsystem the poll loop must not know about is reached. Split out of
//! `subscription_service.rs` for size (RD-110-37); the types are unchanged and are
//! re-exported from the parent module, so every path into them stayed the same.

use std::sync::Arc;

/// Fetches feed documents through the scheduler's pooled client (RD-080-10).
///
/// Conditional by default: the validators the last poll stored are sent back, so an
/// unchanged feed costs a `304` rather than a full download. That is the difference between
/// polling a feed hourly being polite and being a nuisance.
pub struct HttpFeedFetcher {
    scheduler: rd_scheduler::SchedulerHandle,
}

impl HttpFeedFetcher {
    #[must_use]
    pub fn new(scheduler: rd_scheduler::SchedulerHandle) -> Self {
        Self { scheduler }
    }
}

#[async_trait::async_trait]
impl rd_subscription::FeedFetcher for HttpFeedFetcher {
    async fn fetch(
        &self,
        url: &url::Url,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> anyhow::Result<rd_subscription::FetchedFeed> {
        let network = self.scheduler.direct_client(url).await?;
        let mut headers: Vec<(String, String)> = network.headers.clone();
        if let Some(etag) = etag {
            headers.push(("if-none-match".to_owned(), etag.to_owned()));
        }
        if let Some(last_modified) = last_modified {
            headers.push(("if-modified-since".to_owned(), last_modified.to_owned()));
        }
        let response = rd_http::fetch_conditional(
            &network.client,
            url.clone(),
            &headers,
            rd_subscription::MAX_FEED_BYTES,
        )
        .await?;
        Ok(rd_subscription::FetchedFeed {
            body: response.body,
            etag: response.etag,
            last_modified: response.last_modified,
            final_url: Some(response.final_url),
        })
    }
}

/// The site rules a watched release page consults, as the poller can hold them (RD-110-21).
///
/// A cell rather than the catalogue itself, because of the order things are built in: the
/// poll loop starts inside `AppState::new`, and the rules arrive afterwards with the
/// crawlers, which are discovered from the plugin directory rather than configured. Until
/// they do, nothing is claimed — a poll then finds no release page on the listing, which is
/// the honest answer for an installation whose rules have not loaded.
#[derive(Clone, Default)]
pub struct SharedSiteRules {
    rules: Arc<std::sync::RwLock<Option<Arc<rd_plugin_ext::SiteRules>>>>,
}

impl SharedSiteRules {
    /// Puts the rules in force, or takes them away again.
    pub fn set(&self, rules: Option<Arc<rd_plugin_ext::SiteRules>>) {
        let mut held = self
            .rules
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *held = rules;
    }
}

impl rd_subscription::ClaimedAddresses for SharedSiteRules {
    fn claims(&self, url: &url::Url) -> bool {
        self.rules
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|rules| rules.claims(url))
    }
}

/// Resolves a subscription's stored API key out of the vault (RD-080-11).
///
/// The key is fetched at the moment a request is built and lives no longer than that call;
/// nothing here holds it, and the reference itself is opaque.
pub struct VaultSecretResolver {
    secrets: rd_secrets::SecretStore,
}

impl VaultSecretResolver {
    #[must_use]
    pub fn new(secrets: rd_secrets::SecretStore) -> Self {
        Self { secrets }
    }
}

#[async_trait::async_trait]
impl rd_subscription::SecretResolver for VaultSecretResolver {
    async fn resolve(&self, reference: &str) -> anyhow::Result<String> {
        use secrecy::ExposeSecret;

        let secret = self.secrets.get(reference).await?;
        Ok(secret.expose_secret().to_owned())
    }
}

/// Runs a script subscription's script through the post-processing sandbox (RD-130-19).
///
/// The one road from a subscription to a process on this machine, and deliberately the
/// same one post-processing and automation scripts take: the scripts directory, the name
/// rules, the timeout and no shell. What the script learns about the subscription is its id
/// and name; everything else it needs, it knows itself.
pub struct SandboxScriptRunner {
    extraction: rd_extract::ExtractionService,
}

impl SandboxScriptRunner {
    #[must_use]
    pub fn new(extraction: rd_extract::ExtractionService) -> Self {
        Self { extraction }
    }
}

#[async_trait::async_trait]
impl rd_subscription::ScriptRunner for SandboxScriptRunner {
    async fn run(
        &self,
        name: &str,
        subscription: &rd_core::Subscription,
    ) -> anyhow::Result<String> {
        self.extraction
            .run_output_script(
                name,
                vec![
                    ("RD_SUBSCRIPTION_ID".to_owned(), subscription.id.to_string()),
                    ("RD_SUBSCRIPTION_NAME".to_owned(), subscription.name.clone()),
                ],
            )
            .await
    }
}
