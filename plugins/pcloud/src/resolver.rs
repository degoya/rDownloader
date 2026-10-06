//! pCloud's protocol logic, written once for both builds.
//!
//! Everything here goes through the official pCloud HTTP JSON API and nothing else: `stat` for
//! what a file is, `checksumfile` for what it hashes to, `showpublink` for what a public link
//! points at, `userinfo` for the account, and `getfilelink` / `getpublinkdownload` for where
//! the bytes are. No page is scraped and no undocumented endpoint is called.
//!
//! The account's access token is never in this file. Requests carry the marker
//! `{{secret:pcloud_access_token}}` in an `Authorization: Bearer` header, which the host
//! expands on the way out and only towards pCloud's own API hosts; the sibling OAuth plugin is
//! what puts a value behind it. A public-link call carries no credential at all — pCloud asks
//! for none — so a shared link is opened without the account's token going anywhere near it.
//!
//! # Two installations, and how one is chosen
//!
//! pCloud runs two separate installations, `api.pcloud.com` and `eapi.pcloud.com`, and an
//! access token, a `fileid` and a public link code each exist in exactly one of them. The
//! other answers "I do not know that" — `result: 2094` for a token, the 7xxx family for a link
//! code — which reads exactly like a bad credential or a dead link if it is taken at face
//! value. So it is not taken at face value:
//!
//! 1. **The address names the region.** A pCloud address is spelled on the installation's own
//!    host — `e.pcloud.com` and `e.pcloud.link` in Europe, `my.pcloud.com` and `u.pcloud.link`
//!    in the United States — and `pcloud_common::address` reads that out of the host.
//! 2. **One refusal, and only two of them, is retried at the other installation.** A refused
//!    credential and a refused link code are the two answers that can mean "wrong region"; a
//!    missing file, a denied operation and a rate limit mean the region was right and the
//!    answer was no. [`call`] retries exactly once, and only for the two.
//! 3. **The region that answered is then pinned** for every further call of the same
//!    invocation, so a walk or a resolve pays the correction at most once.
//!
//! An account probe, which has no address to read, starts at `api.pcloud.com` — the host
//! pCloud's own documentation calls without qualification — and falls through to the European
//! one. Whichever answered is put on the account's label, so a person can see which
//! installation their account lives in instead of guessing at it.
//!
//! # A download address that expires, answered without writing one down
//!
//! `getfilelink` does not serve bytes; it hands back a list of content servers, a path and an
//! `expires`. That ticket is short-lived, and pCloud offers no stable byte endpoint at all —
//! there is no equivalent of Dropbox's `content.dropboxapi.com/2/files/download`. What keeps
//! that from being an expired address in a queue is that **nothing built from it is ever
//! written down**: the durable address of a download is `file.source`, which is the canonical
//! `pcloud_common::address` spelling of the file — a `fileid`, or a link code plus a `fileid` —
//! and `rd_scheduler::worker::run` re-resolves from it on every attempt, with
//! `rd_scheduler::replay::before_resume` doing the same before a partial file is continued.
//! So a ticket lives for exactly one attempt and is minted again for the next, and what proves
//! the new one is the same bytes is the size and the checksum, which are read afresh with it.
//!
//! The token does not travel to the content host either, and must not: the ticket is already
//! authorised, and pCloud's content servers are picked per request, so they are not — and
//! could not be — among the provider's `secret_domains`. `provider_download_bearer` therefore
//! never fires for pCloud, which is the correct outcome rather than a missing feature.

use pcloud_common::{
    address::Region,
    metadata::{self, Checksums, Link, Metadata, UserInfo},
};
use plugin_common::failure::{SecretSlot, coded, require_account, require_secret};
use plugin_common::{
    Account, CheckInput, Failure, FailureKind, Label, LabelPart, LinkCheck, PluginHost,
    ResolveInput, Resolved,
};

use crate::{
    messages,
    target::{self, Target},
};

mod request;

use request::{call, fixed};

/// The secret every call needs, and the words its absence is refused with.
const ACCOUNT_SECRET: SecretSlot = SecretSlot {
    reference: SECRET,
    missing: messages::SIGN_IN_REQUIRED,
};

/// The vault reference the pCloud provider keeps its access token under. The value never
/// reaches this plugin.
const SECRET: &str = "pcloud_access_token";
/// Where an account probe starts when no address says. pCloud's documentation calls this host
/// without qualification, so it is the one to try first; the European installation is one
/// refusal away.
const DEFAULT_REGION: Region = Region::Us;
/// Most links one `check` call looks up. A check is one request per link, so an unbounded
/// batch is an unbounded number of requests inside one invocation's budget.
const MAX_CHECKS: usize = 50;
/// How deep the file named by a `fileid` is looked for in a public link's tree. `showpublink`
/// answers a folder link with its whole tree at once, and a tree a stranger built is not a
/// reason to recurse without a floor.
const MAX_TREE_DEPTH: u32 = 16;

/// What one claimed address turned out to be.
struct Described {
    item: Metadata,
    /// The installation that answered, pinned for every call after it.
    region: Region,
    /// Whether the public link this came from points at a **folder** rather than at the file
    /// itself. `getpublinkdownload` wants `fileid` in exactly that case and not otherwise, and
    /// guessing it from the metadata is not safe: pCloud states a `parentfolderid` on a file
    /// whether or not the link points at its folder. So the answer is read from the shape of
    /// the document — the root either *was* the file or it was a tree the file was found in —
    /// and carried rather than re-derived. Always `false` for an own-drive address.
    link_is_folder: bool,
}

/// Whether this plugin claims `url`. Answered from the address alone and reaching nothing.
#[must_use]
pub(crate) fn matches(url: &str) -> bool {
    target::claim(url).is_some()
}

/// The hosts this plugin serves, for the account's catalogue.
pub(crate) async fn hosters<H: PluginHost>(
    _host: &H,
    _account_id: &str,
) -> Result<Vec<String>, Failure> {
    Ok(vec![
        "my.pcloud.com".to_owned(),
        "e.pcloud.com".to_owned(),
        "u.pcloud.link".to_owned(),
        "e.pcloud.link".to_owned(),
    ])
}

/// What the account is, from `userinfo` — and which of pCloud's two installations it lives in.
pub(crate) async fn check_account<H: PluginHost>(
    host: &H,
    account_id: &str,
) -> Result<Account, Failure> {
    require_secret(host, account_id, ACCOUNT_SECRET).await?;
    let (body, region) = call(host, DEFAULT_REGION, "userinfo", &[], true).await?;
    let info: UserInfo = metadata::read(&body)
        .ok_or_else(|| coded(FailureKind::Permanent, messages::INVALID_RESPONSE))?;
    Ok(Account {
        valid: true,
        premium: info.premium,
        label: Label::new()
            .user(info.email.as_deref().filter(|email| !email.is_empty()))
            // Which installation answered. Shown rather than kept, because the whole family of
            // region mistakes reads like a bad credential until somebody can see this.
            .part(
                LabelPart::coded(messages::ACCOUNT_REGION.0, messages::ACCOUNT_REGION.1)
                    .with_param("region", region.to_string()),
            )
            .into(),
        // Deliberately not `quota` minus `usedquota`. That is space left to *upload* into, and
        // reporting it as remaining traffic would tell somebody with a full pCloud that they
        // cannot download from it — which is not true.
        traffic_left: None,
    })
}

/// Turns one pCloud address into one download.
pub(crate) async fn resolve<H: PluginHost>(
    host: &H,
    input: &ResolveInput,
) -> Result<Resolved, Failure> {
    let account_id = require_account(input.account_id.as_deref(), messages::ACCOUNT_MISSING)?;
    require_secret(host, account_id, ACCOUNT_SECRET).await?;
    let claimed = target::claim(&input.url)
        .ok_or_else(|| coded(FailureKind::Unsupported, messages::NOT_A_PCLOUD_LINK))?;
    let Described {
        item,
        region,
        link_is_folder,
    } = describe(host, &claimed).await?;
    let (url, checksum) = match &claimed {
        Target::Own { file_id, .. } => {
            let identifier = file_id.to_string();
            // Best effort, and deliberately not fatal: a file whose checksum pCloud will not
            // state is still a file. What comes back depends on the installation — Europe
            // answers `sha256`, the United States `md5`, both answer `sha1` — so the strongest
            // on offer is taken rather than one being demanded.
            let checksum = match fixed(
                host,
                region,
                "checksumfile",
                &[("fileid", identifier.clone())],
                true,
            )
            .await
            {
                Ok(body) => metadata::read::<Checksums>(&body).and_then(|sums| sums.best()),
                Err(_) => None,
            };
            let body = fixed(
                host,
                region,
                "getfilelink",
                &[("fileid", identifier), ("forcedownload", "1".to_owned())],
                true,
            )
            .await?;
            (download_address(&body)?, checksum)
        }
        Target::Public { code, file_id, .. } => {
            let mut query = vec![("code", code.clone())];
            // `fileid` is what `getpublinkdownload` wants once the link points at a folder,
            // and what it does not want when the link *is* the file. Which of the two this is
            // was settled by `describe`, from the shape of the answer rather than from a field.
            if link_is_folder {
                query.push(("fileid", file_id.to_string()));
            }
            query.push(("forcedownload", "1".to_owned()));
            let body = fixed(host, region, "getpublinkdownload", &query, false).await?;
            // pCloud states no checksum for a public link, and there is nothing to invent one
            // from: a `hash` in its metadata is pCloud's own number, not a digest.
            (download_address(&body)?, None)
        }
    };
    Ok(Resolved {
        url,
        file_name: Some(item.name().to_owned()).filter(|name| !name.is_empty()),
        size: item.size,
        // Nothing to repeat: the ticket pCloud handed back is already authorised, and the
        // account's token has no business at a content server.
        headers: Vec::new(),
        checksum: checksum.map(|(algorithm, value)| (algorithm.to_owned(), value)),
    })
}

/// Whether each of a batch of links is still there.
pub(crate) async fn check<H: PluginHost>(
    host: &H,
    input: &CheckInput,
) -> Result<Vec<LinkCheck>, Failure> {
    let account_id = require_account(input.account_id.as_deref(), messages::ACCOUNT_MISSING)?;
    require_secret(host, account_id, ACCOUNT_SECRET).await?;
    let mut results = Vec::new();
    for url in input.urls.iter().take(MAX_CHECKS) {
        let Some(claimed) = target::claim(url) else {
            results.push(LinkCheck::unknown(url));
            continue;
        };
        results.push(match describe(host, &claimed).await {
            Ok(Described { item, .. }) => LinkCheck::online(
                url,
                Some(item.name().to_owned()).filter(|name| !name.is_empty()),
                item.size,
            ),
            // A file pCloud says is gone, and a link it will not open, are offline. Anything
            // else says nothing about the link, so it stays unknown rather than being reported
            // as missing — including a refused token, which is about the account.
            Err(failure)
                if matches!(
                    failure.code.as_deref(),
                    Some(code)
                        if code == messages::FILE_NOT_FOUND.0
                            || code == messages::LINK_UNAVAILABLE.0
                ) =>
            {
                LinkCheck::offline(url)
            }
            Err(_) => LinkCheck::unknown(url),
        });
    }
    Ok(results)
}

/// What one claimed address *is*, and the installation that said so.
///
/// The one place a region is settled. Everything after it runs at the region this returned.
async fn describe<H: PluginHost>(host: &H, target: &Target) -> Result<Described, Failure> {
    match target {
        Target::Own { file_id, region } => {
            let (body, region) = call(
                host,
                *region,
                "stat",
                &[("fileid", file_id.to_string())],
                true,
            )
            .await?;
            let item = metadata::item(&body)
                .ok_or_else(|| coded(FailureKind::Permanent, messages::INVALID_RESPONSE))?;
            if item.is_folder() {
                // The sibling crawler's address, pasted at the resolver.
                return Err(coded(FailureKind::Unsupported, messages::IS_A_FOLDER));
            }
            if !item.is_file() {
                return Err(coded(FailureKind::Permanent, messages::FILE_NOT_FOUND));
            }
            Ok(Described {
                item,
                region,
                link_is_folder: false,
            })
        }
        Target::Public {
            code,
            file_id,
            region,
        } => {
            let (body, region) = call(
                host,
                *region,
                "showpublink",
                &[("code", code.clone())],
                false,
            )
            .await?;
            let root = metadata::item(&body)
                .ok_or_else(|| coded(FailureKind::Permanent, messages::INVALID_RESPONSE))?;
            // The link points at the file itself when the document pCloud answered with *is*
            // that file; anything else means it answered with a tree and the file was found in
            // it, which is the case `getpublinkdownload` wants a `fileid` for.
            let link_is_folder = !(root.is_file() && root.fileid == Some(*file_id));
            let item = find_file(&root, *file_id, 0)
                .ok_or_else(|| coded(FailureKind::Permanent, messages::FILE_NOT_FOUND))?;
            Ok(Described {
                item,
                region,
                link_is_folder,
            })
        }
    }
}

/// The file with this `fileid` somewhere in a public link's tree.
///
/// `showpublink` answers a folder link with the whole tree at once, so the file the address
/// named is in there rather than one call away. Bounded, because the tree was built by
/// whoever shared the link.
fn find_file(node: &Metadata, file_id: u64, depth: u32) -> Option<Metadata> {
    if node.is_file() && node.fileid == Some(file_id) {
        return Some(node.clone());
    }
    if depth >= MAX_TREE_DEPTH {
        return None;
    }
    node.contents
        .iter()
        .find_map(|child| find_file(child, file_id, depth + 1))
}

/// The address the bytes come from, out of a `getfilelink` or `getpublinkdownload` answer.
fn download_address(body: &[u8]) -> Result<String, Failure> {
    let link: Link = metadata::read(body)
        .ok_or_else(|| coded(FailureKind::Permanent, messages::INVALID_RESPONSE))?;
    link.download_url()
        .ok_or_else(|| coded(FailureKind::Permanent, messages::INVALID_DOWNLOAD_HOST))
}

#[cfg(test)]
#[path = "resolver/tests.rs"]
mod tests;
