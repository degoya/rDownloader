//! The component: a OneDrive or SharePoint folder link in, the files behind it out.
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
use onedrive_common::{
    address::{self, GRAPH, LinkKind},
    reason,
};
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

/// The vault reference the OneDrive provider keeps its access token under. The value never
/// reaches this plugin: the host substitutes it into `{{secret:…}}` on the way out, and only
/// towards the hosts the provider declared for that reference.
const TOKEN_REFERENCE: &str = "onedrive_access_token";
/// The fields one listing needs. The three facets are what say what an entry *is*.
const LIST_FIELDS: &str = "id,name,size,file,folder,package,deleted";
/// Graph's own maximum for one page of children.
const PAGE_SIZE: &str = "200";

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
/// failure and never decides a second time what a status code meant. The code Graph named is
/// sanitised before it is looked at, so an error document that quoted a token publishes
/// nothing.
fn fetch(url: &str, parameters: &[RequestQuery]) -> Result<Vec<u8>, Failure> {
    let response = http::http_request("GET", url, parameters, &headers(), &[])?;
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let named = reason::of(&response.body);
    Err(
        match (response.status, named.as_deref().unwrap_or_default()) {
            (401, _) | (_, "unauthenticated" | "InvalidAuthenticationToken") => {
                refuse(messages::SIGN_IN_REQUIRED, FailureKind::AuthRequired)
            }
            (_, "accessDenied") | (403, "") => {
                refuse(messages::ACCESS_DENIED, FailureKind::Permanent)
            }
            (429, _) | (_, "activityLimitReached" | "tooManyRequests") => {
                let seconds = response
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, value)| value.trim().parse::<u64>().ok());
                refuse(messages::RATE_LIMITED, FailureKind::RateLimited(seconds))
            }
            (500..=599, _) | (_, "serviceNotAvailable") => {
                refuse(messages::FOLDER_UNREACHABLE, FailureKind::Transient(None))
            }
            _ => refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent),
        },
    )
}

/// The Graph route of one folder in the share: the shared root by the share alone, anything
/// below it by its item id inside that share.
fn route(share_id: &str, item_id: &str) -> String {
    if item_id.is_empty() {
        address::root_address(share_id)
    } else {
        address::item_address(share_id, item_id)
    }
}

/// Reads every page of one folder's listing, up to the cap.
fn absorb_folder(
    walk: &mut Walk,
    share_id: &str,
    pending: &crate::walk::Pending,
) -> Result<(), Failure> {
    let first = format!("{}/children", route(share_id, &pending.id));
    let mut next: Option<String> = None;
    for page_number in 0..MAX_PAGES {
        let body = match &next {
            // Graph's next-page address is followed as given: the `$skiptoken` in it is
            // opaque. But only while it is still Graph's own address — an answer that pointed
            // the walk anywhere else would be refused by the sandbox anyway, and is refused
            // here first so the refusal has a name.
            Some(link) if link.starts_with(&format!("{GRAPH}/")) => fetch(link, &[])?,
            Some(_) => return Err(refuse(messages::INVALID_RESPONSE, FailureKind::Permanent)),
            None => fetch(
                &first,
                &[query("$select", LIST_FIELDS), query("$top", PAGE_SIZE)],
            )?,
        };
        let page = listing::page(&body)
            .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
        let entries: Vec<Entry> = page
            .value
            .into_iter()
            .filter_map(listing::Item::entry)
            .collect();
        walk.absorb(pending, entries);
        match page.next_link.filter(|value| !value.is_empty()) {
            // A full walk stops paging: the files past the limit are not going to be handed
            // back, so fetching them costs requests for nothing.
            Some(_) if walk.is_full() => return Ok(()),
            Some(link) if page_number + 1 < MAX_PAGES => next = Some(link),
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
        // What the link points at. The name becomes the root of every package hint; the
        // facet decides whether there is anything to walk.
        let body = fetch(
            &address::root_address(&claimed.share_id),
            &[query("$select", LIST_FIELDS)],
        )?;
        let root = listing::item(&body)
            .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
        if root.is_deleted() {
            return Err(refuse(messages::FOLDER_UNREACHABLE, FailureKind::Permanent));
        }
        let root_id = root.id.clone().unwrap_or_default();
        // The name is a stranger's, so it goes through the same guard a child's name does
        // before it becomes the first segment of every package hint.
        let root_name = crate::walk::join("", root.name.as_deref().unwrap_or_default());
        if !root.is_folder() {
            // The long personal address does not say what it points at, and this one pointed
            // at a file: one file is the answer. A link that *said* it was a folder and is not
            // one is refused by name instead.
            if claimed.kind == LinkKind::Unknown && root.is_file() {
                return Ok(vec![CrawledLink {
                    url: address::root_address(&claimed.share_id),
                    file_name: Some(root_name).filter(|name| !name.is_empty()),
                    size: root.size.as_ref().and_then(listing::Flexible::as_u64),
                    package_hint: None,
                }]);
            }
            return Err(refuse(messages::NOT_A_FOLDER, FailureKind::Unsupported));
        }
        let mut walk = Walk::start(&root_id);
        while let Some(mut pending) = walk.next_folder() {
            if pending.depth == 0 {
                pending.path = root_name.clone();
            }
            absorb_folder(&mut walk, &claimed.share_id, &pending)?;
        }
        if let Some(limit) = walk.limit() {
            // Reported rather than silent: somebody who pasted a folder and got 500 of its 900
            // files has to be able to find out which half they are looking at.
            host::log(
                "warn",
                match limit {
                    Limit::Depth => "onedrive folder is nested deeper than this crawl walks",
                    Limit::Files => "onedrive folder holds more files than this crawl lists",
                    Limit::Folders => "onedrive folder holds more subfolders than this crawl reads",
                    Limit::Pages => "onedrive folder lists further than this crawl pages",
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
                // The canonical address, which the sibling resolver claims. It carries the
                // share the file was found through, because a link shared with the account
                // grants access through that share and not by the item's own id. Nothing else
                // passes between the two packages.
                url: address::item_address(&claimed.share_id, &found.id),
                file_name: Some(found.name).filter(|name| !name.is_empty()),
                size: found.size,
                package_hint: Some(found.path).filter(|path| !path.is_empty()),
            })
            .collect())
    }
}

export!(Component);
