//! The component: a magnet or a `.torrent` in, a transfer at Put.io out.
//!
//! Nine short calls and one loop. Six of the calls make a single request and return on Put.io's
//! next answer; nothing waits for a transfer, because the waiting is the host's — it holds the
//! row, the clock and the restart. That split is argued in
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! The one loop is the walk over a finished transfer's folder, and it is bounded twice over:
//! `api::MAX_TREE_REQUESTS` caps how many listings it may ask for and `api::MAX_FILES` how many
//! files it may hand over. It exists because Put.io's transfer record names a folder and not
//! files, and a package whose members had no names and no sizes is not a file tree anybody can
//! choose from.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "remote-job-plugin",
});

use exports::rdownloader::plugin::remote_job::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, Guest, JobSource, RemoteArtifact, RemoteHandle,
    RemoteProgress, RemoteWork, SubmitRequest,
};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{
    api::{self, ApiFailure, ErrorKind, Stage},
    messages, source,
};
use putio_common::{address, reason::ErrorEnvelope};

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
/// substitutes it on the way out, towards `api.put.io` and nowhere else.
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
/// second time what a status code meant.
fn call(
    method: &str,
    url: &str,
    query: &[RequestQuery],
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Vec<u8>, Failure> {
    let response = http::http_request(method, url, query, &headers(content_type), body)?;
    let envelope = ErrorEnvelope::of(&response.body);
    // The clock is asked for only when it is needed: `now-unix-seconds` is a host call, and a
    // rate-limit header is on the one answer in a thousand that is a rate limit.
    let reset = if response.status == 429 {
        api::rate_limit_wait(
            response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("x-ratelimit-reset"))
                .map(|(_, value)| value.as_str()),
            host::now_unix_seconds(),
        )
    } else {
        None
    };
    match api::failure_from(response.status, reset, &envelope) {
        Some(failure) => Err(from_api(failure)),
        None => Ok(response.body),
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Failure> {
    serde_json::from_slice(body)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// The address Put.io is handed for a source, or the refusal that says it is not one of ours.
///
/// A magnet travels verbatim. A container becomes the magnet it is equivalent to, which is the
/// trade-off `source::container_magnet` argues: Put.io takes one address, and its only way to
/// accept container bytes is a second host and a session a restart could lose.
fn submit_address(source: &JobSource) -> Result<String, Failure> {
    let address = match source {
        JobSource::Magnet(magnet) => {
            source::magnet_info_hash(magnet).map(|_| (*magnet).to_string())
        }
        JobSource::Container(bytes) => source::container_magnet(bytes),
        // Put.io's `transfers/add` does take an ordinary web address, and the third shape of
        // source (RD-120-20) crosses the contract intact -- but a plain address has no info
        // hash, so there is no key that would keep a restart from submitting it twice. Until
        // that has an answer this plugin does not claim one, rather than claiming a download it
        // cannot make idempotent.
        JobSource::Address(_) => None,
    };
    address.ok_or_else(|| refuse(messages::NOT_A_TORRENT, FailureKind::Unsupported))
}

/// The content key of a source, or the refusal that says it is not one of ours.
fn key_of(source: &JobSource) -> Result<String, Failure> {
    let key = match source {
        JobSource::Magnet(magnet) => source::magnet_info_hash(magnet),
        JobSource::Container(bytes) => source::container_info_hash(bytes),
        JobSource::Address(_) => None,
    };
    key.ok_or_else(|| refuse(messages::NOT_A_TORRENT, FailureKind::Unsupported))
}

/// Checks an identifier before it is spliced into a request path.
fn safe_id(handle: &RemoteHandle) -> Result<&str, Failure> {
    if api::is_safe_remote_id(&handle.remote_id) {
        Ok(&handle.remote_id)
    } else {
        Err(refuse(messages::TRANSFER_GONE, FailureKind::Permanent))
    }
}

fn handle_for(account_id: &str, remote_id: String) -> RemoteHandle {
    RemoteHandle {
        remote_id,
        account_id: account_id.to_owned(),
        // Nothing to carry: the identifier is the whole handle, and a plugin that put something
        // here would be storing state the host would have to keep for no reason.
        job_state: None,
    }
}

impl Guest for Component {
    /// None. No cache query is bound for put.io (RD-130-11 left every provider but TorBox and
    /// Premiumize out); one that gets it is a job of its own. So the host never calls
    /// `check_cached` here.
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
            JobSource::Magnet(magnet) => source::magnet_info_hash(magnet).is_some(),
            JobSource::Container(bytes) => source::container_info_hash(bytes).is_some(),
            JobSource::Address(_) => false,
        }
    }

    /// The BitTorrent info hash, lower-case hex, derived locally and identically for a magnet
    /// and for the `.torrent` file of the same content.
    ///
    /// It is also the number `adopt` looks for in what Put.io says about a transfer, which is
    /// what lets an installation recognise a transfer it may have created seconds before a
    /// crash.
    fn identify(source: JobSource) -> Result<String, Failure> {
        key_of(&source)
    }

    /// Hands the source over. One request, no retry, no loop.
    ///
    /// Not idempotent, and deliberately not made to look idempotent here: a retry hidden in this
    /// function would be the duplicate the host's row exists to prevent, moved somewhere nobody
    /// would think to look for it.
    fn submit(request: SubmitRequest) -> Result<RemoteHandle, Failure> {
        let magnet = submit_address(&request.source)?;
        let answer: api::TransferResponse = parse(&call(
            "POST",
            &format!("{}/transfers/add", address::API),
            &[],
            Some("application/x-www-form-urlencoded"),
            &source::add_body(&magnet),
        )?)?;
        let id = answer
            .transfer
            .and_then(|transfer| transfer.id)
            .filter(|id| *id > 0)
            .map(|id| id.to_string());
        let Some(id) = id else {
            // Something may well have been created and this installation cannot name it. The
            // host is told plainly rather than being handed an empty handle it would poll for
            // ever; its adoption check is what looks for the orphan.
            return Err(refuse(messages::NO_TRANSFER_ID, FailureKind::Permanent));
        };
        Ok(handle_for(&request.account_id, id))
    }

    /// The transfer the account already holds for this info hash, if any.
    ///
    /// One `GET /v2/transfers/list`, and no walk beyond it: this exists for a submit that was
    /// lost seconds ago, which is the newest entry there is.
    fn adopt(account_id: String, content_key: String) -> Result<Option<RemoteHandle>, Failure> {
        let body = call(
            "GET",
            &format!("{}/transfers/list", address::API),
            &[],
            None,
            &[],
        )?;
        let listed: api::TransferListResponse = parse(&body)?;
        let found = listed
            .transfers
            .into_iter()
            .take(api::ADOPT_LIMIT)
            .find(|transfer| transfer.carries(&content_key))
            .and_then(|transfer| transfer.id)
            .filter(|id| *id > 0)
            .map(|id| id.to_string());
        Ok(found.map(|id| handle_for(&account_id, id)))
    }

    fn poll(handle: RemoteHandle) -> Result<RemoteProgress, Failure> {
        let id = safe_id(&handle)?;
        let body = call(
            "GET",
            &format!("{}/transfers/{id}", address::API),
            &[],
            None,
            &[],
        )?;
        let answer: api::TransferResponse = parse(&body)?;
        let transfer = answer.transfer.unwrap_or_default();
        let status = transfer.status.clone().unwrap_or_default();
        Ok(match api::stage_of(&status) {
            Stage::Preparing(seconds) => RemoteProgress::Preparing(Some(seconds)),
            // `working` carries no suggested wait: a running transfer is polled on the host's
            // own interval, and the one this plugin would name travels on `preparing`, where
            // there is a field for it.
            Stage::Working => RemoteProgress::Working(RemoteWork {
                progress_permille: api::permille(transfer.percent_done),
                speed_bytes_per_second: transfer.down_speed,
                seconds_remaining: transfer.estimated_time,
            }),
            Stage::Ready => ready_of(&transfer)?,
            Stage::Failed(message) => {
                RemoteProgress::Failed(refuse(message, FailureKind::Permanent))
            }
        })
    }

    /// Never reached. `poll` never answers `awaiting-choice`, because Put.io fetches a torrent
    /// whole and has no way to be told to leave a file out; the choice happens in the
    /// LinkGrabber, over the tree `ready` handed it. Being asked anyway is a stable refusal
    /// rather than a silent success that changed nothing.
    fn choose(_handle: RemoteHandle, _chosen: Vec<u32>) -> Result<(), Failure> {
        Err(refuse(
            messages::NO_REMOTE_SELECTION,
            FailureKind::Unsupported,
        ))
    }

    /// Removes the transfer at Put.io. Nothing in this plugin calls it.
    ///
    /// `transfers/cancel` removes the transfer, and deliberately nothing else: the files a
    /// finished transfer put in the account stay where they are. Discarding a job rDownloader
    /// created is one thing; deleting somebody's files is another, and nothing here does the
    /// second on its way to the first.
    fn discard(handle: RemoteHandle) -> Result<(), Failure> {
        let id = safe_id(&handle)?;
        call(
            "POST",
            &format!("{}/transfers/cancel", address::API),
            &[],
            Some("application/x-www-form-urlencoded"),
            &api::cancel_body(id),
        )?;
        Ok(())
    }
}

/// What a finished transfer produced, as addresses the LinkGrabber can take.
///
/// Put.io's transfer record names one `file_id` — a folder for a multi-file torrent, the file
/// itself for a single-file one — so this walks from there. Every artifact carries the stable
/// per-file address, its name, its size and the folders it sat in; none of them carries a
/// signed short-lived URL, which is the whole argument in `putio_common::address`.
fn ready_of(transfer: &api::Transfer) -> Result<RemoteProgress, Failure> {
    let Some(root_id) = transfer.file_id.filter(|id| *id > 0) else {
        // Finished with nothing named is not a result.
        return Err(refuse(messages::NO_CONTENT, FailureKind::Permanent));
    };
    let mut requests = 0_usize;
    let root = fetch_file(root_id, &mut requests)?;
    let root_name = root.name.clone().unwrap_or_default();
    let mut artifacts = Vec::new();
    if root.is_folder() {
        walk(
            root_id,
            &mut vec![root_name],
            &mut artifacts,
            &mut requests,
            0,
        )?;
    } else {
        artifacts.push(artifact_of(&root, root_id, &[]));
    }
    if artifacts.is_empty() {
        // A package with no files in it and nothing to explain why is the shape of the defect
        // ADR 0001 was opened for.
        return Err(refuse(messages::NO_CONTENT, FailureKind::Permanent));
    }
    Ok(RemoteProgress::Ready(artifacts))
}

/// One `GET /v2/files/{id}`, counted against the walk's budget.
fn fetch_file(file_id: u64, requests: &mut usize) -> Result<api::FileRecord, Failure> {
    spend(requests)?;
    let body = call("GET", &address::file_url(file_id), &[], None, &[])?;
    let answer: api::FileResponse = parse(&body)?;
    answer
        .file
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// Walks one folder, depth first, adding every file it holds.
///
/// `folders` is the path from the transfer's root to the folder being walked, which becomes
/// each artifact's `package-hint`. It is a stack rather than a joined string so a sibling
/// folder cannot inherit the last one's name.
fn walk(
    parent_id: u64,
    folders: &mut Vec<String>,
    artifacts: &mut Vec<RemoteArtifact>,
    requests: &mut usize,
    depth: u32,
) -> Result<(), Failure> {
    if depth > api::MAX_DEPTH {
        return Err(refuse(messages::TOO_MANY_FILES, FailureKind::Unsupported));
    }
    spend(requests)?;
    let body = call(
        "GET",
        &format!("{}/files/list", address::API),
        &[
            RequestQuery {
                name: "parent_id".to_owned(),
                value_template: parent_id.to_string(),
            },
            RequestQuery {
                name: "per_page".to_owned(),
                value_template: api::LIST_PAGE.to_string(),
            },
        ],
        None,
        &[],
    )?;
    let listing: api::FileListResponse = parse(&body)?;
    for entry in listing.files {
        let Some(id) = entry.id.filter(|id| *id > 0) else {
            continue;
        };
        if entry.is_folder() {
            folders.push(entry.name.clone().unwrap_or_default());
            let walked = walk(id, folders, artifacts, requests, depth + 1);
            folders.pop();
            walked?;
        } else {
            if artifacts.len() >= api::MAX_FILES {
                return Err(refuse(messages::TOO_MANY_FILES, FailureKind::Unsupported));
            }
            artifacts.push(artifact_of(&entry, id, folders));
        }
    }
    Ok(())
}

fn artifact_of(file: &api::FileRecord, file_id: u64, folders: &[String]) -> RemoteArtifact {
    RemoteArtifact {
        url: address::download_url(file_id),
        file_name: file.name.clone().filter(|name| !name.is_empty()),
        size: file.size,
        package_hint: api::package_hint(folders),
    }
}

/// Spends one of the walk's requests, or refuses the whole job.
fn spend(requests: &mut usize) -> Result<(), Failure> {
    *requests += 1;
    if *requests > api::MAX_TREE_REQUESTS {
        return Err(refuse(messages::TOO_MANY_FILES, FailureKind::Unsupported));
    }
    Ok(())
}

export!(Component);
