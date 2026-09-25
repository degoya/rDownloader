//! Deciding whether an address is a MediaFire *folder*.
//!
//! The mirror image of the sibling resolver's decision, and the reason both are narrow:
//! `claims-url` is asked of every link a person pastes, and a crawler that claimed a file
//! address would fetch a listing for every download in the queue. The line is the path:
//! `/folder/<key>` is a folder, `/file/<key>` and its siblings are the resolver's. A bare
//! `/?<key>` is claimed here because only the service can tell, and disclaimed again — as
//! `unsupported` — when it turns out to be a file.

use mediafire_common::address::{self, Address};

/// A folder this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A folder, by its key.
    Folder { key: String },
    /// A bare key: a folder or a file, and `folder/get_info` decides which.
    Undecided { key: String },
    /// Several file keys in one address; listed through `file/get_info`.
    Keys(Vec<String>),
}

/// Reads an address, returning what this plugin would list — or `None`, which is what
/// `claims-url` answers for everything that is not a MediaFire folder address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    match address::parse(url)? {
        Address::Folder { key } => Some(Target::Folder { key }),
        Address::Bare { key } => Some(Target::Undecided { key }),
        Address::Keys(keys) => Some(Target::Keys(keys)),
        // A file — the resolver's, whichever way it is spelled.
        Address::File { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};

    #[test]
    fn every_spelling_of_a_folder_address_is_claimed() {
        for url in [
            "https://www.mediafire.com/folder/rww7bhhi0yc1l",
            "https://www.mediafire.com/folder/rww7bhhi0yc1l/Droidfeats",
            "https://www.mediafire.com/folder/rww7bhhi0yc1l/shared",
            "https://mediafire.com/folder/rww7bhhi0yc1l",
            "https://app.mediafire.com/folder/rww7bhhi0yc1l",
        ] {
            assert_eq!(
                claim(url),
                Some(Target::Folder {
                    key: "rww7bhhi0yc1l".to_owned()
                }),
                "{url}"
            );
        }
        assert_eq!(
            claim("https://www.mediafire.com/?rww7bhhi0yc1l"),
            Some(Target::Undecided {
                key: "rww7bhhi0yc1l".to_owned()
            })
        );
        assert_eq!(
            claim("https://mfi.re/?ipnyzofjcwri357"),
            Some(Target::Undecided {
                key: "ipnyzofjcwri357".to_owned()
            })
        );
        assert_eq!(
            claim("https://www.mediafire.com/?ipnyzofjcwri357,8ipst0t9u6sibpx"),
            Some(Target::Keys(vec![
                "ipnyzofjcwri357".to_owned(),
                "8ipst0t9u6sibpx".to_owned()
            ]))
        );
    }

    /// A file address belongs to the sibling resolver. Claiming it here would mean a listing
    /// request for every MediaFire download in the queue.
    #[test]
    fn a_file_address_is_left_to_the_resolver() {
        for url in [
            "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file",
            "https://www.mediafire.com/file/ipnyzofjcwri357",
            "https://www.mediafire.com/download/ipnyzofjcwri357",
            "https://www.mediafire.com/view/ipnyzofjcwri357",
            "https://www.mediafire.com/download.php?ipnyzofjcwri357",
        ] {
            assert_eq!(claim(url), None, "{url}");
        }
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        for url in [
            "https://mediafire.com.evil.test/folder/rww7bhhi0yc1l",
            "https://x@www.mediafire.com/folder/rww7bhhi0yc1l",
            "https://www.mediafire.com/",
            "https://www.mediafire.com/folder/abc",
            "https://www.dropbox.com/sh/abc/h1",
            "https://download1514.mediafire.com/token/rww7bhhi0yc1l/x",
        ] {
            assert_eq!(claim(url), None, "{url}");
        }
    }
}
