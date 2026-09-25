//! The component: a magnet or a `.torrent` in, a job at the provider out.
//!
//! Nine short calls and no loop. Every function here makes at most one request and returns on
//! the provider's next answer; nothing waits for a job, because the waiting is the host's — it
//! holds the row, the clock and the person's answer.
//!
//! Replace the two constants below and the endpoints in each function with the provider's
//! own. The shape around them — which calls reach the network, which do not, what a state
//! means, what a failure carries — is the contract and changes far less often.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
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
    reply::{self, Stage},
    source,
};

/// The provider's API. Every request this plugin makes starts here, and the manifest's
/// `capabilities.net_http` domains are what actually allow it — the host refuses the rest.
const API_BASE: &str = "https://api.example.com/v1";
/// The vault slot the account's token lives in. Its value never reaches this guest: it goes
/// out as `{{secret:<reference>}}` and the host substitutes it on the way.
const SECRET_REFERENCE: &str = "example_access_token";
/// How many jobs `adopt` looks through. One page and no paging beyond it: adoption exists for
/// a submit that was lost seconds ago, which is the newest entry there is, and an account with
/// a thousand jobs must not cost a thousand requests against a shared request budget.
const ADOPT_PAGE: u32 = 50;

struct Component;

/// A failure carrying a stable translation code and nothing the provider wrote.
///
/// The message is English and redaction-safe, for a log. What a person reads is the code,
/// translated in `locales/`; repeating a provider's own sentence would put untranslated and
/// occasionally identifying text in front of them.
fn refuse(code: &str, message: &str, category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

fn headers(content_type: Option<&str>) -> Vec<RequestHeader> {
    let mut headers = vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: format!("Bearer {{{{secret:{SECRET_REFERENCE}}}}}"),
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
/// The vocabulary stays small on purpose: a caller gets a body or a failure and never decides
/// a second time what a status code means. Which category a refusal carries decides what the
/// host does with the row — transient, rate-limited and IP-blocked keep it and wait, and
/// every other category ends it — so it is worth getting right rather than defaulting.
fn call(
    method: &str,
    url: &str,
    query: &[RequestQuery],
    content_type: Option<&str>,
    body: &[u8],
) -> Result<String, Failure> {
    let response = http::http_request(method, url, query, &headers(content_type), body)?;
    let retry_after = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok());
    match response.status {
        200..=299 => Ok(String::from_utf8_lossy(&response.body).into_owned()),
        401 | 403 => Err(refuse(
            "rejected",
            "the provider did not accept this account",
            FailureKind::AccountInvalid,
        )),
        404 | 410 => Err(refuse(
            "job_gone",
            "the provider no longer holds this job",
            FailureKind::Permanent,
        )),
        429 => Err(refuse(
            "rejected",
            "the provider is rate limiting this account",
            FailureKind::RateLimited(retry_after),
        )),
        500..=599 => Err(refuse(
            "rejected",
            "the provider answered with a server error",
            // Transient, so the host keeps the row and comes back. A provider having a bad
            // minute must not cost somebody a job that is running perfectly well at it.
            FailureKind::Transient(retry_after),
        )),
        _ => Err(refuse(
            "rejected",
            "the provider refused this request",
            FailureKind::Permanent,
        )),
    }
}

/// The content key of a source, or the refusal that says it is not one of ours.
fn key_of(source: &JobSource) -> Result<String, Failure> {
    let key = match source {
        JobSource::Magnet(address) => source::magnet_info_hash(address),
        JobSource::Container(bytes) => source::container_info_hash(bytes),
        JobSource::Address(address) => source::address_key(address),
    };
    key.ok_or_else(|| {
        refuse(
            "not_a_job",
            "this is not a source this plugin runs",
            FailureKind::Unsupported,
        )
    })
}

/// Checks an identifier before it is spliced into a request path.
fn safe_id(handle: &RemoteHandle) -> Result<&str, Failure> {
    if reply::is_safe_id(&handle.remote_id) {
        Ok(&handle.remote_id)
    } else {
        Err(refuse(
            "job_gone",
            "this job has no identifier this plugin can use",
            FailureKind::Permanent,
        ))
    }
}

fn handle_for(account_id: &str, remote_id: String) -> RemoteHandle {
    RemoteHandle {
        remote_id,
        account_id: account_id.to_owned(),
        // Nothing to carry: the identifier is the whole handle here. Put something in it only
        // when the next call genuinely needs it, because the host stores whatever is here for
        // the life of the job.
        job_state: None,
    }
}

/// Percent-encodes one form value. A magnet carries `&`, `:` and `=`, every one of which
/// would end the field it sits in.
fn form_value(value: &str) -> String {
    value.bytes().fold(String::new(), |mut out, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

impl Guest for Component {
    /// The kinds of source your provider's cache can be asked about -- `torrent`, `usenet`,
    /// `hoster`. Reaches nothing. Empty until you bind a read-only cache query: the host then
    /// never calls `check_cached`. A provider whose only way to find out is to submit has no
    /// cache query and stays empty.
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
        key_of(&source).is_ok()
    }

    /// The content key: derived locally, without a request, and the same for the same content
    /// every time. It is what makes a duplicate preventable — see `src/source.rs`.
    fn identify(source: JobSource) -> Result<String, Failure> {
        key_of(&source)
    }

    /// Hands the source over. One request, no retry, no loop.
    ///
    /// Deliberately not made to look idempotent: a retry hidden in this function would be the
    /// duplicate the host's row exists to prevent, moved somewhere nobody would think to look.
    fn submit(request: SubmitRequest) -> Result<RemoteHandle, Failure> {
        let (method, content_type, body) = match &request.source {
            JobSource::Magnet(address) => (
                "POST",
                "application/x-www-form-urlencoded",
                format!("magnet={}", form_value(address)).into_bytes(),
            ),
            JobSource::Container(bytes) => ("PUT", "application/x-bittorrent", bytes.clone()),
            // The third shape (RD-120-20): a plain address the provider fetches for itself.
            // Most providers take it on the same endpoint as a magnet, under another field.
            JobSource::Address(address) => (
                "POST",
                "application/x-www-form-urlencoded",
                format!("src={}", form_value(address)).into_bytes(),
            ),
        };
        let answer = call(
            method,
            &format!("{API_BASE}/jobs"),
            &[],
            Some(content_type),
            &body,
        )?;
        let id = reply::string_field(&answer, "id").filter(|id| reply::is_safe_id(id));
        let Some(id) = id else {
            // Something may well have been created and this installation cannot name it. The
            // host is told plainly rather than handed an empty handle it would poll for ever;
            // its adoption check is what looks for the orphan.
            return Err(refuse(
                "no_job_id",
                "the provider accepted the job without naming it",
                FailureKind::Permanent,
            ));
        };
        Ok(handle_for(&request.account_id, id))
    }

    /// The job the provider already holds for this content key, if there is one.
    ///
    /// Called before a second `submit` and never on its own. It is the one answer to a crash
    /// between the request leaving and the identifier coming back, and `none` is a real
    /// answer rather than an error: the provider has nothing for that key.
    fn adopt(account_id: String, content_key: String) -> Result<Option<RemoteHandle>, Failure> {
        let body = call(
            "GET",
            &format!("{API_BASE}/jobs"),
            &[RequestQuery {
                name: "limit".to_owned(),
                value_template: ADOPT_PAGE.to_string(),
            }],
            None,
            &[],
        )?;
        let found = reply::objects(&body, "jobs").into_iter().find_map(|job| {
            let key = reply::string_field(&job, "key")?;
            if !key.eq_ignore_ascii_case(&content_key) {
                return None;
            }
            reply::string_field(&job, "id").filter(|id| reply::is_safe_id(id))
        });
        Ok(found.map(|id| handle_for(&account_id, id)))
    }

    /// Where the job stands now. Called on the host's schedule and never on this plugin's.
    fn poll(handle: RemoteHandle) -> Result<RemoteProgress, Failure> {
        let id = safe_id(&handle)?;
        let body = call("GET", &format!("{API_BASE}/jobs/{id}"), &[], None, &[])?;
        let state = reply::string_field(&body, "state").unwrap_or_default();
        Ok(match reply::stage_of(&state) {
            Stage::Preparing(seconds) => RemoteProgress::Preparing(Some(seconds)),
            Stage::AwaitingChoice => RemoteProgress::AwaitingChoice(entries_of(&body)),
            // No suggested wait on `working`: a running job is polled on the host's own
            // interval, and the suggestion travels on `preparing`, where there is a field
            // for it.
            Stage::Working => RemoteProgress::Working(RemoteWork {
                progress_permille: reply::permille(reply::number_field(&body, "progress")),
                speed_bytes_per_second: reply::number_field(&body, "speed"),
                seconds_remaining: None,
            }),
            Stage::Ready => ready_of(&body)?,
            Stage::Failed => RemoteProgress::Failed(refuse(
                "rejected",
                "the provider ended this job",
                FailureKind::Permanent,
            )),
        })
    }

    /// Answers the question `awaiting-choice` asked. The host never sends an empty list, so
    /// this never has to decide what "nothing" would have meant.
    fn choose(handle: RemoteHandle, chosen: Vec<u32>) -> Result<(), Failure> {
        let id = safe_id(&handle)?;
        if chosen.is_empty() {
            return Err(refuse(
                "empty_choice",
                "no entries were chosen",
                FailureKind::Permanent,
            ));
        }
        let ids = chosen
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        call(
            "POST",
            &format!("{API_BASE}/jobs/{id}/choose"),
            &[],
            Some("application/json"),
            format!("{{\"files\":[{ids}]}}").as_bytes(),
        )?;
        Ok(())
    }

    /// Removes the job at the provider. Nothing in this plugin calls it, and nothing in the
    /// host's sweep does either: it is reached from one explicit, confirmed request.
    fn discard(handle: RemoteHandle) -> Result<(), Failure> {
        let id = safe_id(&handle)?;
        call("DELETE", &format!("{API_BASE}/jobs/{id}"), &[], None, &[])?;
        Ok(())
    }
}

/// What the person is shown while the job waits for an answer.
///
/// The ids are the provider's own numbering and not positions in this list, because `choose`
/// names them back and a position would be wrong the moment the provider reorders anything.
fn entries_of(body: &str) -> Vec<RemoteEntry> {
    reply::objects(body, "files")
        .iter()
        .filter_map(|file| {
            Some(RemoteEntry {
                id: u32::try_from(reply::number_field(file, "id")?).ok()?,
                path: reply::string_field(file, "path").unwrap_or_default(),
                size: reply::number_field(file, "size"),
                selected: reply::flag_field(file, "selected"),
            })
        })
        .collect()
}

/// What a finished job produced: addresses, not bytes.
///
/// At a debrid provider the links a finished job carries are still restricted ones, so they
/// go to the resolver that already exists and then down the ordinary resumable queue.
fn ready_of(body: &str) -> Result<RemoteProgress, Failure> {
    let artifacts: Vec<RemoteArtifact> = reply::objects(body, "links")
        .iter()
        .filter_map(|link| {
            let url = reply::string_field(link, "url").filter(|url| !url.is_empty())?;
            let (file_name, package_hint) =
                reply::place(&reply::string_field(link, "path").unwrap_or_default());
            Some(RemoteArtifact {
                url,
                file_name,
                size: reply::number_field(link, "size"),
                package_hint,
            })
        })
        .collect();
    if artifacts.is_empty() {
        // Finished with nothing is not a result. A package with no files in it and nothing to
        // explain why is exactly what an empty `crawl` would have been.
        return Err(refuse(
            "job_empty",
            "the job finished without producing anything to download",
            FailureKind::Permanent,
        ));
    }
    Ok(RemoteProgress::Ready(artifacts))
}

export!(Component);
