//! The component: a magnet, a `.torrent`, an `.nzb` or a link in, a job at TorBox out.
//!
//! Nine short calls and no loop. Every function here makes at most two requests -- the cache
//! check three, one per cache (RD-130-11) -- and returns on TorBox's next answer; nothing
//! waits for a job, because the waiting is the host's -- it holds the row, the clock and the
//! restart. That split is argued in
//! `docs/adr/0003-a-job-that-runs-at-the-provider.md`, and it is what lets this plugin keep a
//! fuel and timeout budget a crawler's could not have kept.
//!
//! One state machine, three sets of paths. The kind travels twice: inside the content key, so
//! that `adopt` knows which list to read from the key alone, and inside the handle's
//! `job-state`, so that `poll` and `discard` know it without deriving anything.
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
    host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{
    api::{self, ApiFailure, ErrorKind, Stage},
    messages,
    source::{self, Handed, Kind},
};

struct Component;

/// How many random bytes the multipart boundary is built from.
const BOUNDARY_BYTES: u32 = 16;

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
            // A duplicate is the provider saying the job already exists. Reported as
            // permanent so the host stops submitting: asking again only creates another one,
            // and the adoption check is what turns this into a handle.
            ErrorKind::Duplicate => FailureKind::Permanent,
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
/// substitutes it on the way out, towards `api.torbox.app` and nowhere else.
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
    path: &str,
    query: &[RequestQuery],
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Vec<u8>, Failure> {
    let url = format!("{}{path}", api::API_BASE);
    let response = http::http_request(method, &url, query, &headers(content_type), body)?;
    let retry_after = api::retry_after_seconds(
        response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
            .map(|(_, value)| value.as_str()),
    );
    // The `error` word decides whatever the status says, and the status decides when there is
    // no document to read. Both directions matter: TorBox answers refusals with 200.
    let envelope: api::ErrorEnvelope = serde_json::from_slice(&response.body).unwrap_or_default();
    if let Some(failure) = api::failure_from(response.status, retry_after, &envelope) {
        return Err(from_api(failure));
    }
    Ok(response.body)
}

/// The `data` half of an answer, parsed.
fn data<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Failure> {
    let envelope: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    let payload = envelope
        .get("data")
        .cloned()
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    serde_json::from_value(payload)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// `mylist` answers one entry when it is asked for one and a list when it is not; both are
/// read as a list so that the two call sites share a shape.
fn entries(body: &[u8]) -> Result<Vec<api::JobEntry>, Failure> {
    let envelope: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    match envelope.get("data") {
        Some(serde_json::Value::Array(items)) => Ok(items
            .iter()
            .filter_map(|item| serde_json::from_value(item.clone()).ok())
            .collect()),
        Some(value @ serde_json::Value::Object(_)) => serde_json::from_value(value.clone())
            .map(|entry| vec![entry])
            .map_err(|_| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent)),
        // `null` for an account that holds nothing of this kind is a real answer, not an error.
        _ => Ok(Vec::new()),
    }
}

/// The contract's source shape, as [`source`] reads it.
fn handed(source: &JobSource) -> Handed<'_> {
    match source {
        JobSource::Magnet(address) => Handed::Magnet(address),
        JobSource::Container(bytes) => Handed::Container(bytes),
        JobSource::Address(address) => Handed::Address(address),
    }
}

/// The job kind whose cache answers for a cache kind the host decided (RD-130-11).
const fn job_kind(kind: CacheKind) -> Kind {
    match kind {
        CacheKind::Torrent => Kind::Torrent,
        CacheKind::Usenet => Kind::Usenet,
        CacheKind::Hoster => Kind::Web,
    }
}

const fn not_known() -> CacheAnswer {
    CacheAnswer {
        state: CacheState::Unknown,
        file_name: None,
        size: None,
    }
}

/// Checks an identifier before it is spliced into a request.
fn safe_id(handle: &RemoteHandle) -> Result<&str, Failure> {
    if api::is_safe_remote_id(&handle.remote_id) {
        Ok(&handle.remote_id)
    } else {
        Err(refuse(messages::JOB_GONE, FailureKind::Permanent))
    }
}

/// The kind a handle belongs to.
///
/// It was written into `job-state` when the job was created, and the host hands that back
/// verbatim. A handle from before this plugin wrote one -- there is none, but a row is a row
/// and the field is an `option` -- is a torrent, because that is the only kind the sibling
/// plugin this one succeeds could ever have created.
fn kind_of(handle: &RemoteHandle) -> Kind {
    handle
        .job_state
        .as_deref()
        .and_then(Kind::parse)
        .unwrap_or(Kind::Torrent)
}

fn handle_for(account_id: &str, kind: Kind, remote_id: String) -> RemoteHandle {
    RemoteHandle {
        remote_id,
        account_id: account_id.to_owned(),
        // The kind, and nothing else. It is not a credential, it is never shown, and without
        // it a `poll` would have to guess which of three endpoints the job lives behind.
        job_state: Some(kind.as_str().to_owned()),
    }
}

impl Guest for Component {
    /// All three of TorBox's caches: torrents, NZBs and web downloads (RD-130-11).
    fn cache_kinds() -> Vec<CacheKind> {
        vec![CacheKind::Torrent, CacheKind::Usenet, CacheKind::Hoster]
    }

    /// Whether TorBox holds each source ready. At most one request per kind -- three in all --
    /// and each answer at its query's position.
    ///
    /// TorBox's `checkcached` names only what it holds, so a digest it leaves out is
    /// `unknown` and never `known`: "not in the cache" says nothing about whether the file
    /// exists. A query whose digest cannot be derived is `unknown` without a request.
    fn check_cached(
        _account_id: String,
        queries: Vec<CacheQuery>,
    ) -> Result<Vec<CacheAnswer>, Failure> {
        let digests: Vec<Option<(Kind, String)>> = queries
            .iter()
            .map(|query| {
                let kind = job_kind(query.kind);
                source::cache_digest(kind, handed(&query.source)).map(|digest| (kind, digest))
            })
            .collect();
        let mut held: Vec<(Kind, api::CachedEntry)> = Vec::new();
        for kind in [Kind::Torrent, Kind::Usenet, Kind::Web] {
            let mut hashes: Vec<&str> = digests
                .iter()
                .flatten()
                .filter(|(asked, _)| *asked == kind)
                .map(|(_, digest)| digest.as_str())
                .collect();
            hashes.sort_unstable();
            hashes.dedup();
            if hashes.is_empty() {
                continue;
            }
            let body = call(
                "GET",
                api::check_cached_path(kind),
                &[
                    RequestQuery {
                        name: "hash".to_owned(),
                        value_template: hashes.join(","),
                    },
                    RequestQuery {
                        name: "format".to_owned(),
                        value_template: "object".to_owned(),
                    },
                    RequestQuery {
                        name: "list_files".to_owned(),
                        value_template: "false".to_owned(),
                    },
                ],
                None,
                &[],
            )?;
            let entries = api::cached_entries(&body)
                .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
            held.extend(entries.into_iter().map(|entry| (kind, entry)));
        }
        Ok(digests
            .iter()
            .map(|digest| {
                let Some((kind, digest)) = digest else {
                    return not_known();
                };
                held.iter()
                    .find(|(held_kind, entry)| {
                        held_kind == kind && entry.hash.eq_ignore_ascii_case(digest)
                    })
                    .map_or_else(not_known, |(_, entry)| CacheAnswer {
                        state: CacheState::Cached,
                        file_name: entry.name.clone(),
                        size: entry.size,
                    })
            })
            .collect())
    }

    /// Reaches nothing. Asked of every source before anything is handed to anybody, and
    /// answered from the bytes alone.
    fn claims(source: JobSource) -> bool {
        source::identify(handed(&source)).is_some()
    }

    /// The kind and the digest TorBox knows the content by, as `<kind>:<hex>`.
    ///
    /// Derived locally and identically every time, which is what lets the host write the row
    /// before the first request and what lets `adopt` recognise a job this installation may
    /// have created seconds before a crash.
    fn identify(source: JobSource) -> Result<String, Failure> {
        source::identify(handed(&source))
            .map(|(_, key)| key)
            .ok_or_else(|| refuse(messages::NOT_A_JOB, FailureKind::Unsupported))
    }

    /// Hands the source over. One request, no retry, no loop.
    ///
    /// Not idempotent, and deliberately not made to look idempotent here: a retry hidden in
    /// this function would be the duplicate the host's row exists to prevent, moved somewhere
    /// nobody would think to look for it.
    fn submit(request: SubmitRequest) -> Result<RemoteHandle, Failure> {
        let Some((kind, _)) = source::identify(handed(&request.source)) else {
            return Err(refuse(messages::NOT_A_JOB, FailureKind::Unsupported));
        };
        // A boundary anybody could recompute would be a boundary a hostile container could
        // carry, and a container that carried it would be submitted cut in half.
        let entropy = host::random_bytes(BOUNDARY_BYTES);
        if entropy.len() != BOUNDARY_BYTES as usize {
            return Err(refuse(
                messages::NO_ENTROPY,
                FailureKind::Transient(Some(5)),
            ));
        }
        let boundary = api::boundary(&entropy);
        let body = match &request.source {
            JobSource::Magnet(address) => {
                api::multipart(&boundary, &[(api::text_field(kind), address)], None)
            }
            JobSource::Address(address) => {
                api::multipart(&boundary, &[(api::text_field(kind), address)], None)
            }
            JobSource::Container(bytes) => api::multipart(
                &boundary,
                &[],
                Some(("file", api::container_name(kind), bytes)),
            ),
        };
        let answer: api::CreatedJob = data(&call(
            "POST",
            api::create_path(kind),
            &[],
            Some(&api::multipart_content_type(&boundary)),
            &body,
        )?)?;
        let Some(id) = answer.id() else {
            // Something may well have been created and this installation cannot name it. The
            // host is told plainly rather than being handed an empty handle it would poll for
            // ever; its adoption check is what looks for the orphan.
            return Err(refuse(messages::NO_JOB_ID, FailureKind::Permanent));
        };
        Ok(handle_for(&request.account_id, kind, id))
    }

    /// The job the account already holds for this content key, if any.
    ///
    /// One page of the kind's own list, and no paging beyond it: this exists for a submit that
    /// was lost seconds ago, which is the newest entry there is, and an account with a
    /// thousand jobs must not cost a thousand requests against a shared budget.
    fn adopt(account_id: String, content_key: String) -> Result<Option<RemoteHandle>, Failure> {
        // The key came back out of the host's own row. It is checked rather than trusted,
        // because what is read out of it decides which endpoint this call reaches.
        let Some((kind, digest)) = source::split_key(&content_key) else {
            return Err(refuse(messages::NOT_A_JOB, FailureKind::Unsupported));
        };
        let body = call(
            "GET",
            api::list_path(kind),
            &[
                RequestQuery {
                    name: "limit".to_owned(),
                    value_template: api::ADOPT_LIMIT.to_string(),
                },
                // The account's list is cached at TorBox, and a cached list is exactly the
                // one that cannot contain the job created a second ago.
                RequestQuery {
                    name: "bypass_cache".to_owned(),
                    value_template: "true".to_owned(),
                },
            ],
            None,
            &[],
        )?;
        let found = entries(&body)?
            .into_iter()
            .find(|entry| {
                entry
                    .hash
                    .as_deref()
                    .is_some_and(|hash| hash.trim().eq_ignore_ascii_case(digest))
            })
            .and_then(|entry| entry.identifier());
        Ok(found.map(|id| handle_for(&account_id, kind, id)))
    }

    fn poll(handle: RemoteHandle) -> Result<RemoteProgress, Failure> {
        let kind = kind_of(&handle);
        let id = safe_id(&handle)?;
        let body = call(
            "GET",
            api::list_path(kind),
            &[
                RequestQuery {
                    name: "id".to_owned(),
                    value_template: id.to_owned(),
                },
                RequestQuery {
                    name: "bypass_cache".to_owned(),
                    value_template: "true".to_owned(),
                },
            ],
            None,
            &[],
        )?;
        // Asked for one job by its identifier, so one entry is the answer. None at all means
        // the job is not in the account any anymore, which is an end and not a wait.
        let Some(entry) = entries(&body)?.into_iter().next() else {
            return Ok(RemoteProgress::Failed(refuse(
                messages::JOB_GONE,
                FailureKind::Offline,
            )));
        };
        Ok(match api::stage_of(&entry) {
            Stage::Preparing(seconds) => RemoteProgress::Preparing(Some(seconds)),
            // `working` carries no suggested wait: a running job is polled on the host's own
            // interval, and the one this plugin would name travels on `preparing`, where
            // there is a field for it.
            Stage::Working(_) => RemoteProgress::Working(RemoteWork {
                progress_permille: api::permille(entry.progress),
                speed_bytes_per_second: entry.download_speed,
                seconds_remaining: entry.eta,
            }),
            Stage::Ready => ready_of(kind, id, &entry)?,
            Stage::Failed(message) => {
                RemoteProgress::Failed(refuse(message, FailureKind::Permanent))
            }
        })
    }

    /// Refused, because TorBox has no call that answers it.
    ///
    /// Unreachable from the host, which only asks after a job answered `awaiting-choice`, and
    /// this plugin never does: TorBox fetches a whole job and offers its files afterwards, so
    /// the place a person picks is the LinkGrabber the finished addresses land in. Answering
    /// `ok` here would be the worse lie -- a selection recorded as honoured that nothing
    /// anywhere acted on.
    fn choose(_handle: RemoteHandle, _chosen: Vec<u32>) -> Result<(), Failure> {
        Err(refuse(messages::NO_SELECTION, FailureKind::Unsupported))
    }

    /// Removes the job at TorBox. Nothing in this plugin calls it.
    fn discard(handle: RemoteHandle) -> Result<(), Failure> {
        let kind = kind_of(&handle);
        let id = safe_id(&handle)?;
        call(
            "POST",
            api::control_path(kind),
            &[],
            Some("application/json"),
            &api::control_body(kind, id, "delete"),
        )?;
        Ok(())
    }
}

/// What a person is offered once TorBox holds the bytes.
///
/// Every file, with the address it is fetched from. That address is the stable `requestdl`
/// one and carries no key: minting the short-lived ticket is the resolver sibling's job, on
/// every attempt, which is what makes a resume after a pause work.
fn ready_of(kind: Kind, id: &str, entry: &api::JobEntry) -> Result<RemoteProgress, Failure> {
    let job_name = entry.name.clone().unwrap_or_default();
    let artifacts: Vec<RemoteArtifact> = entry
        .files
        .iter()
        .take(api::MAX_FILES)
        .filter_map(|file| {
            let file_id = u32::try_from(file.id.as_ref()?.as_u64()?).ok()?;
            let path = file.name.clone().unwrap_or_default();
            let (derived, package_hint) = api::place(&job_name, &path);
            Some(RemoteArtifact {
                url: api::download_address(kind, id, file_id),
                // TorBox states the bare name beside the path at most endpoints; the path is
                // what it always states, so the derived name is the fallback and not the rule.
                file_name: file
                    .short_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .or(derived),
                size: file.size,
                package_hint,
            })
        })
        .collect();
    if artifacts.is_empty() {
        // Finished with nothing is not a result. A package with no files in it and nothing to
        // explain why is the shape of the defect ADR 0001 was opened for.
        return Err(refuse(messages::NO_FILES, FailureKind::Permanent));
    }
    Ok(RemoteProgress::Ready(artifacts))
}

/// Never constructed: TorBox asks nobody anything, so nothing here ever builds a
/// `remote-entry`. Named rather than deleted because the world imports the type either way,
/// and a reader who looks for the selection step should find why there is none.
#[allow(dead_code)]
type NoSelection = RemoteEntry;

export!(Component);
