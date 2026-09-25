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
    api as pcloud_api,
    metadata::{self, Checksums, Link, Metadata, UserInfo},
};
use plugin_common::{
    Account, CheckInput, Failure, FailureKind, HttpRequest, Label, LabelPart, LinkCheck,
    LinkStatus, PluginHost, ResolveInput, Resolved,
};

use crate::{
    api, messages,
    target::{self, Target},
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

/// One refusal pCloud made: its own number, and the wait it asked for if it asked for one.
struct Refusal {
    result: u64,
    retry_after: Option<u64>,
}

/// Why one call did not produce an answer.
enum Rejected {
    /// The host refused, the transport failed, or the document was not pCloud's. Final.
    Fatal(Failure),
    /// pCloud answered, and the answer was no. May be worth asking the other installation.
    Refused(Refusal),
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
    require_token(host, account_id).await?;
    let (body, region) = call(host, DEFAULT_REGION, "userinfo", &[], true).await?;
    let info: UserInfo = metadata::read(&body)
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
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
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let claimed = target::claim(&input.url)
        .ok_or_else(|| refuse(messages::NOT_A_PCLOUD_LINK, FailureKind::Unsupported))?;
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
    let account_id = account(input.account_id.as_deref())?;
    require_token(host, account_id).await?;
    let mut results = Vec::new();
    for url in input.urls.iter().take(MAX_CHECKS) {
        let Some(claimed) = target::claim(url) else {
            results.push(unknown(url));
            continue;
        };
        results.push(match describe(host, &claimed).await {
            Ok(Described { item, .. }) => LinkCheck {
                url: url.clone(),
                status: LinkStatus::Online,
                file_name: Some(item.name().to_owned()).filter(|name| !name.is_empty()),
                size: item.size,
            },
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
                offline(url)
            }
            Err(_) => unknown(url),
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
                .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
            if item.is_folder() {
                // The sibling crawler's address, pasted at the resolver.
                return Err(refuse(messages::IS_A_FOLDER, FailureKind::Unsupported));
            }
            if !item.is_file() {
                return Err(refuse(messages::FILE_NOT_FOUND, FailureKind::Permanent));
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
                .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
            // The link points at the file itself when the document pCloud answered with *is*
            // that file; anything else means it answered with a tree and the file was found in
            // it, which is the case `getpublinkdownload` wants a `fileid` for.
            let link_is_folder = !(root.is_file() && root.fileid == Some(*file_id));
            let item = find_file(&root, *file_id, 0)
                .ok_or_else(|| refuse(messages::FILE_NOT_FOUND, FailureKind::Permanent))?;
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
        .ok_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent))?;
    link.download_url()
        .ok_or_else(|| refuse(messages::INVALID_DOWNLOAD_HOST, FailureKind::Permanent))
}

/// One call at one installation, with no correction.
async fn once<H: PluginHost>(
    host: &H,
    region: Region,
    method: &str,
    query: &[(&'static str, String)],
    authenticated: bool,
) -> Result<Vec<u8>, Rejected> {
    let mut request = HttpRequest::get(format!("{}/{method}", region.api()));
    for (name, value) in query {
        request = request.with_query(name, value.clone());
    }
    if authenticated {
        request = request.with_header("Authorization", format!("Bearer {{{{secret:{SECRET}}}}}"));
    }
    let response = host.http(request).await.map_err(Rejected::Fatal)?;
    if !(200..300).contains(&response.status) {
        // pCloud answers 200 to its own refusals, so a status that is not 2xx never came from
        // pCloud's application: it is a gateway or the network.
        return Err(Rejected::Fatal(Failure::coded(
            FailureKind::Transient(pcloud_api::retry_after(&response.headers)),
            messages::UNAVAILABLE.0,
            messages::UNAVAILABLE.1,
        )));
    }
    let Some(result) = pcloud_api::result_of(&response.body) else {
        return Err(Rejected::Fatal(refuse(
            messages::INVALID_RESPONSE,
            FailureKind::Permanent,
        )));
    };
    if result == pcloud_api::OK {
        return Ok(response.body);
    }
    Err(Rejected::Refused(Refusal {
        result,
        retry_after: pcloud_api::retry_after(&response.headers),
    }))
}

/// One call at an installation that is already settled. No correction, because there is
/// nothing left to correct.
async fn fixed<H: PluginHost>(
    host: &H,
    region: Region,
    method: &str,
    query: &[(&'static str, String)],
    authenticated: bool,
) -> Result<Vec<u8>, Failure> {
    once(host, region, method, query, authenticated)
        .await
        .map_err(|rejected| match rejected {
            Rejected::Fatal(failure) => failure,
            Rejected::Refused(refusal) => fail(&refusal),
        })
}

/// One call, corrected once if pCloud's own answer says the installation was wrong.
///
/// The correction is deliberately narrow (see the module comment): only a refused credential
/// and a refused link code, only once, and only the region that actually answered is returned
/// — so the caller pins it and the rest of the invocation costs nothing extra.
async fn call<H: PluginHost>(
    host: &H,
    start: Region,
    method: &str,
    query: &[(&'static str, String)],
    authenticated: bool,
) -> Result<(Vec<u8>, Region), Failure> {
    let mut carried: Option<Failure> = None;
    for (attempt, region) in start.both_from().into_iter().enumerate() {
        match once(host, region, method, query, authenticated).await {
            Ok(body) => return Ok((body, region)),
            Err(Rejected::Fatal(failure)) => return Err(failure),
            Err(Rejected::Refused(refusal)) => {
                let worth_the_other_region = attempt == 0
                    && pcloud_api::Category::of(refusal.result).may_be_the_other_region();
                if !worth_the_other_region {
                    return Err(fail(&refusal));
                }
                host.log(
                    "debug",
                    "pcloud refused this at the first data centre; asking the other one",
                );
                carried = Some(fail(&refusal));
            }
        }
    }
    Err(carried.unwrap_or_else(|| refuse(messages::INVALID_RESPONSE, FailureKind::Permanent)))
}

/// Turns one pCloud refusal into a failure, carrying its number and nothing else.
fn fail(refusal: &Refusal) -> Failure {
    let ((code, message), kind) = api::classify(refusal.result, refusal.retry_after);
    // pCloud's own decimal number. It is an integer, so unlike an `error` sentence there is
    // nothing in it that could ever have been a token, a file name or a path.
    Failure::coded(kind, code, message).with_param("result", refusal.result.to_string())
}

/// Refuses early when the account holds no token at all, rather than making a call that pCloud
/// is certain to refuse — twice, once per installation — and reporting whatever it says.
async fn require_token<H: PluginHost>(host: &H, account_id: &str) -> Result<(), Failure> {
    if host.secret_available(account_id, SECRET).await {
        return Ok(());
    }
    Err(refuse(
        messages::SIGN_IN_REQUIRED,
        FailureKind::AuthRequired,
    ))
}

fn account(account_id: Option<&str>) -> Result<&str, Failure> {
    account_id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| refuse(messages::ACCOUNT_MISSING, FailureKind::AuthRequired))
}

fn refuse((code, message): (&str, &str), kind: FailureKind) -> Failure {
    Failure::coded(kind, code, message)
}

fn unknown(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Unknown,
        file_name: None,
        size: None,
    }
}

fn offline(url: &str) -> LinkCheck {
    LinkCheck {
        url: url.to_owned(),
        status: LinkStatus::Offline,
        file_name: None,
        size: None,
    }
}

#[cfg(test)]
mod tests {
    use pcloud_common::metadata::Metadata;

    use super::{MAX_TREE_DEPTH, find_file, matches};

    #[test]
    fn only_pcloud_file_addresses_are_claimed() {
        assert!(matches(
            "https://my.pcloud.com/#/filemanager?folder=42&fileid=123"
        ));
        assert!(matches(
            "https://e.pcloud.link/publink/show?code=XZabc&fileid=7"
        ));
        // A folder and a bare public link are the crawler's, and a stranger's host is nobody's.
        assert!(!matches("https://my.pcloud.com/#/filemanager?folder=42"));
        assert!(!matches("https://e.pcloud.link/publink/show?code=XZabc"));
        assert!(!matches("https://ddownload.com/f/abc"));
    }

    fn folder(folder_id: u64, contents: Vec<Metadata>) -> Metadata {
        Metadata {
            name: Some(format!("d{folder_id}")),
            isfolder: true,
            folderid: Some(folder_id),
            contents,
            ..Metadata::default()
        }
    }

    fn file(file_id: u64) -> Metadata {
        Metadata {
            name: Some(format!("f{file_id}.bin")),
            isfolder: false,
            fileid: Some(file_id),
            size: Some(file_id),
            ..Metadata::default()
        }
    }

    /// `showpublink` answers a folder link with its whole tree, so the file the address named
    /// is found in it rather than fetched a second time.
    #[test]
    fn the_file_a_public_address_names_is_found_in_the_tree_the_link_answered_with() {
        let tree = folder(1, vec![file(10), folder(2, vec![file(20), file(21)])]);
        assert_eq!(
            find_file(&tree, 21, 0).and_then(|found| found.fileid),
            Some(21)
        );
        assert!(find_file(&tree, 99, 0).is_none());
        // A link that *is* one file answers with that file, not with a tree.
        assert_eq!(find_file(&file(5), 5, 0).and_then(|f| f.fileid), Some(5));
    }

    /// A tree a stranger built is not a reason to recurse without a floor.
    #[test]
    fn a_tree_deeper_than_the_floor_is_not_followed_for_ever() {
        let mut node = file(7);
        for level in 0..MAX_TREE_DEPTH + 5 {
            node = folder(u64::from(level) + 100, vec![node]);
        }
        assert!(find_file(&node, 7, 0).is_none());
        let shallow = folder(1, vec![folder(2, vec![file(7)])]);
        assert!(find_file(&shallow, 7, 0).is_some());
    }
}
