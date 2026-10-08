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
use chrono::Utc;
use rd_core::{
    ChunkId, DownloadFile, DownloadSource, Failure, FailureKind, PieceHashes, SourceState,
};
use rd_http::{
    ChunkSpec, DownloadEngine, DownloadOutcome, MultiSourceRequest, SourceEndpoint, chunks_aligned,
    plan_aligned_chunks,
};
use tokio_util::sync::CancellationToken;

use super::{NetworkClient, build_replay_client, phases, to_persisted, transition_stopped};
use crate::{
    BlockReason, SchedulerHandle,
    failures::{record_error, record_http_error},
};

#[path = "worker_sources_ledger.rs"]
mod ledger;
#[path = "worker_sources_probe.rs"]
mod probe;

use ledger::DatabaseLedger;

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

    let ControlFlow::Continue(probe::Probed {
        reference,
        endpoints,
    }) = probe::probe_sources(scheduler, file, ready, &policy, &network, &cancellation).await?
    else {
        return Ok(ControlFlow::Break(()));
    };
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

    let ControlFlow::Continue(plan) = plan_transfer(scheduler, file, total, &endpoints).await?
    else {
        return Ok(ControlFlow::Break(()));
    };
    transfer(
        scheduler,
        file,
        network,
        total,
        endpoints,
        plan,
        cancellation,
    )
    .await?;
    Ok(ControlFlow::Break(()))
}

/// How one attempt fetches the file: its chunks, how many mirrors fetch at the same time, the
/// piece hashes each chunk is checked by and the chunks an earlier attempt left unchecked.
struct Plan {
    chunks: Vec<ChunkSpec>,
    parallel: usize,
    pieces: Option<Arc<PieceHashes>>,
    unverified: HashMap<ChunkId, Option<u32>>,
}

/// Plans the attempt's chunks, or takes the recorded plan up again. `Break` when bytes are
/// already committed for a different size: the download is blocked.
async fn plan_transfer(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    total: u64,
    endpoints: &[SourceEndpoint],
) -> Result<ControlFlow<(), Plan>> {
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
        plan_chunks(scheduler, file, total, endpoints, parallel, pieces.as_ref()).await?
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
    Ok(ControlFlow::Continue(Plan {
        chunks,
        parallel,
        pieces,
        unverified,
    }))
}

/// Plans fresh chunks on piece boundaries, as many as the mirrors fetching at once allow, and
/// writes the plan down.
async fn plan_chunks(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    total: u64,
    endpoints: &[SourceEndpoint],
    parallel: usize,
    pieces: Option<&PieceHashes>,
) -> Result<Vec<ChunkSpec>> {
    // Without a hash an FTP or SFTP mirror serves only from the first byte (TR-02), so a set
    // led by one is fetched as one chunk that mirror can serve.
    let unproven = pieces.is_none() && file.expected_checksum.is_none();
    let budget = if unproven && endpoints.first().is_some_and(|first| first.via.is_some()) {
        1
    } else {
        endpoints
            .iter()
            .take(parallel)
            .map(|endpoint| scheduler.chunk_budget(&endpoint.url))
            .sum::<usize>()
            .clamp(1, MAX_CHUNKS)
    };
    let align = pieces.map_or(1, |pieces| pieces.length);
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
    Ok(planned)
}

/// Prepares the destination, hands the planned chunks and the agreeing sources to the engine
/// and records how the transfer ended.
async fn transfer(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    network: NetworkClient,
    total: u64,
    endpoints: Vec<SourceEndpoint>,
    plan: Plan,
    cancellation: CancellationToken,
) -> Result<()> {
    let ControlFlow::Continue(phases::Destination {
        root,
        staging,
        part_path,
        final_path,
    }) = phases::prepare_destination(scheduler, file, Some(total)).await?
    else {
        return Ok(());
    };
    let engine = DownloadEngine::new(network.client, scheduler.scoped_limiter(file).await)
        .with_host_limits(scheduler.host_limits().clone());
    scheduler.host_handed_over(file.id).await;
    let outcome = engine
        .download_from_sources(
            MultiSourceRequest {
                part_path: part_path.clone(),
                total_bytes: total,
                chunks: plan.chunks,
                sources: endpoints,
                parallel_sources: plan.parallel,
                pieces: plan.pieces,
                unverified: plan.unverified,
                whole_file_hash: file.expected_checksum.is_some(),
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
    Ok(())
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
