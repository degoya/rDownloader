//! The component: one entry address in, the hoster links behind it out.
//!
//! One `GET` and nothing else in the ordinary case, which is the whole finding of RD-110-17:
//! an entry page carries its links in clear text and nothing stands in front of it. There is
//! no tree to walk, so there is no walk — an entry is one page and one page only, and no link
//! it names is followed. What comes back is a proposal the selection resolves.
//!
//! Four refusals have to be told apart, and the service tells three of them apart itself: an
//! identifier it never knew answers `404`, one it has deleted answers `200` at its front page,
//! and an entry that exists but names nothing is an empty `<article>`. The fourth, the access
//! password, is built from `PrrpLinkIn.java`'s marker and is **not** covered by a recorded
//! page: none of the seven pages of the measurement carries a protected entry, and none could
//! be found. It is written to be right and it is honestly untested.
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
    entry, messages,
    target::{self, Entry},
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

/// A browser-shaped `Accept`, because the service content-negotiates its own error pages.
fn headers() -> Vec<RequestHeader> {
    vec![RequestHeader {
        name: "Accept".to_owned(),
        value_template: "text/html,application/xhtml+xml".to_owned(),
    }]
}

/// One request against the entry, with every status that is not a page turned into a refusal.
///
/// The answer carries its final address, because a deleted entry is a `200` at the front page
/// rather than a `404`; the caller decides on that, not this function.
fn request(method: &str, target: &Entry, body: &[u8]) -> Result<(String, String), Failure> {
    let mut sent = headers();
    if !body.is_empty() {
        sent.push(RequestHeader {
            name: "Content-Type".to_owned(),
            value_template: "application/x-www-form-urlencoded".to_owned(),
        });
    }
    let response = http::http_request(method, &target.to_url(), &[], &sent, body)?;
    match response.status {
        200..=299 => Ok((
            String::from_utf8_lossy(&response.body).into_owned(),
            response.final_url,
        )),
        // The identifier was never one of theirs. Permanent, not `unsupported`: the address
        // is on their domain and in their shape, so handing it to the next crawler would only
        // replace one honest refusal with a less honest one.
        404 | 410 => Err(refuse(messages::ENTRY_NOT_FOUND, FailureKind::Permanent)),
        429 => Err(refuse(
            messages::SITE_UNREACHABLE,
            FailureKind::RateLimited(None),
        )),
        500..=599 => Err(refuse(
            messages::SITE_UNREACHABLE,
            FailureKind::Transient(None),
        )),
        _ => Err(refuse(messages::SITE_UNREACHABLE, FailureKind::Permanent)),
    }
}

/// The page of an entry, after the access password has been dealt with if there is one.
fn page_of(target: &Entry) -> Result<String, Failure> {
    let (page, final_url) = request("GET", target, &[])?;
    if target::redirected_off_entry(target, &final_url) {
        // A deleted entry answers `200` at the front page. Reading that page would find the
        // service's own submission form and call it an empty entry.
        return Err(refuse(messages::ENTRY_NOT_FOUND, FailureKind::Permanent));
    }
    if !entry::asks_for_password(&page) {
        return Ok(page);
    }
    let Some(password) = &target.password else {
        return Err(refuse(
            messages::PASSWORD_REQUIRED,
            FailureKind::AuthRequired,
        ));
    };
    // Untested against a real page: no protected entry was findable on either measuring day
    // (RD-110-17), and an invented fixture would only prove that this code agrees with
    // itself. The shape is `PrrpLinkIn.java`'s — post the password to the entry, then look at
    // the answer for the same marker again.
    host::log("info", "this entry asks for its access password");
    let body = format!("pwd={}", target::encode_form_value(password));
    let (answered, final_url) = request("POST", target, body.as_bytes())?;
    if target::redirected_off_entry(target, &final_url) {
        return Err(refuse(messages::ENTRY_NOT_FOUND, FailureKind::Permanent));
    }
    if entry::asks_for_password(&answered) {
        return Err(refuse(messages::PASSWORD_WRONG, FailureKind::AuthRequired));
    }
    Ok(answered)
}

/// Reads one entry: one page, its `<article>`, the links in it.
fn crawl_entry(target: &Entry) -> Result<Vec<CrawledLink>, Failure> {
    let page = page_of(target)?;
    let Some(body) = entry::article(&page) else {
        // Every page the service serves has an `<article>`, the `404` and the front page
        // included. One without is not a page of this service at all.
        return Err(refuse(messages::SITE_UNREACHABLE, FailureKind::Permanent));
    };
    let links = entry::links(body);
    if links.is_empty() {
        // An empty answer is not a result: it would create a package with nothing in it and
        // nothing in the interface to say why.
        return Err(refuse(messages::ENTRY_EMPTY, FailureKind::Permanent));
    }
    Ok(links
        .into_iter()
        .map(|url| CrawledLink {
            url,
            // An entry page names addresses and nothing else. The file name and the size are
            // the resolver's to find at the hoster, and guessing either from the address here
            // would put a wrong name in front of the right file.
            file_name: None,
            size: None,
            package_hint: None,
        })
        .collect())
}

impl Guest for Component {
    /// Reaches nothing: asked of every link a person pastes, answered from the address alone.
    fn claims_url(url: String) -> bool {
        target::claim(&url).is_some()
    }

    fn crawl(url: String) -> Result<Vec<CrawledLink>, Failure> {
        let Some(target) = target::claim(&url) else {
            // Defensive: the host asks `claims_url` first. `unsupported` is the refusal the
            // selection walks past, so an address that is not this plugin's carries on.
            return Err(refuse(messages::ENTRY_NOT_FOUND, FailureKind::Unsupported));
        };
        crawl_entry(&target)
    }
}

export!(Component);
