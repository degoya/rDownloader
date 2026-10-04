//! A download with several sources (RD-150-03): the counterpart of [`super::run`] for a file
//! a Metalink document named at several addresses.
//!
//! The sources come from the row in the order they were written. Each attempt probes the ones
//! that are neither isolated nor waiting out a backoff, keeps those that agree on the file,
//! plans chunks on piece boundaries and hands the lot to
//! [`rd_http::DownloadEngine::download_from_sources`]. What the engine learns — a delivery, a
//! failure, a refused piece — is written down as it happens through [`DatabaseLedger`], so the
//! next attempt, after a restart or not, starts from the same order and the same health.
//!
//! Every source is an address a stranger's document named, so before it is probed it is held
//! to [`address_policy`]: never this machine, and the person's own network only for a set they
//! handed over themselves. A source that fails the rule is isolated with
//! [`rd_core::CODE_INTERNAL_ADDRESS`] and the others go on. The client the sources are fetched
//! with carries the same rule into its resolver and its redirects, so a name that answers
//! differently the second time is refused when the connection is made.
//!
//! An FTP or SFTP mirror is fetched through its runner's [`rd_http::RangeSource`]: every
//! connection it opens — the size query, and one per chunk at the chunk's offset — resolves
//! and checks the address itself when the socket is opened.

use std::{collections::HashMap, ops::ControlFlow, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use rd_core::{
    ByteCount, DownloadFile, DownloadSource, Failure, FailureKind, SourceOutcome, SourceProtocol,
    SourceState,
};
use rd_db::Database;
use rd_http::{
    CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, MultiSourceRequest, SourceEndpoint,
    SourceLedger, chunks_aligned, plan_aligned_chunks, probe_with_headers,
};
use tokio_util::sync::CancellationToken;

use super::{
    NetworkClient, build_replay_client, phases, provider_authorization, to_persisted,
    transition_stopped,
};
use crate::{
    BlockReason, SchedulerHandle,
    failures::{record_error, record_http_error},
    profile_boundary::admitted,
};

/// Most chunks one multi-source run plans, however many sources take part.
const MAX_CHUNKS: usize = 32;

/// Runs one attempt of a download that has sources.
///
/// `Continue` hands the attempt back to the single-source path: no source said how big the
/// file is and the set did not either, so nothing can be split and the ordinary path — one
/// connection to the download's own address — is the honest answer.
pub(super) async fn run(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    sources: Vec<DownloadSource>,
    cancellation: CancellationToken,
) -> Result<ControlFlow<()>> {
    let now = Utc::now();
    let ready: Vec<&DownloadSource> = sources
        .iter()
        .filter(|source| source.state_at(now) == SourceState::Ready)
        .collect();
    if ready.is_empty() {
        record_error(scheduler, file, nothing_ready(&sources, now)).await?;
        return Ok(ControlFlow::Break(()));
    }
    let policy = address_policy(scheduler, &sources);
    let network = tokio::select! {
        () = cancellation.cancelled() => {
            transition_stopped(scheduler, file).await?;
            return Ok(ControlFlow::Break(()));
        }
        result = build_replay_client(scheduler, file, None, Some(policy.clone())) => result?,
    };

    // Probe in order and keep the sources that agree on the file. The size the set stated
    // is the reference; without one, the first source that names a size sets it.
    let stated = file.total_bytes.map(ByteCount::get);
    let mut reference = stated;
    let mut endpoints = Vec::new();
    for source in ready {
        match rd_http::check_target(&policy, &rd_http::SystemLookup, &source.url).await {
            Ok(_) => {}
            Err(rd_http::TargetRefusal::Refused(refused)) => {
                tracing::warn!(
                    download_id = %file.id,
                    position = source.position,
                    address = ?refused.address,
                    "a mirror points at an address it may not reach; it is not requested"
                );
                isolate(
                    scheduler,
                    file,
                    source.position,
                    rd_core::CODE_INTERNAL_ADDRESS,
                )
                .await?;
                continue;
            }
            Err(rd_http::TargetRefusal::Unresolved(error)) => {
                let failure = transient(
                    "download.network_failed",
                    rd_core::error_with_causes(&error),
                );
                note_failure(scheduler, file, source.position, &failure).await?;
                continue;
            }
        }
        let probed = if matches!(
            source.protocol,
            SourceProtocol::Http | SourceProtocol::Https
        ) {
            tokio::select! {
                () = cancellation.cancelled() => {
                    transition_stopped(scheduler, file).await?;
                    return Ok(ControlFlow::Break(()));
                }
                result = http_endpoint(scheduler, &network, source) => result?,
            }
        } else {
            tokio::select! {
                () = cancellation.cancelled() => {
                    transition_stopped(scheduler, file).await?;
                    return Ok(ControlFlow::Break(()));
                }
                result = mirror_endpoint(scheduler, source, &policy) => result,
            }
        };
        let (endpoint, offered) = match probed {
            Ok(Some(probed)) => probed,
            // No runner for this protocol in this service: nothing to fetch it with.
            Ok(None) => continue,
            Err(failure) => {
                note_failure(scheduler, file, source.position, &failure).await?;
                continue;
            }
        };
        match (reference, offered) {
            (Some(expected), Some(offered)) if expected != offered => {
                // A different size is a different file. Stated by the document, that is
                // final; merely disagreeing with another mirror, it is one failure.
                if stated.is_some() {
                    isolate(
                        scheduler,
                        file,
                        source.position,
                        rd_core::CODE_SOURCE_SIZE_MISMATCH,
                    )
                    .await?;
                } else {
                    let failure = transient(
                        rd_core::CODE_SOURCE_SIZE_MISMATCH,
                        "a mirror offers a different size",
                    );
                    note_failure(scheduler, file, source.position, &failure).await?;
                }
                continue;
            }
            (None, Some(offered)) => reference = Some(offered),
            _ => {}
        }
        endpoints.push(endpoint);
    }
    let Some(total) = reference else {
        // The single path fetches the download's own address, which the set named as well:
        // it is judged by the same rule first, whether or not it was among the sources tried
        // in this attempt. Its client carries the rule too.
        if let Err(rd_http::TargetRefusal::Refused(_)) =
            rd_http::check_target(&policy, &rd_http::SystemLookup, &file.source).await
        {
            record_error(scheduler, file, super::internal_address()).await?;
            return Ok(ControlFlow::Break(()));
        }
        return Ok(ControlFlow::Continue(()));
    };
    if endpoints.is_empty() {
        let sources = scheduler.database.download_sources(file.id).await?;
        record_error(scheduler, file, nothing_ready(&sources, Utc::now())).await?;
        return Ok(ControlFlow::Break(()));
    }

    let pieces = scheduler.database.download_piece_hashes(file.id).await?;
    // Chunks from several mirrors in one file only where something proves the bytes; without
    // it one mirror works at a time and the rest stand by.
    let parallel = if pieces.is_some() || file.expected_checksum.is_some() {
        endpoints.len()
    } else {
        1
    };
    let transfer = scheduler.database.load_transfer(file.id).await?;
    let committed = transfer
        .chunks
        .iter()
        .any(|chunk| chunk.committed > chunk.start);
    if committed
        && transfer
            .total_bytes
            .is_some_and(|recorded| recorded != total)
    {
        scheduler
            .database
            .block_download(file.id, BlockReason::ValidatorsChanged.as_str())
            .await?;
        return Ok(ControlFlow::Break(()));
    }
    let chunks: Vec<ChunkSpec> = if transfer.chunks.is_empty() || !committed {
        let budget = endpoints
            .iter()
            .take(parallel)
            .map(|endpoint| scheduler.chunk_budget(&endpoint.url))
            .sum::<usize>()
            .clamp(1, MAX_CHUNKS);
        let align = pieces.as_ref().map_or(1, |pieces| pieces.length);
        let planned = plan_aligned_chunks(total, budget, align);
        scheduler
            .database
            .prepare_transfer(
                file.id,
                Some(total),
                None,
                None,
                planned.iter().map(to_persisted).collect(),
            )
            .await?;
        planned
    } else {
        transfer
            .chunks
            .iter()
            .map(|chunk| ChunkSpec {
                id: chunk.id,
                start: chunk.start,
                end: chunk.end,
                committed: chunk.committed,
            })
            .collect()
    };
    // A plan that does not fall on piece boundaries — written before the set had pieces —
    // cannot be checked chunk by chunk; the whole-file hash still is, before promotion.
    let pieces = pieces
        .filter(|pieces| chunks_aligned(&chunks, pieces.length, total))
        .map(Arc::new);
    let unverified: HashMap<_, _> = scheduler
        .database
        .chunk_marks(file.id)
        .await?
        .into_iter()
        .filter(|mark| !mark.verified)
        .map(|mark| (mark.chunk_id, mark.source_position))
        .collect();

    let ControlFlow::Continue(phases::Destination {
        root,
        staging,
        part_path,
        final_path,
    }) = phases::prepare_destination(scheduler, file, Some(total)).await?
    else {
        return Ok(ControlFlow::Break(()));
    };
    let engine = DownloadEngine::new(network.client, scheduler.scoped_limiter(file).await)
        .with_host_limits(scheduler.host_limits().clone());
    let outcome = engine
        .download_from_sources(
            MultiSourceRequest {
                part_path: part_path.clone(),
                total_bytes: total,
                chunks,
                sources: endpoints,
                parallel_sources: parallel,
                pieces,
                unverified,
            },
            Arc::new(DatabaseLedger {
                database: scheduler.database.clone(),
                download_id: file.id,
            }),
            cancellation,
        )
        .await;
    match outcome {
        Ok(DownloadOutcome::Complete) => {
            phases::finish_download(scheduler, file, &root, &staging, &part_path, final_path)
                .await?;
        }
        Ok(DownloadOutcome::Paused) => transition_stopped(scheduler, file).await?,
        Err(error) => record_http_error(scheduler, file, error).await?,
    }
    Ok(ControlFlow::Break(()))
}

/// Probes an HTTP mirror: the endpoint the chunks go to — where the probe ended, with the
/// headers decided again for that address (RD-120-38) — and the size it offers. A failure is
/// the source's, and the attempt goes on with the others.
async fn http_endpoint(
    scheduler: &SchedulerHandle,
    network: &NetworkClient,
    source: &DownloadSource,
) -> Result<std::result::Result<Option<(SourceEndpoint, Option<u64>)>, Failure>> {
    let headers = match headers_for(scheduler, network, &source.url).await? {
        Ok(headers) => headers,
        Err(failure) => return Ok(Err(failure)),
    };
    let probe = match probe_with_headers(&network.client, source.url.clone(), &headers).await {
        Ok(probe) => probe,
        Err(rd_http::HttpDownloadError::Failure(failure)) => return Ok(Err(failure)),
        Err(other) => return Ok(Err(transient("download.network_failed", other.to_string()))),
    };
    if !probe.looks_downloadable() {
        return Ok(Err(transient(
            "download.not_a_file",
            "a mirror answered with a page",
        )));
    }
    if !probe.accepts_ranges {
        return Ok(Err(transient(
            "download.range_ignored",
            "a mirror refuses ranges",
        )));
    }
    let headers = if probe.final_url == source.url {
        headers
    } else {
        match headers_for(scheduler, network, &probe.final_url).await? {
            Ok(headers) => headers,
            Err(failure) => return Ok(Err(failure)),
        }
    };
    Ok(Ok(Some((
        SourceEndpoint {
            position: source.position,
            url: probe.final_url,
            headers,
            via: None,
        },
        probe.total_bytes,
    ))))
}

/// An FTP or SFTP mirror, fetched through its runner (RD-150-03): the size it reports over a
/// connection held to `policy`, and the endpoint whose chunks each open one more such
/// connection. `Ok(None)` when this service has no runner for the protocol.
async fn mirror_endpoint(
    scheduler: &SchedulerHandle,
    source: &DownloadSource,
    policy: &rd_http::AddressPolicy,
) -> std::result::Result<Option<(SourceEndpoint, Option<u64>)>, Failure> {
    let kind = match source.protocol {
        SourceProtocol::Ftp | SourceProtocol::Ftps => rd_core::DownloadKind::Ftp,
        SourceProtocol::Sftp => rd_core::DownloadKind::Sftp,
        SourceProtocol::Http | SourceProtocol::Https => return Ok(None),
    };
    let Some(range_source) = scheduler
        .runners
        .get(kind)
        .and_then(|runner| runner.range_source())
    else {
        tracing::debug!(
            position = source.position,
            protocol = source.protocol.as_str(),
            "no runner fetches this mirror's protocol here"
        );
        return Ok(None);
    };
    let Some(target) = rd_core::RemoteTarget::parse(&source.url) else {
        return Err(Failure::coded(
            FailureKind::Permanent,
            "download.mirror_address_invalid",
            "the mirror's address is not a valid remote link",
        ));
    };
    let size = range_source.size(&target, Some(policy)).await?;
    Ok(Some((
        SourceEndpoint {
            position: source.position,
            url: source.url.clone(),
            headers: Vec::new(),
            via: Some(rd_http::RangeTransport {
                source: range_source,
                target,
                policy: Some(policy.clone()),
            }),
        },
        Some(size),
    )))
}

/// The headers a request to `target` carries: the profile's where its scope admits them, the
/// account's credential where its provider's gate does. Decided per address, because the
/// mirrors of one file sit on hosts that have nothing to do with each other.
async fn headers_for(
    scheduler: &SchedulerHandle,
    network: &NetworkClient,
    target: &url::Url,
) -> Result<std::result::Result<Vec<(String, String)>, Failure>> {
    let mut headers = admitted(&network.headers, network.profile_boundary.as_ref(), target);
    match provider_authorization(scheduler, network.provider_credential.as_ref(), target).await? {
        Ok(Some(header)) => headers.push(header),
        Ok(None) => {}
        Err(failure) => return Ok(Err(failure)),
    }
    Ok(Ok(headers))
}

/// The address rule a download's sources keep to: never this machine — loopback, link-local,
/// the address the service listens on — and the person's own network only when the set came
/// from their own hand (`local_network`, decided at intake).
pub(super) fn address_policy(
    scheduler: &SchedulerHandle,
    sources: &[DownloadSource],
) -> rd_http::AddressPolicy {
    scheduler.source_address_policy(sources)
}

/// Whether the sources are the download's own address alone, with no piece hashes to check
/// it by: what a link a document or a page proposed without mirrors is written with
/// (RD-150-03). The single-source path fetches such a download; the rule stays the same.
pub(super) async fn only_its_own_address(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    sources: &[DownloadSource],
) -> Result<bool> {
    let [only] = sources else {
        return Ok(false);
    };
    let without_password = |url: &url::Url| {
        let mut url = url.clone();
        let _ = url.set_password(None);
        url
    };
    if without_password(&only.url) != without_password(&file.source) {
        return Ok(false);
    }
    Ok(scheduler
        .database
        .download_piece_hashes(file.id)
        .await?
        .is_none())
}

/// Takes a source out for good with a stable code; the attempt goes on with the others.
async fn isolate(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    position: u32,
    code: &str,
) -> Result<()> {
    scheduler
        .database
        .record_source_outcome(
            file.id,
            position,
            SourceOutcome::Isolated {
                code: code.to_owned(),
            },
        )
        .await
}

/// Records one source's failure; the attempt goes on with the others. A failure that says the
/// source points inside the network — a probe the guarded client refused — isolates it.
async fn note_failure(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    position: u32,
    failure: &Failure,
) -> Result<()> {
    if failure.code.as_deref() == Some(rd_core::CODE_INTERNAL_ADDRESS) {
        return isolate(scheduler, file, position, rd_core::CODE_INTERNAL_ADDRESS).await;
    }
    let retry_after_seconds = match failure.category {
        FailureKind::RateLimited {
            retry_after_seconds,
        }
        | FailureKind::Transient {
            retry_after_seconds,
        } => retry_after_seconds,
        _ => None,
    };
    scheduler
        .database
        .record_source_outcome(
            file.id,
            position,
            SourceOutcome::Failed {
                code: failure
                    .code
                    .clone()
                    .unwrap_or_else(|| "download.failed".to_owned()),
                retry_after_seconds,
            },
        )
        .await
}

fn transient(code: &str, message: impl Into<String>) -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        code,
        message.into(),
    )
}

/// Why no source can be tried: all isolated is final, a backoff is a wait for the earliest.
fn nothing_ready(sources: &[DownloadSource], now: chrono::DateTime<Utc>) -> Failure {
    let wait = sources
        .iter()
        .filter(|source| source.state_at(now) == SourceState::BackingOff)
        .filter_map(|source| source.backoff_until)
        .min()
        .map(|until| u64::try_from((until - now).num_seconds().max(1)).unwrap_or(1));
    match wait {
        Some(seconds) => Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: Some(seconds),
            },
            rd_core::CODE_NO_USABLE_SOURCE,
            "every source of this file is waiting out a failure".to_owned(),
        )
        .with_param("seconds", seconds),
        None => Failure::coded(
            FailureKind::Permanent,
            rd_core::CODE_NO_USABLE_SOURCE,
            "no source of this file can be used".to_owned(),
        ),
    }
}

/// Writes what the engine learns straight to the download's rows.
struct DatabaseLedger {
    database: Database,
    download_id: rd_core::DownloadId,
}

#[async_trait]
impl CheckpointSink for DatabaseLedger {
    async fn commit(&self, chunk_id: rd_core::ChunkId, committed_offset: u64) -> Result<()> {
        self.database
            .checkpoint_chunk(chunk_id, committed_offset)
            .await
    }
}

#[async_trait]
impl SourceLedger for DatabaseLedger {
    async fn source_delivered(&self, position: u32, bytes: u64) -> Result<()> {
        self.database
            .record_source_outcome(
                self.download_id,
                position,
                SourceOutcome::Delivered { bytes },
            )
            .await
    }

    async fn source_failed(
        &self,
        position: u32,
        code: &str,
        retry_after_seconds: Option<u64>,
    ) -> Result<()> {
        self.database
            .record_source_outcome(
                self.download_id,
                position,
                SourceOutcome::Failed {
                    code: code.to_owned(),
                    retry_after_seconds,
                },
            )
            .await
    }

    async fn source_isolated(&self, position: u32, code: &str) -> Result<()> {
        // A position the row does not know — a mark left by a set that was since replaced —
        // has nothing to isolate; the rewind that follows still happens.
        match self
            .database
            .record_source_outcome(
                self.download_id,
                position,
                SourceOutcome::Isolated {
                    code: code.to_owned(),
                },
            )
            .await
        {
            Err(error) if rd_db::store_kind(&error) == Some(rd_db::StoreErrorKind::NotFound) => {
                Ok(())
            }
            other => other,
        }
    }

    async fn chunk_marked(
        &self,
        chunk_id: rd_core::ChunkId,
        position: Option<u32>,
        verified: bool,
    ) -> Result<()> {
        self.database.mark_chunk(chunk_id, position, verified).await
    }

    async fn chunk_rewound(&self, chunk_id: rd_core::ChunkId, committed: u64) -> Result<()> {
        self.database.rewind_chunk(chunk_id, committed).await
    }
}
