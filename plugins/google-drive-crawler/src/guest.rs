//! The component: a Google Drive folder address in, the files behind it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own — there is no way to hand a half-finished walk back and be
//! asked again. What keeps that from being unbounded is [`crate::walk`]: depth, breadth, pages
//! and cycles are refused there, and the manifest's fuel and time budget sit underneath as the
//! last resort rather than as the plan.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use google_drive_common::{address, reason};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{
    listing::{self, Entry},
    messages, target,
    walk::{Limit, MAX_PAGES, Walk},
};

const API: &str = "https://www.googleapis.com/drive/v3";
/// The vault reference the Google Drive provider keeps its access token under. The value never
/// reaches this plugin: the host substitutes it into `{{secret:…}}` on the way out, and only
/// towards the hosts the provider declared for that reference.
const TOKEN_REFERENCE: &str = "google_drive_access_token";
/// The fields one listing needs. Not an optimisation: Drive answers `files.list` without this
/// with rows that have no `mimeType` and no size, so every entry would look like a file of
/// unknown length.
const LIST_FIELDS: &str = "nextPageToken,files(id,name,mimeType,size,trashed)";
/// Drive's own maximum for one page.
const PAGE_SIZE: &str = "1000";

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
            value_template: format!("Bearer {{{{secret:{TOKEN_REFERENCE}}}}}"),
        },
        RequestHeader {
            name: "Accept".to_owned(),
            value_template: "application/json".to_owned(),
        },
    ]
}

fn query(name: &str, value: &str) -> RequestQuery {
    RequestQuery {
        name: name.to_owned(),
        value_template: value.to_owned(),
    }
}

/// Fetches one API document, turning every status that is not an answer into one refusal.
///
/// A crawl makes many requests, so the vocabulary stays small: the caller gets bytes or a
/// failure and never decides a second time what a status code meant. The reason Google named is
/// sanitised before it is looked at, so an error document that quoted a token publishes nothing.
fn fetch(url: &str, parameters: &[RequestQuery]) -> Result<Vec<u8>, Failure> {
    let response = http::http_request("GET", url, parameters, &headers(), &[])?;
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let named = reason::of(&response.body);
    Err(
        match (response.status, named.as_deref().unwrap_or_default()) {
            (401, _) | (_, "authError" | "unauthorized") => {
                refuse(messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
            }
            (429, _)
            | (_, "rateLimitExceeded" | "userRateLimitExceeded" | "dailyLimitExceeded") => {
                let seconds = response
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, value)| value.trim().parse::<u64>().ok());
                refuse(messages::RATE_LIMITED, FailureKind::RateLimited(seconds))
            }
            (500..=599, _) => refuse(messages::FOLDER_UNREACHABLE, FailureKind::Transient(None)),
            _ => refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent),
        },
    )
}

/// The crawled folder's own name, which becomes the root of every path below it.
fn folder_name(id: &str) -> Result<String, Failure> {
    let body = fetch(
        &format!("{API}/files/{id}"),
        &[
            query("fields", "id,name,mimeType,trashed"),
            query("supportsAllDrives", "true"),
        ],
    )?;
    let item = listing::item(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    if item.trashed == Some(true) {
        return Err(refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent));
    }
    // The name is a stranger's, so it goes through the same guard a child's name does before it
    // becomes the first segment of every package hint.
    Ok(crate::walk::join(
        "",
        item.name.as_deref().unwrap_or_default(),
    ))
}

/// Reads every page of one folder's listing, up to the cap.
fn absorb_folder(walk: &mut Walk, pending: &crate::walk::Pending) -> Result<(), Failure> {
    let mut token: Option<String> = None;
    for page_number in 0..MAX_PAGES {
        let mut parameters = vec![
            query(
                "q",
                &format!("'{}' in parents and trashed = false", pending.id),
            ),
            query("fields", LIST_FIELDS),
            query("pageSize", PAGE_SIZE),
            // A shared drive is reached only when all three of these are set; without them
            // Drive answers a shared-drive folder as if it were empty.
            query("supportsAllDrives", "true"),
            query("includeItemsFromAllDrives", "true"),
            query("corpora", "allDrives"),
        ];
        if let Some(token) = &token {
            parameters.push(query("pageToken", token));
        }
        let body = fetch(&format!("{API}/files"), &parameters)?;
        let page = listing::page(&body)
            .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
        let entries: Vec<Entry> = page
            .files
            .into_iter()
            .filter_map(listing::Item::entry)
            .collect();
        walk.absorb(pending, entries);
        match page.next_page_token.filter(|value| !value.is_empty()) {
            // A full walk stops paging: the files past the limit are not going to be handed
            // back, so fetching them costs requests for nothing.
            Some(_) if walk.is_full() => return Ok(()),
            Some(next) if page_number + 1 < MAX_PAGES => token = Some(next),
            Some(_) => {
                walk.note(Limit::Pages);
                return Ok(());
            }
            None => return Ok(()),
        }
    }
    Ok(())
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
        let root = folder_name(&claimed.id)?;
        let mut walk = Walk::start(&claimed.id);
        while let Some(mut pending) = walk.next_folder() {
            if pending.depth == 0 {
                pending.path = root.clone();
            }
            absorb_folder(&mut walk, &pending)?;
        }
        if let Some(limit) = walk.limit() {
            // Reported rather than silent: somebody who pasted a folder and got 500 of its 900
            // files has to be able to find out which half they are looking at.
            host::log(
                "warn",
                match limit {
                    Limit::Depth => "google drive folder is nested deeper than this crawl walks",
                    Limit::Files => "google drive folder holds more files than this crawl lists",
                    Limit::Folders => {
                        "google drive folder holds more subfolders than this crawl reads"
                    }
                    Limit::Pages => "google drive folder lists further than this crawl pages",
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
                // The canonical address, which the sibling resolver claims. Nothing else passes
                // between the two packages.
                url: address::file_address(&found.id),
                file_name: Some(found.name).filter(|name| !name.is_empty()),
                size: found.size,
                package_hint: Some(found.path).filter(|path| !path.is_empty()),
            })
            .collect())
    }
}

export!(Component);
