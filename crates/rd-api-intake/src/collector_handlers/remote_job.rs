//! A LinkGrabber package handed to a remote-job provider instead of the queue (RD-1440-07).
//!
//! Every link of the package becomes one remote-job source -- a magnet, a plain address, or the
//! stored `.torrent` of an imported file -- and goes through `submit_named`, the entry point of
//! every remote job, so the same duplicate guard and the same plugin `identify` decide what is
//! taken. The provider fetches the content on its side, and what it finished comes back into the
//! LinkGrabber as a package of its own (`RemoteJob::package_id`).
//!
//! No credential in an address goes along (RD-1480-05). An NZB link carries the indexer's API
//! key, so its document is fetched here and handed over as a container (`nzb`), as a
//! subscription's hit is (RD-1460-03) -- a link the online check has not recognised included,
//! when it has an indexer's form or sits at a configured one; any other address with a user name,
//! a password or a secret query parameter stays.
//!
//! What becomes of the links afterwards is a setting, read here and never taken from the
//! request (owner, 2026-11-21): `linkgrabber_hand_over_remove_links` on, the default, takes each
//! handed-over link out of the LinkGrabber and a package left empty with it; off, the links stay
//! marked with their job, and a second hand-over reports that job instead of sending anything.
//! Either way, a link that could not go -- refused, not claimed, a source no provider takes, an
//! error -- stays, and the answer names it with its code. A job the account already had
//! (`AlreadyOurs`) counts as handed over: the content is at the provider.

use std::path::{Path as FsPath, PathBuf};

use axum::extract::State;
use rd_api_core::extract::{Json, Path};
use rd_api_core::remote_job_service::{self, MAX_REMOTE_JOB_CONTAINER_BYTES, SubmitOutcome};
use rd_core::{
    AccountId, CandidateId, CollectorPackage, CollectorPackageId, LinkCandidate,
    LinkCandidateState, RemoteJob,
};
use rd_db::HandedOverLinks;
use rd_plugin_host::extension::RemoteJobSource;
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{ApiError, AppState};

mod nzb;

/// Longest address handed over, the ceiling `POST /api/v1/accounts/{id}/remote-jobs` keeps too.
const MAX_ADDRESS: usize = 8 * 1024;

/// Which account's provider a LinkGrabber package goes to.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CollectorPackageRemoteJobRequest {
    /// A remote-job account (`GET /api/v1/remote-jobs/providers` names the providers that run
    /// remote jobs, with `container=torrent` those that take a `.torrent` file).
    pub account_id: AccountId,
}

/// What handing a LinkGrabber package over produced, link by link.
#[derive(Debug, Serialize, ToSchema)]
pub struct CollectorPackageRemoteJobResponse {
    /// The links that are at the provider now, each with its job.
    pub handed_over: Vec<CandidateHandedOver>,
    /// The links that stayed in the LinkGrabber, each with the reason.
    pub stayed: Vec<CandidateStayed>,
    /// Whether the links handed over left the LinkGrabber (`linkgrabber_hand_over_remove_links`)
    /// or stay marked with their job.
    pub links_removed: bool,
    /// Whether the package left the LinkGrabber because no link was left in it.
    pub package_removed: bool,
}

/// One link that reached the provider.
#[derive(Debug, Serialize, ToSchema)]
pub struct CandidateHandedOver {
    pub candidate_id: CandidateId,
    pub job: RemoteJob,
    /// True when the account already had a job for this content, or the link was marked with
    /// one by an earlier hand-over. Nothing was written and nothing was sent.
    pub already_running: bool,
}

/// One link that stayed, and why.
#[derive(Debug, Serialize, ToSchema)]
pub struct CandidateStayed {
    pub candidate_id: CandidateId,
    /// Stable code: `collector.remote_job_*`, `collector.nzb_*`, `nzb.remote_job_*`,
    /// `remote_job.*`, or a provider's own.
    pub code: String,
    /// The English wording, shown when no catalogue translates the code.
    pub message: String,
    /// What the translation of the code names (an indexer's reason, a limit).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: rd_api_core::error::MessageParams,
}

/// Hands every link of a LinkGrabber package to one account's provider as remote jobs.
///
/// Refused before anything leaves the machine when the package or the account does not exist,
/// or when no installed plugin runs remote jobs on the account's provider. Past that the answer
/// is `200` whatever each link's outcome: what went and what stayed are both listed.
#[utoipa::path(post, path = "/api/v1/collector/packages/{id}/remote-job", tag = "collector", params(("id" = CollectorPackageId, Path)), request_body = CollectorPackageRemoteJobRequest, responses((status = 200, body = CollectorPackageRemoteJobResponse), (status = 400, description = "No installed plugin runs remote jobs on the account's provider"), (status = 404, description = "The package or the account does not exist")))]
pub async fn submit_collector_package_remote_job(
    State(state): State<AppState>,
    Path(id): Path<CollectorPackageId>,
    Json(request): Json<CollectorPackageRemoteJobRequest>,
) -> Result<Json<CollectorPackageRemoteJobResponse>, ApiError> {
    let package = state
        .database
        .get_collector_package(id)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    runs_remote_jobs(&state, request.account_id).await?;
    let links = if crate::settings_store::read_settings(&state)
        .await?
        .linkgrabber_hand_over_remove_links
    {
        HandedOverLinks::Remove
    } else {
        HandedOverLinks::Mark
    };
    let candidates: Vec<LinkCandidate> = state
        .database
        .list_candidates()
        .await?
        .into_iter()
        .filter(|candidate| candidate.package_id == Some(id))
        .collect();
    let torrents = state.queue.torrent.torrent_file_directory();
    let indexers = nzb::IndexerHosts::read(&state).await?;
    let mut handed_over = Vec::new();
    let mut stayed = Vec::new();
    // One after another: the jobs are charged to one account, and the provider sees them in
    // the package's order.
    for candidate in &candidates {
        let handed = hand_over(
            &state,
            &package,
            candidate,
            request.account_id,
            &torrents,
            &indexers,
        );
        match handed.await {
            Ok(done) => handed_over.push(done),
            Err(reason) => stayed.push(reason),
        }
    }
    let settled = handed_over
        .iter()
        .map(|done| (done.candidate_id, done.job.id))
        .collect();
    state
        .database
        .settle_handed_over_candidates(settled, links)
        .await?;
    if links == HandedOverLinks::Remove && !handed_over.is_empty() {
        crate::torrent_intake::prune_checked_torrents(&state).await;
    }
    let package_removed = state.database.get_collector_package(id).await?.is_none();
    tracing::info!(
        collector_package = %id,
        account = %request.account_id,
        handed_over = handed_over.len(),
        stayed = stayed.len(),
        links_removed = links == HandedOverLinks::Remove,
        "a LinkGrabber package was handed to a provider"
    );
    Ok(Json(CollectorPackageRemoteJobResponse {
        handed_over,
        stayed,
        links_removed: links == HandedOverLinks::Remove,
        package_removed,
    }))
}

/// The account exists and an installed plugin runs remote jobs on its provider.
async fn runs_remote_jobs(state: &AppState, account_id: AccountId) -> Result<(), ApiError> {
    let account = state
        .database
        .get_account(account_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                "remote_job.no_account",
                "the account this job would run on does not exist",
            )
        })?;
    if !state
        .queue
        .remote_jobs
        .providers()
        .await?
        .contains(&account.provider.to_ascii_lowercase())
    {
        return Err(ApiError::bad_request(
            "collector.remote_job_not_remote_account",
            "No installed plugin runs remote jobs on this account's provider",
        ));
    }
    Ok(())
}

/// One link to the provider, or the reason it stays.
async fn hand_over(
    state: &AppState,
    package: &CollectorPackage,
    candidate: &LinkCandidate,
    account_id: AccountId,
    torrents: &FsPath,
    indexers: &nzb::IndexerHosts,
) -> Result<CandidateHandedOver, CandidateStayed> {
    let stay = |code: &str, message: &str| CandidateStayed {
        candidate_id: candidate.id,
        code: code.to_owned(),
        message: message.to_owned(),
        params: Default::default(),
    };
    let handed = |job: RemoteJob, already_running: bool| CandidateHandedOver {
        candidate_id: candidate.id,
        job,
        already_running,
    };
    // Marked by an earlier hand-over: that job is the answer, and nothing is sent again.
    if let Some(mark) = &candidate.handed_over {
        match state.database.remote_job(mark.remote_job_id).await {
            Ok(Some(job)) => return Ok(handed(job, true)),
            // Forgotten in between; the mark goes with the row, so the link is free again.
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(candidate = %candidate.id, %error, "the marked job could not be read");
                return Err(stay(FAILED, "The link could not be handed over"));
            }
        }
    }
    if let Some(reason) = held_back(candidate) {
        return Err(stay(reason.0, reason.1));
    }
    let shape = shape_of(&candidate.url);
    if shape == Shape::Address && nzb::is_nzb_call(candidate, indexers) {
        return match nzb::submit(state, package, candidate, account_id).await {
            Ok(done) => Ok(handed(done.job, done.already_running)),
            Err(error) => {
                // The code alone: a fetch's reason is for the person, not for the log.
                tracing::info!(candidate = %candidate.id, code = error.code(), "an NZB link was not handed over");
                let reason = error.into_message();
                Err(CandidateStayed {
                    candidate_id: candidate.id,
                    code: reason.code,
                    message: reason.message,
                    params: reason.params,
                })
            }
        };
    }
    let source = match shape {
        Shape::Magnet => RemoteJobSource::Magnet(candidate.url.as_str().to_owned()),
        Shape::Address if carries_credential(&candidate.url) => {
            return Err(stay(
                SECRET_ADDRESS,
                "The address of this link carries a login or an access key, which stays on this machine",
            ));
        }
        Shape::Address if candidate.url.as_str().len() <= MAX_ADDRESS => {
            RemoteJobSource::Address(candidate.url.as_str().to_owned())
        }
        Shape::File(path) => match stored_torrent(&path, torrents).await {
            Some(bytes) => RemoteJobSource::Container(bytes),
            None => {
                return Err(stay(
                    TORRENT_UNREADABLE,
                    "The torrent file behind this link is not in the torrent store or cannot be read",
                ));
            }
        },
        Shape::Address | Shape::Unsupported => {
            return Err(stay(
                SOURCE_UNSUPPORTED,
                "Only a magnet, an http(s) address or an imported torrent file can go to a provider",
            ));
        }
    };
    let name = remote_job_service::source_name(candidate.file_name.as_deref());
    match state
        .queue
        .remote_jobs
        .submit_named(account_id, source, name)
        .await
    {
        Ok(SubmitOutcome::Started(job)) => Ok(handed(job, false)),
        Ok(SubmitOutcome::AlreadyOurs(job)) => Ok(handed(job, true)),
        Ok(SubmitOutcome::NotClaimed) => Err(stay(
            "remote_job.not_claimed",
            "no installed plugin runs a job like this on this account's provider",
        )),
        Ok(SubmitOutcome::Refused(refusal)) => Err(CandidateStayed {
            candidate_id: candidate.id,
            code: refusal.code,
            message: refusal.message,
            params: Default::default(),
        }),
        Err(error) => {
            tracing::warn!(candidate = %candidate.id, %error, "a link could not be handed over");
            Err(stay(FAILED, "The link could not be handed over"))
        }
    }
}

const BUSY: &str = "collector.remote_job_busy";
const HIDDEN: &str = "collector.remote_job_hidden";
const MIRROR_SKIPPED: &str = "collector.remote_job_mirror_skipped";
const SECRET_ADDRESS: &str = "collector.remote_job_secret_address";
const SECRET_FRAGMENT: &str = "collector.remote_job_secret_fragment";
const SOURCE_UNSUPPORTED: &str = "collector.remote_job_source_unsupported";
const TORRENT_UNREADABLE: &str = "collector.remote_job_torrent_unreadable";
const FAILED: &str = "collector.remote_job_failed";

/// Why a link is not offered to the provider at all, before its source is looked at.
///
/// The same links the enqueue leaves out stay out here: one being queued, one a LinkFilter rule
/// hid, a mirror that is not the group's chosen copy -- handing every mirror over would start
/// the same file once per host. A link whose key the vault keeps (RD-110-38) stays too: the
/// stored address lacks the key, and a provider given it could only fail.
fn held_back(candidate: &LinkCandidate) -> Option<(&'static str, &'static str)> {
    if candidate.state == LinkCandidateState::Resolving {
        return Some((BUSY, "The link is being queued"));
    }
    if candidate.hidden_by_filter.is_some() {
        return Some((HIDDEN, "A LinkFilter rule hides this link"));
    }
    if candidate
        .mirror
        .as_ref()
        .is_some_and(|mirror| !mirror.selected)
    {
        return Some((
            MIRROR_SKIPPED,
            "Another mirror of this file is the one handed over",
        ));
    }
    if candidate.secret_fragment {
        return Some((
            SECRET_FRAGMENT,
            "The key of this link is kept on this machine and does not travel to a provider",
        ));
    }
    None
}

/// What kind of remote-job source a link's address makes.
#[derive(Debug, Eq, PartialEq)]
enum Shape {
    Magnet,
    Address,
    /// A local file: only an imported `.torrent` in the torrent store goes.
    File(PathBuf),
    Unsupported,
}

fn shape_of(url: &Url) -> Shape {
    match url.scheme() {
        "magnet" => Shape::Magnet,
        "http" | "https" => Shape::Address,
        "file" => url
            .to_file_path()
            .ok()
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("torrent"))
            })
            .map_or(Shape::Unsupported, Shape::File),
        _ => Shape::Unsupported,
    }
}

/// Whether an address names a login or a secret the provider would keep in its job list: user
/// info, or a query parameter the central redaction hides (`rd_core::is_secret_parameter`).
fn carries_credential(url: &Url) -> bool {
    !url.username().is_empty()
        || url.password().is_some()
        || url
            .query_pairs()
            .any(|(name, value)| !value.is_empty() && rd_core::is_secret_parameter(&name))
}

/// The bytes of an imported `.torrent`, only from inside the torrent store and only up to what
/// a remote job's container may carry. Both paths are resolved first, so neither `..` nor a
/// link can reach a file elsewhere on the machine.
async fn stored_torrent(path: &FsPath, torrents: &FsPath) -> Option<Vec<u8>> {
    let directory = tokio::fs::canonicalize(torrents).await.ok()?;
    let file = tokio::fs::canonicalize(path).await.ok()?;
    if !file.starts_with(&directory) {
        return None;
    }
    let length = tokio::fs::metadata(&file).await.ok()?.len();
    if length == 0 || usize::try_from(length).ok()? > MAX_REMOTE_JOB_CONTAINER_BYTES {
        return None;
    }
    tokio::fs::read(&file).await.ok()
}

#[cfg(test)]
mod tests {
    use super::{Shape, carries_credential, shape_of, stored_torrent};

    fn url(value: &str) -> url::Url {
        value.parse().expect("url")
    }

    /// A directory of its own under the system's temporary one; the crate has no `tempfile`.
    fn scratch() -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("rd-1440-07-{}", rd_core::CandidateId::new()));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    #[test]
    fn a_link_becomes_the_source_its_address_names() {
        assert_eq!(
            shape_of(&url(
                "magnet:?xt=urn:btih:c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2c8f1a0b2"
            )),
            Shape::Magnet
        );
        assert_eq!(
            shape_of(&url("https://files.example/a.mkv")),
            Shape::Address
        );
        assert_eq!(shape_of(&url("http://files.example/a.nzb")), Shape::Address);
        assert_eq!(
            shape_of(&url("ftp://files.example/a.mkv")),
            Shape::Unsupported
        );
        assert_eq!(
            shape_of(&url("sftp://files.example/a.mkv")),
            Shape::Unsupported
        );
        // A local file goes only as a torrent; anything else on disk stays.
        let directory = scratch();
        let torrent = directory.join("abc.torrent");
        let file = url::Url::from_file_path(&torrent).expect("file url");
        assert_eq!(shape_of(&file), Shape::File(torrent));
        let other = url::Url::from_file_path(directory.join("notes.txt")).expect("file url");
        assert_eq!(shape_of(&other), Shape::Unsupported);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_address_with_a_login_or_a_key_is_recognised() {
        for secret in [
            "https://indexer.example/api?t=get&id=1&apikey=SECRET",
            "https://files.example/a.mkv?X-Amz-Signature=SECRET",
            "https://files.example/a.mkv?Token=SECRET#part",
            "https://user:SECRET@files.example/a.mkv",
            "https://user@files.example/a.mkv",
        ] {
            assert!(carries_credential(&url(secret)), "{secret}");
        }
        for plain in [
            "https://files.example/a.mkv",
            "https://files.example/a.mkv?lang=de&page=2",
            "https://files.example/a.mkv?token=",
            "https://files.example/apikey/a.mkv",
        ] {
            assert!(!carries_credential(&url(plain)), "{plain}");
        }
    }

    #[tokio::test]
    async fn only_a_torrent_inside_the_store_is_read() {
        let directory = scratch();
        let store = directory.join("torrents");
        std::fs::create_dir_all(&store).expect("store");
        let inside = store.join("abc.torrent");
        std::fs::write(&inside, b"d4:infod4:name1:aee").expect("torrent");
        let outside = directory.join("elsewhere.torrent");
        std::fs::write(&outside, b"d4:infod4:name1:bee").expect("torrent");
        let escaping = store.join("..").join("elsewhere.torrent");

        assert_eq!(
            stored_torrent(&inside, &store).await.as_deref(),
            Some(&b"d4:infod4:name1:aee"[..])
        );
        assert_eq!(stored_torrent(&outside, &store).await, None);
        assert_eq!(stored_torrent(&escaping, &store).await, None);
        assert_eq!(
            stored_torrent(&store.join("gone.torrent"), &store).await,
            None
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
