//! The component: an address that ends in a slash in, the files under it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own. What keeps that from being unbounded is [`crate::walk`]:
//! depth, breadth and repeats are refused there, and the manifest's fuel and time budget sit
//! underneath as the last resort rather than as the plan.
//!
//! This plugin reaches exactly one host — the one the pasted address names. Its manifest says
//! `*` because an open listing is any web server at all; the host narrows that to the crawled
//! address for the duration of the call, and a redirect off it is refused (RD-107-05).
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};

use crate::{
    listing, messages,
    target::{self, Address},
    walk::{Limit, Walk},
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

/// Says "this address is not mine after all", the one refusal the selection walks past.
///
/// A crawler that claims by the shape of an address is sometimes wrong, and before RD-107-05
/// being wrong once ended the link. `unsupported` is how it hands the address on.
fn not_mine(message: (&str, &str)) -> Failure {
    refuse(message, FailureKind::Unsupported)
}

/// A browser-shaped `Accept`, because a server that content-negotiates will otherwise answer
/// a directory request with something that is not the listing.
fn headers() -> Vec<RequestHeader> {
    vec![RequestHeader {
        name: "Accept".to_owned(),
        value_template: "text/html,application/xhtml+xml".to_owned(),
    }]
}

/// Fetches one listing page, turning every status that is not a page into one refusal.
fn fetch(url: &str) -> Result<String, Failure> {
    let response = http::http_request("GET", url, &[], &headers(), &[])?;
    match response.status {
        200..=299 => Ok(String::from_utf8_lossy(&response.body).into_owned()),
        401 | 403 => Err(refuse(
            messages::SIGN_IN_REQUIRED,
            FailureKind::AuthRequired,
        )),
        // Not there is not "empty": it is an address that never was one.
        404 | 410 => Err(not_mine(messages::NOT_A_LISTING)),
        429 => Err(refuse(
            messages::SERVER_BUSY,
            FailureKind::RateLimited(None),
        )),
        500..=599 => Err(refuse(
            messages::DIRECTORY_UNREACHABLE,
            FailureKind::Transient(None),
        )),
        _ => Err(refuse(
            messages::DIRECTORY_UNREACHABLE,
            FailureKind::Permanent,
        )),
    }
}

/// Walks the tree under the crawled address, breadth first, under the limits in `crate::walk`.
fn crawl_directory(root: &Address) -> Result<Vec<CrawledLink>, Failure> {
    let root_url = root.to_url();
    let mut walk = Walk::start(&root_url, &target::directory_name(&root.path));
    let mut first = true;
    while let Some(pending) = walk.next_directory() {
        let page = match fetch(&pending.url) {
            Ok(page) => page,
            // Only the address a person actually pasted gets to end the crawl. A directory
            // three levels down that answers 403 is a hole in the tree, not a failure of
            // the whole listing, and dropping it is better than losing the other 400 files.
            Err(failure) if first => return Err(failure),
            Err(_) => {
                host::log("info", "a subdirectory of this listing could not be read");
                continue;
            }
        };
        if first {
            // The recogniser runs on the crawled address only. Below it the shape is already
            // established, and a server that renders one subdirectory differently should not
            // cost the whole crawl.
            if !listing::is_index(&page) {
                return Err(not_mine(messages::NOT_A_LISTING));
            }
            first = false;
        }
        let Some(address) = target::parse(&pending.url) else {
            continue;
        };
        walk.absorb(&pending, listing::entries(&address, &page));
    }
    if let Some(limit) = walk.limit() {
        // Reported rather than silent: somebody who pasted a directory and got 500 of its 900
        // files has to be able to find out which half they are looking at.
        host::log(
            "warn",
            match limit {
                Limit::Depth => "this listing is nested deeper than the crawl walks",
                Limit::Files => "this listing holds more files than the crawl lists",
                Limit::Directories => "this listing holds more directories than the crawl reads",
            },
        );
    }
    let files = walk.into_files();
    if files.is_empty() {
        // An empty answer is not a result. Handing one back would create a package with
        // nothing in it and nothing in the interface to explain why.
        return Err(refuse(messages::DIRECTORY_EMPTY, FailureKind::Permanent));
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
        let Some(root) = target::claim(&url) else {
            return Err(not_mine(messages::NOT_A_DIRECTORY));
        };
        crawl_directory(&root)
    }
}

export!(Component);
