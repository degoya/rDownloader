//! The component: a MEGA folder address in, the files behind it out.
//!
//! One call, one answer, one walk. MEGA has no cursor for `a=f`, so there is no loop here to
//! bound: what bounds this crawl is the manifest's `max_response_bytes` before the guest sees
//! a byte, and `walk::MAX_FILES` after it.
//!
//! A folder of the signed-in account (RD-120-30) is listed with the session as the host's
//! marker and walked by `account`, which asks the host to unwrap each node's key under the
//! session's key half. One host call per node, never the master key in here.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use mega_common::{Target, api};
use rdownloader::plugin::{
    http::{self, RequestHeader, RequestQuery},
    key_derivation::{self, SecretHandle, Step},
    types::{Failure, FailureKind},
};
use serde_json::Value;

use crate::{account, messages, walk};

struct Component;

fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// How one of MEGA's negative answers is reported, for a call made with or without the
/// account's session.
fn api_failure(code: i64, signed_in: bool) -> Failure {
    match code {
        api::ESID if signed_in => refuse(messages::SESSION_EXPIRED, FailureKind::AuthRequired),
        api::ENOENT => refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent),
        api::EACCESS | api::ESID => refuse(messages::FOLDER_PRIVATE, FailureKind::Permanent),
        api::EAGAIN | api::ERATELIMIT => {
            refuse(messages::RATE_LIMITED, FailureKind::RateLimited(None))
        }
        // A quota is not a rate limit: waiting a few seconds does not clear it, and telling
        // a person their folder is "being throttled" when MEGA said the allowance is gone
        // sends them back to retry. Both are `rate-limited` to the scheduler, which is what
        // holds the link back; only the text and the code differ.
        api::EOVERQUOTA => refuse(messages::QUOTA_EXCEEDED, FailureKind::RateLimited(None)),
        api::EBLOCKED | api::ETEMPUNAVAIL => {
            refuse(messages::UNAVAILABLE, FailureKind::Transient(None))
        }
        other => Failure {
            category: FailureKind::Permanent,
            message: messages::api_error(other),
            code: Some(messages::API_ERROR.to_owned()),
            params: vec![("status".to_owned(), other.to_string())],
        },
    }
}

/// A refusal this plugin has its own name for, in the shape the failure record wants.
fn named((code, message): (&str, &str)) -> (String, String) {
    (code.to_owned(), message.to_owned())
}

/// The node list of one shared folder, or -- with `None` -- of the signed-in account.
fn listing(folder: Option<&str>) -> Result<Value, Failure> {
    let headers = vec![RequestHeader {
        name: "Content-Type".to_owned(),
        value_template: "application/json".to_owned(),
    }];
    let (query, body) = match folder {
        Some(_) => (Vec::new(), api::folder_request()),
        None => (
            vec![RequestQuery {
                name: api::SESSION_QUERY.0.to_owned(),
                value_template: api::SESSION_QUERY.1.to_owned(),
            }],
            api::account_listing_request(),
        ),
    };
    let response = http::http_request("POST", &api::endpoint(folder), &query, &headers, &body)?;
    if !(200..300).contains(&response.status) {
        // How long to wait is the provider's to say, and it says it in a header rather than
        // in the body: `X-MEGA-Time-Left` on a `509`, `Retry-After` otherwise. Passing it on
        // is what turns a bandwidth refusal into a scheduled retry instead of a loop.
        let delay = api::retry_after(&response.headers);
        let (code, message) = match response.status {
            api::STATUS_BANDWIDTH_EXCEEDED => named(messages::QUOTA_EXCEEDED),
            429 => named(messages::RATE_LIMITED),
            status => (
                messages::HTTP_ERROR.to_owned(),
                messages::http_error(status),
            ),
        };
        return Err(Failure {
            category: match response.status {
                429 | api::STATUS_BANDWIDTH_EXCEEDED => FailureKind::RateLimited(delay),
                500..=599 => FailureKind::Transient(delay),
                _ => FailureKind::Permanent,
            },
            message,
            code: Some(code),
            params: vec![("status".to_owned(), response.status.to_string())],
        });
    }
    api::first_object(&response.body).map_err(|number| match number {
        None => refuse(messages::INVALID_RESPONSE, FailureKind::Permanent),
        Some(code) => api_failure(code, folder.is_none()),
    })
}

/// A folder of the signed-in account (RD-120-30).
fn crawl_account(handle: &str) -> Result<Vec<CrawledLink>, Failure> {
    let answer = listing(None)?;
    let session = SecretHandle {
        reference: api::SESSION_SECRET.to_owned(),
    };
    let found = account::expand(&answer, handle, |wrapped: &[u8]| {
        key_derivation::derive(&session, &[Step::AesEcbDecrypt(wrapped.to_vec())])
    })
    .map_err(|refusal| match refusal {
        account::Refusal::NotInAccount => refuse(messages::NOT_IN_ACCOUNT, FailureKind::Permanent),
        account::Refusal::Empty => refuse(messages::FOLDER_EMPTY, FailureKind::Permanent),
        account::Refusal::TooMany => too_many(),
        account::Refusal::Host(failure) => failure,
    })?;
    Ok(found
        .into_iter()
        .map(|file| CrawledLink {
            url: Target::account_file_url(&file.node),
            file_name: Some(file.name),
            size: Some(file.size),
            package_hint: Some(file.path).filter(|path| !path.is_empty()),
        })
        .collect())
}

fn too_many() -> Failure {
    Failure {
        category: FailureKind::Permanent,
        message: messages::too_many_files(walk::MAX_FILES),
        code: Some(messages::TOO_MANY_FILES.to_owned()),
        params: vec![("limit".to_owned(), walk::MAX_FILES.to_string())],
    }
}

impl Guest for Component {
    fn claims_url(url: String) -> bool {
        matches!(
            Target::parse(&url),
            Some(Target::Folder { .. } | Target::AccountNode { .. })
        )
    }

    fn crawl(url: String) -> Result<Vec<CrawledLink>, Failure> {
        let (handle, key) = match Target::parse(&url) {
            Some(Target::Folder { handle, key }) => (handle, key),
            Some(Target::AccountNode { handle }) => return crawl_account(&handle),
            _ => return Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported)),
        };
        let Some(share) =
            mega_common::crypto::b64_decode(&key).and_then(|raw| <[u8; 16]>::try_from(raw).ok())
        else {
            return Err(refuse(messages::KEY_INVALID, FailureKind::Permanent));
        };
        let answer = listing(Some(&handle))?;
        let found = walk::expand(&answer, &share).map_err(|refusal| match refusal {
            walk::Refusal::NoRoot => refuse(messages::INVALID_RESPONSE, FailureKind::Permanent),
            walk::Refusal::Empty => refuse(messages::FOLDER_EMPTY, FailureKind::Permanent),
            walk::Refusal::TooMany => too_many(),
        })?;
        Ok(found
            .into_iter()
            .map(|file| CrawledLink {
                url: Target::child_url(&handle, &key, &file.node),
                file_name: Some(file.name),
                size: Some(file.size),
                package_hint: Some(file.path).filter(|path| !path.is_empty()),
            })
            .collect())
    }
}

export!(Component);
