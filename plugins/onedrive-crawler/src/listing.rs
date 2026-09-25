//! Reading what `/children` and a single `driveItem` answered.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it without
//! a WebAssembly toolchain, which is the only place a listing's odd shapes — a row with no id,
//! a OneNote package between the files, a `@odata.nextLink` to the next page — can be pinned
//! down.

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
        name: String,
        size: Option<u64>,
    },
}

/// Graph states sizes as numbers; being lenient costs nothing.
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

/// One row of `/children`, and the whole of a single-item answer.
#[derive(Debug, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<Flexible>,
    #[serde(default)]
    pub file: Option<serde_json::Value>,
    #[serde(default)]
    pub folder: Option<serde_json::Value>,
    #[serde(default)]
    pub package: Option<serde_json::Value>,
    #[serde(default)]
    pub deleted: Option<serde_json::Value>,
}

/// `GET …/children`.
#[derive(Debug, Deserialize)]
pub struct Page {
    /// The whole address of the next page, as Graph spells it. Followed as given — the
    /// `$skiptoken` in it is opaque — but only when it is still on Graph.
    #[serde(rename = "@odata.nextLink", default)]
    pub next_link: Option<String>,
    #[serde(default)]
    pub value: Vec<Item>,
}

impl Item {
    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.folder.is_some()
    }

    #[must_use]
    pub fn is_file(&self) -> bool {
        self.file.is_some() && self.folder.is_none()
    }

    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.deleted.is_some()
    }

    /// Turns one row into an entry, or drops it.
    ///
    /// Dropped rather than reported: a single unusable row of a folder is not a reason to
    /// refuse the whole folder, and a person who pasted it cannot fix Graph's answer. What is
    /// dropped is a row with no id, a tombstone, and a OneNote package — a notebook has no
    /// bytes to download, and offering it would produce a queue entry that can only fail.
    #[must_use]
    pub fn entry(self) -> Option<Entry> {
        if self.is_deleted() {
            return None;
        }
        let id = self.id.clone().filter(|id| !id.is_empty())?;
        let name = self.name.clone().unwrap_or_default();
        if self.is_folder() {
            return Some(Entry::Folder { id, name });
        }
        if !self.is_file() {
            return None;
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
      "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#driveItem",
      "@odata.nextLink": "https://graph.microsoft.com/v1.0/shares/u!aHR0/driveItem/children?$skiptoken=abc",
      "value": [
        {"id": "f1", "name": "Extras", "folder": {"childCount": 3}},
        {"id": "x1", "name": "e01.mkv", "size": 1024, "file": {"mimeType": "video/x-matroska"}},
        {"id": "x2", "name": "Notes", "package": {"type": "oneNote"}},
        {"id": "x3", "name": "old.mkv", "file": {}, "deleted": {"state": "deleted"}},
        {"name": "nameless", "file": {}}
      ]
    }"#;

    #[test]
    fn a_page_yields_its_folders_and_its_files_and_says_where_the_next_one_is() {
        let page = page(LISTING).expect("a page");
        assert_eq!(
            page.next_link.as_deref(),
            Some(
                "https://graph.microsoft.com/v1.0/shares/u!aHR0/driveItem/children?$skiptoken=abc"
            )
        );
        let entries: Vec<Entry> = page
            .value
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
            ],
            "the notebook, the tombstone and the row with no id are dropped, not guessed"
        );
    }

    #[test]
    fn a_single_item_says_what_it_is() {
        let folder =
            item(br#"{"id":"root","name":"Season 1","folder":{"childCount":2}}"#).expect("an item");
        assert!(folder.is_folder());
        assert_eq!(folder.name.as_deref(), Some("Season 1"));
        let file = item(br#"{"id":"x","name":"a.bin","size":5,"file":{}}"#).expect("an item");
        assert!(file.is_file());
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_folder() {
        assert!(page(b"<html>502 Bad Gateway</html>").is_none());
        assert!(item(b"").is_none());
    }
}
