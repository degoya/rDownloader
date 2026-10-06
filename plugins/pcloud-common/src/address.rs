//! Reading a pCloud address, and deciding which of pCloud's two data centres it belongs to.
//!
//! Shared because the resolver and the crawler have to agree on it exactly. They claim
//! *different* addresses — one files, one folders and public links — but they must agree on
//! which hosts are pCloud's, on how a file inside a folder is spelled, and above all on the
//! [`Region`]: pCloud runs two separate installations, and an identifier minted in one is
//! unknown to the other. Two implementations of that would eventually be two answers.
//!
//! Written without a URL parser so the same code compiles into a component, where the `url`
//! crate is not part of the guest build.
//!
//! # The fragment is where pCloud keeps the address
//!
//! Unlike Dropbox's or Google Drive's, pCloud's own addresses are fragment-based: the public
//! link its API hands back is documented as
//! `https://my.pcloud.com/#page=publink&code=<code>`, and its file manager spells a folder
//! `https://my.pcloud.com/#/filemanager?folder=<folderid>`. So this parser reads the fragment
//! rather than dropping it, and the short spelling `https://u.pcloud.link/publink/show?code=…`
//! goes through the same two fields by a different route.
//!
//! # `fileid` is the line between the two plugins
//!
//! `claims-url` and `match-url` must be disjoint (RD-106-04, rule 2), and pCloud gives no
//! other handle: a public link code is opaque, so an address alone cannot say whether the link
//! points at a file or at a folder. What it can say is whether a *particular file* is named,
//! and that is `fileid` — the very parameter `getpublinkdownload` requires once a link points
//! at a folder. An address carrying one is the resolver's; an address without one is the
//! crawler's, and a public link to a single file comes back from the crawler as that one file,
//! spelled with its `fileid`.

use std::fmt;

/// One of pCloud's two installations.
///
/// Not a preference and not a setting: an account, an access token, a `fileid` and a public
/// link code all exist in exactly one of them, and the other answers "I do not know that" to
/// every one of them. Picking the wrong one is the failure mode this whole type exists to
/// keep visible — see `region_hint`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    /// `api.pcloud.com`, `locationid` 1.
    Us,
    /// `eapi.pcloud.com`, `locationid` 2.
    Eu,
}

impl Region {
    /// The API root every call of this region goes to.
    #[must_use]
    pub const fn api(self) -> &'static str {
        match self {
            Self::Us => "https://api.pcloud.com",
            Self::Eu => "https://eapi.pcloud.com",
        }
    }

    /// The web application host, which is where an own-drive address is spelled.
    #[must_use]
    pub const fn web_host(self) -> &'static str {
        match self {
            Self::Us => "my.pcloud.com",
            Self::Eu => "e.pcloud.com",
        }
    }

    /// The short public-link host, which is where a shared address is spelled.
    #[must_use]
    pub const fn link_host(self) -> &'static str {
        match self {
            Self::Us => "u.pcloud.link",
            Self::Eu => "e.pcloud.link",
        }
    }

    /// The other one. The whole of the retry: there are two, so a wrong guess has exactly one
    /// correction and the walk can never wander between regions.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Us => Self::Eu,
            Self::Eu => Self::Us,
        }
    }

    /// Both, starting at `self`. What a call that may have guessed wrong iterates.
    #[must_use]
    pub const fn both_from(self) -> [Self; 2] {
        [self, self.other()]
    }
}

impl fmt::Display for Region {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Us => "us",
            Self::Eu => "eu",
        })
    }
}

/// The pCloud hosts these plugins know, and the region each one names.
///
/// A host merely *ending* in the same letters is not one of them, which is what
/// `pcloud.com.evil.test` would be.
pub const HOSTS: [(&str, Region); 5] = [
    ("my.pcloud.com", Region::Us),
    ("www.pcloud.com", Region::Us),
    ("e.pcloud.com", Region::Eu),
    ("u.pcloud.link", Region::Us),
    ("e.pcloud.link", Region::Eu),
];

/// Whether the region a host names is certain.
///
/// `e.pcloud.link` and `e.pcloud.com` are the European installation's own hosts and say so.
/// `my.pcloud.com` does not: pCloud's `getfilepublink` documents the link it hands back as
/// `https://my.pcloud.com/#page=publink&code=…` for **both** installations, so that host is a
/// starting point and not an answer. Everything downstream treats an uncertain region as a
/// guess to be corrected by pCloud's own refusal rather than as a fact.
#[must_use]
pub fn region_is_certain(host: &str) -> bool {
    matches!(
        normalise_host(host).as_deref(),
        Some("e.pcloud.com" | "e.pcloud.link" | "u.pcloud.link")
    )
}

fn normalise_host(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    HOSTS
        .iter()
        .any(|(known, _)| *known == host)
        .then_some(host)
}

/// Which of [`HOSTS`] this is, and the region it names.
#[must_use]
pub fn pcloud_host(host: &str) -> Option<(&'static str, Region)> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    HOSTS.into_iter().find(|(known, _)| *known == host)
}

/// Scheme, host and the rest of an address.
///
/// Returns `None` for anything that is not plain http(s), and for an authority carrying
/// credentials — accepting those would let `x@evil.test` read as one of pCloud's own hosts.
/// The fragment is **kept**, because pCloud's addresses live in it.
#[must_use]
pub fn split(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let (authority, rest) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return None;
    }
    Some((authority.split(':').next()?, rest))
}

/// The route and the parameters of an address, read out of the fragment when it has one.
///
/// pCloud's web application is a single page, so everything that identifies a file lives after
/// the `#`: `#page=publink&code=…` and `#/filemanager?folder=…`. An address without a fragment
/// is read the ordinary way, which is how the short `u.pcloud.link/publink/show?code=…`
/// spelling arrives.
#[must_use]
pub fn route_and_parameters(rest: &str) -> (&str, &str) {
    let source = match rest.split_once('#') {
        Some((_, fragment)) => fragment,
        None => rest,
    };
    match source.split_once('?') {
        Some((route, parameters)) => (route, parameters),
        // `#page=publink&code=…` has no `?`: the whole fragment is parameters.
        None if source.contains('=') => ("", source),
        None => (source, ""),
    }
}

/// The value of one parameter, undecoded: the reader every cloud plugin shares (RD-1120-10).
pub use plugin_common::address::parameter;

/// The route, reduced to its non-empty segments in lower case.
#[must_use]
pub fn segments(route: &str) -> Vec<String> {
    route
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// A public link code as pCloud issues them — URL-safe characters and nothing else.
///
/// Bounded and character-checked rather than trusted, because it is pasted straight into an
/// API parameter. Anything else could be a second address.
#[must_use]
pub fn valid_code(code: &str) -> bool {
    plugin_common::address::valid_token(code, 128)
}

/// A `fileid` or `folderid` as pCloud issues them: a decimal number and nothing else.
#[must_use]
pub fn valid_id(value: &str) -> Option<u64> {
    (!value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

/// A file or folder name as one segment of a path: non-empty, not a dot entry, no separator
/// and no control character.
pub use plugin_common::address::valid_name;

/// A SHA-1, SHA-256 or MD5 digest as `checksumfile` states them.
pub use plugin_common::address::valid_hex as valid_digest;

/// A pCloud address, read down to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Address {
    /// A public link: `#page=publink&code=…` or `/publink/show?code=…`, optionally naming one
    /// file inside it.
    Public {
        code: String,
        /// The file inside the link this address names. `None` is the link itself, which is
        /// the crawler's; `Some` is one file, which is the resolver's.
        file_id: Option<u64>,
        /// Where the code is looked up first. Only a guess on `my.pcloud.com`; see
        /// [`region_is_certain`].
        region: Region,
        /// Whether `region` was read off a host that names one, or merely started there.
        region_certain: bool,
    },
    /// A place in the account's own drive: `#/filemanager?folder=<folderid>`, optionally
    /// naming one file in it.
    Own {
        folder_id: u64,
        /// The file in the folder this address names. `None` is the folder, which is the
        /// crawler's; `Some` is one file, which is the resolver's.
        file_id: Option<u64>,
        region: Region,
        region_certain: bool,
    },
}

impl Address {
    /// The region this address starts at, and whether that is a fact or a guess.
    #[must_use]
    pub const fn region(&self) -> (Region, bool) {
        match self {
            Self::Public {
                region,
                region_certain,
                ..
            }
            | Self::Own {
                region,
                region_certain,
                ..
            } => (*region, *region_certain),
        }
    }

    /// The file this address names, when it names one.
    #[must_use]
    pub const fn file_id(&self) -> Option<u64> {
        match self {
            Self::Public { file_id, .. } | Self::Own { file_id, .. } => *file_id,
        }
    }
}

/// Reads an address, or `None` for anything that is not a pCloud address.
#[must_use]
pub fn parse(url: &str) -> Option<Address> {
    let (host, rest) = split(url)?;
    let (_, region) = pcloud_host(host)?;
    let region_certain = region_is_certain(host);
    let (route, parameters) = route_and_parameters(rest);
    let route = segments(route);
    let file_id = match parameter(parameters, "fileid") {
        // A `fileid` that is not one makes the whole address unreadable rather than silently
        // becoming the folder it sits in: the two belong to different plugins.
        Some(value) => Some(valid_id(value)?),
        None => None,
    };
    let page = parameter(parameters, "page").unwrap_or_default();
    let is_publink_route = matches!(route.as_slice(), [a, b] if a == "publink" && b == "show")
        || (route.is_empty() && page == "publink");
    if is_publink_route {
        let code = parameter(parameters, "code").filter(|code| valid_code(code))?;
        return Some(Address::Public {
            code: code.to_owned(),
            file_id,
            region,
            region_certain,
        });
    }
    if matches!(route.as_slice(), [only] if only == "filemanager") {
        let folder_id = valid_id(parameter(parameters, "folder")?)?;
        return Some(Address::Own {
            folder_id,
            file_id,
            region,
            region_certain,
        });
    }
    None
}

/// The canonical address of one file in the account's own drive, and the one the crawler hands
/// the resolver.
///
/// The folder travels with the file because pCloud's file manager addresses a file through the
/// folder it is open in, and because it costs the resolver nothing: `stat` is answered from
/// the `fileid` alone.
#[must_use]
pub fn file_address(region: Region, folder_id: u64, file_id: u64) -> String {
    format!(
        "https://{}/#/filemanager?folder={folder_id}&fileid={file_id}",
        region.web_host()
    )
}

/// The canonical address of one file behind a public link, and the one the crawler hands the
/// resolver.
#[must_use]
pub fn public_file_address(region: Region, code: &str, file_id: u64) -> String {
    format!(
        "https://{}/publink/show?code={code}&fileid={file_id}",
        region.link_host()
    )
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;
