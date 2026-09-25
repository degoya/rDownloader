//! The component: a magnet or a `.torrent` in, a job at Real-Debrid out.
//!
//! Nine short calls and no loop. Every function here makes at most two requests and returns
//! on the provider's next answer; nothing waits for a torrent, because the waiting is the
//! host's — it holds the row, the clock and the person's answer. That split is argued in
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`, and it is what lets this plugin keep a
//! fuel and timeout budget a crawler's could not have kept.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "remote-job-plugin",
});

use exports::rdownloader::plugin::remote_job::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, Guest, JobSource, RemoteArtifact, RemoteEntry,
    RemoteHandle, RemoteProgress, RemoteWork, SubmitRequest,
};
use rdownloader::plugin::{
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{
    api::{self, ApiFailure, ErrorKind, Stage},
    messages, source,
};

struct Component;

fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

fn from_api(failure: ApiFailure) -> Failure {
    Failure {
        category: match failure.kind {
            ErrorKind::Transient(seconds) => FailureKind::Transient(seconds),
            ErrorKind::Permanent => FailureKind::Permanent,
            ErrorKind::Offline => FailureKind::Offline,
            ErrorKind::AccountInvalid => FailureKind::AccountInvalid,
            ErrorKind::RateLimited(seconds) => FailureKind::RateLimited(seconds),
            ErrorKind::Unsupported => FailureKind::Unsupported,
            ErrorKind::IpBlocked => FailureKind::IpBlocked(None),
        },
        message: failure.message,
        code: Some(failure.code.to_owned()),
        params: failure
            .params
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    }
}

/// The bearer header, as a template. The token's value never reaches this plugin: the host
/// substitutes it on the way out, towards `api.real-debrid.com` and nowhere else.
fn headers(content_type: Option<&str>) -> Vec<RequestHeader> {
    let mut headers = vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: format!("Bearer {{{{secret:{}}}}}", api::TOKEN_REFERENCE),
        },
        RequestHeader {
            name: "Accept".to_owned(),
            value_template: "application/json".to_owned(),
        },
    ];
    if let Some(value) = content_type {
        headers.push(RequestHeader {
            name: "Content-Type".to_owned(),
            value_template: value.to_owned(),
        });
    }
    headers
}

/// One request, with every status that is not an answer turned into one refusal.
///
/// The vocabulary stays small on purpose: a caller gets bytes or a failure and never decides a
/// second time what a status code means.
fn call(
    method: &str,
    url: &str,
    query: &[RequestQuery],
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Vec<u8>, Failure> {
    let response = http::http_request(method, url, query, &headers(content_type), body)?;
    let retry_after = api::retry_after_seconds(
        response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
            .map(|(_, value)| value.as_str()),
    );
    // An `error_code` decides whatever the status says, and a status decides when there is no
    // document to read. Both directions matter: Real-Debrid answers refusals with 2xx.
    let envelope: api::ErrorEnvelope = serde_json::from_slice(&response.body).unwrap_or_default();
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(from_api(failure));
    }
    Ok(response.body)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Failure> {
    serde_json::from_slice(body)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// The content key of a source, or the refusal that says it is not one of ours.
fn key_of(source: &JobSource) -> Result<String, Failure> {
    let key = match source {
        JobSource::Magnet(address) => source::magnet_info_hash(address),
        JobSource::Container(bytes) => source::container_info_hash(bytes),
        // Real-Debrid's torrent endpoints take a magnet or a `.torrent` file and nothing
        // else, so the third shape of source (RD-120-20) is simply not this plugin's. It is
        // answered here rather than ignored, because an unmatched arm would be a plugin that
        // silently claimed a web download it cannot submit.
        JobSource::Address(_) => None,
    };
    key.ok_or_else(|| refuse(messages::NOT_A_TORRENT, FailureKind::Unsupported))
}

/// Checks an identifier before it is spliced into a request path.
fn safe_id(handle: &RemoteHandle) -> Result<&str, Failure> {
    if api::is_safe_remote_id(&handle.remote_id) {
        Ok(&handle.remote_id)
    } else {
        Err(refuse(messages::TORRENT_GONE, FailureKind::Permanent))
    }
}

fn handle_for(account_id: &str, remote_id: String) -> RemoteHandle {
    RemoteHandle {
        remote_id,
        account_id: account_id.to_owned(),
        // Nothing to carry: the identifier is the whole handle, and a plugin that put
        // something here would be storing state the host would have to keep for no reason.
        job_state: None,
    }
}

impl Guest for Component {
    /// None. Real-Debrid switched its `instantAvailability` endpoint off, and nothing else in
    /// its API says whether a torrent is held without adding it -- which is a submit, not a
    /// question (RD-130-11). So the host never calls `check_cached` here.
    fn cache_kinds() -> Vec<CacheKind> {
        Vec::new()
    }

    /// Never reached while `cache_kinds` is empty. Answers every query `unknown`, one per
    /// query and in order, without a request -- "nothing to say" is never a failure.
    fn check_cached(
        _account_id: String,
        queries: Vec<CacheQuery>,
    ) -> Result<Vec<CacheAnswer>, Failure> {
        Ok(queries
            .iter()
            .map(|_| CacheAnswer {
                state: CacheState::Unknown,
                file_name: None,
                size: None,
            })
            .collect())
    }

    /// Reaches nothing. Asked of every source before anything is handed to anybody, and
    /// answered from the bytes alone.
    fn claims(source: JobSource) -> bool {
        match &source {
            JobSource::Magnet(address) => source::magnet_info_hash(address).is_some(),
            JobSource::Container(bytes) => source::container_info_hash(bytes).is_some(),
            JobSource::Address(_) => false,
        }
    }

    /// The BitTorrent info hash, lower-case hex, derived locally and identically for a magnet
    /// and for the `.torrent` file of the same content.
    ///
    /// It is also the field Real-Debrid calls `hash`, which is what lets `adopt` recognise a
    /// torrent this installation may have created seconds before a crash.
    fn identify(source: JobSource) -> Result<String, Failure> {
        key_of(&source)
    }

    /// Hands the source over. One request, no retry, no loop.
    ///
    /// Not idempotent, and deliberately not made to look idempotent here: a retry hidden in
    /// this function would be the duplicate the host's row exists to prevent, moved somewhere
    /// nobody would think to look for it.
    fn submit(request: SubmitRequest) -> Result<RemoteHandle, Failure> {
        let body = match &request.source {
            JobSource::Magnet(address) => api::magnet_body(address),
            JobSource::Container(bytes) => bytes.clone(),
            JobSource::Address(_) => {
                return Err(refuse(messages::NOT_A_TORRENT, FailureKind::Unsupported));
            }
        };
        let (method, url, content_type) = match &request.source {
            JobSource::Magnet(_) => (
                "POST",
                format!("{}/torrents/addMagnet", api::API_BASE),
                Some("application/x-www-form-urlencoded"),
            ),
            JobSource::Container(_) | JobSource::Address(_) => (
                "PUT",
                format!("{}/torrents/addTorrent", api::API_BASE),
                Some("application/x-bittorrent"),
            ),
        };
        let answer: api::AddedTorrent = parse(&call(method, &url, &[], content_type, &body)?)?;
        let Some(id) = answer.id.filter(|id| api::is_safe_remote_id(id)) else {
            // Something may well have been created and this installation cannot name it. The
            // host is told plainly rather than being handed an empty handle it would poll for
            // ever; its adoption check is what looks for the orphan.
            return Err(refuse(messages::NO_TORRENT_ID, FailureKind::Permanent));
        };
        Ok(handle_for(&request.account_id, id))
    }

    /// The torrent the account already holds for this info hash, if any.
    ///
    /// One page of `GET /torrents`, and no paging beyond it: this exists for a submit that was
    /// lost seconds ago, which is the newest entry there is, and an account with a thousand
    /// torrents must not cost a thousand requests against a budget of 250 a minute.
    fn adopt(account_id: String, content_key: String) -> Result<Option<RemoteHandle>, Failure> {
        let body = call(
            "GET",
            &format!("{}/torrents", api::API_BASE),
            &[RequestQuery {
                name: "limit".to_owned(),
                value_template: api::ADOPT_PAGE.to_string(),
            }],
            None,
            &[],
        )?;
        let listed: Vec<api::ListedTorrent> = parse(&body)?;
        let found = listed
            .into_iter()
            .find(|torrent| {
                torrent
                    .hash
                    .as_deref()
                    .is_some_and(|hash| hash.eq_ignore_ascii_case(&content_key))
            })
            .and_then(|torrent| torrent.id)
            .filter(|id| api::is_safe_remote_id(id));
        Ok(found.map(|id| handle_for(&account_id, id)))
    }

    fn poll(handle: RemoteHandle) -> Result<RemoteProgress, Failure> {
        let id = safe_id(&handle)?;
        let body = call(
            "GET",
            &format!("{}/torrents/info/{id}", api::API_BASE),
            &[],
            None,
            &[],
        )?;
        let info: api::TorrentInfo = parse(&body)?;
        let status = info.status.clone().unwrap_or_default();
        Ok(match api::stage_of(&status) {
            Stage::Preparing(seconds) => RemoteProgress::Preparing(Some(seconds)),
            Stage::AwaitingChoice => RemoteProgress::AwaitingChoice(entries_of(&info)),
            // `working` carries no suggested wait: a running torrent is polled on the
            // host's own interval, and the one this plugin would name travels on
            // `preparing`, where there is a field for it.
            Stage::Working(_) => RemoteProgress::Working(RemoteWork {
                progress_permille: api::permille(info.progress),
                speed_bytes_per_second: info.speed,
                seconds_remaining: None,
            }),
            Stage::Ready => ready_of(&info)?,
            Stage::Failed(message) => {
                RemoteProgress::Failed(refuse(message, FailureKind::Permanent))
            }
        })
    }

    /// Answers the question. The host never sends an empty list, so this never has to decide
    /// what "nothing" would have meant.
    fn choose(handle: RemoteHandle, chosen: Vec<u32>) -> Result<(), Failure> {
        let id = safe_id(&handle)?;
        if chosen.is_empty() {
            return Err(refuse(messages::EMPTY_CHOICE, FailureKind::Permanent));
        }
        call(
            "POST",
            &format!("{}/torrents/selectFiles/{id}", api::API_BASE),
            &[],
            Some("application/x-www-form-urlencoded"),
            &api::selection_body(&chosen),
        )?;
        Ok(())
    }

    /// Removes the torrent at Real-Debrid. Nothing in this plugin calls it.
    fn discard(handle: RemoteHandle) -> Result<(), Failure> {
        let id = safe_id(&handle)?;
        call(
            "DELETE",
            &format!("{}/torrents/delete/{id}", api::API_BASE),
            &[],
            None,
            &[],
        )?;
        Ok(())
    }
}

fn entries_of(info: &api::TorrentInfo) -> Vec<RemoteEntry> {
    info.files
        .iter()
        .filter_map(|file| {
            let id = u32::try_from(file.id?).ok()?;
            Some(RemoteEntry {
                id,
                path: file.path.clone().unwrap_or_default(),
                size: file.bytes,
                selected: api::is_selected(file.selected),
            })
        })
        .collect()
}

fn ready_of(info: &api::TorrentInfo) -> Result<RemoteProgress, Failure> {
    let torrent_name = info.filename.clone().unwrap_or_default();
    let artifacts: Vec<RemoteArtifact> = api::pair_links(&info.links, &info.files)
        .into_iter()
        .map(|(url, file)| {
            let path = file.and_then(|file| file.path.clone()).unwrap_or_default();
            let (file_name, package_hint) = api::place(&torrent_name, &path);
            RemoteArtifact {
                url: url.to_owned(),
                file_name,
                size: file.and_then(|file| file.bytes),
                package_hint,
            }
        })
        .collect();
    if artifacts.is_empty() {
        // Finished with nothing is not a result. A package with no files in it and nothing to
        // explain why is the shape of the defect ADR 0001 was opened for.
        return Err(refuse(messages::NO_LINKS, FailureKind::Permanent));
    }
    Ok(RemoteProgress::Ready(artifacts))
}

export!(Component);
