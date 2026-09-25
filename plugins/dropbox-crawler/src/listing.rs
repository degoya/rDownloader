//! Turning one entry of a listing into something to walk into or hand back.
//!
//! The reading itself is `dropbox_common::metadata`, shared with the resolver; what is decided
//! here is which rows are dropped. Dropped rather than reported: a single unusable row of a
//! folder is not a reason to refuse the whole folder, and a person who pasted it cannot fix
//! Dropbox's answer. What is dropped is a deleted entry, a row with no name, and a file
//! Dropbox will not serve bytes for — a Paper document has none, and offering it as a download
//! would produce a queue entry that can only ever fail.

use dropbox_common::{address::valid_name, metadata::Metadata};

/// One entry of a listing: either something to walk into, or a file to hand back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Folder { name: String },
    File { name: String, size: Option<u64> },
}

/// Turns one row into an entry, or drops it.
#[must_use]
pub fn entry(item: &Metadata) -> Option<Entry> {
    let name = item.name();
    if !valid_name(name) {
        return None;
    }
    if item.is_folder() {
        return Some(Entry::Folder {
            name: name.to_owned(),
        });
    }
    if !item.is_file() || !item.downloadable() {
        return None;
    }
    Some(Entry::File {
        name: name.to_owned(),
        size: item.size,
    })
}

#[cfg(test)]
mod tests {
    use dropbox_common::metadata::listing;

    use super::{Entry, entry};

    const LISTING: &[u8] = br#"{
      "entries": [
        {".tag": "folder", "name": "Extras", "id": "id:f1"},
        {".tag": "file", "name": "e01.mkv", "size": 1024, "id": "id:x1", "is_downloadable": true},
        {".tag": "file", "name": "Notes", "id": "id:x2", "is_downloadable": false},
        {".tag": "deleted", "name": "old.mkv"},
        {".tag": "file", "name": "..", "id": "id:x3"},
        {".tag": "file", "id": "id:x4"}
      ],
      "cursor": "AAE_redacted",
      "has_more": false
    }"#;

    #[test]
    fn a_page_yields_its_folders_and_its_files_and_drops_the_rest() {
        let page = listing(LISTING).expect("a page");
        let entries: Vec<Entry> = page.entries.iter().filter_map(entry).collect();
        assert_eq!(
            entries,
            vec![
                Entry::Folder {
                    name: "Extras".to_owned(),
                },
                Entry::File {
                    name: "e01.mkv".to_owned(),
                    size: Some(1024),
                },
            ],
            "the Paper document, the deleted entry, the dot entry and the nameless row are dropped, not guessed"
        );
    }
}
