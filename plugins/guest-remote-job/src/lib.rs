//! The WebAssembly bindings of the remote-job world, generated once for every remote-job plugin.
//!
//! A remote job's guest is its provider's own API — how a job is submitted, polled and read
//! back — so there is no shared adapter to write as there is for the resolvers in
//! `plugin_guest`. What was the same in every one of them is the glue underneath: the generated
//! bindings, the refusal constructor, the conversion of [`plugin_common::ApiFailure`] and the
//! [`call`] around a request (RD-1110-02). They live here, so a remote-job plugin's `guest.rs`
//! imports them, implements [`Guest`] and ends in [`remote_job_plugin!`]. The component that
//! comes out exports the same world under the same names; the host cannot tell the difference.

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

/// Exports a remote-job plugin: `$component` implements [`Guest`].
#[macro_export]
macro_rules! remote_job_plugin {
    ($component:ident) => {
        $crate::export_remote_job!($component with_types_in $crate);
    };
}
