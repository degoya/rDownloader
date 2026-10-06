use std::ops::ControlFlow;

use anyhow::Result;
use rd_core::{DownloadFile, DownloadState, Failure};
use rd_db::PersistedChunk;
use rd_http::ChunkSpec;
use tokio_util::sync::CancellationToken;

use crate::{ProfileBoundary, SchedulerHandle, StopReason};

#[path = "worker_client.rs"]
mod client;
#[path = "worker_phases.rs"]
mod phases;
#[path = "worker_sources.rs"]
mod sources;
#[path = "worker_steps.rs"]
mod steps;

use client::provider_authorization;
pub(crate) use client::{build_client, build_replay_client, build_test_client};

pub(crate) async fn run(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    cancellation: CancellationToken,
) -> Result<()> {
    let ControlFlow::Continue((resolved, transform)) =
        phases::resolve_source(scheduler, file, &cancellation).await?
    else {
        return Ok(());
    };
    let unclaimed = resolved.is_none() && transform.is_none();
    let ControlFlow::Continue(address_policy) =
        steps::try_sources(scheduler, file, unclaimed, &cancellation).await?
    else {
        return Ok(());
    };

    // A plugin-resolved link points at a transfer URL; anything else is a plain direct link
    // that is downloaded exactly as it was added.
    let resolved_by_plugin = resolved.is_some();
    let steps::Origin {
        file: working_file,
        source,
        headers,
        resolved_size,
    } = steps::adopt_resolution(scheduler, file, resolved).await?;
    let file = &working_file;
    let replay = crate::replay::load(scheduler, file).await?;
    let ControlFlow::Continue((source, connected)) = steps::connect(
        scheduler,
        file,
        source,
        headers,
        replay.as_ref(),
        address_policy,
        &cancellation,
    )
    .await?
    else {
        return Ok(());
    };
    let ControlFlow::Continue(probed) = steps::probe(
        scheduler,
        file,
        source,
        connected,
        resolved_by_plugin,
        &cancellation,
    )
    .await?
    else {
        return Ok(());
    };
    transfer(
        scheduler,
        file,
        probed,
        resolved_size,
        transform,
        replay.as_ref(),
        cancellation,
    )
    .await
}

/// The transfer itself once the address has answered: the chunks planned, the destination and
/// the stream transform prepared, the engine run and its answer recorded.
async fn transfer(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    probed: steps::Probed,
    resolved_size: Option<rd_core::ByteCount>,
    transform: Option<(rd_core::ContentTransform, rd_core::TransformKey)>,
    replay: Option<&crate::replay::ReplayContext>,
    cancellation: CancellationToken,
) -> Result<()> {
    let steps::Probed {
        source,
        connected,
        probe_result,
    } = probed;
    let ControlFlow::Continue((total_bytes, chunks)) =
        phases::plan_transfer(scheduler, file, &probe_result, resolved_size).await?
    else {
        return Ok(());
    };
    let ControlFlow::Continue(destination) =
        phases::prepare_destination(scheduler, file, total_bytes).await?
    else {
        return Ok(());
    };

    // The description becomes something computable only here, where the key is checked
    // against the primitives this build implements and the previous attempt's chunk MACs are
    // read back. The key goes straight into the transform and is dropped with it: it is
    // never written to a row, never put in a header and never printed.
    let ControlFlow::Continue(transform) =
        phases::prepare_transform(scheduler, file, transform).await?
    else {
        return Ok(());
    };
    // The address the chunks are fetched from: the per-host budget and the memory of a
    // host that ignores ranges both belong to it, not to the link the user pasted.
    let transfer_url = probe_result.final_url.clone();
    let ControlFlow::Continue((client, headers)) =
        steps::transfer_headers(scheduler, file, connected, &source, &transfer_url).await?
    else {
        return Ok(());
    };
    let request = steps::request(
        probe_result,
        destination.part_path.clone(),
        total_bytes,
        chunks,
        headers,
        replay,
        transform,
    );
    let outcome = steps::fetch(scheduler, file, client, request, cancellation).await;
    steps::settle(scheduler, file, outcome, replay, &transfer_url, destination).await
}

/// A download's address points where a stranger's document may not reach (RD-150-03).
pub(crate) fn internal_address() -> Failure {
    Failure::coded(
        rd_core::FailureKind::Permanent,
        rd_core::CODE_INTERNAL_ADDRESS,
        "The download's address points at an address a remote document may not reach",
    )
}

pub(crate) async fn transition_stopped(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
) -> Result<()> {
    let reason = scheduler.active.lock().await.reasons.get(&file.id).copied();
    let reason = match reason {
        Some(reason) => reason,
        // Nobody stopped this file: the service is stopping — an update, a restart, the tray's
        // quit. The row keeps its running state, and the next start's `recover_interrupted`
        // queues it again from its checkpoint, as after a crash. Writing `Paused` here left
        // every running download paused after an update until somebody resumed it by hand
        // (the 1.12.0 release candidate's self-update run, 2026-10-06).
        None if scheduler.shutdown.is_cancelled() => return Ok(()),
        None => StopReason::Paused,
    };
    match reason {
        StopReason::Paused => {
            scheduler
                .database
                .transition_download(file.id, DownloadState::Paused)
                .await?;
        }
        StopReason::Cancelled => {
            scheduler
                .database
                .transition_download(file.id, DownloadState::Cancelled)
                .await?;
        }
        // The cause is written with the state: the release that undoes this stop has to be
        // able to pick out exactly the transfers it stopped and leave the rest alone.
        StopReason::Blocked(blocked) => {
            scheduler
                .database
                .block_download(file.id, blocked.as_str())
                .await?;
        }
    }
    Ok(())
}

/// A client together with the credential headers that belong on every request it makes.
///
/// `Basic` and `Bearer` ride along per request rather than as client defaults: that keeps
/// the pool from fragmenting, lets the online-check probe use the same credential, and
/// leaves reqwest free to strip the header on a cross-origin redirect.
pub struct NetworkClient {
    pub client: reqwest::Client,
    pub headers: Vec<(String, String)>,
    /// The scope `headers` are confined to, when a profile contributed any.
    ///
    /// `headers` fit the address the client was built for. A request to any other address —
    /// the end of a redirect, a resolver's answer — takes them only where this admits it
    /// (RD-120-43).
    pub profile_boundary: Option<ProfileBoundary>,
    /// The account's provider, user name and stored secret, when it has a secret.
    ///
    /// Carried out rather than turned into a header here, because whether that secret may be
    /// sent depends on the address the transfer ends up at — which is the resolver's answer,
    /// not the source this client was built for. See [`provider_authorization`].
    pub provider_credential: Option<ProviderCredential>,
}

/// The account credential a transfer may carry, by reference. Never the value: that is read
/// from the vault only once an address has been found that may receive it.
#[derive(Clone)]
pub struct ProviderCredential {
    /// The account's provider slug.
    pub provider: String,
    /// The account's user name, which HTTP Basic pairs with the secret (RD-120-38).
    pub username: Option<String>,
    /// The vault reference of the secret.
    pub reference: String,
}

fn validators_changed(transfer: &rd_db::TransferMetadata, probe: &rd_http::ProbeResult) -> bool {
    transfer
        .total_bytes
        .zip(probe.total_bytes)
        .is_some_and(|(old, new)| old != new)
        || transfer
            .etag
            .as_ref()
            .zip(probe.etag.as_ref())
            .is_some_and(|(old, new)| old != new)
        || (transfer.etag.is_none()
            && transfer
                .last_modified
                .as_ref()
                .zip(probe.last_modified.as_ref())
                .is_some_and(|(old, new)| old != new))
}

fn to_persisted(chunk: &ChunkSpec) -> PersistedChunk {
    PersistedChunk {
        id: chunk.id,
        start: chunk.start,
        end: chunk.end,
        committed: chunk.committed,
    }
}
