//! The component: a share address in, the files behind it out.
//!
//! One invocation walks the whole tree, because a guest is instantiated fresh for every call
//! and remembers nothing of its own. What keeps that from being unbounded is [`crate::walk`]:
//! depth, breadth and cycles are refused there, and the manifest's fuel and time budget sit
//! underneath as the last resort rather than as the plan.
//!
//! Two endpoints, tried in that order. Nextcloud 29 and later serve
//! `/public.php/dav/files/<token>`, which wants `X-Requested-With: XMLHttpRequest` on
//! everything that is not a `GET`; older Nextcloud and every ownCloud serve
//! `/public.php/webdav` with the share token as the user name. A server that does not know
//! the first one answers 404 or 405, which is the signal to try the second rather than to
//! give up.
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
    messages, propfind,
    target::{self, Share},
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
/// `/s/<token>` is a shape plenty of sites use, so this plugin is sometimes wrong about an
/// address it claimed. Before RD-107-05 being wrong once ended the link; now the address goes
/// on to the next crawler.
fn not_mine(message: (&str, &str)) -> Failure {
    refuse(message, FailureKind::Unsupported)
}

/// Which of the two public endpoints a server speaks.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Endpoint {
    /// `/public.php/dav/files/<token>` — Nextcloud 29 and later.
    Modern,
    /// `/public.php/webdav` — older Nextcloud, and ownCloud.
    Legacy,
}

/// The headers one `PROPFIND` carries.
///
/// The credential is a share password somebody appended to the address they pasted, not a
/// stored secret: this plugin has no account and no vault reference. A share with a password
/// is deliberately not an account -- RD-108-07 settled that -- but an auth profile the host
/// mints from the login this plugin puts in the addresses it hands back.
fn headers(share: &Share, endpoint: Endpoint) -> Vec<RequestHeader> {
    let mut out = vec![
        RequestHeader {
            name: "Depth".to_owned(),
            value_template: "1".to_owned(),
        },
        RequestHeader {
            name: "Content-Type".to_owned(),
            value_template: "application/xml; charset=utf-8".to_owned(),
        },
        // Nextcloud refuses everything but `GET` on the public endpoint without it.
        RequestHeader {
            name: "X-Requested-With".to_owned(),
            value_template: "XMLHttpRequest".to_owned(),
        },
    ];
    if let Some(password) = &share.password {
        let user = basic_user(share, endpoint);
        out.push(RequestHeader {
            name: "Authorization".to_owned(),
            value_template: format!(
                "Basic {}",
                target::base64(format!("{user}:{password}").as_bytes())
            ),
        });
    }
    out
}

/// Lists one collection. `Ok(None)` means "this server does not know this endpoint".
fn list(
    share: &Share,
    endpoint: Endpoint,
    url: &str,
) -> Result<Option<Vec<propfind::Item>>, Failure> {
    let response = http::http_request(
        "PROPFIND",
        url,
        &[],
        &headers(share, endpoint),
        propfind::BODY.as_bytes(),
    )?;
    match response.status {
        // 207 Multi-Status is the answer; 200 is what a few proxies rewrite it to.
        200 | 207 => {
            let body = String::from_utf8_lossy(&response.body).into_owned();
            let items = propfind::items(&body);
            if items.is_empty() {
                return Err(refuse(messages::INVALID_RESPONSE, FailureKind::Permanent));
            }
            Ok(Some(items))
        }
        401 => Err(match share.password {
            Some(_) => refuse(messages::PASSWORD_WRONG, FailureKind::AuthRequired),
            None => refuse(messages::PASSWORD_REQUIRED, FailureKind::AuthRequired),
        }),
        // The share is gone or was withdrawn. 403 on a public endpoint is Nextcloud's answer
        // to an expired share, which is a fact about the share rather than about the plugin.
        403 | 410 => Err(refuse(messages::SHARE_UNREACHABLE, FailureKind::Permanent)),
        // The endpoint is unknown here: an older Nextcloud, an ownCloud, or no Nextcloud.
        404 | 405 | 501 => Ok(None),
        429 => Err(refuse(
            messages::SERVER_BUSY,
            FailureKind::RateLimited(None),
        )),
        500..=599 => Err(refuse(
            messages::SHARE_UNREACHABLE,
            FailureKind::Transient(None),
        )),
        _ => Err(refuse(messages::SHARE_UNREACHABLE, FailureKind::Permanent)),
    }
}

/// The user name a protected share's `Authorization: Basic` carries.
///
/// Fixed by the endpoint, never chosen: `anonymous` for the modern one, the share token
/// itself for the old one. It is not a secret -- the token is already in the address -- which
/// is what makes it safe to hand to the host inside the address (see [`link_address`]).
fn basic_user(share: &Share, endpoint: Endpoint) -> &str {
    match endpoint {
        Endpoint::Modern => "anonymous",
        Endpoint::Legacy => share.token.as_str(),
    }
}

/// The address of one entry, from the path the server gave.
fn absolute(share: &Share, href: &str) -> String {
    let origin = format!("{}://{}", share.scheme, share.authority);
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_owned();
    }
    format!("{origin}{href}")
}

/// The address one found file is *handed back* at (RD-108-07).
///
/// The same address [`absolute`] builds for a request, plus the login when the share is
/// protected -- because a listed address that carries no credential is worth nothing: the
/// server does not serve the file without `Authorization: Basic`, and that is the whole
/// defect this job exists to close. The password stays here; only the user name travels, and
/// only for an address on the share's own origin, so a share that points somewhere else
/// cannot make the host mint a credential for a host it never saw.
fn link_address(share: &Share, endpoint: Endpoint, href: &str) -> String {
    let address = absolute(share, href);
    let origin = format!("{}://{}/", share.scheme, share.authority);
    if share.password.is_none() || !address.starts_with(&origin) {
        return address;
    }
    target::with_login(&address, basic_user(share, endpoint))
}

/// Reads the share, trying the modern endpoint and falling back to the old one.
fn open(share: &Share) -> Result<(Endpoint, String, Vec<propfind::Item>), Failure> {
    for endpoint in [Endpoint::Modern, Endpoint::Legacy] {
        let root = match endpoint {
            Endpoint::Modern => format!("{}/", share.modern_endpoint()),
            Endpoint::Legacy => format!("{}/", share.legacy_endpoint()),
        };
        if let Some(items) = list(share, endpoint, &root)? {
            return Ok((endpoint, root, items));
        }
    }
    // Both endpoints answered "no such thing". The address had the shape of a share and
    // there is no Nextcloud behind it, so it goes on to the next crawler.
    Err(not_mine(messages::NOT_A_NEXTCLOUD))
}

/// The path part of an address, which is what a `href` is compared against.
fn path_of(url: &str) -> String {
    match url.split_once("://") {
        Some((_, rest)) => match rest.find('/') {
            Some(index) => rest[index..].to_owned(),
            None => "/".to_owned(),
        },
        None => url.to_owned(),
    }
}

fn crawl_share(share: &Share) -> Result<Vec<CrawledLink>, Failure> {
    let (endpoint, root_url, first_page) = open(share)?;
    let root_href = path_of(&root_url);
    // A share of one file answers with exactly that file and nothing under it. Handing it
    // back is more use than "this share holds no files", which is what the walk would say.
    if first_page.len() == 1 && !first_page[0].is_collection {
        let item = &first_page[0];
        return Ok(vec![CrawledLink {
            url: link_address(share, endpoint, &item.href),
            file_name: Some(item.name.clone()).filter(|name| !name.is_empty()),
            size: item.size,
            package_hint: None,
        }]);
    }
    // The share's own name, which becomes the package suggestion. The server names it in the
    // entry describing the collection itself; without one the token would be the only name
    // available, and a package called `abcdefghijklmno` helps nobody.
    let root_name = first_page
        .iter()
        .find(|item| crate::walk::normalize(&item.href) == crate::walk::normalize(&root_href))
        .map(|item| item.name.clone())
        .unwrap_or_default();
    let mut walk = Walk::start(&root_href, &crate::walk::join("", &root_name));
    let mut page = Some(first_page);
    while let Some(pending) = walk.next_folder() {
        let items = match page.take() {
            Some(items) => items,
            None => {
                let url = absolute(share, &format!("{}/", pending.href));
                match list(share, endpoint, &url) {
                    Ok(Some(items)) => items,
                    // A subfolder that is gone or refused is a hole in the tree, not a
                    // failure of the whole share: losing it beats losing the other 400 files.
                    Ok(None) => continue,
                    Err(_) => {
                        host::log("info", "a subfolder of this share could not be read");
                        continue;
                    }
                }
            }
        };
        walk.absorb(&pending, items);
    }
    if let Some(limit) = walk.limit() {
        // Reported rather than silent: somebody who pasted a share and got 500 of its 900
        // files has to be able to find out which half they are looking at.
        host::log(
            "warn",
            match limit {
                Limit::Depth => "this share is nested deeper than the crawl walks",
                Limit::Files => "this share holds more files than the crawl lists",
                Limit::Folders => "this share holds more subfolders than the crawl reads",
            },
        );
    }
    let files = walk.into_files();
    if files.is_empty() {
        // An empty answer is not a result. Handing one back would create a package with
        // nothing in it and nothing in the interface to explain why.
        return Err(refuse(messages::SHARE_EMPTY, FailureKind::Permanent));
    }
    Ok(files
        .into_iter()
        .map(|found| CrawledLink {
            url: link_address(share, endpoint, &found.href),
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
        let Some(share) = target::claim(&url) else {
            return Err(not_mine(messages::NOT_A_SHARE));
        };
        crawl_share(&share)
    }
}

export!(Component);
