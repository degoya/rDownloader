//! Resolving the re-resolved files at once, without transferring them (RD-1250-05).
//!
//! A tester asked for it: "Re-resolve" only dropped the pin, so the size of a file was known
//! once its download started. Each file now goes through the resolver part of a worker's
//! attempt (`SchedulerHandle::resolve_now`) and a plain direct link through the LinkGrabber's
//! HEAD probe; the answer is written back with `Database::record_resolution` and named per file.
//!
//! Only links with an account are resolved ahead (owner, 2026-10-11): a hoster link without one
//! would run the hoster's free flow — a captcha, a wait, the one free download per IP — so it is
//! skipped, nothing written, and named in the answer with [`CODE_RESOLVE_SKIPPED_FREE`]. A plain
//! direct link no resolver claims is no hoster link and is probed.
//!
//! Bounded the way the queue is: at most [`PER_HOST`] files of one host at a time, and
//! [`AT_ONCE`] in all. One file is given [`RESOLVE_TIMEOUT`]; a longer one is answered as timed
//! out and resolves at its next start as before. A file paused for this is started again as soon
//! as its own resolution is over, not after the whole selection.

use std::{collections::HashMap, sync::Arc, time::Duration};

use rd_core::{DownloadFile, DownloadKind, Failure, FailureKind, LinkStatus};
use rd_db::StoreErrorKind;
use rd_scheduler::ResolvedNow;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    ApiError, AppState,
    dto::{ReresolveResponse, ReresolveResult},
};

/// Files of one host resolved at the same time.
const PER_HOST: usize = 2;
/// Files resolved at the same time in all.
const AT_ONCE: usize = 8;
/// How long one file's resolution may take.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(60);

/// Stable code: the resolution took longer than [`RESOLVE_TIMEOUT`].
const CODE_RESOLVE_TIMEOUT: &str = "download.resolve_timeout";
/// Stable code: the server answered a direct link with 404, 410 or 451.
const CODE_RESOLVE_OFFLINE: &str = "download.resolve_offline";
/// Stable code: the resolution failed on this side (the store, a client that could not be built).
const CODE_RESOLVE_FAILED: &str = "download.resolve_failed";
/// Stable code: a hoster link without an account, not resolved ahead.
const CODE_RESOLVE_SKIPPED_FREE: &str = "download.resolve_skipped_free";

/// A file whose pin was dropped, and whether it was paused for it.
pub(super) struct Held {
    pub(super) file: DownloadFile,
    pub(super) paused: bool,
}

/// Resolves every held file, writes back what was found and adds it to `answer` in the
/// request's order.
pub(super) async fn resolve_all(state: &AppState, held: Vec<Held>, answer: &mut ReresolveResponse) {
    let all = Arc::new(Semaphore::new(AT_ONCE));
    let mut hosts: HashMap<String, Arc<Semaphore>> = HashMap::new();
    let mut tasks = JoinSet::new();
    for (index, held) in held.into_iter().enumerate() {
        let host = Arc::clone(
            hosts
                .entry(host_key(&held.file.source))
                .or_insert_with(|| Arc::new(Semaphore::new(PER_HOST))),
        );
        let all = Arc::clone(&all);
        let state = state.clone();
        tasks.spawn(async move {
            // The host first: a file waiting for its host holds no place of another host's.
            let result = match host.acquire_owned().await {
                Ok(_host) => match all.acquire_owned().await {
                    Ok(_all) => resolve_one(&state, &held.file).await,
                    Err(_) => None,
                },
                Err(_) => None,
            };
            let resumed = if held.paused {
                state
                    .scheduler
                    .resume(held.file.id)
                    .await
                    .map_err(super::refusal)
            } else {
                Ok(())
            };
            (index, held.file.id, result, resumed)
        });
    }
    let mut finished = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(done) => finished.push(done),
            Err(error) => tracing::warn!(%error, "a re-resolve task ended early"),
        }
    }
    finished.sort_by_key(|(index, ..)| *index);
    for (_, id, result, resumed) in finished {
        if let Some(result) = result {
            if result.skipped {
                answer.skipped += 1;
            } else if matches!(result.status, LinkStatus::Online | LinkStatus::Cached) {
                answer.resolved += 1;
            } else {
                answer.failed += 1;
            }
            answer.results.push(result);
        }
        if let Err(error) = resumed {
            refuse(answer, id, error);
        }
    }
}

fn refuse(answer: &mut ReresolveResponse, id: rd_core::DownloadId, error: ApiError) {
    answer.errors.push(format!("{id}: {}", error.message()));
    answer.refusals.push(error.into_message());
}

/// The host files are counted against, without a leading `www.`.
fn host_key(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if let Some(rest) = host.strip_prefix("www.") {
        return rest.to_owned();
    }
    host
}

/// Resolves one file and writes back what was found; `None` for a kind with no resolver step.
async fn resolve_one(state: &AppState, file: &DownloadFile) -> Option<ReresolveResult> {
    // Usenet, torrents, media, galleries and remote folders are read by their own runners at
    // the start; there is nothing a resolver would say about them first.
    if file.kind != DownloadKind::Http {
        return None;
    }
    let found = match tokio::time::timeout(RESOLVE_TIMEOUT, find(state, file)).await {
        Ok(found) => found,
        Err(_) => Found::failed(
            LinkStatus::Unknown,
            Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                CODE_RESOLVE_TIMEOUT,
                "The link could not be resolved in time; it resolves at its next start",
            ),
        ),
    };
    let file_name = found.file_name.as_deref().map(rd_files::sanitize_file_name);
    // Only a conclusion is written: a resolution, a file that is gone or is no file. An
    // inconclusive answer — a timeout, a busy hoster, a skipped link — says nothing about it.
    if found.status != LinkStatus::Unknown {
        let written = state
            .database
            .record_resolution(
                file.id,
                file_name.clone(),
                found.total_bytes.map(rd_core::ByteCount::get),
                found.failure.clone(),
            )
            .await;
        match written {
            Ok(()) => {}
            // Started meanwhile: its worker resolves it and writes the answer itself.
            Err(error) if rd_db::store_kind(&error) == Some(StoreErrorKind::WrongState) => {}
            Err(error) => {
                tracing::warn!(download_id = %file.id, %error, "the resolution was not stored");
            }
        }
    }
    Some(ReresolveResult {
        download_id: file.id,
        status: found.status,
        file_name,
        total_bytes: found.total_bytes,
        skipped: found.skipped,
        failure: found.failure.map(rd_core::redact_failure),
    })
}

/// A hoster link — one an installed resolver or a registered provider claims — that carries
/// no account. Resolving it would run the hoster's free flow (captcha, wait, the one free
/// download per IP) for a size, so it is skipped (owner, 2026-10-11). A plain direct link is no
/// hoster link.
fn is_hoster_link_without_account(state: &AppState, file: &DownloadFile) -> bool {
    file.account_id.is_none()
        && (state.scheduler.resolvers().has_resolver(&file.source)
            || rd_provider_registry::provider_for_url(&file.source).is_some())
}

/// What resolving one file found, before it is written.
struct Found {
    status: LinkStatus,
    file_name: Option<String>,
    total_bytes: Option<rd_core::ByteCount>,
    skipped: bool,
    failure: Option<Failure>,
}

impl Found {
    fn failed(status: LinkStatus, failure: Failure) -> Self {
        Self {
            status,
            file_name: None,
            total_bytes: None,
            skipped: false,
            failure: Some(failure),
        }
    }
}

/// Asks the resolver step, then probes a plain direct link.
async fn find(state: &AppState, file: &DownloadFile) -> Found {
    if is_hoster_link_without_account(state, file) {
        return Found {
            skipped: true,
            ..Found::failed(
                LinkStatus::Unknown,
                Failure::coded(
                    FailureKind::AuthRequired,
                    CODE_RESOLVE_SKIPPED_FREE,
                    "A hoster link without an account is not resolved ahead; it resolves at its \
                     start",
                ),
            )
        };
    }
    let network = match state.scheduler.resolve_now(file).await {
        Ok(Ok(ResolvedNow::Resolved(resolved))) => {
            return Found {
                status: LinkStatus::Online,
                file_name: resolved.file_name,
                total_bytes: resolved.size,
                skipped: false,
                failure: None,
            };
        }
        Ok(Ok(ResolvedNow::Direct(network))) => network,
        // The plugin's own answer: gone is a conclusion, everything else is not.
        Ok(Err(failure)) => {
            let status = if failure.category == FailureKind::Offline {
                LinkStatus::Offline
            } else {
                LinkStatus::Unknown
            };
            return Found::failed(status, failure);
        }
        Err(error) => {
            tracing::warn!(download_id = %file.id, %error, "the link could not be resolved now");
            return Found::failed(
                LinkStatus::Unknown,
                Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    CODE_RESOLVE_FAILED,
                    "The link could not be resolved now; it resolves at its next start",
                ),
            );
        }
    };
    // A plain direct link: the LinkGrabber's probe, with the same verdicts.
    let probed = rd_api_core::link_check_probe::probe_direct(
        &network.client,
        &network.headers,
        file.source.clone(),
    )
    .await;
    let failure = match probed.status {
        LinkStatus::Offline => Some(Failure::coded(
            FailureKind::Offline,
            CODE_RESOLVE_OFFLINE,
            "The server says the file is gone",
        )),
        LinkStatus::Unresolvable => Some(Failure::coded(
            FailureKind::Permanent,
            "collector.check_not_a_file",
            "The address answered with a page rather than a file",
        )),
        LinkStatus::Unknown => Some(Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            "collector.check_inconclusive",
            "The server did not answer the check, so the link could not be confirmed",
        )),
        LinkStatus::Online | LinkStatus::Cached => None,
    };
    Found {
        status: probed.status,
        file_name: probed.file_name,
        total_bytes: probed.size,
        skipped: false,
        failure,
    }
}
