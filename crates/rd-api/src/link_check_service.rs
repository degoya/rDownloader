//! Background online check for LinkGrabber candidates (provider API or direct probe).

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use rd_core::{Account, BatchId, CandidateId, LinkCandidate, LinkCheckResult, LinkStatus};
use rd_db::Database;
use rd_scheduler::SchedulerHandle;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{hosters, link_check_cache, link_check_probe::probe_direct};

/// Stable code: a link a document or a page proposed points at this machine or, for one the
/// person did not hand over themselves, into their network; it was not requested (RD-150-03).
pub(crate) const CODE_CHECK_INTERNAL_ADDRESS: &str = "collector.check_internal_address";

enum CheckJob {
    Candidates(Vec<CandidateId>),
    Batch(BatchId),
}

struct Inner {
    database: Database,
    scheduler: SchedulerHandle,
    media_probe: Arc<dyn rd_media::MediaProbe>,
    ftp: rd_ftp::FtpService,
    sftp: rd_sftp::SftpService,
    object_storage: rd_object_storage::ObjectStorageService,
    /// Keeps a re-routed torrent the check read for its download (RD-130-18).
    torrent: rd_torrent::TorrentService,
    jobs: mpsc::Sender<CheckJob>,
    /// Announces a finished batch check. Lets a caller act on the outcome — the subscription
    /// auto-queue does — without the check service having to know who is waiting.
    completed: broadcast::Sender<BatchId>,
    cancellation: CancellationToken,
    /// Where the enricher plugins live, and the host they reach the outside world through.
    plugins: rd_plugin_host::PluginInstaller,
    plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    /// Installed metadata enrichers (RD-090-14), compiled on first use. An installation with
    /// none — or with enrichment switched off — never pays for compiling one.
    enrichers: tokio::sync::OnceCell<Arc<rd_plugin_ext::MetadataEnrichers>>,
    /// The remote-job plugins asked whether a provider's cache holds a link (RD-130-11).
    /// Attached after start, because that service is built with this one; unset, no cache
    /// is asked and the check is what it was before.
    cache_checkers: std::sync::OnceLock<crate::remote_job_service::RemoteJobService>,
}

/// Cloneable handle; checks run sequentially on one background task.
#[derive(Clone)]
pub struct LinkCheckService {
    inner: Arc<Inner>,
}

impl LinkCheckService {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        database: Database,
        scheduler: SchedulerHandle,
        media_probe: Arc<dyn rd_media::MediaProbe>,
        ftp: rd_ftp::FtpService,
        sftp: rd_sftp::SftpService,
        object_storage: rd_object_storage::ObjectStorageService,
        torrent: rd_torrent::TorrentService,
        plugins: rd_plugin_host::PluginInstaller,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(512);
        let service = Self {
            inner: Arc::new(Inner {
                database,
                scheduler,
                media_probe,
                ftp,
                sftp,
                object_storage,
                torrent,
                jobs: sender,
                completed: broadcast::channel(64).0,
                cancellation: CancellationToken::new(),
                plugins,
                plugin_host,
                enrichers: tokio::sync::OnceCell::new(),
                cache_checkers: std::sync::OnceLock::new(),
            }),
        };
        tokio::spawn(service.clone().run(receiver));
        service
    }

    /// Checks the given candidates (ignored while the service is shutting down).
    pub async fn check(&self, ids: Vec<CandidateId>) {
        if !ids.is_empty() {
            let _ = self.inner.jobs.send(CheckJob::Candidates(ids)).await;
        }
    }

    /// Checks every open candidate of a batch and regroups its packages afterwards.
    pub async fn check_batch(&self, batch_id: BatchId) {
        let _ = self.inner.jobs.send(CheckJob::Batch(batch_id)).await;
    }

    /// Lets the check ask the remote-job plugins' caches (RD-130-11). Only the first call
    /// counts.
    pub fn attach_cache_checkers(&self, checkers: crate::remote_job_service::RemoteJobService) {
        let _ = self.inner.cache_checkers.set(checkers);
    }

    /// Receives the id of every batch whose check has finished, successfully or not.
    #[must_use]
    pub fn subscribe_completed(&self) -> broadcast::Receiver<BatchId> {
        self.inner.completed.subscribe()
    }

    pub fn shutdown(&self) {
        self.inner.cancellation.cancel();
    }

    async fn run(self, mut receiver: mpsc::Receiver<CheckJob>) {
        loop {
            let job = tokio::select! {
                () = self.inner.cancellation.cancelled() => return,
                job = receiver.recv() => match job {
                    Some(job) => job,
                    None => return,
                },
            };
            let batch = match &job {
                CheckJob::Batch(id) => Some(*id),
                CheckJob::Candidates(_) => None,
            };
            if let Err(error) = self.process(job).await {
                tracing::warn!(%error, "link check failed");
            }
            // Announced even when the check failed: the candidates have reached a final state
            // either way, and a waiter that never hears back is worse than one that finds
            // nothing enqueueable.
            if let Some(batch) = batch {
                let _ = self.inner.completed.send(batch);
            }
        }
    }

    async fn process(&self, job: CheckJob) -> Result<()> {
        let (ids, batch_id) = match job {
            CheckJob::Candidates(ids) => (ids, None),
            CheckJob::Batch(batch_id) => (
                self.inner
                    .database
                    .list_candidates()
                    .await?
                    .into_iter()
                    .filter(|candidate| candidate.batch_id == batch_id)
                    .map(|candidate| candidate.id)
                    .collect(),
                Some(batch_id),
            ),
        };
        let claimed = self.inner.database.claim_candidates_for_check(ids).await?;
        if claimed.is_empty() {
            return Ok(());
        }
        let (claimed, guards) = self.screen_remote(claimed).await?;
        if claimed.is_empty() {
            if let Some(batch_id) = batch_id {
                self.inner
                    .database
                    .regroup_collector_batches(vec![batch_id])
                    .await?;
            }
            return Ok(());
        }
        let accounts: Vec<Account> = self
            .inner
            .database
            .list_accounts()
            .await?
            .into_iter()
            .filter(|account| account.enabled)
            .collect();
        let resolvers = self.inner.scheduler.resolvers();
        let mut catalogues = Vec::with_capacity(accounts.len());
        for account in &accounts {
            catalogues.push((account.id, hosters::catalogue(&resolvers, account.id).await));
        }
        // Which provider's cache holds what, asked before any link is checked and merged into
        // each result as it is written (RD-130-11).
        let hints = match self.inner.cache_checkers.get() {
            Some(checkers) => {
                link_check_cache::hints(checkers, &self.inner.database, &claimed, &accounts).await
            }
            None => HashMap::new(),
        };

        // The candidate travels with whether *its own* hoster has an account, because that is
        // what an inconclusive answer has to be read against: a check that ran through a
        // covering multihoster, or through no account of this hoster at all, is inconclusive
        // for a reason the reader can do something about (RD-109-43).
        let mut by_account: HashMap<rd_core::AccountId, Vec<(LinkCandidate, bool)>> =
            HashMap::new();
        let mut direct = Vec::new();
        let mut unsupported = Vec::new();
        let mut media = Vec::new();
        let mut gallery = Vec::new();
        let mut remote = Vec::new();
        for candidate in claimed {
            let provider = candidate.provider.as_deref().unwrap_or("direct_http");
            if provider == rd_core::MEDIA_PROVIDER {
                media.push(candidate);
                continue;
            }
            if matches!(
                provider,
                rd_core::FTP_PROVIDER
                    | rd_core::SFTP_PROVIDER
                    | rd_core::WEBDAV_PROVIDER
                    | rd_core::OBJECT_STORAGE_PROVIDER
            ) {
                remote.push(candidate);
                continue;
            }
            if provider == rd_core::GALLERY_PROVIDER
                || provider == rd_core::TORRENT_PROVIDER
                // An NZB is fetched and parsed at enqueue time, not probed here: reading it
                // twice would double the load on an indexer for no new information.
                || provider == rd_core::NZB_PROVIDER
                // A live manifest was already probed and classified by an earlier check;
                // re-opening the stream would only tell us what we know (RD-080-06).
                || provider == rd_core::RECORD_PROVIDER
            {
                gallery.push(candidate);
                continue;
            }
            let own_account = accounts
                .iter()
                .find(|account| account.provider.eq_ignore_ascii_case(provider))
                .map(|account| account.id);
            let covering_account = catalogues
                .iter()
                .find(|(_, hosters)| hosters::supports(hosters, &candidate.url))
                .map(|(id, _)| *id);
            if let Some(account) = own_account.or(covering_account) {
                by_account
                    .entry(account)
                    .or_default()
                    .push((candidate, own_account.is_some()));
            } else if provider == "direct_http" {
                direct.push(candidate);
            } else {
                unsupported.push(candidate);
            }
        }
        for (account, candidates) in by_account {
            let checked_by = accounts
                .iter()
                .find(|known| known.id == account)
                .map(|known| known.provider.to_ascii_lowercase());
            let urls: Vec<url::Url> = candidates.iter().map(|(c, _)| c.url.clone()).collect();
            match self.inner.scheduler.resolvers().check(account, urls).await {
                Ok(results) => {
                    for (candidate, own_account) in candidates {
                        let result = results.iter().find(|r| r.url == candidate.url).cloned();
                        let message = provider_message(result.as_ref(), own_account);
                        self.record_with_cache(
                            &candidate,
                            result,
                            message,
                            hints.get(&candidate.id),
                            checked_by.as_deref(),
                        )
                        .await;
                    }
                }
                Err(failure) => {
                    // The whole batch failed: the failure already carries its own code, and
                    // flattening it to prose here would throw that away.
                    for (candidate, _) in candidates {
                        self.record(&candidate, None, Some((&failure).into())).await;
                    }
                }
            }
        }
        if !direct.is_empty() {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
            let mut tasks = Vec::with_capacity(direct.len());
            for candidate in direct {
                let permit = Arc::clone(&semaphore).acquire_owned().await?;
                let scheduler = self.inner.scheduler.clone();
                let guard = guards.get(&candidate.id).cloned();
                tasks.push(tokio::spawn(async move {
                    let _permit = permit;
                    let network = match guard {
                        Some(policy) => scheduler.guarded_client(&candidate.url, policy).await,
                        None => scheduler.direct_client(&candidate.url).await,
                    };
                    let result = match network {
                        Ok(network) => Some(
                            probe_direct(&network.client, &network.headers, candidate.url.clone())
                                .await,
                        ),
                        Err(_) => None,
                    };
                    (candidate, result)
                }));
            }
            for task in tasks {
                if let Ok((candidate, result)) = task.await {
                    // An indexer link often hides the extension behind a query — NZBHydra
                    // and Prowlarr both do — so the declared content type is what identifies
                    // it (RD-080-11). Asked first since RD-130-18: a torrent served as
                    // `application/octet-stream` is plausible as a manifest too, and reading
                    // it for one would be a grab of its own.
                    let guard = guards.get(&candidate.id);
                    let rerouted = self.reclassify_document(&candidate, guard).await;
                    // A manifest does not have to announce itself in its address: a signed
                    // CDN link has no extension and is often served as `text/plain`
                    // (RD-080-06). Reclassifying here is what keeps it from being queued as
                    // a text file.
                    if matches!(rerouted, Rerouted::No)
                        && result.is_some()
                        && self.classify_manifest(&candidate, false, guard).await
                    {
                        continue;
                    }
                    let result = rerouted_document(&rerouted, result);
                    let message = direct_message(result.as_ref());
                    self.record(&candidate, result, message).await;
                }
            }
        }
        for candidate in media {
            let guard = guards.get(&candidate.id).cloned();
            self.check_media(candidate, guard.as_ref()).await;
        }
        for candidate in remote {
            self.check_remote(candidate).await;
        }
        // Enumerating a gallery means crawling it, and probing a torrent means joining the
        // swarm — both cost as much as the download, so these links pass without a probe.
        for candidate in gallery {
            let result = LinkCheckResult {
                url: candidate.url.clone(),
                status: LinkStatus::Online,
                file_name: None,
                size: None,
                media: None,
            };
            // A cache that holds the torrent or the NZB raises it to `cached` (RD-130-11).
            self.record_with_cache(
                &candidate,
                Some(result),
                None,
                hints.get(&candidate.id),
                None,
            )
            .await;
        }
        // These links were never checked, so they must not be presented as online: checking
        // a hoster link needs an account. Downloading it may still work when a resolver
        // offers a free path, so the message distinguishes the two cases.
        for candidate in unsupported {
            let message = if resolvers.has_free_resolver(&candidate.url) {
                rd_core::CandidateMessage::coded(
                    "collector.check_not_checked_free",
                    "Not checked (no account); a free download will be attempted",
                )
            } else {
                rd_core::CandidateMessage::coded(
                    "collector.check_no_source",
                    "No check source for this hoster (account missing)",
                )
            };
            // Another provider's cache may hold the file all the same (RD-130-11): the state and
            // the message stay, and the answer is stamped next to them.
            let cached_by = link_check_cache::unsupported_cached_by(hints.get(&candidate.id));
            if let Err(problem) = self
                .inner
                .database
                .mark_candidate_unsupported(candidate.id, message, cached_by)
                .await
            {
                tracing::warn!(candidate_id = %candidate.id, %problem, "could not store link check");
            }
        }
        if let Some(batch_id) = batch_id {
            self.inner
                .database
                .regroup_collector_batches(vec![batch_id])
                .await?;
        }
        Ok(())
    }

    /// Holds the links a document or a page proposed to their address rule (RD-150-03).
    ///
    /// A candidate the person added themselves passes as it always did. One a Metalink, an
    /// intake parser or a crawler on a stranger's page proposed is checked before anything
    /// asks for it: an address that is, or resolves to, this machine — or the person's own
    /// network, unless they handed the document over themselves — is marked with
    /// `collector.check_internal_address` and never requested. The others go on with their
    /// rule, which the HTTP probes below take into a guarded client, so a name that answers
    /// differently when the connection is made is refused there too. The yt-dlp probe and the
    /// FTP, SFTP, WebDAV and bucket checks open their own connections and have the check
    /// before the request only.
    async fn screen_remote(
        &self,
        claimed: Vec<LinkCandidate>,
    ) -> Result<(
        Vec<LinkCandidate>,
        HashMap<CandidateId, rd_http::AddressPolicy>,
    )> {
        let reaches = self.inner.database.candidates_remote_reach().await?;
        let mut guards = HashMap::new();
        let mut admitted = Vec::with_capacity(claimed.len());
        for candidate in claimed {
            let Some(local_network) = reaches.get(&candidate.id).copied() else {
                admitted.push(candidate);
                continue;
            };
            let policy = self.inner.scheduler.remote_address_policy(local_network);
            if let Err(rd_http::TargetRefusal::Refused(_)) =
                rd_http::check_target(&policy, &rd_http::SystemLookup, &candidate.url).await
            {
                tracing::warn!(
                    candidate_id = %candidate.id,
                    "a proposed link points at an address it may not reach; it was not checked"
                );
                let message = rd_core::CandidateMessage::coded(
                    CODE_CHECK_INTERNAL_ADDRESS,
                    "This link points at this machine or into your own network, so it was not checked",
                );
                self.record(&candidate, None, Some(message)).await;
                continue;
            }
            guards.insert(candidate.id, policy);
            admitted.push(candidate);
        }
        Ok((admitted, guards))
    }

    /// The client a check of `url` goes out with: held to `guard` when the link was proposed.
    async fn client_for(
        &self,
        url: &url::Url,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> Result<rd_scheduler::NetworkClient> {
        match guard {
            Some(policy) => {
                self.inner
                    .scheduler
                    .guarded_client(url, policy.clone())
                    .await
            }
            None => self.inner.scheduler.direct_client(url).await,
        }
    }

    /// Probes a media page; playlists fan out into additional candidates of the package.
    /// Classifies a direct HLS/DASH manifest before it is handed to the extractor
    /// (RD-080-06).
    ///
    /// Returns `true` when the candidate has been dealt with here — refused as DRM, or
    /// routed to the recorder as a live stream — and `false` when it is an ordinary VOD the
    /// yt-dlp probe should describe as usual.
    /// `force` skips the cheap gate below, for a link whose address already ends in
    /// `.m3u8`/`.mpd` and is therefore worth reading in full.
    async fn classify_manifest(
        &self,
        candidate: &LinkCandidate,
        force: bool,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> bool {
        let Ok(network) = self.client_for(&candidate.url, guard).await else {
            return false;
        };
        // Reading the body of every direct link to see whether it might be a playlist would
        // mean downloading a slice of every file in the queue. A HEAD first keeps that to
        // the responses that could plausibly be one: a manifest media type, or something
        // text-ish and small enough to be a document rather than a video.
        if !force {
            let probed = rd_http::probe_with_headers(
                &network.client,
                candidate.url.clone(),
                &network.headers,
            )
            .await;
            let Ok(probed) = probed else { return false };
            if !manifest_plausible(probed.content_type.as_deref(), probed.total_bytes) {
                return false;
            }
        }
        // Verbatim, not `peek_body_text`: that one strips tags and collapses whitespace to
        // summarise a hoster's HTML error page, which would leave a playlist as one line and
        // an MPD as nothing at all.
        let body = rd_http::fetch_text_verbatim(
            &network.client,
            candidate.url.clone(),
            &network.headers,
            rd_media::MAX_MANIFEST_BYTES,
        )
        .await;
        let Some(body) = body else { return false };
        let Some(kind) =
            rd_media::detect_manifest(&body.final_url, body.content_type.as_deref(), &body.text)
        else {
            return false;
        };
        // Relative URIs resolve against where the body actually came from, not against the
        // address that was pasted: a redirect to another host would otherwise make every
        // segment URL point at the wrong origin.
        let base = &body.final_url;
        let report = match kind {
            rd_media::ManifestKind::Hls => rd_media::parse_hls(&body.text, base),
            rd_media::ManifestKind::Dash => match rd_media::parse_dash(&body.text, base) {
                Ok(report) => report,
                Err(_) => return false,
            },
        };

        // A protected stream is a stated non-goal, so it ends here rather than as an obscure
        // ffmpeg failure ten minutes into a download. Deliberately without a code: the
        // protection scheme is the whole content of the sentence and no catalogue can carry
        // it, the same reason the host-key fingerprint stays untranslated below.
        if let Some(reason) = report.drm {
            tracing::info!(
                candidate_id = %candidate.id,
                detail = %reason.detail(),
                "manifest is DRM-protected; refusing"
            );
            self.record(
                candidate,
                None,
                Some(rd_core::CandidateMessage::plain(format!(
                    "DRM-protected stream ({})",
                    reason.detail()
                ))),
            )
            .await;
            return true;
        }

        // Credentials belong to the manifest's own origin. A CDN on a second hostname is
        // ordinary, but it does not inherit this link's session.
        let foreign = report.foreign_origins(base);
        if !foreign.is_empty() {
            tracing::debug!(
                candidate_id = %candidate.id,
                count = foreign.len(),
                "manifest references other origins; credentials stay with the manifest host"
            );
        }

        // A live manifest has no end, so it belongs to the recorder. Handing it to the file
        // downloader produces a job that can never reach 100 %.
        if report.class == rd_media::ManifestClass::Live {
            tracing::info!(candidate_id = %candidate.id, "manifest is live; routing to the recorder");
            if let Err(error) = self
                .inner
                .database
                .set_candidate_provider(candidate.id, rd_core::RECORD_PROVIDER.to_owned())
                .await
            {
                tracing::warn!(%error, "live manifest could not be routed to the recorder");
                return false;
            }
            let result = LinkCheckResult {
                url: candidate.url.clone(),
                status: LinkStatus::Online,
                file_name: None,
                size: None,
                media: None,
            };
            self.record(candidate, Some(result), None).await;
            return true;
        }
        false
    }

    /// Re-routes a direct link the server declares to be an NZB or a torrent
    /// (RD-080-11).
    ///
    /// A HEAD is enough: the content type is the whole signal, and fetching the document
    /// here would mean asking the indexer for it twice. Both Newznab and Torznab hand out
    /// download links with the identity in the header rather than the path, and so do the
    /// NZBHydra and Prowlarr proxies in front of them.
    ///
    /// Answers whether the link was re-routed, which the caller needs since RD-110-07: a
    /// small XML document is not `looks_downloadable`, and an NZB that has just been sent to
    /// its import path must not also be recorded as a page that is not a file.
    ///
    /// A torrent is read here as well (RD-120-68): its `info.name` is the release name, and
    /// the address it came from ends in whatever token the site hands out. The file tree is
    /// stored on the candidate the way an uploaded `.torrent` stores it, so the review and
    /// the info hash exist before the row does.
    async fn reclassify_document(
        &self,
        candidate: &LinkCandidate,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> Rerouted {
        let Ok(network) = self.client_for(&candidate.url, guard).await else {
            return Rerouted::No;
        };
        let Ok(probed) =
            rd_http::probe_with_headers(&network.client, candidate.url.clone(), &network.headers)
                .await
        else {
            return Rerouted::No;
        };
        let essence = probed
            .content_type
            .as_deref()
            .map(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or(value)
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let mut sniffed_torrent = None;
        let provider = if rd_core::NZB_CONTENT_TYPES.contains(&essence.as_str()) {
            rd_core::NZB_PROVIDER
        } else if rd_core::TORRENT_CONTENT_TYPES.contains(&essence.as_str()) {
            rd_core::TORRENT_PROVIDER
        } else if let Some(sniffed) = crate::link_check_probe::sniff_document(
            &network.client,
            &network.headers,
            &candidate.url,
        )
        .await
        {
            match sniffed {
                crate::link_check_probe::Sniffed::Nzb => rd_core::NZB_PROVIDER,
                crate::link_check_probe::Sniffed::Torrent(bytes) => {
                    sniffed_torrent = bytes;
                    rd_core::TORRENT_PROVIDER
                }
            }
        } else {
            // Logged rather than swallowed: this is where an indexer link that ends up being
            // downloaded as a document is decided, and the content type it answered with is
            // the one thing needed to work out why.
            tracing::debug!(
                url = %rd_core::redact_url(&candidate.url),
                content_type = %essence,
                "link was not recognised as a container"
            );
            return Rerouted::No;
        };
        tracing::info!(
            url = %rd_core::redact_url(&candidate.url),
            provider,
            "link re-routed to its import path"
        );
        if let Err(error) = self
            .inner
            .database
            .set_candidate_provider(candidate.id, provider.to_owned())
            .await
        {
            tracing::warn!(%error, provider, "link could not be re-routed to its import path");
        }
        if provider != rd_core::TORRENT_PROVIDER {
            return Rerouted::Container;
        }
        let parsed = crate::link_check_probe::read_torrent(
            &network.client,
            &network.headers,
            candidate,
            &self.inner.torrent,
            sniffed_torrent,
        )
        .await;
        if let Some(parsed) = &parsed
            && let Err(error) = self
                .inner
                .database
                .set_candidate_torrent_state(
                    candidate.id,
                    rd_core::TorrentCandidateState::ready(parsed.metadata.clone()),
                )
                .await
        {
            tracing::warn!(%error, "file tree of a re-routed torrent could not be stored");
        }
        Rerouted::Torrent(parsed.map(Box::new))
    }

    async fn check_media(&self, candidate: LinkCandidate, guard: Option<&rd_http::AddressPolicy>) {
        // A direct manifest is classified first: whether it is protected, and whether it is
        // live, decides where it goes before the extractor is asked anything.
        if self.classify_manifest(&candidate, true, guard).await {
            return;
        }
        match self.inner.media_probe.probe(&candidate.url).await {
            Ok(mut entries) if !entries.is_empty() => {
                let first = entries.remove(0);
                let file_name = rd_db::media_file_name(&first.info);
                let size = first
                    .info
                    .selected_variant()
                    .and_then(|variant| variant.filesize_approx)
                    .and_then(|value| rd_core::ByteCount::new(value).ok());
                let inventory = first.state.clone();
                let result = LinkCheckResult {
                    url: candidate.url.clone(),
                    status: LinkStatus::Online,
                    file_name: Some(file_name),
                    size,
                    media: Some(first.info),
                };
                self.record(&candidate, Some(result), None).await;
                // The bounded variant list travels with the check result; the full
                // inventory is stored separately so it never rides in a list response.
                if let Err(error) = self
                    .inner
                    .database
                    .set_candidate_media_inventory(candidate.id, inventory)
                    .await
                {
                    tracing::warn!(candidate_id = %candidate.id, %error, "media inventory could not be stored");
                }
                if !entries.is_empty()
                    && let Some(package_id) = candidate.package_id
                    && let Err(error) = self
                        .inner
                        .database
                        .add_media_candidates(package_id, entries)
                        .await
                {
                    tracing::warn!(%error, "playlist entries could not be added");
                }
            }
            Ok(_) => {
                self.record(
                    &candidate,
                    None,
                    Some(rd_core::CandidateMessage::coded(
                        "collector.check_no_media",
                        "No media found on this page",
                    )),
                )
                .await;
            }
            Err(failure) => {
                self.record(&candidate, None, Some((&failure).into())).await;
            }
        }
    }

    /// Resolves an ftp/sftp/webdav link into the listing that is reviewed before queueing.
    ///
    /// This is the one online check that genuinely contacts the server and enumerates it,
    /// which is affordable here because a directory listing is cheap on all three
    /// protocols — unlike crawling a gallery or joining a torrent swarm.
    async fn check_remote(&self, candidate: LinkCandidate) {
        if rd_core::ObjectStorageProvider::from_scheme(candidate.url.scheme()).is_some() {
            self.check_object(candidate).await;
            return;
        }
        let Some(target) = rd_core::RemoteTarget::parse(&candidate.url) else {
            self.record(
                &candidate,
                None,
                Some(rd_core::CandidateMessage::coded(
                    "collector.check_remote_invalid",
                    "The address is not a valid remote link",
                )),
            )
            .await;
            return;
        };
        let probed = match target.protocol.family() {
            rd_core::RemoteFamily::Ftp => self
                .inner
                .ftp
                .probe(&target, candidate.remote_credential_id)
                .await
                .map(|(probed, credential)| {
                    (
                        match probed {
                            rd_ftp::Probed::Resolved(listing) => Ok(listing),
                            rd_ftp::Probed::Failed(failure) => Err(failure),
                        },
                        credential,
                    )
                }),
            rd_core::RemoteFamily::Sftp => self
                .inner
                .sftp
                .probe(&target, candidate.remote_credential_id)
                .await
                .map(|(probed, credential)| {
                    (
                        match probed {
                            rd_sftp::Probed::Resolved(listing) => Ok(listing),
                            rd_sftp::Probed::Failed(failure) => Err(failure),
                        },
                        credential,
                    )
                }),
            rd_core::RemoteFamily::Webdav => self.probe_webdav(&candidate, &target).await,
        };
        let (outcome, credential_id) = match probed {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(candidate_id = %candidate.id, %error, "remote link check failed");
                self.record(
                    &candidate,
                    None,
                    Some(rd_core::CandidateMessage::coded(
                        "collector.check_remote_unreachable",
                        "The server could not be reached",
                    )),
                )
                .await;
                return;
            }
        };
        let listing = match outcome {
            Ok(listing) => listing,
            // A blocked host key or a missing login is a coded failure the UI can act on,
            // so the code travels with the message rather than being flattened to text.
            Err(failure) => {
                self.record_failure(&candidate, &failure).await;
                return;
            }
        };
        let summary = listing.summary();
        if let Err(error) = self
            .inner
            .database
            .set_candidate_listing(candidate.id, *listing, credential_id)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %error, "listing could not be stored");
        }
        let result = LinkCheckResult {
            url: candidate.url.clone(),
            status: LinkStatus::Online,
            // A single-file link keeps its own name; a directory is named by its folder.
            file_name: (summary.single_file || summary.file_count > 0)
                .then(|| file_name_for(&candidate, &summary)),
            size: Some(summary.total_bytes),
            media: None,
        };
        self.record(&candidate, Some(result), None).await;
    }

    /// Resolves an object storage link (RD-150-04): one object, or the prefix it names.
    ///
    /// The profile is chosen here as it will be at the transfer's start, so a link that no
    /// profile serves is marked before anybody queues it.
    async fn check_object(&self, candidate: LinkCandidate) {
        let listing = match self.inner.object_storage.probe(&candidate.url).await {
            Ok(Ok(listing)) => listing,
            Ok(Err(failure)) => {
                self.record_failure(&candidate, &failure).await;
                return;
            }
            Err(error) => {
                tracing::warn!(candidate_id = %candidate.id, %error, "object storage link check failed");
                self.record(
                    &candidate,
                    None,
                    Some(rd_core::CandidateMessage::coded(
                        "collector.check_remote_unreachable",
                        "The server could not be reached",
                    )),
                )
                .await;
                return;
            }
        };
        let summary = listing.summary();
        if let Err(error) = self
            .inner
            .database
            .set_candidate_listing(candidate.id, listing, None)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %error, "listing could not be stored");
        }
        let result = LinkCheckResult {
            url: candidate.url.clone(),
            status: LinkStatus::Online,
            file_name: (summary.single_file || summary.file_count > 0)
                .then(|| file_name_for(&candidate, &summary)),
            size: Some(summary.total_bytes),
            media: None,
        };
        self.record(&candidate, Some(result), None).await;
    }

    /// WebDAV probes through the same pooled client the transfer will use, so the auth
    /// profile, proxy and custom CA that apply to the download apply to the check too.
    async fn probe_webdav(
        &self,
        candidate: &LinkCandidate,
        target: &rd_core::RemoteTarget,
    ) -> anyhow::Result<(
        Result<Box<rd_core::RemoteListing>, rd_core::Failure>,
        Option<rd_core::RemoteCredentialId>,
    )> {
        let Some(scope) = target.sanitized_url() else {
            return Ok((
                Err(rd_core::Failure::coded(
                    rd_core::FailureKind::Permanent,
                    rd_webdav::PROPFIND_FAILED,
                    "The WebDAV address is not valid",
                )),
                None,
            ));
        };
        let network = self
            .inner
            .scheduler
            .network_client(None, None, rd_core::AuthProfileSelection::Auto, &scope)
            .await?;
        let probed = rd_webdav::probe(&network.client, target).await?;
        let _ = candidate;
        Ok((
            match probed {
                rd_webdav::Probed::Resolved(listing) => Ok(listing),
                rd_webdav::Probed::Failed(failure) => Err(failure),
            },
            None,
        ))
    }

    /// Stores a failure on the candidate.
    ///
    /// A candidate carries a plain message rather than a code, so the parameters a person
    /// actually needs are folded into the text: an unknown SSH host key is useless without
    /// the fingerprint to compare, which is public data, not a secret.
    async fn record_failure(&self, candidate: &LinkCandidate, failure: &rd_core::Failure) {
        let mut message = rd_core::CandidateMessage::from(failure);
        if let Some(fingerprint) = failure.params.get("fingerprint") {
            message.text.push_str(" (");
            message.text.push_str(fingerprint);
            if let Some(stored) = failure.params.get("stored_fingerprint") {
                message.text.push_str(", previously ");
                message.text.push_str(stored);
            }
            message.text.push(')');
            // The fingerprint is in the text and in no catalogue, so the translated sentence
            // would drop exactly the part this message exists for.
            message.code = None;
        }
        self.record(candidate, None, Some(message)).await;
    }

    async fn record(
        &self,
        candidate: &LinkCandidate,
        result: Option<LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
    ) {
        self.record_with_cache(candidate, result, error, None, None)
            .await;
    }

    /// Like [`Self::record`], with what a provider's cache said about the link merged in
    /// first (RD-130-11). `checked_by` is the slug of the account whose resolver checked it,
    /// which a resolver's own `cached` answer is stamped with.
    async fn record_with_cache(
        &self,
        candidate: &LinkCandidate,
        result: Option<LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
        hint: Option<&link_check_cache::CacheHint>,
        checked_by: Option<&str>,
    ) {
        let (result, cached_by) = link_check_cache::merge(result, hint, checked_by);
        let error = if result.is_some() {
            error
        } else {
            error.or_else(|| {
                Some(rd_core::CandidateMessage::coded(
                    "collector.check_failed",
                    "The check failed",
                ))
            })
        };
        // `candidate` carries the pre-claim state: a duplicate keeps its warning but still
        // gains the probed file name and media metadata.
        let was_duplicate = candidate.state == rd_core::LinkCandidateState::Duplicate;
        let online = result
            .as_ref()
            .is_some_and(|result| matches!(result.status, LinkStatus::Online | LinkStatus::Cached));
        let known = result
            .as_ref()
            .and_then(|result| result.media.as_ref())
            .and_then(|media| serde_json::to_string(media).ok());
        let file_name = result.as_ref().and_then(|result| result.file_name.clone());
        if let Err(problem) = self
            .inner
            .database
            .record_candidate_check(candidate.id, result, error, was_duplicate, cached_by)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %problem, "could not store link check");
        }
        if online {
            self.enrich(candidate, file_name.as_deref(), known.as_deref())
                .await;
        }
    }

    /// Asks the installed enrichers about a link that resolved.
    ///
    /// Only after the check succeeded, and only when somebody switched enrichment on: an
    /// enricher reaches a service outside this machine, and doing that for every media link
    /// on the strength of having installed a plugin would be a decision nobody made.
    async fn enrich(
        &self,
        candidate: &LinkCandidate,
        file_name: Option<&str>,
        known: Option<&str>,
    ) {
        // The switch is checked before the plugins are compiled: an installation that never
        // asked for enrichment should not pay for building an enricher it will not call.
        if !self.enrichment_enabled().await {
            return;
        }
        let enrichers = self.enrichers().await;
        if enrichers.is_empty() {
            return;
        }
        // What the indexer already declared about this hit, read back and gated once more
        // before it leaves the process (RD-107-02).
        let declared = match self
            .inner
            .database
            .candidate_source_attributes(candidate.id)
            .await
        {
            Ok(attributes) => attributes,
            Err(error) => {
                tracing::warn!(
                    candidate_id = %candidate.id,
                    %error,
                    "declared attributes unreadable"
                );
                std::collections::BTreeMap::new()
            }
        };
        let known = known_for(known, &declared);
        let fields = enrichers
            .enrich(&candidate.url, file_name, known.as_deref())
            .await;
        if fields.is_empty() {
            return;
        }
        if let Err(error) = self
            .inner
            .database
            .set_candidate_enrichment(candidate.id, fields)
            .await
        {
            tracing::warn!(candidate_id = %candidate.id, %error, "enrichment could not be stored");
        }
    }

    /// The installed enrichers, compiled on first use.
    async fn enrichers(&self) -> Arc<rd_plugin_ext::MetadataEnrichers> {
        Arc::clone(
            self.inner
                .enrichers
                .get_or_init(|| async {
                    match rd_plugin_ext::MetadataEnrichers::load(
                        &self.inner.plugins,
                        Some(Arc::clone(&self.inner.plugin_host)),
                    )
                    .await
                    {
                        Ok(enrichers) => Arc::new(enrichers),
                        Err(error) => {
                            tracing::warn!(%error, "could not load metadata enricher plugins");
                            Arc::new(rd_plugin_ext::MetadataEnrichers::none())
                        }
                    }
                })
                .await,
        )
    }

    /// Whether the person switched metadata enrichment on. Read per check rather than cached:
    /// switching it off has to take effect on the next link, not after a restart.
    async fn enrichment_enabled(&self) -> bool {
        // A database error still reads as "off", as before: enrichment is an optional extra
        // and must not fail a check. A field stored with the wrong type is reported.
        self.inner
            .database
            .service_setting_field::<bool>("metadata_enrichment_enabled")
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }
}

/// Name shown for a resolved remote link: the file for a single-file link, the folder
/// otherwise.
fn file_name_for(candidate: &LinkCandidate, summary: &rd_core::RemoteListingSummary) -> String {
    let from_path = candidate
        .url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|part| !part.is_empty()))
        .map(str::to_owned);
    from_path
        .or_else(|| {
            summary
                .root
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "download".to_owned())
}

/// The message a provider check leaves on a candidate.
///
/// Three unrelated situations used to share one sentence. `Some("Check result missing")` was
/// handed to every candidate of a provider batch and the store kept it for every result of
/// status `Unknown`, so the reader saw the same words whether the plugin had skipped the URL
/// or answered about it (RD-109-43) — and in the second, commoner case the words were simply
/// untrue, a result was there.
///
/// They are told apart here, and each carries a stable code the interface translates:
///
/// * no result for this URL — the plugin answered for the batch and said nothing about it;
/// * `Unknown` with an account for this hoster — the hoster itself could not say;
/// * `Unknown` without one — the case worth naming, because several hosters answer a check
///   only to an account of their own while the download works without one. The check ran
///   through a covering multihoster, or through an account of a different hoster.
///
/// A conclusive answer — online or gone — leaves no message at all.
fn provider_message(
    result: Option<&LinkCheckResult>,
    own_account: bool,
) -> Option<rd_core::CandidateMessage> {
    match result {
        None => Some(rd_core::CandidateMessage::coded(
            "collector.check_no_result",
            "The check returned no answer for this link",
        )),
        Some(result) if result.status == LinkStatus::Unknown => Some(if own_account {
            rd_core::CandidateMessage::coded(
                "collector.check_unknown",
                "The hoster could not say whether this link is still available",
            )
        } else {
            rd_core::CandidateMessage::coded(
                "collector.check_unknown_no_account",
                "The hoster could not say whether this link is still available; checking it \
                 needs an account for this hoster. The download may still work without one",
            )
        }),
        Some(_) => None,
    }
}

/// The same for a direct link, which is probed here rather than by a plugin.
///
/// `probe_direct` returns `Unknown` for everything that is neither a reply nor a 4xx — a
/// timeout, a refused connection, a proxy in the way. That answer used to be stored as "No
/// HTTP client available", which names the one situation it was not.
fn direct_message(result: Option<&LinkCheckResult>) -> Option<rd_core::CandidateMessage> {
    match result {
        None => Some(rd_core::CandidateMessage::coded(
            "collector.check_no_client",
            "No HTTP client was available for this link",
        )),
        Some(result) if result.status == LinkStatus::Unknown => {
            Some(rd_core::CandidateMessage::coded(
                "collector.check_inconclusive",
                "The server did not answer the check, so the link could not be confirmed",
            ))
        }
        // The one answer that is a statement about the *file* rather than about the check,
        // so it always carries its reason: a state nobody can queue out of has to say why.
        // *Which* reason it is depends on the installation and not on the response, and this
        // is the one place that has both answers at once -- see `unresolvable_message`.
        Some(result) if result.status == LinkStatus::Unresolvable => Some(unresolvable_message(
            rd_provider_registry::provider_for_url(&result.url).is_some(),
        )),
        Some(_) => None,
    }
}

/// Which of the two unresolvable cases a page response is (RD-120-18).
///
/// Every one of them used to be `collector.check_not_a_file`, "this address answers with a
/// page, not with a file". The owner's live check of 1.1 ended on that sentence four times --
/// `controlc.com`, `nfile.cc`, `dwp.la`, `icerbox.com` -- and not once was the address the
/// cause. The site rule had resolved correctly and handed over an address this installation
/// has no resolver for. The old sentence is literally true of a hoster's download page and
/// names the wrong culprit, so the reader goes looking at the rule, the site or their browser
/// instead of at the installation.
///
/// `known_hoster` is `rd_provider_registry::provider_for_url(...).is_some()`, and it is asked
/// for its **negative**. `None` means no installed plugin manifest claims this host, so
/// nothing in this installation can turn the address into a file. The positive is deliberately
/// *not* read as "this will work": a registered provider says the host is known, not that its
/// resolver is loaded, enabled, or able to serve this particular address. That is why `true`
/// keeps the older code -- a statement about the address, which promises nothing about the
/// installation.
///
/// **The new sentence stops where the evidence stops and does not say a plugin is missing.**
/// ADR 0019 measured the four cases afterwards: `nfile.cc` and `dwp.la` are not hosters at all
/// but affiliate cloakers, and `dwp.la` forwards to `downup.me`, which `plugins/xfs-generic`
/// already claims. For those two, "a plugin for this hoster is missing" would replace one
/// false diagnosis with another. What holds for all four is only this: no resolver is
/// installed for the host in the address. Telling a forwarder from a genuine dead end means
/// following the redirect, and nothing here can -- `probe_direct` keeps
/// `rd_http::ProbeResult::final_url` to itself and `LinkCheckResult` carries the declared
/// address alone. Following it at intake is a collector decision about untrusted redirects
/// with its own job, recorded as the open gap of RD-120-18.
///
/// The host is not written into the sentence here. It is the candidate's own address, which
/// the row already carries, and the catalogues name it as `{host}`; the message therefore
/// stays a stable code plus a parameter rather than prose assembled in Rust.
fn unresolvable_message(known_hoster: bool) -> rd_core::CandidateMessage {
    if known_hoster {
        rd_core::CandidateMessage::coded(
            "collector.check_not_a_file",
            "The address answered with a page rather than a file",
        )
    } else {
        rd_core::CandidateMessage::coded(
            "collector.check_no_resolver",
            "No resolver is installed for this host, so this address cannot be turned into a \
             file here",
        )
    }
}

/// Undoes the "not a file" verdict for a document that has just found its import path.
///
/// An NZB or a `.torrent` is XML or bencode a few kilobytes long, which is exactly what
/// `looks_downloadable` refuses -- rightly, because nothing should *download* it. It is not
/// downloaded: it was re-routed a line earlier and will be imported. Leaving the verdict
/// standing would park an indexer link in a state no one can queue out of (RD-080-11,
/// RD-110-07).
///
/// A torrent that could be read also gives the link its name and size (RD-120-68). The name
/// is recorded as declared, so the regroup after the check names the package after the
/// release rather than after the address's last segment -- an opaque download token that
/// once became a package, and with it a folder, called `JKs2Jt3Fo=_l3XcwZVDZXkBOYSX+nhd+A==`.
fn rerouted_document(
    rerouted: &Rerouted,
    result: Option<LinkCheckResult>,
) -> Option<LinkCheckResult> {
    let mut probed = result?;
    if !matches!(rerouted, Rerouted::No) && probed.status == LinkStatus::Unresolvable {
        probed.status = LinkStatus::Online;
    }
    if let Rerouted::Torrent(Some(torrent)) = rerouted {
        probed.file_name = Some(rd_files::sanitize_file_name(&torrent.name));
        probed.size = rd_core::ByteCount::new(torrent.total_bytes).ok();
    }
    Some(probed)
}

/// Where [`LinkCheckService::reclassify_document`] sent a link.
enum Rerouted {
    /// Not a container: the link stays on the download path.
    No,
    /// An NZB, imported under its own name later.
    Container,
    /// A torrent, with its metadata when the file could be read.
    Torrent(Option<Box<rd_torrent::ParsedTorrent>>),
}

/// Whether a response could be an HLS/DASH manifest, from its headers alone.
///
/// Deliberately generous on the type and strict on the size: CDNs serve playlists as
/// `text/plain` all the time, but no manifest is 4 GB, and reading the first megabytes of
/// every large file to find out would cost more than the check is worth.
fn manifest_plausible(content_type: Option<&str>, total_bytes: Option<u64>) -> bool {
    let essence = content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let declared = matches!(
        essence.as_str(),
        "application/vnd.apple.mpegurl"
            | "application/x-mpegurl"
            | "audio/mpegurl"
            | "audio/x-mpegurl"
            | "application/dash+xml"
    );
    if declared {
        return true;
    }
    let texty = matches!(
        essence.as_str(),
        "text/plain" | "application/xml" | "text/xml" | "application/octet-stream" | ""
    );
    texty
        && total_bytes.is_none_or(|bytes| bytes > 0 && bytes <= rd_media::MAX_MANIFEST_BYTES as u64)
}

/// Builds the `known` JSON an enricher is asked with (RD-107-02).
///
/// The decision this job made first: the indexer's attributes ride inside the JSON the WIT
/// contract already carries, under an added `indexer` key, rather than in a new record field.
/// A field added to a WIT record is not additive for a component already shipped — every
/// installed enricher would have to be rebuilt and re-signed before the host got an answer
/// out of it again — while an added JSON key is ignored by any reader that does not know it.
///
/// The resolved media metadata therefore stays exactly where it was, at the top level of the
/// object, and `indexer` joins it as a sibling.
///
/// The attributes pass `rd_subscription::retain_attributes` here, not only on the way into
/// the database: the rule is "nothing that gate discards reaches a plugin", and this is the
/// last place before it does. `retain` is idempotent, so gating an already gated map costs a
/// pass over at most 40 short strings and changes nothing.
///
/// A link with no declared attributes — every pasted, captured and container link — gets the
/// media JSON through unchanged, so a plugin sees precisely what it saw before this existed.
fn known_for(
    media_json: Option<&str>,
    declared: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let gated = rd_subscription::retain_attributes(declared, None).attributes;
    if gated.is_empty() {
        return media_json.map(str::to_owned);
    }
    let indexer = serde_json::Value::Object(
        gated
            .into_iter()
            .map(|(name, value)| (name, serde_json::Value::String(value)))
            .collect(),
    );
    let mut object = match media_json.map(serde_json::from_str::<serde_json::Value>) {
        Some(Ok(serde_json::Value::Object(object))) => object,
        // No media, or media that is not an object: the attributes still travel, and the
        // media JSON is not silently dropped — it keeps its own key instead.
        Some(Ok(other)) => {
            let mut object = serde_json::Map::new();
            object.insert("media".to_owned(), other);
            object
        }
        Some(Err(_)) | None => serde_json::Map::new(),
    };
    object.insert("indexer".to_owned(), indexer);
    serde_json::to_string(&serde_json::Value::Object(object)).ok()
}

#[cfg(test)]
#[path = "link_check_service_tests.rs"]
mod tests;
