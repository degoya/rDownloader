//! The steps of [`super::run`] from the resolver's answer to the engine's, each ending the run
//! where the original ended it, by the rule of `worker_phases.rs`.

use std::{ops::ControlFlow, path::PathBuf, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::DownloadFile;
use rd_db::Database;
use rd_http::{
    CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
    ProbeResult, TransformPlan, probe_with_headers,
};
use rd_plugin_api::ResolvedDownload;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    NetworkClient, ProviderCredential, build_replay_client, internal_address,
    phases::{self, Step, recorded, stopped},
    provider_authorization, sources, transition_stopped,
};
use crate::{
    ProfileBoundary, SchedulerHandle,
    failures::{not_a_file, record_http_error, record_http_error_with_replay},
    profile_boundary::admitted,
    replay::ReplayContext,
};

/// A file with several sources (RD-150-03) is fetched from them, unless a resolver or a
/// transform plugin claimed its address (`unclaimed` is false): then the address is a
/// hoster's, not a mirror's.
///
/// Answers the address rule the single path keeps to. Its own address is one of the set's, so
/// the single path keeps to the set's address rule as well. So does a link a document or a
/// page proposed without mirrors: its one source row is its own address, which the single
/// path fetches — it needs no ranges — held to the rule the row was written with.
pub(super) async fn try_sources(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    unclaimed: bool,
    cancellation: &CancellationToken,
) -> Step<Option<rd_http::AddressPolicy>> {
    let mut address_policy = None;
    if unclaimed && file.kind == rd_core::DownloadKind::Http {
        let listed = scheduler.database.download_sources(file.id).await?;
        if !listed.is_empty() {
            address_policy = Some(sources::address_policy(scheduler, &listed));
        }
        if listed.iter().any(|source| source.protocol.serves_chunks())
            && !sources::only_its_own_address(scheduler, file, &listed).await?
            && sources::run(scheduler, file, listed, cancellation.clone())
                .await?
                .is_break()
        {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(address_policy))
}

/// What the transfer starts from once the resolver had its say.
pub(super) struct Origin {
    /// The download, under the name the resolver gave it.
    pub(super) file: DownloadFile,
    pub(super) source: Url,
    pub(super) headers: Vec<(String, String)>,
    pub(super) resolved_size: Option<rd_core::ByteCount>,
}

/// Takes over the resolver's transfer URL, headers, size and file name; without a resolver
/// the link is fetched exactly as it was added.
pub(super) async fn adopt_resolution(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    resolved: Option<ResolvedDownload>,
) -> Result<Origin> {
    let mut working_file = file.clone();
    let (source, headers, resolved_size) = match resolved {
        Some(resolved) => {
            if let Some(name) = resolved.file_name {
                let name = rd_files::sanitize_file_name(&name);
                scheduler
                    .database
                    .set_download_file_name(file.id, name.clone())
                    .await?;
                // The package may still be named after the hoster, because intake had nothing
                // else: this address carries no path segment and no check result (RD-109-45).
                // It happens here, before the destination directory is created below, so the
                // folder is named rather than renamed. A failure costs a name, not a download.
                if let Err(error) = scheduler.adopt_resolved_package_name(file, &name).await {
                    tracing::warn!(
                        package_id = %file.package_id,
                        %error,
                        "the package kept its hoster name"
                    );
                }
                working_file.file_name = name;
            }
            let headers = resolved
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect::<Vec<_>>();
            (resolved.url, headers, resolved.size)
        }
        None => (file.source.clone(), Vec::new(), None),
    };
    Ok(Origin {
        file: working_file,
        source,
        headers,
        resolved_size,
    })
}

/// Only a resume risks continuing a partial file that was fetched from a URL which has
/// since expired. A fresh start has nothing to protect and pays nothing here.
async fn refresh_before_resume(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    source: Url,
    headers: Vec<(String, String)>,
    replay: Option<&ReplayContext>,
) -> Step<(Url, Vec<(String, String)>)> {
    let mut source = source;
    let mut headers = headers;
    if file.committed_bytes.get() > 0 {
        match crate::replay::before_resume(scheduler, file, &source, replay).await? {
            crate::replay::Refreshed::Fresh => {}
            crate::replay::Refreshed::Replaced {
                url,
                headers: resolved,
            } => {
                source = url;
                headers = resolved;
            }
            crate::replay::Refreshed::Impossible(reason) => {
                return recorded(scheduler, file, crate::replay::blocked(reason)).await;
            }
        }
    }
    Ok(ControlFlow::Continue((source, headers)))
}

/// An address a stranger's document or page named is judged before the first request
/// (RD-150-03). The client below holds every name to the same rule when it connects, but a
/// literal address never reaches its resolver, so this is where `127.0.0.1` is refused.
async fn refuse_internal_address(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    address_policy: Option<&rd_http::AddressPolicy>,
    source: &Url,
) -> Step<()> {
    if let Some(policy) = address_policy
        && let Err(rd_http::TargetRefusal::Refused(refused)) =
            rd_http::check_target(policy, &rd_http::SystemLookup, source).await
    {
        tracing::warn!(
            download_id = %file.id,
            address = ?refused.address,
            "the download's address points at an address it may not reach; it is not requested"
        );
        return recorded(scheduler, file, internal_address()).await;
    }
    Ok(ControlFlow::Continue(()))
}

/// The client of an attempt and the headers that go to its source.
pub(super) struct Connected {
    client: reqwest::Client,
    /// What goes to `source`: the resolver's or the capture's own headers, the profile's inside
    /// its scope and the account's credential where it may go.
    headers: Vec<(String, String)>,
    /// What goes to every address: the resolver's or the capture's own headers. The profile's
    /// and the account's are decided per address, because each is confined to a scope.
    unauthenticated: Vec<(String, String)>,
    profile_headers: Vec<(String, String)>,
    profile_boundary: Option<ProfileBoundary>,
    provider_credential: Option<ProviderCredential>,
}

/// The address the attempt starts from, refreshed for a resume and held to the address rule,
/// with the client and the headers that go to it.
pub(super) async fn connect(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    source: Url,
    headers: Vec<(String, String)>,
    replay: Option<&ReplayContext>,
    address_policy: Option<rd_http::AddressPolicy>,
    cancellation: &CancellationToken,
) -> Step<(Url, Connected)> {
    let ControlFlow::Continue((source, headers)) =
        refresh_before_resume(scheduler, file, source, headers, replay).await?
    else {
        return Ok(ControlFlow::Break(()));
    };
    let ControlFlow::Continue(()) =
        refuse_internal_address(scheduler, file, address_policy.as_ref(), &source).await?
    else {
        return Ok(ControlFlow::Break(()));
    };
    let ControlFlow::Continue(connected) = client_for(
        scheduler,
        file,
        replay,
        address_policy,
        &source,
        headers,
        cancellation,
    )
    .await?
    else {
        return Ok(ControlFlow::Break(()));
    };
    Ok(ControlFlow::Continue((source, connected)))
}

/// Builds the transfer client and the headers for `source`.
async fn client_for(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    replay: Option<&ReplayContext>,
    address_policy: Option<rd_http::AddressPolicy>,
    source: &Url,
    headers: Vec<(String, String)>,
    cancellation: &CancellationToken,
) -> Step<Connected> {
    let network = tokio::select! {
        () = cancellation.cancelled() => return stopped(scheduler, file).await,
        result = build_replay_client(scheduler, file, replay, address_policy) => result?,
    };
    let NetworkClient {
        client,
        headers: profile_headers,
        profile_boundary,
        provider_credential,
    } = network;
    let unauthenticated = headers.clone();
    let mut headers = headers;
    // The profile was chosen for the link as it was added; `source` may be a resolver's answer
    // on another host, and gets the profile's headers only inside its scope (RD-120-43).
    headers.extend(admitted(
        &profile_headers,
        profile_boundary.as_ref(),
        source,
    ));
    // Credential headers go in before the probe so the online check authenticates too.
    match provider_authorization(scheduler, provider_credential.as_ref(), source).await? {
        Ok(Some(header)) => headers.push(header),
        Ok(None) => {}
        Err(failure) => return recorded(scheduler, file, failure).await,
    }
    Ok(ControlFlow::Continue(Connected {
        client,
        headers,
        unauthenticated,
        profile_headers,
        profile_boundary,
        provider_credential,
    }))
}

/// An attempt whose address has answered the probe.
pub(super) struct Probed {
    pub(super) source: Url,
    pub(super) connected: Connected,
    pub(super) probe_result: ProbeResult,
}

/// Probes `source` with the attempt's headers.
pub(super) async fn probe(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    source: Url,
    connected: Connected,
    resolved_by_plugin: bool,
    cancellation: &CancellationToken,
) -> Step<Probed> {
    let probe_result = match tokio::select! {
        () = cancellation.cancelled() => return stopped(scheduler, file).await,
        result = probe_with_headers(&connected.client, source.clone(), &connected.headers) => result,
    } {
        Ok(result) => result,
        Err(error) => {
            return record_http_error(scheduler, file, error)
                .await
                .map(ControlFlow::Break);
        }
    };
    // A resolver that hands back a landing page (expired direct link, hoster limit notice)
    // must not be written to disk as if it were the file.
    if resolved_by_plugin && !probe_result.looks_downloadable() {
        let failure = not_a_file(
            &connected.client,
            &source,
            &connected.headers,
            &probe_result,
        )
        .await;
        return recorded(scheduler, file, failure).await;
    }
    Ok(ControlFlow::Continue(Probed {
        source,
        connected,
        probe_result,
    }))
}

/// The client and the headers the chunks are fetched with from `transfer_url`.
///
/// The account's credential belongs to the address the chunks come from (RD-120-38). The probe
/// followed the source's redirects, and reqwest dropped `Authorization` at every change of host
/// on the way; the chunks are then fetched from where it ended, directly. Reusing the probe's
/// headers there would hand the credential to exactly the foreign host the redirect was
/// stripped for. So it is decided again, against the address the bytes actually come from.
/// The profile's headers the same way, against the profile's scope (RD-120-43): they had the
/// same hole.
pub(super) async fn transfer_headers(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    connected: Connected,
    source: &Url,
    transfer_url: &Url,
) -> Step<(reqwest::Client, Vec<(String, String)>)> {
    let Connected {
        client,
        headers,
        unauthenticated,
        profile_headers,
        profile_boundary,
        provider_credential,
    } = connected;
    let headers = if transfer_url == source {
        headers
    } else {
        let mut rebuilt = unauthenticated;
        rebuilt.extend(admitted(
            &profile_headers,
            profile_boundary.as_ref(),
            transfer_url,
        ));
        match provider_authorization(scheduler, provider_credential.as_ref(), transfer_url).await? {
            Ok(Some(header)) => rebuilt.push(header),
            Ok(None) => {}
            Err(failure) => return recorded(scheduler, file, failure).await,
        }
        rebuilt
    };
    Ok(ControlFlow::Continue((client, headers)))
}

/// The engine's request for the probed address.
pub(super) fn request(
    probe_result: ProbeResult,
    part_path: PathBuf,
    total_bytes: Option<u64>,
    chunks: Vec<ChunkSpec>,
    headers: Vec<(String, String)>,
    replay: Option<&ReplayContext>,
    transform: Option<TransformPlan>,
) -> DownloadRequest {
    DownloadRequest {
        url: probe_result.final_url,
        part_path,
        total_bytes,
        etag: probe_result.etag,
        last_modified: probe_result.last_modified,
        use_ranges: probe_result.accepts_ranges,
        chunks,
        headers,
        method: replay.map(|r| r.method).unwrap_or_default(),
        body: replay.and_then(|r| r.body.clone()),
        approved_origins: Arc::new(
            replay
                .map(|r| r.approved_origins.clone())
                .unwrap_or_default(),
        ),
        captured_user_agent: replay.and_then(|r| r.captured_user_agent.clone()),
        // `None` for every ordinary download, which therefore runs exactly the
        // code it ran before the twelfth world existed (RD-110-33).
        transform,
    }
}

/// Runs the transfer, checkpointing into the database.
pub(super) async fn fetch(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    client: reqwest::Client,
    request: DownloadRequest,
    cancellation: CancellationToken,
) -> Result<DownloadOutcome, HttpDownloadError> {
    // Which description the MACs this run produces belong to, so a continuation can tell its
    // own state from somebody else's.
    let mac_stream = request
        .transform
        .as_ref()
        .map(|plan| (file.id, plan.transform.fingerprint().to_owned()));
    let engine = DownloadEngine::new(client, scheduler.scoped_limiter(file).await)
        .with_host_limits(scheduler.host_limits().clone());
    // From here the chunks ask the limiter themselves; the dispatcher stops counting the
    // connections this file promised (RD-1130-02).
    scheduler.host_handed_over(file.id).await;
    engine
        .download(
            request,
            Arc::new(DatabaseCheckpoint {
                database: scheduler.database.clone(),
                mac_stream,
            }),
            cancellation,
        )
        .await
}

/// Records how the transfer ended: the finished file promoted, a stop, or the failure.
pub(super) async fn settle(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    outcome: Result<DownloadOutcome, HttpDownloadError>,
    replay: Option<&ReplayContext>,
    transfer_url: &Url,
    destination: phases::Destination,
) -> Result<()> {
    let phases::Destination {
        root,
        staging,
        part_path,
        final_path,
    } = destination;
    match outcome {
        Ok(DownloadOutcome::Complete) => {
            phases::finish_download(scheduler, file, &root, &staging, &part_path, final_path).await
        }
        Ok(DownloadOutcome::Paused) => transition_stopped(scheduler, file).await,
        Err(error) => {
            let is_post_replay = replay.is_some_and(ReplayContext::is_post);
            // Remembered for the retry: this host does not serve the parts it was asked
            // for, so the next attempt asks for the whole file in one connection instead
            // of repeating the same refusal four times.
            if matches!(error, HttpDownloadError::RangeIgnored) && !is_post_replay {
                scheduler.host_limits().note_ranges_ignored(transfer_url);
            }
            record_http_error_with_replay(scheduler, file, error, is_post_replay).await
        }
    }
}

struct DatabaseCheckpoint {
    database: Database,
    /// Which download and which transform description the chunk MACs belong to. `None` for
    /// an ordinary transfer, which produces none.
    mac_stream: Option<(rd_core::DownloadId, String)>,
}

#[async_trait]
impl CheckpointSink for DatabaseCheckpoint {
    async fn commit(&self, chunk_id: rd_core::ChunkId, committed_offset: u64) -> Result<()> {
        self.database
            .checkpoint_chunk(chunk_id, committed_offset)
            .await
    }

    async fn commit_chunk_mac(&self, index: u64, mac: [u8; 16]) -> Result<()> {
        let Some((download_id, fingerprint)) = &self.mac_stream else {
            // A run with no transform has no MAC to record; a call here would be a bug in
            // the engine rather than something to write down.
            return Ok(());
        };
        self.database
            .checkpoint_chunk_mac(*download_id, fingerprint.clone(), index, mac)
            .await
    }
}
