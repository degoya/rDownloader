//! The network side of the handle: clients, address rules, captchas, host blocks and holds.

use std::sync::Arc;

use anyhow::Result;
use rd_core::{AccountId, DownloadState, Failure};
use rd_http::SharedNetworkDefaults;
use url::Url;

use crate::{HoldSource, NetworkClient, SchedulerHandle, worker};

impl SchedulerHandle {
    /// The host capabilities the resolver chain runs on.
    ///
    /// The extension plugin types reach the network through the same host a resolver does,
    /// each narrowed to its own manifest, so this is handed on rather than a second host
    /// being built beside it with its own idea of what is allowed.
    #[must_use]
    pub fn plugin_host(&self) -> Arc<dyn rd_plugin_api::ResolverHost> {
        self.resolvers.host()
    }

    /// Resolver chain (the installed components) for link checks outside the scheduler.
    #[must_use]
    pub fn resolvers(&self) -> rd_plugin_host::ResolverService {
        self.resolvers.clone()
    }

    /// Client for one specific auth profile, used by the profile test action so a
    /// disabled or not-yet-approved profile can still be verified.
    pub async fn test_client(
        &self,
        scope: &Url,
        profile: rd_core::AuthProfile,
    ) -> Result<NetworkClient> {
        worker::build_test_client(self, profile, scope).await
    }

    /// The proxy and CA a tool is handed, for a run outside the queue: the channel monitor's
    /// probe (RD-1240-22).
    #[must_use]
    pub fn tool_network(&self) -> crate::ToolNetworkSource {
        crate::ToolNetworkSource::new(
            self.database.clone(),
            self.secrets.clone(),
            self.network_defaults.clone(),
        )
    }

    /// HTTP client honouring the global proxy/TLS defaults without an account identity,
    /// plus any credential headers of the auth profile matching `scope`.
    pub async fn direct_client(&self, scope: &Url) -> Result<NetworkClient> {
        worker::build_client(
            self,
            None,
            None,
            rd_core::AuthProfileSelection::Auto,
            scope,
            None,
        )
        .await
    }

    /// [`Self::direct_client`] held to `policy`: names are resolved through the guard at
    /// connect time and no redirect goes to a refused literal address (RD-150-03). For a
    /// request made on a stranger's word — a link a document or a page proposed.
    pub async fn guarded_client(
        &self,
        scope: &Url,
        policy: rd_http::AddressPolicy,
    ) -> Result<NetworkClient> {
        worker::build_client(
            self,
            None,
            None,
            rd_core::AuthProfileSelection::Auto,
            scope,
            Some(policy),
        )
        .await
    }

    /// The address rule for a request made on a stranger's word (RD-150-03): never this
    /// machine — its loopback and link-local addresses and the address the service listens
    /// on — and the person's own network only when `local_network`.
    #[must_use]
    pub fn remote_address_policy(&self, local_network: bool) -> rd_http::AddressPolicy {
        rd_http::AddressPolicy::new(local_network).listening_on(self.config.own_address)
    }

    /// The address rule a download's source rows were written with: the person's own network
    /// only when every row came from their own hand (`local_network`, decided at intake).
    pub(crate) fn source_address_policy(
        &self,
        sources: &[rd_core::DownloadSource],
    ) -> rd_http::AddressPolicy {
        let local_network =
            !sources.is_empty() && sources.iter().all(|source| source.local_network);
        self.remote_address_policy(local_network)
    }

    /// The rule a queued download's address keeps to, when a stranger's document or page
    /// proposed it or its mirrors (RD-150-03). `None` for a download without source rows,
    /// which is an address the person gave.
    pub(crate) async fn address_policy_for(
        &self,
        id: rd_core::DownloadId,
    ) -> Result<Option<rd_http::AddressPolicy>> {
        let sources = self.database.download_sources(id).await?;
        Ok((!sources.is_empty()).then(|| self.source_address_policy(&sources)))
    }

    /// The captcha broker resolvers hand their challenges to; REST handlers use it to list
    /// and answer the ones waiting for a person.
    #[must_use]
    pub fn captcha(&self) -> rd_captcha::CaptchaBroker {
        self.captcha.clone()
    }

    /// Holds back a hoster's free downloads until `until` after it reported an IP limit.
    pub(crate) fn block_host(&self, source: &url::Url, until: chrono::DateTime<chrono::Utc>) {
        self.host_blocks.block(source, until);
    }

    /// The hosters currently held back by an IP limit, soonest to free up first.
    #[must_use]
    pub fn blocked_hosts(&self) -> Vec<(String, chrono::DateTime<chrono::Utc>)> {
        self.host_blocks.active(chrono::Utc::now())
    }

    /// Forgets every IP limit, because they were tied to an address we no longer have.
    pub fn clear_host_blocks(&self) {
        self.host_blocks.clear();
    }

    /// Puts files that were waiting out an IP limit back in the queue.
    ///
    /// Only those: a file waiting for anything else still has its own reason to wait, and a
    /// reconnect says nothing about a hoster that refused the credentials or a server that
    /// was briefly unreachable.
    pub async fn requeue_ip_blocked(&self) -> anyhow::Result<usize> {
        let waiting = self.database.startable_downloads().await?;
        let mut requeued = 0;
        for file in waiting {
            if file.state != DownloadState::RetryWait {
                continue;
            }
            if !matches!(
                file.last_error.as_ref().map(|failure| &failure.category),
                Some(rd_core::FailureKind::IpBlocked { .. })
            ) {
                continue;
            }
            self.database
                .transition_download(file.id, DownloadState::Queued)
                .await?;
            requeued += 1;
        }
        Ok(requeued)
    }

    /// Holds the queue while the machine runs on battery or on a metered connection
    /// (RD-050-13), or while a reconnect is running. Running transfers keep going; only new
    /// starts wait.
    ///
    /// Each source owns its own hold: the power supervisor re-asserts its state every few
    /// seconds, and a single shared slot meant it cleared everybody else's on the way past.
    pub async fn set_network_hold(&self, source: HoldSource, reason: Option<&'static str>) {
        self.network_hold.set(source, reason).await;
    }

    /// Why the queue is currently held, if it is.
    pub async fn network_hold(&self) -> Option<&'static str> {
        self.network_hold.reason().await
    }

    /// The shared proxy/CA defaults, for transports that build their own connections.
    #[must_use]
    pub fn network_defaults(&self) -> SharedNetworkDefaults {
        self.network_defaults.clone()
    }

    /// Verifies a configured provider identity through its installed resolver.
    pub async fn check_account(
        &self,
        account_id: AccountId,
    ) -> Result<rd_plugin_host::AccountStatus, Failure> {
        self.resolvers.check_account(account_id).await
    }
}
