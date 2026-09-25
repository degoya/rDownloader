//! Reading what the pCloud API answered.
//!
//! Shared because the same record comes back from five endpoints — `stat`, `listfolder`,
//! `showpublink`, `checksumfile` and `getfilelink` — and the resolver and the crawler have to
//! read a size, a `fileid` and a checksum the same way. Kept apart from the components so
//! `cargo test` covers the odd shapes without a WebAssembly toolchain: a folder with no
//! `contents`, a digest that is not one, a download host that is not pCloud's.

use serde::Deserialize;

use crate::address::{valid_digest, valid_name};

/// One file or folder, as every metadata endpoint answers it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub isfolder: bool,
    #[serde(default)]
    pub fileid: Option<u64>,
    #[serde(default)]
    pub folderid: Option<u64>,
    #[serde(default)]
    pub parentfolderid: Option<u64>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub contenttype: Option<String>,
    /// RFC 1123, as pCloud writes it. Carried for completeness; the transfer takes the
    /// modified time off the content host's own `Last-Modified`, because the plugin contract
    /// has no field for one.
    #[serde(default)]
    pub modified: Option<String>,
    /// A folder's children. `listfolder` fills it one level deep; `showpublink` fills it for
    /// the whole tree at once.
    #[serde(default)]
    pub contents: Vec<Metadata>,
}

impl Metadata {
    /// The name, or empty.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or_default()
    }

    /// Whether this row is a file with a usable name and a `fileid`.
    #[must_use]
    pub fn is_file(&self) -> bool {
        !self.isfolder && self.fileid.is_some() && valid_name(self.name())
    }

    /// Whether this row is a folder with a usable name and a `folderid`.
    ///
    /// The root of an own drive has `folderid: 0` and, at `showpublink`, sometimes no name at
    /// all; the caller names it rather than this.
    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.isfolder && self.folderid.is_some()
    }
}

/// `getfilelink` and `getpublinkdownload`.
///
/// The one pCloud answer that is **not** stable: `hosts` plus `path` is a ticket, and
/// `expires` says when it stops being one. Nothing built from it is ever written down — see
/// the resolver's module comment.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Link {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
    /// RFC 1123, as pCloud writes it.
    #[serde(default)]
    pub expires: Option<String>,
}

impl Link {
    /// The address the bytes come from, or `None` when pCloud named a host that is not its
    /// own.
    ///
    /// The check is the point. A resolver that answered with somebody else's host would send
    /// the transfer there, so the host has to be one of pCloud's content servers — a
    /// sub-domain of `pcloud.com`, over TLS, and nothing that could smuggle a second address
    /// into the string.
    #[must_use]
    pub fn download_url(&self) -> Option<String> {
        let path = self.path.as_deref().filter(|path| path.starts_with('/'))?;
        if path.contains(['#', '\\', ' ']) || path.contains("//") {
            return None;
        }
        let host = self
            .hosts
            .iter()
            .find(|host| is_content_host(host))
            .map(String::as_str)?;
        Some(format!("https://{host}{path}"))
    }
}

/// Whether `host` is one of pCloud's own content servers.
#[must_use]
pub fn is_content_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    !host.is_empty()
        && host.len() <= 128
        && host.ends_with(".pcloud.com")
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
        && !host.contains("..")
}

/// `checksumfile`.
///
/// pCloud answers with different algorithms in its two installations, which it documents:
/// `sha1` everywhere, `sha256` only in Europe, `md5` only in the United States. So the
/// algorithm a file arrives with depends on where the account lives, and the strongest one on
/// offer is taken rather than a fixed one being demanded.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Checksums {
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub md5: Option<String>,
}

impl Checksums {
    /// The strongest digest pCloud stated that has the shape of one, as
    /// `(algorithm, lowercase value)`.
    #[must_use]
    pub fn best(&self) -> Option<(&'static str, String)> {
        for (algorithm, value, length) in [
            ("sha256", self.sha256.as_deref(), 64),
            ("sha1", self.sha1.as_deref(), 40),
            ("md5", self.md5.as_deref(), 32),
        ] {
            if let Some(value) = value.filter(|value| valid_digest(value, length)) {
                return Some((algorithm, value.to_ascii_lowercase()));
            }
        }
        None
    }
}

/// `userinfo`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub premium: bool,
}

/// Reads one answer of type `T`, or `None` when the document is not that.
#[must_use]
pub fn read<T: serde::de::DeserializeOwned>(body: &[u8]) -> Option<T> {
    serde_json::from_slice(body).ok()
}

/// Reads the `metadata` member of an answer that wraps one — `stat`, `listfolder`,
/// `showpublink` and `checksumfile` all do.
#[must_use]
pub fn item(body: &[u8]) -> Option<Metadata> {
    let document: serde_json::Value = serde_json::from_slice(body).ok()?;
    serde_json::from_value(document.get("metadata")?.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::{Checksums, Link, Metadata, is_content_host, item, read};

    // The fixtures keep the fields pCloud actually sends, including the ones this crate does
    // not read: a parser that broke on an unexpected field would break on pCloud's next one.
    const FILE: &[u8] = br#"{"result":0,"metadata":{
      "name":"release.bin","created":"Fri, 02 Jan 2026 03:04:05 +0000","ismine":true,
      "thumb":false,"modified":"Fri, 02 Jan 2026 03:04:06 +0000","isfolder":false,
      "fileid":123456,"hash":9876543210,"category":0,"id":"f123456","isshared":false,
      "size":1048576,"contenttype":"application/octet-stream","parentfolderid":42}}"#;

    const FOLDER: &[u8] = br#"{"result":0,"metadata":{
      "name":"Show","isfolder":true,"folderid":42,"parentfolderid":0,"id":"d42",
      "contents":[
        {"name":"Season 1","isfolder":true,"folderid":43,"parentfolderid":42,"id":"d43"},
        {"name":"readme.txt","isfolder":false,"fileid":7,"size":12,"parentfolderid":42},
        {"name":"..","isfolder":false,"fileid":8,"size":1,"parentfolderid":42},
        {"name":"nameless","isfolder":false,"size":1,"parentfolderid":42}
      ]}}"#;

    #[test]
    fn a_file_is_read_with_its_identifier_its_size_and_its_folder() {
        let file = item(FILE).expect("metadata");
        assert!(file.is_file() && !file.is_folder());
        assert_eq!(file.name(), "release.bin");
        assert_eq!(file.fileid, Some(123_456));
        assert_eq!(file.size, Some(1_048_576));
        assert_eq!(file.parentfolderid, Some(42));
        assert_eq!(
            file.modified.as_deref(),
            Some("Fri, 02 Jan 2026 03:04:06 +0000")
        );
    }

    /// A row without a usable name or without the identifier its kind needs is not that kind.
    /// Dropped rather than guessed: a dot entry would climb out of the path it belongs to, and
    /// a file with no `fileid` cannot be asked for.
    #[test]
    fn rows_that_do_not_have_the_shape_pcloud_issues_are_not_files_or_folders() {
        let folder = item(FOLDER).expect("metadata");
        assert!(folder.is_folder());
        assert_eq!(folder.folderid, Some(42));
        assert_eq!(folder.contents.len(), 4);
        assert!(folder.contents[0].is_folder());
        assert!(folder.contents[1].is_file());
        assert!(!folder.contents[2].is_file(), "a dot entry is not a name");
        assert!(
            !folder.contents[3].is_file(),
            "no fileid, nothing to ask for"
        );
        let empty = Metadata::default();
        assert!(!empty.is_file() && !empty.is_folder());
    }

    /// The download ticket is built only from a host that is pCloud's own.
    #[test]
    fn a_download_address_is_built_only_from_pclouds_own_content_hosts() {
        let link = Link {
            hosts: vec!["edef2.pcloud.com".to_owned(), "evd4.pcloud.com".to_owned()],
            path: Some("/cBZredacted/release.bin".to_owned()),
            expires: Some("Fri, 02 Jan 2026 09:04:06 +0000".to_owned()),
        };
        assert_eq!(
            link.download_url().as_deref(),
            Some("https://edef2.pcloud.com/cBZredacted/release.bin")
        );
        // A host that is not pCloud's is skipped, and one that is nobody's is no address.
        let foreign = Link {
            hosts: vec!["evil.test".to_owned(), "pcloud.com.evil.test".to_owned()],
            path: Some("/x".to_owned()),
            expires: None,
        };
        assert_eq!(foreign.download_url(), None);
        assert!(is_content_host("EDEF2.PCLOUD.COM."));
        assert!(!is_content_host("pcloud.com"));
        assert!(!is_content_host("a@b.pcloud.com"));
        assert!(!is_content_host("a..b.pcloud.com"));
        // A path that could smuggle a second address into the string is no address either.
        for path in ["x", "/a//b", "/a#b", "/a b", "/a\\b"] {
            assert_eq!(
                Link {
                    hosts: vec!["edef2.pcloud.com".to_owned()],
                    path: Some(path.to_owned()),
                    expires: None,
                }
                .download_url(),
                None,
                "{path}"
            );
        }
    }

    /// Europe answers `sha256`, the United States `md5`; both answer `sha1`. The strongest on
    /// offer is taken, and a value that is not a digest is not one.
    #[test]
    fn the_strongest_digest_pcloud_stated_is_the_one_that_travels() {
        let europe: Checksums = read(
            br#"{"result":0,"sha1":"0000000000000000000000000000000000000001",
                 "sha256":"E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"}"#,
        )
        .expect("checksums");
        assert_eq!(
            europe.best(),
            Some((
                "sha256",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned()
            ))
        );
        let united_states: Checksums = read(
            br#"{"result":0,"sha1":"0000000000000000000000000000000000000001",
                 "md5":"00000000000000000000000000000002"}"#,
        )
        .expect("checksums");
        assert_eq!(
            united_states.best(),
            Some((
                "sha1",
                "0000000000000000000000000000000000000001".to_owned()
            ))
        );
        let nonsense: Checksums = read(br#"{"result":0,"sha1":"not a digest"}"#).expect("c");
        assert_eq!(nonsense.best(), None);
        assert_eq!(Checksums::default().best(), None);
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_answer() {
        assert!(item(b"<html>502 Bad Gateway</html>").is_none());
        assert!(item(br#"{"result":2009,"error":"File not found."}"#).is_none());
        assert!(read::<Link>(b"").is_none());
    }
}
