//! Reading what `files.list` and `files.get` answered.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it without
//! a WebAssembly toolchain, which is the only place a listing's odd shapes — a size quoted as a
//! string, a row with no id, a shortcut pointing at something else — can be pinned down.

use google_drive_common::export;
use serde::Deserialize;

/// One entry of a listing: either something to walk into, or a file to hand back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Folder {
        id: String,
        name: String,
    },
    File {
        id: String,
        /// The name this file will arrive under — already carrying the export extension when
        /// it is a Workspace document, because the LinkGrabber is where somebody has to be
        /// able to see that their spreadsheet is about to become a `.xlsx`.
        name: String,
        /// `None` for a Workspace document: the bytes do not exist until the export runs, and
        /// Drive states no size for one.
        size: Option<u64>,
    },
}

/// Drive quotes byte counts as JSON strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Flexible {
    Number(u64),
    Text(String),
}

impl Flexible {
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
        }
    }
}

/// One row of `files.list`, and the whole of a `files.get` answer.
#[derive(Debug, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size: Option<Flexible>,
    #[serde(default)]
    pub trashed: Option<bool>,
}

/// `GET /drive/v3/files?q=…`.
#[derive(Debug, Deserialize)]
pub struct Page {
    #[serde(rename = "nextPageToken", default)]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub files: Vec<Item>,
}

impl Item {
    /// Turns one row into an entry, or drops it.
    ///
    /// Dropped rather than reported: a single unusable row of a folder is not a reason to
    /// refuse the whole folder, and a person who pasted it cannot fix Google's answer. What is
    /// dropped is a row with no id, a row Drive already put in the bin, and a document type
    /// Drive exports nothing for — a Form has no bytes at all, and offering it as a download
    /// would produce a queue entry that can only ever fail.
    #[must_use]
    pub fn entry(self) -> Option<Entry> {
        if self.trashed == Some(true) {
            return None;
        }
        let id = self.id.filter(|id| !id.is_empty())?;
        let name = self.name.unwrap_or_default();
        let mime = self.mime_type.unwrap_or_default();
        if export::is_folder(&mime) {
            return Some(Entry::Folder { id, name });
        }
        if export::is_workspace_document(&mime) {
            // The default format for this document type. A crawled folder carries no address a
            // person could have written `?format=` into, so there is nothing else to honour —
            // and a single file pasted by hand still can.
            let chosen = export::resolve(&mime, None).ok()?;
            return Some(Entry::File {
                id,
                name: export::export_name(&name, &chosen),
                size: None,
            });
        }
        Some(Entry::File {
            id,
            name,
            size: self.size.as_ref().and_then(Flexible::as_u64),
        })
    }
}

/// Reads one page of a listing, or `None` when the document is not one.
#[must_use]
pub fn page(body: &[u8]) -> Option<Page> {
    serde_json::from_slice(body).ok()
}

/// Reads a single item, or `None` when the document is not one.
#[must_use]
pub fn item(body: &[u8]) -> Option<Item> {
    serde_json::from_slice(body).ok()
}

#[cfg(test)]
mod tests {
    use super::{Entry, item, page};

    const LISTING: &[u8] = br#"{
      "nextPageToken": "CAESBgiA",
      "files": [
        {"id": "f1", "name": "Extras", "mimeType": "application/vnd.google-apps.folder"},
        {"id": "x1", "name": "e01.mkv", "mimeType": "video/x-matroska", "size": "1024"},
        {"id": "x2", "name": "Quarterly numbers",
         "mimeType": "application/vnd.google-apps.spreadsheet"},
        {"id": "x3", "name": "Signup", "mimeType": "application/vnd.google-apps.form"},
        {"id": "x4", "name": "old.mkv", "mimeType": "video/x-matroska", "trashed": true},
        {"name": "nameless", "mimeType": "video/x-matroska"}
      ]
    }"#;

    #[test]
    fn a_page_yields_its_folders_and_its_files_and_says_there_is_more() {
        let page = page(LISTING).expect("a page");
        assert_eq!(page.next_page_token.as_deref(), Some("CAESBgiA"));
        let entries: Vec<Entry> = page
            .files
            .into_iter()
            .filter_map(super::Item::entry)
            .collect();
        assert_eq!(
            entries,
            vec![
                Entry::Folder {
                    id: "f1".to_owned(),
                    name: "Extras".to_owned(),
                },
                Entry::File {
                    id: "x1".to_owned(),
                    name: "e01.mkv".to_owned(),
                    size: Some(1024),
                },
                // The extension the export will produce, visible before anything is queued.
                Entry::File {
                    id: "x2".to_owned(),
                    name: "Quarterly numbers.xlsx".to_owned(),
                    size: None,
                },
            ],
            "the Form, the binned file and the row with no id are dropped, not guessed"
        );
    }

    #[test]
    fn a_single_item_carries_the_folders_own_name() {
        let folder = item(
            br#"{"id":"root","name":"Season 1","mimeType":"application/vnd.google-apps.folder"}"#,
        )
        .expect("an item");
        assert_eq!(folder.name.as_deref(), Some("Season 1"));
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_folder() {
        assert!(page(b"<html>502 Bad Gateway</html>").is_none());
        assert!(item(b"").is_none());
    }
}
