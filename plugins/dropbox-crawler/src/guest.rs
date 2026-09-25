//! The component: a Dropbox folder address in, the files behind it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own — there is no way to hand a half-finished walk back and be
//! asked again. What keeps that from being unbounded is [`crate::walk`]: depth, breadth, pages
//! and cycles are refused there, and the manifest's fuel and time budget sit underneath as the
//! last resort rather than as the plan. The cursor Dropbox hands back between two pages is
//! part of that walk's state, never a local here.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use dropbox_common::{address, metadata, reason};
use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};
use serde_json::{Value, json};

use crate::{
    listing::{self, Entry},
    messages,
    target::{self, Target},
    walk::{Limit, Pending, Walk},
};

const API: &str = "https://api.dropboxapi.com/2";
/// The vault reference the Dropbox provider keeps its access token under. The value never
/// reaches this plugin: the host substitutes it into `{{secret:…}}` on the way out, and only
/// towards the hosts the provider declared for that reference.
const TOKEN_REFERENCE: &str = "dropbox_access_token";
/// Dropbox's own maximum for one page.
const PAGE_LIMIT: u32 = 2000;
/// The name the root of an account's own Dropbox goes by: it has no metadata of its own.
const ROOT_NAME: &str = "Dropbox";

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
            name: "Content-Type".to_owned(),
            value_template: "application/json".to_owned(),
        },
    ]
}

/// The `Retry-After` Dropbox sent, or the wait inside its document.
fn retry_after(headers: &[(String, String)], body: &[u8]) -> Option<u64> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
        .or_else(|| reason::retry_after_in(body))
}

/// Calls one RPC endpoint, turning every status that is not an answer into one refusal.
///
/// A crawl makes many requests, so the vocabulary stays small: the caller gets bytes or a
/// failure and never decides a second time what a status code meant. The reason Dropbox named
/// is sanitised before it is looked at, so an error document that quoted a token publishes
/// nothing.
fn call(endpoint: &str, argument: &Value) -> Result<Vec<u8>, Failure> {
    let response = http::http_request(
        "POST",
        &format!("{API}/{endpoint}"),
        &[],
        &headers(),
        argument.to_string().as_bytes(),
    )?;
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let named = reason::of(&response.body);
    Err(
        match (response.status, named.as_deref().unwrap_or_default()) {
            (401, _) | (_, "invalid_access_token" | "expired_access_token" | "missing_scope") => {
                refuse(messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
            }
            // A crawl is one invocation, so a rate limit ends it as a wait of its own; there
            // is no queue of Dropbox links here to hold back, and nothing else is touched.
            (429, _) | (_, "too_many_requests") => refuse(
                messages::RATE_LIMITED,
                FailureKind::RateLimited(retry_after(&response.headers, &response.body)),
            ),
            (_, "shared_link_access_denied") => {
                refuse(messages::LINK_ACCESS_DENIED, FailureKind::Permanent)
            }
            (500..=599, _) => refuse(messages::FOLDER_UNREACHABLE, FailureKind::Transient(None)),
            _ => refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent),
        },
    )
}

/// The argument every shared-link call carries.
fn shared_link(link: &str, password: Option<&str>) -> Value {
    let mut argument = json!({ "url": link });
    if let Some(password) = password {
        argument["password"] = Value::String(password.to_owned());
    }
    argument
}

/// The crawled folder's own name, which becomes the root of every path below it.
fn folder_name(target: &Target) -> Result<String, Failure> {
    let item = match target {
        // The root has no metadata of its own.
        Target::Own { path } if path.is_empty() => return Ok(ROOT_NAME.to_owned()),
        Target::Own { path } => call("files/get_metadata", &json!({ "path": path }))?,
        Target::Shared {
            link,
            sub_path,
            password,
        } => {
            let mut argument = json!({ "url": link });
            if !sub_path.is_empty() {
                argument["path"] = Value::String(sub_path.clone());
            }
            if let Some(password) = password {
                argument["link_password"] = Value::String(password.clone());
            }
            call("sharing/get_shared_link_metadata", &argument)?
        }
    };
    let item = metadata::item(&item)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    if item.is_file() {
        // The sibling resolver's address, pasted at the crawler.
        return Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported));
    }
    if !item.is_folder() {
        return Err(refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent));
    }
    // The name is a stranger's, so it goes through the same guard a child's name does before it
    // becomes the first segment of every package hint.
    Ok(crate::walk::join("", item.name()))
}

/// Reads one page of one folder: the first from its path, every later one from the cursor the
/// walk kept for it.
fn list(target: &Target, pending: &Pending) -> Result<metadata::Listing, Failure> {
    let body = match &pending.cursor {
        Some(cursor) => call("files/list_folder/continue", &json!({ "cursor": cursor }))?,
        None => {
            let mut argument = json!({ "path": pending.api_path, "limit": PAGE_LIMIT });
            if let Target::Shared { link, password, .. } = target {
                // Inside a shared folder link the path is relative to the link's root, and the
                // password travels in the link argument.
                argument["shared_link"] = shared_link(link, password.as_deref());
            }
            call("files/list_folder", &argument)?
        }
    };
    metadata::listing(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))
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
        let root_name = folder_name(&claimed)?;
        let root_path = match &claimed {
            Target::Own { path } => path.clone(),
            Target::Shared { sub_path, .. } => sub_path.clone(),
        };
        let mut walk = Walk::start(&root_path);
        while let Some(mut pending) = walk.next_folder() {
            if pending.depth == 0 {
                pending.path = root_name.clone();
            }
            let page = list(&claimed, &pending)?;
            let entries: Vec<Entry> = page.entries.iter().filter_map(listing::entry).collect();
            walk.absorb(&pending, entries);
            if page.has_more
                && let Some(cursor) = page.cursor.filter(|cursor| !cursor.is_empty())
            {
                // The cursor goes into the walk, not into a local: the folder comes back as the
                // next thing to read, carrying it.
                walk.continue_folder(&pending, cursor);
            }
        }
        if let Some(limit) = walk.limit() {
            // Reported rather than silent: somebody who pasted a folder and got 500 of its 900
            // files has to be able to find out which half they are looking at.
            host::log(
                "warn",
                match limit {
                    Limit::Depth => "dropbox folder is nested deeper than this crawl walks",
                    Limit::Files => "dropbox folder holds more files than this crawl lists",
                    Limit::Folders => "dropbox folder holds more subfolders than this crawl reads",
                    Limit::Pages => "dropbox folder lists further than this crawl pages",
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
                // between the two packages — the password rides in the address so a crawled
                // file resolves the same way its folder was listed.
                url: match &claimed {
                    Target::Own { .. } => address::file_address(&found.folder, &found.name),
                    Target::Shared { link, password, .. } => address::shared_file_address(
                        link,
                        &found.folder,
                        &found.name,
                        password.as_deref(),
                    ),
                },
                file_name: Some(found.name).filter(|name| !name.is_empty()),
                size: found.size,
                package_hint: Some(found.path).filter(|path| !path.is_empty()),
            })
            .collect())
    }
}

export!(Component);
