//! Reading a OneDrive or SharePoint address without a URL parser.
//!
//! Shared because the resolver and the crawler have to agree on it exactly. They claim
//! *different* addresses — one files, one folders — but they must agree on which hosts are
//! Microsoft's, on how a sharing link is turned into the share id Graph wants, and above all
//! on **which sharing link is a folder and which a file**, because that is the whole of the
//! line between the two packages: an address claimed by both would be fetched twice, and an
//! address claimed by neither would sit in the LinkGrabber as a dead link. Two
//! implementations of that would eventually be two answers.
//!
//! Written without a URL parser so the same code compiles into a component, where the `url`
//! crate is not part of the guest build.

/// The Microsoft Graph API, and the only address the three plugins reach.
pub const GRAPH: &str = "https://graph.microsoft.com/v1.0";

/// Which Microsoft host an address is on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Host {
    /// `1drv.ms`, the short sharing links OneDrive hands out.
    Short,
    /// `onedrive.live.com`, the long personal-account addresses.
    Live,
    /// `<tenant>.sharepoint.com` and `<tenant>-my.sharepoint.com`: every SharePoint tenant,
    /// including the OneDrive for Business one.
    SharePoint,
    /// `graph.microsoft.com`, the canonical per-item address the crawler emits.
    Graph,
}

/// Which of Microsoft's hosts this is, or `None`.
///
/// A host merely *ending* in the same letters is not one of them, which is what
/// `onedrive.live.com.evil.test` would be. SharePoint is the one wildcard, and it is bounded:
/// exactly one label in front of `sharepoint.com`, made of the characters a tenant name may
/// contain.
#[must_use]
pub fn microsoft_host(host: &str) -> Option<Host> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    match host.as_str() {
        "1drv.ms" => Some(Host::Short),
        "onedrive.live.com" => Some(Host::Live),
        "graph.microsoft.com" => Some(Host::Graph),
        _ => host
            .strip_suffix(".sharepoint.com")
            .filter(|tenant| valid_tenant(tenant))
            .map(|_| Host::SharePoint),
    }
}

/// A SharePoint tenant label: letters, digits and hyphens, one label and no more.
fn valid_tenant(tenant: &str) -> bool {
    !tenant.is_empty()
        && tenant.len() <= 63
        && !tenant.contains('.')
        && tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Scheme, host and the rest of an address.
///
/// Returns `None` for anything that is not plain http(s), and for an authority carrying
/// credentials — accepting those would let `x@evil.test` read as one of Microsoft's own hosts.
#[must_use]
pub fn split(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let rest = rest.split('#').next()?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return None;
    }
    Some((authority.split(':').next()?, path))
}

/// The value of one query parameter, undecoded.
#[must_use]
pub fn query_value<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// The path segments of an address, without the query and without empty ones.
#[must_use]
pub fn segments(path: &str) -> Vec<&str> {
    path.split('?')
        .next()
        .unwrap_or_default()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

/// What a sharing link stands for, read from the address alone.
///
/// Microsoft spells it into the link: a short link is `1drv.ms/<letter>/…` and a SharePoint
/// sharing link `…sharepoint.com/:<letter>:/…`, where `f` is a folder and every other letter
/// a kind of file — `u` for one of no particular kind, `w`, `x`, `p` for Office documents,
/// `b` for a PDF, `i`, `v`, `t` for images, video and text. The long `onedrive.live.com`
/// addresses carry no such letter, and there is no way to know from one whether it names a
/// folder or a file; those are [`LinkKind::Unknown`], and the crawler takes them — it has to
/// ask Graph anyway, and a crawl that finds a single file answers with that one file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkKind {
    Folder,
    File,
    Unknown,
}

/// Reads a sharing link, returning what it stands for — or `None` for an address on one of
/// Microsoft's hosts that is not a sharing link at all, such as a sign-in page or a site's
/// document library view.
///
/// This is the one decision both plugins make, so it is made once: the resolver claims
/// [`LinkKind::File`], the crawler claims [`LinkKind::Folder`] and [`LinkKind::Unknown`], and
/// nothing is claimed twice.
#[must_use]
pub fn share_link_kind(host: Host, path: &str) -> Option<LinkKind> {
    let parts = segments(path);
    match host {
        // `https://1drv.ms/f/s!Ab…`, `https://1drv.ms/u/c/<cid>/<token>`.
        Host::Short => match parts.as_slice() {
            [letter, _, ..] => kind_letter(letter),
            _ => None,
        },
        // `https://<tenant>.sharepoint.com/:f:/g/personal/<user>/<token>`,
        // `https://<tenant>.sharepoint.com/:x:/s/<site>/<token>`, `/:b:/r/<path>`.
        Host::SharePoint => match parts.as_slice() {
            [marker, _, ..] => marker
                .strip_prefix(':')
                .and_then(|rest| rest.strip_suffix(':'))
                .and_then(kind_letter),
            _ => None,
        },
        Host::Live => match parts.as_slice() {
            // `https://onedrive.live.com/?id=<item>&cid=<drive>` — the address the web app shows
            // for a folder *and* for a file, and the old `redir?resid=` and `embed?` spellings.
            [] | ["redir"] | ["embed"] => {
                let named = ["id", "cid", "resid"]
                    .iter()
                    .any(|name| query_value(path, name).is_some_and(|value| !value.is_empty()));
                named.then_some(LinkKind::Unknown)
            }
            // The viewers only ever open a file.
            ["view.aspx"] | ["edit.aspx"] | ["download"] | ["download.aspx"] => {
                let named = ["id", "resid"]
                    .iter()
                    .any(|name| query_value(path, name).is_some_and(|value| !value.is_empty()));
                named.then_some(LinkKind::File)
            }
            _ => None,
        },
        // A Graph address is claimed by its shape, not as a sharing link.
        Host::Graph => None,
    }
}

/// The kind a sharing link's type letter names.
fn kind_letter(letter: &str) -> Option<LinkKind> {
    match letter {
        "f" => Some(LinkKind::Folder),
        // OneNote notebooks are packages, not files: nothing to download, nothing to list.
        "o" => None,
        one if one.len() == 1 && one.bytes().all(|byte| byte.is_ascii_lowercase()) => {
            Some(LinkKind::File)
        }
        _ => None,
    }
}

/// Most bytes a sharing link may be before it is refused rather than encoded.
const MAX_SHARE_URL_BYTES: usize = 2048;

/// The share id Graph uses for a sharing link: `u!` and the link, base64url without padding.
///
/// This is Microsoft's own encoding (`/shares/{shareId}`), and it is applied to the address
/// exactly as pasted — minus a fragment, which never reaches a server — because the encoded
/// value has to name the link somebody was actually given. It works for the account's own
/// items as well as for links shared with it, which is what lets one route serve both.
#[must_use]
pub fn share_id(url: &str) -> Option<String> {
    let url = url.trim().split('#').next()?;
    if url.is_empty() || url.len() > MAX_SHARE_URL_BYTES {
        return None;
    }
    Some(format!("u!{}", base64_url(url.as_bytes())))
}

/// A share id as [`share_id`] builds one — or as Graph would accept one, which is the same
/// alphabet with a `u!`, `s!` or `b!` prefix. Bounded and character-checked rather than
/// trusted, because it is pasted straight into a request path.
#[must_use]
pub fn valid_share_id(id: &str) -> bool {
    let Some((prefix, rest)) = id.split_once('!') else {
        return false;
    };
    matches!(prefix, "u" | "s" | "b")
        && !rest.is_empty()
        && id.len() <= 4096
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// A drive or item id as Graph issues them: the letters and digits of a SharePoint item id
/// (`01BYE5RZ…`), a personal one with its `!` (`E4A6…!1234`), or a drive id with its `b!`
/// prefix. Anything else could be a path segment, an escape or a second address.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'!'))
}

/// The canonical address of the item a sharing link points at.
///
/// Half of the contract between the crawler and the resolver, with [`item_address`] the other
/// half: the crawler answers with these, the resolver claims exactly these, and neither needs
/// to know the other exists. The share id is carried rather than a bare drive and item id,
/// because a link shared with the account grants access *through the share* — an item reached
/// by its own address alone can be refused for an account that may read it through the link.
#[must_use]
pub fn root_address(share_id: &str) -> String {
    format!("{GRAPH}/shares/{share_id}/driveItem")
}

/// The canonical address of one item inside a shared folder.
#[must_use]
pub fn item_address(share_id: &str, item_id: &str) -> String {
    format!("{GRAPH}/shares/{share_id}/items/{item_id}")
}

/// base64url without padding, as RFC 4648 section 5 spells it.
#[must_use]
pub fn base64_url(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let bits = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        let characters = chunk.len() + 1;
        for index in 0..characters {
            let shift = 18 - 6 * index;
            out.push(ALPHABET[((bits >> shift) & 0x3F) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Host, LinkKind, base64_url, item_address, microsoft_host, query_value, root_address,
        segments, share_id, share_link_kind, split, valid_id, valid_share_id,
    };

    #[test]
    fn only_microsofts_own_hosts_are_microsofts() {
        assert_eq!(microsoft_host("1drv.ms"), Some(Host::Short));
        assert_eq!(microsoft_host("ONEDRIVE.LIVE.COM."), Some(Host::Live));
        assert_eq!(microsoft_host("graph.microsoft.com"), Some(Host::Graph));
        assert_eq!(
            microsoft_host("contoso.sharepoint.com"),
            Some(Host::SharePoint)
        );
        assert_eq!(
            microsoft_host("contoso-my.sharepoint.com"),
            Some(Host::SharePoint)
        );
        assert_eq!(microsoft_host("onedrive.live.com.evil.test"), None);
        assert_eq!(microsoft_host("evil-1drv.ms"), None);
        assert_eq!(microsoft_host("sharepoint.com"), None);
        // One label and no more: a tenant is not a path of subdomains.
        assert_eq!(microsoft_host("a.b.sharepoint.com"), None);
        assert_eq!(microsoft_host("login.microsoftonline.com"), None);
    }

    #[test]
    fn an_authority_carrying_credentials_is_refused() {
        assert_eq!(split("https://x@1drv.ms/f/s!abc"), None);
        assert_eq!(split("ftp://1drv.ms/f/s!abc"), None);
        assert_eq!(
            split("https://onedrive.live.com:443/?id=a&cid=b#x"),
            Some(("onedrive.live.com", "?id=a&cid=b"))
        );
    }

    #[test]
    fn a_query_value_is_read_without_decoding_it() {
        assert_eq!(query_value("?id=A%21123&cid=B", "id"), Some("A%21123"));
        assert_eq!(query_value("redir?x=1&resid=abc", "resid"), Some("abc"));
        assert_eq!(query_value("redir", "resid"), None);
        assert_eq!(
            segments("/:f:/g/personal/x?e=1"),
            vec![":f:", "g", "personal", "x"]
        );
    }

    /// The one decision both plugins make: which link is a folder and which a file. Every
    /// spelling lands in exactly one of the three kinds, and the type letter Microsoft put in
    /// the link is what decides.
    #[test]
    fn a_sharing_links_type_letter_says_what_it_stands_for() {
        assert_eq!(
            share_link_kind(Host::Short, "f/s!AkXy_Zabc"),
            Some(LinkKind::Folder)
        );
        assert_eq!(
            share_link_kind(Host::Short, "u/s!AkXy_Zabc"),
            Some(LinkKind::File)
        );
        assert_eq!(
            share_link_kind(Host::Short, "f/c/ab12cd34/EaBcDeF?e=x"),
            Some(LinkKind::Folder)
        );
        assert_eq!(
            share_link_kind(Host::Short, "w/c/ab12cd34/EaBcDeF"),
            Some(LinkKind::File)
        );
        assert_eq!(
            share_link_kind(
                Host::SharePoint,
                ":f:/g/personal/someone_example_com/Eabc?e=1"
            ),
            Some(LinkKind::Folder)
        );
        assert_eq!(
            share_link_kind(Host::SharePoint, ":b:/s/Team/Eabc"),
            Some(LinkKind::File)
        );
        assert_eq!(
            share_link_kind(
                Host::SharePoint,
                ":x:/r/sites/Team/Shared%20Documents/a.xlsx"
            ),
            Some(LinkKind::File)
        );
        // The long personal address does not say, so the crawler asks.
        assert_eq!(
            share_link_kind(Host::Live, "?id=ABC%21123&cid=ABC"),
            Some(LinkKind::Unknown)
        );
        assert_eq!(
            share_link_kind(Host::Live, "redir?resid=ABC%21123&authkey=x"),
            Some(LinkKind::Unknown)
        );
        // The viewers only ever open a file.
        assert_eq!(
            share_link_kind(Host::Live, "view.aspx?resid=ABC%21123"),
            Some(LinkKind::File)
        );
        assert_eq!(
            share_link_kind(Host::Live, "download?resid=ABC%21123&authkey=x"),
            Some(LinkKind::File)
        );
    }

    /// An address on one of Microsoft's hosts that is not a sharing link belongs to nobody.
    #[test]
    fn what_is_not_a_sharing_link_is_nobodys() {
        assert_eq!(share_link_kind(Host::Short, ""), None);
        assert_eq!(share_link_kind(Host::Short, "f"), None);
        assert_eq!(share_link_kind(Host::Short, "ff/s!abc"), None);
        // OneNote notebooks are packages: nothing to download, nothing to list.
        assert_eq!(share_link_kind(Host::Short, "o/s!abc"), None);
        assert_eq!(
            share_link_kind(
                Host::SharePoint,
                "sites/Team/Shared%20Documents/Forms/AllItems.aspx"
            ),
            None
        );
        assert_eq!(
            share_link_kind(Host::SharePoint, "_layouts/15/guestaccess.aspx?share=x"),
            None
        );
        assert_eq!(share_link_kind(Host::Live, ""), None);
        assert_eq!(share_link_kind(Host::Live, "about"), None);
        assert_eq!(share_link_kind(Host::Live, "?id="), None);
        assert_eq!(
            share_link_kind(Host::Graph, "v1.0/drives/b!x/items/y"),
            None
        );
    }

    /// Microsoft's own encoding of a sharing link, checked against the worked example in the
    /// Graph documentation.
    #[test]
    fn a_sharing_link_is_encoded_the_way_graph_documents_it() {
        let link = "https://onedrive.live.com/redir?resid=1231244193912!12&authKey=1201919!12921!1";
        assert_eq!(
            share_id(link).as_deref(),
            Some(
                "u!aHR0cHM6Ly9vbmVkcml2ZS5saXZlLmNvbS9yZWRpcj9yZXNpZD0xMjMxMjQ0MTkzOTEyITEyJmF1dGhLZXk9MTIwMTkxOSExMjkyMSEx"
            )
        );
        // A fragment never reaches a server, so it is not part of what is encoded.
        assert_eq!(share_id(&format!("{link}#frag")), share_id(link));
        assert_eq!(share_id(""), None);
        assert_eq!(share_id(&"x".repeat(3000)), None);
        assert!(valid_share_id(&share_id(link).expect("encoded")));
    }

    #[test]
    fn base64url_is_unpadded_and_uses_the_url_alphabet() {
        assert_eq!(base64_url(b""), "");
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
        assert_eq!(base64_url(&[0xFB, 0xFF]), "-_8");
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert!(valid_id("01BYE5RZ6QN3ZWBTUFOFD3GSPGOHDJD36K"));
        assert!(valid_id("E4A6B0C2D1F3G5H7!123"));
        assert!(valid_id("b!abc-DEF_ghi"));
        assert!(!valid_id(""));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("a%2Fb"));
        assert!(!valid_id(".."));
        assert!(!valid_id(&"x".repeat(257)));
        assert!(valid_share_id("u!aHR0cHM"));
        assert!(!valid_share_id("aHR0cHM"));
        assert!(!valid_share_id("u!"));
        assert!(!valid_share_id("u!a/b"));
        assert!(!valid_share_id("x!aHR0cHM"));
    }

    /// The two addresses the crawler and the resolver hand each other.
    #[test]
    fn the_canonical_addresses_are_the_ones_the_resolver_claims() {
        assert_eq!(
            root_address("u!aHR0"),
            "https://graph.microsoft.com/v1.0/shares/u!aHR0/driveItem"
        );
        assert_eq!(
            item_address("u!aHR0", "01ABC"),
            "https://graph.microsoft.com/v1.0/shares/u!aHR0/items/01ABC"
        );
    }
}
