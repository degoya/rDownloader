//! The links a crawler hands back, and the login one may ask them to be fetched under: an
//! address is split from its user name and refused when it carries a password, and the one
//! share login a whole answer names becomes a scope an auth profile can be minted for.
//!
//! Split out of `crawler.rs` (PLUG-21).

/// What a crawler found behind one address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrawledLink {
    /// The address, with any login the crawler put in front of it already taken out.
    pub url: url::Url,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    pub package_hint: Option<String>,
    /// What the source said about this link being one of several copies of the same file
    /// (RD-110-18). A release page knows it; a folder listing does not.
    pub mirror: Option<rd_core::MirrorHint>,
    /// The user name the crawler asked for these files to be fetched under, when it named
    /// one (RD-108-07). Never a password: see [`split_crawled_address`].
    pub login: Option<String>,
    /// Whether a site rule found it on a release page rather than a crawler plugin in a
    /// folder (RD-1190-18). The page's operator chose such an address, so it never reaches
    /// this machine or the person's network, whoever pasted the page.
    pub by_rule: bool,
}

/// The login a crawler's whole answer asks for, and the addresses it covers.
///
/// A protected share is deliberately **not** an account. An account is a per-provider login
/// with a life of its own -- listed, checked, reused by every link of that provider. A share
/// password authenticates one share, has no provider and no meaning anywhere else. What
/// already fits that exactly is an *auth profile*: a credential scoped to a host and a path
/// prefix, its secret a `vault://` reference, applied by the queue to every address the scope
/// covers. So this is what the caller needs to mint one, and nothing more -- the password
/// itself never passes through here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShareLogin {
    pub username: String,
    /// Host plus the longest path all the found files share, so the credential reaches the
    /// share's files and stops there.
    pub scope: rd_core::AuthScope,
}

/// Separates a crawled address from the login a crawler put in front of it (RD-108-07).
///
/// `None` when the address cannot be used: not a URL, not `http`/`https`, or -- the one that
/// matters -- carrying a **password** in its userinfo. A crawler may name the user its files
/// are fetched as, because that is not a secret and the address is the only channel the
/// `crawled-link` record has. It may not put a credential there: an address is written to a
/// database column, returned over REST and printed in log lines, so a password in one leaks
/// everywhere at once. Such a link is dropped rather than cleaned up, because a crawler that
/// did it once will have done it to every link in the answer.
#[must_use]
pub fn split_crawled_address(address: &str) -> Option<(url::Url, Option<String>)> {
    let mut url = url::Url::parse(address).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.password().is_some() {
        return None;
    }
    let login = url.username().to_owned();
    if login.is_empty() {
        return Some((url, None));
    }
    // Plain logins only. Percent-encoding here would need a decoder and would let a `:` or
    // an `@` back in through the side door; the user names this is for are already plain.
    if !login
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return None;
    }
    url.set_username("").ok()?;
    Some((url, Some(login)))
}

/// The one login the whole answer asks for, or `None` when there is not exactly one.
///
/// Every file must name the same user on the same host, and the scope is the longest path
/// they all sit under. All three are refusals rather than best guesses: a credential minted
/// from a crawler's say-so must reach the files it was given for and nothing else, so a
/// disagreement, a second host or a prefix that has shrunk to `/` ends in no profile at all.
#[must_use]
pub fn share_login(links: &[CrawledLink]) -> Option<ShareLogin> {
    let first = links.first()?;
    let username = first.login.clone()?;
    let host = first.url.host_str()?.to_ascii_lowercase();
    let mut prefix: Vec<&str> = directory_segments(&first.url);
    for link in &links[1..] {
        if link.login.as_deref() != Some(username.as_str())
            || link.url.host_str().map(str::to_ascii_lowercase).as_deref() != Some(host.as_str())
        {
            return None;
        }
        let segments = directory_segments(&link.url);
        let shared = prefix
            .iter()
            .zip(segments.iter())
            .take_while(|(left, right)| left == right)
            .count();
        prefix.truncate(shared);
    }
    if prefix.is_empty() {
        return None;
    }
    let scope = rd_core::AuthScope {
        host,
        include_subdomains: false,
        path_prefix: Some(format!("/{}", prefix.join("/"))),
    };
    Some(ShareLogin { username, scope })
}

/// The path segments of the folder an address sits in, still percent-encoded so they compare
/// the way `AuthScope` later matches them.
fn directory_segments(url: &url::Url) -> Vec<&str> {
    let mut segments: Vec<&str> = url
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    // The file's own name is not part of the folder every file shares.
    segments.pop();
    segments
}
