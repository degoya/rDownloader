//! The component: one address in, the files behind it out.
//!
//! Replace the three constants below with the provider's own host and endpoints. Everything
//! else is the shape of a walk rather than the provider, and changes far less often.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "crawler-plugin",
});

use exports::rdownloader::plugin::crawler::{CrawledLink, Guest};
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader},
    types::{Failure, FailureKind},
};

use crate::{
    listing,
    target::{self, Kind},
    walk::{Limit, Walk},
};

/// The provider's host. `claims-url` accepts this host and its subdomains and nothing else.
const HOST: &str = "api.example.com";
/// Lists one folder. The identifier is appended; it is checked before it gets here.
const FOLDER_ENDPOINT: &str = "https://api.example.com/api/folder/list?id=";
/// Looks one item up, for an address that names a file rather than a folder.
const ITEM_ENDPOINT: &str = "https://api.example.com/api/item/details?id=";
/// The vault reference this plugin's account keeps its credential under. The value never
/// reaches the guest: it is substituted into `{{secret:<reference>}}` on the way out, and
/// only towards the hosts that reference is allowed to be sent to.
const SECRET_REFERENCE: &str = "example_api_key";

struct Component;

/// A failure carrying a stable translation code and nothing a provider wrote.
fn refuse(code: &str, message: String, category: FailureKind) -> Failure {
    Failure {
        category,
        message,
        code: Some(format!("{{PLUGIN_SLUG}}.{code}")),
        params: Vec::new(),
    }
}

fn authorized() -> Vec<RequestHeader> {
    vec![
        RequestHeader {
            name: "Authorization".to_owned(),
            value_template: format!("Bearer {{{{secret:{SECRET_REFERENCE}}}}}"),
        },
        RequestHeader {
            name: "Accept".to_owned(),
            value_template: "application/json".to_owned(),
        },
    ]
}

/// Fetches one document, turning everything that is not a readable answer into one refusal.
///
/// A crawl makes many requests, so this is where the vocabulary stays small: the caller sees
/// a body or a failure, and never has to decide what a status code means a second time.
fn fetch(url: &str) -> Result<String, Failure> {
    let response = http::http_request("GET", url, &[], &authorized(), &[])?;
    let body = String::from_utf8_lossy(&response.body).into_owned();
    match response.status {
        200..=299 if listing::string_field(&body, "status").as_deref() != Some("error") => Ok(body),
        401 | 403 => Err(refuse(
            "folder_unreachable",
            "the provider did not accept this account for that folder".to_owned(),
            FailureKind::AuthRequired,
        )),
        429 => Err(refuse(
            "folder_unreachable",
            "the provider is rate limiting this account".to_owned(),
            FailureKind::RateLimited(None),
        )),
        500..=599 => Err(refuse(
            "folder_unreachable",
            format!("the provider answered {}", response.status),
            FailureKind::Transient(None),
        )),
        status => Err(refuse(
            "folder_unreachable",
            format!("the provider answered {status} to the listing request"),
            FailureKind::Permanent,
        )),
    }
}

impl Guest for Component {
    /// Reaches nothing: this is asked of every link a person pastes, and it answers from the
    /// address alone.
    fn claims_url(url: String) -> bool {
        target::claim(&url, HOST).is_some()
    }

    fn crawl(url: String) -> Result<Vec<CrawledLink>, Failure> {
        let Some(claimed) = target::claim(&url, HOST) else {
            return Err(refuse(
                "folder_unreachable",
                "this address is not one this plugin opens".to_owned(),
                FailureKind::Unsupported,
            ));
        };
        if claimed.kind == Kind::Item {
            let body = fetch(&format!("{ITEM_ENDPOINT}{}", claimed.id))?;
            let Some(listing::Entry::File { name, url, size }) = listing::entry(&body) else {
                return Err(refuse(
                    "bad_reply",
                    "the provider described this item in a way this plugin cannot read".to_owned(),
                    FailureKind::Permanent,
                ));
            };
            return Ok(vec![CrawledLink {
                url,
                file_name: Some(name).filter(|name| !name.is_empty()),
                size,
                package_hint: None,
            }]);
        }
        let mut walk = Walk::start(&claimed.id);
        while let Some(pending) = walk.next_folder() {
            let body = fetch(&format!("{FOLDER_ENDPOINT}{}", pending.id))?;
            walk.absorb(&pending, listing::entries(&body));
        }
        if let Some(limit) = walk.limit() {
            // Reported, never silent: a person who pasted a folder and got 500 of its 900
            // files has to be told which half they are looking at.
            host::log(
                "warn",
                &match limit {
                    Limit::Depth => "the folder is nested deeper than this crawl walks",
                    Limit::Files => "the folder holds more files than this crawl lists",
                    Limit::Folders => "the folder holds more subfolders than this crawl reads",
                }
                .to_owned(),
            );
        }
        let files = walk.into_files();
        if files.is_empty() {
            // An empty answer is not a result. Handing one back would create a package with
            // nothing in it, and nothing in the interface would explain why.
            return Err(refuse(
                "folder_empty",
                "this folder holds no files".to_owned(),
                FailureKind::Permanent,
            ));
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
}

export!(Component);
