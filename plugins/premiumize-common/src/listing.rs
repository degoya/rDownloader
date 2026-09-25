//! Reading what `folder/list` and `item/details` answered.
//!
//! Kept apart from the component so it runs on the host target: `cargo test` covers it
//! without a WebAssembly toolchain, which is the only place a listing's odd shapes — a size
//! quoted as a string, an entry with no link, a folder with no id — can be pinned down.

use serde::Deserialize;

/// One entry of a listing: either something to walk into, or a file to hand back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Folder {
        id: String,
        name: String,
    },
    File {
        name: String,
        url: String,
        size: Option<u64>,
    },
}

/// `GET /api/folder/list?id=<id>`.
#[derive(Debug, Deserialize)]
pub struct FolderListing {
    pub status: String,
    /// The folder's own name, which becomes the package suggestion for the crawled address.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub content: Vec<Item>,
    #[serde(default)]
    pub message: Option<String>,
}

/// One row of a listing, and the whole of an `item/details` answer.
#[derive(Debug, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub size: Option<Flexible>,
    #[serde(default)]
    pub link: Option<String>,
}

/// `GET /api/item/details?id=<id>`: the item's fields at the top level.
#[derive(Debug, Deserialize)]
pub struct ItemDetails {
    pub status: String,
    #[serde(flatten)]
    pub item: Item,
    #[serde(default)]
    pub message: Option<String>,
}

/// Premiumize reports a size as a number, a float or a quoted string depending on endpoint.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Flexible {
    Number(u64),
    Float(f64),
    Text(String),
}

impl Flexible {
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Self::Float(value) if *value >= 0.0 => Some(*value as u64),
            Self::Float(_) => None,
            Self::Text(value) => value.parse().ok(),
        }
    }
}

impl Item {
    /// Turns one row into an entry, or drops it.
    ///
    /// Dropped rather than reported: a single unusable row of a folder is not a reason to
    /// refuse the whole folder, and a person who pasted it cannot fix the provider's answer.
    #[must_use]
    pub fn entry(self) -> Option<Entry> {
        let name = self.name.unwrap_or_default();
        match self.kind.as_deref() {
            Some("folder") => {
                let id = self.id.filter(|id| !id.is_empty())?;
                Some(Entry::Folder { id, name })
            }
            Some("file") => {
                let url = self.link.filter(|link| !link.is_empty())?;
                Some(Entry::File {
                    name,
                    url,
                    size: self.size.as_ref().and_then(Flexible::as_u64),
                })
            }
            _ => None,
        }
    }
}

/// Reads a folder listing, or `None` when the document is not one.
#[must_use]
pub fn folder(body: &[u8]) -> Option<FolderListing> {
    serde_json::from_slice(body).ok()
}

/// Reads an item's details, or `None` when the document is not one.
#[must_use]
pub fn item(body: &[u8]) -> Option<ItemDetails> {
    serde_json::from_slice(body).ok()
}

#[cfg(test)]
mod tests {
    use super::{Entry, folder, item};

    const LISTING: &[u8] = br#"{
      "status": "success",
      "name": "Season 1",
      "content": [
        {"id": "f1", "name": "Extras", "type": "folder"},
        {"id": "x1", "name": "e01.mkv", "type": "file", "size": "1024",
         "link": "https://8.premiumize.me/dl/e01.mkv"},
        {"id": "x2", "name": "e02.mkv", "type": "file", "size": 2048.0,
         "link": "https://8.premiumize.me/dl/e02.mkv", "stream_link": "https://x/s"},
        {"id": "x3", "name": "broken.mkv", "type": "file"},
        {"name": "nameless folder", "type": "folder"}
      ]
    }"#;

    #[test]
    fn a_listing_yields_its_folders_and_its_files() {
        let listing = folder(LISTING).expect("a listing");
        assert_eq!(listing.status, "success");
        assert_eq!(listing.name.as_deref(), Some("Season 1"));
        let entries: Vec<Entry> = listing
            .content
            .into_iter()
            .filter_map(super::Item::entry)
            .collect();
        assert_eq!(
            entries,
            vec![
                Entry::Folder {
                    id: "f1".to_owned(),
                    name: "Extras".to_owned()
                },
                Entry::File {
                    name: "e01.mkv".to_owned(),
                    url: "https://8.premiumize.me/dl/e01.mkv".to_owned(),
                    size: Some(1024),
                },
                Entry::File {
                    name: "e02.mkv".to_owned(),
                    url: "https://8.premiumize.me/dl/e02.mkv".to_owned(),
                    size: Some(2048),
                },
            ],
            "the file without a link and the folder without an id are dropped, not guessed"
        );
    }

    #[test]
    fn an_error_answer_is_read_as_one_rather_than_as_an_empty_folder() {
        let listing =
            folder(br#"{"status":"error","message":"folder not found"}"#).expect("an answer");
        assert_eq!(listing.status, "error");
        assert!(listing.content.is_empty());
        assert_eq!(listing.message.as_deref(), Some("folder not found"));
    }

    #[test]
    fn item_details_carry_the_item_at_the_top_level() {
        let details = item(
            br#"{"status":"success","id":"x1","name":"a.mkv","type":"file","size":7,
                 "link":"https://8.premiumize.me/dl/a.mkv"}"#,
        )
        .expect("details");
        assert_eq!(details.status, "success");
        assert_eq!(
            details.item.entry(),
            Some(Entry::File {
                name: "a.mkv".to_owned(),
                url: "https://8.premiumize.me/dl/a.mkv".to_owned(),
                size: Some(7),
            })
        );
    }

    #[test]
    fn something_that_is_not_the_expected_json_is_not_read_as_an_empty_folder() {
        assert!(folder(b"<html>502</html>").is_none());
        assert!(item(b"").is_none());
    }
}
