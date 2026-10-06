//! Probing a download's sources (RD-150-03): each ready source is held to the address rule,
//! probed over HTTP or through its protocol's runner, and kept when it agrees on the file.

use std::ops::ControlFlow;

use anyhow::Result;
use rd_core::{
    ByteCount, DownloadFile, DownloadSource, Failure, FailureKind, SourceOutcome, SourceProtocol,
};
use rd_http::{SourceEndpoint, probe_with_headers};
use tokio_util::sync::CancellationToken;

use super::super::{NetworkClient, provider_authorization, transition_stopped};
use crate::{SchedulerHandle, profile_boundary::admitted};

/// What probing one source gave: its endpoint and the size it offers, `None` when nothing here
/// fetches its protocol, or the source's own failure.
type Probe = std::result::Result<Option<(SourceEndpoint, Option<u64>)>, Failure>;

/// The sources that agree on the file, and the size they agree on, if any names one.
pub(super) struct Probed {
    pub(super) reference: Option<u64>,
    pub(super) endpoints: Vec<SourceEndpoint>,
}

/// Probes the ready sources in order. `Break` when the attempt was cancelled meanwhile: the
/// download is stopped.
pub(super) async fn probe_sources(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    ready: Vec<&DownloadSource>,
    policy: &rd_http::AddressPolicy,
    network: &NetworkClient,
    cancellation: &CancellationToken,
) -> Result<ControlFlow<(), Probed>> {
    // Probe in order and keep the sources that agree on the file. The size the set stated
    // is the reference; without one, the first source that names a size sets it.
    let stated = file.total_bytes.map(ByteCount::get);
    let mut reference = stated;
    let mut endpoints = Vec::new();
    for source in ready {
        if !target_allowed(scheduler, file, policy, source).await? {
            continue;
        }
        let ControlFlow::Continue(probed) =
            probe_one(scheduler, file, network, policy, source, cancellation).await?
        else {
            return Ok(ControlFlow::Break(()));
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
    Ok(ControlFlow::Continue(Probed {
        reference,
        endpoints,
    }))
}

/// Holds a source's address to the rule before it is probed. `false` when it may not be
/// requested — isolated, or a failure noted when its name does not resolve.
async fn target_allowed(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    policy: &rd_http::AddressPolicy,
    source: &DownloadSource,
) -> Result<bool> {
    match rd_http::check_target(policy, &rd_http::SystemLookup, &source.url).await {
        Ok(_) => Ok(true),
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
            Ok(false)
        }
        Err(rd_http::TargetRefusal::Unresolved(error)) => {
            let failure = transient(
                "download.network_failed",
                rd_core::error_with_causes(&error),
            );
            note_failure(scheduler, file, source.position, &failure).await?;
            Ok(false)
        }
    }
}

/// Probes one source over HTTP or through its runner. `Break` when the attempt is cancelled
/// first: the download is stopped.
async fn probe_one(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    network: &NetworkClient,
    policy: &rd_http::AddressPolicy,
    source: &DownloadSource,
    cancellation: &CancellationToken,
) -> Result<ControlFlow<(), Probe>> {
    let probed = if matches!(
        source.protocol,
        SourceProtocol::Http | SourceProtocol::Https
    ) {
        tokio::select! {
            () = cancellation.cancelled() => {
                transition_stopped(scheduler, file).await?;
                return Ok(ControlFlow::Break(()));
            }
            result = http_endpoint(scheduler, network, source) => result?,
        }
    } else {
        tokio::select! {
            () = cancellation.cancelled() => {
                transition_stopped(scheduler, file).await?;
                return Ok(ControlFlow::Break(()));
            }
            result = mirror_endpoint(scheduler, source, policy) => result,
        }
    };
    Ok(ControlFlow::Continue(probed))
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
