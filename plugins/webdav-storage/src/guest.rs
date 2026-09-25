//! The component: one package file, uploaded to a WebDAV collection and then confirmed.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "storage-plugin",
});

use exports::rdownloader::plugin::storage::{Guest, UploadEnd, UploadJob};
use rdownloader::plugin::{
    http::{self, RequestHeader},
    source,
    types::{Failure, FailureKind},
};

use crate::target;

struct Component;

/// Bytes read from the package per call.
const CHUNK: u32 = 512 * 1024;

impl Guest for Component {
    fn put(job: UploadJob) -> UploadEnd {
        // The collection is created before the first byte: `PUT` to a path whose parent does
        // not exist is a 409 on every server, and creating it once is cheaper than finding
        // out per file.
        let collection = target::collection(&job.destination, &job.handle);
        if let Err(failure) = ensure_collection(&job, &collection) {
            return UploadEnd::Failed(failure);
        }
        let url = target::file_url(&collection, &job.file_name);

        // Read the file whole before sending it. WebDAV has no resumable upload in the base
        // protocol, so the checkpoint records whole files rather than byte offsets: a stopped
        // upload resumes at the next file, never in the middle of one.
        let mut body = Vec::with_capacity(usize::try_from(job.size).unwrap_or(0));
        while (body.len() as u64) < job.size {
            if source::should_stop() {
                return UploadEnd::Stopped(Vec::new());
            }
            match source::read_at(&job.handle, &job.file_name, body.len() as u64, CHUNK) {
                Ok(chunk) if chunk.is_empty() => break,
                Ok(chunk) => body.extend_from_slice(&chunk),
                Err(failure) => return UploadEnd::Failed(failure),
            }
            source::progress(body.len() as u64, Some(job.size));
        }

        let response = match http::http_request("PUT", &url, &[], &headers(&job), &body) {
            Ok(response) => response,
            Err(failure) => return UploadEnd::Failed(failure),
        };
        if !(200..300).contains(&response.status) {
            return UploadEnd::Failed(refuse(
                format!("the server answered {} to the upload", response.status),
                response.status,
            ));
        }
        // The address is the remote identity: `verify` is a separate call that may run much
        // later, so it has to be something that still means the same thing then.
        UploadEnd::Complete(Some(url))
    }

    fn verify(handle: String, remote_id: String) -> Result<bool, Failure> {
        // Answering `false` rather than failing when the object is not there: a missing file
        // is a fact about the destination, and the host keeps the local copy either way.
        let _ = handle;
        let response = http::http_request(
            "PROPFIND",
            &remote_id,
            &[],
            &[
                RequestHeader {
                    name: "Depth".to_owned(),
                    value_template: "0".to_owned(),
                },
                RequestHeader {
                    name: "Content-Type".to_owned(),
                    value_template: "application/xml; charset=utf-8".to_owned(),
                },
                RequestHeader {
                    name: "Authorization".to_owned(),
                    value_template: "Basic {{secret}}".to_owned(),
                },
            ],
            target::PROPFIND_BODY.as_bytes(),
        )?;
        if response.status == 404 {
            return Ok(false);
        }
        if !(200..300).contains(&response.status) {
            return Err(refuse(
                format!("the server answered {} to the check", response.status),
                response.status,
            ));
        }
        let body = String::from_utf8_lossy(&response.body);
        // A length of zero for a file that has bytes means the server accepted the request
        // and stored nothing, which is the case this whole call exists to catch.
        Ok(target::content_length(&body).is_some_and(|length| length > 0))
    }
}

/// `MKCOL` for the package's collection. An existing one answers 405, which is success here.
fn ensure_collection(job: &UploadJob, collection: &str) -> Result<(), Failure> {
    let response = http::http_request("MKCOL", collection, &[], &headers(job), &[])?;
    if (200..300).contains(&response.status) || response.status == 405 {
        return Ok(());
    }
    Err(refuse(
        format!(
            "the server answered {} when creating the folder",
            response.status
        ),
        response.status,
    ))
}

/// The credential header, when this destination has a stored login.
///
/// The host substitutes `{{secret}}` with the one secret this invocation was granted; the
/// plugin never holds it. Without a credential the header is left off entirely rather than
/// sent empty, so a public collection still works and a missing password fails as a 401 the
/// person can read.
fn headers(job: &UploadJob) -> Vec<RequestHeader> {
    if job.credential_ref.is_none() {
        return Vec::new();
    }
    vec![RequestHeader {
        name: "Authorization".to_owned(),
        value_template: "Basic {{secret}}".to_owned(),
    }]
}

fn refuse(message: String, status: u16) -> Failure {
    Failure {
        // 5xx and the lock conflict are worth another attempt; a 401 or a 403 will not become
        // anything else by repeating it.
        category: if status >= 500 || status == 423 || status == 429 {
            FailureKind::Transient(None)
        } else if status == 401 || status == 403 {
            FailureKind::AuthRequired
        } else {
            FailureKind::Permanent
        },
        message,
        code: Some("webdav_storage.upload_rejected".to_owned()),
        params: Vec::new(),
    }
}

export!(Component);
