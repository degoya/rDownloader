//! The component: a MediaFire folder address in, the files behind it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own. What keeps that from being unbounded is [`crate::walk`]:
//! depth, breadth, chunks and cycles are refused there, and the manifest's fuel and time
//! budget sit underneath as the last resort rather than as the plan.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use mediafire_common::{
    address,
    api::{self, ApiError, Envelope},
};
use rdownloader::plugin::{
    host,
    http::{self, RequestQuery},
    types::{Failure, FailureKind},
};
use serde_json::Value;

use crate::{
    listing::{self, Entry},
    messages,
    target::{self, Target},
    walk::{Limit, MAX_CHUNKS, Walk},
};

struct Component;

fn refuse((code, message): (&str, &str), category: FailureKind) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

/// One API call; the `response` object on success.
fn call(name: &str, params: &[(&str, &str)]) -> Result<Value, Failure> {
    let query: Vec<RequestQuery> = params
        .iter()
        .chain(&[("response_format", "json")])
        .map(|(name, value)| RequestQuery {
            name: (*name).to_owned(),
            value_template: (*value).to_owned(),
        })
        .collect();
    let response = http::http_request("GET", &address::api_call(name), &query, &[], &[])?;
    match api::envelope(&response.body) {
        Some(Envelope::Success(value)) => Ok(value),
        Some(Envelope::Error(error)) => Err(api_failure(&error)),
        None if (200..300).contains(&response.status) => {
            Err(refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
        }
        None => Err(Failure {
            category: match response.status {
                429 => FailureKind::RateLimited(None),
                500..=599 => FailureKind::Transient(None),
                _ => FailureKind::Permanent,
            },
            message: messages::http_error(response.status),
            code: Some(messages::HTTP_ERROR.to_owned()),
            params: vec![("status".to_owned(), response.status.to_string())],
        }),
    }
}

/// An API error, in the scheduler's categories. An unknown folder key is reported by the API
/// as a missing session token (104), measured 2026-09-21; 112 and 113 are its documented
/// names for the same thing.
fn api_failure(error: &ApiError) -> Failure {
    match error.code {
        api::ERROR_RATE_LIMIT => refuse(messages::RATE_LIMITED, FailureKind::RateLimited(None)),
        api::ERROR_TOKEN_MISSING
        | api::ERROR_TOKEN_INVALID
        | api::ERROR_FOLDERKEY_UNKNOWN
        | api::ERROR_FOLDERKEY_MISSING => {
            refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent)
        }
        api::ERROR_ACCESS_DENIED => refuse(messages::FOLDER_PRIVATE, FailureKind::Permanent),
        // A key list where no key exists: nothing behind the address.
        api::ERROR_QUICKKEY_UNKNOWN | api::ERROR_QUICKKEY_MISSING => {
            refuse(messages::FOLDER_EMPTY, FailureKind::Permanent)
        }
        _ => Failure {
            category: FailureKind::Permanent,
            message: messages::api_error(&error.message),
            code: Some(messages::API_ERROR.to_owned()),
            params: vec![("message".to_owned(), error.message.clone())],
        },
    }
}

/// The folder's own record, or the refusal when the key is no folder. For an undecided key
/// that refusal is `unsupported` — "not mine after all" — and the selection moves on to the
/// resolver.
fn folder_info(key: &str, undecided: bool) -> Result<api::FolderInfo, Failure> {
    let response = match call("folder/get_info", &[("folder_key", key)]) {
        Ok(response) => response,
        Err(failure)
            if undecided && failure.code.as_deref() == Some(messages::FOLDER_UNREACHABLE.0) =>
        {
            return Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported));
        }
        Err(failure) => return Err(failure),
    };
    response
        .get("folder_info")
        .and_then(api::folder_info)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
}

/// Everything in one folder — both content types, every chunk up to the limit — and whether
/// the reading stopped short.
fn read_folder(key: &str) -> Result<(Vec<Entry>, bool), Failure> {
    let mut entries = Vec::new();
    let mut truncated = false;
    for content_type in ["folders", "files"] {
        let mut chunk = 1;
        loop {
            let response = call(
                "folder/get_content",
                &[
                    ("folder_key", key),
                    ("content_type", content_type),
                    ("chunk", &chunk.to_string()),
                ],
            )?;
            let content = api::folder_content(&response)
                .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
            entries.extend(content.folders.iter().filter_map(listing::folder_entry));
            entries.extend(content.files.iter().filter_map(listing::file_entry));
            if !content.more_chunks {
                break;
            }
            if chunk >= MAX_CHUNKS {
                truncated = true;
                break;
            }
            chunk += 1;
        }
    }
    Ok((entries, truncated))
}

/// Walks a folder tree from `key`.
fn crawl_folder(key: &str, undecided: bool) -> Result<Vec<CrawledLink>, Failure> {
    let info = folder_info(key, undecided)?;
    if info.private {
        return Err(refuse(messages::FOLDER_PRIVATE, FailureKind::Permanent));
    }
    let mut walk = Walk::start(key, &info.name);
    while let Some(pending) = walk.next_folder() {
        let (entries, truncated) = read_folder(&pending.key)?;
        walk.absorb(&pending, entries, truncated);
    }
    if let Some(limit) = walk.limit() {
        // Reported rather than silent: somebody who pasted a folder and got 500 of its 900
        // files has to be able to find out which half they are looking at.
        host::log(
            "warn",
            match limit {
                Limit::Depth => "mediafire folder is nested deeper than this crawl walks",
                Limit::Files => "mediafire folder holds more files than this crawl lists",
                Limit::Folders => "mediafire folder holds more subfolders than this crawl reads",
                Limit::Chunks => "mediafire folder lists further than this crawl pages",
            },
        );
    }
    let links: Vec<CrawledLink> = walk
        .into_files()
        .into_iter()
        .map(|found| link(&found.key, &found.name, found.size, Some(found.path)))
        .collect();
    if links.is_empty() {
        // An empty answer is not a result. Handing one back would create a package with
        // nothing in it and nothing in the interface to explain why.
        return Err(refuse(messages::FOLDER_EMPTY, FailureKind::Permanent));
    }
    Ok(links)
}

/// The files behind a `/?key,key` list, through one batched `file/get_info`.
fn crawl_keys(keys: &[String]) -> Result<Vec<CrawledLink>, Failure> {
    let response = call("file/get_info", &[("quick_key", &keys.join(","))])?;
    let links: Vec<CrawledLink> = api::file_infos(&response)
        .iter()
        .filter_map(listing::file_entry)
        .filter_map(|entry| match entry {
            Entry::File { key, name, size } => Some(link(&key, &name, size, None)),
            Entry::Folder { .. } => None,
        })
        .collect();
    if links.is_empty() {
        return Err(refuse(messages::FOLDER_EMPTY, FailureKind::Permanent));
    }
    Ok(links)
}

/// The canonical file address, which the sibling resolver claims.
fn link(key: &str, name: &str, size: Option<u64>, package_hint: Option<String>) -> CrawledLink {
    CrawledLink {
        url: address::file_link(key, Some(name)),
        file_name: Some(name.to_owned()),
        size,
        package_hint: package_hint.filter(|path| !path.is_empty()),
    }
}

impl Guest for Component {
    /// Reaches nothing: asked of every link a person pastes, answered from the address alone.
    fn claims_url(url: String) -> bool {
        target::claim(&url).is_some()
    }

    fn crawl(url: String) -> Result<Vec<CrawledLink>, Failure> {
        match target::claim(&url) {
            None => Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported)),
            Some(Target::Folder { key }) => crawl_folder(&key, false),
            Some(Target::Undecided { key }) => crawl_folder(&key, true),
            Some(Target::Keys(keys)) => crawl_keys(&keys),
        }
    }
}

export!(Component);
