//! The component: a Box folder address or shared link in, the files behind it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own — there is no way to hand a half-finished walk back and be
//! asked again. What keeps that from being unbounded is [`crate::walk`]: depth, breadth, pages
//! and cycles are refused there, and the manifest's fuel and time budget sit underneath as the
//! last resort rather than as the plan.
//!
//! A shared link's password never leaves this plugin except inside the `boxapi` header, which
//! is the one place Box's API takes it. It is not in an address, not in a query parameter and
//! not in anything that is logged.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use box_common::{address, reason};
use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader, RequestQuery},
    types::{Failure, FailureKind},
};

use crate::{
    listing::{self, Entry},
    messages,
    target::{self, Target},
    walk::{Limit, MAX_PAGES, Walk},
};

/// The vault reference the Box provider keeps its access token under. The value never reaches
/// this plugin: the host substitutes it into `{{secret:…}}` on the way out, and only towards
/// the hosts the provider declared for that reference.
const TOKEN_REFERENCE: &str = "box_access_token";
/// The fields one listing needs. `fields` replaces Box's standard set rather than adding to it,
/// so an unnamed field is one that does not travel.
const LIST_FIELDS: &str = "type,id,name,size,item_status";
/// Box's own maximum for one page of items.
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

/// The headers every request of one crawl carries: the token marker, and the shared link the
/// folder is reached through when it is reached through one.
fn headers(claimed: &Target) -> Vec<RequestHeader> {
    let mut headers = vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: format!("Bearer {{{{secret:{TOKEN_REFERENCE}}}}}"),
        },
        RequestHeader {
            name: "Accept".to_owned(),
            value_template: "application/json".to_owned(),
        },
    ];
    if let Some(value) = claimed.box_api() {
        headers.push(RequestHeader {
            name: "boxapi".to_owned(),
            value_template: value,
        });
    }
    headers
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
/// failure and never decides a second time what a status code meant. The code Box named is
/// sanitised before it is looked at, so an error document that quoted a token publishes nothing,
/// and nothing Box wrote reaches a message.
fn fetch(claimed: &Target, url: &str, parameters: &[RequestQuery]) -> Result<Vec<u8>, Failure> {
    let response = http::http_request("GET", url, parameters, &headers(claimed), &[])?;
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let named = reason::of(&response.body);
    let shared = claimed.link().is_some();
    Err(
        match (response.status, named.as_deref().unwrap_or_default()) {
            (401, _) | (_, "unauthorized" | "invalid_token") => {
                refuse(messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
            }
            (429, _) | (_, "rate_limit_exceeded") => {
                let seconds = response
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, value)| value.trim().parse::<u64>().ok());
                refuse(messages::RATE_LIMITED, FailureKind::RateLimited(seconds))
            }
            // Box answers a wrong or missing shared-link password with the same refusal it uses
            // for a folder somebody may not read, deliberately, so that a link cannot be probed
            // for whether its password is the only thing in the way. Through a link that is
            // said as an access refusal, which is the one a person can act on. A 401 is not in
            // this arm: that one is about the account's token and is answered above.
            (403 | 404, _) if shared => refuse(messages::ACCESS_DENIED, FailureKind::Permanent),
            (403, _) | (_, "forbidden" | "access_denied_insufficient_permissions") => {
                refuse(messages::ACCESS_DENIED, FailureKind::Permanent)
            }
            (500..=599, _) | (_, "internal_server_error" | "unavailable") => {
                refuse(messages::FOLDER_UNREACHABLE, FailureKind::Transient(None))
            }
            _ => refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent),
        },
    )
}

/// What the crawled address points at: a folder to walk, or the one file it turned out to be.
enum Root {
    Folder { id: String, name: String },
    File(CrawledLink),
}

/// Asks Box what the crawled address is, and what it is called.
///
/// A shared link is the one address Box does not spell the kind of, so it goes to
/// `/2.0/shared_items` — the endpoint whose whole job is to say what a link points at. A folder
/// address says so already and is read at `/2.0/folders/<id>`, which is also where a folder
/// inside a shared link is read, through that link.
fn root(claimed: &Target) -> Result<Root, Failure> {
    let body = match claimed.start_id() {
        Some(id) => fetch(
            claimed,
            &format!("{}/folders/{id}", address::API),
            &[query("fields", LIST_FIELDS)],
        )?,
        None => fetch(
            claimed,
            &format!("{}/shared_items", address::API),
            &[query("fields", LIST_FIELDS)],
        )?,
    };
    let item = listing::item(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    if item.is_gone() {
        return Err(refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent));
    }
    let id = item
        .id
        .clone()
        .filter(|id| address::valid_id(id))
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    // The name is a stranger's, so it goes through the same guard a child's name does before it
    // becomes the first segment of every package hint.
    let name = crate::walk::join("", item.name.as_deref().unwrap_or_default());
    if item.is_folder() {
        return Ok(Root::Folder { id, name });
    }
    // A shared link does not say what it points at, and this one pointed at a file: one file is
    // the answer. A link that *said* it was a folder and is not one is refused by name instead.
    if claimed.start_id().is_none() && item.is_file() {
        return Ok(Root::File(CrawledLink {
            url: claimed.found_address(&id),
            file_name: Some(name).filter(|name| !name.is_empty()),
            size: item.size.as_ref().and_then(listing::Flexible::as_u64),
            package_hint: None,
        }));
    }
    Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported))
}

/// Reads every page of one folder's listing, up to the cap.
///
/// Box paginates by offset and says how many rows the folder holds, so the next page is asked
/// for by number rather than by following an address it handed back — there is no opaque
/// continuation token to be pointed anywhere by an answer.
fn absorb_folder(
    walk: &mut Walk,
    claimed: &Target,
    pending: &crate::walk::Pending,
) -> Result<(), Failure> {
    let url = format!("{}/folders/{}/items", address::API, pending.id);
    let mut offset: u64 = 0;
    for page_number in 0..MAX_PAGES {
        let body = fetch(
            claimed,
            &url,
            &[
                query("fields", LIST_FIELDS),
                query("limit", PAGE_SIZE),
                query("offset", &offset.to_string()),
            ],
        )?;
        let page = listing::page(&body)
            .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
        let next = page.next_offset();
        let entries: Vec<Entry> = page
            .entries
            .into_iter()
            .filter_map(listing::Item::entry)
            .collect();
        walk.absorb(pending, entries);
        match next {
            // A full walk stops paging: the files past the limit are not going to be handed
            // back, so fetching them costs requests for nothing.
            Some(_) if walk.is_full() => return Ok(()),
            Some(next) if page_number + 1 < MAX_PAGES => offset = next,
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
        let (root_id, root_name) = match root(&claimed)? {
            Root::File(link) => return Ok(vec![link]),
            Root::Folder { id, name } => (id, name),
        };
        let mut walk = Walk::start(&root_id);
        while let Some(mut pending) = walk.next_folder() {
            if pending.depth == 0 {
                pending.path = root_name.clone();
            }
            absorb_folder(&mut walk, &claimed, &pending)?;
        }
        if let Some(limit) = walk.limit() {
            // Reported rather than silent: somebody who pasted a folder and got 500 of its 900
            // files has to be able to find out which half they are looking at.
            host::log(
                "warn",
                match limit {
                    Limit::Depth => "box folder is nested deeper than this crawl walks",
                    Limit::Files => "box folder holds more files than this crawl lists",
                    Limit::Folders => "box folder holds more subfolders than this crawl reads",
                    Limit::Pages => "box folder lists further than this crawl pages",
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
                // The canonical address, which the sibling resolver claims. A file found behind
                // a shared link carries that link, because the link grants the access and the
                // item's own id does not. Nothing else passes between the two packages — and
                // never the password, which lives in the `boxapi` header alone.
                url: claimed.found_address(&found.id),
                file_name: Some(found.name).filter(|name| !name.is_empty()),
                size: found.size,
                package_hint: Some(found.path).filter(|path| !path.is_empty()),
            })
            .collect())
    }
}

export!(Component);
