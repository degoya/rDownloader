//! The component: one list address in, the files behind it out.
//!
//! One `GET` against `/api/list/{id}` and nothing else. The candidates it produces are `/u/`
//! addresses the sibling resolver handles one at a time, so no link here is ever followed and
//! no byte of content is fetched.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use rdownloader::plugin::{
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};

use crate::{list, messages};

struct Component;

fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// The refusal a stable Pixeldrain token stands for.
///
/// The same five groups the sibling resolver tells apart, because they ask the same different
/// things of the person. An unknown token travels as `api_code`; the provider's sentence never
/// does.
fn from_token(token: &str) -> Failure {
    match token {
        "not_found" | "list_not_found" | "file_not_found" => {
            refuse(messages::LIST_NOT_FOUND, FailureKind::Permanent)
        }
        "authentication_required" | "unauthorized" | "permission_denied" => {
            refuse(messages::ACCOUNT_REQUIRED, FailureKind::AuthRequired)
        }
        "ip_rate_limit_reached"
        | "rate_limited"
        | "too_many_requests"
        | "transfer_limit_exceeded" => {
            refuse(messages::RATE_LIMITED, FailureKind::RateLimited(Some(3600)))
        }
        "internal" | "internal_error" => {
            refuse(messages::SERVER_ERROR, FailureKind::Transient(Some(300)))
        }
        "" => refuse(
            messages::INVALID_RESPONSE,
            FailureKind::Transient(Some(300)),
        ),
        other => {
            let mut failure = refuse(messages::API_ERROR, FailureKind::Permanent);
            failure.params = vec![("api_code".to_owned(), other.to_owned())];
            failure
        }
    }
}

/// The refusal an HTTP status stands for when the document explains nothing.
fn from_status(status: u16) -> Failure {
    match status {
        401 | 403 => refuse(messages::ACCOUNT_REQUIRED, FailureKind::AuthRequired),
        404 | 410 => refuse(messages::LIST_NOT_FOUND, FailureKind::Permanent),
        429 => refuse(messages::RATE_LIMITED, FailureKind::RateLimited(Some(3600))),
        500..=599 => refuse(messages::SERVER_ERROR, FailureKind::Transient(Some(300))),
        _ => refuse(messages::INVALID_RESPONSE, FailureKind::Permanent),
    }
}

fn crawl_list(id: &str) -> Result<Vec<CrawledLink>, Failure> {
    let headers = vec![RequestHeader {
        name: "Accept".to_owned(),
        value_template: "application/json".to_owned(),
    }];
    let response = http::http_request("GET", &list::list_url(id), &[], &headers, &[])?;
    let Some(answer) = list::parse(&response.body) else {
        // Not a JSON object at all. The status decides, because there is no document to read.
        return Err(from_status(response.status));
    };
    // The document is read before the status: Pixeldrain's token tells a deleted list from a
    // blocked one, and a bare 404 cannot.
    if let Some(token) = list::refusal_token(&answer) {
        return Err(from_token(&token));
    }
    if response.status < 200 || response.status > 299 {
        return Err(from_status(response.status));
    }
    let children = list::children(&answer);
    if children.is_empty() {
        // An empty answer is not a result: it would create a package with nothing in it and
        // nothing in the interface to say why.
        return Err(refuse(messages::LIST_EMPTY, FailureKind::Permanent));
    }
    Ok(children
        .into_iter()
        .map(|child| CrawledLink {
            url: child.url,
            file_name: child.file_name,
            size: child.size,
            package_hint: child.package_hint,
        })
        .collect())
}

impl Guest for Component {
    /// Reaches nothing: asked of every link a person pastes, answered from the address alone.
    fn claims_url(url: String) -> bool {
        list::list_id(&url).is_some()
    }

    fn crawl(url: String) -> Result<Vec<CrawledLink>, Failure> {
        let Some(id) = list::list_id(&url) else {
            // Defensive: the host asks `claims_url` first. `unsupported` is the refusal the
            // selection walks past, so an address that is not this plugin's carries on.
            return Err(refuse(messages::LIST_NOT_FOUND, FailureKind::Unsupported));
        };
        crawl_list(&id)
    }
}

export!(Component);
