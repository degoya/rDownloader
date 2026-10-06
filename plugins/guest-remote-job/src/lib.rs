//! The WebAssembly bindings of the remote-job world, generated once for every remote-job plugin.
//!
//! A remote job's guest is its provider's own API — how a job is submitted, polled and read
//! back — so there is no shared adapter to write as there is for the resolvers in
//! `plugin_guest`. What was the same in every one of them is the glue underneath: the generated
//! bindings, the refusal constructor, the conversion of [`plugin_common::ApiFailure`] and the
//! [`call`] around a request (RD-1110-02), and since RD-1120-10 the request [`headers`], the
//! JSON reader [`parse`], the identifier check [`safe_id`], the stateless [`handle_for`] and the
//! cache answers of a plugin with nothing to say. They live here, so a remote-job plugin's
//! `guest.rs` imports them, implements [`Guest`] and ends in [`remote_job_plugin!`]. The
//! component that comes out exports the same world under the same names; the host cannot tell
//! the difference.

#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "remote-job-plugin",
    // The bindings live here rather than in each plugin, so the export macro has to be usable
    // from another crate and needs a name that says what it exports.
    pub_export_macro: true,
    export_macro_name: "export_remote_job",
});

pub use exports::rdownloader::plugin::remote_job::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, Guest, JobSource, RemoteArtifact, RemoteEntry,
    RemoteHandle, RemoteProgress, RemoteWork, SubmitRequest,
};
pub use rdownloader::plugin::{host, http, job_context, types};

use http::{RequestHeader, RequestQuery};
use plugin_common::ApiFailure;
use serde::de::DeserializeOwned;
use types::{Failure, FailureKind};

/// A failure carrying a stable translation code and its English fallback, and nothing else.
#[must_use]
pub fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// A classified refusal, or any failure in [`plugin_common`]'s vocabulary, in the WIT one.
#[must_use]
pub fn to_wit_failure(failure: impl Into<plugin_common::Failure>) -> Failure {
    let failure = failure.into();
    Failure {
        category: to_wit_kind(failure.kind),
        message: failure.message,
        code: failure.code,
        params: failure.params,
    }
}

fn to_wit_kind(kind: plugin_common::FailureKind) -> FailureKind {
    use plugin_common::FailureKind as Kind;
    match kind {
        Kind::Transient(seconds) => FailureKind::Transient(seconds),
        Kind::Permanent => FailureKind::Permanent,
        Kind::Offline => FailureKind::Offline,
        Kind::AuthRequired => FailureKind::AuthRequired,
        Kind::AccountInvalid => FailureKind::AccountInvalid,
        Kind::RateLimited(seconds) => FailureKind::RateLimited(seconds),
        Kind::NeedsCaptcha => FailureKind::NeedsCaptcha,
        Kind::Unsupported => FailureKind::Unsupported,
        Kind::IpBlocked(seconds) => FailureKind::IpBlocked(seconds),
        Kind::CaptchaFailed => FailureKind::CaptchaFailed,
    }
}

/// One request, with every answer `refused` names a refusal turned into one failure; the
/// remote-job twin of [`plugin_common::failure::call`].
///
/// `refused` reads the status, the stated `Retry-After` (seconds only, never `0`, at most a
/// day: the reader every plugin shares, RD-191-07) and the body. The vocabulary stays small on
/// purpose: a caller gets the body or a failure and never decides a second time what a status
/// code means.
///
/// # Errors
///
/// The host's failure to make the request, or the refusal `refused` named.
pub fn call(
    method: &str,
    url: &str,
    query: &[RequestQuery],
    headers: &[RequestHeader],
    body: &[u8],
    refused: impl FnOnce(u16, Option<u64>, &[u8]) -> Option<ApiFailure>,
) -> Result<Vec<u8>, Failure> {
    let response = http::http_request(method, url, query, headers, body)?;
    let retry_after = plugin_common::retry_after(&response.headers);
    match refused(response.status, retry_after, &response.body) {
        Some(failure) => Err(to_wit_failure(failure)),
        None => Ok(response.body),
    }
}

/// `Bearer {{secret:<reference>}}`: the credential as a template. Its value never reaches the
/// plugin; the host substitutes it on the way out, towards the hosts the manifest grants and
/// nowhere else.
#[must_use]
pub fn bearer(reference: &str) -> String {
    format!("Bearer {{{{secret:{reference}}}}}")
}

/// The headers every request of a remote-job plugin carries: the `authorization` template (see
/// [`bearer`]), `Accept: application/json`, and the body's type when there is a body.
#[must_use]
pub fn headers(authorization: String, content_type: Option<&str>) -> Vec<RequestHeader> {
    let mut headers = vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: authorization,
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

/// An answer read as JSON, or `invalid` as a permanent refusal: an answer that is not the shape
/// the provider documents is not going to become it by asking again.
///
/// # Errors
///
/// `invalid`, when the body does not parse into `T`.
pub fn parse<T: DeserializeOwned>(body: &[u8], invalid: (&str, &str)) -> Result<T, Failure> {
    serde_json::from_slice(body).map_err(|_| refuse(invalid, FailureKind::Permanent))
}

/// The identifier on a handle, checked with `is_safe` before it is spliced into a request path,
/// a query or a body.
///
/// The host hands back what a row holds, and a row is a row: an identifier carrying a slash or
/// a dot segment would be a request to somewhere else on the very host the plugin may reach. A
/// handle that fails is a job that cannot be named any more, so `gone`, permanently.
///
/// # Errors
///
/// `gone`, when `is_safe` refuses the identifier.
pub fn safe_id<'a>(
    handle: &'a RemoteHandle,
    is_safe: fn(&str) -> bool,
    gone: (&str, &str),
) -> Result<&'a str, Failure> {
    if is_safe(&handle.remote_id) {
        Ok(&handle.remote_id)
    } else {
        Err(refuse(gone, FailureKind::Permanent))
    }
}

/// The handle of a job whose identifier is the whole of it.
///
/// Nothing in `job-state`: a plugin that put something there would be storing state the host
/// would have to keep for no reason.
#[must_use]
pub fn handle_for(account_id: &str, remote_id: String) -> RemoteHandle {
    RemoteHandle {
        remote_id,
        account_id: account_id.to_owned(),
        job_state: None,
    }
}

/// The answer for a cache query that was not asked, or that the provider said nothing about.
#[must_use]
pub const fn unknown_answer() -> CacheAnswer {
    CacheAnswer {
        state: CacheState::Unknown,
        file_name: None,
        size: None,
    }
}

/// Every query `unknown`, one per query and in order, without a request -- "nothing to say" is
/// never a failure. The whole of `check-cached` for a plugin whose `cache-kinds` is empty, and
/// the starting point of one that answers only some queries.
#[must_use]
pub fn unknown_answers(queries: &[CacheQuery]) -> Vec<CacheAnswer> {
    queries.iter().map(|_| unknown_answer()).collect()
}

/// Exports a remote-job plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! remote_job_plugin {
    ($component:ident) => {
        $crate::export_remote_job!($component with_types_in $crate);
    };
}
