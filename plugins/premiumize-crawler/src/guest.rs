//! The component: a Premiumize folder address in, the files behind it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own — there is no way to hand a half-finished walk back and
//! be asked again. What keeps that from being unbounded is [`crate::walk`]: depth, breadth
//! and cycles are refused there, and the manifest's fuel and time budget sit underneath as
//! the last resort rather than as the plan.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{
    listing::{self, Entry},
    messages,
    target::{self, Kind},
    walk::{Limit, Walk},
};

const API: &str = "https://www.premiumize.me/api";
/// The vault reference the Premiumize provider keeps its API key under. The value never
/// reaches this plugin: the host substitutes it into `{{secret:...}}` on the way out, and
/// only towards the hosts the provider declared for that reference.
const API_KEY_REFERENCE: &str = "premiumize_api_key";

struct Component;

fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

fn headers() -> Vec<RequestHeader> {
    vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: format!("Bearer {{{{secret:{API_KEY_REFERENCE}}}}}"),
        },
        RequestHeader {
            name: "Accept".to_owned(),
            value_template: "application/json".to_owned(),
        },
    ]
}

/// Fetches one API document, turning every status that is not an answer into one refusal.
///
/// A crawl makes many requests, so the vocabulary stays small: the caller gets bytes or a
/// failure and never decides a second time what a status code means.
fn fetch(path: &str, id: &str) -> Result<Vec<u8>, Failure> {
    let response = http::http_request(
        "GET",
        &format!("{API}{path}"),
        &[RequestQuery {
            name: "id".to_owned(),
            value_template: id.to_owned(),
        }],
        &headers(),
        &[],
    )?;
    match response.status {
        200..=299 => Ok(response.body),
        401 | 403 => Err(refuse(
            messages::API_KEY_REQUIRED,
            FailureKind::AuthRequired,
        )),
        429 => Err(refuse(
            messages::RATE_LIMITED,
            FailureKind::RateLimited(None),
        )),
        500..=599 => Err(refuse(
            messages::FOLDER_UNREACHABLE,
            FailureKind::Transient(None),
        )),
        _ => Err(refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent)),
    }
}

/// Premiumize answers `status: "error"` with HTTP 200, so the document decides as well.
fn ensure_success(status: &str) -> Result<(), Failure> {
    if status == "success" {
        return Ok(());
    }
    Err(refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent))
}

/// Reduces a folder name to something that can stand as a package suggestion.
fn package_name(name: Option<&str>) -> String {
    name.unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\'))
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(120)
        .collect()
}

/// Lists one cloud item, for an address naming a file rather than a folder.
fn crawl_item(id: &str) -> Result<Vec<CrawledLink>, Failure> {
    let body = fetch("/item/details", id)?;
    let details = listing::item(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    ensure_success(&details.status)?;
    let Some(Entry::File { name, url, size }) = details.item.entry() else {
        return Err(refuse(messages::FOLDER_EMPTY, FailureKind::Permanent));
    };
    Ok(vec![CrawledLink {
        url,
        file_name: Some(name).filter(|name| !name.is_empty()),
        size,
        package_hint: None,
    }])
}

/// Walks a cloud folder, breadth first, under the limits in `crate::walk`.
fn crawl_folder(id: &str) -> Result<Vec<CrawledLink>, Failure> {
    let mut walk = Walk::start(id);
    while let Some(mut pending) = walk.next_folder() {
        let body = fetch("/folder/list", &pending.id)?;
        let folder = listing::folder(&body)
            .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
        ensure_success(&folder.status)?;
        // The crawled folder's own name becomes the root of every path below it, which is
        // what turns "the folder I pasted" into the package the person expects.
        if pending.depth == 0 {
            pending.path = package_name(folder.name.as_deref());
        }
        let entries = folder
            .content
            .into_iter()
            .filter_map(listing::Item::entry)
            .collect();
        walk.absorb(&pending, entries);
    }
    if let Some(limit) = walk.limit() {
        // Reported rather than silent: somebody who pasted a folder and got 500 of its 900
        // files has to be able to find out which half they are looking at.
        host::log(
            "warn",
            match limit {
                Limit::Depth => "premiumize folder is nested deeper than this crawl walks",
                Limit::Files => "premiumize folder holds more files than this crawl lists",
                Limit::Folders => "premiumize folder holds more subfolders than this crawl reads",
            },
        );
    }
    let files = walk.into_files();
    if files.is_empty() {
        // An empty answer is not a result. Handing one back would create a package with
        // nothing in it and nothing in the interface to explain why.
        return Err(refuse(messages::FOLDER_EMPTY, FailureKind::Permanent));
    }
    Ok(files
        .into_iter()
        .map(|found| CrawledLink {
            url: found.url,
            file_name: Some(found.name).filter(|name| !name.is_empty()),
            size: found.size,
            package_hint: Some(found.path).filter(|path| !path.is_empty()),
        })
        .collect())
}

impl Guest for Component {
    /// Reaches nothing: asked of every link a person pastes, answered from the address alone.
    fn claims_url(url: String) -> bool {
        target::claim(&url).is_some()
    }

    fn crawl(url: String) -> Result<Vec<CrawledLink>, Failure> {
        let Some(claimed) = target::claim(&url) else {
            return Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported));
        };
        match claimed.kind {
            Kind::Item => crawl_item(&claimed.id),
            Kind::Folder => crawl_folder(&claimed.id),
        }
    }
}

export!(Component);
