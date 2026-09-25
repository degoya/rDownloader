//! Reading a Box address without a URL parser.
//!
//! Shared because the resolver and the crawler have to agree on it exactly. They claim
//! *different* addresses — one files, one folders — but they must agree on which hosts are
//! Box's, on how a shared link is spelled back to the API, and above all on **which address is
//! a folder and which a file**, because that is the whole of the line between the two packages:
//! an address claimed by both would be fetched twice, and an address claimed by neither would
//! sit in the LinkGrabber as a dead link. Two implementations of that would eventually be two
//! answers.
//!
//! Written without a URL parser so the same code compiles into a component, where the `url`
//! crate is not part of the guest build.

/// The Box Content API, and the only address the resolver and the crawler reach.
pub const API: &str = "https://api.box.com/2.0";

/// The canonical web host, the one an address with no tenant of its own is spelled on.
pub const WEB_HOST: &str = "app.box.com";

/// Longest shared link this will carry into a `boxapi` header. A link is a value a stranger
/// chose; bounded rather than trusted.
const MAX_LINK_BYTES: usize = 512;

/// Which Box host an address is on, or `None`.
///
/// A host merely *ending* in the same letters is not one of them, which is what
/// `app.box.com.evil.test` would be. The one wildcard is the enterprise subdomain — Box gives
/// every enterprise its own `<name>.app.box.com`, and a shared link handed out there only works
/// when it is spelled back with that host — and it is bounded: exactly one label in front of
/// `app.box.com`, made of the characters a subdomain may contain.
#[must_use]
pub fn box_host(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == WEB_HOST {
        return Some(host);
    }
    host.strip_suffix(".app.box.com")
        .filter(|label| valid_label(label))
        .map(|_| host.clone())
}

/// An enterprise subdomain label: letters, digits and hyphens, one label and no more.
fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.contains('.')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Scheme, host and the rest of an address.
///
/// Returns `None` for anything that is not plain http(s), and for an authority carrying
/// credentials — accepting those would let `x@evil.test` read as one of Box's own hosts.
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

/// Percent-encodes one query value.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Percent-decodes one query value, or `None` when it is not valid UTF-8.
///
/// `+` is decoded to a space: this is read out of a query string, which is where a browser
/// writes a space that way, and a shared-link password is exactly the kind of value somebody
/// pastes with a space in it.
#[must_use]
pub fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = bytes.get(index + 1..index + 3)?;
                let text = std::str::from_utf8(hex).ok()?;
                decoded.push(u8::from_str_radix(text, 16).ok()?);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// A Box item id as the API issues them: a decimal number and nothing else.
///
/// Checked rather than trusted, because it is pasted straight into a request path. Anything
/// else could be a second address.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 24 && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// A shared-link name as Box issues them — the `<name>` of `/s/<name>`: URL-safe characters
/// and nothing else.
#[must_use]
pub fn valid_share(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// A file version id, which Box issues in the same shape as an item id.
#[must_use]
pub fn valid_version(version: &str) -> bool {
    valid_id(version)
}

/// A SHA-1 as the API states it — 40 hexadecimal characters.
#[must_use]
pub fn valid_sha1(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A Box address, read down to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Address {
    /// A file in the account's own Box: `/file/<id>`.
    File { host: String, id: String },
    /// A folder in the account's own Box: `/folder/<id>`.
    Folder { host: String, id: String },
    /// A shared link that does not say what it points at: `/s/<name>`. Box spells a shared file
    /// and a shared folder identically, so this is the one address only the API can decide —
    /// which is the crawler's job, because it has to ask anyway.
    Shared {
        link: String,
        password: Option<String>,
    },
    /// A file reached through a shared link: `/s/<name>/file/<id>`.
    SharedFile {
        link: String,
        id: String,
        password: Option<String>,
    },
    /// A folder reached through a shared link: `/s/<name>/folder/<id>`.
    SharedFolder {
        link: String,
        id: String,
        password: Option<String>,
    },
}

/// Reads an address, or `None` for anything that is not a Box item address.
///
/// This is the one decision both plugins make, so it is made once: the resolver claims
/// [`Address::File`] and [`Address::SharedFile`], the crawler claims the other three, and
/// nothing is claimed twice.
#[must_use]
pub fn parse(url: &str) -> Option<Address> {
    let (authority, path) = split(url)?;
    let host = box_host(authority)?;
    let password = link_password(path);
    let parts = segments(path);
    match parts.as_slice() {
        ["file", id] if valid_id(id) => Some(Address::File {
            host,
            id: (*id).to_owned(),
        }),
        ["folder", id] if valid_id(id) => Some(Address::Folder {
            host,
            id: (*id).to_owned(),
        }),
        ["s", name] if valid_share(name) => Some(Address::Shared {
            link: share_link(&host, name),
            password,
        }),
        ["s", name, "file", id] if valid_share(name) && valid_id(id) => Some(Address::SharedFile {
            link: share_link(&host, name),
            id: (*id).to_owned(),
            password,
        }),
        ["s", name, "folder", id] if valid_share(name) && valid_id(id) => {
            Some(Address::SharedFolder {
                link: share_link(&host, name),
                id: (*id).to_owned(),
                password,
            })
        }
        _ => None,
    }
}

/// The password a pasted address carries, if it carries one.
///
/// A resolver sees nothing of what a person typed but the address, so this is the only place a
/// shared-link password can come from. Both spellings are read: Box's own parameter name, and
/// the short one people write. The core redacts both out of everything it logs, and neither
/// travels any further than the `boxapi` header [`box_api`] builds.
fn link_password(path: &str) -> Option<String> {
    ["shared_link_password", "password"]
        .into_iter()
        .find_map(|name| query_value(path, name))
        .filter(|value| !value.is_empty())
        .and_then(percent_decode)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
}

/// The canonical spelling of a shared link, on the host it was handed out on.
fn share_link(host: &str, name: &str) -> String {
    format!("https://{host}/s/{name}")
}

/// The canonical address of one file in the account's own Box — what the crawler hands back and
/// the resolver claims, and the only thing that passes between the two packages.
#[must_use]
pub fn file_address(host: &str, id: &str) -> String {
    format!("https://{host}/file/{id}")
}

/// The canonical address of one file reached through a shared link.
///
/// It carries the link the file was found through, because a shared link grants access through
/// that link and not by the item's own id: an account that may open the share need not be able
/// to open the file on its own.
///
/// And it carries the link's password, for the same reason and with the same reluctance as
/// `dropbox_common::address::shared_file_address` (RD-106-06). Nothing passes between the
/// crawler and the resolver but an address, so an address is the only place a password can
/// travel; leaving it out would list a protected folder perfectly and then fail every file in
/// it. `rd_core::is_secret_parameter` knows the parameter, so it is struck out of every log line and
/// every message, and it never reaches the address the *bytes* come from — that one is
/// `content_address`, which carries an item id and a version and nothing else.
#[must_use]
pub fn shared_file_address(link: &str, id: &str, password: Option<&str>) -> String {
    match password.filter(|password| !password.is_empty()) {
        Some(password) => format!(
            "{link}/file/{id}?shared_link_password={}",
            percent_encode(password)
        ),
        None => format!("{link}/file/{id}"),
    }
}

/// The **stable** address the bytes of one file version come from.
///
/// Deliberately the API route rather than the `dl.boxcloud.com` address Box redirects to. That
/// one is pre-authenticated, short-lived and single-use; this one still means the same thing
/// after a restart, which is what lets `rd_scheduler::replay::before_resume` ask for it again
/// (RD-106-04, rule 5).
///
/// And deliberately pinned to a version. `/files/<id>/content` means "whatever is in that file
/// now", so a transfer that outlived an edit would splice two files together; with `?version=`
/// it means one set of bytes for good, and a Box that no longer has them refuses instead of
/// serving different ones.
#[must_use]
pub fn content_address(id: &str, version: Option<&str>) -> String {
    match version.filter(|version| valid_version(version)) {
        Some(version) => format!("{API}/files/{id}/content?version={version}"),
        None => format!("{API}/files/{id}/content"),
    }
}

/// The `boxapi` header value that opens a shared link, password and all.
///
/// Box reads this header as a query string, so the password is percent-encoded: it is a value a
/// stranger chose, and one containing `&` or `=` would otherwise end the header early and be
/// read as a parameter of its own. The link needs no encoding — it is rebuilt by [`share_link`]
/// out of a validated host and a validated name, so it cannot contain either character.
///
/// The password exists in this one string and nowhere else: never in the address the transfer
/// goes to, never in a query parameter, never in a message.
#[must_use]
pub fn box_api(link: &str, password: Option<&str>) -> Option<String> {
    if link.is_empty() || link.len() > MAX_LINK_BYTES {
        return None;
    }
    Some(match password {
        Some(password) if !password.is_empty() => format!(
            "shared_link={link}&shared_link_password={}",
            percent_encode(password)
        ),
        _ => format!("shared_link={link}"),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Address, box_api, box_host, content_address, file_address, parse, percent_decode,
        shared_file_address,
    };

    #[test]
    fn every_spelling_of_a_box_address_is_read() {
        assert_eq!(
            parse("https://app.box.com/file/123456789"),
            Some(Address::File {
                host: "app.box.com".to_owned(),
                id: "123456789".to_owned()
            })
        );
        assert_eq!(
            parse("https://app.box.com/folder/987654321"),
            Some(Address::Folder {
                host: "app.box.com".to_owned(),
                id: "987654321".to_owned()
            })
        );
        // An enterprise keeps its own host, because a shared link handed out there only works
        // when it is spelled back with it.
        assert_eq!(
            parse("https://contoso.app.box.com/s/abc123def456"),
            Some(Address::Shared {
                link: "https://contoso.app.box.com/s/abc123def456".to_owned(),
                password: None
            })
        );
        assert_eq!(
            parse("https://app.box.com/s/abc123/file/42"),
            Some(Address::SharedFile {
                link: "https://app.box.com/s/abc123".to_owned(),
                id: "42".to_owned(),
                password: None
            })
        );
        assert_eq!(
            parse("https://app.box.com/s/abc123/folder/7"),
            Some(Address::SharedFolder {
                link: "https://app.box.com/s/abc123".to_owned(),
                id: "7".to_owned(),
                password: None
            })
        );
    }

    /// Both spellings of the password are read, decoded, and never end up in the link.
    #[test]
    fn a_shared_link_password_is_read_out_of_the_address_and_stays_out_of_the_link() {
        for spelling in ["shared_link_password", "password"] {
            let Some(Address::Shared { link, password }) = parse(&format!(
                "https://app.box.com/s/abc123?{spelling}=hunter%202"
            )) else {
                panic!("{spelling} was not read");
            };
            assert_eq!(password.as_deref(), Some("hunter 2"));
            assert_eq!(link, "https://app.box.com/s/abc123");
            assert!(!link.contains("hunter"));
        }
        // A `+` in a query string is a space.
        assert_eq!(percent_decode("a+b"), Some("a b".to_owned()));
        // Empty and control characters are not passwords.
        assert!(matches!(
            parse("https://app.box.com/s/abc123?password="),
            Some(Address::Shared { password: None, .. })
        ));
        assert!(matches!(
            parse("https://app.box.com/s/abc123?password=a%00b"),
            Some(Address::Shared { password: None, .. })
        ));
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_read() {
        for foreign in [
            "https://app.box.com.evil.test/file/1",
            "https://x@app.box.com/file/1",
            "ftp://app.box.com/file/1",
            "https://box.com/file/1",
            "https://evil.test/file/1",
            "https://app.box.com/",
            "https://app.box.com/settings",
            "https://api.box.com/2.0/files/1/content",
        ] {
            assert_eq!(parse(foreign), None, "{foreign}");
        }
        assert_eq!(box_host("app.box.com.evil.test"), None);
        assert_eq!(box_host("..app.box.com"), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        for bad in [
            "https://app.box.com/file/../etc/passwd",
            "https://app.box.com/file/abc",
            "https://app.box.com/file/",
            "https://app.box.com/s/a%2Fb",
            "https://app.box.com/s/abc/file/x",
            "https://app.box.com/folder/12345678901234567890123456789",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    /// The canonical addresses the crawler emits are exactly the ones the resolver reads back.
    #[test]
    fn the_canonical_addresses_round_trip() {
        assert_eq!(
            parse(&file_address("app.box.com", "42")),
            Some(Address::File {
                host: "app.box.com".to_owned(),
                id: "42".to_owned()
            })
        );
        assert_eq!(
            parse(&shared_file_address(
                "https://app.box.com/s/abc123",
                "42",
                None
            )),
            Some(Address::SharedFile {
                link: "https://app.box.com/s/abc123".to_owned(),
                id: "42".to_owned(),
                password: None
            })
        );
        // A protected link's password survives the hand-over, because an address is the only
        // channel between the two packages — and it survives it encoded, so a password holding
        // an `&` cannot become a second parameter.
        assert_eq!(
            parse(&shared_file_address(
                "https://app.box.com/s/abc123",
                "42",
                Some("hunter 2&x")
            )),
            Some(Address::SharedFile {
                link: "https://app.box.com/s/abc123".to_owned(),
                id: "42".to_owned(),
                password: Some("hunter 2&x".to_owned())
            })
        );
    }

    /// The download address is pinned to one version, so two versions of one file are two
    /// addresses and a resume can never continue one with the other's bytes.
    #[test]
    fn the_download_address_names_the_version_it_is_the_bytes_of() {
        assert_eq!(
            content_address("42", Some("1001")),
            "https://api.box.com/2.0/files/42/content?version=1001"
        );
        assert_ne!(
            content_address("42", Some("1001")),
            content_address("42", Some("1002"))
        );
        // A version Box did not state, or one that is not one, leaves the route unpinned rather
        // than pinned to a value this plugin invented.
        assert_eq!(
            content_address("42", None),
            "https://api.box.com/2.0/files/42/content"
        );
        assert_eq!(
            content_address("42", Some("../1")),
            content_address("42", None)
        );
    }

    /// The password reaches the `boxapi` header and no other part of a request.
    #[test]
    fn the_box_api_header_carries_the_password_and_encodes_it() {
        assert_eq!(
            box_api("https://app.box.com/s/abc123", Some("a&b=c")),
            Some(
                "shared_link=https://app.box.com/s/abc123&shared_link_password=a%26b%3Dc"
                    .to_owned()
            )
        );
        assert_eq!(
            box_api("https://app.box.com/s/abc123", None),
            Some("shared_link=https://app.box.com/s/abc123".to_owned())
        );
        assert_eq!(box_api("", Some("x")), None);
        assert_eq!(box_api(&"x".repeat(600), None), None);
    }
}
