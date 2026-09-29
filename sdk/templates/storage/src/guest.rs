//! The component: one package file, uploaded in chunks and then confirmed.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "storage-plugin",
});

use exports::rdownloader::plugin::storage::{Guest, UploadEnd, UploadJob};
use rdownloader::plugin::{
    http::{self, HttpResponse, RequestHeader, RequestQuery},
    source,
    types::{Failure, FailureKind},
};

use crate::Session;

struct Component;

/// Bytes read and sent per request. Large enough to be worth the crossing, small enough to
/// leave room for the guest's own memory limit.
const CHUNK: u32 = 256 * 1024;

impl Guest for Component {
    fn put(job: UploadJob) -> UploadEnd {
        match upload(&job) {
            Ok(end) => end,
            Err(failure) => UploadEnd::Failed(failure),
        }
    }

    /// Asks the destination whether it really holds the object.
    ///
    /// Answer `false` rather than an error when it does not: a missing object is a fact about
    /// the destination, and the host keeps the local copy either way.
    fn verify(_handle: String, remote_id: String) -> Result<bool, Failure> {
        // `verify` is not told whether a login is stored, so it asks without one first and
        // repeats the question with it only when the server wants one.
        let mut response = request("GET", &remote_id, &[], &[], false)?;
        if matches!(response.status, 401 | 403) {
            response = request("GET", &remote_id, &[], &[], true)?;
        }
        if response.status == 404 {
            return Ok(false);
        }
        checked(&response)?;
        // A size of zero for a file that has bytes means the server accepted the request and
        // stored nothing — the case this whole call exists to catch.
        let body = String::from_utf8_lossy(&response.body);
        Ok(crate::answered_size(&body).is_some_and(|size| size > 0))
    }
}

fn upload(job: &UploadJob) -> Result<UploadEnd, Failure> {
    let Some(base) = crate::base(&job.destination) else {
        return Err(refuse(
            FailureKind::Permanent,
            "bad_destination",
            "the destination is not an https address",
        ));
    };
    let credential = job.credential_ref.is_some();
    // Resume the upload the checkpoint names, or open one.
    let mut session = match Session::from_bytes(job.checkpoint.as_deref()) {
        Some(session) => session,
        None => open(base, job, credential)?,
    };
    while session.offset < job.size {
        if source::should_stop() {
            return Ok(UploadEnd::Stopped(session.to_bytes()));
        }
        let chunk = source::read_at(&job.handle, &job.file_name, session.offset, CHUNK)?;
        if chunk.is_empty() {
            break;
        }
        // A `PUT` at an offset is idempotent, which is what makes this safe after a crash: the
        // checkpoint may lag one chunk behind the server, and sending that chunk again
        // overwrites it with the same bytes.
        let url = format!("{base}/uploads/{}", session.id);
        let offset = session.offset.to_string();
        let response = request("PUT", &url, &[("offset", &offset)], &chunk, credential)?;
        checked(&response)?;
        session.offset += chunk.len() as u64;
        source::progress(session.offset, Some(job.size));
    }
    let complete = format!("{base}/uploads/{}/complete", session.id);
    let response = request("POST", &complete, &[], &[], credential)?;
    checked(&response)?;
    let Some(file) = crate::answered_id(&String::from_utf8_lossy(&response.body)) else {
        return Err(refuse(
            FailureKind::Permanent,
            "bad_reply",
            "the server did not name the stored file",
        ));
    };
    // The address is the remote identity: `verify` is a separate call that may run much later,
    // so it has to be something that still means the same thing then.
    Ok(UploadEnd::Complete(Some(format!("{base}/files/{file}"))))
}

/// Opens an upload for the job's file.
fn open(base: &str, job: &UploadJob, credential: bool) -> Result<Session, Failure> {
    let size = job.size.to_string();
    let query = [("name", job.file_name.as_str()), ("size", size.as_str())];
    let response = request("POST", &format!("{base}/uploads"), &query, &[], credential)?;
    checked(&response)?;
    let id = crate::answered_id(&String::from_utf8_lossy(&response.body)).ok_or_else(|| {
        refuse(
            FailureKind::Permanent,
            "bad_reply",
            "the server did not name the upload",
        )
    })?;
    Ok(Session { id, offset: 0 })
}

/// One request, with the stored login when the destination has one.
///
/// The host substitutes `{{secret}}` with the one secret this invocation was granted; the
/// plugin never holds it. Without a credential the header is left off entirely rather than
/// sent empty, so a public destination still works and a missing password fails as a 401 the
/// person can read.
fn request(
    method: &str,
    url: &str,
    query: &[(&str, &str)],
    body: &[u8],
    credential: bool,
) -> Result<HttpResponse, Failure> {
    let query: Vec<RequestQuery> = query
        .iter()
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: (*value).to_owned(),
        })
        .collect();
    let mut headers = vec![RequestHeader {
        name: "Content-Type".to_owned(),
        value_template: "application/octet-stream".to_owned(),
    }];
    if credential {
        headers.push(RequestHeader {
            name: "Authorization".to_owned(),
            value_template: "Bearer {{secret}}".to_owned(),
        });
    }
    http::http_request(method, url, &query, &headers, body)
}

/// The server's answer as a failure the host can act on, or `Ok` for any 2xx.
fn checked(response: &HttpResponse) -> Result<(), Failure> {
    match response.status {
        200..=299 => Ok(()),
        // Neither becomes anything else by repeating it; the person has to fix the login.
        401 | 403 => Err(refuse(
            FailureKind::AuthRequired,
            "unauthorized",
            "the destination refused the stored login",
        )),
        // Worth another attempt: the checkpoint carries the upload on from where it stopped.
        429 | 500..=599 => Err(refuse(
            FailureKind::Transient(None),
            "unavailable",
            "the destination did not answer",
        )),
        _ => Err(refuse(
            FailureKind::Permanent,
            "upload_rejected",
            "the destination refused the upload",
        )),
    }
}

/// A failure carrying a stable translation code and nothing the server wrote.
fn refuse(category: FailureKind, code: &str, message: &str) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

export!(Component);
