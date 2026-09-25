//! The component: a magnet or an address in, a cloud download at Offcloud out.
//!
//! Nine short calls and no loop. Every function here makes at most two requests and returns
//! on the provider's next answer; nothing waits for a download, because the waiting is the
//! host's — it holds the row, the clock and the restart. That split is argued in
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`, and it is what lets this plugin keep a
//! fuel and timeout budget a crawler's could not have kept.
//!
//! Two of the seven are answered without reaching Offcloud at all, and that is the whole
//! restart story: `identify` derives the content key locally, so the host can write the row —
//! and refuse a duplicate — before anything is handed over, and `adopt` is what finds a job
//! whose submit answer never arrived.
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

/// The bearer header, as a template. The key's value never reaches this plugin: the host
/// substitutes it on the way out, towards `offcloud.com` and nowhere else.
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

/// One request, with every refusal — in either of Offcloud's two shapes — turned into one
/// failure.
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
    // A refusal decides whatever the status says, and a status decides when there is no
    // document to read. Both directions matter: Offcloud answers refusals with a 200.
    let envelope = api::error_envelope(&response.body);
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(from_api(failure));
    }
    Ok(response.body)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Failure> {
    serde_json::from_slice(body)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

fn form(method: &str, path: &str, pairs: &[(&str, &str)]) -> Result<Vec<u8>, Failure> {
    call(
        method,
        &format!("{}{path}", api::API_BASE),
        &[],
        Some("application/x-www-form-urlencoded"),
        &api::form_body(pairs),
    )
}

/// The content key of a source, or the refusal that says it is not one of ours.
fn key_of(source: &JobSource) -> Result<String, Failure> {
    match source {
        JobSource::Magnet(address) => source::magnet_key(address)
            .ok_or_else(|| refuse(messages::NOT_A_CLOUD_SOURCE, FailureKind::Unsupported)),
        JobSource::Address(address) => source::address_key(address)
            .ok_or_else(|| refuse(messages::NOT_A_CLOUD_SOURCE, FailureKind::Unsupported)),
        // Offcloud's cloud takes exactly one field and it is an address, so there is nowhere
        // for a container's bytes to go. Answered here rather than ignored, because an
        // unmatched arm would be a plugin that silently claimed a `.torrent` it cannot submit.
        JobSource::Container(_) => Err(refuse(
            messages::CONTAINER_UNSUPPORTED,
            FailureKind::Unsupported,
        )),
    }
}

/// The string that goes into `url`, for the two shapes that have one.
fn submitted_address(source: &JobSource) -> Result<&str, Failure> {
    match source {
        JobSource::Magnet(address) | JobSource::Address(address) => Ok(address.as_str()),
        JobSource::Container(_) => Err(refuse(
            messages::CONTAINER_UNSUPPORTED,
            FailureKind::Unsupported,
        )),
    }
}

/// Checks an identifier before it is spliced into a request path or a JSON body.
fn safe_id(handle: &RemoteHandle) -> Result<&str, Failure> {
    if api::is_safe_request_id(&handle.remote_id) {
        Ok(&handle.remote_id)
    } else {
        Err(refuse(messages::JOB_GONE, FailureKind::Permanent))
    }
}

fn handle_for(account_id: &str, remote_id: String) -> RemoteHandle {
    RemoteHandle {
        remote_id,
        account_id: account_id.to_owned(),
        // Nothing to carry: the request identifier is the whole handle, and a plugin that put
        // something here would be storing state the host would have to keep for no reason.
        job_state: None,
    }
}

impl Guest for Component {
    /// None. No cache query is bound for Offcloud (RD-130-11 left every provider but TorBox
    /// and Premiumize out); one that gets it is a job of its own. So the host never calls
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
            JobSource::Magnet(address) => source::magnet_key(address).is_some(),
            JobSource::Address(address) => source::address_key(address).is_some(),
            JobSource::Container(_) => false,
        }
    }

    /// The content key, derived locally and identically every time for the same source.
    ///
    /// Two key spaces with two prefixes — `btih:` for a magnet, `url:` for an address — because
    /// `remote_jobs(account_id, content_key)` is one unique index and a bare digest from either
    /// derivation would be forty characters in it.
    fn identify(source: JobSource) -> Result<String, Failure> {
        key_of(&source)
    }

    /// Hands the source over. One request, no retry, no loop.
    ///
    /// Not idempotent, and deliberately not made to look idempotent here: a retry hidden in
    /// this function would be the duplicate the host's row exists to prevent, moved somewhere
    /// nobody would think to look for it.
    fn submit(request: SubmitRequest) -> Result<RemoteHandle, Failure> {
        let address = submitted_address(&request.source)?;
        let body = form("POST", api::SUBMIT_PATH, &[("url", address)])?;
        let answer: api::CreatedRequest = parse(&body)?;
        let Some(id) = answer.request_id.filter(|id| api::is_safe_request_id(id)) else {
            // Something may well have been created and this installation cannot name it. The
            // host is told plainly rather than being handed an empty handle it would poll for
            // ever; its adoption check is what looks for the orphan.
            return Err(refuse(messages::NO_REQUEST_ID, FailureKind::Permanent));
        };
        Ok(handle_for(&request.account_id, id))
    }

    /// The cloud download the account already holds for this content key, if any.
    ///
    /// The crash window, closed at the provider. Offcloud's history records what each request
    /// was started from, so the comparison is made by putting that string through the very
    /// derivation `identify` used — never by comparing the addresses themselves, which would
    /// miss the same magnet spelled in base32.
    fn adopt(account_id: String, content_key: String) -> Result<Option<RemoteHandle>, Failure> {
        let body = call(
            "GET",
            &format!("{}{}", api::API_BASE, api::HISTORY_PATH),
            &[],
            None,
            &[],
        )?;
        let history: Vec<api::HistoryEntry> = parse(&body)?;
        let found = history
            .into_iter()
            .take(api::ADOPT_PAGE)
            .find(|entry| {
                entry
                    .original_link
                    .as_deref()
                    .and_then(source::key_of_original_link)
                    .is_some_and(|key| key == content_key)
            })
            .and_then(|entry| entry.request_id)
            .filter(|id| api::is_safe_request_id(id));
        Ok(found.map(|id| handle_for(&account_id, id)))
    }

    fn poll(handle: RemoteHandle) -> Result<RemoteProgress, Failure> {
        let id = safe_id(&handle)?;
        let body = form("POST", api::STATUS_PATH, &[("requestId", id)])?;
        let envelope: api::StatusEnvelope = parse(&body)?;
        let status = api::status_word(&envelope).unwrap_or_default().to_owned();
        let detail = api::status_detail(&envelope);
        Ok(match api::stage_of(&status) {
            Stage::Preparing(seconds) => RemoteProgress::Preparing(Some(seconds)),
            // `working` carries no suggested wait: a running download is polled on the host's
            // own interval, and the one this plugin would name travels on `preparing`, where
            // there is a field for it.
            Stage::Working(_) => RemoteProgress::Working(RemoteWork {
                progress_permille: detail
                    .and_then(|detail| api::permille(detail.amount, detail.file_size)),
                speed_bytes_per_second: None,
                seconds_remaining: None,
            }),
            Stage::Ready => ready_of(id, detail)?,
            Stage::Failed(message) => {
                RemoteProgress::Failed(refuse(message, FailureKind::Permanent))
            }
        })
    }

    /// Never reached: `poll` has no `awaiting-choice` to answer, because Offcloud fetches the
    /// whole of what it was given. Refusing under a stable code is what an interface call
    /// nobody should make ought to do; a silent `Ok` would let a selection be made and quietly
    /// ignored.
    fn choose(_handle: RemoteHandle, _chosen: Vec<u32>) -> Result<(), Failure> {
        Err(refuse(messages::NO_SELECTION, FailureKind::Unsupported))
    }

    /// Removes the request at Offcloud. Nothing in this plugin calls it: the host reaches it
    /// from one explicit, confirmed request and from no other path.
    fn discard(handle: RemoteHandle) -> Result<(), Failure> {
        let id = safe_id(&handle)?;
        call(
            "POST",
            &format!("{}{}", api::API_BASE, api::REMOVE_PATH),
            &[],
            // The one call here that sends JSON: its parameter is a list, and a form body has
            // no unambiguous spelling for one.
            Some("application/json"),
            &api::remove_body(id),
        )?;
        Ok(())
    }
}

/// The addresses a finished job produced, with the folder each file sat in.
///
/// A second request, and the reason for it: `cloud/status` knows at most one address and knows
/// no paths at all, so a multi-file job read from it alone would arrive as one nameless entry.
/// `cloud/explore` is where the structure is, and the single address `status` carries is the
/// fallback for the job that has exactly one file.
fn ready_of(id: &str, detail: Option<&api::RequestStatus>) -> Result<RemoteProgress, Failure> {
    let job_name = detail
        .and_then(|detail| detail.file_name.clone())
        .unwrap_or_default();
    // An exploration that could not be *read* is not a failure of the job: a job with one file
    // has nothing to explore, and the address `status` carries is the answer for it. An
    // exploration that could not be *made* is a different thing entirely. A rate limit or an
    // outage on the second of these two calls would otherwise close a finished multi-file job
    // with the single address the first one named -- permanently, and with no sign that the
    // other files ever existed. So a refusal worth waiting out is carried out of here and the
    // host polls again; only a refusal that waiting cannot repair falls through to the address.
    let files = match call(
        "GET",
        &format!("{}{}/{id}", api::API_BASE, api::EXPLORE_PATH),
        &[],
        None,
        &[],
    ) {
        Ok(body) => api::explore_files(&body),
        Err(failure) if worth_waiting(&failure) => return Err(failure),
        Err(_) => Vec::new(),
    };
    let mut artifacts: Vec<RemoteArtifact> = files
        .into_iter()
        .take(api::MAX_ENTRIES)
        .filter_map(|file| artifact_of(&job_name, &file))
        .collect();
    if artifacts.is_empty()
        && let Some(url) = detail.and_then(|detail| detail.url.clone())
    {
        artifacts.push(single_artifact(&job_name, url));
    }
    if artifacts.is_empty() {
        // Finished with nothing is not a result. A package with no files in it and nothing to
        // explain why is the shape of the defect ADR 0001 was opened for.
        return Err(refuse(messages::NO_LINKS, FailureKind::Permanent));
    }
    Ok(RemoteProgress::Ready(artifacts))
}

/// Whether asking again could plausibly change this answer.
///
/// The same three categories the host treats as a wait, asked here because this is the one
/// place a refusal would otherwise be swallowed.
fn worth_waiting(failure: &Failure) -> bool {
    matches!(
        failure.category,
        FailureKind::Transient(_) | FailureKind::RateLimited(_) | FailureKind::IpBlocked(_)
    )
}

fn artifact_of(job_name: &str, file: &api::ExploreFile) -> Option<RemoteArtifact> {
    let url = file.url.clone().filter(|url| !url.trim().is_empty())?;
    let path = file
        .path
        .clone()
        .or_else(|| file.name.clone())
        .or_else(|| api::name_from_url(&url))
        .unwrap_or_default();
    let (file_name, package_hint) = api::place(job_name, &path);
    Some(RemoteArtifact {
        url,
        file_name,
        size: file.size,
        package_hint,
    })
}

/// The one-file job: its name is the job's own, and it sits in no folder inside it.
fn single_artifact(job_name: &str, url: String) -> RemoteArtifact {
    let file_name = (!job_name.trim().is_empty())
        .then(|| job_name.trim().to_owned())
        .or_else(|| api::name_from_url(&url));
    RemoteArtifact {
        url,
        file_name,
        size: None,
        // Deliberately none. A single file does not need a folder of its own, and inventing
        // one would put every one-file Offcloud job in a package by itself.
        package_hint: None,
    }
}

export!(Component);
