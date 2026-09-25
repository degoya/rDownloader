//! Deciding whether an address is a Dropbox *folder*.
//!
//! The mirror image of the sibling resolver's decision, and the reason both are narrow:
//! `claims-url` is asked of every link a person pastes, and a crawler that claimed a file
//! address would fetch a listing for every download in the queue. The line is `preview`: a
//! shared folder link or an own folder without one is a folder, with one it is a file.

use dropbox_common::address::{self, Address};

/// A folder this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A folder in the account's own Dropbox, by its API path — empty for the root.
    Own { path: String },
    /// A shared folder link, and the sub-folder below its root the address points into.
    Shared {
        link: String,
        sub_path: String,
        password: Option<String>,
    },
}

/// Reads an address, returning the folder this plugin would list — or `None`, which is what
/// `claims-url` answers for everything that is not a Dropbox folder address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    match address::parse(url)? {
        Address::SharedFolder {
            link,
            sub_path,
            preview: None,
            password,
        } => Some(Target::Shared {
            link,
            sub_path,
            password,
        }),
        Address::Own {
            path,
            preview: None,
        } => Some(Target::Own { path }),
        // A file — the resolver's, whichever way it is spelled.
        Address::SharedFile { .. } | Address::SharedFolder { .. } | Address::Own { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};

    #[test]
    fn every_spelling_of_a_folder_address_is_claimed() {
        assert_eq!(
            claim("https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1&dl=0"),
            Some(Target::Shared {
                link: "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1".to_owned(),
                sub_path: String::new(),
                password: None,
            })
        );
        assert_eq!(
            claim("https://www.dropbox.com/sh/abc/h1/Season%201?dl=0&link_password=pw"),
            Some(Target::Shared {
                link: "https://www.dropbox.com/sh/abc/h1".to_owned(),
                sub_path: "/Season 1".to_owned(),
                password: Some("pw".to_owned()),
            })
        );
        assert_eq!(
            claim("https://www.dropbox.com/home/Show/Season%201"),
            Some(Target::Own {
                path: "/Show/Season 1".to_owned()
            })
        );
        assert_eq!(
            claim("https://www.dropbox.com/home"),
            Some(Target::Own {
                path: String::new()
            })
        );
    }

    /// A file address belongs to the sibling resolver. Claiming it here would mean a listing
    /// request for every Dropbox download in the queue.
    #[test]
    fn a_file_address_is_left_to_the_resolver() {
        assert_eq!(claim("https://www.dropbox.com/s/abc/release.bin"), None);
        assert_eq!(
            claim("https://www.dropbox.com/scl/fi/abc/release.bin?rlkey=k1"),
            None
        );
        assert_eq!(
            claim("https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1&preview=e01.mkv"),
            None
        );
        assert_eq!(
            claim("https://www.dropbox.com/home/Show?preview=e01.mkv"),
            None
        );
        assert_eq!(claim("https://www.dropbox.com/preview/Show/e01.mkv"), None);
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(claim("https://dropbox.com.evil.test/sh/abc/h1"), None);
        assert_eq!(claim("https://x@www.dropbox.com/sh/abc/h1"), None);
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://www.dropbox.com/"), None);
        assert_eq!(claim("https://www.dropbox.com/sh/abc/h1/../x"), None);
        assert_eq!(claim("https://www.dropbox.com/scl/fo/abc/h1"), None);
    }
}
