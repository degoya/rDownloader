//! The phases of [`super::run`], each ending the run where the original ended it.
//!
//! A phase answers [`ControlFlow::Break`] when it has already recorded how this attempt ended
//! -- stopped, failed, blocked, adopted -- and `run` then returns `Ok(())` exactly as it did
//! when the code stood inline; [`ControlFlow::Continue`] carries what the next phase needs.

use std::{
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use rd_core::{
    ContentTransform, DownloadFile, DownloadState, Failure, FailureKind, StorageRootId,
    TransformKey,
};
use rd_files::StorageRoot;
use rd_http::{
    ChunkSpec, ProbeResult, StreamTransform, TransformCheckpoint, TransformPlan, plan_chunks,
};
use rd_plugin_api::ResolvedDownload;
use tokio_util::sync::CancellationToken;

use super::{to_persisted, transition_stopped, validators_changed};
use crate::{
    BlockReason, SchedulerHandle,
    failures::{record_error, size_mismatch},
    finish::{adopt_existing_final, current_destination, promote, remove_if_empty, verify_part},
};

/// A phase's answer: `Break` once the attempt's outcome is recorded, `Continue` otherwise.
pub(super) type Step<T> = Result<ControlFlow<(), T>>;

/// What a resolver or a stream-transform plugin made of the link.
pub(super) type Resolution = (
    Option<ResolvedDownload>,
    Option<(ContentTransform, TransformKey)>,
);

/// Records the stop the cancellation asked for and ends the run.
async fn stopped<T>(scheduler: &SchedulerHandle, file: &DownloadFile) -> Step<T> {
    transition_stopped(scheduler, file)
        .await
        .map(ControlFlow::Break)
}

/// Records `failure` on the download and ends the run.
async fn recorded<T>(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    failure: Failure,
) -> Step<T> {
    record_error(scheduler, file, failure)
        .await
        .map(ControlFlow::Break)
}

/// Moves the download to `Resolving` and asks the resolvers, then the stream-transform
/// plugins, what the link stands for.
pub(super) async fn resolve_source(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    cancellation: &CancellationToken,
) -> Step<Resolution> {
    if file.state == DownloadState::RetryWait {
        scheduler
            .database
            .transition_download(file.id, DownloadState::Queued)
            .await?;
    }
    scheduler
        .database
        .transition_download(file.id, DownloadState::Resolving)
        .await?;
    let resolver_pin = scheduler.database.resolver_pin(file.id).await?;
    // A span of its own inside the download's trace (RD-110-03), so a slow or failing
    // resolver is visible as a step rather than as a gap. No address goes on it: the span
    // carries the download id, and the trace leads to the log records that name the rest.
    let resolve = tracing::Instrument::instrument(
        scheduler.resolvers.resolve(
            file.source.clone(),
            file.account_id,
            file.proxy_profile_id,
            resolver_pin.as_ref(),
        ),
        tracing::info_span!("download.resolve", download_id = %file.id),
    );
    let resolved = match tokio::select! {
        () = cancellation.cancelled() => return stopped(scheduler, file).await,
        result = resolve => result,
    } {
        Ok(resolved) => resolved,
        Err(failure) => return recorded(scheduler, file, failure).await,
    };
    // A provider that encrypts on the client answers with an address *and* how its bytes
    // become a file (RD-103-02, ADR 0011). Asked only where the resolver chain said nothing,
    // so an ordinary link never pays for a world no installed plugin may even implement.
    let transformed = if resolved.is_some() || scheduler.transforms.is_empty() {
        None
    } else {
        // The fragment the intake put in the vault comes back here, and only here
        // (RD-110-38). The stored address lost it so that no row, event or log line could
        // ever carry a decryption key; the plugin needs it to derive one, so it is restored
        // onto the address a single call before it is handed over, and the restored `Url`
        // never leaves this block. A download with no reference, or a vault that cannot open
        // it, is asked with the address exactly as it is stored -- the plugin then refuses on
        // its own terms instead of being handed something half-rebuilt.
        let mut url = file.source.clone();
        match scheduler.database.download_secret_fragment(file.id).await {
            Ok(Some(fragment)) => url.set_fragment(Some(&fragment)),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                %error,
                download_id = %file.id,
                "the vaulted link fragment could not be read back"
            ),
        }
        let request = rd_plugin_api::ResolveRequest {
            url,
            client: rd_plugin_api::ClientIdentity {
                account_id: file.account_id,
                proxy_profile_id: file.proxy_profile_id,
                tls_revision: 0,
            },
        };
        let ask = tracing::Instrument::instrument(
            scheduler.transforms.resolve(&request),
            tracing::info_span!("download.transform", download_id = %file.id),
        );
        match tokio::select! {
            () = cancellation.cancelled() => return stopped(scheduler, file).await,
            answer = ask => answer,
        } {
            None => None,
            Some(Ok(answer)) => Some(answer),
            Some(Err(failure)) => return recorded(scheduler, file, failure).await,
        }
    };
    // From here the address is handled exactly like a resolver's, which is the point: the
    // only thing the twelfth world adds is the description travelling beside it.
    let (resolved, transform) = match transformed {
        Some(answer) => (Some(answer.download), Some((answer.transform, answer.key))),
        None => (resolved, None),
    };
    Ok(ControlFlow::Continue((resolved, transform)))
}

/// Checks the probe against what the hoster announced and what an earlier attempt recorded,
/// and plans or reloads the chunks. Answers the size the transfer works with and the chunks.
pub(super) async fn plan_transfer(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    probe_result: &ProbeResult,
    resolved_size: Option<rd_core::ByteCount>,
) -> Step<(Option<u64>, Vec<ChunkSpec>)> {
    // The size the hoster announced for this file: what the resolver just read off the link
    // page, else what the online check recorded when the link was added. It is the only signal
    // that outlives a transfer which otherwise succeeds -- 1150 bytes fetched for a 405 MB
    // release completed, was checksummed, and showed a green tick (RD-109-36). Once the first
    // probe has run, `total_bytes` holds that probe's own number, so a later attempt compares a
    // value with itself and the rule cannot fire twice on the same evidence.
    let announced_size = resolved_size
        .or(file.total_bytes)
        .map(rd_core::ByteCount::get);
    if let (Some(announced), Some(offered)) = (announced_size, probe_result.total_bytes)
        && rd_http::contradicts_announced_size(announced, offered)
    {
        return recorded(scheduler, file, size_mismatch(announced, offered)).await;
    }
    let total_bytes = probe_result.total_bytes.or(announced_size);
    let transfer = scheduler.database.load_transfer(file.id).await?;
    let committed = transfer
        .chunks
        .iter()
        .any(|chunk| chunk.committed > chunk.start);
    // Named causes, not a bare `Blocked`: these two must survive a storage release untouched.
    // Restarting a transfer whose ETag moved writes a different file's bytes over confirmed
    // ones, which is the whole reason it is stopped here.
    if committed && validators_changed(&transfer, probe_result) {
        scheduler
            .database
            .block_download(file.id, BlockReason::ValidatorsChanged.as_str())
            .await?;
        return Ok(ControlFlow::Break(()));
    }
    if committed && !probe_result.accepts_ranges {
        scheduler
            .database
            .block_download(file.id, BlockReason::RangesRefused.as_str())
            .await?;
        return Ok(ControlFlow::Break(()));
    }

    let chunks = if transfer.chunks.is_empty() || !committed {
        let planned = plan_chunks(
            total_bytes,
            probe_result.accepts_ranges,
            scheduler.chunk_budget(&probe_result.final_url),
        );
        let persisted = planned.iter().map(to_persisted).collect::<Vec<_>>();
        scheduler
            .database
            .prepare_transfer(
                file.id,
                total_bytes,
                probe_result.etag.clone(),
                probe_result.last_modified.clone(),
                persisted,
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
    Ok(ControlFlow::Continue((total_bytes, chunks)))
}

/// Where one attempt writes: the package folder, its staging directory, the part file and
/// the final path.
pub(super) struct Destination {
    pub(super) root: StorageRoot,
    pub(super) staging: PathBuf,
    pub(super) part_path: PathBuf,
    pub(super) final_path: PathBuf,
}

/// Creates the staging directory, adopts a file an earlier run already put in place, checks
/// the capacity and moves the download to `Downloading`.
pub(super) async fn prepare_destination(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    total_bytes: Option<u64>,
) -> Step<Destination> {
    let packages = scheduler.database.list_packages().await?;
    let package = packages
        .into_iter()
        .find(|package| package.id == file.package_id)
        .context("download package not found")?;
    let root = StorageRoot::create(
        StorageRootId::new(),
        "download destination".to_owned(),
        PathBuf::from(&package.destination),
    )
    .await?;
    let staging = root.resolve(Path::new(".rdownloader"))?;
    tokio::fs::create_dir_all(&staging).await?;
    let part_path = staging.join(format!("{}.part", file.id));
    // A previous run may have got the file into its final place and stopped before recording
    // it (`scheduler.before_promote`). Adopting it is the difference between finishing and
    // fetching the whole thing again to file it next to itself as `name (1).ext`.
    if adopt_existing_final(
        scheduler,
        file,
        root.path(),
        &staging,
        &part_path,
        total_bytes,
    )
    .await?
    {
        return Ok(ControlFlow::Break(()));
    }
    // The collision policy decides the name before a byte is fetched (RD-150-01): a `skip`,
    // an `ask` or a `compare` that can adopt the existing file ends the attempt right here.
    let ControlFlow::Continue(final_path) =
        crate::collision::before_transfer(scheduler, file, root.path(), &part_path, total_bytes)
            .await?
    else {
        return Ok(ControlFlow::Break(()));
    };
    // The same policy every other runner passes through; an insufficient root is blocked
    // instead of letting the transfer fail on a write halfway through.
    let remaining = total_bytes.map(|total| total.saturating_sub(file.committed_bytes.get()));
    if !scheduler
        .ensure_capacity(root.path().to_string_lossy().as_ref(), remaining)
        .await?
    {
        scheduler
            .database
            .block_download(file.id, BlockReason::Capacity.as_str())
            .await?;
        return Ok(ControlFlow::Break(()));
    }
    scheduler
        .database
        .transition_download(file.id, DownloadState::Downloading)
        .await?;
    Ok(ControlFlow::Continue(Destination {
        root,
        staging,
        part_path,
        final_path,
    }))
}

/// Turns a plugin's transform description into a computable stream transform.
pub(super) async fn prepare_transform(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    transform: Option<(ContentTransform, TransformKey)>,
) -> Step<Option<TransformPlan>> {
    let plan = match transform {
        None => None,
        Some((description, key)) => {
            // The key goes to the vault before the description is computable at all, and the
            // reference it comes back as goes into the description (RD-120-11). `rd-http`
            // refuses a description without one, so this is not a precaution but the step
            // that makes a transformed download possible; the reference is also part of the
            // fingerprint, which is why `adopt_transform_key` hands the *same* one back for
            // the same key rather than minting a fresh one per attempt.
            let description = match scheduler
                .database
                .adopt_transform_key(file.id, key.expose())
                .await
            {
                Ok(Some(reference)) => description.with_key_reference(reference),
                Ok(None) => {
                    return recorded(
                        scheduler,
                        file,
                        rd_core::Failure::coded(
                            rd_core::FailureKind::Permanent,
                            rd_core::CODE_KEY_MISSING,
                            "this installation has no vault to put a transform key in".to_owned(),
                        ),
                    )
                    .await;
                }
                Err(error) => {
                    return recorded(
                        scheduler,
                        file,
                        rd_core::Failure::coded(
                            rd_core::FailureKind::Transient {
                                retry_after_seconds: None,
                            },
                            rd_core::CODE_KEY_MISSING,
                            format!("the transform key could not be put away: {error}"),
                        ),
                    )
                    .await;
                }
            };
            let stream = match StreamTransform::new(description, &key) {
                Ok(stream) => stream,
                Err(failure) => return recorded(scheduler, file, failure).await,
            };
            let (fingerprint, macs) = scheduler.database.transform_checkpoint(file.id).await?;
            Some(TransformPlan {
                transform: Arc::new(stream),
                checkpoint: TransformCheckpoint { fingerprint, macs },
            })
        }
    };
    Ok(ControlFlow::Continue(plan))
}

/// Verifies a completely fetched part file and promotes it into the destination that is
/// current now.
pub(super) async fn finish_download(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    root: &StorageRoot,
    staging: &Path,
    part_path: &Path,
    final_path: PathBuf,
) -> Result<()> {
    scheduler
        .database
        .transition_download(file.id, DownloadState::Verifying)
        .await?;
    // The package category may have changed while downloading; finish into the
    // destination that is current now.
    let final_path = match current_destination(scheduler, file).await {
        Ok(Some(destination)) if destination != *root.path() => {
            tokio::fs::create_dir_all(&destination).await?;
            destination.join(&file.file_name)
        }
        _ => final_path,
    };
    let result: Result<()> = async {
        let computed = verify_part(scheduler, file, part_path).await?;
        // The name may have been taken while the transfer ran; the policy is applied again
        // rather than letting the rename replace whatever arrived there in between.
        let ControlFlow::Continue((final_path, overwrite)) = crate::collision::after_transfer(
            scheduler,
            file,
            part_path,
            final_path,
            computed.as_ref(),
        )
        .await?
        else {
            return Ok(());
        };
        // Recorded before the replacement, so no file is overwritten without its record: an
        // audit log that cannot be written stops the overwrite rather than missing it.
        if let Some(overwrite) = overwrite {
            crate::collision::audit_overwrite(scheduler, file, &final_path, overwrite).await?;
        }
        promote(scheduler, file, part_path, &final_path, computed).await?;
        // The replaced file's own download no longer lies there: its index entry would claim
        // this download's bytes carry its digest.
        if overwrite.is_some() {
            scheduler
                .database
                .forget_indexed_path(final_path.to_string_lossy().into_owned(), file.id)
                .await?;
        }
        scheduler.database.clear_collision_prompt(file.id).await?;
        Ok(())
    }
    .await;
    if result.is_ok() {
        remove_if_empty(staging).await;
    }
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            // The bytes arrived; putting them in place on this machine did not work
            // — a checksum that did not match, a rename that was refused, a
            // destination that filled up. Coded so the mirror group does not read a
            // local obstacle as a reason to go and ask the next hoster.
            let failure = match error.downcast::<Failure>() {
                Ok(stated) => stated,
                Err(local) => Failure::coded(
                    FailureKind::Permanent,
                    crate::mirrors::LOCAL_PROMOTE_CODE,
                    local.to_string(),
                ),
            };
            record_error(scheduler, file, failure).await
        }
    }
}
