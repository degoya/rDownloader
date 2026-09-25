//! What the Turbobit and HitFile resolvers have to agree on.
//!
//! The two are one operator's brands: the same Vue shell, the same JSON API under
//! `app.<host>/api`, the same free flow (`download/info` -> `free/init` -> `captcha` ->
//! `free/captcha` -> `free/prepare` -> `free/start`) ending in a one-shot, IP-bound direct link,
//! and the same login. What differs is a handful of parameters — the host, the shape of a file
//! id, whether a file link ends in `.html`, the Turnstile site key — so the logic lives here once
//! and each brand is a [`Brand`] value in its own thin plugin. A plain library with no manifest:
//! `scripts/build-plugins.sh` packages directories that have one, so this is never a plugin and
//! never signed, the same arrangement `plugins/google-drive-common` uses.
//!
//! Deliberately *not* here: the failure codes' spelling. Each plugin owns its own `<slug>.`
//! namespace and its own translations, so a code cannot be emitted by one package and
//! translated by another; the plugin hands its constants in through [`Codes`].
//!
//! Two things the code never does, whatever the server would let it do. It never fetches the
//! direct link itself: the address `free/start` answers is good for exactly one `GET` from this
//! IP, so it is resolved right before the transfer and handed over untouched, and every retry,
//! restart or `dcount` refusal resolves again. And it never skips a step of the site's own
//! sequence: the feasibility measurement found `free/prepare` and `free/start` answering without
//! a solved captcha, and that gap is not used — what the server does not check today is not a
//! contract.

#![forbid(unsafe_code)]

pub mod account;
pub mod api;
pub mod brand;
pub mod check;
pub mod free;
pub mod link;
pub mod reason;

#[cfg(test)]
mod tests;

pub use brand::{Brand, Codes, IdRule};

use plugin_common::{
    CheckInput, Failure, FailureKind, LinkCheck, PluginHost, ResolveInput, Resolved,
};

/// Whether `brand` claims `url` — a file link in any of the accepted shapes, or a folder link,
/// which is claimed so that it ends in `folder_not_file` rather than in a download of the
/// site's HTML shell.
#[must_use]
pub fn matches(brand: &Brand, url: &str) -> bool {
    link::parse(brand, url).is_ok()
}

/// Turns a link into a download: the free flow without an account, the account's own session
/// with one.
///
/// # Errors
///
/// One of the plugin's own codes for every refusal the API is known to give, and the host's own
/// for a captcha or wait budget it could not provide.
pub async fn resolve<H: PluginHost>(
    brand: &Brand,
    host: &H,
    request: &ResolveInput,
) -> Result<Resolved, Failure> {
    let id = match link::parse(brand, &request.url) {
        Ok(link::Link::File(id)) => id,
        Ok(link::Link::Folder) => {
            return Err(reason::coded(
                brand,
                FailureKind::Unsupported,
                brand.codes.folder_not_file,
                "folder links need a crawler; this resolver takes single files",
            ));
        }
        Err(link::LinkError::Invalid) => {
            return Err(reason::coded(
                brand,
                FailureKind::Permanent,
                brand.codes.invalid_link,
                "the link could not be parsed",
            ));
        }
        Err(link::LinkError::Unsupported) => {
            return Err(reason::coded(
                brand,
                FailureKind::Unsupported,
                brand.codes.unsupported_link,
                "not a supported file link",
            ));
        }
    };
    match request.account_id.as_deref() {
        None => free::resolve(brand, host, &id).await,
        Some(account_id) => account::resolve(brand, host, account_id, &id).await,
    }
}

/// Batched link check through the operator's documented `links/check` endpoint, which takes
/// no account.
///
/// # Errors
///
/// An HTTP or read failure of one batch fails the whole call; a link the API calls `invalid`
/// is reported as `Unknown`, not as a failure.
pub async fn check<H: PluginHost>(
    brand: &Brand,
    host: &H,
    request: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    check::check(brand, host, request).await
}

/// What the account is worth, read off `user/info` and `premium/info` after signing in.
///
/// # Errors
///
/// `account_missing` without a stored password, `login_failed` for a refused one, and
/// `account_banned` for an account the site has locked.
pub async fn check_account<H: PluginHost>(
    brand: &Brand,
    host: &H,
    account_id: &str,
) -> Result<plugin_common::Account, Failure> {
    account::check_account(brand, host, account_id).await
}

/// The hosts this brand serves. A hoster serves its own, so the account changes nothing.
#[must_use]
pub fn hosters(brand: &Brand) -> Vec<String> {
    brand
        .match_hosts
        .iter()
        .map(|host| (*host).to_owned())
        .collect()
}
