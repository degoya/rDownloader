//! Background online check for LinkGrabber candidates (provider API or direct probe).

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use rd_core::{Account, BatchId, CandidateId, LinkCandidate, LinkCheckResult, LinkStatus};
use rd_db::Database;
use rd_scheduler::SchedulerHandle;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{hosters, link_check_cache, link_check_probe::probe_direct};

pub use completed::CompletedBatches;

mod completed;
mod documents;
mod process;
mod protocols;
mod record;

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
    completed: Arc<completed::Completed>,
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
                completed: completed::Completed::new(),
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
    /// counts; a second one is a wiring mistake (API-06).
    pub fn attach_cache_checkers(&self, checkers: crate::remote_job_service::RemoteJobService) {
        let first = self.inner.cache_checkers.set(checkers).is_ok();
        debug_assert!(first, "the cache checkers are attached once");
    }

    /// Receives the id of every batch whose check has finished, successfully or not; a
    /// waiter that falls behind a burst takes the ids it missed from the kept ones (CORE-01).
    #[must_use]
    pub fn follow_completed(&self) -> CompletedBatches {
        self.inner.completed.follow()
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
                self.inner.completed.announce(batch);
            }
        }
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
