//! Reading what the Dropbox API answered.
//!
//! Shared because the same record comes back from four endpoints — `files/get_metadata`,
//! `files/list_folder`, `files/list_folder/continue` and `sharing/get_shared_link_metadata` —
//! and the resolver and the crawler have to read a size, a revision and a `content_hash` the
//! same way. Kept apart from the components so `cargo test` covers the odd shapes without a
//! WebAssembly toolchain: an entry with no `.tag`, a deleted entry, a hash that is not one.

use serde::Deserialize;

use crate::address::{valid_content_hash, valid_id, valid_rev};

/// One file, folder or deleted entry, as every metadata endpoint answers it.
#[derive(Debug, Default, Deserialize)]
pub struct Metadata {
    /// `file`, `folder` or `deleted`.
    #[serde(rename = ".tag", default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub content_hash: Option<String>,
    /// `false` for a file Dropbox will not serve bytes for — a Paper document, a Google Doc
    /// living in Dropbox.
    #[serde(default)]
    pub is_downloadable: Option<bool>,
    /// The path as the owner spelled it; absent inside a shared link.
    #[serde(default)]
    pub path_display: Option<String>,
}

impl Metadata {
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.tag.as_deref() == Some("file")
    }

    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.tag.as_deref() == Some("folder")
    }

    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.tag.as_deref() == Some("deleted")
    }

    /// Whether Dropbox will serve the bytes. Absent means yes: the field is only present on
    /// files, and a listing that omitted it would otherwise refuse every entry.
    #[must_use]
    pub fn downloadable(&self) -> bool {
        self.is_downloadable.unwrap_or(true)
    }

    /// The `content_hash` Dropbox stated, when it has the shape of one — the value is passed
    /// on to the checksum verifier.
    #[must_use]
    pub fn content_hash(&self) -> Option<String> {
        self.content_hash
            .as_deref()
            .filter(|hash| valid_content_hash(hash))
            .map(str::to_ascii_lowercase)
    }

    /// The revision, when it has the shape of one.
    #[must_use]
    pub fn revision(&self) -> Option<&str> {
        self.rev.as_deref().filter(|rev| valid_rev(rev))
    }

    /// The id, when it has the shape of one.
    #[must_use]
    pub fn identifier(&self) -> Option<&str> {
        self.id.as_deref().filter(|id| valid_id(id))
    }

    /// The name, or empty.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or_default()
    }
}

/// `files/list_folder` and `files/list_folder/continue`.
#[derive(Debug, Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub entries: Vec<Metadata>,
    /// The cursor the next page is asked for with. Stated even on the last page.
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub has_more: bool,
}

/// `users/get_current_account`.
#[derive(Debug, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<AccountName>,
}

#[derive(Debug, Deserialize)]
pub struct AccountName {
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Reads one metadata answer, or `None` when the document is not that.
#[must_use]
pub fn item(body: &[u8]) -> Option<Metadata> {
    serde_json::from_slice(body).ok()
}

/// Reads one page of a listing, or `None`.
#[must_use]
pub fn listing(body: &[u8]) -> Option<Listing> {
    serde_json::from_slice(body).ok()
}

/// Reads an account answer, or `None`.
#[must_use]
pub fn account(body: &[u8]) -> Option<Account> {
    serde_json::from_slice(body).ok()
}

#[cfg(test)]
mod tests {
    use super::{account, item, listing};

    const FILE: &[u8] = br#"{
      ".tag": "file", "name": "release.bin", "path_lower": "/show/release.bin",
      "path_display": "/Show/release.bin", "id": "id:a1b2C3d4E5f6G7h8I9j0K",
      "client_modified": "2026-01-02T03:04:05Z", "server_modified": "2026-01-02T03:04:06Z",
      "rev": "015f3d2a1b2c3d4e5f6a7", "size": 1048576, "is_downloadable": true,
      "content_hash": "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
    }"#;
    // The fixture keeps the fields Dropbox actually sends, including the ones this crate does
    // not read: a parser that broke on an unexpected field would break on Dropbox's next one.

    #[test]
    fn a_file_is_read_with_its_size_its_revision_and_its_hash_lowercased() {
        let file = item(FILE).expect("metadata");
        assert!(file.is_file() && !file.is_folder());
        assert_eq!(file.name(), "release.bin");
        assert_eq!(file.size, Some(1_048_576));
        assert_eq!(file.revision(), Some("015f3d2a1b2c3d4e5f6a7"));
        assert_eq!(file.identifier(), Some("id:a1b2C3d4E5f6G7h8I9j0K"));
        assert_eq!(
            file.content_hash().as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert!(file.downloadable());
    }

    /// A hash, a revision or an id that is not one never reaches a request or the verifier.
    #[test]
    fn values_that_do_not_have_the_shape_dropbox_issues_are_dropped() {
        let odd = item(
            br#"{".tag":"file","name":"a","rev":"../x","content_hash":"not a hash","id":"x"}"#,
        )
        .expect("metadata");
        assert_eq!(odd.content_hash(), None);
        assert_eq!(odd.revision(), None);
        assert_eq!(odd.identifier(), None);
        assert!(odd.downloadable(), "absent means not asked, never refused");
        let paper =
            item(br#"{".tag":"file","name":"Notes","is_downloadable":false}"#).expect("metadata");
        assert!(!paper.downloadable());
    }

    #[test]
    fn a_listing_carries_its_cursor_and_says_whether_there_is_more() {
        let page = listing(
            br#"{"entries":[{".tag":"folder","name":"Extras","id":"id:f1"},
                 {".tag":"file","name":"e01.mkv","size":1024,"id":"id:x1"},
                 {".tag":"deleted","name":"old.mkv"}],
                 "cursor":"AAE_redacted","has_more":true}"#,
        )
        .expect("listing");
        assert_eq!(page.entries.len(), 3);
        assert!(page.entries[0].is_folder());
        assert!(page.entries[2].is_deleted());
        assert_eq!(page.cursor.as_deref(), Some("AAE_redacted"));
        assert!(page.has_more);
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_answer() {
        assert!(item(b"<html>502 Bad Gateway</html>").is_none());
        assert!(listing(b"").is_none());
        assert!(account(b"Error in call to API function").is_none());
    }

    #[test]
    fn an_account_answer_carries_the_address_the_person_signed_in_with() {
        let answer = account(
            br#"{"account_id":"dbid:redacted","email":"someone@example.invalid",
                 "name":{"display_name":"Someone"}}"#,
        )
        .expect("account");
        assert_eq!(answer.email.as_deref(), Some("someone@example.invalid"));
    }
}
