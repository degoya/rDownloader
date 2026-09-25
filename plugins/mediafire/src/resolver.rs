//! MediaFire's protocol logic, written once for both builds.
//!
//! Every function here takes the host as a parameter rather than reaching for one, so the same
//! code runs against the native `ResolverHost` and against the WIT imports. The two adapters
//! that supply it — `native.rs` and `guest.rs` — hold nothing but type conversions.
//!
//! The route is the one the job file decided: the API for everything the API answers without
//! a session token (name, size, SHA-256, privacy, password state, link checks in batches),
//! the file page only for the direct link. There is no account: `credentials = "none"`, so
//! `check_account` is refused and `resolve` ignores whatever account the request names.

pub(crate) mod api;
mod flow;

use mediafire_common::address::{self, Address};
use plugin_common::{
    Account, CheckInput, Failure, FailureKind, LinkCheck, LinkStatus, PluginHost, ResolveInput,
    Resolved,
};
use url::Url;

use self::api::coded;
use crate::messages;

/// Whether this plugin claims `url`: a file address, or a bare key that may be one.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    matches!(
        address::parse(url),
        Some(Address::File { .. } | Address::Bare { .. })
    )
}

/// Hoster domains this plugin serves. A single hoster serves its own, so neither the host nor
/// the account changes the answer — the arguments are here because a multihoster's catalogue
/// does depend on both.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(crate::HOSTERS
        .iter()
        .map(|host| (*host).to_owned())
        .collect())
}

/// There is no account to check: public files need none, and the premium route is not
/// offered (see `lib.rs`).
pub(crate) async fn check_account<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Account, Failure> {
    Err(coded(FailureKind::Unsupported, messages::NO_ACCOUNT))
}

/// Turns a file link into a download: the API for what the file is, the page for where it is.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let (key, bare) = file_key(&request.url)?;
    let info = match api::file_info(host, &key).await {
        Ok(info) => info,
        // The API reports an unknown *file* key as "missing" (111), a folder key included.
        // A bare key is asked about as a folder before that is called an invalid link.
        Err(failure)
            if bare
                && failure.code.as_deref() == Some(messages::INVALID_LINK.0)
                && api::is_folder(host, &key).await =>
        {
            return Err(coded(FailureKind::Unsupported, messages::FOLDER_NOT_FILE));
        }
        Err(failure) => return Err(failure),
    };
    if info.private {
        return Err(coded(FailureKind::Permanent, messages::PRIVATE_FILE));
    }
    if info.password_protected {
        return Err(coded(FailureKind::Permanent, messages::PASSWORD_REQUIRED));
    }
    if !info.ready {
        return Err(coded(
            FailureKind::Transient(Some(300)),
            messages::FILE_NOT_READY,
        ));
    }
    let url = flow::direct_link(host, &key).await?;
    Url::parse(&url).map_err(|error| api::invalid_url(&error))?;
    Ok(Resolved {
        file_name: Some(info.name)
            .filter(|name| !name.is_empty())
            .or_else(|| last_segment(&url)),
        size: info.size,
        checksum: info.hash.map(|hash| ("sha256".to_owned(), hash)),
        url,
        headers: Vec::new(),
    })
}

/// Batched link check through `file/get_info`, up to [`api::CHECK_BATCH`] keys per call.
/// A key the API did not return is offline, as JD reads it; a call refused with 110 as a
/// whole makes every key in it offline; anything else fails the batch.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let keyed: Vec<(String, Option<String>)> = request
        .urls
        .iter()
        .map(|url| (url.clone(), file_key(url).ok().map(|(key, _)| key)))
        .collect();
    let mut results = Vec::with_capacity(keyed.len());
    for chunk in keyed.chunks(api::CHECK_BATCH) {
        let keys: Vec<&str> = chunk.iter().filter_map(|(_, key)| key.as_deref()).collect();
        let infos = if keys.is_empty() {
            Vec::new()
        } else {
            match api::file_infos(host, &keys).await {
                Ok(infos) => infos,
                Err(failure) if failure.code.as_deref() == Some(messages::FILE_UNAVAILABLE.0) => {
                    Vec::new()
                }
                Err(failure) => return Err(failure),
            }
        };
        for (url, key) in chunk {
            let Some(key) = key else {
                results.push(unknown(url));
                continue;
            };
            let info = infos.iter().find(|info| &info.key == key);
            results.push(LinkCheck {
                url: url.clone(),
                status: if info.is_some() {
                    LinkStatus::Online
                } else {
                    LinkStatus::Offline
                },
                file_name: info
                    .map(|info| info.name.clone())
                    .filter(|name| !name.is_empty()),
                size: info.and_then(|info| info.size),
            });
        }
    }
    Ok(results)
}

/// The file key of a link, and whether the link left open that it might be a folder.
fn file_key(url: &str) -> Result<(String, bool), Failure> {
    match address::parse(url) {
        Some(Address::File { key }) => Ok((key, false)),
        Some(Address::Bare { key }) => Ok((key, true)),
        Some(Address::Folder { .. } | Address::Keys(_)) => {
            Err(coded(FailureKind::Unsupported, messages::FOLDER_NOT_FILE))
        }
        None if Url::parse(url).is_err() => {
            Err(coded(FailureKind::Permanent, messages::INVALID_LINK))
        }
        None => Err(coded(FailureKind::Unsupported, messages::UNSUPPORTED_LINK)),
    }
}

fn last_segment(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .path_segments()?
        .next_back()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}
