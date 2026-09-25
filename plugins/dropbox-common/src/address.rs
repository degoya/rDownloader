//! Reading a Dropbox address without a URL parser.
//!
//! Shared because the resolver and the crawler have to agree on it exactly. They claim
//! *different* addresses — one files, one folders — but they must agree on which hosts are
//! Dropbox's, on what a shared link looks like, and on how a file inside a shared folder is
//! spelled. Two implementations of that would eventually be two answers, and the one that
//! mattered would be "is `dropbox.com.evil.test` Dropbox".
//!
//! Written without a URL parser so the same code compiles into a component, where the `url`
//! crate is not part of the guest build.
//!
//! What the two plugins hand each other is an [`Address`]. The crawler answers with the
//! addresses [`file_address`] and [`shared_file_address`] build, and the resolver claims exactly
//! those; the one bit that keeps `claims-url` and `match-url` disjoint is `preview`: a folder
//! address carries none, a file inside a folder carries the file's name in it — which is how
//! Dropbox's own web interface spells a previewed file.

/// The Dropbox hosts these plugins know. A host merely *ending* in the same letters is not one
/// of them, which is what `dropbox.com.evil.test` would be.
pub const HOSTS: [&str; 3] = [
    "www.dropbox.com",
    "dropbox.com",
    "dl.dropboxusercontent.com",
];

/// The host every shared link is handed to the API under.
pub const CANONICAL_HOST: &str = "www.dropbox.com";

/// The query parameter a person adds to a password-protected shared link, spelled as the API
/// argument it becomes. `password` is accepted too; both are redacted by the core.
pub const PASSWORD_PARAMETERS: [&str; 2] = ["link_password", "password"];

/// Which of [`HOSTS`] this is, or `None`.
#[must_use]
pub fn dropbox_host(host: &str) -> Option<&'static str> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    HOSTS.into_iter().find(|known| *known == host)
}

/// Scheme, host and the rest of an address.
///
/// Returns `None` for anything that is not plain http(s), and for an authority carrying
/// credentials — accepting those would let `x@evil.test` read as one of Dropbox's own hosts.
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

/// Percent-encodes one path segment or query value.
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

/// Percent-decodes one path segment or query value, or `None` when it is not valid UTF-8.
///
/// `+` is left alone: it is a plus sign in a path, and Dropbox's own links never encode a
/// space that way.
#[must_use]
pub fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            decoded.push(u8::from_str_radix(text, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// A link key as Dropbox issues them — the `<key>` of `/s/<key>/`, the id and hash of
/// `/scl/fo/<id>/<hash>`, the `rlkey`: URL-safe characters and nothing else.
///
/// Bounded and character-checked rather than trusted, because it is pasted straight into an
/// API argument. Anything else could be a second address.
#[must_use]
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// A file or folder name as one segment of a path: decoded, non-empty, not a dot entry, no
/// separator and no control character.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|character| matches!(character, '/' | '\\') || character.is_control())
}

/// A revision as the API issues them — lowercase hexadecimal, at least nine characters.
#[must_use]
pub fn valid_rev(rev: &str) -> bool {
    rev.len() >= 9 && rev.len() <= 64 && rev.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A `content_hash` as the API issues them — 64 hexadecimal characters.
#[must_use]
pub fn valid_content_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A Dropbox id as the API issues them: `id:` followed by URL-safe characters.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("id:").is_some_and(valid_key)
}

/// A Dropbox address, read down to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Address {
    /// A shared link to one file: `/s/<key>/<name>` or `/scl/fi/<id>/<name>?rlkey=<key>`.
    SharedFile {
        /// The link as the API takes it, on the canonical host and with its `rlkey`.
        link: String,
        password: Option<String>,
    },
    /// A shared link to a folder: `/sh/<key>/<hash>` or `/scl/fo/<id>/<hash>?rlkey=<key>`,
    /// optionally followed by a sub-folder path and, for a file inside it, its name as
    /// `?preview=`.
    SharedFolder {
        /// The link as the API takes it, on the canonical host and with its `rlkey`.
        link: String,
        /// Path below the link's root — empty for the root, `/Season 1` below it.
        sub_path: String,
        /// The file inside the folder this address previews, decoded.
        preview: Option<String>,
        password: Option<String>,
    },
    /// A path in the account's own Dropbox: `/home/<folder>?preview=<name>` and
    /// `/preview/<folder>/<name>` for a file, `/home/<folder>` for a folder.
    Own {
        /// The folder's API path — empty for the root, `/Show/Season 1` below it.
        path: String,
        /// The file inside the folder this address previews, decoded.
        preview: Option<String>,
    },
}

/// Reads an address, or `None` for anything that is not a Dropbox address.
#[must_use]
pub fn parse(url: &str) -> Option<Address> {
    let (host, path) = split(url)?;
    dropbox_host(host)?;
    let route = path.split('?').next()?;
    let segments: Vec<&str> = route.split('/').filter(|part| !part.is_empty()).collect();
    let password = PASSWORD_PARAMETERS
        .iter()
        .find_map(|name| query_value(path, name))
        .and_then(percent_decode)
        .filter(|value| !value.is_empty() && value.len() <= 128);
    let preview = query_value(path, "preview")
        .and_then(percent_decode)
        .filter(|name| valid_name(name));
    let rlkey = query_value(path, "rlkey").filter(|key| valid_key(key));
    match segments.as_slice() {
        // The legacy file link, and the current one — which needs its `rlkey` to be a link.
        ["s", key, name] if valid_key(key) && valid_name(&percent_decode(name)?) => {
            Some(Address::SharedFile {
                link: format!("https://{CANONICAL_HOST}/s/{key}/{name}"),
                password,
            })
        }
        ["scl", "fi", id, name] if valid_key(id) && valid_name(&percent_decode(name)?) => {
            let rlkey = rlkey?;
            Some(Address::SharedFile {
                link: format!("https://{CANONICAL_HOST}/scl/fi/{id}/{name}?rlkey={rlkey}"),
                password,
            })
        }
        ["sh", key, hash, rest @ ..] if valid_key(key) && valid_key(hash) => {
            Some(Address::SharedFolder {
                link: format!("https://{CANONICAL_HOST}/sh/{key}/{hash}"),
                sub_path: sub_path(rest)?,
                preview,
                password,
            })
        }
        ["scl", "fo", id, hash, rest @ ..] if valid_key(id) && valid_key(hash) => {
            let rlkey = rlkey?;
            Some(Address::SharedFolder {
                link: format!("https://{CANONICAL_HOST}/scl/fo/{id}/{hash}?rlkey={rlkey}"),
                sub_path: sub_path(rest)?,
                preview,
                password,
            })
        }
        ["home", rest @ ..] => Some(Address::Own {
            path: sub_path(rest)?,
            preview,
        }),
        // `/preview/<folder>/<name>`: the folder and the name in one route.
        ["preview", rest @ .., name] => Some(Address::Own {
            path: sub_path(rest)?,
            preview: Some(percent_decode(name).filter(|name| valid_name(name))?),
        }),
        _ => None,
    }
}

/// Decodes route segments into an API path: empty for none, `/a/b` otherwise.
fn sub_path(segments: &[&str]) -> Option<String> {
    let mut path = String::new();
    for segment in segments {
        let name = percent_decode(segment)?;
        if !valid_name(&name) {
            return None;
        }
        path.push('/');
        path.push_str(&name);
    }
    Some(path)
}

/// Encodes an API path as route segments: `/a b/c` becomes `/a%20b/c`.
fn encode_path(path: &str) -> String {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| format!("/{}", percent_encode(segment)))
        .collect()
}

/// The canonical address for one file in the account's own Dropbox, and the one the crawler
/// hands the resolver: `https://www.dropbox.com/home/<folder>?preview=<name>` — the address
/// Dropbox's own web interface shows for a previewed file.
///
/// `folder` is the API path of the folder the file sits in, empty for the root.
#[must_use]
pub fn file_address(folder: &str, name: &str) -> String {
    format!(
        "https://{CANONICAL_HOST}/home{}?preview={}",
        encode_path(folder),
        percent_encode(name)
    )
}

/// The canonical address for one file inside a shared folder link: the link itself, the
/// sub-folder below its root, and the file's name as `preview`. A password travels along, so a
/// crawled file can be resolved the same way the folder was.
#[must_use]
pub fn shared_file_address(
    link: &str,
    sub_path: &str,
    name: &str,
    password: Option<&str>,
) -> String {
    let (base, query) = link.split_once('?').unwrap_or((link, ""));
    let mut address = format!("{base}{}?", encode_path(sub_path));
    if !query.is_empty() {
        address.push_str(query);
        address.push('&');
    }
    address.push_str("preview=");
    address.push_str(&percent_encode(name));
    if let Some(password) = password {
        address.push_str("&link_password=");
        address.push_str(&percent_encode(password));
    }
    address
}

/// The `path` argument for a file inside a shared folder: the sub-folder plus the name.
#[must_use]
pub fn shared_file_path(sub_path: &str, name: &str) -> String {
    format!("{sub_path}/{name}")
}

#[cfg(test)]
mod tests {
    use super::{
        Address, dropbox_host, file_address, parse, percent_decode, percent_encode, query_value,
        shared_file_address, split, valid_content_hash, valid_id, valid_key, valid_name, valid_rev,
    };

    #[test]
    fn only_dropboxs_own_hosts_are_dropboxs() {
        assert_eq!(dropbox_host("www.dropbox.com"), Some("www.dropbox.com"));
        assert_eq!(dropbox_host("WWW.DROPBOX.COM."), Some("www.dropbox.com"));
        assert_eq!(
            dropbox_host("dl.dropboxusercontent.com"),
            Some("dl.dropboxusercontent.com")
        );
        assert_eq!(dropbox_host("dropbox.com.evil.test"), None);
        assert_eq!(dropbox_host("evil-dropbox.com"), None);
    }

    #[test]
    fn an_authority_carrying_credentials_is_refused() {
        assert_eq!(split("https://x@www.dropbox.com/s/a/b"), None);
        assert_eq!(split("ftp://www.dropbox.com/s/a/b"), None);
        assert_eq!(
            split("https://www.dropbox.com:443/s/a/b?dl=0#x"),
            Some(("www.dropbox.com", "s/a/b?dl=0"))
        );
    }

    #[test]
    fn a_query_value_is_read_without_decoding_it() {
        assert_eq!(query_value("s/a/b?dl=0&rlkey=abc", "rlkey"), Some("abc"));
        assert_eq!(query_value("s/a/b", "rlkey"), None);
    }

    #[test]
    fn percent_encoding_round_trips_a_name_a_stranger_chose() {
        let name = "Folge 01 — Ωmega/…";
        assert_eq!(percent_decode(&percent_encode(name)).as_deref(), Some(name));
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_decode("a%2"), None);
        assert_eq!(percent_decode("%FF"), None);
    }

    #[test]
    fn identifiers_that_are_not_ones_are_refused() {
        assert!(valid_key("AbC-1_2"));
        assert!(!valid_key("a/b"));
        assert!(!valid_key(""));
        assert!(!valid_key(&"x".repeat(129)));
        assert!(valid_name("Season 1"));
        assert!(!valid_name(".."));
        assert!(!valid_name("a\u{0}b"));
        assert!(!valid_name(""));
        assert!(valid_rev("015f3d2a1b2c3d4e5f6a7"));
        assert!(!valid_rev("short"));
        assert!(!valid_rev("../etc"));
        assert!(valid_content_hash(&"a".repeat(64)));
        assert!(!valid_content_hash(&"a".repeat(63)));
        assert!(valid_id("id:AbC123"));
        assert!(!valid_id("AbC123"));
    }

    #[test]
    fn every_spelling_of_a_shared_file_link_is_read() {
        assert_eq!(
            parse("https://www.dropbox.com/s/abc123/release.bin?dl=0"),
            Some(Address::SharedFile {
                link: "https://www.dropbox.com/s/abc123/release.bin".to_owned(),
                password: None,
            })
        );
        assert_eq!(
            parse(
                "https://www.dropbox.com/scl/fi/abc123/release.bin?rlkey=k1&st=tracking&dl=1&link_password=pw%201"
            ),
            Some(Address::SharedFile {
                link: "https://www.dropbox.com/scl/fi/abc123/release.bin?rlkey=k1".to_owned(),
                password: Some("pw 1".to_owned()),
            })
        );
        // The direct-download host is the same link.
        assert_eq!(
            parse("https://dl.dropboxusercontent.com/s/abc123/release.bin"),
            Some(Address::SharedFile {
                link: "https://www.dropbox.com/s/abc123/release.bin".to_owned(),
                password: None,
            })
        );
        // A current link without its `rlkey` is not a link.
        assert_eq!(
            parse("https://www.dropbox.com/scl/fi/abc123/release.bin"),
            None
        );
    }

    #[test]
    fn a_shared_folder_link_carries_its_sub_folder_and_its_preview() {
        assert_eq!(
            parse("https://www.dropbox.com/sh/abc/h1?dl=0"),
            Some(Address::SharedFolder {
                link: "https://www.dropbox.com/sh/abc/h1".to_owned(),
                sub_path: String::new(),
                preview: None,
                password: None,
            })
        );
        assert_eq!(
            parse(
                "https://www.dropbox.com/scl/fo/abc/h1/Season%201?rlkey=k1&preview=e01.mkv&password=secret"
            ),
            Some(Address::SharedFolder {
                link: "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1".to_owned(),
                sub_path: "/Season 1".to_owned(),
                preview: Some("e01.mkv".to_owned()),
                password: Some("secret".to_owned()),
            })
        );
        assert_eq!(parse("https://www.dropbox.com/sh/abc/h1/../x"), None);
    }

    #[test]
    fn an_address_in_the_accounts_own_dropbox_is_read() {
        assert_eq!(
            parse("https://www.dropbox.com/home/Show/Season%201?preview=e01.mkv"),
            Some(Address::Own {
                path: "/Show/Season 1".to_owned(),
                preview: Some("e01.mkv".to_owned()),
            })
        );
        assert_eq!(
            parse("https://www.dropbox.com/home"),
            Some(Address::Own {
                path: String::new(),
                preview: None,
            })
        );
        assert_eq!(
            parse("https://www.dropbox.com/preview/Show/e01.mkv"),
            Some(Address::Own {
                path: "/Show".to_owned(),
                preview: Some("e01.mkv".to_owned()),
            })
        );
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_read() {
        assert_eq!(parse("https://dropbox.com.evil.test/s/abc/x.bin"), None);
        assert_eq!(parse("https://x@www.dropbox.com/s/abc/x.bin"), None);
        assert_eq!(parse("https://ddownload.com/f/abc"), None);
        assert_eq!(parse("https://www.dropbox.com/"), None);
        assert_eq!(parse("https://www.dropbox.com/login"), None);
    }

    /// The addresses the crawler hands the resolver are ones the resolver reads back to the
    /// same file.
    #[test]
    fn the_canonical_addresses_round_trip() {
        let own = file_address("/Show/Season 1", "e01 — final.mkv");
        assert_eq!(
            own,
            "https://www.dropbox.com/home/Show/Season%201?preview=e01%20%E2%80%94%20final.mkv"
        );
        assert_eq!(
            parse(&own),
            Some(Address::Own {
                path: "/Show/Season 1".to_owned(),
                preview: Some("e01 — final.mkv".to_owned()),
            })
        );
        let root = file_address("", "readme.txt");
        assert_eq!(root, "https://www.dropbox.com/home?preview=readme.txt");

        let link = "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1";
        let shared = shared_file_address(link, "/Season 1", "e01.mkv", Some("pw"));
        assert_eq!(
            shared,
            "https://www.dropbox.com/scl/fo/abc/h1/Season%201?rlkey=k1&preview=e01.mkv&link_password=pw"
        );
        assert_eq!(
            parse(&shared),
            Some(Address::SharedFolder {
                link: link.to_owned(),
                sub_path: "/Season 1".to_owned(),
                preview: Some("e01.mkv".to_owned()),
                password: Some("pw".to_owned()),
            })
        );
        let legacy = shared_file_address("https://www.dropbox.com/sh/abc/h1", "", "a.bin", None);
        assert_eq!(legacy, "https://www.dropbox.com/sh/abc/h1?preview=a.bin");
    }
}
