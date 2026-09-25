//! The component: a magnet or a `.torrent` in, a transfer at Seedr out.
//!
//! Nine short calls and no loop. Nothing here waits for a download, because the waiting is the
//! host's — it holds the row, the clock and the restart. That split is argued in
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`, and it is what lets this plugin keep a
//! fuel and timeout budget a crawler waiting for a torrent could never have kept.
//!
//! Two of the seven are answered without reaching Seedr at all, and that is the whole restart
//! story: `identify` derives the content key locally, so the host can write the row — and
//! refuse a duplicate — before anything is handed over, and `adopt` is what finds a transfer
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
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};
use seedr_common::{address, folder::Listing, reason::ErrorEnvelope, torrent};

use crate::{
    api::{self, ApiFailure, ErrorKind, Stage},
    messages,
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

/// The credential header, as a template. Neither half of it ever reaches this plugin: the host
/// pairs the account's e-mail address with its password, encodes the two and sends the result
/// towards `www.seedr.cc` and nowhere else.
fn headers(content_type: Option<&str>) -> Vec<RequestHeader> {
    let mut headers = vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: address::AUTHORIZATION_TEMPLATE.to_owned(),
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

/// One request, with every refusal — in either of Seedr's two shapes — turned into one failure.
///
/// The vocabulary stays small on purpose: a caller gets bytes or a failure and never decides a
/// second time what a status code means.
fn call(
    method: &str,
    url: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Vec<u8>, Failure> {
    let response = http::http_request(method, url, &[], &headers(content_type), body)?;
    let retry_after = api::retry_after_seconds(
        response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
            .map(|(_, value)| value.as_str()),
    );
    // A refusal decides whatever the status says, and a status decides when there is no
    // document to read. Both directions matter: Seedr answers refusals with a 200.
    let envelope = ErrorEnvelope::of(&response.body);
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(from_api(failure));
    }
    Ok(response.body)
}

/// One folder listing, or the refusal that says the answer was not one.
fn listing(folder_id: Option<u64>) -> Result<Listing, Failure> {
    let body = call("GET", &address::folder_url(folder_id), None, &[])?;
    Listing::of(&body).ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// The info hash of a source, or `None` when it is not one of ours.
///
/// A magnet and the matching `.torrent` answer with the same twenty bytes, which is what makes
/// them one remote job rather than two.
fn info_hash_of(source: &JobSource) -> Option<String> {
    match source {
        JobSource::Magnet(magnet) => torrent::magnet_info_hash(magnet),
        JobSource::Container(bytes) => torrent::container_info_hash(bytes),
        // Deliberately not claimed. Seedr's `POST /rest/transfer/url` sits in the Transfers
        // section beside the magnet and the torrent file, so it is a torrent source rather than
        // a general downloader; claiming every http address for it would take them away from
        // the hosters that can actually fetch them, on a guess about what Seedr would accept.
        JobSource::Address(_) => None,
    }
}

/// The magnet that goes into the form body, for whichever shape arrived.
fn submitted_magnet(source: &JobSource) -> Result<String, Failure> {
    match source {
        JobSource::Magnet(magnet) => torrent::magnet_info_hash(magnet)
            .map(|_| magnet.clone())
            .ok_or_else(|| refuse(messages::NOT_A_TORRENT, FailureKind::Unsupported)),
        JobSource::Container(bytes) => torrent::container_magnet(bytes)
            .ok_or_else(|| refuse(messages::NOT_A_TORRENT, FailureKind::Unsupported)),
        JobSource::Address(_) => Err(refuse(
            messages::ADDRESS_UNSUPPORTED,
            FailureKind::Unsupported,
        )),
    }
}

/// The transfer identifier on a handle, checked before it is spliced into a request path.
fn transfer_id(handle: &RemoteHandle) -> Result<u64, Failure> {
    address::parse_id(&handle.remote_id)
        .ok_or_else(|| refuse(messages::TRANSFER_GONE, FailureKind::Permanent))
}

/// The transfer's own name, as the handle remembers it.
///
/// `job-state` exists for exactly this: a guest remembers nothing between calls, and the host
/// stores what it is handed and gives it back. The name is what a finished transfer's folder is
/// found by, and it is written at submit and adopt because those are the only two calls whose
/// answer the host records.
fn job_name(handle: &RemoteHandle) -> String {
    handle.job_state.clone().unwrap_or_default()
}

fn handle_for(account_id: &str, transfer_id: u64, name: String) -> RemoteHandle {
    RemoteHandle {
        remote_id: transfer_id.to_string(),
        account_id: account_id.to_owned(),
        job_state: (!name.is_empty()).then_some(name),
    }
}

impl Guest for Component {
    /// None. No cache query is bound for Seedr (RD-130-11 left every provider but TorBox and
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
        info_hash_of(&source).is_some()
    }

    /// The content key, derived locally and identically every time for the same source.
    ///
    /// One key space, prefixed all the same: `remote_jobs(account_id, content_key)` is a single
    /// index shared with every other provider's plugin, and a bare forty-character digest in it
    /// says nothing about what it is a digest of.
    fn identify(source: JobSource) -> Result<String, Failure> {
        info_hash_of(&source)
            .map(|hash| format!("btih:{hash}"))
            .ok_or_else(|| refuse(messages::NOT_A_TORRENT, FailureKind::Unsupported))
    }

    /// Hands the source over. One request, no retry, no loop.
    ///
    /// Not idempotent, and deliberately not made to look idempotent here: a retry hidden in
    /// this function would be the duplicate the host's row exists to prevent, moved somewhere
    /// nobody would think to look for it.
    fn submit(request: SubmitRequest) -> Result<RemoteHandle, Failure> {
        let magnet = submitted_magnet(&request.source)?;
        let body = call(
            "POST",
            &address::add_magnet_url(),
            Some("application/x-www-form-urlencoded"),
            &torrent::add_magnet_body(&magnet),
        )?;
        let Some(id) = api::created_transfer_id(&body) else {
            // Something may well have been created and this installation cannot name it. The
            // host is told plainly rather than being handed an empty handle it would poll for
            // ever; its adoption check is what looks for the orphan.
            return Err(refuse(messages::NO_TRANSFER_ID, FailureKind::Permanent));
        };
        // Seedr's own title if it gave one, the magnet's display name if it did not. A
        // transfer with no name at all could never be matched to the folder it becomes, so
        // this is worth the fallback.
        let name = api::created_title(&body)
            .or_else(|| api::magnet_display_name(&magnet))
            .unwrap_or_default();
        Ok(handle_for(&request.account_id, id, name))
    }

    /// The transfer the account already holds for this content key, if any.
    ///
    /// The crash window, closed at the provider. Seedr's root listing names the info hash of
    /// every running transfer, so the comparison is made on the twenty bytes rather than on a
    /// name — a name would eventually adopt somebody else's transfer as this job.
    ///
    /// **A transfer that finished inside the crash window is not adopted**, and that is stated
    /// rather than hidden: once it is a folder it carries no info hash any more, so there is
    /// nothing left to compare. The row then falls through to the host's two-attempt ceiling.
    /// Matching the folder by name instead would be a guess with somebody's account on the
    /// other end of it.
    fn adopt(account_id: String, content_key: String) -> Result<Option<RemoteHandle>, Failure> {
        let listing = listing(None)?;
        let wanted = content_key.strip_prefix("btih:").unwrap_or(&content_key);
        let found = listing.torrents.iter().find(|entry| {
            entry
                .hash
                .as_deref()
                .and_then(torrent::info_hash_within)
                .is_some_and(|hash| hash == wanted)
        });
        Ok(found.and_then(|entry| {
            entry.id.map(|id| {
                handle_for(
                    &account_id,
                    id,
                    entry
                        .name
                        .as_deref()
                        .map(api::bounded_name)
                        .unwrap_or_default(),
                )
            })
        }))
    }

    /// Where the transfer stands, read out of the account's root listing in one request.
    fn poll(handle: RemoteHandle) -> Result<RemoteProgress, Failure> {
        let id = transfer_id(&handle)?;
        let name = job_name(&handle);
        let root = listing(None)?;
        Ok(match api::stage_of(&root, id, &name) {
            Stage::Preparing(seconds) => RemoteProgress::Preparing(Some(seconds)),
            // `working` carries no suggested wait: a running transfer is polled on the host's
            // own interval, and the one this plugin would name travels on `preparing`, where
            // there is a field for it.
            Stage::Working(progress_permille) => RemoteProgress::Working(RemoteWork {
                progress_permille,
                // Seedr's listing states neither, and a figure invented out of two polls would
                // be a speed nobody measured.
                speed_bytes_per_second: None,
                seconds_remaining: None,
            }),
            Stage::Ready(folder_id) => ready_of(folder_id, &name)?,
            Stage::Failed(message) => {
                RemoteProgress::Failed(refuse(message, FailureKind::Permanent))
            }
        })
    }

    /// Never reached: `poll` has no `awaiting-choice` to answer, because Seedr fetches the
    /// whole torrent and offers no call that means "these files and not the others".
    ///
    /// Refusing under a stable code is what an interface call nobody should make ought to do; a
    /// silent `Ok` would let a selection be made and quietly ignored. The alternative
    /// considered and rejected is the one Put.io rejected for the same reason: expressing a
    /// selection by deleting the unwanted files at Seedr, which is exactly the implicit remote
    /// deletion ADR 0003 forbids. Note that the contract could not have expressed it either —
    /// `choose` returns no handle, and nothing the host stores about the answer reaches the
    /// guest again, so a guest cannot tell a question it has already had answered from one
    /// nobody has answered yet (RD-120-35).
    fn choose(_handle: RemoteHandle, _chosen: Vec<u32>) -> Result<(), Failure> {
        Err(refuse(messages::NO_SELECTION, FailureKind::Unsupported))
    }

    /// Removes the transfer at Seedr. Nothing in this plugin calls it: the host reaches it from
    /// one explicit, confirmed request and from no other path.
    ///
    /// **A finished transfer's folder is not removed**, and that is deliberate. By the time a
    /// transfer finishes it is a folder full of the person's files, and the confirmed request
    /// said to discard the *job*. Deleting the files as a side effect of that is precisely the
    /// implicit remote deletion ADR 0003 forbids. Seedr answering 404 here — the transfer is
    /// already gone — is therefore success and not a failure: there is nothing left to remove.
    fn discard(handle: RemoteHandle) -> Result<(), Failure> {
        let id = transfer_id(&handle)?;
        match call("DELETE", &address::transfer_url(id), None, &[]) {
            Ok(_) => Ok(()),
            Err(failure) if failure.code.as_deref() == Some(messages::TRANSFER_GONE.0) => Ok(()),
            Err(failure) => Err(failure),
        }
    }
}

/// The addresses a finished transfer produced, with the folder each file sat in.
///
/// Seedr's listing is a tree and the contract takes a flat list, so the tree is walked to its
/// leaves — bounded three ways, because a folder listing comes from a provider and each level
/// of it is another request against the account's shared rate limit. The bounds stop the walk;
/// they do not fail the job, because a transfer whose first two thousand files arrived is a
/// transfer somebody can use.
fn ready_of(folder_id: u64, job_name: &str) -> Result<RemoteProgress, Failure> {
    let mut artifacts: Vec<RemoteArtifact> = Vec::new();
    // Every folder's own files are emitted before any of its subfolders is walked, so the
    // files a person actually wanted arrive first and a bound that stops the walk stops it
    // somewhere useful rather than half-way down one branch.
    let mut queue: Vec<(u64, Vec<String>, u32)> = vec![(folder_id, Vec::new(), 0)];
    let mut listings = 0_usize;
    while let Some((id, relative, depth)) = queue.pop() {
        if listings >= api::MAX_LISTINGS || artifacts.len() >= api::MAX_ENTRIES {
            break;
        }
        listings += 1;
        let folder = listing(Some(id))?;
        let package_hint = api::place(job_name, &relative);
        for file in &folder.files {
            if artifacts.len() >= api::MAX_ENTRIES {
                break;
            }
            let Some(file_id) = file.id else { continue };
            artifacts.push(RemoteArtifact {
                url: address::file_url(file_id),
                file_name: file
                    .name
                    .as_deref()
                    .map(api::bounded_name)
                    .filter(|name| !name.is_empty()),
                size: file.size,
                package_hint: package_hint.clone(),
            });
        }
        if depth >= api::MAX_DEPTH {
            continue;
        }
        for child in &folder.folders {
            let Some(child_id) = child.id else { continue };
            // A child that names itself as its own parent is a listing answering with itself,
            // which would otherwise be a walk that only the request bound above ends.
            if child_id == id {
                continue;
            }
            let mut below = relative.clone();
            below.push(
                child
                    .name
                    .as_deref()
                    .map(api::bounded_name)
                    .unwrap_or_default(),
            );
            queue.push((child_id, below, depth + 1));
        }
    }
    if artifacts.is_empty() {
        // Finished with nothing is not a result. A package with no files in it and nothing to
        // explain why is the shape of the defect ADR 0001 was opened for.
        return Err(refuse(messages::NO_FILES, FailureKind::Permanent));
    }
    Ok(RemoteProgress::Ready(artifacts))
}

export!(Component);
