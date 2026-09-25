//! Reading what `/2.0/folders/<id>/items`, `/2.0/folders/<id>` and `/2.0/shared_items` answered.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it without
//! a WebAssembly toolchain, which is the only place a listing's odd shapes — a row with no id,
//! a bookmark between the files, a page that says there are more — can be pinned down.

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

/// Box states sizes as numbers; being lenient costs nothing.
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

/// One row of `/items`, and the whole of a single-item answer.
#[derive(Debug, Default, Deserialize)]
pub struct Item {
    /// `file`, `folder` or `web_link`.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<Flexible>,
    /// `active`, `trashed` or `deleted`.
    #[serde(default)]
    pub item_status: Option<String>,
}

/// `GET …/items`, in the offset pagination Box answers it with.
#[derive(Debug, Default, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub total_count: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub entries: Vec<Item>,
}

impl Item {
    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.kind.as_deref() == Some("folder")
    }

    #[must_use]
    pub fn is_file(&self) -> bool {
        self.kind.as_deref() == Some("file")
    }

    #[must_use]
    pub fn is_gone(&self) -> bool {
        matches!(self.item_status.as_deref(), Some("trashed" | "deleted"))
    }

    /// Turns one row into an entry, or drops it.
    ///
    /// Dropped rather than reported: a single unusable row of a folder is not a reason to refuse
    /// the whole folder, and a person who pasted it cannot fix Box's answer. What is dropped is
    /// a row with no id, a trashed item, and a bookmark — a `web_link` has no bytes to download,
    /// and offering it would produce a queue entry that can only fail.
    #[must_use]
    pub fn entry(self) -> Option<Entry> {
        if self.is_gone() {
            return None;
        }
        let id = self.id.clone().filter(|id| !id.is_empty())?;
        if !box_common::address::valid_id(&id) {
            // An id that is not one cannot be put back into a request path, and guessing at it
            // would be worse than leaving the row out.
            return None;
        }
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

impl Page {
    /// The offset the next page starts at, or `None` when this was the last one.
    ///
    /// Read from Box's own three numbers rather than counted here: a page that answered fewer
    /// rows than it was asked for is still not the end if Box says the folder holds more, and a
    /// page that answered none is, whatever the count says.
    #[must_use]
    pub fn next_offset(&self) -> Option<u64> {
        if self.entries.is_empty() {
            return None;
        }
        let offset = self.offset.unwrap_or_default();
        let next = offset.checked_add(self.entries.len() as u64)?;
        (next < self.total_count?).then_some(next)
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
      "total_count": 7,
      "offset": 0,
      "limit": 5,
      "entries": [
        {"type":"folder","id":"11","name":"Extras","item_status":"active"},
        {"type":"file","id":"12","name":"e01.mkv","size":1024},
        {"type":"web_link","id":"13","name":"a bookmark"},
        {"type":"file","id":"14","name":"old.mkv","size":1,"item_status":"trashed"},
        {"type":"file","name":"nameless"}
      ]
    }"#;

    #[test]
    fn a_page_yields_its_folders_and_its_files_and_says_where_the_next_one_starts() {
        let page = page(LISTING).expect("a page");
        assert_eq!(page.next_offset(), Some(5));
        let entries: Vec<Entry> = page
            .entries
            .into_iter()
            .filter_map(super::Item::entry)
            .collect();
        assert_eq!(
            entries,
            vec![
                Entry::Folder {
                    id: "11".to_owned(),
                    name: "Extras".to_owned(),
                },
                Entry::File {
                    id: "12".to_owned(),
                    name: "e01.mkv".to_owned(),
                    size: Some(1024),
                },
            ],
            "the bookmark, the trashed file and the row with no id are dropped, not guessed"
        );
    }

    #[test]
    fn the_last_page_says_it_is_the_last() {
        let last = page(br#"{"total_count":2,"offset":1,"limit":1,"entries":[{"type":"file","id":"2","name":"b"}]}"#)
            .expect("a page");
        assert_eq!(last.next_offset(), None);
        // A page with nothing on it ends the walk whatever the count claims.
        let empty =
            page(br#"{"total_count":99,"offset":0,"limit":100,"entries":[]}"#).expect("a page");
        assert_eq!(empty.next_offset(), None);
        // And so does a folder that never said how many it holds.
        let countless =
            page(br#"{"entries":[{"type":"file","id":"2","name":"b"}]}"#).expect("a page");
        assert_eq!(countless.next_offset(), None);
    }

    #[test]
    fn a_single_item_says_what_it_is() {
        let folder = item(br#"{"type":"folder","id":"1","name":"Season 1"}"#).expect("an item");
        assert!(folder.is_folder());
        assert_eq!(folder.name.as_deref(), Some("Season 1"));
        let file = item(br#"{"type":"file","id":"2","name":"a.bin","size":5}"#).expect("an item");
        assert!(file.is_file());
    }

    /// An id that could not be put back into a request path is dropped rather than trusted.
    #[test]
    fn an_identifier_that_is_not_one_is_dropped() {
        let odd = item(br#"{"type":"file","id":"../../etc","name":"a"}"#).expect("an item");
        assert_eq!(odd.entry(), None);
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_folder() {
        assert!(page(b"<html>502 Bad Gateway</html>").is_none());
        assert!(item(b"").is_none());
    }
}
