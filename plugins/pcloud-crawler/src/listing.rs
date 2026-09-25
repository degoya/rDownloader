//! Turning one entry of a listing into something to walk into or hand back.
//!
//! The reading itself is `pcloud_common::metadata`, shared with the resolver; what is decided
//! here is which rows are dropped. Dropped rather than reported: a single unusable row of a
//! folder is not a reason to refuse the whole folder, and a person who pasted it cannot fix
//! pCloud's answer. What is dropped is a row with no usable name, and a row missing the
//! identifier its kind is asked for by — a file with no `fileid` cannot be fetched and a
//! folder with no `folderid` cannot be listed, so offering either would produce a queue entry
//! that can only ever fail.

use pcloud_common::metadata::Metadata;

/// One entry of a listing: either something to walk into, or a file to hand back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Folder {
        folder_id: u64,
        name: String,
    },
    File {
        file_id: u64,
        name: String,
        size: Option<u64>,
    },
}

/// Turns one row into an entry, or drops it.
#[must_use]
pub fn entry(item: &Metadata) -> Option<Entry> {
    if item.is_folder() {
        return Some(Entry::Folder {
            folder_id: item.folderid?,
            name: item.name().to_owned(),
        });
    }
    if item.is_file() {
        return Some(Entry::File {
            file_id: item.fileid?,
            name: item.name().to_owned(),
            size: item.size,
        });
    }
    None
}

/// Every entry of one answer, in the order pCloud gave them.
#[must_use]
pub fn entries(folder: &Metadata) -> Vec<Entry> {
    folder.contents.iter().filter_map(entry).collect()
}

#[cfg(test)]
mod tests {
    use pcloud_common::metadata::item;

    use super::{Entry, entries};

    const LISTING: &[u8] = br#"{"result":0,"metadata":{
      "name":"Show","isfolder":true,"folderid":42,"contents":[
        {"name":"Extras","isfolder":true,"folderid":43,"parentfolderid":42},
        {"name":"e01.mkv","isfolder":false,"fileid":7,"size":1024,"parentfolderid":42},
        {"name":"..","isfolder":false,"fileid":8,"size":1,"parentfolderid":42},
        {"name":"no id","isfolder":false,"size":1,"parentfolderid":42},
        {"name":"no id either","isfolder":true,"parentfolderid":42},
        {"isfolder":false,"fileid":9,"size":1,"parentfolderid":42}
      ]}}"#;

    #[test]
    fn a_listing_yields_its_folders_and_its_files_and_drops_the_rest() {
        let folder = item(LISTING).expect("a folder");
        assert_eq!(
            entries(&folder),
            vec![
                Entry::Folder {
                    folder_id: 43,
                    name: "Extras".to_owned(),
                },
                Entry::File {
                    file_id: 7,
                    name: "e01.mkv".to_owned(),
                    size: Some(1024),
                },
            ],
            "the dot entry, the rows without an identifier and the nameless row are dropped, \
             not guessed"
        );
    }
}
