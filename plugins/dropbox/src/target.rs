//! Deciding whether an address is a Dropbox *file*, and which one.
//!
//! Answered from the address alone, because `match-url` is asked of every link a person pastes
//! and must reach nothing. What it must not do is over-claim: a folder address belongs to the
//! sibling crawler, and a resolver that swallowed it would turn a folder of two hundred files
//! into one refusal. The line between the two is `preview`: a shared folder link or an own
//! folder without one is a folder; with one it is the file inside it that Dropbox's web
//! interface would show.

use dropbox_common::address::{self, Address};

/// A Dropbox file this plugin claims, reduced to what the API needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A file in the account's own Dropbox, by its API path.
    Own { path: String },
    /// A file behind a shared link — the link itself, or a file inside a shared folder named
    /// by its path below the link's root.
    Shared {
        link: String,
        path: Option<String>,
        password: Option<String>,
    },
}

/// Reads an address, returning the file this plugin would fetch — or `None`, which is what
/// `match-url` answers for everything that is not a Dropbox *file* address.
#[must_use]
pub fn claim(url: &str) -> Option<Target> {
    match address::parse(url)? {
        Address::SharedFile { link, password } => Some(Target::Shared {
            link,
            path: None,
            password,
        }),
        Address::SharedFolder {
            link,
            sub_path,
            preview: Some(name),
            password,
        } => Some(Target::Shared {
            link,
            path: Some(address::shared_file_path(&sub_path, &name)),
            password,
        }),
        Address::Own {
            path,
            preview: Some(name),
        } => Some(Target::Own {
            path: format!("{path}/{name}"),
        }),
        // A folder — the crawler's, whichever way it is spelled.
        Address::SharedFolder { preview: None, .. } | Address::Own { preview: None, .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Target, claim};

    #[test]
    fn every_spelling_of_a_file_address_is_claimed() {
        assert_eq!(
            claim("https://www.dropbox.com/s/abc123/release.bin?dl=0"),
            Some(Target::Shared {
                link: "https://www.dropbox.com/s/abc123/release.bin".to_owned(),
                path: None,
                password: None,
            })
        );
        assert_eq!(
            claim("https://www.dropbox.com/scl/fi/abc123/release.bin?rlkey=k1&dl=0"),
            Some(Target::Shared {
                link: "https://www.dropbox.com/scl/fi/abc123/release.bin?rlkey=k1".to_owned(),
                path: None,
                password: None,
            })
        );
        // A file inside a shared folder: the folder link plus the path below it.
        assert_eq!(
            claim(
                "https://www.dropbox.com/scl/fo/abc/h1/Season%201?rlkey=k1&preview=e01.mkv&link_password=pw"
            ),
            Some(Target::Shared {
                link: "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1".to_owned(),
                path: Some("/Season 1/e01.mkv".to_owned()),
                password: Some("pw".to_owned()),
            })
        );
        // The account's own file, both ways Dropbox spells it — and the canonical address the
        // sibling crawler hands back.
        assert_eq!(
            claim("https://www.dropbox.com/home/Show?preview=e01.mkv"),
            Some(Target::Own {
                path: "/Show/e01.mkv".to_owned()
            })
        );
        assert_eq!(
            claim("https://www.dropbox.com/preview/Show/e01.mkv"),
            Some(Target::Own {
                path: "/Show/e01.mkv".to_owned()
            })
        );
        assert_eq!(
            claim(&dropbox_common::address::file_address("", "readme.txt")),
            Some(Target::Own {
                path: "/readme.txt".to_owned()
            })
        );
    }

    /// A folder belongs to the sibling crawler. This is the over-claim that would turn a folder
    /// of two hundred files into one refusal.
    #[test]
    fn a_folder_address_is_left_to_the_crawler() {
        assert_eq!(
            claim("https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1&dl=0"),
            None
        );
        assert_eq!(claim("https://www.dropbox.com/sh/abc/h1/Season%201"), None);
        assert_eq!(claim("https://www.dropbox.com/home/Show"), None);
        assert_eq!(claim("https://www.dropbox.com/home"), None);
    }

    #[test]
    fn an_address_belonging_to_somebody_else_is_never_claimed() {
        assert_eq!(claim("https://dropbox.com.evil.test/s/abc/x.bin"), None);
        assert_eq!(claim("https://x@www.dropbox.com/s/abc/x.bin"), None);
        assert_eq!(claim("ftp://www.dropbox.com/s/abc/x.bin"), None);
        assert_eq!(claim("https://ddownload.com/f/abc"), None);
        assert_eq!(claim("https://www.dropbox.com/"), None);
        assert_eq!(claim("https://api.dropboxapi.com/2/files/download"), None);
    }

    #[test]
    fn an_identifier_that_is_not_one_is_refused() {
        assert_eq!(claim("https://www.dropbox.com/s/../etc/passwd"), None);
        assert_eq!(claim("https://www.dropbox.com/s/a%2Fb/x.bin"), None);
        assert_eq!(claim("https://www.dropbox.com/home/Show?preview=.."), None);
        assert_eq!(claim("https://www.dropbox.com/home/Show?preview="), None);
    }
}
