//! The component: a magnet, a container or a plain address in, a transfer at Premiumize out.
//!
//! Nine short calls and no loop. Every function here returns on the provider's next answer;
//! nothing waits for a transfer, because the waiting is the host's -- it holds the row, the
//! clock and the restart. That split is argued in
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`.
//!
//! **This plugin never answers `awaiting-choice`, and the reason is worth stating.** The
//! `transfer/*` API has no selection: there is no call that tells Premiumize which files of a
//! transfer to keep, and by the time `file_id` and `folder_id` exist the fetching is already
//! done. A question could still be asked -- but not answered. `choose` returns
//! `result<_, failure>` and `job-state` is written by the host only when `submit` or `adopt`
//! names the job, so a guest cannot remember that a question was answered; the next `poll`
//! would ask it again, and the job would sit between `awaiting-choice` and `working` for
//! ever. So every file a finished transfer produced is handed over, as one package rooted at
//! the transfer's name, and the person takes what they want there -- the LinkGrabber is this
//! application's selection surface, and `awaiting-choice` exists for providers that have to
//! be told *before* they fetch. `docs/roadmap/jobs/120-23-*.md` records the contract change
//! this would need instead.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "remote-job-plugin",
});

use exports::rdownloader::plugin::remote_job::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, Guest, JobSource, RemoteArtifact, RemoteHandle,
    RemoteProgress, RemoteWork, SubmitRequest,
};
use premiumize_common::{
    cache::{self, CacheCheckResponse, Holding},
    container,
    listing::{self, Entry},
    status::{self, Kind},
};
use rdownloader::plugin::{
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{api, messages, source};

/// How many `folder/list` calls one poll may make before it refuses to walk further.
const MAX_FOLDER_REQUESTS: usize = 16;

/// How deep inside a transfer's folder one poll may go.
const MAX_DEPTH: usize = 4;

/// Most addresses one finished transfer may hand back. The host bounds this again; the bound
/// here keeps a runaway answer from being assembled in the first place.
const MAX_ARTIFACTS: usize = 500;

struct Component;

fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// One of [`api::refusal`]'s answers, in the world's own vocabulary.
fn from_refusal(refusal: api::Refusal) -> Failure {
    let mut failure = refuse((refusal.code, refusal.message), category_of(refusal.kind));
    if let Some(code) = refusal.api_code {
        failure.params.push(("api_code".to_owned(), code));
    }
    failure
}

fn category_of(kind: Kind) -> FailureKind {
    match kind {
        Kind::AccountInvalid => FailureKind::AccountInvalid,
        Kind::Unsupported => FailureKind::Unsupported,
        Kind::Offline => FailureKind::Offline,
        Kind::Transient(seconds) => FailureKind::Transient(seconds),
        Kind::RateLimited(seconds) => FailureKind::RateLimited(seconds),
        Kind::Permanent => FailureKind::Permanent,
    }
}

/// The bearer header, as a template. The key's value never reaches this plugin: the host
/// substitutes it on the way out, towards `www.premiumize.me` and nowhere else.
fn headers(content_type: Option<&str>) -> Vec<RequestHeader> {
    let mut headers = vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: format!("Bearer {{{{secret:{}}}}}", api::KEY_REFERENCE),
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

/// One request, with every answer that is not an answer turned into one refusal.
///
/// The body decides, and the status decides only when there is no body to read. Both
/// directions matter, and the first one is the whole reason this is not the usual shape:
/// Premiumize reports `Not logged in` with HTTP `200`, so a reader that believed the status
/// line would take a refusal for a success.
fn call(
    method: &str,
    url: &str,
    query: &[RequestQuery],
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Vec<u8>, Failure> {
    let response = http::http_request(method, url, query, &headers(content_type), body)?;
    let retry_after = status::retry_after_seconds(
        response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
            .map(|(_, value)| value.as_str()),
    );
    let envelope: status::Envelope = serde_json::from_slice(&response.body).unwrap_or_default();
    if envelope.status.is_some() && !envelope.is_success() {
        return Err(from_refusal(api::refusal(
            envelope.code.as_deref(),
            envelope.message.as_deref(),
            retry_after,
        )));
    }
    if let Some(kind) = api::http_refusal(response.status, retry_after) {
        // No envelope to read -- a gateway page, an empty body. Classified by its status
        // alone, and the status travels as a parameter rather than as a sentence.
        let mut failure = refuse(messages::HTTP_ERROR, category_of(kind));
        failure.message = messages::http_error(response.status);
        failure
            .params
            .push(("status".to_owned(), response.status.to_string()));
        return Err(failure);
    }
    Ok(response.body)
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Failure> {
    serde_json::from_slice(body)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// The content key of a source, or the refusal that says it is not one of ours.
fn key_of(source: &JobSource) -> Result<String, Failure> {
    match source {
        JobSource::Magnet(address) => source::magnet_key(address)
            .ok_or_else(|| refuse(messages::NOT_A_SOURCE, FailureKind::Unsupported)),
        JobSource::Address(address) => source::address_key(address)
            .ok_or_else(|| refuse(messages::NOT_A_SOURCE, FailureKind::Unsupported)),
        // A container that cannot be named says so in its own words: the upload needs a file
        // name and `.ccf` carries no marker of its own, so this is not "not a source" but
        // "not a container this build can name".
        JobSource::Container(bytes) => source::container_key(bytes)
            .ok_or_else(|| refuse(messages::CONTAINER_UNKNOWN, FailureKind::Unsupported)),
    }
}

/// Checks an identifier before it is spliced into a request.
fn safe_id(handle: &RemoteHandle) -> Result<&str, Failure> {
    if api::is_safe_id(&handle.remote_id) {
        Ok(&handle.remote_id)
    } else {
        Err(refuse(messages::TRANSFER_GONE, FailureKind::Permanent))
    }
}

fn query(name: &str, value: &str) -> RequestQuery {
    RequestQuery {
        name: name.to_owned(),
        value_template: value.to_owned(),
    }
}

/// The answer for a query that was not asked, or that Premiumize said nothing about.
fn unknown() -> CacheAnswer {
    CacheAnswer {
        state: CacheState::Unknown,
        file_name: None,
        size: None,
    }
}

impl Guest for Component {
    /// Magnets only (RD-130-11). The resolver `plugins/premiumize/` already asks `cache/check`
    /// about every hoster link its account covers during the link check (RD-120-36); a second
    /// request here for the same question would be twice the load for no new answer.
    fn cache_kinds() -> Vec<CacheKind> {
        vec![CacheKind::Torrent]
    }

    /// Whether Premiumize holds each magnet ready: one `cache/check` for all of them.
    ///
    /// Read-only at the provider -- nothing is created, no transfer and no folder. `true` is
    /// `cached`, `false` with a name is `known`, anything else `unknown`; every query that is
    /// not a magnet this plugin would take answers `unknown` without a request. One answer
    /// per query, in order: the host pairs them by position.
    fn check_cached(
        _account_id: String,
        queries: Vec<CacheQuery>,
    ) -> Result<Vec<CacheAnswer>, Failure> {
        let mut answers: Vec<CacheAnswer> = queries.iter().map(|_| unknown()).collect();
        let asked: Vec<(usize, String)> = queries
            .iter()
            .enumerate()
            .filter_map(|(index, query)| match (&query.kind, &query.source) {
                (CacheKind::Torrent, JobSource::Magnet(address))
                    if source::magnet_key(address).is_some() =>
                {
                    Some((index, address.trim().to_owned()))
                }
                _ => None,
            })
            .take(cache::MAX_ITEMS)
            .collect();
        if asked.is_empty() {
            return Ok(answers);
        }
        let items: Vec<String> = asked.iter().map(|(_, address)| address.clone()).collect();
        let response: CacheCheckResponse = parse(&call(
            "POST",
            &format!("{}/cache/check", api::API_BASE),
            &[],
            Some("application/x-www-form-urlencoded"),
            &cache::check_body(&items),
        )?)?;
        for ((index, _), item) in asked.iter().zip(cache::items(items.len(), &response)) {
            let state = match item.holding() {
                Holding::Cached => CacheState::Cached,
                Holding::Known => CacheState::Known,
                Holding::Unknown => continue,
            };
            if let Some(answer) = answers.get_mut(*index) {
                *answer = CacheAnswer {
                    state,
                    file_name: item.file_name,
                    size: item.size,
                };
            }
        }
        Ok(answers)
    }

    /// Reaches nothing. Asked of every source before anything is handed to anybody, and
    /// answered from the bytes alone.
    ///
    /// All three shapes are claimed, which is where this plugin goes past
    /// `plugins/realdebrid-torrents/`: `src` takes a plain address and a container as readily
    /// as a magnet, and refusing either would be refusing what the provider is for.
    fn claims(source: JobSource) -> bool {
        match &source {
            JobSource::Magnet(address) => source::magnet_key(address).is_some(),
            JobSource::Address(address) => source::address_key(address).is_some(),
            JobSource::Container(bytes) => source::container_key(bytes).is_some(),
        }
    }

    /// The key this source is known by, derived locally and identically every time.
    fn identify(source: JobSource) -> Result<String, Failure> {
        key_of(&source)
    }

    /// Hands the source over. One request, no retry, no loop.
    ///
    /// Not idempotent, and deliberately not made to look idempotent here: a retry hidden in
    /// this function would be the duplicate the host's row exists to prevent, moved somewhere
    /// nobody would think to look for it.
    fn submit(request: SubmitRequest) -> Result<RemoteHandle, Failure> {
        let (content_type, body) = match &request.source {
            JobSource::Magnet(address) => (
                "application/x-www-form-urlencoded".to_owned(),
                api::form_body("src", address),
            ),
            JobSource::Address(address) => (
                "application/x-www-form-urlencoded".to_owned(),
                api::form_body("src", address),
            ),
            JobSource::Container(bytes) => {
                let Some(file_name) = container::file_name(bytes) else {
                    return Err(refuse(
                        messages::CONTAINER_UNKNOWN,
                        FailureKind::Unsupported,
                    ));
                };
                let boundary = api::boundary_for(&sanitised(&request.content_key));
                let Some(body) = api::multipart_body(&boundary, file_name, bytes) else {
                    return Err(refuse(messages::CONTAINER_UNKNOWN, FailureKind::Permanent));
                };
                (api::multipart_content_type(&boundary), body)
            }
        };
        let answer: api::CreatedTransfer = parse(&call(
            "POST",
            &format!("{}/transfer/create", api::API_BASE),
            &[],
            Some(&content_type),
            &body,
        )?)?;
        let Some(id) = answer.id.filter(|id| api::is_safe_id(id)) else {
            // Something may well have been created and this installation cannot name it. The
            // host is told plainly rather than being handed a handle it would poll for ever.
            return Err(refuse(messages::NO_TRANSFER_ID, FailureKind::Permanent));
        };
        Ok(RemoteHandle {
            remote_id: id,
            account_id: request.account_id,
            // Nothing to carry: the identifier is the whole handle.
            job_state: None,
        })
    }

    /// Nothing to adopt, and that is an answer rather than an omission.
    ///
    /// `transfer/list` carries `id`, `name`, `status`, `progress`, `message`, `folder_id` and
    /// `file_id` -- and nothing derived from what was handed over. There is no hash, no `src`
    /// and no key, so a transfer this installation created seconds before a crash cannot be
    /// told from one somebody else started; matching on `name` would eventually adopt a
    /// stranger's transfer and poll it as this job, which is worse than the duplicate the
    /// adoption exists to avoid. So `none` is returned, the host falls back to its own
    /// attempt ceiling, and the row keyed by `(account, content_key)` remains the guard that
    /// a restart cannot get past.
    fn adopt(_account_id: String, _content_key: String) -> Result<Option<RemoteHandle>, Failure> {
        Ok(None)
    }

    fn poll(handle: RemoteHandle) -> Result<RemoteProgress, Failure> {
        let id = safe_id(&handle)?.to_owned();
        // There is no per-transfer endpoint: the account's list is the only place a transfer
        // describes itself, so the row is looked up in it.
        let listed: api::TransferList = parse(&call(
            "GET",
            &format!("{}/transfer/list", api::API_BASE),
            &[],
            None,
            &[],
        )?)?;
        let Some(transfer) = listed
            .transfers
            .into_iter()
            .find(|transfer| transfer.id.as_deref() == Some(id.as_str()))
        else {
            return Err(refuse(messages::TRANSFER_UNLISTED, FailureKind::Permanent));
        };
        let state = transfer.status.clone().unwrap_or_default();
        Ok(match api::stage_of(&state) {
            api::Stage::Preparing(seconds) => RemoteProgress::Preparing(Some(seconds)),
            api::Stage::Working => RemoteProgress::Working(RemoteWork {
                progress_permille: api::permille(transfer.progress),
                // `transfer/list` states neither, and a figure this plugin invented would be
                // a number somebody read off a screen.
                speed_bytes_per_second: None,
                seconds_remaining: None,
            }),
            api::Stage::Ready { still_running } => ready_of(&transfer, still_running)?,
            api::Stage::Failed(message) => {
                RemoteProgress::Failed(refuse(message, FailureKind::Permanent))
            }
        })
    }

    /// Never reached: `poll` here does not ask. See this file's header for why, and why the
    /// answer is a refusal rather than a silent success -- accepting a selection that changed
    /// nothing would be reporting work that did not happen.
    fn choose(_handle: RemoteHandle, chosen: Vec<u32>) -> Result<(), Failure> {
        if chosen.is_empty() {
            return Err(refuse(messages::EMPTY_CHOICE, FailureKind::Permanent));
        }
        Err(refuse(messages::NO_CHOICE, FailureKind::Unsupported))
    }

    /// Removes the transfer at Premiumize. Nothing in this plugin calls it.
    ///
    /// `transfer/delete` and never `transfer/clearfinished`: the second takes no argument and
    /// removes every finished transfer in the account, including ones rDownloader never
    /// created. What this application did not put in somebody's account it does not take out.
    fn discard(handle: RemoteHandle) -> Result<(), Failure> {
        let id = safe_id(&handle)?;
        call(
            "POST",
            &format!("{}/transfer/delete", api::API_BASE),
            &[],
            Some("application/x-www-form-urlencoded"),
            &api::form_body("id", id),
        )?;
        Ok(())
    }
}

/// The content key reduced to what may appear in a multipart boundary.
fn sanitised(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(40)
        .collect()
}

/// What a finished transfer produced, as addresses the LinkGrabber can take.
///
/// Two shapes, decided by the measurement: `file_id` names the one file a single-file
/// transfer produced, and is `null` both while a transfer runs and when it holds several
/// files -- so an absent `file_id` with a `folder_id` is the multi-file case and the folder
/// is read.
fn ready_of(transfer: &api::Transfer, still_running: bool) -> Result<RemoteProgress, Failure> {
    let name = transfer.name.clone().unwrap_or_default();
    let artifacts =
        if let Some(file_id) = transfer.file_id.as_deref().filter(|id| api::is_safe_id(id)) {
            single_file(&name, file_id)?
        } else if let Some(folder_id) = transfer
            .folder_id
            .as_deref()
            .filter(|id| api::is_safe_id(id))
        {
            walk(&name, folder_id)?
        } else if still_running {
            // Seeding with neither named yet: the transfer is still going, so there is something
            // to wait for. The same answer for `finished` would be a job that never ends.
            return Ok(RemoteProgress::Preparing(Some(60)));
        } else {
            return Err(refuse(messages::NO_LOCATION, FailureKind::Permanent));
        };
    if artifacts.is_empty() {
        if still_running {
            return Ok(RemoteProgress::Preparing(Some(60)));
        }
        // Finished with nothing is not a result. A package with no files in it and nothing to
        // explain why is the shape of the defect ADR 0001 was opened for.
        return Err(refuse(messages::NO_FILES, FailureKind::Permanent));
    }
    Ok(RemoteProgress::Ready(artifacts))
}

/// The one file `file_id` names.
fn single_file(transfer_name: &str, file_id: &str) -> Result<Vec<RemoteArtifact>, Failure> {
    let body = call(
        "GET",
        &format!("{}/item/details", api::API_BASE),
        &[query("id", file_id)],
        None,
        &[],
    )?;
    let details = listing::item(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    let Some(Entry::File { name, url, size }) = details.item.entry() else {
        return Ok(Vec::new());
    };
    let (file_name, package_hint) = api::place(transfer_name, &[], &name);
    Ok(vec![RemoteArtifact {
        url,
        file_name,
        size,
        package_hint,
    }])
}

/// Walks the folder a multi-file transfer produced, depth first and bounded.
///
/// Bounded in three directions -- requests, depth and files -- and refusing rather than
/// truncating when a bound is reached: a package that silently lost half its files, with
/// nothing to say so, is worse than one that did not arrive.
///
/// A walk inside one call rather than a `preparing` that comes back for the next folder,
/// because a transfer's folder is finished and will not change: reading it in pieces would
/// cost the same requests spread over minutes, and the bounds keep the one call short.
fn walk(transfer_name: &str, folder_id: &str) -> Result<Vec<RemoteArtifact>, Failure> {
    let mut artifacts = Vec::new();
    let mut pending = vec![(folder_id.to_owned(), Vec::<String>::new(), 0_usize)];
    let mut requests = 0_usize;
    while let Some((id, path, depth)) = pending.pop() {
        if requests >= MAX_FOLDER_REQUESTS || depth > MAX_DEPTH {
            return Err(refuse(messages::FOLDER_TOO_LARGE, FailureKind::Permanent));
        }
        requests += 1;
        let body = call(
            "GET",
            &format!("{}/folder/list", api::API_BASE),
            &[query("id", &id)],
            None,
            &[],
        )?;
        let folder = listing::folder(&body)
            .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
        for item in folder.content {
            match item.entry() {
                Some(Entry::File { name, url, size }) => {
                    if artifacts.len() >= MAX_ARTIFACTS {
                        return Err(refuse(messages::FOLDER_TOO_LARGE, FailureKind::Permanent));
                    }
                    let (file_name, package_hint) = api::place(transfer_name, &path, &name);
                    artifacts.push(RemoteArtifact {
                        url,
                        file_name,
                        size,
                        package_hint,
                    });
                }
                Some(Entry::Folder { id, name }) => {
                    if !api::is_safe_id(&id) {
                        continue;
                    }
                    let mut deeper = path.clone();
                    deeper.push(name);
                    pending.push((id, deeper, depth + 1));
                }
                None => {}
            }
        }
    }
    Ok(artifacts)
}

export!(Component);
