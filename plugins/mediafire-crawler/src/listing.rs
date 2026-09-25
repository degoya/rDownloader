//! Turning one entry of a folder listing into something to walk into or hand back.
//!
//! The reading itself is `mediafire_common::api`, shared with the resolver; what is decided
//! here is which rows are dropped. Dropped rather than reported: a single unusable row of a
//! folder is not a reason to refuse the whole folder, and a person who pasted it cannot fix
//! MediaFire's answer. What is dropped is a row with no usable name, a private file, and a
//! password-protected file — the sibling resolver answers no file passwords yet, so offering
//! one would produce a queue entry that can only ever fail.

use mediafire_common::api::{FileInfo, FolderInfo};

/// One entry of a listing: either something to walk into, or a file to hand back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Entry {
    Folder {
        key: String,
        name: String,
    },
    File {
        key: String,
        name: String,
        size: Option<u64>,
    },
}

/// Turns one file row into an entry, or drops it.
#[must_use]
pub fn file_entry(info: &FileInfo) -> Option<Entry> {
    if !valid_name(&info.name) || info.private || info.password_protected {
        return None;
    }
    Some(Entry::File {
        key: info.key.clone(),
        name: info.name.clone(),
        size: info.size,
    })
}

/// Turns one folder row into an entry, or drops it.
#[must_use]
pub fn folder_entry(info: &FolderInfo) -> Option<Entry> {
    if !valid_name(&info.name) || info.private {
        return None;
    }
    Some(Entry::Folder {
        key: info.key.clone(),
        name: info.name.clone(),
    })
}

/// A name that can stand on its own: not empty, not a dot entry, no control character.
/// Separators are not refused here — [`crate::walk::join`] replaces them, and a name that
/// carried one is still the file's name.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use mediafire_common::api::{FileInfo, FolderInfo};

    use super::{Entry, file_entry, folder_entry, valid_name};

    fn file(name: &str) -> FileInfo {
        FileInfo {
            key: "8ipst0t9u6sibpx".to_owned(),
            name: name.to_owned(),
            size: Some(3_117_448),
            hash: None,
            private: false,
            password_protected: false,
            ready: true,
        }
    }

    #[test]
    fn a_file_keeps_its_name_and_size() {
        assert_eq!(
            file_entry(&file("Galaxy-s9-wallpaper-1.png")),
            Some(Entry::File {
                key: "8ipst0t9u6sibpx".to_owned(),
                name: "Galaxy-s9-wallpaper-1.png".to_owned(),
                size: Some(3_117_448),
            })
        );
    }

    #[test]
    fn what_cannot_be_downloaded_is_dropped() {
        assert_eq!(file_entry(&file("")), None);
        assert_eq!(file_entry(&file("..")), None);
        assert_eq!(file_entry(&file("a\u{0}b")), None);
        let mut locked = file("secret.zip");
        locked.password_protected = true;
        assert_eq!(file_entry(&locked), None);
        let mut private = file("mine.zip");
        private.private = true;
        assert_eq!(file_entry(&private), None);
        assert!(valid_name("Season 1"));
        assert!(
            valid_name("a/b"),
            "a separator is replaced later, not refused"
        );
    }

    #[test]
    fn a_folder_row_becomes_something_to_walk_into() {
        let info = FolderInfo {
            key: "gtrp6u25m6nmb".to_owned(),
            name: "TestFolder".to_owned(),
            private: false,
            file_count: Some(2),
            folder_count: Some(1),
        };
        assert_eq!(
            folder_entry(&info),
            Some(Entry::Folder {
                key: "gtrp6u25m6nmb".to_owned(),
                name: "TestFolder".to_owned()
            })
        );
        let private = FolderInfo {
            private: true,
            ..info
        };
        assert_eq!(folder_entry(&private), None);
    }
}
